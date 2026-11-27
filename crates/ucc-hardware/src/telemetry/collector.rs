//! Unified telemetry collector, freshness tracking, and IPC sentinel boundary (`M7.3`–`M7.5`, `M7.7`).

use crate::nvml::{NvmlWrapper, NVIDIA_PCI_VENDOR_ID};
use crate::telemetry::cpu::{CpuTelemetryReader, CpuTelemetrySnapshot, RaplEnergyCounter};
use crate::telemetry::gpu::{
    find_intel_gpu_rapl_path_for_card, nvidia_device_telemetry_due, read_amd_gpu_hwmon,
    read_amd_igpu_sysfs, read_intel_igpu_energy_uj, read_intel_igpu_sysfs, read_pci_slot_name,
    DGpuSnapshot, GpuVendor, IGpuSnapshot,
};
use crate::telemetry::history::{MetricId, MetricsHistoryStore};
use crate::traits::Sysfs;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Complete telemetry snapshot across CPU, integrated GPU, and discrete GPU.
#[derive(Debug, Clone)]
pub struct TelemetrySnapshot {
    pub timestamp_ms: i64,
    pub collected_at: Instant,
    pub cpu: CpuTelemetrySnapshot,
    pub igpu: Option<IGpuSnapshot>,
    pub dgpu: Option<DGpuSnapshot>,
}

impl TelemetrySnapshot {
    /// Returns `true` when every included source (CPU, iGPU, active dGPU) and the bundle timestamp
    /// are no older than `max_age` relative to `now`.
    pub fn is_fresh(&self, now: Instant, max_age: Duration) -> bool {
        now.saturating_duration_since(self.collected_at) <= max_age
            && self.is_cpu_fresh(now, max_age)
            && self
                .igpu
                .as_ref()
                .is_none_or(|g| now.saturating_duration_since(g.collected_at) <= max_age)
            && self
                .dgpu
                .as_ref()
                .is_none_or(|d| now.saturating_duration_since(d.collected_at) <= max_age)
    }

    pub fn is_cpu_fresh(&self, now: Instant, max_age: Duration) -> bool {
        now.saturating_duration_since(self.cpu.collected_at) <= max_age
    }

    pub fn is_igpu_fresh(&self, now: Instant, max_age: Duration) -> bool {
        self.igpu
            .as_ref()
            .is_some_and(|g| now.saturating_duration_since(g.collected_at) <= max_age)
    }

    pub fn is_dgpu_fresh(&self, now: Instant, max_age: Duration) -> bool {
        self.dgpu
            .as_ref()
            .is_some_and(|d| now.saturating_duration_since(d.collected_at) <= max_age)
    }

    pub fn dgpu_from_cache(&self) -> bool {
        self.dgpu.as_ref().is_some_and(|d| d.from_cache)
    }
}

/// Converts [`DGpuSnapshot`] to legacy D-Bus JSON with exact C++ sentinel keys (`M7.4`, `M7-A07`).
pub fn dgpu_to_legacy_json(sample: &DGpuSnapshot) -> Value {
    json!({
        "vendor": sample.vendor.as_str(),
        "isSleeping": sample.is_sleeping,
        "d0MetricsUsage": sample.d0_metrics_usage,
        "temp": sample.temp_c.unwrap_or(-1.0),
        "coreFrequency": sample.core_freq_mhz.unwrap_or(-1.0),
        "maxCoreFrequency": sample.max_core_freq_mhz.unwrap_or(-1.0),
        "vramFrequency": sample.vram_freq_mhz.unwrap_or(-1.0),
        "powerDraw": sample.power_w.unwrap_or(-1.0),
        "maxPowerLimit": sample.max_power_limit_w.unwrap_or(-1.0),
        "enforcedPowerLimit": sample.enforced_power_limit_w.unwrap_or(-1.0),
        "computeUtilPct": sample.compute_util_pct.map(i64::from).unwrap_or(-1),
        "memoryUtilPct": sample.memory_util_pct.map(i64::from).unwrap_or(-1),
        "vramUsedMiB": sample.vram_used_mib.map(|v| v as i64).unwrap_or(-1),
        "vramTotalMiB": sample.vram_total_mib.map(|v| v as i64).unwrap_or(-1),
        "perfLimitReason": sample.perf_limit_reason.as_deref().unwrap_or(""),
        "encoderUtilPct": sample.encoder_util_pct.map(i64::from).unwrap_or(-1),
        "decoderUtilPct": sample.decoder_util_pct.map(i64::from).unwrap_or(-1),
        "currentPstate": sample.current_pstate.map(i64::from).unwrap_or(-1),
        "grClockOffsetMHz": sample.gr_clock_offset_mhz.map(i64::from).unwrap_or(-999),
        "memClockOffsetMHz": sample.mem_clock_offset_mhz.map(i64::from).unwrap_or(-999),
        "coreVoltageMv": sample.core_voltage_mv.map(i64::from).unwrap_or(-1),
    })
}

