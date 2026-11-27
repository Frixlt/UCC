// SPDX-License-Identifier: GPL-3.0-or-later

//! Profile transaction engine, rollback, power reconciliation, and custom profile CRUD (`ISSUE-01`, `ISSUE-08`, `ISSUE-13`, `ISSUE-24`, `ISSUE-34`, `ISSUE-35`).

use super::{
    unix_ms, ApplyOutcome, CommittedProfileRecord, DbusSignalEvent, InFlightTxGuard,
    LiveUccdBackend, SupersededUndo,
};
use std::path::Path;
use std::sync::atomic::Ordering;
use tracing::warn;
use ucc_config::{
    default_high_performance_profile, default_max_energy_save_profile, default_office_profile,
    default_quiet_profile,
};
use ucc_core::{ProfileState, UccProfile};
use ucc_hardware::{DeviceVendor, PlatformBackend, PlatformCapabilities, PlatformError, Sysfs};
use ucc_lighting::LightbarPowerMode;

const MAX_CUSTOM_PROFILES: usize = 64;
const MAX_PROFILE_JSON_BYTES: usize = 1024 * 1024;

pub(super) fn is_builtin_profile_id(id: &str) -> bool {
    [
        default_quiet_profile().id,
        default_office_profile().id,
        default_high_performance_profile().id,
        default_max_energy_save_profile().id,
    ]
    .iter()
    .any(|builtin| builtin == id)
}

pub(super) fn is_builtin_profile_name(name: &str) -> bool {
    let trimmed = name.trim();
    ["Max Energy Save", "Quiet", "Office", "High Performance"]
        .iter()
        .any(|builtin| builtin.eq_ignore_ascii_case(trimmed))
}

pub(super) fn fallback_odm_profile_code(vendor: DeviceVendor, name: &str) -> Option<i32> {
    let lower = name.trim().to_ascii_lowercase();
    match vendor {
        DeviceVendor::Clevo => match lower.as_str() {
            "quiet" | "silent" => Some(0),
            "power_saving" | "powersave" => Some(1),
            "performance" | "balanced" => Some(2),
            "entertainment" | "overboost" => Some(3),
            _ => lower.parse::<i32>().ok().filter(|v| (0..=3).contains(v)),
        },
        DeviceVendor::Uniwill => match lower.as_str() {
            "power_save" | "powersave" | "quiet" => Some(1),
            "balanced" | "enthusiast" | "office" => Some(2),
            "overboost" | "performance" | "high_performance" => Some(3),
            _ => lower.parse::<i32>().ok().filter(|v| (1..=3).contains(v)),
        },
        DeviceVendor::Unsupported => None,
    }
}

impl LiveUccdBackend {
    /// Resolves ODM profile name -> integer EC code, clamping 2-profile Uniwill laptops (< 2) (`ISSUE-35`, `t_daemon_18`).
    pub(super) fn resolve_odm_profile_code(&self, name: &str) -> Option<i32> {
        let code = fallback_odm_profile_code(self.capabilities.device.vendor, name)?;
        if self.capabilities.device.vendor == DeviceVendor::Uniwill
            && self.capabilities.perf_profiles_available == 2
        {
            return Some(code.min(1));
        }
        Some(code)
    }

    pub(super) fn odm_profile_name_for_code(
        capabilities: &PlatformCapabilities,
        code: i32,
    ) -> Option<String> {
        let names = match capabilities.device.vendor {
            DeviceVendor::Clevo if capabilities.perf_profiles_available > 0 => vec![
                "quiet".to_string(),
                "power_saving".to_string(),
                "performance".to_string(),
                "entertainment".to_string(),
            ],
            DeviceVendor::Uniwill if capabilities.perf_profiles_available >= 2 => {
                let mut v = vec!["power_save".to_string(), "enthusiast".to_string()];
                if capabilities.perf_profiles_available >= 3 {
                    v.push("overboost".to_string());
                }
                v
            }
            _ => Vec::new(),
        };
        names
            .into_iter()
            .find(|n| fallback_odm_profile_code(capabilities.device.vendor, n) == Some(code))
    }

    pub(super) fn populate_profile_from_hw_readback(
        profile: &mut UccProfile,
        platform: &dyn PlatformBackend,
        sysfs: &dyn Sysfs,
        sys_root: &Path,
        capabilities: &PlatformCapabilities,
        preserve_empty_tdps: bool,
    ) {
        if platform.is_available() && capabilities.tdp_channels > 0 {
            if !preserve_empty_tdps || !profile.odm_power_limits.is_empty() {
                let target_len = if preserve_empty_tdps {
                    profile
                        .odm_power_limits
                        .len()
                        .min(capabilities.tdp_channels)
                } else {
                    capabilities.tdp_channels
                };
                let mut read_tdps = Vec::with_capacity(target_len);
                let mut all_ok = true;
                for idx in 0..target_len {
                    match platform.read_tdp(idx) {
                        Ok(w) => match u32::try_from(w) {
                            Ok(u) => read_tdps.push(u),
                            Err(_) => {
                                all_ok = false;
                                break;
                            }
                        },
                        Err(_) => {
                            all_ok = false;
                            break;
                        }
                    }
                }
                if all_ok || !read_tdps.is_empty() {
                    profile.odm_power_limits = read_tdps;
                } else if preserve_empty_tdps {
                    profile.odm_power_limits.clear();
                }
            }
            if let Ok(code) = platform.read_perf_profile() {
                if let Some(name) = Self::odm_profile_name_for_code(capabilities, code) {
                    profile.odm_profile_name = Some(name);
                }
            }
        }
        let cpu_root = sys_root.join("sys/devices/system/cpu");
        if let Ok(nt) = sysfs.read(&cpu_root.join("intel_pstate/no_turbo")) {
            profile.cpu.no_turbo = nt.trim() == "1";
        } else if let Ok(boost) = sysfs.read(&cpu_root.join("cpufreq/boost")) {
            profile.cpu.no_turbo = boost.trim() == "0";
        }
        if let Ok(hwp) = sysfs.read(&cpu_root.join("intel_pstate/hwp_dynamic_boost")) {
            profile.cpu.hwp_dynamic_boost = hwp.trim() == "1";
        }
        if let Some((_, first_cpu)) = Self::discover_all_cpu_dirs_for(sysfs, sys_root)
            .into_iter()
            .next()
        {
            let cpufreq = first_cpu.join("cpufreq");
            if let Ok(gov) = sysfs.read(&cpufreq.join("scaling_governor")) {
                let g = gov.trim();
                if !g.is_empty() {
                    profile.cpu.governor = g.to_string();
                }
            }
            if let Ok(epp) = sysfs.read(&cpufreq.join("energy_performance_preference")) {
                let e = epp.trim();
                if !e.is_empty() {
                    profile.cpu.energy_performance_preference = e.to_string();
                }
            }
        }
    }

    pub(super) fn build_unapplied_hardware_profile(
        platform: &dyn PlatformBackend,
        sysfs: &dyn Sysfs,
        sys_root: &Path,
        capabilities: &PlatformCapabilities,
    ) -> UccProfile {
        let mut profile = UccProfile::new("unapplied", "Hardware Baseline (Unapplied)");
        profile.description =
            "Confirmed hardware readback (configured profile not yet applied)".to_string();
        Self::populate_profile_from_hw_readback(
            &mut profile,
            platform,
            sysfs,
            sys_root,
            capabilities,
            false,
        );
        profile
    }

