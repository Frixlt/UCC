// SPDX-License-Identifier: GPL-3.0-or-later

//! CPU discovery, frequency ordering, AMD boost, cached topology, and drift recovery (`ISSUE-07`, `ISSUE-19`, `ISSUE-28`).

use super::{CpuApplyResult, CpuDefaultsSnapshot, LiveUccdBackend, PerCpuSnapshot};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tracing::warn;
use ucc_core::UccProfile;
use ucc_hardware::{parse_cpu_range_list, CpuTopology, Sysfs, MAX_REAPPLY_ATTEMPTS};

impl LiveUccdBackend {
    pub(super) fn read_whitespace_tokens(&self, path: &Path) -> Vec<String> {
        self.sysfs
            .read(path)
            .ok()
            .map(|raw| raw.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub(super) fn read_u32_sysfs(&self, path: &Path) -> Option<u32> {
        self.sysfs
            .read(path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    }

    pub(super) fn discover_all_cpu_dirs_for(
        sysfs: &dyn Sysfs,
        sys_root: &Path,
    ) -> Vec<(u32, PathBuf)> {
        let cpu_dir = sys_root.join("sys/devices/system/cpu");
        let Ok(entries) = sysfs.read_dir(&cpu_dir) else {
            return Vec::new();
        };
        let mut cpus: Vec<(u32, PathBuf)> = entries
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name()?.to_str()?;
                let idx_str = name.strip_prefix("cpu")?;
                if idx_str.is_empty() || !idx_str.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let idx = idx_str.parse::<u32>().ok()?;
                Some((idx, entry))
            })
            .collect();
        cpus.sort_by_key(|(idx, _)| *idx);
        cpus
    }

    pub(super) fn discover_all_cpu_dirs(&self) -> Vec<(u32, PathBuf)> {
        Self::discover_all_cpu_dirs_for(self.sysfs.as_ref(), &self.sys_root)
    }

    pub(super) fn discover_cpu_dirs(&self) -> Vec<PathBuf> {
        self.discover_all_cpu_dirs()
            .into_iter()
            .filter_map(|(_, entry)| {
                let is_online = match self.sysfs.read(&entry.join("online")) {
                    Ok(v) => v.trim() == "1",
                    Err(_) => true,
                };
                is_online.then_some(entry)
            })
            .collect()
    }

    /// Returns present CPU core count (`ISSUE-07`: reads `/sys/devices/system/cpu/present`, not merely online count).
    pub(super) fn read_present_cpu_count(&self) -> i32 {
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        for node in ["present", "possible"] {
            if let Ok(raw) = self.sysfs.read(&cpu_root.join(node)) {
                let parsed = parse_cpu_range_list(&raw);
                if !parsed.is_empty() {
                    return parsed.len() as i32;
                }
            }
        }
        self.discover_all_cpu_dirs().len().max(1) as i32
    }

    /// Refreshes and returns cached CPU topology so P/E core labels survive offlining (`ISSUE-28`).
    pub(super) fn read_cpu_topology_cached_json(&self) -> String {
        if self.sys_root == Path::new("/") {
            let mut topo = self.lock_mutex(&self.cached_real_topology);
            let t = topo.get_or_insert_with(CpuTopology::default);
            let cores = t.read();
            if !cores.is_empty() {
                return serde_json::to_string(&cores).unwrap_or_else(|_| "[]".to_string());
            }
        }
        let all_cpus = self.discover_all_cpu_dirs();
        let mut type_cache = self.lock_mutex(&self.cached_mock_core_types);
        let rows: Vec<Value> = all_cpus
            .into_iter()
            .map(|(id, dir)| {
                let online_path = dir.join("online");
                let hotpluggable = self.sysfs.exists(&online_path);
                let online = if hotpluggable {
                    self.sysfs
                        .read(&online_path)
                        .ok()
                        .is_some_and(|v| v.trim() == "1")
                } else {
                    true
                };
                let physical_core = if online {
                    self.sysfs
                        .read(&dir.join("topology/core_id"))
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(id)
                } else {
                    id
                };
                let siblings = if online {
                    self.sysfs
                        .read(&dir.join("topology/thread_siblings_list"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| id.to_string())
                } else {
                    id.to_string()
                };
                let core_type = if let Some(cached) = type_cache.get(&id) {
                    cached.clone()
                } else {
                    let probed = self
                        .sysfs
                        .read(&dir.join("topology/core_type"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "unknown".to_string());
                    if online && probed != "unknown" {
                        type_cache.insert(id, probed.clone());
                    }
                    probed
                };
                json!({
                    "id": id,
                    "online": online,
                    "hotpluggable": hotpluggable,
                    "type": core_type,
                    "physicalCore": physical_core,
                    "siblings": siblings,
                })
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    pub(super) fn capture_initial_cpu_defaults(&self) {
        let all_cpus = self.discover_all_cpu_dirs();
        if all_cpus.is_empty() {
            return;
        }
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        let mut defaults = self.lock_mutex(&self.initial_cpu_defaults);
        if defaults.is_none() {
            let mut per_cpu = BTreeMap::new();
            for (_, dir) in &all_cpus {
                let cpufreq = dir.join("cpufreq");
                per_cpu.insert(
                    dir.clone(),
                    PerCpuSnapshot {
                        online: self
                            .sysfs
                            .read(&dir.join("online"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        governor: self
                            .sysfs
                            .read(&cpufreq.join("scaling_governor"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        epp: self
                            .sysfs
                            .read(&cpufreq.join("energy_performance_preference"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        min_freq_khz: self
                            .sysfs
                            .read(&cpufreq.join("scaling_min_freq"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        max_freq_khz: self
                            .sysfs
                            .read(&cpufreq.join("scaling_max_freq"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                    },
                );
            }
            *defaults = Some(CpuDefaultsSnapshot {
                per_cpu,
                no_turbo: self
                    .sysfs
                    .read(&cpu_root.join("intel_pstate/no_turbo"))
                    .ok()
                    .map(|s| s.trim().to_string()),
                amd_boost: self
                    .sysfs
                    .read(&cpu_root.join("cpufreq/boost"))
                    .ok()
                    .map(|s| s.trim().to_string()),
                hwp_dynamic_boost: self
                    .sysfs
                    .read(&cpu_root.join("intel_pstate/hwp_dynamic_boost"))
                    .ok()
                    .map(|s| s.trim().to_string()),
            });
        }
    }

    pub(super) fn check_cpu_support_for_dir(
        &self,
        cpu_dir: &Path,
        cpu: &ucc_core::ProfileCpu,
    ) -> (bool, bool, bool, bool) {
        let cpufreq = cpu_dir.join("cpufreq");
        let mut gov_ok =
            !cpu.governor.is_empty() && self.sysfs.exists(&cpufreq.join("scaling_governor"));
        let mut gov_skipped = false;
        if gov_ok {
            let avail_path = cpufreq.join("scaling_available_governors");
            if self.sysfs.exists(&avail_path) {
                let avail = self.read_whitespace_tokens(&avail_path);
                if !avail.iter().any(|g| g == &cpu.governor) {
                    warn!(
                        "Skipping unavailable CPU governor '{}' for {} (available: {:?})",
                        cpu.governor,
                        cpu_dir.display(),
                        avail
                    );
                    gov_ok = false;
                    gov_skipped = true;
                }
            }
        }
        let mut epp_ok = !cpu.energy_performance_preference.is_empty()
            && self
                .sysfs
                .exists(&cpufreq.join("energy_performance_preference"));
        let mut epp_skipped = false;
        if epp_ok {
            let avail_path = cpufreq.join("energy_performance_available_preferences");
            if self.sysfs.exists(&avail_path) {
                let avail = self.read_whitespace_tokens(&avail_path);
                if !avail
                    .iter()
                    .any(|e| e == &cpu.energy_performance_preference)
                {
                    warn!(
                        "Skipping unavailable CPU EPP '{}' for {} (available: {:?})",
                        cpu.energy_performance_preference,
                        cpu_dir.display(),
                        avail
                    );
                    epp_ok = false;
                    epp_skipped = true;
                }
            }
        }
        (gov_ok, gov_skipped, epp_ok, epp_skipped)
    }

    pub(super) fn write_cpu_with_rollback(
        &self,
        path: &Path,
        val: &str,
        rollback: &mut Vec<(PathBuf, String)>,
    ) -> Result<(), String> {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return Err("Shutdown deadline expired".to_string());
        }
        let prev = self
            .sysfs
            .read(path)
            .map_err(|e| format!("Failed reading baseline {}: {e}", path.display()))?;
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return Err("Shutdown deadline expired".to_string());
        }
        self.sysfs
            .write(path, val)
            .map_err(|e| format!("Failed writing {}: {e}", path.display()))?;
        rollback.push((path.to_path_buf(), prev.trim().to_string()));
        Ok(())
    }

    #[must_use]
    pub(super) fn rollback_cpu_writes(&self, writes: Vec<(PathBuf, String)>) -> bool {
        let mut ok = true;
        for (path, prev) in writes.into_iter().rev() {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if let Err(e) = self.sysfs.write(&path, &prev) {
                warn!("Failed rolling back CPU sysfs node {}: {e}", path.display());
                ok = false;
            } else if let Ok(actual) = self.sysfs.read(&path) {
                if actual.trim() != prev.trim() {
                    warn!(
                        "CPU sysfs readback mismatch on {}: expected '{}', got '{}'",
                        path.display(),
                        prev.trim(),
                        actual.trim()
                    );
                    ok = false;
                }
            }
        }
        ok
    }

    pub(super) fn apply_cpu_settings(
        &self,
        profile: &UccProfile,
    ) -> Result<CpuApplyResult, String> {
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            return Ok(CpuApplyResult {
                rollback: Vec::new(),
                fully_applied: true,
            });
        }
        let enabled = self
            .read_rw(&self.settings)
            .cpu_settings_enabled
            .unwrap_or(true);
        if !enabled {
            return Ok(CpuApplyResult {
                rollback: Vec::new(),
                fully_applied: true,
            });
        }

        let mode_name = self.current_decision().effective.mode_name();
        let mut overlaid = profile.clone();
        self.power_devices.overlay_cpu(&mut overlaid, mode_name);
        let cpu = &overlaid.cpu;

        let all_cpus = self.discover_all_cpu_dirs();
        if all_cpus.is_empty() {
            return Ok(CpuApplyResult {
                rollback: Vec::new(),
                fully_applied: true,
            });
        }

        let all_ids: HashSet<u32> = all_cpus.iter().map(|(id, _)| *id).collect();
        let desired_online: HashSet<u32> = if let Some(ref sel) = cpu.enabled_cores {
            if sel.is_empty() || !sel.contains(&0) {
                return Err("enabled_cores must be non-empty and include core 0".to_string());
            }
            for id in sel {
                if !all_ids.contains(id) {
                    return Err(format!("Requested CPU core {id} does not exist"));
                }
            }
            sel.iter().copied().collect()
        } else if let Some(count) = cpu.online_cores {
            if count == 0 || (count as usize) > all_cpus.len() {
                return Err(format!(
                    "online_cores ({count}) outside detected core count ({})",
                    all_cpus.len()
                ));
            }
            all_cpus
                .iter()
                .take(count as usize)
                .map(|(id, _)| *id)
                .collect()
        } else {
            all_ids
        };

        self.capture_initial_cpu_defaults();
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        let mut rollback: Vec<(PathBuf, String)> = Vec::new();
        let mut fully_applied = true;
        let res = (|| -> Result<(), String> {
            // 1. Bring desired cores online first so cpufreq nodes are accessible.
            for (id, dir) in &all_cpus {
                if *id > 0 && desired_online.contains(id) {
                    let online_path = dir.join("online");
                    if self.sysfs.read(&online_path).is_ok_and(|v| v.trim() != "1") {
                        self.write_cpu_with_rollback(&online_path, "1", &mut rollback)?;
                    }
                }
            }

            // 2. Apply turbo / boost / HWP flags (Intel + AMD).
            let no_turbo_path = cpu_root.join("intel_pstate/no_turbo");
            if self.sysfs.exists(&no_turbo_path) {
                self.write_cpu_with_rollback(
                    &no_turbo_path,
                    if cpu.no_turbo { "1" } else { "0" },
                    &mut rollback,
                )?;
            }
            let amd_boost_path = cpu_root.join("cpufreq/boost");
            if self.sysfs.exists(&amd_boost_path) {
                self.write_cpu_with_rollback(
                    &amd_boost_path,
                    if cpu.no_turbo { "0" } else { "1" },
                    &mut rollback,
                )?;
            }
            let hwp_path = cpu_root.join("intel_pstate/hwp_dynamic_boost");
            if self.sysfs.exists(&hwp_path) {
                self.write_cpu_with_rollback(
                    &hwp_path,
                    if cpu.hwp_dynamic_boost { "1" } else { "0" },
                    &mut rollback,
                )?;
            }

            // 3. Apply per-CPU governor, EPP (tolerating EBUSY), and ordered/reset scaling frequencies.
            for (id, dir) in &all_cpus {
                if !desired_online.contains(id) {
                    continue;
                }
                let (apply_gov, skipped_gov, apply_epp, skipped_epp) =
                    self.check_cpu_support_for_dir(dir, cpu);
                if skipped_gov || skipped_epp {
                    fully_applied = false;
                }
                let cpufreq = dir.join("cpufreq");
                if apply_gov {
                    self.write_cpu_with_rollback(
                        &cpufreq.join("scaling_governor"),
                        &cpu.governor,
                        &mut rollback,
                    )?;
                }
                if apply_epp {
                    let epp_path = cpufreq.join("energy_performance_preference");
                    match self.sysfs.read(&epp_path) {
                        Ok(prev_epp) => {
                            if let Err(err) = self
                                .sysfs
                                .write(&epp_path, &cpu.energy_performance_preference)
                            {
                                if err.raw_os_error() == Some(nix::libc::EBUSY) {
                                    warn!(
                                        "Ignoring EBUSY while writing EPP '{}' on {}; governor likely owns EPP",
                                        cpu.energy_performance_preference,
                                        dir.display()
                                    );
                                    fully_applied = false;
                                } else {
                                    return Err(format!(
                                        "Failed writing {}: {err}",
                                        epp_path.display()
                                    ));
                                }
                            } else {
                                rollback.push((epp_path, prev_epp.trim().to_string()));
                            }
                        }
                        Err(e) => {
                            return Err(format!(
                                "Failed reading baseline {}: {e}",
                                epp_path.display()
                            ));
                        }
                    }
                }

                // Frequency ordering & full reset to cpuinfo bounds when None (`ISSUE-07`).
                let hw_min = self.read_u32_sysfs(&cpufreq.join("cpuinfo_min_freq"));
                let hw_max = self.read_u32_sysfs(&cpufreq.join("cpuinfo_max_freq"));
                let min_path = cpufreq.join("scaling_min_freq");
                let max_path = cpufreq.join("scaling_max_freq");
                let has_min = self.sysfs.exists(&min_path);
                let has_max = self.sysfs.exists(&max_path);
                let cur_min = self.read_u32_sysfs(&min_path);
                let cur_max = self.read_u32_sysfs(&max_path);

                let clamp_hw = |val: u32| -> u32 {
                    let lo = hw_min.unwrap_or(val);
                    let hi = hw_max.unwrap_or(val).max(lo);
                    val.clamp(lo, hi)
                };

                let target_min = cpu.min_freq_khz.map(clamp_hw).or(hw_min);
                let target_max = cpu.max_freq_khz.map(clamp_hw).or(hw_max);

                let min_first = match (target_min, target_max, cur_min, cur_max) {
                    (_, Some(t_max), Some(c_min), _) if t_max < c_min => true,
                    (Some(t_min), _, _, Some(c_max)) if t_min > c_max => false,
                    _ => false,
                };

                if min_first {
                    if let (true, Some(v)) = (has_min, target_min) {
                        self.write_cpu_with_rollback(&min_path, &v.to_string(), &mut rollback)?;
                    }
                    if let (true, Some(v)) = (has_max, target_max) {
                        self.write_cpu_with_rollback(&max_path, &v.to_string(), &mut rollback)?;
                    }
                } else {
                    if let (true, Some(v)) = (has_max, target_max) {
                        self.write_cpu_with_rollback(&max_path, &v.to_string(), &mut rollback)?;
                    }
                    if let (true, Some(v)) = (has_min, target_min) {
                        self.write_cpu_with_rollback(&min_path, &v.to_string(), &mut rollback)?;
                    }
                }
            }

            // 4. Offline non-selected cores (`id > 0`).
            for (id, dir) in &all_cpus {
                if *id > 0 && !desired_online.contains(id) {
                    let online_path = dir.join("online");
                    if self.sysfs.exists(&online_path) {
                        self.write_cpu_with_rollback(&online_path, "0", &mut rollback)?;
                    }
                }
            }

            Ok(())
        })();

        match res {
            Ok(()) => {
                self.cpu_reapply_attempts.store(0, Ordering::SeqCst);
                Ok(CpuApplyResult {
                    rollback,
                    fully_applied,
                })
            }
            Err(err) => {
                if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                    return Err(err);
                }
                if !self.rollback_cpu_writes(rollback) {
                    if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                        return Err(err);
                    }
                    self.sync_active_profile_from_hw_readback();
                    *self.lock_mutex(&self.last_applied_profile_state) = None;
                    *self.lock_mutex(&self.cpu_policy_applied_rev) = None;
                    self.set_mode_reapply_pending(true);
                }
                Err(err)
            }
        }
    }

    #[must_use]
    pub(super) fn write_sysfs_if_changed(&self, path: &Path, target: &str) -> bool {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if self
            .sysfs
            .read(path)
            .is_ok_and(|cur| cur.trim() == target.trim())
        {
            return true;
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if let Err(err) = self.sysfs.write(path, target) {
            warn!("Failed restoring CPU sysfs node {}: {err}", path.display());
            return false;
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        match self.sysfs.read(path) {
            Ok(cur) if cur.trim() == target.trim() => true,
            result => {
                warn!(
                    "Could not verify restored CPU sysfs node {}: {result:?}",
                    path.display()
                );
                false
            }
        }
    }

    #[must_use]
    pub(super) fn apply_cpu_defaults_snapshot(&self, defaults: &CpuDefaultsSnapshot) -> bool {
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        if let Some(ref nt) = defaults.no_turbo {
            if !self.write_sysfs_if_changed(&cpu_root.join("intel_pstate/no_turbo"), nt) {
                return false;
            }
        }
        if let Some(ref boost) = defaults.amd_boost {
            if !self.write_sysfs_if_changed(&cpu_root.join("cpufreq/boost"), boost) {
                return false;
            }
        }
        if let Some(ref hwp) = defaults.hwp_dynamic_boost {
            if !self.write_sysfs_if_changed(&cpu_root.join("intel_pstate/hwp_dynamic_boost"), hwp) {
                return false;
            }
        }
        for (dir, snap) in &defaults.per_cpu {
            if snap.online.as_deref() == Some("1")
                && !self.write_sysfs_if_changed(&dir.join("online"), "1")
            {
                return false;
            }
            let cpufreq = dir.join("cpufreq");
            if let Some(ref gov) = snap.governor {
                if !self.write_sysfs_if_changed(&cpufreq.join("scaling_governor"), gov) {
                    return false;
                }
            }
            if let Some(ref epp) = snap.epp {
                if !self.write_sysfs_if_changed(&cpufreq.join("energy_performance_preference"), epp)
                {
                    return false;
                }
            }
            if let Some(ref max_khz) = snap.max_freq_khz {
                if !self.write_sysfs_if_changed(&cpufreq.join("scaling_max_freq"), max_khz) {
                    return false;
                }
            }
            if let Some(ref min_khz) = snap.min_freq_khz {
                if !self.write_sysfs_if_changed(&cpufreq.join("scaling_min_freq"), min_khz) {
                    return false;
                }
            }
            if snap.online.as_deref() == Some("0")
                && !self.write_sysfs_if_changed(&dir.join("online"), "0")
            {
                return false;
            }
        }
        true
    }

    #[must_use]
    pub(super) fn restore_cpu_defaults(&self) -> bool {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let Some(defaults) = self.lock_mutex(&self.initial_cpu_defaults).clone() else {
            return true;
        };
        if !self.apply_cpu_defaults_snapshot(&defaults) {
            return false;
        }
        *self.lock_mutex(&self.initial_cpu_defaults) = None;
        true
    }

    pub(crate) fn take_unrestored_cpu_defaults(&self) -> Option<CpuDefaultsSnapshot> {
        self.lock_mutex(&self.initial_cpu_defaults).take()
    }

    pub(crate) fn seed_inherited_cpu_defaults(&self, defaults: Option<CpuDefaultsSnapshot>) {
        let Some(snapshot) = defaults else {
            return;
        };
        {
            let mut slot = self.lock_mutex(&self.initial_cpu_defaults);
            if slot.is_none() {
                *slot = Some(snapshot.clone());
            }
        }
        if !ucc_config::is_read_only_preview()
            && !self.shutting_down.load(Ordering::SeqCst)
            && !self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            let _ = self.apply_cpu_defaults_snapshot(&snapshot);
        }
    }

    /// Periodic 10s CPU drift validator & re-applier bounded by `MAX_REAPPLY_ATTEMPTS` (`ISSUE-07`).
    pub fn validate_and_reapply_cpu_once(&self) {
        if self.shutting_down.load(Ordering::SeqCst)
            || self.is_vendor_unsupported()
            || ucc_config::is_read_only_preview()
            || !self.hardware_activated.load(Ordering::SeqCst)
        {
            return;
        }
        let enabled = self
            .read_rw(&self.settings)
            .cpu_settings_enabled
            .unwrap_or(true);
        if !enabled {
            return;
        }
        if self.cpu_reapply_attempts.load(Ordering::SeqCst) >= MAX_REAPPLY_ATTEMPTS {
            return;
        }
        let active = self.read_rw(&self.active_profile).clone();
        let mode_name = self.current_decision().effective.mode_name();
        let mut overlaid = active.clone();
        self.power_devices.overlay_cpu(&mut overlaid, mode_name);
        let cpu = &overlaid.cpu;

        let all_cpus = self.discover_all_cpu_dirs();
        let mut drifted = false;
        for (id, dir) in &all_cpus {
            let online = if *id == 0 {
                true
            } else {
                self.sysfs
                    .read(&dir.join("online"))
                    .map(|v| v.trim() == "1")
                    .unwrap_or(true)
            };
            if !online {
                continue;
            }
            let cpufreq = dir.join("cpufreq");
            let (gov_ok, _, _, _) = self.check_cpu_support_for_dir(dir, cpu);
            if gov_ok {
                if let Ok(cur_gov) = self.sysfs.read(&cpufreq.join("scaling_governor")) {
                    if cur_gov.trim() != cpu.governor {
                        drifted = true;
                        break;
                    }
                }
            }
            if let Some(min_khz) = cpu.min_freq_khz {
                if let Some(cur) = self.read_u32_sysfs(&cpufreq.join("scaling_min_freq")) {
                    if cur != min_khz {
                        drifted = true;
                        break;
                    }
                }
            }
            if let Some(max_khz) = cpu.max_freq_khz {
                if let Some(cur) = self.read_u32_sysfs(&cpufreq.join("scaling_max_freq")) {
                    if cur != max_khz {
                        drifted = true;
                        break;
                    }
                }
            }
        }

        if drifted {
            let attempt = self.cpu_reapply_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= MAX_REAPPLY_ATTEMPTS {
                let _ = self.apply_cpu_settings(&active);
            }
        }
    }
}