/// Converts [`IGpuSnapshot`] to legacy D-Bus JSON with `-1` sentinels strictly at the IPC boundary.
pub fn igpu_to_legacy_json(sample: &IGpuSnapshot) -> Value {
    json!({
        "vendor": sample.vendor.as_str(),
        "temp": sample.temp_c.unwrap_or(-1.0),
        "coreFrequency": sample.core_freq_mhz.unwrap_or(-1.0),
        "maxCoreFrequency": sample.max_core_freq_mhz.unwrap_or(-1.0),
        "powerDraw": sample.power_w.unwrap_or(-1.0),
    })
}

/// Configuration for [`TelemetryCollector`].
pub struct TelemetryConfig {
    pub sys_root: PathBuf,
    pub intel_card_path: Option<PathBuf>,
    pub amd_igpu_hwmon: Option<PathBuf>,
    pub amd_dgpu_hwmon: Option<PathBuf>,
    pub nvml: Option<Arc<NvmlWrapper>>,
    pub dgpu_cache_max_age: Duration,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            sys_root: PathBuf::from("/"),
            intel_card_path: None,
            amd_igpu_hwmon: None,
            amd_dgpu_hwmon: None,
            nvml: None,
            dgpu_cache_max_age: Duration::from_secs(20),
        }
    }
}

/// Coordinates CPU/RAPL, iGPU, guarded NVML dGPU polling, and 10-series history storage (`M7.3`–`M7.7`).
pub struct TelemetryCollector {
    sysfs: Arc<dyn Sysfs>,
    config: TelemetryConfig,
    cpu_reader: CpuTelemetryReader,
    intel_igpu_rapl: Option<RaplEnergyCounter>,
    intel_prev_hwmon_energy: Option<(u64, Instant)>,
    history: Arc<MetricsHistoryStore>,
    cycle_counter: u32,
    nvidia_cache: HashMap<String, DGpuSnapshot>,
    dgpu_d0_metrics: bool,
}

impl TelemetryCollector {
    pub fn new(
        sysfs: Arc<dyn Sysfs>,
        config: TelemetryConfig,
        history: Arc<MetricsHistoryStore>,
    ) -> Self {
        let cpu_reader = CpuTelemetryReader::new(&config.sys_root);
        Self {
            sysfs,
            config,
            cpu_reader,
            intel_igpu_rapl: None,
            intel_prev_hwmon_energy: None,
            history,
            cycle_counter: 0,
            nvidia_cache: HashMap::new(),
            dgpu_d0_metrics: true,
        }
    }

    /// Enables or disables active D0 telemetry polling for discrete GPUs.
    pub fn set_dgpu_d0_metrics(&mut self, enabled: bool) {
        self.dgpu_d0_metrics = enabled;
        if !enabled {
            self.nvidia_cache.clear();
        }
    }

    pub fn dgpu_d0_metrics(&self) -> bool {
        self.dgpu_d0_metrics
    }

