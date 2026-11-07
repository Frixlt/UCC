// SPDX-License-Identifier: GPL-3.0-or-later
use super::whitelist::{is_valid_mode, is_valid_path, validate_settings_json};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use ucc_core::UccProfile;

pub type ProcessRunner = Arc<dyn Fn(&str, &[&str]) -> bool + Send + Sync>;

pub fn default_process_runner() -> ProcessRunner {
    Arc::new(|program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Controller managing PCIe ASPM, runtime PM for PCI/USB devices, Wi-Fi power save,
/// system tunables, and NVIDIA dynamic boost without shell interpolation.
pub struct PowerDeviceController {
    sys_path: PathBuf,
    proc_path: PathBuf,
    programs_path: PathBuf,
    runner: RwLock<ProcessRunner>,
    settings: RwLock<Value>,
    status: RwLock<Value>,
    revision: AtomicU64,
}

impl Default for PowerDeviceController {
    fn default() -> Self {
        Self::new("/sys", "/proc", default_process_runner(), "/usr/bin")
    }
}

impl PowerDeviceController {
    pub fn new(
        sys: impl Into<PathBuf>,
        proc: impl Into<PathBuf>,
        runner: ProcessRunner,
        programs: impl Into<PathBuf>,
    ) -> Self {
        Self {
            sys_path: sys.into(),
            proc_path: proc.into(),
            programs_path: programs.into(),
            runner: RwLock::new(runner),
            settings: RwLock::new(json!({
                "enabled": false,
                "keepNvidiaAwake": false,
                "cpu": {},
                "devices": []
            })),
            status: RwLock::new(json!({
                "mode": "ac",
                "enabled": false,
                "devices": []
            })),
            revision: AtomicU64::new(0),
        }
    }

    pub fn set_runner(&self, runner: ProcessRunner) {
        let mut r = self.runner.write().unwrap();
        *r = runner;
    }

    pub fn validate(settings: &Value) -> Result<(), String> {
        validate_settings_json(settings)
    }

    pub fn set_settings(&self, settings: Value) -> Result<(), String> {
        Self::validate(&settings)?;
        {
            let mut s = self.settings.write().unwrap();
            *s = settings;
        }
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn settings(&self) -> Value {
        self.settings.read().unwrap().clone()
    }

    pub fn status(&self) -> Value {
        self.status.read().unwrap().clone()
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    /// Map a virtual sysfs or procfs path to the actual directory configured for this controller.
    pub fn mapped_path(&self, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("/sys/") {
            self.sys_path.join(rest)
        } else if let Some(rest) = path.strip_prefix("/proc/") {
            self.proc_path.join(rest)
        } else {
            PathBuf::from(path)
        }
    }

    pub fn read_file(&self, path: &str) -> String {
        let target = self.mapped_path(path);
        fs::read_to_string(target)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    pub fn write_file(&self, path: &str, value: &str) -> bool {
        let target = self.mapped_path(path);
        if !target.exists() {
            return false;
        }
        if let Ok(current) = fs::read_to_string(&target) {
            if current.trim() == value {
                return true;
            }
        }
        fs::write(&target, value).is_ok()
    }

    fn run_command(&self, program: &str, args: &[&str]) -> bool {
        let runner = self.runner.read().unwrap().clone();
        runner(program, args)
    }

    /// Overlay mode-specific CPU governor, EPP, and noTurbo parameters onto a UccProfile.
    pub fn overlay_cpu(&self, profile: &mut UccProfile, mode: &str) {
        let config = self.settings();
        if !config.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            return;
        }

        if let Some(cpu_mode) = config.get("cpu").and_then(|c| c.get(mode)) {
            if let Some(gov) = cpu_mode.get("governor").and_then(|g| g.as_str()) {
                if !gov.is_empty() {
                    profile.cpu.governor = gov.to_string();
                }
            }
            if let Some(epp) = cpu_mode.get("epp").and_then(|e| e.as_str()) {
                if !epp.is_empty() {
                    profile.cpu.energy_performance_preference = epp.to_string();
                }
            }
            if let Some(no_turbo) = cpu_mode.get("noTurbo").and_then(|n| n.as_bool()) {
                profile.cpu.no_turbo = no_turbo;
            }
        }

        if let Some(devices) = config.get("devices").and_then(|d| d.as_array()) {
            for dev in devices {
                if dev.get("path").and_then(|p| p.as_str()) == Some("/sys/devices/system/cpu/intel_pstate/no_turbo") {
                    let key = format!("{}_val", mode);
                    if let Some(val) = dev.get(&key).and_then(|v| v.as_str()) {
                        profile.cpu.no_turbo = val == "1";
                    }
                }
            }
        }
    }

    /// Apply device, ASPM, Wi-Fi, and service policies for the given operational mode.
    pub fn apply(&self, mode: &str) {
        if !is_valid_mode(mode) {
            return;
        }

        let config = self.settings();
        let enabled = config.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut result = json!({
            "mode": mode,
            "enabled": enabled,
            "devices": []
        });

        if enabled {
            // 1. PCIe ASPM policy
            if let Some(cpu_mode) = config.get("cpu").and_then(|c| c.get(mode)) {
                if let Some(aspm) = cpu_mode.get("aspm").and_then(|a| a.as_str()) {
                    if !aspm.is_empty() {
                        let applied = self.write_file("/sys/module/pcie_aspm/parameters/policy", aspm);
                        result["aspmApplied"] = json!(applied);
                    }
                }
            }

            // 2. NVIDIA GPU power guard and persistence
            let keep_awake = config.get("keepNvidiaAwake").and_then(|k| k.as_bool()).unwrap_or(false);
            let mut wake_nvidia = false;
            let mut manage_nvidia = false;

            if let Some(devices) = config.get("devices").and_then(|d| d.as_array()) {
                for dev in devices {
                    if dev.get("type").and_then(|t| t.as_str()) == Some("pci") {
                        if let Some(path) = dev.get("path").and_then(|p| p.as_str()) {
                            let p = Path::new(path);
                            if let Some(parent) = p.parent().and_then(|p| p.parent()) {
                                let vendor = self.read_file(&parent.join("vendor").to_string_lossy());
                                let class = self.read_file(&parent.join("class").to_string_lossy());
                                if vendor == "0x10de" && class.starts_with("0x03") {
                                    manage_nvidia = true;
                                    let mode_key = format!("{}_val", mode);
                                    let mode_val = dev.get(&mode_key).and_then(|v| v.as_str()).unwrap_or("auto");
                                    if keep_awake || mode_val == "on" {
                                        wake_nvidia = self.write_file(path, "on") || wake_nvidia;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Undo persistence before allowing runtime suspend
            if manage_nvidia && self.proc_path.join("driver/nvidia/version").exists() {
                result["nvidiaPersistenceEnabled"] = json!(wake_nvidia);
                let pm_val = if wake_nvidia { "1" } else { "0" };
                let applied = self.run_command("nvidia-smi", &["-pm", pm_val]);
                result["nvidiaPersistenceApplied"] = json!(applied);
            }

            // 3. Apply individual device policies
            let mut device_results = Vec::new();

            if let Some(devices) = config.get("devices").and_then(|d| d.as_array()) {
                for dev in devices {
                    let dev_type = dev.get("type").and_then(|t| t.as_str()).unwrap_or_default();
                    let path = dev.get("path").and_then(|p| p.as_str()).unwrap_or_default();
                    let mode_key = format!("{}_val", mode);
                    let mut value = dev.get(&mode_key).and_then(|v| v.as_str()).unwrap_or("auto").to_string();

                    let mut guarded = false;

                    let ok = if dev_type == "wifi" {
                        self.run_command("iw", &["dev", path, "set", "power_save", &value])
                    } else if dev_type == "service" {
                        let action = if value == "active" { "start" } else { "stop" };
                        self.run_command("systemctl", &[action, "nvidia-powerd.service"])
                    } else {
                        if dev_type == "pci" && keep_awake {
                            let p = Path::new(path);
                            if let Some(parent) = p.parent().and_then(|p| p.parent()) {
                                let vendor = self.read_file(&parent.join("vendor").to_string_lossy());
                                let class = self.read_file(&parent.join("class").to_string_lossy());
                                if vendor == "0x10de" && class.starts_with("0x03") {
                                    value = "on".to_string();
                                    guarded = true;
                                }
                            }
                        }
                        self.write_file(path, &value)
                    };

                    device_results.push(json!({
                        "path": path,
                        "value": value,
                        "ok": ok,
                        "guarded": guarded
                    }));
                }
            }

            result["devices"] = json!(device_results);

            // Record current CPU status
            result["cpu"] = json!({
                "governor": self.read_file("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
                "epp": self.read_file("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"),
                "noTurbo": self.read_file("/sys/devices/system/cpu/intel_pstate/no_turbo")
            });
        }

        let mut st = self.status.write().unwrap();
        *st = result;
    }

    /// Scan hardware buses and merge with existing saved devices, preserving user edits and disconnected items.
    pub fn scan(&self) -> Value {
        let mut config = self.settings();
        if !config.get("cpu").map(|c| c.is_object()).unwrap_or(false) {
            config["cpu"] = json!({});
        }

        let mut devices = config
            .get("devices")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut existing = HashSet::new();
        for dev in &devices {
            if let Some(p) = dev.get("path").and_then(|p| p.as_str()) {
                existing.insert(p.to_string());
            }
        }

        let mut append_device = |dev_type: &str, path: &str, name: &str, ac: &str, bat: &str| {
            if existing.insert(path.to_string()) {
                devices.push(json!({
                    "type": dev_type,
                    "path": path,
                    "name": name,
                    "ac_val": ac,
                    "type_c_val": bat,
                    "bat_val": bat
                }));
            }
        };

        // 1. Enumerate PCI and USB devices
        for bus in &["pci", "usb"] {
            let bus_dir = self.sys_path.join(format!("bus/{}/devices", bus));
            if let Ok(entries) = fs::read_dir(bus_dir) {
                for entry in entries.flatten() {
                    let control_path = entry.path().join("power/control");
                    if !control_path.exists() {
                        continue;
                    }

                    let id = entry.file_name().to_string_lossy().to_string();
                    let sysfs_path = format!("/sys/bus/{}/devices/{}/power/control", bus, id);

                    let name = if *bus == "usb" {
                        let mfg = fs::read_to_string(entry.path().join("manufacturer"))
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                        let prod = fs::read_to_string(entry.path().join("product"))
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                        let combined = format!("{} {}", mfg, prod).trim().to_string();
                        if !combined.is_empty() {
                            combined
                        } else {
                            format!("usb {}", id)
                        }
                    } else {
                        let vendor = fs::read_to_string(entry.path().join("vendor"))
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                        let device = fs::read_to_string(entry.path().join("device"))
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                        format!("pci {} ({}:{})", id, vendor, device)
                    };

                    let lower = name.to_lowercase();
                    let critical = *bus == "usb"
                        && (lower.contains("keyboard")
                            || lower.contains("mouse")
                            || lower.contains("bluetooth")
                            || lower.contains("receiver")
                            || lower.contains("controller"));

                    if is_valid_path(bus, &sysfs_path) {
                        append_device(bus, &sysfs_path, &name, "on", if critical { "on" } else { "auto" });
                    }
                }
            }
        }

        // 2. Enumerate Wi-Fi devices
        let net_dir = self.sys_path.join("class/net");
        if let Ok(entries) = fs::read_dir(net_dir) {
            for entry in entries.flatten() {
                if entry.path().join("wireless").exists() {
                    let iface = entry.file_name().to_string_lossy().to_string();
                    let name = format!("Wi-Fi power save ({})", iface);
                    append_device("wifi", &iface, &name, "off", "on");
                }
            }
        }

        // 3. Enumerate nvidia-powerd service
        if self.programs_path.join("nvidia-powerd").exists() {
            append_device("service", "nvidia-powerd", "NVIDIA Dynamic Boost", "active", "inactive");
        }

        // 4. Enumerate system tunables
        let system_tunables = [
            ("Audio Powersave", "/sys/module/snd_hda_intel/parameters/power_save", "0", "1"),
            ("NMI Watchdog", "/proc/sys/kernel/nmi_watchdog", "0", "0"),
            ("VM Writeback", "/proc/sys/vm/dirty_writeback_centisecs", "500", "1500"),
            ("CPU Turbo Boost (1 = disabled)", "/sys/devices/system/cpu/intel_pstate/no_turbo", "0", "1"),
        ];

        for (name, path, ac, bat) in system_tunables {
            if self.mapped_path(path).exists() {
                append_device("sys", path, name, ac, bat);
            }
        }

        config["devices"] = json!(devices);
        config
    }
}
