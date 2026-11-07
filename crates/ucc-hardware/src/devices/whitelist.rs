// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Allowed operational power modes for device policies.
pub const VALID_MODES: &[&str] = &["ac", "type_c", "bat"];

/// Allowed CPU governors.
pub const VALID_GOVERNORS: &[&str] = &[
    "",
    "performance",
    "powersave",
    "ondemand",
    "conservative",
    "schedutil",
    "userspace",
];

/// Allowed Energy Performance Preferences.
pub const VALID_EPP: &[&str] = &[
    "",
    "default",
    "performance",
    "balance_performance",
    "balance_power",
    "power",
];

/// Allowed PCIe ASPM policies.
pub const VALID_ASPM: &[&str] = &[
    "",
    "default",
    "performance",
    "powersave",
    "powersupersave",
];

/// Whitelisted system tunables.
pub const WHITELISTED_SYS_PATHS: &[&str] = &[
    "/sys/module/snd_hda_intel/parameters/power_save",
    "/proc/sys/kernel/nmi_watchdog",
    "/proc/sys/vm/dirty_writeback_centisecs",
    "/sys/devices/system/cpu/intel_pstate/no_turbo",
];

/// A single controllable device entry across AC, Type-C, and Battery modes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    #[serde(rename = "type")]
    pub device_type: String,
    pub path: String,
    pub name: String,
    pub ac_val: String,
    pub type_c_val: String,
    pub bat_val: String,
}

/// Mode-specific CPU and ASPM tunables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CpuModeSettings {
    #[serde(default)]
    pub governor: String,
    #[serde(default)]
    pub epp: String,
    #[serde(default)]
    pub aspm: String,
    #[serde(rename = "noTurbo", default, skip_serializing_if = "Option::is_none")]
    pub no_turbo: Option<bool>,
}

/// Full configuration for power devices and tunables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PowerDeviceSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(rename = "keepNvidiaAwake", default)]
    pub keep_nvidia_awake: bool,
    #[serde(default)]
    pub cpu: HashMap<String, CpuModeSettings>,
    #[serde(default)]
    pub devices: Vec<DeviceEntry>,
}

/// Validates whether the given mode is a recognized power mode.
pub fn is_valid_mode(mode: &str) -> bool {
    VALID_MODES.contains(&mode)
}

pub fn is_valid_governor(gov: &str) -> bool {
    VALID_GOVERNORS.contains(&gov)
}

pub fn is_valid_epp(epp: &str) -> bool {
    VALID_EPP.contains(&epp)
}

pub fn is_valid_aspm(aspm: &str) -> bool {
    VALID_ASPM.contains(&aspm)
}

/// Validates whether a device path is sanctioned.
pub fn is_valid_path(device_type: &str, path: &str) -> bool {
    if path.contains("..") {
        return false;
    }

    match device_type {
        "pci" => {
            // Must match /sys/bus/pci/devices/[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]/power/control
            let prefix = "/sys/bus/pci/devices/";
            let suffix = "/power/control";
            if !path.starts_with(prefix) || !path.ends_with(suffix) {
                return false;
            }
            let bdf = &path[prefix.len()..path.len() - suffix.len()];
            if bdf.len() != 12 {
                return false;
            }
            let bytes = bdf.as_bytes();
            bytes[0..4].iter().all(|b| b.is_ascii_hexdigit())
                && bytes[4] == b':'
                && bytes[5..7].iter().all(|b| b.is_ascii_hexdigit())
                && bytes[7] == b':'
                && bytes[8..10].iter().all(|b| b.is_ascii_hexdigit())
                && bytes[10] == b'.'
                && (b'0'..=b'7').contains(&bytes[11])
        }
        "usb" => {
            // Must match /sys/bus/usb/devices/[A-Za-z0-9_.:-]+/power/control
            let prefix = "/sys/bus/usb/devices/";
            let suffix = "/power/control";
            if !path.starts_with(prefix) || !path.ends_with(suffix) {
                return false;
            }
            let id = &path[prefix.len()..path.len() - suffix.len()];
            !id.is_empty()
                && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
        }
        "wifi" => {
            // Interface name: 1..=32 chars in [A-Za-z0-9_.:-]
            !path.is_empty()
                && path.len() <= 32
                && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
        }
        "service" => path == "nvidia-powerd" || path == "nvidia-powerd.service",
        "sys" => WHITELISTED_SYS_PATHS.contains(&path),
        _ => false,
    }
}