    /// Intel iGPU PCI device IDs ported verbatim from `uccd/inc/workers/HardwareMonitorWorker.hpp`.
    const INTEL_IGPU_DEVICE_IDS: &[u16] = &[
        0x6420, 0x64B0, 0x7D51, 0x7D67, 0x7D41, 0x7DD5, 0x7D45, 0x7D40, 0xA780, 0xA781, 0xA788,
        0xA789, 0xA78A, 0xA782, 0xA78B, 0xA783, 0xA7A0, 0xA7A1, 0xA7A8, 0xA7AA, 0xA7AB, 0xA7AC,
        0xA7AD, 0xA7A9, 0xA721, 0xA720, 0x4680, 0x4690, 0x4688, 0x468A, 0x468B, 0x4682, 0x4692,
        0x4693, 0x46D3, 0x46D4, 0x46D0, 0x46D1, 0x46D2, 0x4626, 0x4628, 0x462A, 0x46A2, 0x46B3,
        0x46C2, 0x46A3, 0x46B2, 0x46C3, 0x46A0, 0x46B0, 0x46C0, 0x46A6, 0x46AA, 0x46A8, 0x46A1,
        0x46B1, 0x46C1, 0x4C8A, 0x4C8B, 0x4C90, 0x4C9A, 0x4C8C, 0x4C80, 0x4E71, 0x4E61, 0x4E57,
        0x4E55, 0x4E51, 0x4557, 0x4555, 0x4571, 0x4551, 0x4541, 0x9A59, 0x9A78, 0x9A60, 0x9A70,
        0x9A68, 0x9A40, 0x9A49, 0x9AC0, 0x9AC9, 0x9AD9, 0x9AF8, 0x8A70, 0x8A71, 0x8A56, 0x8A58,
        0x8A5B, 0x8A5D, 0x8A54, 0x8A5A, 0x8A5C, 0x8A57, 0x8A59, 0x8A50, 0x8A51, 0x8A52, 0x8A53,
        0x3EA5, 0x3EA8, 0x3EA6, 0x3EA7, 0x3EA2, 0x3E90, 0x3E93, 0x3E99, 0x3E9C, 0x3EA1, 0x9BA5,
        0x9BA8, 0x3EA4, 0x9B21, 0x9BA0, 0x9BA2, 0x9BA4, 0x9BAA, 0x9BAB, 0x9BAC, 0x87CA, 0x3EA3,
        0x9B41, 0x9BC0, 0x9BC2, 0x9BC4, 0x9BCA, 0x9BCB, 0x9BCC, 0x3E91, 0x3E92, 0x3E98, 0x3E9B,
        0x9BC5, 0x9BC8, 0x3E96, 0x3E9A, 0x3E94, 0x9BC6, 0x9BE6, 0x9BF6, 0x3EA9, 0x3EA0, 0x593B,
        0x5923, 0x5926, 0x5927, 0x5917, 0x5912, 0x591B, 0x5916, 0x5921, 0x591A, 0x591D, 0x591E,
        0x591C, 0x87C0, 0x5913, 0x5915, 0x5902, 0x5906, 0x590B, 0x590A, 0x5908, 0x590E, 0x3185,
        0x3184, 0x1A85, 0x5A85, 0x0A84, 0x1A84, 0x5A84, 0x192A, 0x1932, 0x193B, 0x193A, 0x193D,
        0x1923, 0x1926, 0x1927, 0x192B, 0x192D, 0x1912, 0x191B, 0x1913, 0x1915, 0x1917, 0x191A,
        0x1916, 0x1921, 0x191D, 0x191E, 0x1902, 0x1906, 0x190B, 0x190A, 0x190E, 0x163D, 0x163A,
        0x1632, 0x163E, 0x163B, 0x1636, 0x1622, 0x1626, 0x162A, 0x162B, 0x162D, 0x162E, 0x1612,
        0x1616, 0x161A, 0x161B, 0x161D, 0x161E, 0x1602, 0x1606, 0x160A, 0x160B, 0x160D, 0x160E,
        0x22B0, 0x22B2, 0x22B3, 0x22B1, 0x0F30, 0x0F31, 0x0F32, 0x0F33, 0x0157, 0x0155, 0x0422,
        0x0426, 0x042A, 0x042B, 0x042E, 0x0C22, 0x0C26, 0x0C2A, 0x0C2B, 0x0C2E, 0x0A22, 0x0A2A,
        0x0A2B, 0x0D2A, 0x0D2B, 0x0D2E, 0x0A26, 0x0A2E, 0x0D22, 0x0D26, 0x0412, 0x0416, 0x0D12,
        0x041A, 0x041B, 0x0C12, 0x0C16, 0x0C1A, 0x0C1B, 0x0C1E, 0x0A12, 0x0A1A, 0x0A1B, 0x0D16,
        0x0D1A, 0x0D1B, 0x0D1E, 0x041E, 0x0A16, 0x0A1E, 0x0402, 0x0406, 0x040A, 0x040B, 0x040E,
        0x0C02, 0x0C06, 0x0C0A, 0x0C0B, 0x0C0E, 0x0A02, 0x0A06, 0x0A0A, 0x0A0B, 0x0A0E, 0x0D02,
        0x0D06, 0x0D0A, 0x0D0B, 0x0D0E, 0x0162, 0x0166, 0x016A, 0x0152, 0x0156, 0x015A, 0x0112,
        0x0122, 0x0116, 0x0126, 0x0102, 0x0106, 0x010A, 0x0042, 0x0046,
    ];

