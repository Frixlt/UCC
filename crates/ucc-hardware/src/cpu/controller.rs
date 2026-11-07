// SPDX-License-Identifier: GPL-3.0-or-later
use super::topology::parse_cpu_range_list;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Supported CPU scaling drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScalingDriver {
    AcpiCpufreq,
    IntelPstate,
    AmdPstate,
    AmdPstateEpp,
    Unknown,
}

impl ScalingDriver {
    pub fn from_driver_str(s: &str) -> Self {
        match s.trim() {
            "acpi-cpufreq" => Self::AcpiCpufreq,
            "intel_pstate" => Self::IntelPstate,
            "amd-pstate" => Self::AmdPstate,
            "amd-pstate-epp" => Self::AmdPstateEpp,
            _ => Self::Unknown,
        }
    }
}

/// Controller for a single logical CPU core via sysfs.
#[derive(Debug, Clone)]
pub struct LogicalCpu {
    pub base_path: PathBuf,
    pub core_index: u32,
    pub cpu_path: PathBuf,
    pub cpufreq_path: PathBuf,
}

impl LogicalCpu {
    pub fn new(base: impl Into<PathBuf>, index: u32) -> Self {
        let base_path = base.into();
        let cpu_path = base_path.join(format!("cpu{}", index));
        let cpufreq_path = cpu_path.join("cpufreq");
        Self {
            base_path,
            core_index: index,
            cpu_path,
            cpufreq_path,
        }
    }

    /// Whether this core has an /online node (mandatory core 0 does not).
    pub fn is_online_controllable(&self) -> bool {
        self.cpu_path.join("online").exists()
    }

    pub fn read_online(&self) -> Option<bool> {
        let path = self.cpu_path.join("online");
        if !path.exists() {
            return Some(true); // Non-hotpluggable core 0 is always online
        }
        let content = fs::read_to_string(path).ok()?;
        match content.trim() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        }
    }

    pub fn write_online(&self, online: bool) -> Result<(), std::io::Error> {
        let path = self.cpu_path.join("online");
        if !path.exists() {
            return Ok(()); // Non-hotpluggable core 0
        }
        fs::write(path, if online { "1" } else { "0" })
    }

    pub fn read_scaling_cur_freq(&self) -> Option<u32> {
        read_u32(&self.cpufreq_path.join("scaling_cur_freq"))
    }

    pub fn read_scaling_min_freq(&self) -> Option<u32> {
        read_u32(&self.cpufreq_path.join("scaling_min_freq"))
    }

    pub fn write_scaling_min_freq(&self, freq: u32) -> Result<(), std::io::Error> {
        fs::write(self.cpufreq_path.join("scaling_min_freq"), freq.to_string())
    }

    pub fn read_scaling_max_freq(&self) -> Option<u32> {
        read_u32(&self.cpufreq_path.join("scaling_max_freq"))
    }

    pub fn write_scaling_max_freq(&self, freq: u32) -> Result<(), std::io::Error> {
        fs::write(self.cpufreq_path.join("scaling_max_freq"), freq.to_string())
    }

    pub fn read_scaling_available_frequencies(&self) -> Option<Vec<u32>> {
        let content = fs::read_to_string(self.cpufreq_path.join("scaling_available_frequencies")).ok()?;
        let freqs: Vec<u32> = content
            .split_whitespace()
            .filter_map(|w| w.parse::<u32>().ok())
            .collect();
        if freqs.is_empty() {
            None
        } else {
            Some(freqs)
        }
    }

    pub fn read_scaling_driver(&self) -> Option<String> {
        fs::read_to_string(self.cpufreq_path.join("scaling_driver"))
            .map(|s| s.trim().to_string())
            .ok()
    }

    pub fn read_energy_performance_available_preferences(&self) -> Option<Vec<String>> {
        let content = fs::read_to_string(self.cpufreq_path.join("energy_performance_available_preferences")).ok()?;
        let prefs: Vec<String> = content.split_whitespace().map(|s| s.to_string()).collect();
        if prefs.is_empty() {
            None
        } else {
            Some(prefs)
        }
    }

    pub fn read_energy_performance_preference(&self) -> Option<String> {
        fs::read_to_string(self.cpufreq_path.join("energy_performance_preference"))
            .map(|s| s.trim().to_string())
            .ok()
    }

    pub fn write_energy_performance_preference(&self, epp: &str) -> Result<(), std::io::Error> {
        fs::write(self.cpufreq_path.join("energy_performance_preference"), epp)
    }

    pub fn read_scaling_available_governors(&self) -> Option<Vec<String>> {
        let content = fs::read_to_string(self.cpufreq_path.join("scaling_available_governors")).ok()?;
        let govs: Vec<String> = content.split_whitespace().map(|s| s.to_string()).collect();
        if govs.is_empty() {
            None
        } else {
            Some(govs)
        }
    }

    pub fn read_scaling_governor(&self) -> Option<String> {
        fs::read_to_string(self.cpufreq_path.join("scaling_governor"))
            .map(|s| s.trim().to_string())
            .ok()
    }

    pub fn write_scaling_governor(&self, gov: &str) -> Result<(), std::io::Error> {
        fs::write(self.cpufreq_path.join("scaling_governor"), gov)
    }

    pub fn read_cpuinfo_min_freq(&self) -> Option<u32> {
        read_u32(&self.cpufreq_path.join("cpuinfo_min_freq"))
    }

    pub fn read_cpuinfo_max_freq(&self) -> Option<u32> {
        read_u32(&self.cpufreq_path.join("cpuinfo_max_freq"))
    }

    pub fn read_base_frequency(&self) -> Option<u32> {
        read_u32(&self.cpufreq_path.join("base_frequency"))
    }

    /// Get reduced available frequency (middle of available frequency list, or average of min and max).
    pub fn get_reduced_available_freq(&self) -> Option<u32> {
        if let Some(avail) = self.read_scaling_available_frequencies() {
            if !avail.is_empty() {
                return Some(avail[avail.len() / 2]);
            }
        }
        let min = self.read_cpuinfo_min_freq()?;
        let max = self.read_cpuinfo_max_freq()?;
        Some((min + max) / 2)
    }
}

