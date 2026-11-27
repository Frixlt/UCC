//! GPU sysfs readers (Intel `i915`/`xe`, AMD `hwmon`, NVIDIA D3cold cadence) (`M7.2`, `M7.5`).

use crate::cpu::parse_cpu_range_list;
use crate::nvml::NVIDIA_PCI_VENDOR_ID;
use crate::traits::Sysfs;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Hardware vendor for integrated/discrete GPUs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GpuVendor {
    Intel,
    Amd,
    Nvidia,
    #[default]
    Unknown,
}

impl GpuVendor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intel => "intel",
            Self::Amd => "amd",
            Self::Nvidia => "nvidia",
            Self::Unknown => "unknown",
        }
    }
}

/// Option-typed integrated GPU telemetry snapshot (`None` when a sensor is absent).
#[derive(Debug, Clone)]
pub struct IGpuSnapshot {
    pub vendor: GpuVendor,
    pub bdf: Option<String>,
    pub temp_c: Option<f64>,
    pub core_freq_mhz: Option<f64>,
    pub max_core_freq_mhz: Option<f64>,
    pub power_w: Option<f64>,
    pub collected_at: Instant,
}

/// Option-typed discrete GPU telemetry snapshot (`None` when sleeping or sensor unavailable).
#[derive(Debug, Clone)]
pub struct DGpuSnapshot {
    pub vendor: GpuVendor,
    pub bdf: Option<String>,
    pub is_sleeping: bool,
    pub from_cache: bool,
    pub d0_metrics_usage: bool,
    pub temp_c: Option<f64>,
    pub core_freq_mhz: Option<f64>,
    pub max_core_freq_mhz: Option<f64>,
    pub vram_freq_mhz: Option<f64>,
    pub power_w: Option<f64>,
    pub default_power_limit_w: Option<f64>,
    pub max_power_limit_w: Option<f64>,
    pub enforced_power_limit_w: Option<f64>,
    pub compute_util_pct: Option<u32>,
    pub memory_util_pct: Option<u32>,
    pub vram_used_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
    pub perf_limit_reason: Option<String>,
    pub encoder_util_pct: Option<u32>,
    pub decoder_util_pct: Option<u32>,
    pub current_pstate: Option<u32>,
    pub gr_clock_offset_mhz: Option<i32>,
    pub mem_clock_offset_mhz: Option<i32>,
    pub core_voltage_mv: Option<u32>,
    pub collected_at: Instant,
}

impl DGpuSnapshot {
    pub fn sleeping_nvidia(bdf: Option<String>, now: Instant) -> Self {
        Self {
            vendor: GpuVendor::Nvidia,
            bdf,
            is_sleeping: true,
            from_cache: false,
            d0_metrics_usage: false,
            temp_c: None,
            core_freq_mhz: None,
            max_core_freq_mhz: None,
            vram_freq_mhz: None,
            power_w: None,
            default_power_limit_w: None,
            max_power_limit_w: None,
            enforced_power_limit_w: None,
            compute_util_pct: None,
            memory_util_pct: None,
            vram_used_mib: None,
            vram_total_mib: None,
            perf_limit_reason: None,
            encoder_util_pct: None,
            decoder_util_pct: None,
            current_pstate: None,
            gr_clock_offset_mhz: None,
            mem_clock_offset_mhz: None,
            core_voltage_mv: None,
            collected_at: now,
        }
    }
}

fn read_i64(sysfs: &dyn Sysfs, path: &Path) -> Option<i64> {
    sysfs.read(path).ok()?.trim().parse::<i64>().ok()
}