    /// AMD iGPU PCI device IDs ported verbatim from `uccd/inc/workers/HardwareMonitorWorker.hpp`.
    const AMD_IGPU_DEVICE_IDS: &[u16] = &[
        0x164E, 0x1506, 0x15DD, 0x15D8, 0x15E7, 0x1636, 0x1638, 0x164C, 0x164D, 0x1681, 0x15BF,
        0x15C8, 0x1304, 0x1305, 0x1306, 0x1307, 0x1309, 0x130A, 0x130B, 0x130C, 0x130D, 0x130E,
        0x130F, 0x1310, 0x1311, 0x1312, 0x1313, 0x1315, 0x1316, 0x1317, 0x1318, 0x131B, 0x131C,
        0x131D, 0x13C0, 0x9830, 0x9831, 0x9832, 0x9833, 0x9834, 0x9835, 0x9836, 0x9837, 0x9838,
        0x9839, 0x983A, 0x983B, 0x983C, 0x983D, 0x983E, 0x983F, 0x9850, 0x9851, 0x9852, 0x9853,
        0x9854, 0x9855, 0x9856, 0x9857, 0x9858, 0x9859, 0x985A, 0x985B, 0x985C, 0x985D, 0x985E,
        0x985F, 0x9870, 0x9874, 0x9875, 0x9876, 0x9877, 0x98E4, 0x13FE, 0x143F, 0x74A0, 0x1435,
        0x163F, 0x1900, 0x1901, 0x1114, 0x150E,
    ];

    fn parse_pci_hex_u16(raw: &str) -> Option<u16> {
        let trimmed = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
        u16::from_str_radix(trimmed, 16).ok()
    }

    fn read_pci_vendor_device(sysfs: &dyn Sysfs, device_dir: &Path) -> (Option<u16>, Option<u16>) {
        let mut vendor = sysfs
            .read(&device_dir.join("vendor"))
            .ok()
            .and_then(|s| Self::parse_pci_hex_u16(&s));
        let mut device = sysfs
            .read(&device_dir.join("device"))
            .ok()
            .and_then(|s| Self::parse_pci_hex_u16(&s));

        if vendor.is_none() || device.is_none() {
            if let Ok(uevent) = sysfs.read(&device_dir.join("uevent")) {
                for line in uevent.lines() {
                    if let Some(id_pair) = line.trim().strip_prefix("PCI_ID=") {
                        if let Some((v_str, d_str)) = id_pair.split_once(':') {
                            if vendor.is_none() {
                                vendor = Self::parse_pci_hex_u16(v_str);
                            }
                            if device.is_none() {
                                device = Self::parse_pci_hex_u16(d_str);
                            }
                        }
                    }
                }
            }
        }

        (vendor, device)
    }