/// High-level CPU Controller managing all logical cores, frequency clamping, and core scheduling.
#[derive(Debug, Clone)]
pub struct CpuController {
    pub root_path: PathBuf,
    pub cores: Vec<LogicalCpu>,
}

impl Default for CpuController {
    fn default() -> Self {
        Self::new("/sys/devices/system/cpu")
    }
}

impl CpuController {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let mut controller = Self {
            root_path: root.into(),
            cores: Vec::new(),
        };
        controller.refresh_available_cores();
        controller
    }

    /// Discover and populate logical CPU cores from present and possible sysfs files.
    pub fn refresh_available_cores(&mut self) {
        self.cores.clear();

        let possible = fs::read_to_string(self.root_path.join("possible"))
            .map(|s| parse_cpu_range_list(&s))
            .unwrap_or_default();
        let present = fs::read_to_string(self.root_path.join("present"))
            .map(|s| parse_cpu_range_list(&s))
            .unwrap_or_default();

        let mut to_add: Vec<u32> = possible
            .into_iter()
            .filter(|id| present.contains(id))
            .collect();
        to_add.sort_unstable();

        for id in to_add {
            self.cores.push(LogicalCpu::new(&self.root_path, id));
        }
    }

    /// Validate the requested core selection:
    /// - Non-empty
    /// - No duplicate IDs
    /// - All core IDs exist in available cores
    /// - Mandatory cores (without /online file, e.g. core 0) cannot be omitted
    pub fn validate_enabled_cores(&self, selection: &[u32]) -> Result<(), String> {
        if selection.is_empty() {
            return Err("CPU selection is empty or contains duplicates".to_string());
        }

        let mut seen = HashSet::new();
        for &id in selection {
            if !seen.insert(id) {
                return Err("CPU selection is empty or contains duplicates".to_string());
            }
        }

        for &id in selection {
            if !self.cores.iter().any(|c| c.core_index == id) {
                return Err(format!("Unknown CPU {}", id));
            }
        }

        for core in &self.cores {
            if !core.is_online_controllable() && !seen.contains(&core.core_index) {
                return Err(format!("CPU {} cannot be disabled", core.core_index));
            }
        }

        Ok(())
    }

    /// Set enabled cores using strict enable-before-disable scheduling.
    ///
    /// If enabling any core fails, execution stops immediately without disabling any online cores.
    pub fn set_enabled_cores<W>(&self, selection: &[u32], mut writer: W) -> Result<(), String>
    where
        W: FnMut(&LogicalCpu, bool) -> bool,
    {
        self.validate_enabled_cores(selection)?;

        let target_set: HashSet<u32> = selection.iter().copied().collect();

        // Pass 1: Bring all requested CPUs online first
        for core in &self.cores {
            if !core.is_online_controllable() {
                continue;
            }
            if target_set.contains(&core.core_index) {
                let currently_online = core.read_online().unwrap_or(false);
                if !currently_online && !writer(core, true) {
                    return Err(format!("Cannot change CPU {}", core.core_index));
                }
            }
        }

        // Pass 2: Disable unselected CPUs only after all requested CPUs are confirmed online
        for core in &self.cores {
            if !core.is_online_controllable() {
                continue;
            }
            if !target_set.contains(&core.core_index) {
                let currently_online = core.read_online().unwrap_or(true);
                if currently_online && !writer(core, false) {
                    return Err(format!("Cannot change CPU {}", core.core_index));
                }
            }
        }

        Ok(())
    }

    /// Use the specified number of online cores (defaulting to all cores).
    pub fn use_cores(&self, number_of_cores: Option<u32>) -> Result<(), String> {
        if self.cores.is_empty() {
            return Ok(());
        }
        let total = self.cores.len() as u32;
        let count = number_of_cores.unwrap_or(total).clamp(1, total);

        let mut selection = Vec::new();
        for (i, core) in self.cores.iter().enumerate() {
            if (i as u32) < count || !core.is_online_controllable() {
                selection.push(core.core_index);
            }
        }

        self.set_enabled_cores(&selection, |core, enable| core.write_online(enable).is_ok())
    }

    /// Effective hardware maximum frequency for a core.
    ///
    /// If intel_pstate/no_turbo is enabled, maximum frequency is clamped to base_frequency.
    pub fn effective_hardware_max_freq(&self, core: &LogicalCpu) -> Option<u32> {
        let mut maximum = core.read_cpuinfo_max_freq()?;
        if self.read_intel_pstate_no_turbo().unwrap_or(false) {
            if let Some(base) = core.read_base_frequency() {
                if base > 0 {
                    maximum = maximum.min(base);
                }
            }
        }
        Some(maximum)
    }

    /// Compute the effective max scaling frequency that will be written to a core.
    ///
    /// Handles per-core clamping, no_turbo base frequency limits, and snaps to nearest available frequency.
    pub fn compute_effective_max_freq(
        &self,
        core: &LogicalCpu,
        target_max: Option<i64>,
        acpi_fallback: bool,
    ) -> Option<u32> {
        if !core.cpufreq_path.join("scaling_min_freq").exists()
            || !core.cpufreq_path.join("scaling_max_freq").exists()
            || !core.cpufreq_path.join("cpuinfo_min_freq").exists()
            || !core.cpufreq_path.join("cpuinfo_max_freq").exists()
        {
            return None;
        }

        let core_max = self.effective_hardware_max_freq(core)?;
        let mut scaling_min = core.read_scaling_min_freq()?;
        scaling_min = scaling_min.min(core_max);

        let mut freq = match target_max {
            None => core_max,
            Some(-1) => {
                if acpi_fallback {
                    core_max
                } else {
                    core.get_reduced_available_freq().unwrap_or(core_max)
                }
            }
            Some(val) => val.clamp(0, u32::MAX as i64) as u32,
        };

        // Clamp to per-core limits
        freq = freq.clamp(scaling_min, core_max);

        // Snap to closest available frequency >= scaling_min
        if let Some(avail) = core.read_scaling_available_frequencies() {
            let filtered: Vec<u32> = avail.into_iter().filter(|&f| f >= scaling_min).collect();
            if !filtered.is_empty() {
                freq = find_closest_value(freq, &filtered);
            }
        }

        Some(freq)
    }

    /// Compute the effective min scaling frequency that will be written to a core.
    pub fn compute_effective_min_freq(
        &self,
        core: &LogicalCpu,
        target_min: Option<i64>,
    ) -> Option<u32> {
        if !core.cpufreq_path.join("scaling_min_freq").exists()
            || !core.cpufreq_path.join("scaling_max_freq").exists()
            || !core.cpufreq_path.join("cpuinfo_min_freq").exists()
            || !core.cpufreq_path.join("cpuinfo_max_freq").exists()
        {
            return None;
        }

        let core_min = core.read_cpuinfo_min_freq()?;
        let core_max = self.effective_hardware_max_freq(core)?;
        let mut scaling_max = core.read_scaling_max_freq()?;
        scaling_max = scaling_max.min(core_max);

        if core_min > scaling_max {
            return None;
        }

        let mut freq = match target_min {
            None => core_min,
            Some(-2) => core_max,
            Some(val) => (val.clamp(0, u32::MAX as i64) as u32).clamp(core_min, scaling_max),
        };

        // Snap to closest available frequency <= scaling_max
        if let Some(avail) = core.read_scaling_available_frequencies() {
            let filtered: Vec<u32> = avail.into_iter().filter(|&f| f <= scaling_max).collect();
            if !filtered.is_empty() {
                freq = find_closest_value(freq, &filtered);
            }
        }

        Some(freq)
    }

    /// Set maximum scaling frequency for all online cores.
    pub fn set_governor_scaling_max_frequency(&self, set_max_frequency: Option<i64>) {
        let mut scaling_driver_str: Option<String> = None;
        let mut acpi_fallback = false;

        for core in &self.cores {
            if !core.cpufreq_path.join("scaling_min_freq").exists()
                || !core.cpufreq_path.join("scaling_max_freq").exists()
                || !core.cpufreq_path.join("cpuinfo_min_freq").exists()
                || !core.cpufreq_path.join("cpuinfo_max_freq").exists()
            {
                continue;
            }

            if core.core_index != 0 && !core.read_online().unwrap_or(false) {
                continue;
            }

            if scaling_driver_str.is_none() {
                scaling_driver_str = core.read_scaling_driver();
                acpi_fallback = self.has_boost()
                    && scaling_driver_str.as_deref() == Some("acpi-cpufreq");
            }

            if let Some(effective) = self.compute_effective_max_freq(core, set_max_frequency, acpi_fallback) {
                let _ = core.write_scaling_max_freq(effective);
            }
        }

        // Handle boost for acpi-cpufreq
        if !self.cores.is_empty() {
            let max_frequency = self.cores[0].read_cpuinfo_max_freq().unwrap_or(0);
            let available_frequencies = self.cores[0].read_scaling_available_frequencies();
            let maximum_available = available_frequencies
                .as_ref()
                .and_then(|v| v.first().copied())
                .unwrap_or(max_frequency);

            if self.has_boost() && scaling_driver_str.as_deref() == Some("acpi-cpufreq") {
                let enable_boost = match set_max_frequency {
                    None => true,
                    Some(req) => req > maximum_available as i64,
                };
                let _ = self.write_boost(enable_boost);
            }
        }
    }

    /// Set minimum scaling frequency for all online cores.
    pub fn set_governor_scaling_min_frequency(&self, set_min_frequency: Option<i64>) {
        for core in &self.cores {
            if !core.cpufreq_path.join("scaling_min_freq").exists()
                || !core.cpufreq_path.join("scaling_max_freq").exists()
                || !core.cpufreq_path.join("cpuinfo_min_freq").exists()
                || !core.cpufreq_path.join("cpuinfo_max_freq").exists()
            {
                continue;
            }

            if core.core_index != 0 && !core.read_online().unwrap_or(false) {
                continue;
            }

            if let Some(effective) = self.compute_effective_min_freq(core, set_min_frequency) {
                let _ = core.write_scaling_min_freq(effective);
            }
        }
    }

    /// Set scaling governor on all online cores.
    pub fn set_governor(&self, governor: Option<&str>) {
        let Some(gov) = governor else { return };
        for core in &self.cores {
            if core.core_index != 0 && !core.read_online().unwrap_or(false) {
                continue;
            }
            if let Some(avail) = core.read_scaling_available_governors() {
                if avail.iter().any(|g| g == gov) {
                    let _ = core.write_scaling_governor(gov);
                }
            }
        }
    }

    /// Set energy performance preference on all online cores.
    pub fn set_energy_performance_preference(&self, preference: Option<&str>) {
        let Some(pref) = preference else { return };
        for core in &self.cores {
            if core.core_index != 0 && !core.read_online().unwrap_or(false) {
                continue;
            }
            if let Some(avail) = core.read_energy_performance_available_preferences() {
                if avail.iter().any(|p| p == pref) {
                    let _ = core.write_energy_performance_preference(pref);
                }
            }
        }
    }

    pub fn read_intel_pstate_no_turbo(&self) -> Option<bool> {
        let path = self.root_path.join("intel_pstate/no_turbo");
        let content = fs::read_to_string(path).ok()?;
        match content.trim() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        }
    }

    pub fn write_intel_pstate_no_turbo(&self, val: bool) -> Result<(), std::io::Error> {
        fs::write(self.root_path.join("intel_pstate/no_turbo"), if val { "1" } else { "0" })
    }

    pub fn read_intel_hwp_dynamic_boost(&self) -> Option<bool> {
        let path = self.root_path.join("intel_pstate/hwp_dynamic_boost");
        let content = fs::read_to_string(path).ok()?;
        match content.trim() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        }
    }

    pub fn write_intel_hwp_dynamic_boost(&self, val: bool) -> Result<(), std::io::Error> {
        fs::write(self.root_path.join("intel_pstate/hwp_dynamic_boost"), if val { "1" } else { "0" })
    }

    pub fn has_boost(&self) -> bool {
        self.root_path.join("cpufreq/boost").exists()
    }

    pub fn write_boost(&self, val: bool) -> Result<(), std::io::Error> {
        fs::write(self.root_path.join("cpufreq/boost"), if val { "1" } else { "0" })
    }
}

/// Find closest value in a slice of frequencies.
pub fn find_closest_value(target: u32, values: &[u32]) -> u32 {
    if values.is_empty() {
        return target;
    }
    let mut closest = values[0];
    let mut min_diff = (target as i64 - values[0] as i64).abs();
    for &val in values.iter().skip(1) {
        let diff = (target as i64 - val as i64).abs();
        if diff < min_diff {
            min_diff = diff;
            closest = val;
        }
    }
    closest
}

fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse::<u32>().ok()
}
