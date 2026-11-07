// SPDX-License-Identifier: GPL-3.0-or-later
use super::controller::{CpuController, LogicalCpu};
use super::topology::CpuTopology;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use ucc_core::UccProfile;

pub const MAX_REAPPLY_ATTEMPTS: u32 = 3;

/// Manages CPU profile application, periodic validation, and graceful restoration.
#[derive(Debug, Clone)]
pub struct CpuWorker {
    pub cpu_ctrl: CpuController,
    pub topology: CpuTopology,
    pub system_default_governor: Option<String>,
    pub system_default_epp: Option<String>,
    pub no_epp_write_quirk: bool,
    pub validation_failure_count: u32,
    pub reapply_gave_up: bool,
    warned_governors: HashSet<String>,
    warned_epps: HashSet<String>,
}

impl CpuWorker {
    pub fn new(cpu_root: impl Into<PathBuf>) -> Self {
        let root = cpu_root.into();
        let cpu_ctrl = CpuController::new(&root);
        let topology = CpuTopology::new(&root);

        let system_default_governor = if !cpu_ctrl.cores.is_empty() {
            cpu_ctrl.cores[0].read_scaling_governor()
        } else {
            None
        };

        let system_default_epp = if !cpu_ctrl.cores.is_empty() {
            cpu_ctrl.cores[0].read_energy_performance_preference()
        } else {
            None
        };

        Self {
            cpu_ctrl,
            topology,
            system_default_governor,
            system_default_epp,
            no_epp_write_quirk: false,
            validation_failure_count: 0,
            reapply_gave_up: false,
            warned_governors: HashSet::new(),
            warned_epps: HashSet::new(),
        }
    }

    pub fn root_path(&self) -> &Path {
        &self.cpu_ctrl.root_path
    }

    /// Apply CPU settings from profile.
    pub fn apply_cpu_profile(&mut self, profile: &UccProfile) -> Result<(), String> {
        let cpu = &profile.cpu;

        // 1. Validate and configure online cores
        if let Some(ref enabled) = cpu.enabled_cores {
            self.cpu_ctrl.set_enabled_cores(enabled, |core, val| core.write_online(val).is_ok())?;
        } else {
            self.cpu_ctrl.use_cores(cpu.online_cores)?;
        }

        // 2. Set intel_pstate flags before frequency bounds
        let _ = self.cpu_ctrl.write_intel_pstate_no_turbo(cpu.no_turbo);
        let _ = self.cpu_ctrl.write_intel_hwp_dynamic_boost(cpu.hwp_dynamic_boost);

        // 3. Reset frequency bounds on selected cores to avoid clamp conflicts
        self.cpu_ctrl.set_governor_scaling_min_frequency(None);
        self.cpu_ctrl.set_governor_scaling_max_frequency(None);

        if !self.no_epp_write_quirk {
            if let Some(ref default_epp) = self.system_default_epp {
                self.cpu_ctrl.set_energy_performance_preference(Some(default_epp));
            }
        }

        // 4. Resolve and apply governor
        let desired_governor = if !cpu.governor.is_empty() {
            Some(cpu.governor.as_str())
        } else {
            self.system_default_governor.as_deref()
        };

        if let Some(gov) = desired_governor {
            if self.is_governor_available(gov) {
                self.cpu_ctrl.set_governor(Some(gov));
            } else if self.warned_governors.insert(gov.to_string()) {
                warn!("CpuWorker: Governor '{}' is not available on this system", gov);
            }
        }

        // 5. Apply Energy Performance Preference (EPP)
        if !self.no_epp_write_quirk && !cpu.energy_performance_preference.is_empty() {
            let epp = &cpu.energy_performance_preference;
            if self.is_epp_available(epp) {
                self.cpu_ctrl.set_energy_performance_preference(Some(epp));

                // Verification check: detect EPP write rejection quirks
                if !self.cpu_ctrl.cores.is_empty() {
                    let first_core = &self.cpu_ctrl.cores[0];
                    if let Some(ref current_epp) = first_core.read_energy_performance_preference() {
                        if current_epp != epp && !self.intel_pstate_performance_owns_epp(first_core, epp, current_epp) {
                            self.no_epp_write_quirk = true;
                            warn!(
                                "CpuWorker: EPP write to sysfs rejected (requested '{}', got '{}'). Disabling EPP management",
                                epp, current_epp
                            );
                        } else if self.intel_pstate_performance_owns_epp(first_core, epp, current_epp) {
                            info!("CpuWorker: intel_pstate performance governor owns EPP; sysfs reports 'default'");
                        }
                    }
                }
            } else if self.warned_epps.insert(epp.clone()) {
                warn!("CpuWorker: EPP '{}' is not available on this system", epp);
            }
        }

        // 6. Write requested scaling frequencies
        self.cpu_ctrl.set_governor_scaling_min_frequency(cpu.min_freq_khz.map(|f| f as i64));
        self.cpu_ctrl.set_governor_scaling_max_frequency(cpu.max_freq_khz.map(|f| f as i64));

        // 7. Ensure turbo settings remain applied after frequency bounds writes
        let _ = self.cpu_ctrl.write_intel_pstate_no_turbo(cpu.no_turbo);
        let _ = self.cpu_ctrl.write_intel_hwp_dynamic_boost(cpu.hwp_dynamic_boost);

        Ok(())
    }