/// Validates whether a target value for a device path is within admissible limits.
pub fn is_valid_value(device_type: &str, path: &str, value: &str) -> bool {
    match device_type {
        "pci" | "usb" => value == "on" || value == "auto",
        "wifi" => value == "on" || value == "off",
        "service" => value == "active" || value == "inactive",
        "sys" => {
            if value.is_empty() || value.len() > 6 || !value.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            let Ok(number) = value.parse::<u32>() else {
                return false;
            };
            if path == "/sys/module/snd_hda_intel/parameters/power_save" {
                number <= 3600
            } else if path == "/proc/sys/vm/dirty_writeback_centisecs" {
                number <= 60000
            } else {
                number <= 1
            }
        }
        _ => false,
    }
}

/// Validates a raw JSON settings object.
pub fn validate_settings_json(settings: &Value) -> Result<(), String> {
    let obj = settings.as_object().ok_or("Settings must be an object")?;

    if let Some(enabled) = obj.get("enabled") {
        if !enabled.is_boolean() {
            return Err("Invalid enabled flag".to_string());
        }
    }

    if let Some(keep) = obj.get("keepNvidiaAwake") {
        if !keep.is_boolean() {
            return Err("Invalid NVIDIA policy".to_string());
        }
    }

    if let Some(cpu_val) = obj.get("cpu") {
        let cpu_obj = cpu_val.as_object().ok_or("Invalid CPU policy")?;
        for (mode, policy_val) in cpu_obj {
            if !is_valid_mode(mode) {
                return Err("Invalid CPU mode".to_string());
            }
            let policy = policy_val.as_object().ok_or("Invalid CPU mode")?;
            for (field, val) in policy {
                match field.as_str() {
                    "governor" => {
                        let s = val.as_str().ok_or("Invalid CPU field: governor")?;
                        if !VALID_GOVERNORS.contains(&s) {
                            return Err(format!("Invalid governor: {}", s));
                        }
                    }
                    "epp" => {
                        let s = val.as_str().ok_or("Invalid CPU field: epp")?;
                        if !VALID_EPP.contains(&s) {
                            return Err(format!("Invalid epp: {}", s));
                        }
                    }
                    "aspm" => {
                        let s = val.as_str().ok_or("Invalid CPU field: aspm")?;
                        if !VALID_ASPM.contains(&s) {
                            return Err(format!("Invalid aspm: {}", s));
                        }
                    }
                    "noTurbo" => {
                        if !val.is_boolean() {
                            return Err("Invalid CPU field: noTurbo".to_string());
                        }
                    }
                    other => return Err(format!("Invalid CPU field: {}", other)),
                }
            }
        }
    }

    if let Some(devices_val) = obj.get("devices") {
        let devices = devices_val.as_array().ok_or("Invalid device list")?;
        if devices.len() > 2048 {
            return Err("Invalid device list".to_string());
        }

        let mut paths = HashSet::new();

        for dev in devices {
            let dev_obj = dev.as_object().ok_or("Invalid device entry")?;
            let dev_type = dev_obj.get("type").and_then(|v| v.as_str()).ok_or("Missing device type")?;
            let path = dev_obj.get("path").and_then(|v| v.as_str()).ok_or("Missing device path")?;

            if !is_valid_path(dev_type, path) || !paths.insert(path.to_string()) {
                return Err(format!("Invalid or duplicate device: {}", path));
            }

            let name = dev_obj.get("name").and_then(|v| v.as_str()).ok_or("Invalid device name")?;
            if name.len() > 512 {
                return Err("Invalid device name".to_string());
            }

            for mode in VALID_MODES {
                let key = format!("{}_val", mode);
                let val = dev_obj.get(&key).and_then(|v| v.as_str()).ok_or_else(|| format!("Missing {}", key))?;
                if !is_valid_value(dev_type, path, val) {
                    return Err(format!("Invalid value for {}", path));
                }
            }
        }
    }

    Ok(())
}