    /// Discovers Intel `i915`/`xe` (`cardN` matching [`Self::INTEL_IGPU_DEVICE_IDS`]) or AMD iGPU hwmon under `sys_root`
    /// when explicit paths were not injected in [`TelemetryConfig`].
    fn discover_igpu_paths(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        let drm_dir = self.config.sys_root.join("sys/class/drm");
        let mut fallback_intel: Option<PathBuf> = None;
        if let Ok(mut entries) = self.sysfs.read_dir(&drm_dir) {
            entries.sort();
            for entry in entries {
                let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.starts_with("card")
                    || name.len() <= 4
                    || !name[4..].bytes().all(|b| b.is_ascii_digit())
                {
                    continue;
                }
                let dev_dir = entry.join("device");
                let (vendor, dev_id) = Self::read_pci_vendor_device(self.sysfs.as_ref(), &dev_dir);
                if vendor != Some(0x8086) {
                    continue;
                }
                if let Ok(class) = self.sysfs.read(&dev_dir.join("class")) {
                    let cls = class.trim().to_ascii_lowercase();
                    if !cls.is_empty() && !cls.starts_with("0x03") && !cls.starts_with("03") {
                        continue;
                    }
                }
                if let Some(did) = dev_id {
                    if Self::INTEL_IGPU_DEVICE_IDS.contains(&did) {
                        return (Some(entry), None);
                    }
                    // Reject known non-integrated Intel devices (e.g. Intel Arc dGPUs like 0x56A0).
                    continue;
                }
                if fallback_intel.is_none() {
                    fallback_intel = Some(entry);
                }
            }
        }
        if let Some(card) = fallback_intel {
            return (Some(card), None);
        }

        let hwmon_dir = self.config.sys_root.join("sys/class/hwmon");
        let mut fallback_amd: Option<PathBuf> = None;
        if let Ok(mut entries) = self.sysfs.read_dir(&hwmon_dir) {
            entries.sort();
            for entry in entries {
                if self
                    .config
                    .amd_dgpu_hwmon
                    .as_ref()
                    .is_some_and(|d| d == &entry)
                {
                    continue;
                }
                let Ok(hw_name) = self.sysfs.read(&entry.join("name")) else {
                    continue;
                };
                if hw_name.trim() == "amdgpu" {
                    let dev_dir = entry.join("device");
                    let (_, dev_id) = Self::read_pci_vendor_device(self.sysfs.as_ref(), &dev_dir);
                    if let Some(did) = dev_id {
                        if Self::AMD_IGPU_DEVICE_IDS.contains(&did) {
                            return (None, Some(entry));
                        }
                        continue;
                    }
                    let is_boot_vga = self
                        .sysfs
                        .read(&dev_dir.join("boot_vga"))
                        .map(|v| v.trim() == "1")
                        .unwrap_or(false);
                    if is_boot_vga && fallback_amd.is_none() {
                        fallback_amd = Some(entry);
                    }
                }
            }
        }
        if let Some(hwmon) = fallback_amd {
            return (None, Some(hwmon));
        }

        (None, None)
    }

    pub fn history(&self) -> &Arc<MetricsHistoryStore> {
        &self.history
    }

    /// Records external fan duty readings (`CpuFanDuty`, `GpuFanDuty`) into the history store.
    pub fn record_fan_duties(
        &self,
        timestamp_ms: i64,
        cpu_fan_pct: Option<u8>,
        gpu_fan_pct: Option<u8>,
    ) {
        if let Some(cpu) = cpu_fan_pct {
            self.history
                .push(MetricId::CpuFanDuty, timestamp_ms, f64::from(cpu));
        }
        if let Some(gpu) = gpu_fan_pct {
            self.history
                .push(MetricId::GpuFanDuty, timestamp_ms, f64::from(gpu));
        }
    }

    fn pci_root(&self) -> PathBuf {
        self.config.sys_root.join("sys/bus/pci/devices")
    }

