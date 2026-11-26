//! CPU frequency, package temperature, and RAPL energy/power telemetry (`M7.1`).

use crate::traits::Sysfs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Intel RAPL microjoule counter with wrap-around handling across `max_energy_range_uj`.
#[derive(Debug, Clone)]
pub struct RaplEnergyCounter {
    domain_path: PathBuf,
    prev_energy_uj: Option<u64>,
    prev_time: Option<Instant>,
    max_energy_range_uj: Option<u64>,
}

impl RaplEnergyCounter {
    pub fn new(domain_path: impl AsRef<Path>) -> Self {
        Self {
            domain_path: domain_path.as_ref().to_path_buf(),
            prev_energy_uj: None,
            prev_time: None,
            max_energy_range_uj: None,
        }
    }

    pub fn domain_path(&self) -> &Path {
        &self.domain_path
    }

    /// Clears cached RAPL baseline state (used on reset/unbind or sensor read failure).
    pub fn reset(&mut self) {
        self.prev_energy_uj = None;
        self.prev_time = None;
        self.max_energy_range_uj = None;
    }

    /// Reads `energy_uj` and returns average power in Watts since the previous sample (`None` on first sample).
    pub fn sample_power_watts(&mut self, sysfs: &dyn Sysfs, now: Instant) -> Option<f64> {
        let Ok(energy_raw) = sysfs.read(&self.domain_path.join("energy_uj")) else {
            self.reset();
            return None;
        };
        let Ok(curr) = energy_raw.trim().parse::<u64>() else {
            self.reset();
            return None;
        };

        if self.max_energy_range_uj.is_none() {
            self.max_energy_range_uj = sysfs
                .read(&self.domain_path.join("max_energy_range_uj"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
        }

        let prev_e = self.prev_energy_uj.replace(curr);
        let prev_t = self.prev_time.replace(now);

        let (Some(prev), Some(t0)) = (prev_e, prev_t) else {
            return None;
        };
        let dt_s = now.saturating_duration_since(t0).as_secs_f64();
        if dt_s <= 0.0 || !dt_s.is_finite() {
            return None;
        }

        let delta_uj = if curr >= prev {
            curr - prev
        } else {
            let max_range = self.max_energy_range_uj?;
            if max_range > prev {
                (max_range - prev).saturating_add(curr)
            } else {
                return None;
            }
        };

        let watts = (delta_uj as f64) / 1_000_000.0 / dt_s;
        if watts.is_finite() && watts >= 0.0 {
            Some(watts)
        } else {
            None
        }
    }

    /// Reads a RAPL power limit constraint (`constraint_<idx>_power_limit_uw`) in Watts.
    pub fn read_constraint_watts(
        sysfs: &dyn Sysfs,
        domain_path: &Path,
        constraint_idx: usize,
    ) -> Option<f64> {
        let node = domain_path.join(format!("constraint_{constraint_idx}_power_limit_uw"));
        let uw = sysfs.read(&node).ok()?.trim().parse::<u64>().ok()?;
        Some(uw as f64 / 1_000_000.0)
    }

    /// Reads the highest configured RAPL power limit across constraints `0..=2` (`max_power_uw`,
    /// falling back to `power_limit_uw`), returning Watts.
    pub fn read_max_power_limit_watts(sysfs: &dyn Sysfs, domain_path: &Path) -> Option<f64> {
        let mut best_uw: Option<u64> = None;
        for idx in 0..=2 {
            let max_node = domain_path.join(format!("constraint_{idx}_max_power_uw"));
            let lim_node = domain_path.join(format!("constraint_{idx}_power_limit_uw"));
            let val = sysfs
                .read(&max_node)
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .filter(|&uw| uw > 0)
                .or_else(|| {
                    sysfs
                        .read(&lim_node)
                        .ok()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .filter(|&uw| uw > 0)
                });
            if let Some(uw) = val {
                best_uw = Some(best_uw.map_or(uw, |prev| prev.max(uw)));
            }
        }
        best_uw.map(|uw| uw as f64 / 1_000_000.0)
    }
}

/// CPU telemetry readings using explicit `Option` for missing sensors (sentinels added only at IPC).
#[derive(Debug, Clone)]
pub struct CpuTelemetrySnapshot {
    pub temp_c: Option<f64>,
    pub freq_mhz: Option<u32>,
    pub power_w: Option<f64>,
    pub max_power_limit_w: Option<f64>,
    pub collected_at: Instant,
}

/// Discovers and samples CPU package temperature, scaling frequencies, and package RAPL power.
pub struct CpuTelemetryReader {
    sys_root: PathBuf,
    rapl: Option<RaplEnergyCounter>,
}

impl CpuTelemetryReader {
    pub fn new(sys_root: impl AsRef<Path>) -> Self {
        Self {
            sys_root: sys_root.as_ref().to_path_buf(),
            rapl: None,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.sys_root.join(rel.trim_start_matches('/'))
    }

    fn ensure_rapl(&mut self, sysfs: &dyn Sysfs) {
        if self.rapl.is_some() {
            return;
        }
        let powercap = self.path("sys/class/powercap");
        let Ok(mut entries) = sysfs.read_dir(&powercap) else {
            return;
        };
        entries.sort();
        for entry in entries {
            let Some(fname) = entry.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !fname.starts_with("intel-rapl:") {
                continue;
            }
            if let Ok(name) = sysfs.read(&entry.join("name")) {
                if name.trim().starts_with("package-") {
                    self.rapl = Some(RaplEnergyCounter::new(entry));
                    return;
                }
            }
        }
    }

    /// Reads average online core frequency in MHz across `cpufreq/scaling_cur_freq` (kHz -> MHz).
    pub fn read_average_freq_mhz(&self, sysfs: &dyn Sysfs) -> Option<u32> {
        let cpu_dir = self.path("sys/devices/system/cpu");
        let Ok(mut entries) = sysfs.read_dir(&cpu_dir) else {
            return None;
        };
        entries.sort();

        let mut sum_khz: u64 = 0;
        let mut count: u64 = 0;

        for entry in entries {
            let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.len() <= 3
                || !name.starts_with("cpu")
                || !name[3..].bytes().all(|b| b.is_ascii_digit())
            {
                continue;
            }
            if let Ok(online) = sysfs.read(&entry.join("online")) {
                if online.trim() != "1" {
                    continue;
                }
            }
            if let Ok(khz_str) = sysfs.read(&entry.join("cpufreq/scaling_cur_freq")) {
                if let Ok(khz) = khz_str.trim().parse::<u64>() {
                    if khz > 0 {
                        sum_khz = sum_khz.saturating_add(khz);
                        count += 1;
                    }
                }
            }
        }

        sum_khz.checked_div(count).map(|avg| (avg / 1000) as u32)
    }

    /// Reads CPU package temperature in Celsius from `coretemp`, `k10temp`, or `zenpower` hwmon nodes.
    pub fn read_package_temp_c(&self, sysfs: &dyn Sysfs) -> Option<f64> {
        let hwmon_root = self.path("sys/class/hwmon");
        let Ok(mut entries) = sysfs.read_dir(&hwmon_root) else {
            return None;
        };
        entries.sort();

        for entry in entries {
            let Ok(name) = sysfs.read(&entry.join("name")) else {
                continue;
            };
            let trimmed = name.trim();
            if !matches!(trimmed, "coretemp" | "k10temp" | "zenpower") {
                continue;
            }
            if let Ok(milli_c) = sysfs.read(&entry.join("temp1_input")) {
                if let Ok(val) = milli_c.trim().parse::<i64>() {
                    if val >= 0 {
                        return Some(val as f64 / 1000.0);
                    }
                }
            }
        }
        None
    }

    pub fn sample(&mut self, sysfs: &dyn Sysfs, now: Instant) -> CpuTelemetrySnapshot {
        self.ensure_rapl(sysfs);
        let temp_c = self.read_package_temp_c(sysfs);
        let freq_mhz = self.read_average_freq_mhz(sysfs);
        let mut clear_rapl = false;
        let (power_w, max_power_limit_w) = match self.rapl.as_mut() {
            Some(rapl) => {
                let p = rapl.sample_power_watts(sysfs, now);
                let lim = RaplEnergyCounter::read_max_power_limit_watts(sysfs, rapl.domain_path());
                if p.is_none() && !sysfs.exists(&rapl.domain_path().join("energy_uj")) {
                    clear_rapl = true;
                }
                (p, lim)
            }
            None => (None, None),
        };
        if clear_rapl {
            self.rapl = None;
        }

        CpuTelemetrySnapshot {
            temp_c,
            freq_mhz,
            power_w,
            max_power_limit_w,
            collected_at: now,
        }
    }
}