/// Parses `PCI_SLOT_NAME=<bdf>` from a sysfs `uevent` file.
pub fn read_pci_slot_name(sysfs: &dyn Sysfs, uevent_path: &Path) -> Option<String> {
    let content = sysfs.read(uevent_path).ok()?;
    for line in content.lines() {
        if let Some(bdf) = line.trim().strip_prefix("PCI_SLOT_NAME=") {
            let trimmed = bdf.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn parse_max_amd_freq_mhz(content: &str) -> Option<f64> {
    let mut best: Option<f64> = None;
    for token in content.split(|c: char| !c.is_ascii_alphanumeric()) {
        if token.len() <= 3 {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if let Some(num_str) = lower.strip_suffix("mhz") {
            if let Ok(mhz) = num_str.parse::<f64>() {
                if mhz.is_finite() && mhz > 0.0 {
                    best = Some(best.map_or(mhz, |prev| prev.max(mhz)));
                }
            }
        }
    }
    best
}

/// Reads Intel iGPU frequency, max frequency, and strictly per-GPU hwmon sensors (`i915` or `xe`).
/// Never substitutes CPU package temperature for GPU temperature (`M7.2`).
pub fn read_intel_igpu_sysfs(sysfs: &dyn Sysfs, card_dir: &Path, now: Instant) -> IGpuSnapshot {
    let i915_cur = card_dir.join("gt_act_freq_mhz");
    let i915_multi_gt_cur = card_dir.join("gt/gt0/rps_act_freq_mhz");
    let (freq_path, fallback_freq_path, max_path) = if sysfs.exists(&i915_cur) {
        (
            i915_cur,
            card_dir.join("gt_cur_freq_mhz"),
            card_dir.join("gt_RP0_freq_mhz"),
        )
    } else if sysfs.exists(&i915_multi_gt_cur) {
        (
            i915_multi_gt_cur,
            card_dir.join("gt/gt0/rps_cur_freq_mhz"),
            card_dir.join("gt/gt0/rps_RP0_freq_mhz"),
        )
    } else {
        (
            card_dir.join("device/tile0/gt0/freq0/act_freq"),
            card_dir.join("device/tile0/gt0/freq0/cur_freq"),
            card_dir.join("device/tile0/gt0/freq0/rp0_freq"),
        )
    };

    // Intel xe/i915 drops `act_freq` to 0 during brief RC6 C-state idle windows while
    // `cur_freq` still reflects the requested active clock; fall back to `cur_freq` when > 0.
    let raw_act = read_i64(sysfs, &freq_path).filter(|&v| v >= 0);
    let raw_cur = read_i64(sysfs, &fallback_freq_path).filter(|&v| v > 0);
    let core_freq_mhz = match (raw_act, raw_cur) {
        (Some(v), _) if v > 0 => Some(v as f64),
        (_, Some(cur)) => Some(cur as f64),
        (Some(0), None) => Some(0.0),
        (None, None) => None,
        (Some(v), None) => Some(v as f64),
    };
    let max_core_freq_mhz = read_i64(sysfs, &max_path)
        .filter(|&v| v >= 0)
        .map(|v| v as f64);

    let mut temp_c = None;
    let mut power_w = None;

    if let Ok(mut hwmons) = sysfs.read_dir(&card_dir.join("device/hwmon")) {
        hwmons.sort();
        for h in hwmons {
            if temp_c.is_none() {
                if let Some(t) = read_i64(sysfs, &h.join("temp1_input")).filter(|&v| v >= 0) {
                    temp_c = Some(t as f64 / 1000.0);
                }
            }
            if power_w.is_none() {
                let raw_uw = read_i64(sysfs, &h.join("power1_average"))
                    .filter(|&v| v >= 0)
                    .or_else(|| read_i64(sysfs, &h.join("power1_input")).filter(|&v| v >= 0));
                if let Some(uw) = raw_uw {
                    power_w = Some(uw as f64 / 1_000_000.0);
                }
            }
        }
    }

    let bdf = read_pci_slot_name(sysfs, &card_dir.join("device/uevent"));

    IGpuSnapshot {
        vendor: GpuVendor::Intel,
        bdf,
        temp_c,
        core_freq_mhz,
        max_core_freq_mhz,
        power_w,
        collected_at: now,
    }
}

/// Reads cumulative microjoule counter from Intel iGPU `hwmon*/energy1_input` (`ISSUE-22`).
pub fn read_intel_igpu_energy_uj(sysfs: &dyn Sysfs, card_dir: &Path) -> Option<u64> {
    let mut hwmons = sysfs.read_dir(&card_dir.join("device/hwmon")).ok()?;
    hwmons.sort();
    for h in hwmons {
        if let Some(val) = read_i64(sysfs, &h.join("energy1_input")).filter(|&v| v >= 0) {
            return Some(val as u64);
        }
    }
    None
}

/// Reads AMD iGPU telemetry from `hwmon` (`temp1_input`, `freq1_input`, `power1_average`/`power1_input`),
/// `device/pp_dpm_sclk` max frequency, and PCI BDF from `device/uevent`.
pub fn read_amd_igpu_sysfs(sysfs: &dyn Sysfs, hwmon_dir: &Path, now: Instant) -> IGpuSnapshot {
    let (temp_c, power_w, core_freq_mhz) = read_amd_gpu_hwmon(sysfs, hwmon_dir);
    let max_core_freq_mhz = sysfs
        .read(&hwmon_dir.join("device/pp_dpm_sclk"))
        .ok()
        .as_deref()
        .and_then(parse_max_amd_freq_mhz);
    let bdf = read_pci_slot_name(sysfs, &hwmon_dir.join("device/uevent"));

    IGpuSnapshot {
        vendor: GpuVendor::Amd,
        bdf,
        temp_c,
        core_freq_mhz,
        max_core_freq_mhz,
        power_w,
        collected_at: now,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardPackageLookup {
    Unique(u32),
    Ambiguous,
    Unknown,
}

fn parse_rapl_package_id(sysfs: &dyn Sysfs, powercap_root: &Path, entry: &Path) -> Option<u32> {
    let fname = entry.file_name()?.to_str()?;
    let rest = fname.strip_prefix("intel-rapl:")?;
    let pkg_part = rest.split(':').next()?;
    let parent_name_path = powercap_root.join(format!("intel-rapl:{pkg_part}/name"));
    if let Ok(parent_name) = sysfs.read(&parent_name_path) {
        if let Some(idx_str) = parent_name.trim().strip_prefix("package-") {
            if let Ok(pkg) = idx_str.trim().parse::<u32>() {
                return Some(pkg);
            }
        }
    }
    pkg_part.parse::<u32>().ok()
}

fn resolve_card_cpu_package_id(
    sysfs: &dyn Sysfs,
    sys_root: &Path,
    card_dir: &Path,
) -> CardPackageLookup {
    let Ok(cpulist) = sysfs.read(&card_dir.join("device/local_cpulist")) else {
        return CardPackageLookup::Unknown;
    };
    let cpus = parse_cpu_range_list(cpulist.trim());
    if cpus.is_empty() {
        return CardPackageLookup::Unknown;
    }
    let mut packages = BTreeSet::new();
    for cpu_id in cpus {
        let pkg_path = sys_root.join(format!(
            "sys/devices/system/cpu/cpu{cpu_id}/topology/physical_package_id"
        ));
        let Ok(raw) = sysfs.read(&pkg_path) else {
            return CardPackageLookup::Ambiguous;
        };
        let Ok(pkg) = raw.trim().parse::<u32>() else {
            return CardPackageLookup::Ambiguous;
        };
        packages.insert(pkg);
    }
    match packages.len() {
        0 => CardPackageLookup::Unknown,
        1 => CardPackageLookup::Unique(*packages.iter().next().unwrap()),
        _ => CardPackageLookup::Ambiguous,
    }
}

fn list_intel_gpu_rapl_candidates(sysfs: &dyn Sysfs, powercap_root: &Path) -> Vec<PathBuf> {
    let Ok(mut entries) = sysfs.read_dir(powercap_root) else {
        return Vec::new();
    };
    entries.sort();
    let mut out = Vec::new();
    for entry in entries {
        let Some(fname) = entry.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !fname.starts_with("intel-rapl:") {
            continue;
        }
        if let Ok(name) = sysfs.read(&entry.join("name")) {
            let trimmed = name.trim();
            if matches!(trimmed, "uncore" | "gpu" | "graphics") {
                out.push(entry);
            }
        }
    }
    out
}

/// Finds an unambiguous Intel GPU RAPL domain (`uncore`, `gpu`, `graphics`) under powercap root.
/// Returns `None` when 0 or >1 candidate domains exist so multi-package hosts fail closed.
pub fn find_intel_gpu_rapl_path(sysfs: &dyn Sysfs, powercap_root: &Path) -> Option<PathBuf> {
    let mut candidates = list_intel_gpu_rapl_candidates(sysfs, powercap_root);
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

/// Finds the Intel GPU RAPL domain matching `card_dir`'s CPU package (`local_cpulist` ->
/// `physical_package_id`), failing closed to `None` when multi-package domains are ambiguous.
pub fn find_intel_gpu_rapl_path_for_card(
    sysfs: &dyn Sysfs,
    sys_root: &Path,
    card_dir: &Path,
) -> Option<PathBuf> {
    let powercap_root = sys_root.join("sys/class/powercap");
    let candidates = list_intel_gpu_rapl_candidates(sysfs, &powercap_root);
    if candidates.is_empty() {
        return None;
    }

    match resolve_card_cpu_package_id(sysfs, sys_root, card_dir) {
        CardPackageLookup::Unique(target_pkg) => {
            let mut matched: Vec<PathBuf> = candidates
                .into_iter()
                .filter(|entry| {
                    parse_rapl_package_id(sysfs, &powercap_root, entry) == Some(target_pkg)
                })
                .collect();
            if matched.len() == 1 {
                matched.pop()
            } else {
                None
            }
        }
        CardPackageLookup::Ambiguous => None,
        CardPackageLookup::Unknown => {
            if candidates.len() == 1 {
                candidates.into_iter().next()
            } else {
                None
            }
        }
    }
}

/// Reads AMD GPU `(temp_c, power_w, freq_mhz)` from a specific `hwmon` directory.
pub fn read_amd_gpu_hwmon(
    sysfs: &dyn Sysfs,
    hwmon_dir: &Path,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let temp_c = read_i64(sysfs, &hwmon_dir.join("temp1_input"))
        .filter(|&v| v >= 0)
        .map(|v| v as f64 / 1000.0);

    let power_w = read_i64(sysfs, &hwmon_dir.join("power1_average"))
        .filter(|&v| v >= 0)
        .or_else(|| read_i64(sysfs, &hwmon_dir.join("power1_input")).filter(|&v| v >= 0))
        .map(|v| v as f64 / 1_000_000.0);

    let freq_mhz = read_i64(sysfs, &hwmon_dir.join("freq1_input"))
        .filter(|&v| v >= 0)
        .map(|hz| hz as f64 / 1_000_000.0);

    (temp_c, power_w, freq_mhz)
}

/// Checks whether a specific NVIDIA PCI display device is awake and due for polling on `cycle`.
pub fn nvidia_device_telemetry_due(sysfs: &dyn Sysfs, cycle: u32, dev_path: &Path) -> bool {
    let Ok(status) = sysfs.read(&dev_path.join("power/runtime_status")) else {
        return false;
    };
    if status.trim() != "active" {
        return false;
    }
    let control = sysfs
        .read(&dev_path.join("power/control"))
        .unwrap_or_default()
        .trim()
        .to_string();
    control == "on" || cycle.is_multiple_of(20)
}

/// Determines whether an NVIDIA NVML poll is permitted on `cycle` without breaking D3cold sleep (`M7.5`).
///
/// Mirrors `GpuSysfs::nvidiaTelemetryDue`:
/// - Suspended or unknown `runtime_status` -> `false`.
/// - `active` with `power/control == "on"` -> `true` every cycle.
/// - `active` with `power/control == "auto"` -> `true` only when `cycle % 20 == 0`
///   (~16s idle window at 800 ms cycle cadence so D3cold autosuspend can trigger).
pub fn nvidia_telemetry_due(sysfs: &dyn Sysfs, cycle: u32, pci_root: &Path) -> bool {
    let Ok(mut entries) = sysfs.read_dir(pci_root) else {
        return false;
    };
    entries.sort();
    for base in entries {
        let Ok(vendor) = sysfs.read(&base.join("vendor")) else {
            continue;
        };
        if !vendor.trim().eq_ignore_ascii_case(NVIDIA_PCI_VENDOR_ID) {
            continue;
        }
        let Ok(class) = sysfs.read(&base.join("class")) else {
            continue;
        };
        let cls = class.trim().to_ascii_lowercase();
        if !cls.starts_with("0x03") && !cls.starts_with("03") {
            continue;
        }
        if nvidia_device_telemetry_due(sysfs, cycle, &base) {
            return true;
        }
        let Ok(status) = sysfs.read(&base.join("power/runtime_status")) else {
            continue;
        };
        if status.trim() == "active" {
            return false;
        }
    }
    false
}