    fn poll_intel_igpu(&mut self, card_path: &Path, now: Instant) -> IGpuSnapshot {
        let mut snap = read_intel_igpu_sysfs(self.sysfs.as_ref(), card_path, now);
        if snap.power_w.is_none() {
            let vendor_ok = match self.sysfs.read(&card_path.join("device/vendor")) {
                Ok(v) => v.trim().eq_ignore_ascii_case("0x8086"),
                Err(_) => true,
            };
            if vendor_ok {
                let resolved = find_intel_gpu_rapl_path_for_card(
                    self.sysfs.as_ref(),
                    &self.config.sys_root,
                    card_path,
                );
                match resolved {
                    Some(domain) => {
                        let needs_rebind = self
                            .intel_igpu_rapl
                            .as_ref()
                            .is_none_or(|r| r.domain_path() != domain.as_path());
                        if needs_rebind {
                            self.intel_igpu_rapl = Some(RaplEnergyCounter::new(domain));
                        }
                        let mut clear_rapl = false;
                        if let Some(ref mut rapl) = self.intel_igpu_rapl {
                            snap.power_w = rapl.sample_power_watts(self.sysfs.as_ref(), now);
                            if snap.power_w.is_none()
                                && !self.sysfs.exists(&rapl.domain_path().join("energy_uj"))
                            {
                                clear_rapl = true;
                            }
                        }
                        if clear_rapl {
                            self.intel_igpu_rapl = None;
                        }
                    }
                    None => {
                        self.intel_igpu_rapl = None;
                    }
                }
            }
        }
        if snap.power_w.is_none() {
            if let Some(curr_uj) = read_intel_igpu_energy_uj(self.sysfs.as_ref(), card_path) {
                if let Some((prev_uj, prev_t)) = self.intel_prev_hwmon_energy {
                    let dt = now.saturating_duration_since(prev_t).as_secs_f64();
                    if dt > 0.0 && curr_uj >= prev_uj {
                        let watts = (curr_uj - prev_uj) as f64 / 1_000_000.0 / dt;
                        if watts.is_finite() && (0.0..=1000.0).contains(&watts) {
                            snap.power_w = Some(watts);
                        }
                    }
                }
                self.intel_prev_hwmon_energy = Some((curr_uj, now));
            }
        }
        snap
    }

    /// Polls NVIDIA dGPU respecting D3cold guard, BDF-keyed caching, and hardware freshness.
    /// Returns `(snapshot, is_new_hw_measurement)`.
    fn poll_nvidia_dgpu(&mut self, now: Instant) -> (Option<DGpuSnapshot>, bool) {
        let pci_root = self.pci_root();
        let Ok(mut entries) = self.sysfs.read_dir(&pci_root) else {
            self.nvidia_cache.clear();
            return (None, false);
        };
        entries.sort();

        let mut nvidia_devices = Vec::new();
        let mut active_devs: Vec<(PathBuf, String)> = Vec::new();
        let mut active_bdfs = HashSet::new();

        for base in entries {
            let Ok(vendor) = self.sysfs.read(&base.join("vendor")) else {
                continue;
            };
            if !vendor.trim().eq_ignore_ascii_case(NVIDIA_PCI_VENDOR_ID) {
                continue;
            }
            let Ok(class) = self.sysfs.read(&base.join("class")) else {
                continue;
            };
            let cls = class.trim().to_ascii_lowercase();
            if !cls.starts_with("0x03") && !cls.starts_with("03") {
                continue;
            }
            let status = self
                .sysfs
                .read(&base.join("power/runtime_status"))
                .unwrap_or_default();
            if status.trim() == "active" {
                if let Some(bdf_str) = base.file_name().and_then(|n| n.to_str()) {
                    active_bdfs.insert(bdf_str.to_string());
                    active_devs.push((base.clone(), bdf_str.to_string()));
                }
            }
            nvidia_devices.push(base);
        }

        self.nvidia_cache.retain(|bdf, _| active_bdfs.contains(bdf));

        if nvidia_devices.is_empty() {
            self.nvidia_cache.clear();
            return (None, false);
        }

        let Some((dev_path, bdf_str)) = active_devs.into_iter().next() else {
            self.nvidia_cache.clear();
            let first_bdf = nvidia_devices[0]
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string);
            return (Some(DGpuSnapshot::sleeping_nvidia(first_bdf, now)), false);
        };

        let Some(ref nvml) = self.config.nvml else {
            return (
                Some(DGpuSnapshot {
                    vendor: GpuVendor::Nvidia,
                    bdf: Some(bdf_str),
                    is_sleeping: false,
                    from_cache: false,
                    d0_metrics_usage: true,
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
                }),
                true,
            );
        };

        if !self.dgpu_d0_metrics {
            self.nvidia_cache.remove(&bdf_str);
            return (
                Some(DGpuSnapshot {
                    vendor: GpuVendor::Nvidia,
                    bdf: Some(bdf_str),
                    is_sleeping: false,
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
                }),
                false,
            );
        }