    /// Reset all CPU settings back to unconstrained system defaults.
    pub fn set_cpu_default_config(&mut self) {
        let _ = self.cpu_ctrl.write_intel_pstate_no_turbo(false);
        let _ = self.cpu_ctrl.write_intel_hwp_dynamic_boost(false);

        // Bring all cores online
        let _ = self.cpu_ctrl.use_cores(None);

        // Reset frequency limits to full hardware boundaries
        self.cpu_ctrl.set_governor_scaling_min_frequency(None);
        self.cpu_ctrl.set_governor_scaling_max_frequency(None);

        if !self.no_epp_write_quirk {
            if let Some(ref default_epp) = self.system_default_epp {
                self.cpu_ctrl.set_energy_performance_preference(Some(default_epp));
            }
        }

        let _ = self.cpu_ctrl.write_intel_pstate_no_turbo(false);
        let _ = self.cpu_ctrl.write_intel_hwp_dynamic_boost(false);
    }

    /// Check if current hardware state matches active profile.
    pub fn validate_cpu_freq(&self, profile: &UccProfile) -> bool {
        let cpu = &profile.cpu;

        if let Some(ref enabled) = cpu.enabled_cores {
            let enabled_set: HashSet<u32> = enabled.iter().copied().collect();
            for core in &self.cpu_ctrl.cores {
                let is_online = core.read_online().unwrap_or(true);
                let should_be_online = enabled_set.contains(&core.core_index);
                if is_online != should_be_online {
                    return false;
                }
            }
        }

        let acpi_fallback = self.cpu_ctrl.has_boost()
            && self.cpu_ctrl.cores.first()
                .and_then(|c| c.read_scaling_driver())
                .as_deref() == Some("acpi-cpufreq");

        for core in &self.cpu_ctrl.cores {
            if core.core_index == 0 || core.read_online().unwrap_or(false) {
                if let Some(req_min) = cpu.min_freq_khz {
                    let current_min = core.read_scaling_min_freq();
                    let expected_min = self.cpu_ctrl.compute_effective_min_freq(core, Some(req_min as i64));
                    if let (Some(cur), Some(exp)) = (current_min, expected_min) {
                        if cur != exp {
                            debug!("CpuWorker: Core {} min freq {} != expected {}", core.core_index, cur, exp);
                            return false;
                        }
                    }
                }

                if let Some(req_max) = cpu.max_freq_khz {
                    let current_max = core.read_scaling_max_freq();
                    let expected_max = self.cpu_ctrl.compute_effective_max_freq(core, Some(req_max as i64), acpi_fallback);
                    if let (Some(cur), Some(exp)) = (current_max, expected_max) {
                        if cur != exp {
                            debug!("CpuWorker: Core {} max freq {} != expected {}", core.core_index, cur, exp);
                            return false;
                        }
                    }
                }
            }
        }

        true
    }

    /// Periodic worker iteration.
    pub fn on_work(&mut self, profile: &UccProfile) -> bool {
        if self.validate_cpu_freq(profile) {
            if self.validation_failure_count > 0 {
                self.validation_failure_count = 0;
            }
            if self.reapply_gave_up {
                self.reapply_gave_up = false;
            }
            true
        } else {
            if self.reapply_gave_up {
                return false;
            }

            self.validation_failure_count += 1;
            if self.validation_failure_count >= MAX_REAPPLY_ATTEMPTS {
                warn!(
                    "CpuWorker: CPU settings keep being reverted by another service. Giving up after {} attempts.",
                    MAX_REAPPLY_ATTEMPTS
                );
                self.reapply_gave_up = true;
                return false;
            }

            info!(
                "CpuWorker: Incorrect settings, reapplying profile (attempt {}/{})",
                self.validation_failure_count, MAX_REAPPLY_ATTEMPTS
            );
            let _ = self.apply_cpu_profile(profile);
            false
        }
    }

    /// Reapply profile and reset retry tracking.
    pub fn reapply_profile(&mut self, profile: &UccProfile) -> Result<(), String> {
        self.validation_failure_count = 0;
        self.reapply_gave_up = false;
        self.apply_cpu_profile(profile)
    }

    fn is_governor_available(&self, governor: &str) -> bool {
        self.cpu_ctrl.cores.first()
            .and_then(|c| c.read_scaling_available_governors())
            .map(|govs| govs.iter().any(|g| g == governor))
            .unwrap_or(false)
    }

    fn is_epp_available(&self, epp: &str) -> bool {
        self.cpu_ctrl.cores.first()
            .and_then(|c| c.read_energy_performance_available_preferences())
            .map(|prefs| prefs.iter().any(|p| p == epp))
            .unwrap_or(false)
    }

    fn intel_pstate_performance_owns_epp(&self, core: &LogicalCpu, requested_epp: &str, current_epp: &str) -> bool {
        requested_epp == "performance"
            && current_epp == "default"
            && core.read_scaling_driver().as_deref() == Some("intel_pstate")
            && core.read_scaling_governor().as_deref() == Some("performance")
    }
}
