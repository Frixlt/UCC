// SPDX-License-Identifier: GPL-3.0-or-later

//! Non-waking session battery, PCI GPU runtime state, and GPU process telemetry.

use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SAMPLED_PROCESSES: usize = 512;

#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub supplies: PathBuf,
    pub pci: PathBuf,
    pub proc: PathBuf,
    pub nvidia_prefix: String,
}

impl Default for SessionPaths {
    fn default() -> Self {
        Self {
            supplies: PathBuf::from("/sys/class/power_supply"),
            pci: PathBuf::from("/sys/bus/pci/devices"),
            proc: PathBuf::from("/proc"),
            nvidia_prefix: "/dev/nvidia".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatterySample {
    pub name: String,
    pub capacity: Option<f64>,
    pub status: String,
    pub power_watts: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuCardSample {
    pub pci: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: i64,
    pub start_time: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySnapshot {
    pub batteries: Vec<BatterySample>,
    pub gpu_cards: Vec<GpuCardSample>,
    pub gpu_state: String,
    pub processes: Vec<ProcessIdentity>,
    pub processes_sampled: bool,
    pub truncated: bool,
    pub timestamp_ms: u64,
}

pub struct SessionReader {
    paths: SessionPaths,
}

impl SessionReader {
    pub fn new(paths: SessionPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &SessionPaths {
        &self.paths
    }

    pub fn read_trimmed(path: &Path) -> String {
        fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn read_number(path: &Path) -> Option<f64> {
        let raw = Self::read_trimmed(path);
        let value = raw.parse::<f64>().ok()?;
        value.is_finite().then_some(value)
    }

    /// Inspects `/proc/<pid>` ownership, state, start_time (field 19 after `')'`), and comm.
    pub fn process_identity(&self, pid: i64) -> Option<ProcessIdentity> {
        let euid = unsafe { nix::libc::geteuid() };
        if euid == 0 || pid <= 1 || pid == std::process::id() as i64 {
            return None;
        }
        let dir = self.paths.proc.join(pid.to_string());
        let meta = fs::metadata(&dir).ok()?;
        if meta.uid() != euid {
            return None;
        }

        let stat = Self::read_trimmed(&dir.join("stat"));
        let close_paren = stat.rfind(')')?;
        let fields: Vec<&str> = stat[close_paren + 1..].split_whitespace().collect();
        if fields.len() <= 19 || fields[0] == "Z" || fields[0] == "X" {
            return None;
        }
        let start_time = fields[19].parse::<u64>().ok()?;
        if start_time == 0 {
            return None;
        }

        let comm = Self::read_trimmed(&dir.join("comm"));
        let name = if comm.is_empty() {
            "unknown".to_string()
        } else {
            comm.chars().take(256).collect()
        };

        Some(ProcessIdentity {
            pid,
            start_time,
            name,
        })
    }

    /// Checks `/proc/<pid>/fd/*` symlink targets for NVIDIA device nodes.
    pub fn holds_nvidia_device(&self, pid: i64) -> bool {
        let fd_dir = self.paths.proc.join(pid.to_string()).join("fd");
        let Ok(entries) = fs::read_dir(&fd_dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let Ok(target) = fs::read_link(entry.path()) else {
                continue;
            };
            let target_str = target.to_string_lossy();
            let Some(suffix) = target_str.strip_prefix(&self.paths.nvidia_prefix) else {
                continue;
            };
            if suffix == "ctl"
                || suffix == "-uvm"
                || suffix == "-uvm-tools"
                || suffix == "-modeset"
                || (!suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
            {
                return true;
            }
        }
        false
    }

    pub fn snapshot(&self, include_processes: bool) -> TelemetrySnapshot {
        let mut batteries = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.paths.supplies) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            names.sort();

            for name in names {
                let path = self.paths.supplies.join(&name);
                if Self::read_trimmed(&path.join("type")) != "Battery"
                    || Self::read_trimmed(&path.join("scope")) == "Device"
                    || Self::read_trimmed(&path.join("present")) == "0"
                {
                    continue;
                }
                let capacity = Self::read_number(&path.join("capacity"))
                    .filter(|c| (0.0..=100.0).contains(c));
                let mut watts = Self::read_number(&path.join("power_now")).map(|w| w.abs() / 1e6);
                if watts.is_none() {
                    if let (Some(amps), Some(volts)) = (
                        Self::read_number(&path.join("current_now")),
                        Self::read_number(&path.join("voltage_now")),
                    ) {
                        watts = Some((amps.abs() * volts.abs()) / 1e12);
                    }
                }
                watts = watts.filter(|w| w.is_finite());
                let status_raw = Self::read_trimmed(&path.join("status"));
                let status = if status_raw.is_empty() {
                    "Unknown".to_string()
                } else {
                    status_raw
                };
                batteries.push(BatterySample {
                    name,
                    capacity,
                    status,
                    power_watts: watts,
                });
            }
        }

        let mut gpu_cards = Vec::new();
        let mut any_active = false;
        let mut any_unknown = false;
        if let Ok(entries) = fs::read_dir(&self.paths.pci) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            names.sort();

            for name in names {
                let path = self.paths.pci.join(&name);
                if Self::read_trimmed(&path.join("vendor")) != "0x10de"
                    || !Self::read_trimmed(&path.join("class")).starts_with("0x03")
                {
                    continue;
                }
                let raw_status = Self::read_trimmed(&path.join("power/runtime_status"));
                any_active |= raw_status == "active";
                any_unknown |=
                    raw_status != "active" && raw_status != "suspended" && raw_status != "off";
                let status = if raw_status.is_empty() {
                    "unknown".to_string()
                } else {
                    raw_status
                };
                gpu_cards.push(GpuCardSample { pci: name, status });
            }
        }

        let gpu_state = if gpu_cards.is_empty() {
            "unavailable"
        } else if any_active {
            "active"
        } else if any_unknown {
            "unknown"
        } else {
            "suspended"
        }
        .to_string();

        let mut processes = Vec::new();
        let mut truncated = false;
        if include_processes && (any_active || any_unknown) {
            if let Ok(entries) = fs::read_dir(&self.paths.proc) {
                let mut pids: Vec<i64> = entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok())
                    .collect();
                pids.sort_unstable();

                for pid in pids {
                    let Some(before) = self.process_identity(pid) else {
                        continue;
                    };
                    if !self.holds_nvidia_device(pid) {
                        continue;
                    }
                    let Some(after) = self.process_identity(pid) else {
                        continue;
                    };
                    if before.start_time != after.start_time {
                        continue;
                    }
                    if processes.len() >= MAX_SAMPLED_PROCESSES {
                        truncated = true;
                        break;
                    }
                    processes.push(after);
                }
            }
        }

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        TelemetrySnapshot {
            batteries,
            gpu_cards,
            gpu_state,
            processes,
            processes_sampled: include_processes,
            truncated,
            timestamp_ms,
        }
    }
}

pub fn battery_summary(snapshot: &TelemetrySnapshot) -> String {
    if snapshot.batteries.is_empty() {
        return "Battery data unavailable".to_string();
    }
    snapshot
        .batteries
        .iter()
        .map(|b| {
            let cap = b
                .capacity
                .map(|c| format!("{:.0}%", c))
                .unwrap_or_else(|| "—".to_string());
            let mut line = format!("{}: {} · {}", b.name, cap, b.status);
            if let Some(w) = b.power_watts {
                let sign = match b.status.as_str() {
                    "Charging" => "+",
                    "Discharging" => "−",
                    _ => "",
                };
                line.push_str(&format!(" · {}{:.1} W", sign, w));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn mk_temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("ucc_desk_telem_{tag}_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn snapshot_reads_multi_battery_pci_d3cold_and_nvidia_fds() {
        let root = mk_temp_root("basic");
        let supplies = root.join("power_supply");
        let pci = root.join("pci");
        let proc_dir = root.join("proc");
        fs::create_dir_all(supplies.join("BAT0")).unwrap();
        fs::create_dir_all(supplies.join("BAT1")).unwrap();
        fs::create_dir_all(supplies.join("hidpp_battery_0")).unwrap();
        fs::create_dir_all(pci.join("0000:01:00.0/power")).unwrap();

        fs::write(supplies.join("BAT0/type"), "Battery\n").unwrap();
        fs::write(supplies.join("BAT0/capacity"), "85\n").unwrap();
        fs::write(supplies.join("BAT0/status"), "Discharging\n").unwrap();
        fs::write(supplies.join("BAT0/power_now"), "12500000\n").unwrap();

        fs::write(supplies.join("BAT1/type"), "Battery\n").unwrap();
        fs::write(supplies.join("BAT1/capacity"), "90\n").unwrap();
        fs::write(supplies.join("BAT1/status"), "Charging\n").unwrap();
        fs::write(supplies.join("BAT1/current_now"), "2000000\n").unwrap();
        fs::write(supplies.join("BAT1/voltage_now"), "11500000\n").unwrap();

        fs::write(supplies.join("hidpp_battery_0/type"), "Battery\n").unwrap();
        fs::write(supplies.join("hidpp_battery_0/scope"), "Device\n").unwrap();

        fs::write(pci.join("0000:01:00.0/vendor"), "0x10de\n").unwrap();
        fs::write(pci.join("0000:01:00.0/class"), "0x030000\n").unwrap();
        fs::write(pci.join("0000:01:00.0/power/runtime_status"), "suspended\n").unwrap();

        let paths = SessionPaths {
            supplies: supplies.clone(),
            pci: pci.clone(),
            proc: proc_dir.clone(),
            nvidia_prefix: "/dev/nvidia".to_string(),
        };
        let reader = SessionReader::new(paths);
        let snap = reader.snapshot(true);

        assert_eq!(snap.batteries.len(), 2);
        assert_eq!(snap.batteries[0].name, "BAT0");
        assert_eq!(snap.batteries[0].power_watts, Some(12.5));
        assert_eq!(snap.batteries[1].name, "BAT1");
        assert!((snap.batteries[1].power_watts.unwrap() - 23.0).abs() < 0.01);
        assert_eq!(snap.gpu_state, "suspended");
        assert!(snap.processes.is_empty());

        // Now mark PCI active and mock an owned PID holding /dev/nvidiactl
        fs::write(pci.join("0000:01:00.0/power/runtime_status"), "active\n").unwrap();
        let pid_dir = proc_dir.join("424242");
        fs::create_dir_all(pid_dir.join("fd")).unwrap();
        // 19 fields after ')' -> fields[0] = 'S', fields[19] (18th numeric) = starttime
        let stat_line = format!(
            "424242 (vulkan-app) S {}",
            (0..22).map(|i| if i == 18 { "123456" } else { "0" }).collect::<Vec<_>>().join(" ")
        );
        fs::write(pid_dir.join("stat"), stat_line).unwrap();
        fs::write(pid_dir.join("comm"), "vulkan-app\n").unwrap();
        symlink("/dev/nvidiactl", pid_dir.join("fd/3")).unwrap();

        let active_snap = reader.snapshot(true);
        assert_eq!(active_snap.gpu_state, "active");
        assert_eq!(active_snap.processes.len(), 1);
        assert_eq!(active_snap.processes[0].pid, 424242);
        assert_eq!(active_snap.processes[0].start_time, 123456);
        assert_eq!(active_snap.processes[0].name, "vulkan-app");

        let summary = battery_summary(&active_snap);
        assert!(summary.contains("BAT0: 85%"));
        assert!(summary.contains("BAT1: 90%"));

        let _ = fs::remove_dir_all(&root);
    }
}