        if !nvidia_device_telemetry_due(self.sysfs.as_ref(), self.cycle_counter, &dev_path) {
            if let Some(cached) = self.nvidia_cache.get(&bdf_str) {
                if now.saturating_duration_since(cached.collected_at)
                    <= self.config.dgpu_cache_max_age
                {
                    let mut out = cached.clone();
                    out.from_cache = true;
                    return (Some(out), false);
                }
            }
            self.nvidia_cache.remove(&bdf_str);
            return (None, false);
        }

        let Ok(snap) = nvml.get_telemetry_snapshot_by_bdf(&bdf_str) else {
            self.nvidia_cache.remove(&bdf_str);
            return (None, false);
        };

        let sample = DGpuSnapshot {
            vendor: GpuVendor::Nvidia,
            bdf: Some(bdf_str.clone()),
            is_sleeping: false,
            from_cache: false,
            d0_metrics_usage: true,
            temp_c: snap.temperature_c.map(f64::from),
            core_freq_mhz: snap.core_clock_mhz.map(f64::from),
            max_core_freq_mhz: snap.max_core_clock_mhz.map(f64::from),
            vram_freq_mhz: snap.vram_clock_mhz.map(f64::from),
            power_w: snap.power_usage_mw.map(|mw| f64::from(mw) / 1000.0),
            default_power_limit_w: snap.default_power_limit_mw.map(|mw| f64::from(mw) / 1000.0),
            max_power_limit_w: snap.max_power_limit_mw.map(|mw| f64::from(mw) / 1000.0),
            enforced_power_limit_w: snap.enforced_limit_mw.map(|mw| f64::from(mw) / 1000.0),
            compute_util_pct: snap.utilization.map(|u| u.gpu),
            memory_util_pct: snap.utilization.map(|u| u.memory),
            vram_used_mib: snap.memory.map(|m| m.used / (1024 * 1024)),
            vram_total_mib: snap.memory.map(|m| m.total / (1024 * 1024)),
            perf_limit_reason: snap.perf_limit_reason,
            encoder_util_pct: snap.encoder_util_pct,
            decoder_util_pct: snap.decoder_util_pct,
            current_pstate: snap.current_pstate,
            gr_clock_offset_mhz: snap.gr_clock_offset_mhz,
            mem_clock_offset_mhz: snap.mem_clock_offset_mhz,
            core_voltage_mv: snap.core_voltage_mv,
            collected_at: now,
        };
        self.nvidia_cache.insert(bdf_str, sample.clone());
        (Some(sample), true)
    }

    fn discover_amd_dgpu(&self) -> Option<(PathBuf, Option<PathBuf>)> {
        let pci_root = self.pci_root();
        let Ok(mut entries) = self.sysfs.read_dir(&pci_root) else {
            return None;
        };
        entries.sort();

        for base in entries {
            let (vendor, dev_id) = Self::read_pci_vendor_device(self.sysfs.as_ref(), &base);
            if vendor != Some(0x1002) {
                continue;
            }
            let Ok(class) = self.sysfs.read(&base.join("class")) else {
                continue;
            };
            let cls = class.trim().to_ascii_lowercase();
            if !cls.starts_with("0x03") && !cls.starts_with("03") {
                continue;
            }
            if dev_id.is_some_and(|did| Self::AMD_IGPU_DEVICE_IDS.contains(&did)) {
                continue;
            }
            let is_boot_vga = self
                .sysfs
                .read(&base.join("boot_vga"))
                .map(|v| v.trim() == "1")
                .unwrap_or(false);
            if dev_id.is_none() && is_boot_vga {
                continue;
            }
            let mut hwmon_path = None;
            if let Ok(mut hwmons) = self.sysfs.read_dir(&base.join("hwmon")) {
                hwmons.sort();
                hwmon_path = hwmons.into_iter().next();
            }
            return Some((base, hwmon_path));
        }
        None
    }

    fn poll_dgpu(&mut self, now: Instant) -> (Option<DGpuSnapshot>, bool) {
        let (nv, is_new) = self.poll_nvidia_dgpu(now);
        if nv.is_some() {
            return (nv, is_new);
        }

        let (amd_pci_dir, amd_hwmon_opt) = if let Some(ref amd_hwmon) = self.config.amd_dgpu_hwmon {
            (amd_hwmon.join("device"), Some(amd_hwmon.clone()))
        } else if let Some((pci, hwmon)) = self.discover_amd_dgpu() {
            (pci, hwmon)
        } else {
            return (None, false);
        };

        let rt_status = self
            .sysfs
            .read(&amd_pci_dir.join("power/runtime_status"))
            .unwrap_or_default();
        let is_sleeping = matches!(rt_status.trim(), "suspended" | "suspending");
        let bdf =
            read_pci_slot_name(self.sysfs.as_ref(), &amd_pci_dir.join("uevent")).or_else(|| {
                amd_pci_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
            });

        if is_sleeping {
            return (
                Some(DGpuSnapshot {
                    vendor: GpuVendor::Amd,
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
                }),
                false,
            );
        }

        let (temp_c, power_w, core_freq_mhz) = amd_hwmon_opt
            .as_deref()
            .map(|hw| read_amd_gpu_hwmon(self.sysfs.as_ref(), hw))
            .unwrap_or((None, None, None));
        (
            Some(DGpuSnapshot {
                vendor: GpuVendor::Amd,
                bdf,
                is_sleeping: false,
                from_cache: false,
                d0_metrics_usage: true,
                temp_c,
                core_freq_mhz,
                max_core_freq_mhz: None,
                vram_freq_mhz: None,
                power_w,
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
            }),
            true,
        )
    }

    /// Runs one telemetry cycle, stores fresh finite values in [`MetricsHistoryStore`], and returns the snapshot.
    pub fn poll(&mut self, now: Instant, timestamp_ms: i64) -> TelemetrySnapshot {
        let cpu = self.cpu_reader.sample(self.sysfs.as_ref(), now);
        if let Some(t) = cpu.temp_c {
            self.history.push(MetricId::CpuTemp, timestamp_ms, t);
        }
        if let Some(p) = cpu.power_w {
            self.history.push(MetricId::CpuPower, timestamp_ms, p);
        }
        if let Some(f) = cpu.freq_mhz {
            self.history
                .push(MetricId::CpuFrequency, timestamp_ms, f64::from(f));
        }

        let igpu = if let Some(card_path) = self.config.intel_card_path.clone() {
            Some(self.poll_intel_igpu(&card_path, now))
        } else if let Some(ref hwmon) = self.config.amd_igpu_hwmon {
            Some(read_amd_igpu_sysfs(self.sysfs.as_ref(), hwmon, now))
        } else {
            let (intel_card, amd_hwmon) = self.discover_igpu_paths();
            if let Some(card_path) = intel_card {
                Some(self.poll_intel_igpu(&card_path, now))
            } else {
                amd_hwmon
                    .as_ref()
                    .map(|p| read_amd_igpu_sysfs(self.sysfs.as_ref(), p, now))
            }
        };

        let (dgpu, dgpu_is_new) = self.poll_dgpu(now);
        if dgpu_is_new {
            if let Some(ref d) = dgpu {
                if !d.is_sleeping {
                    if let Some(t) = d.temp_c {
                        self.history.push(MetricId::GpuTemp, timestamp_ms, t);
                    }
                    if let Some(p) = d.power_w {
                        self.history.push(MetricId::GpuPower, timestamp_ms, p);
                    }
                    if let Some(f) = d.core_freq_mhz {
                        self.history.push(MetricId::GpuFrequency, timestamp_ms, f);
                    }
                    if let Some(vf) = d.vram_freq_mhz {
                        self.history
                            .push(MetricId::GpuVramFrequency, timestamp_ms, vf);
                    }
                    if let Some(mv) = d.core_voltage_mv {
                        self.history
                            .push(MetricId::GpuCoreVoltage, timestamp_ms, f64::from(mv));
                    }
                }
            }
        }

        self.cycle_counter = self.cycle_counter.wrapping_add(1);

        TelemetrySnapshot {
            timestamp_ms,
            collected_at: now,
            cpu,
            igpu,
            dgpu,
        }
    }
}