    pub(super) fn record_in_flight_tdp_baseline(&self, tx_id: u64, idx: usize, prev_w: i32) {
        let mut guard = self.lock_mutex(&self.in_flight_tdp_baselines);
        if !guard.iter().any(|(id, ch, _)| *id == tx_id && *ch == idx) {
            guard.push((tx_id, idx, prev_w));
        }
        self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn record_in_flight_odm_baseline(&self, tx_id: u64, prev_code: i32) {
        let mut guard = self.lock_mutex(&self.in_flight_odm_baselines);
        if !guard.iter().any(|(id, _)| *id == tx_id) {
            guard.push((tx_id, prev_code));
        }
        self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn lookup_other_in_flight_tdp(&self, tx_id: u64, idx: usize) -> Option<i32> {
        let guard = self.lock_mutex(&self.in_flight_tdp_baselines);
        guard
            .iter()
            .filter(|(id, ch, _)| *id != tx_id && *ch == idx)
            .min_by_key(|(id, _, _)| *id)
            .map(|(_, _, w)| *w)
    }

    pub(super) fn snapshot_unmanaged_tdp_baseline(&self, tx_id: u64, idx: usize) -> Option<i32> {
        for _ in 0..4 {
            let seq_before = self.tdp_write_seq.load(Ordering::SeqCst);
            if let Some(w) = self.lookup_other_in_flight_tdp(tx_id, idx) {
                return Some(w);
            }
            let live = self.platform.read_tdp(idx).ok();
            if let Some(w) = self.lookup_other_in_flight_tdp(tx_id, idx) {
                return Some(w);
            }
            if self.tdp_write_seq.load(Ordering::SeqCst) == seq_before {
                return live;
            }
        }
        self.lookup_other_in_flight_tdp(tx_id, idx)
            .or_else(|| self.platform.read_tdp(idx).ok())
    }

    pub(super) fn lookup_other_in_flight_odm(&self, tx_id: u64) -> Option<i32> {
        let guard = self.lock_mutex(&self.in_flight_odm_baselines);
        guard
            .iter()
            .filter(|(id, _)| *id != tx_id)
            .min_by_key(|(id, _)| *id)
            .map(|(_, c)| *c)
    }

    pub(super) fn snapshot_unmanaged_odm_baseline(&self, tx_id: u64) -> Option<i32> {
        for _ in 0..4 {
            let seq_before = self.odm_write_seq.load(Ordering::SeqCst);
            if let Some(code) = self.lookup_other_in_flight_odm(tx_id) {
                return Some(code);
            }
            let live = self.platform.read_perf_profile().ok();
            if let Some(code) = self.lookup_other_in_flight_odm(tx_id) {
                return Some(code);
            }
            if self.odm_write_seq.load(Ordering::SeqCst) == seq_before {
                return live;
            }
        }
        self.lookup_other_in_flight_odm(tx_id)
            .or_else(|| self.platform.read_perf_profile().ok())
    }

    pub(super) fn record_in_flight_ctgp_baseline(&self, tx_id: u64, prev_offset: i32) {
        let mut guard = self.lock_mutex(&self.in_flight_ctgp_baselines);
        if !guard.iter().any(|(id, _)| *id == tx_id) {
            guard.push((tx_id, prev_offset));
        }
        self.ctgp_write_seq.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn lookup_other_in_flight_ctgp(&self, tx_id: u64) -> Option<i32> {
        let guard = self.lock_mutex(&self.in_flight_ctgp_baselines);
        guard
            .iter()
            .filter(|(id, _)| *id != tx_id)
            .min_by_key(|(id, _)| *id)
            .map(|(_, offset)| *offset)
    }

    pub(super) fn snapshot_unmanaged_ctgp_baseline(&self, tx_id: u64) -> Option<i32> {
        for _ in 0..4 {
            let seq_before = self.ctgp_write_seq.load(Ordering::SeqCst);
            if let Some(offset) = self.lookup_other_in_flight_ctgp(tx_id) {
                return Some(offset);
            }
            let live = self.read_ctgp_baseline();
            if let Some(offset) = self.lookup_other_in_flight_ctgp(tx_id) {
                return Some(offset);
            }
            if self.ctgp_write_seq.load(Ordering::SeqCst) == seq_before {
                return live;
            }
        }
        self.lookup_other_in_flight_ctgp(tx_id)
            .or_else(|| self.read_ctgp_baseline())
    }

    pub(super) fn record_in_flight_kb_baseline(
        &self,
        tx_id: u64,
        prev_kb: &super::lighting_power::KeyboardRollbackSnapshot,
    ) {
        let mut guard = self.lock_mutex(&self.in_flight_kb_baselines);
        if !guard.iter().any(|(id, _)| *id == tx_id) {
            guard.push((tx_id, prev_kb.clone()));
        }
        self.kb_write_seq.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn lookup_other_in_flight_kb(
        &self,
        tx_id: u64,
    ) -> Option<super::lighting_power::KeyboardRollbackSnapshot> {
        let guard = self.lock_mutex(&self.in_flight_kb_baselines);
        guard
            .iter()
            .filter(|(id, _)| *id != tx_id)
            .min_by_key(|(id, _)| *id)
            .map(|(_, snap)| snap.clone())
    }

    pub(super) fn snapshot_unmanaged_kb_baseline(
        &self,
        tx_id: u64,
    ) -> Option<super::lighting_power::KeyboardRollbackSnapshot> {
        for _ in 0..4 {
            let seq_before = self.kb_write_seq.load(Ordering::SeqCst);
            if let Some(snap) = self.lookup_other_in_flight_kb(tx_id) {
                return Some(snap);
            }
            let live = self.snapshot_keyboard_rollback();
            if let Some(snap) = self.lookup_other_in_flight_kb(tx_id) {
                return Some(snap);
            }
            if self.kb_write_seq.load(Ordering::SeqCst) == seq_before {
                return live;
            }
        }
        self.lookup_other_in_flight_kb(tx_id)
            .or_else(|| self.snapshot_keyboard_rollback())
    }

    pub(super) fn restore_ctgp_and_kb_writes(
        &self,
        prev_ctgp: Option<i32>,
        wrote_ctgp: bool,
        prev_kb: Option<&super::lighting_power::KeyboardRollbackSnapshot>,
        wrote_kb: bool,
        start_epoch: Option<u64>,
    ) {
        if wrote_kb {
            if let Some(snap) = prev_kb {
                let _ = self.restore_keyboard_rollback(snap, start_epoch);
            }
        }
        if wrote_ctgp {
            let _ = self.restore_ctgp_baseline(prev_ctgp, start_epoch);
        }
    }

    pub(super) fn sync_active_profile_from_hw_readback(&self) {
        let mut active = self.read_rw(&self.active_profile).clone();
        Self::populate_profile_from_hw_readback(
            &mut active,
            self.platform.as_ref(),
            self.sysfs.as_ref(),
            &self.sys_root,
            &self.capabilities,
            true,
        );
        *self.write_rw(&self.active_profile) = active;
    }

    pub(super) fn write_tdp_verified(&self, idx: usize, target_w: i32) -> bool {
        self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
        let res = self.platform.set_tdp(idx, target_w);
        self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
        res.is_ok() && matches!(self.platform.read_tdp(idx), Ok(actual) if actual == target_w)
    }

    pub(super) fn apply_fan_and_device_state_locked(
        &self,
        profile: &UccProfile,
        apply_display_webcam: bool,
    ) {
        if apply_display_webcam {
            if profile.display.use_brightness && self.discover_backlight_device().is_some() {
                let _ = self.set_display_brightness_internal(profile.display.brightness);
            }
            if profile.webcam.use_status && self.webcam_sw_available_impl() {
                let _ = self.set_webcam_internal(profile.webcam.status);
            }
        }

        let fan_enabled = self
            .read_rw(&self.settings)
            .fan_control_enabled
            .unwrap_or(profile.fan.use_control)
            && profile.fan.use_control;
        self.fan_control_enabled
            .store(fan_enabled, Ordering::SeqCst);
        {
            *self.lock_mutex(&self.temporary_fan_curves) = None;
            *self.lock_mutex(&self.temporary_saved_fan_profile_id) = None;
            let mut fc = self.lock_mutex(&self.fan_controller);
            fc.clear_temporary_curves();
            fc.set_same_speed(profile.fan.same_speed);
            fc.set_profile(self.fan_profile_from_ucc_profile(profile));
            if !fan_enabled {
                if let Err(e) = fc.restore_auto(self.platform.as_ref()) {
                    warn!("Failed restoring auto fan control for non-controlled profile: {e}");
                }
            }
        }

        // Preserve explicit user disablement and runtime enablement across profile reconciles.
        if profile.fan.enable_water_cooler
            && !self.water_cooler_user_disabled.load(Ordering::SeqCst)
        {
            self.water_cooler_enabled.store(true, Ordering::SeqCst);
        }
        {
            let mut cooler = self.lock_mutex(&self.cooler_controller);
            cooler.apply_manual_profile_targets(
                profile.fan.manual_fan_speed,
                profile.fan.manual_pump_voltage,
            );
            cooler.set_mode(if profile.fan.auto_control_wc {
                ucc_hardware::CoolerMode::Auto
            } else {
                ucc_hardware::CoolerMode::Manual
            });
            self.apply_active_cooler_targets_locked(&mut cooler, profile);
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub(super) fn rollback_hardware(
        &self,
        hw_odm_prev: Option<i32>,
        wrote_odm: bool,
        prev_tdps: &[Option<i32>],
        tdps_to_restore: usize,
        cpu_rollback: Vec<(std::path::PathBuf, String)>,
        start_epoch: Option<u64>,
    ) -> bool {
        let tdp_count = if wrote_odm {
            prev_tdps.len()
        } else {
            tdps_to_restore.min(prev_tdps.len())
        };
        let superseded =
            || start_epoch.is_some_and(|e| self.profile_commit_epoch.load(Ordering::SeqCst) != e);
        let mut cpu_rollback = Some(cpu_rollback);
        let reconcile_superseded = |cpu_rb: &mut Option<Vec<(std::path::PathBuf, String)>>| {
            let _ = self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps,
                tdps_written: tdp_count,
                cpu_rollback: cpu_rb.take().unwrap_or_default(),
                charging_rb: None,
                ..Default::default()
            });
            false
        };
        if superseded() {
            return reconcile_superseded(&mut cpu_rollback);
        }

        let mut ok = true;
        if wrote_odm {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if superseded() {
                return reconcile_superseded(&mut cpu_rollback);
            }
            if let Some(code) = hw_odm_prev {
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                let res = self.platform.set_perf_profile(code);
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                if superseded() {
                    return reconcile_superseded(&mut cpu_rollback);
                }
                if res.is_err() {
                    ok = false;
                } else if let Ok(actual) = self.platform.read_perf_profile() {
                    if actual != code {
                        ok = false;
                    }
                }
            } else {
                ok = false;
            }
        }
        for rb_idx in (0..tdp_count).rev() {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if superseded() {
                return reconcile_superseded(&mut cpu_rollback);
            }
            let Some(prev_w) = prev_tdps.get(rb_idx).copied().flatten() else {
                ok = false;
                continue;
            };
            self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
            let set_res = self.platform.set_tdp(rb_idx, prev_w);
            self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
            if superseded() {
                return reconcile_superseded(&mut cpu_rollback);
            }
            if set_res.is_err() {
                ok = false;
            } else {
                match self.platform.read_tdp(rb_idx) {
                    Ok(actual) if actual == prev_w => {}
                    _ => ok = false,
                }
            }
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if superseded() {
            return reconcile_superseded(&mut cpu_rollback);
        }
        if !self.rollback_cpu_writes(cpu_rollback.take().unwrap_or_default()) {
            ok = false;
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if start_epoch.is_some() {
            let sync = self.lock_mutex(&self.hw_reconcile_lock);
            if superseded() {
                drop(sync);
                return reconcile_superseded(&mut cpu_rollback);
            }
        }
        if !ok {
            warn!("Hardware rollback incomplete; syncing active profile from confirmed hardware readback");
            self.sync_active_profile_from_hw_readback();
            *self.lock_mutex(&self.last_applied_profile_state) = None;
            *self.lock_mutex(&self.cpu_policy_applied_rev) = None;
            self.set_mode_reapply_pending(true);
        }
        ok
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn restore_device_writes(
        &self,
        prev_brightness: Option<i32>,
        wrote_brightness: bool,
        prev_webcam: Option<bool>,
        wrote_webcam: bool,
        start_epoch: Option<u64>,
    ) {
        let superseded =
            || start_epoch.is_some_and(|e| self.profile_commit_epoch.load(Ordering::SeqCst) != e);
        if wrote_webcam {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) || superseded() {
                return;
            }
            if let Some(prev) = prev_webcam {
                let _ = self.set_webcam_internal(prev);
            }
        }
        if wrote_brightness {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) || superseded() {
                return;
            }
            if let Some(prev) = prev_brightness {
                let _ = self.set_display_brightness_internal(prev);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn abort_apply_on_shutdown(
        &self,
        prev_brightness: Option<i32>,
        wrote_brightness: bool,
        prev_webcam: Option<bool>,
        wrote_webcam: bool,
        hw_odm_prev: Option<i32>,
        wrote_odm: bool,
        prev_tdps: &[Option<i32>],
        tdps_written: usize,
        cpu_rollback: Vec<(std::path::PathBuf, String)>,
    ) -> ApplyOutcome {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return ApplyOutcome::Failed;
        }
        self.restore_device_writes(
            prev_brightness,
            wrote_brightness,
            prev_webcam,
            wrote_webcam,
            None,
        );
        let _ = self.rollback_hardware(
            hw_odm_prev,
            wrote_odm,
            prev_tdps,
            tdps_written,
            cpu_rollback,
            None,
        );
        ApplyOutcome::Failed
    }

    pub(super) fn reconcile_to_latest_committed(&self, undo: SupersededUndo<'_>) -> ApplyOutcome {
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        let _guard = self.lock_mutex(&self.hw_reconcile_lock);
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        let latest = self.lock_mutex(&self.latest_committed_profile).clone();
        let Some(latest) = latest else {
            return ApplyOutcome::Failed;
        };

        let mut repair_ok = true;

        if !self.rollback_cpu_writes(undo.cpu_rollback) {
            repair_ok = false;
        }
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        if undo.wrote_odm && latest.profile.odm_profile_name.is_none() {
            if let Some(code) = latest.hw_odm.or(undo.hw_odm_prev) {
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                let res = self.platform.set_perf_profile(code);
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                if res.is_err() {
                    repair_ok = false;
                } else if let Ok(actual) = self.platform.read_perf_profile() {
                    if actual != code {
                        repair_ok = false;
                    }
                }
            } else {
                repair_ok = false;
            }
        }
        let undo_tdp_count = if undo.wrote_odm {
            undo.prev_tdps.len()
        } else {
            undo.tdps_written.min(undo.prev_tdps.len())
        };
        if self.platform.is_available() && self.capabilities.tdp_channels > 0 {
            for idx in 0..undo_tdp_count {
                if self.shutting_down.load(Ordering::SeqCst)
                    || self.shutdown_wait_expired.load(Ordering::SeqCst)
                {
                    return ApplyOutcome::Failed;
                }
                if idx >= latest.profile.odm_power_limits.len() {
                    let expected = latest
                        .hw_tdps
                        .get(idx)
                        .copied()
                        .flatten()
                        .or_else(|| undo.prev_tdps.get(idx).copied().flatten());
                    match expected {
                        Some(w) => {
                            if !self.write_tdp_verified(idx, w) {
                                repair_ok = false;
                            }
                        }
                        None => repair_ok = false,
                    }
                }
            }
        }

        if self.apply_cpu_settings(&latest.profile).is_err() {
            repair_ok = false;
        }
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }

        if let Some(ref odm_name) = latest.profile.odm_profile_name {
            if self.platform.is_available() && self.capabilities.perf_profiles_available > 0 {
                if let Some(code) = self.resolve_odm_profile_code(odm_name) {
                    self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                    let res = self.platform.set_perf_profile(code);
                    self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                    if res.is_err() {
                        repair_ok = false;
                    } else if let Ok(actual) = self.platform.read_perf_profile() {
                        if actual != code {
                            repair_ok = false;
                        }
                    }
                } else {
                    repair_ok = false;
                }
            }
        }
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }

        if !latest.profile.odm_power_limits.is_empty()
            && self.platform.is_available()
            && self.capabilities.tdp_channels > 0
        {
            for (idx, &watts) in latest.profile.odm_power_limits.iter().enumerate() {
                if self.shutting_down.load(Ordering::SeqCst)
                    || self.shutdown_wait_expired.load(Ordering::SeqCst)
                {
                    return ApplyOutcome::Failed;
                }
                let Ok(w_i32) = i32::try_from(watts) else {
                    repair_ok = false;
                    continue;
                };
                if !self.write_tdp_verified(idx, w_i32) {
                    repair_ok = false;
                }
            }
        }

        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        if let Some(rb) = undo.charging_rb {
            let (cfg_start, cfg_end, cfg_type, cfg_prof, cfg_prio) = {
                let s = self.read_rw(&self.settings);
                if self.has_persisted_config {
                    (
                        s.charge_start_threshold,
                        s.charge_end_threshold,
                        s.charge_type.clone(),
                        s.charging_profile.clone(),
                        s.charging_priority.clone(),
                    )
                } else {
                    (None, None, None, None, None)
                }
            };
            let latest_has_charging = latest
                .profile
                .charging
                .start_threshold
                .or(cfg_start)
                .is_some()
                || latest.profile.charging.end_threshold.or(cfg_end).is_some()
                || latest.profile.charging.charge_type.is_some()
                || cfg_type.is_some()
                || latest
                    .profile
                    .charging
                    .charging_profile
                    .as_deref()
                    .or(cfg_prof.as_deref())
                    .is_some()
                || latest
                    .profile
                    .charging
                    .charging_priority
                    .as_deref()
                    .or(cfg_prio.as_deref())
                    .is_some();
            if !latest_has_charging && !self.restore_charging_rollback(rb, None) {
                repair_ok = false;
            }
        }
        if self
            .apply_charging_settings_checked(&latest.profile, None)
            .is_err()
        {
            repair_ok = false;
        }
        if let Some(offset) = latest.profile.nvidia_ctgp_offset {
            let _ = self.set_nvidia_power_offset_internal(offset);
        } else if undo.wrote_ctgp {
            if let Some(prev) = latest.hw_ctgp.or(undo.prev_ctgp) {
                let _ = self.restore_ctgp_baseline(Some(prev), None);
            }
        }
        if Self::profile_has_keyboard(&latest.profile) {
            if self.apply_profile_keyboard(&latest.profile).is_err() {
                repair_ok = false;
            }
        } else if undo.wrote_kb {
            if let Some(prev) = latest.hw_kb.as_ref().or(undo.prev_kb) {
                let _ = self.restore_keyboard_rollback(prev, None);
            }
        }
        self.apply_fan_and_device_state_locked(&latest.profile, true);

        if repair_ok {
            ApplyOutcome::SupersededReconciled
        } else {
            if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
                self.sync_active_profile_from_hw_readback();
            }
            *self.lock_mutex(&self.last_applied_profile_state) = None;
            *self.lock_mutex(&self.cpu_policy_applied_rev) = None;
            self.set_mode_reapply_pending(true);
            ApplyOutcome::Failed
        }
    }

    pub(super) fn apply_profile_internal(
        &self,
        profile: &UccProfile,
        emit_signal: bool,
        persist_state_updates: Option<&[(String, String)]>,
        target_state_key: Option<&str>,
        persist_profile_update: Option<(&UccProfile, bool)>,
        expected_stored_profile: Option<&UccProfile>,
    ) -> ApplyOutcome {
        self.apply_profile_txn(
            profile,
            emit_signal,
            persist_state_updates,
            target_state_key,
            persist_profile_update,
            expected_stored_profile,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_profile_txn(
        &self,
        profile: &UccProfile,
        emit_signal: bool,
        persist_state_updates: Option<&[(String, String)]>,
        target_state_key: Option<&str>,
        persist_profile_update: Option<(&UccProfile, bool)>,
        expected_stored_profile: Option<&UccProfile>,
        delete_profile_id: Option<&str>,
    ) -> ApplyOutcome {
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
        {
            return ApplyOutcome::Failed;
        }
        if profile.validate().is_err() {
            return ApplyOutcome::Failed;
        }
        let committed_profile = self.resolve_profile_with_fan_preset(profile);

        if self.is_vendor_unsupported() {
            if persist_state_updates.is_some()
                || persist_profile_update.is_some()
                || delete_profile_id.is_some()
            {
                let updated = self.update_settings(|s| {
                    if let Some((updated_prof, require_existing)) = persist_profile_update {
                        if is_builtin_profile_id(&updated_prof.id)
                            || (require_existing && !s.profiles.contains_key(&updated_prof.id))
                        {
                            return None;
                        }
                        s.profiles
                            .insert(updated_prof.id.clone(), updated_prof.clone());
                    }
                    if let Some(del_id) = delete_profile_id {
                        if is_builtin_profile_id(del_id) || del_id == committed_profile.id {
                            return None;
                        }
                        s.profiles.remove(del_id)?;
                        let fallback_id = default_office_profile().id;
                        for target in s.state_map.values_mut() {
                            if target == del_id {
                                *target = fallback_id.clone();
                            }
                        }
                    }
                    if let Some(updates) = persist_state_updates {
                        if updates.iter().any(|(_, pid)| !s.profiles.contains_key(pid)) {
                            return None;
                        }
                        for (k, pid) in updates {
                            s.state_map.insert(k.clone(), pid.clone());
                        }
                    }
                    Some(())
                });
                if updated.is_none() {
                    return ApplyOutcome::Failed;
                }
            }
            *self.write_rw(&self.active_profile) = committed_profile.clone();
            if emit_signal {
                self.emit_profile_changed(&committed_profile);
            }
            return ApplyOutcome::Committed;
        }

        let (target_odm_code, hw_odm_prev) = match profile.odm_profile_name {
            Some(ref odm_name)
                if self.platform.is_available()
                    && self.capabilities.perf_profiles_available > 0 =>
            {
                let Some(code) = self.resolve_odm_profile_code(odm_name) else {
                    return ApplyOutcome::Failed;
                };
                let prev = match self.platform.read_perf_profile() {
                    Ok(cur) => Some(cur),
                    Err(PlatformError::Unavailable) => None,
                    Err(_) => return ApplyOutcome::Failed,
                };
                (Some(code), prev)
            }
            _ => (None, None),
        };

        let mut prev_tdps: Vec<Option<i32>> = Vec::new();
        let mut target_tdps: Vec<i32> = Vec::new();
        let has_tdps = !profile.odm_power_limits.is_empty()
            && self.platform.is_available()
            && self.capabilities.tdp_channels > 0;
        if has_tdps {
            if profile.odm_power_limits.len() > self.capabilities.tdp_channels {
                return ApplyOutcome::Failed;
            }
            for (idx, &watts) in profile.odm_power_limits.iter().enumerate() {
                let Ok(w_i32) = i32::try_from(watts) else {
                    return ApplyOutcome::Failed;
                };
                let Ok((min_w, max_w)) = self.platform.read_tdp_limits(idx) else {
                    return ApplyOutcome::Failed;
                };
                if !(min_w..=max_w).contains(&w_i32) {
                    return ApplyOutcome::Failed;
                }
                let Ok(cur_w) = self.platform.read_tdp(idx) else {
                    return ApplyOutcome::Failed;
                };
                prev_tdps.push(Some(cur_w));
                target_tdps.push(w_i32);
            }
            if target_odm_code.is_some() {
                for idx in profile.odm_power_limits.len()..self.capabilities.tdp_channels {
                    let Ok(cur_w) = self.platform.read_tdp(idx) else {
                        return ApplyOutcome::Failed;
                    };
                    prev_tdps.push(Some(cur_w));
                }
            }
        } else if target_odm_code.is_some()
            && self.platform.is_available()
            && self.capabilities.tdp_channels > 0
        {
            for idx in 0..self.capabilities.tdp_channels {
                let Ok(cur_w) = self.platform.read_tdp(idx) else {
                    return ApplyOutcome::Failed;
                };
                prev_tdps.push(Some(cur_w));
            }
        }

        let prev_brightness = (profile.display.use_brightness
            && self.discover_backlight_device().is_some())
        .then(|| self.get_display_brightness_impl())
        .filter(|&b| b >= 0);
        let prev_webcam = (profile.webcam.use_status && self.webcam_sw_available_impl())
            .then(|| self.platform.read_webcam().unwrap_or(false));
        let mut wrote_brightness = false;
        let mut wrote_webcam = false;

        let tx_id = self
            .next_profile_tx_id
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        let _tx_guard = InFlightTxGuard {
            backend: self,
            tx_id,
        };
        let start_epoch = self.profile_commit_epoch.load(Ordering::SeqCst);

        if self.shutting_down.load(Ordering::SeqCst) {
            return ApplyOutcome::Failed;
        }

        // Phase 1: Reversible CPU writes FIRST (`M8-PROD-04`).
        let Ok(cpu_res) = self.apply_cpu_settings(profile) else {
            if !self.shutting_down.load(Ordering::SeqCst)
                && !self.shutdown_wait_expired.load(Ordering::SeqCst)
                && self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch
            {
                let _ = self.reconcile_to_latest_committed(SupersededUndo {
                    hw_odm_prev: None,
                    wrote_odm: false,
                    prev_tdps: &prev_tdps,
                    tdps_written: 0,
                    cpu_rollback: Vec::new(),
                    charging_rb: None,
                    ..Default::default()
                });
            }
            return ApplyOutcome::Failed;
        };

        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                None,
                false,
                &prev_tdps,
                0,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev: None,
                wrote_odm: false,
                prev_tdps: &prev_tdps,
                tdps_written: 0,
                cpu_rollback: cpu_res.rollback,
                charging_rb: None,
                ..Default::default()
            });
        }

        // Phase 2: ODM + TDP with readback verification.
        let mut wrote_odm = false;
        let odm_write_verified = |code: i32| -> bool {
            self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
            let res = self.platform.set_perf_profile(code);
            self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
            if res.is_err() {
                return false;
            }
            if hw_odm_prev.is_some() {
                return matches!(self.platform.read_perf_profile(), Ok(actual) if actual == code);
            }
            true
        };

        if target_odm_code.is_some() && has_tdps && hw_odm_prev.is_none() {
            let mut wrote_tdps = 0usize;
            for (idx, &w) in target_tdps.iter().enumerate() {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        false,
                        &prev_tdps,
                        wrote_tdps,
                        cpu_res.rollback,
                    );
                }
                if let Some(prev_w) = prev_tdps[idx] {
                    self.record_in_flight_tdp_baseline(tx_id, idx, prev_w);
                }
                self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                let set_ok = self.platform.set_tdp(idx, w).is_ok();
                self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        false,
                        &prev_tdps,
                        idx + 1,
                        cpu_res.rollback,
                    );
                }
                if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                    let undo = SupersededUndo {
                        hw_odm_prev: None,
                        wrote_odm: false,
                        prev_tdps: &prev_tdps,
                        tdps_written: idx + 1,
                        cpu_rollback: cpu_res.rollback,
                        charging_rb: None,
                        ..Default::default()
                    };
                    if set_ok {
                        return self.reconcile_to_latest_committed(undo);
                    }
                    let _ = self.reconcile_to_latest_committed(undo);
                    return ApplyOutcome::Failed;
                }
                if !set_ok || !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == w) {
                    let _ = self.rollback_hardware(
                        None,
                        false,
                        &prev_tdps,
                        idx + 1,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
                wrote_tdps += 1;
            }

            if let Some(code) = target_odm_code {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        false,
                        &prev_tdps,
                        wrote_tdps,
                        cpu_res.rollback,
                    );
                }
                for (i, slot) in prev_tdps.iter().enumerate() {
                    if let Some(w) = *slot {
                        self.record_in_flight_tdp_baseline(tx_id, i, w);
                    }
                }
                let odm_ok = odm_write_verified(code);
                wrote_odm = odm_ok;
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        true,
                        &prev_tdps,
                        if wrote_odm {
                            prev_tdps.len()
                        } else {
                            wrote_tdps
                        },
                        cpu_res.rollback,
                    );
                }
                if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                    let undo = SupersededUndo {
                        hw_odm_prev: None,
                        wrote_odm,
                        prev_tdps: &prev_tdps,
                        tdps_written: wrote_tdps,
                        cpu_rollback: cpu_res.rollback,
                        charging_rb: None,
                        ..Default::default()
                    };
                    if odm_ok {
                        return self.reconcile_to_latest_committed(undo);
                    }
                    let _ = self.reconcile_to_latest_committed(undo);
                    return ApplyOutcome::Failed;
                }
                if !odm_ok {
                    let _ = self.rollback_hardware(
                        None,
                        true,
                        &prev_tdps,
                        wrote_tdps,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }

                for (idx, &w) in target_tdps.iter().enumerate() {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            None,
                            true,
                            &prev_tdps,
                            prev_tdps.len(),
                            cpu_res.rollback,
                        );
                    }
                    if let Some(prev_w) = prev_tdps[idx] {
                        self.record_in_flight_tdp_baseline(tx_id, idx, prev_w);
                    }
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    let set_ok = self.platform.set_tdp(idx, w).is_ok();
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            None,
                            true,
                            &prev_tdps,
                            prev_tdps.len(),
                            cpu_res.rollback,
                        );
                    }
                    if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                        let undo = SupersededUndo {
                            hw_odm_prev: None,
                            wrote_odm: true,
                            prev_tdps: &prev_tdps,
                            tdps_written: prev_tdps.len(),
                            cpu_rollback: cpu_res.rollback,
                            charging_rb: None,
                            ..Default::default()
                        };
                        if set_ok {
                            return self.reconcile_to_latest_committed(undo);
                        }
                        let _ = self.reconcile_to_latest_committed(undo);
                        return ApplyOutcome::Failed;
                    }
                    if !set_ok || !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == w)
                    {
                        let _ = self.rollback_hardware(
                            None,
                            true,
                            &prev_tdps,
                            prev_tdps.len(),
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
        } else {
            if let Some(code) = target_odm_code {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        hw_odm_prev,
                        false,
                        &prev_tdps,
                        0,
                        cpu_res.rollback,
                    );
                }
                if let Some(prev_code) = hw_odm_prev {
                    self.record_in_flight_odm_baseline(tx_id, prev_code);
                }
                for (i, slot) in prev_tdps.iter().enumerate() {
                    if let Some(w) = *slot {
                        self.record_in_flight_tdp_baseline(tx_id, i, w);
                    }
                }
                let odm_ok = odm_write_verified(code);
                wrote_odm = true;
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        hw_odm_prev,
                        true,
                        &prev_tdps,
                        prev_tdps.len(),
                        cpu_res.rollback,
                    );
                }
                if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                    let undo = SupersededUndo {
                        hw_odm_prev,
                        wrote_odm,
                        prev_tdps: &prev_tdps,
                        tdps_written: prev_tdps.len(),
                        cpu_rollback: cpu_res.rollback,
                        charging_rb: None,
                        ..Default::default()
                    };
                    if odm_ok {
                        return self.reconcile_to_latest_committed(undo);
                    }
                    let _ = self.reconcile_to_latest_committed(undo);
                    return ApplyOutcome::Failed;
                }
                if !odm_ok {
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        true,
                        &prev_tdps,
                        prev_tdps.len(),
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
            }

            if has_tdps {
                for (idx, &w) in target_tdps.iter().enumerate() {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            if wrote_odm { prev_tdps.len() } else { idx },
                            cpu_res.rollback,
                        );
                    }
                    if let Some(prev_w) = prev_tdps[idx] {
                        self.record_in_flight_tdp_baseline(tx_id, idx, prev_w);
                    }
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    let set_ok = self.platform.set_tdp(idx, w).is_ok();
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            if wrote_odm { prev_tdps.len() } else { idx + 1 },
                            cpu_res.rollback,
                        );
                    }
                    if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                        let undo = SupersededUndo {
                            hw_odm_prev,
                            wrote_odm,
                            prev_tdps: &prev_tdps,
                            tdps_written: if wrote_odm { prev_tdps.len() } else { idx + 1 },
                            cpu_rollback: cpu_res.rollback,
                            charging_rb: None,
                            ..Default::default()
                        };
                        if set_ok {
                            return self.reconcile_to_latest_committed(undo);
                        }
                        let _ = self.reconcile_to_latest_committed(undo);
                        return ApplyOutcome::Failed;
                    }
                    if !set_ok || !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == w)
                    {
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            idx + 1,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
        }

        let post_phase2_tdp_seq = self.tdp_write_seq.load(Ordering::SeqCst);
        let post_phase2_odm_seq = self.odm_write_seq.load(Ordering::SeqCst);
        let tdps_written = if has_tdps || wrote_odm {
            if wrote_odm {
                prev_tdps.len()
            } else {
                target_tdps.len()
            }
        } else {
            0
        };

        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps: &prev_tdps,
                tdps_written,
                cpu_rollback: cpu_res.rollback,
                charging_rb: None,
                ..Default::default()
            });
        }

        if profile.display.use_brightness && self.discover_backlight_device().is_some() {
            if !self.set_display_brightness_internal(profile.display.brightness) {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        false,
                        prev_webcam,
                        false,
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                    );
                }
                let _ = self.rollback_hardware(
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                    Some(start_epoch),
                );
                return ApplyOutcome::Failed;
            }
            wrote_brightness = true;
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if profile.webcam.use_status && self.webcam_sw_available_impl() {
            if !self.set_webcam_internal(profile.webcam.status) {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        false,
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                    );
                }
                self.restore_device_writes(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    Some(start_epoch),
                );
                let _ = self.rollback_hardware(
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                    Some(start_epoch),
                );
                return ApplyOutcome::Failed;
            }
            wrote_webcam = true;
        }

        let charging_rb =
            match self.apply_charging_settings_checked(&committed_profile, Some(start_epoch)) {
                Ok(rb) => rb,
                Err(()) => {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            tdps_written,
                            cpu_res.rollback,
                        );
                    }
                    if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                        self.restore_device_writes(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            Some(start_epoch),
                        );
                        return self.reconcile_to_latest_committed(SupersededUndo {
                            hw_odm_prev,
                            wrote_odm,
                            prev_tdps: &prev_tdps,
                            tdps_written,
                            cpu_rollback: cpu_res.rollback,
                            charging_rb: None,
                            ..Default::default()
                        });
                    }
                    self.restore_device_writes(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        Some(start_epoch),
                    );
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
            };
        let post_phase2_charging_seq = self.charging_write_seq.load(Ordering::SeqCst);
        let prev_ctgp = committed_profile
            .nvidia_ctgp_offset
            .and_then(|_| self.read_ctgp_baseline());
        let mut wrote_ctgp = false;
        let prev_kb = if Self::profile_has_keyboard(&committed_profile) {
            self.snapshot_keyboard_rollback()
        } else {
            None
        };
        let mut wrote_kb = false;

        if self.shutting_down.load(Ordering::SeqCst) {
            let _ = self.restore_charging_rollback(&charging_rb, None);
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            self.restore_device_writes(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                Some(start_epoch),
            );
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps: &prev_tdps,
                tdps_written,
                cpu_rollback: cpu_res.rollback,
                charging_rb: Some(&charging_rb),
                ..Default::default()
            });
        }

        if let Some(offset) = committed_profile.nvidia_ctgp_offset {
            if !self.is_vendor_unsupported()
                && !ucc_config::is_read_only_preview()
                && self.has_ctgp_controller()
            {
                let Some(base) = prev_ctgp else {
                    if !self.shutting_down.load(Ordering::SeqCst)
                        && self.profile_commit_epoch.load(Ordering::SeqCst) == start_epoch
                    {
                        let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                        self.restore_device_writes(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            Some(start_epoch),
                        );
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            tdps_written,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                    }
                    return ApplyOutcome::Failed;
                };
                self.record_in_flight_ctgp_baseline(tx_id, base);
                if self.set_nvidia_power_offset_internal(offset) {
                    wrote_ctgp = true;
                } else {
                    let _ = self.restore_ctgp_baseline(prev_ctgp, Some(start_epoch));
                    let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                    self.restore_device_writes(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        Some(start_epoch),
                    );
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
            }
        }
        let post_phase2_ctgp_seq = self.ctgp_write_seq.load(Ordering::SeqCst);

        if self.shutting_down.load(Ordering::SeqCst) {
            self.restore_ctgp_and_kb_writes(prev_ctgp, wrote_ctgp, prev_kb.as_ref(), false, None);
            let _ = self.restore_charging_rollback(&charging_rb, None);
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            self.restore_device_writes(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                Some(start_epoch),
            );
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps: &prev_tdps,
                tdps_written,
                cpu_rollback: cpu_res.rollback,
                charging_rb: Some(&charging_rb),
                prev_ctgp,
                wrote_ctgp,
                prev_kb: None,
                wrote_kb: false,
            });
        }

        if Self::profile_has_keyboard(&committed_profile) {
            if let Some(ref snap) = prev_kb {
                self.record_in_flight_kb_baseline(tx_id, snap);
            }
            match self.apply_profile_keyboard(&committed_profile) {
                Ok(applied) => {
                    wrote_kb = applied && prev_kb.is_some();
                }
                Err(()) => {
                    if let Some(ref snap) = prev_kb {
                        let _ = self.restore_keyboard_rollback(snap, Some(start_epoch));
                    }
                    self.restore_ctgp_and_kb_writes(
                        prev_ctgp,
                        wrote_ctgp,
                        None,
                        false,
                        Some(start_epoch),
                    );
                    let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                    self.restore_device_writes(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        Some(start_epoch),
                    );
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
            }
        }
        let post_phase2_kb_seq = self.kb_write_seq.load(Ordering::SeqCst);

        if self.shutting_down.load(Ordering::SeqCst) {
            self.restore_ctgp_and_kb_writes(
                prev_ctgp,
                wrote_ctgp,
                prev_kb.as_ref(),
                wrote_kb,
                None,
            );
            let _ = self.restore_charging_rollback(&charging_rb, None);
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            self.restore_device_writes(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                Some(start_epoch),
            );
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps: &prev_tdps,
                tdps_written,
                cpu_rollback: cpu_res.rollback,
                charging_rb: Some(&charging_rb),
                prev_ctgp,
                wrote_ctgp,
                prev_kb: prev_kb.as_ref(),
                wrote_kb,
            });
        }

        // Phase 3: Atomic commit under `hw_reconcile_lock`.
        {
            let guard = self.lock_mutex(&self.hw_reconcile_lock);
            if self.shutting_down.load(Ordering::SeqCst) {
                drop(guard);
                self.restore_ctgp_and_kb_writes(
                    prev_ctgp,
                    wrote_ctgp,
                    prev_kb.as_ref(),
                    wrote_kb,
                    None,
                );
                let _ = self.restore_charging_rollback(&charging_rb, None);
                return self.abort_apply_on_shutdown(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                );
            }
            if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                drop(guard);
                self.restore_device_writes(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    Some(start_epoch),
                );
                return self.reconcile_to_latest_committed(SupersededUndo {
                    hw_odm_prev,
                    wrote_odm,
                    prev_tdps: &prev_tdps,
                    tdps_written,
                    cpu_rollback: cpu_res.rollback,
                    charging_rb: Some(&charging_rb),
                    prev_ctgp,
                    wrote_ctgp,
                    prev_kb: prev_kb.as_ref(),
                    wrote_kb,
                });
            }

            if let Some(code) = target_odm_code {
                if self.odm_write_seq.load(Ordering::SeqCst) != post_phase2_odm_seq {
                    let odm_ok = if hw_odm_prev.is_some()
                        && matches!(self.platform.read_perf_profile(), Ok(actual) if actual == code)
                    {
                        true
                    } else {
                        odm_write_verified(code)
                    };
                    if !odm_ok {
                        drop(guard);
                        self.restore_ctgp_and_kb_writes(
                            prev_ctgp,
                            wrote_ctgp,
                            prev_kb.as_ref(),
                            wrote_kb,
                            Some(start_epoch),
                        );
                        let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                        self.restore_device_writes(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            Some(start_epoch),
                        );
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            tdps_written,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
            if has_tdps && self.tdp_write_seq.load(Ordering::SeqCst) != post_phase2_tdp_seq {
                for (idx, &target_w) in target_tdps.iter().enumerate() {
                    if !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == target_w)
                        && !self.write_tdp_verified(idx, target_w)
                    {
                        drop(guard);
                        self.restore_ctgp_and_kb_writes(
                            prev_ctgp,
                            wrote_ctgp,
                            prev_kb.as_ref(),
                            wrote_kb,
                            Some(start_epoch),
                        );
                        let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                        self.restore_device_writes(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            Some(start_epoch),
                        );
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            tdps_written,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
            if self.charging_write_seq.load(Ordering::SeqCst) != post_phase2_charging_seq
                && self
                    .apply_charging_settings_checked(&committed_profile, None)
                    .is_err()
            {
                drop(guard);
                self.restore_ctgp_and_kb_writes(
                    prev_ctgp,
                    wrote_ctgp,
                    prev_kb.as_ref(),
                    wrote_kb,
                    Some(start_epoch),
                );
                let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                self.restore_device_writes(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    Some(start_epoch),
                );
                let _ = self.rollback_hardware(
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                    Some(start_epoch),
                );
                return ApplyOutcome::Failed;
            }
            if self.ctgp_write_seq.load(Ordering::SeqCst) != post_phase2_ctgp_seq {
                if let Some(offset) = committed_profile.nvidia_ctgp_offset {
                    let _ = self.set_nvidia_power_offset_internal(offset);
                } else if let Some(base) = self.snapshot_unmanaged_ctgp_baseline(tx_id) {
                    let _ = self.restore_ctgp_baseline(Some(base), Some(start_epoch));
                }
            }
            if self.kb_write_seq.load(Ordering::SeqCst) != post_phase2_kb_seq {
                if Self::profile_has_keyboard(&committed_profile) {
                    let _ = self.apply_profile_keyboard(&committed_profile);
                } else if let Some(ref kb) = self.snapshot_unmanaged_kb_baseline(tx_id) {
                    let _ = self.restore_keyboard_rollback(kb, Some(start_epoch));
                }
            }

            if persist_state_updates.is_some()
                || persist_profile_update.is_some()
                || expected_stored_profile.is_some()
                || delete_profile_id.is_some()
            {
                let mut stale_err = false;
                let txn_res = if persist_state_updates.is_some()
                    || persist_profile_update.is_some()
                    || delete_profile_id.is_some()
                {
                    self.update_settings(|s| {
                        if let Some(expected) = expected_stored_profile {
                            match s.profiles.get(&expected.id) {
                                None => return None,
                                Some(stored) if stored != expected => {
                                    stale_err = true;
                                    return None;
                                }
                                Some(_) => {}
                            }
                        }
                        if let Some((updated_prof, require_existing)) = persist_profile_update {
                            if is_builtin_profile_id(&updated_prof.id)
                                || (require_existing && !s.profiles.contains_key(&updated_prof.id))
                            {
                                return None;
                            }
                            s.profiles
                                .insert(updated_prof.id.clone(), updated_prof.clone());
                        }
                        if let Some(del_id) = delete_profile_id {
                            if is_builtin_profile_id(del_id) || del_id == committed_profile.id {
                                return None;
                            }
                            s.profiles.remove(del_id)?;
                            let fallback_id = default_office_profile().id;
                            for target in s.state_map.values_mut() {
                                if target == del_id {
                                    *target = fallback_id.clone();
                                }
                            }
                        }
                        if let Some(updates) = persist_state_updates {
                            if updates
                                .iter()
                                .any(|(_, prof_id)| !s.profiles.contains_key(prof_id))
                            {
                                return None;
                            }
                            for (state_key, prof_id) in updates {
                                s.state_map.insert(state_key.clone(), prof_id.clone());
                            }
                        }
                        Some(())
                    })
                } else if let Some(expected) = expected_stored_profile {
                    let _mgr = self.lock_mutex(&self.config_manager);
                    let s = self.read_rw(&self.settings);
                    match s.profiles.get(&expected.id) {
                        None => None,
                        Some(stored) if stored != expected => {
                            stale_err = true;
                            None
                        }
                        Some(_) => Some(()),
                    }
                } else {
                    Some(())
                };

                if txn_res.is_none() {
                    drop(guard);
                    self.restore_ctgp_and_kb_writes(
                        prev_ctgp,
                        wrote_ctgp,
                        prev_kb.as_ref(),
                        wrote_kb,
                        Some(start_epoch),
                    );
                    let _ = self.restore_charging_rollback(&charging_rb, Some(start_epoch));
                    self.restore_device_writes(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        Some(start_epoch),
                    );
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return if stale_err {
                        ApplyOutcome::StaleRevision
                    } else {
                        ApplyOutcome::Failed
                    };
                }
            }

            self.apply_fan_and_device_state_locked(&committed_profile, false);
            *self.write_rw(&self.active_profile) = committed_profile.clone();
            let hw_tdps = (0..self.capabilities.tdp_channels)
                .map(|idx| {
                    if idx < committed_profile.odm_power_limits.len() {
                        let mut inflight = self.lock_mutex(&self.in_flight_tdp_baselines);
                        inflight.retain(|(id, ch, _)| *id == tx_id || *ch != idx);
                        drop(inflight);
                        self.platform.read_tdp(idx).ok()
                    } else {
                        self.snapshot_unmanaged_tdp_baseline(tx_id, idx)
                    }
                })
                .collect();
            let hw_odm = if committed_profile.odm_profile_name.is_some() {
                let mut inflight = self.lock_mutex(&self.in_flight_odm_baselines);
                inflight.retain(|(id, _)| *id == tx_id);
                drop(inflight);
                self.platform.read_perf_profile().ok()
            } else {
                self.snapshot_unmanaged_odm_baseline(tx_id)
            };
            let hw_ctgp = if committed_profile.nvidia_ctgp_offset.is_some() {
                let mut inflight = self.lock_mutex(&self.in_flight_ctgp_baselines);
                inflight.retain(|(id, _)| *id == tx_id);
                drop(inflight);
                self.read_ctgp_baseline()
            } else {
                self.snapshot_unmanaged_ctgp_baseline(tx_id)
            };
            let hw_kb = if Self::profile_has_keyboard(&committed_profile) {
                let mut inflight = self.lock_mutex(&self.in_flight_kb_baselines);
                inflight.retain(|(id, _)| *id == tx_id);
                drop(inflight);
                self.snapshot_keyboard_rollback()
            } else {
                self.snapshot_unmanaged_kb_baseline(tx_id)
            };
            let _new_epoch = self
                .profile_commit_epoch
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            *self.lock_mutex(&self.latest_committed_profile) = Some(CommittedProfileRecord {
                profile: committed_profile.clone(),
                hw_tdps,
                hw_odm,
                hw_ctgp,
                hw_kb,
            });
            self.record_committed_charging_for_profile(&committed_profile);
            self.charging_op_seq.fetch_add(1, Ordering::SeqCst);
        }

        let decision = self.current_decision();
        let pd_rev = self.power_devices.revision();
        *self.lock_mutex(&self.cpu_policy_applied_rev) = if cpu_res.fully_applied {
            Some((pd_rev, decision.effective))
        } else {
            None
        };
        if persist_state_updates.is_some() {
            *self.write_rw(&self.temp_profile_name) = None;
            if let Some(applied_state) = target_state_key {
                if decision.profile_state.as_str() == applied_state {
                    *self.lock_mutex(&self.last_applied_profile_state) =
                        Some(applied_state.to_string());
                } else {
                    *self.lock_mutex(&self.last_applied_profile_state) = None;
                }
            } else {
                *self.lock_mutex(&self.last_applied_profile_state) =
                    Some(decision.profile_state.as_str().to_string());
            }
        }
        self.set_mode_reapply_pending(false);
        let _ = self.tick_fans_once();
        if emit_signal {
            self.emit_profile_changed(&committed_profile);
        }
        ApplyOutcome::Committed
    }

    /// Reconciles active profile, `PowerDeviceController`, `LightbarController`, and D-Bus signals
    /// with physical power supply and `PowerPolicy` state (`M8-RECHECK-04`, `09`, `M8-PROD-09`).
    pub fn reconcile_power_state(&self, force_apply: bool) {
        let _ = self.reconcile_power_state_checked(force_apply);
    }

    /// Runs power-state reconciliation and returns `true` if a full profile transaction was
    /// applied cleanly (meaning charging and initial fan tick already ran inside the transaction) (`TA-22`).
    pub(super) fn reconcile_power_state_checked(&self, force_apply: bool) -> bool {
        if self.shutting_down.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
            || self.is_vendor_unsupported()
        {
            return false;
        }
        let decision = self.current_decision();
        let state_str = decision.profile_state.as_str().to_string();
        let effective_mode = decision.effective;
        let physical_fallback_mode = effective_mode.as_str().to_string();
        let policy_json = decision.status.to_string();
        let pd_rev = self.power_devices.revision();

        let prev_state = self.lock_mutex(&self.last_power_state).clone();
        let state_changed = prev_state.as_deref() != Some(state_str.as_str());
        let prev_applied_state = self.lock_mutex(&self.last_applied_profile_state).clone();
        let profile_pending = force_apply
            || state_changed
            || prev_applied_state.as_deref() != Some(state_str.as_str());
        let prev_effective = *self.lock_mutex(&self.last_effective_mode);
        let effective_changed = prev_effective != Some(effective_mode);
        let prev_policy = self.lock_mutex(&self.last_policy_json).clone();
        let policy_changed = prev_policy.as_deref() != Some(policy_json.as_str());
        let prev_pd_rev = *self.lock_mutex(&self.last_power_device_revision);
        let pd_changed = prev_pd_rev != Some(pd_rev);
        let cpu_overlay_pending =
            *self.lock_mutex(&self.cpu_policy_applied_rev) != Some((pd_rev, effective_mode));

        if force_apply || effective_changed {
            let lb_mode = match effective_mode {
                ProfileState::Ac | ProfileState::Wc => LightbarPowerMode::Ac,
                ProfileState::TypeC => LightbarPowerMode::TypeC,
                ProfileState::Bat => LightbarPowerMode::Bat,
            };
            self.lock_mutex(&self.lightbar_ctrl).set_power_mode(lb_mode);
            *self.lock_mutex(&self.last_effective_mode) = Some(effective_mode);
        }

        if state_changed {
            *self.lock_mutex(&self.last_power_state) = Some(state_str.clone());
            if prev_state.is_some() {
                self.emit_signal(DbusSignalEvent::PowerStateChanged {
                    state: state_str.clone(),
                });
            }
        }

        let mut applied_profile_now = false;
        if profile_pending {
            if state_changed && !force_apply {
                *self.write_rw(&self.temp_profile_name) = None;
            }
            for _ in 0..3 {
                if self.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                let dec = self.current_decision();
                let apply_state = dec.profile_state.as_str().to_string();
                let phys_fallback = dec.effective.as_str().to_string();
                let (target_profile, expected_stored) = {
                    let s = self.read_rw(&self.settings);
                    match s.get_active_profile_for_mode_with_fallback(
                        &apply_state,
                        Some(&phys_fallback),
                    ) {
                        Some(p) => (p.clone(), Some(p.clone())),
                        None => {
                            let cur = self.read_rw(&self.active_profile).clone();
                            if s.profiles.contains_key(&cur.id) {
                                (cur.clone(), Some(cur))
                            } else {
                                (default_high_performance_profile(), None)
                            }
                        }
                    }
                };
                match self.apply_profile_internal(
                    &target_profile,
                    prev_state.is_some() || state_changed,
                    None,
                    None,
                    None,
                    expected_stored.as_ref(),
                ) {
                    ApplyOutcome::Committed => {
                        let decision_matches =
                            self.current_decision().profile_state.as_str() == apply_state;
                        let stored_matches = expected_stored.as_ref().is_none_or(|exp| {
                            let dec_now = self.current_decision();
                            let phys_now = dec_now.effective.as_str();
                            self.read_rw(&self.settings)
                                .get_active_profile_for_mode_with_fallback(
                                    &apply_state,
                                    Some(phys_now),
                                )
                                .is_some_and(|cur| cur == exp)
                        });
                        if decision_matches && stored_matches {
                            *self.lock_mutex(&self.last_applied_profile_state) = Some(apply_state);
                            applied_profile_now = true;
                            break;
                        }
                        *self.lock_mutex(&self.last_applied_profile_state) = None;
                        self.set_mode_reapply_pending(true);
                    }
                    ApplyOutcome::SupersededReconciled => {
                        applied_profile_now = true;
                        break;
                    }
                    ApplyOutcome::StaleRevision => {
                        *self.lock_mutex(&self.last_applied_profile_state) = None;
                        self.set_mode_reapply_pending(true);
                    }
                    ApplyOutcome::Failed => {
                        *self.lock_mutex(&self.last_applied_profile_state) = None;
                        self.set_mode_reapply_pending(true);
                        break;
                    }
                }
            }
        }

        if self.shutting_down.load(Ordering::SeqCst) {
            return applied_profile_now;
        }

        let _ = physical_fallback_mode;
        if force_apply || effective_changed || pd_changed || cpu_overlay_pending {
            if force_apply || effective_changed || pd_changed {
                self.power_devices.apply(effective_mode.mode_name());
                *self.lock_mutex(&self.last_power_device_revision) = Some(pd_rev);
                if prev_pd_rev.is_some() && (pd_changed || effective_changed) {
                    self.emit_signal(DbusSignalEvent::PowerDeviceStatusChanged {
                        json: self.power_devices.status().to_string(),
                    });
                }
            }
            if !applied_profile_now || cpu_overlay_pending {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return applied_profile_now;
                }
                let current_active = self.read_rw(&self.active_profile).clone();
                match self.apply_cpu_settings(&current_active) {
                    Ok(res) if res.fully_applied => {
                        *self.lock_mutex(&self.cpu_policy_applied_rev) =
                            Some((pd_rev, effective_mode));
                    }
                    _ => {
                        *self.lock_mutex(&self.cpu_policy_applied_rev) = None;
                    }
                }
            }
        }

        if policy_changed {
            *self.lock_mutex(&self.last_policy_json) = Some(policy_json.clone());
            if prev_policy.is_some() {
                self.emit_signal(DbusSignalEvent::PowerPolicyChanged { json: policy_json });
            }
        }
        applied_profile_now
    }

    pub(super) fn persist_or_reapply_custom_profile(
        &self,
        profile_json: &str,
        require_existing: bool,
    ) -> bool {
        if ucc_config::is_read_only_preview()
            || self.shutting_down.load(Ordering::SeqCst)
            || profile_json.len() > MAX_PROFILE_JSON_BYTES
        {
            return false;
        }
        let Ok(mut prof) = serde_json::from_str::<UccProfile>(profile_json) else {
            return false;
        };
        if !require_existing && prof.id.trim().is_empty() {
            let seq = self.next_custom_profile_seq.fetch_add(1, Ordering::SeqCst) + 1;
            prof.id = format!("custom_{}_{}", unix_ms(), seq);
        }
        if prof.id.trim().is_empty()
            || is_builtin_profile_id(&prof.id)
            || is_builtin_profile_name(&prof.name)
            || prof.validate().is_err()
        {
            return false;
        }
        {
            let settings = self.read_rw(&self.settings);
            if require_existing && !settings.profiles.contains_key(&prof.id) {
                return false;
            }
            if !settings.profiles.contains_key(&prof.id) {
                let custom_count = settings
                    .profiles
                    .keys()
                    .filter(|id| !is_builtin_profile_id(id))
                    .count();
                if custom_count >= MAX_CUSTOM_PROFILES {
                    return false;
                }
            }
        }

        for _attempt in 0..4 {
            if self.shutting_down.load(Ordering::SeqCst) {
                return false;
            }
            if self.read_rw(&self.active_profile).id == prof.id {
                let Some(_op_guard) = self.enter_hardware_op() else {
                    return false;
                };
                if self.read_rw(&self.active_profile).id == prof.id {
                    match self.apply_profile_internal(
                        &prof,
                        true,
                        None,
                        None,
                        Some((&prof, require_existing)),
                        None,
                    ) {
                        ApplyOutcome::Committed => return true,
                        ApplyOutcome::SupersededReconciled => continue,
                        ApplyOutcome::StaleRevision | ApplyOutcome::Failed => return false,
                    }
                }
            }

            let _guard = self.lock_mutex(&self.hw_reconcile_lock);
            if self.shutting_down.load(Ordering::SeqCst) {
                return false;
            }
            if self.read_rw(&self.active_profile).id == prof.id {
                continue;
            }
            return self
                .update_settings(|s| {
                    if require_existing && !s.profiles.contains_key(&prof.id) {
                        return None;
                    }
                    if !s.profiles.contains_key(&prof.id) {
                        let custom_count = s
                            .profiles
                            .keys()
                            .filter(|id| !is_builtin_profile_id(id))
                            .count();
                        if custom_count >= MAX_CUSTOM_PROFILES {
                            return None;
                        }
                    }
                    s.profiles.insert(prof.id.clone(), prof.clone());
                    Some(())
                })
                .is_some();
        }
        false
    }

    pub(super) fn delete_custom_profile_impl(&self, profile_id: &str) -> bool {
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        if is_builtin_profile_id(profile_id) {
            return false;
        }
        let fallback_id = default_office_profile().id;

        for _attempt in 0..4 {
            if self.shutting_down.load(Ordering::SeqCst) {
                return false;
            }
            let prev_active = self.read_rw(&self.active_profile).clone();
            if prev_active.id == profile_id {
                let Some(_op_guard) = self.enter_hardware_op() else {
                    return false;
                };
                if !self
                    .read_rw(&self.settings)
                    .profiles
                    .contains_key(profile_id)
                {
                    return false;
                }
                let decision = self.current_decision();
                let apply_state = decision.profile_state.as_str().to_string();
                let fallback_profile = {
                    let mut sim = self.read_rw(&self.settings).clone();
                    sim.profiles.remove(profile_id);
                    for target in sim.state_map.values_mut() {
                        if target == profile_id {
                            *target = fallback_id.clone();
                        }
                    }
                    sim.get_active_profile_for_mode(&apply_state)
                        .cloned()
                        .unwrap_or_else(default_office_profile)
                };
                match self.apply_profile_txn(
                    &fallback_profile,
                    true,
                    None,
                    Some(&apply_state),
                    None,
                    None,
                    Some(profile_id),
                ) {
                    ApplyOutcome::Committed => {
                        if !prev_active.odm_power_limits.is_empty()
                            && fallback_profile.odm_power_limits.is_empty()
                            && self.platform.is_available()
                        {
                            let _guard = self.lock_mutex(&self.hw_reconcile_lock);
                            if self.read_rw(&self.active_profile).id == fallback_profile.id {
                                let init_tdps = self.lock_mutex(&self.initial_tdp_defaults).clone();
                                let channels = prev_active
                                    .odm_power_limits
                                    .len()
                                    .min(self.capabilities.tdp_channels);
                                for idx in 0..channels {
                                    let fallback_w = init_tdps
                                        .as_ref()
                                        .and_then(|v| v.get(idx).copied().flatten())
                                        .or_else(|| {
                                            self.platform
                                                .read_tdp_limits(idx)
                                                .ok()
                                                .map(|(_, max)| max)
                                        });
                                    if let Some(default_w) = fallback_w {
                                        let _ = self.write_tdp_verified(idx, default_w);
                                    }
                                }
                            }
                        }
                        return true;
                    }
                    ApplyOutcome::StaleRevision | ApplyOutcome::SupersededReconciled => continue,
                    ApplyOutcome::Failed => return false,
                }
            }

            let _guard = self.lock_mutex(&self.hw_reconcile_lock);
            if self.shutting_down.load(Ordering::SeqCst) {
                return false;
            }
            if self.read_rw(&self.active_profile).id == profile_id {
                continue;
            }
            return self
                .update_settings(|s| {
                    s.profiles.remove(profile_id)?;
                    for target in s.state_map.values_mut() {
                        if target == profile_id {
                            *target = fallback_id.clone();
                        }
                    }
                    Some(())
                })
                .is_some();
        }
        false
    }
}
