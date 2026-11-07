// SPDX-License-Identifier: GPL-3.0-or-later
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use ucc_core::UccProfile;
use ucc_hardware::devices::PowerDeviceController;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn read_file(path: &Path) -> String {
    fs::read_to_string(path).unwrap().trim().to_string()
}

#[test]
fn test_power_devices_apply_every_mode_without_real_sysfs() {
    let root = TempDir::new().unwrap();
    let sys = root.path().join("sys");
    let proc = root.path().join("proc");

    write_file(&sys.join("bus/pci/devices/0000:02:00.0/power/control"), "auto");
    write_file(&proc.join("sys/vm/dirty_writeback_centisecs"), "500");
    write_file(&sys.join("module/pcie_aspm/parameters/policy"), "default");

    let commands: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cmd_log = Arc::clone(&commands);

    let runner = Arc::new(move |program: &str, args: &[&str]| {
        let mut log = cmd_log.lock().unwrap();
        log.push(format!("{}:{}", program, args.join(" ")));
        true
    });

    let controller = PowerDeviceController::new(
        &sys,
        &proc,
        runner,
        root.path().join("bin"),
    );

    let config = json!({
        "enabled": true,
        "cpu": {
            "ac": { "aspm": "performance" },
            "type_c": { "aspm": "powersave" },
            "bat": { "aspm": "powersupersave" }
        },
        "devices": [
            {
                "type": "pci",
                "path": "/sys/bus/pci/devices/0000:02:00.0/power/control",
                "name": "Test PCI GPU",
                "ac_val": "on",
                "type_c_val": "auto",
                "bat_val": "auto"
            },
            {
                "type": "sys",
                "path": "/proc/sys/vm/dirty_writeback_centisecs",
                "name": "VM Writeback",
                "ac_val": "500",
                "type_c_val": "1500",
                "bat_val": "2000"
            },
            {
                "type": "wifi",
                "path": "wlan0",
                "name": "Wi-Fi Power Save",
                "ac_val": "off",
                "type_c_val": "on",
                "bat_val": "on"
            },
            {
                "type": "service",
                "path": "nvidia-powerd",
                "name": "NVIDIA Dynamic Boost",
                "ac_val": "active",
                "type_c_val": "inactive",
                "bat_val": "inactive"
            }
        ]
    });

    assert!(controller.set_settings(config).is_ok());

    // 1. Apply AC mode
    controller.apply("ac");
    assert_eq!(
        read_file(&sys.join("bus/pci/devices/0000:02:00.0/power/control")),
        "on"
    );
    assert_eq!(
        read_file(&proc.join("sys/vm/dirty_writeback_centisecs")),
        "500"
    );
    assert_eq!(
        read_file(&sys.join("module/pcie_aspm/parameters/policy")),
        "performance"
    );

    // 2. Apply Type-C mode
    controller.apply("type_c");
    assert_eq!(
        read_file(&sys.join("bus/pci/devices/0000:02:00.0/power/control")),
        "auto"
    );
    assert_eq!(
        read_file(&proc.join("sys/vm/dirty_writeback_centisecs")),
        "1500"
    );
    assert_eq!(
        read_file(&sys.join("module/pcie_aspm/parameters/policy")),
        "powersave"
    );

    // 3. Apply Battery mode
    controller.apply("bat");
    assert_eq!(
        read_file(&sys.join("bus/pci/devices/0000:02:00.0/power/control")),
        "auto"
    );
    assert_eq!(
        read_file(&proc.join("sys/vm/dirty_writeback_centisecs")),
        "2000"
    );
    assert_eq!(
        read_file(&sys.join("module/pcie_aspm/parameters/policy")),
        "powersupersave"
    );

    // Verify commands were executed safely with exact arguments
    let log = commands.lock().unwrap();
    assert!(log.contains(&"iw:dev wlan0 set power_save off".to_string()));
    assert!(log.contains(&"iw:dev wlan0 set power_save on".to_string()));
    assert!(log.contains(&"systemctl:start nvidia-powerd.service".to_string()));
    assert!(log.contains(&"systemctl:stop nvidia-powerd.service".to_string()));

    // Verify per-device status tracking
    let status = controller.status();
    assert_eq!(status["mode"], "bat");
    assert!(status["devices"][0]["ok"].as_bool().unwrap());
}

#[test]
fn test_rejects_arbitrary_paths_and_injection_attempts() {
    let test_cases = [
        // Arbitrary system files
        json!({ "type": "sys", "path": "/etc/shadow", "name": "Shadow", "ac_val": "0", "type_c_val": "0", "bat_val": "0" }),
        // Unapproved service
        json!({ "type": "service", "path": "sshd", "name": "SSH", "ac_val": "active", "type_c_val": "active", "bat_val": "active" }),
        // Path traversal in USB
        json!({ "type": "usb", "path": "/sys/bus/usb/devices/../module/power/control", "name": "USB", "ac_val": "on", "type_c_val": "on", "bat_val": "on" }),
        // Command injection attempt in Wi-Fi interface name
        json!({ "type": "wifi", "path": "wlan0;reboot", "name": "WiFi", "ac_val": "on", "type_c_val": "on", "bat_val": "on" }),
        // Invalid value for PCI power control
        json!({ "type": "pci", "path": "/sys/bus/pci/devices/0000:02:00.0/power/control", "name": "PCI", "ac_val": "unsafe", "type_c_val": "on", "bat_val": "on" }),
        // Excessive numeric value for sound powersave
        json!({ "type": "sys", "path": "/sys/module/snd_hda_intel/parameters/power_save", "name": "Sound", "ac_val": "99999", "type_c_val": "0", "bat_val": "0" }),
    ];

    for entry in test_cases {
        let config = json!({
            "devices": [entry]
        });
        assert!(
            PowerDeviceController::validate(&config).is_err(),
            "Expected configuration to be rejected: {:?}",
            config
        );
    }
}

#[test]
fn test_rescan_preserves_disconnected_devices_and_user_edits() {
    let root = TempDir::new().unwrap();
    let sys = root.path().join("sys");
    write_file(&sys.join("bus/pci/devices/0000:00:00.0/power/control"), "on");
    write_file(&sys.join("devices/system/cpu/intel_pstate/no_turbo"), "0");

    let saved = json!({
        "type": "usb",
        "path": "/sys/bus/usb/devices/1-7/power/control",
        "name": "Custom External USB Keyboard",
        "ac_val": "on",
        "type_c_val": "on",
        "bat_val": "on"
    });

    let controller = PowerDeviceController::new(
        &sys,
        root.path().join("fake-proc"),
        Arc::new(|_, _| true),
        root.path().join("bin"),
    );

    assert!(controller.set_settings(json!({ "devices": [saved.clone()] })).is_ok());

    let scanned = controller.scan();
    let devices = scanned["devices"].as_array().unwrap();

    // 1 saved USB + 1 newly scanned PCI + 1 newly scanned intel_pstate/no_turbo
    assert_eq!(devices.len(), 3);
    assert_eq!(devices[0]["path"], "/sys/bus/usb/devices/1-7/power/control");
    assert_eq!(devices[0]["name"], "Custom External USB Keyboard");

    // Check valid settings round-trip
    assert!(PowerDeviceController::validate(&scanned).is_ok());

    // Check turbo tunable discovered
    let has_turbo = devices.iter().any(|d| d["path"] == "/sys/devices/system/cpu/intel_pstate/no_turbo");
    assert!(has_turbo);
}

#[test]
fn test_cpu_overlay_and_nvidia_blackwell_guard() {
    let root = TempDir::new().unwrap();
    let sys = root.path().join("sys");
    let gpu_base = sys.join("bus/pci/devices/0000:02:00.0");
    write_file(&gpu_base.join("vendor"), "0x10de");
    write_file(&gpu_base.join("class"), "0x030000");
    write_file(&gpu_base.join("power/control"), "auto");
    write_file(&sys.join("devices/system/cpu/intel_pstate/no_turbo"), "0");

    let controller = PowerDeviceController::new(
        &sys,
        root.path().join("proc"),
        Arc::new(|_, _| true),
        root.path().join("bin"),
    );

    let config = json!({
        "enabled": true,
        "keepNvidiaAwake": true,
        "cpu": {
            "bat": { "governor": "powersave", "epp": "power" }
        },
        "devices": [
            {
                "type": "pci",
                "path": "/sys/bus/pci/devices/0000:02:00.0/power/control",
                "name": "NVIDIA GPU",
                "ac_val": "on",
                "type_c_val": "auto",
                "bat_val": "auto"
            },
            {
                "type": "sys",
                "path": "/sys/devices/system/cpu/intel_pstate/no_turbo",
                "name": "No Turbo",
                "ac_val": "0",
                "type_c_val": "0",
                "bat_val": "1"
            }
        ]
    });

    assert!(controller.set_settings(config).is_ok());

    let mut profile = UccProfile::new("default", "Default");
    controller.overlay_cpu(&mut profile, "bat");
    assert_eq!(profile.cpu.governor, "powersave");
    assert!(profile.cpu.no_turbo);

    // In battery mode, keepNvidiaAwake forces GPU power control to "on" and marks guarded = true
    controller.apply("bat");
    assert_eq!(read_file(&gpu_base.join("power/control")), "on");
    let status = controller.status();
    assert!(status["devices"][0]["guarded"].as_bool().unwrap());
}

#[test]
fn test_absent_hardware_reported_as_failure() {
    let root = TempDir::new().unwrap();
    let controller = PowerDeviceController::new(
        root.path(),
        root.path(),
        Arc::new(|_, _| true),
        root.path(),
    );

    let config = json!({
        "enabled": true,
        "devices": [
            {
                "type": "pci",
                "path": "/sys/bus/pci/devices/0000:02:00.0/power/control",
                "name": "Missing PCI Device",
                "ac_val": "on",
                "type_c_val": "auto",
                "bat_val": "auto"
            }
        ]
    });

    assert!(controller.set_settings(config).is_ok());
    controller.apply("ac");

    let status = controller.status();
    assert!(!status["devices"][0]["ok"].as_bool().unwrap());
}

#[test]
fn test_disabling_keep_awake_and_switching_to_battery_undoes_persistence() {
    let root = TempDir::new().unwrap();
    let sys = root.path().join("sys");
    let proc = sys.join("proc");
    let gpu = sys.join("bus/pci/devices/0000:02:00.0");
    write_file(&gpu.join("vendor"), "0x10de");
    write_file(&gpu.join("class"), "0x030000");
    write_file(&gpu.join("power/control"), "auto");
    write_file(&proc.join("driver/nvidia/version"), "570.86.16");

    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_log = Arc::clone(&calls);

    let runner = Arc::new(move |cmd: &str, args: &[&str]| {
        calls_log.lock().unwrap().push(format!("{}:{}", cmd, args.last().unwrap_or(&"")));
        true
    });

    let controller = PowerDeviceController::new(&sys, &proc, runner, root.path().join("bin"));

    let mut config = json!({
        "enabled": true,
        "keepNvidiaAwake": true,
        "devices": [
            {
                "type": "pci",
                "path": "/sys/bus/pci/devices/0000:02:00.0/power/control",
                "name": "NVIDIA GPU",
                "ac_val": "on",
                "type_c_val": "auto",
                "bat_val": "auto"
            }
        ]
    });

    assert!(controller.set_settings(config.clone()).is_ok());

    // When keepNvidiaAwake is true on battery, persistence is enabled (-pm 1)
    controller.apply("bat");
    assert_eq!(calls.lock().unwrap().last().unwrap(), "nvidia-smi:1");

    // Disabling keepNvidiaAwake on battery undoes persistence (-pm 0)
    config["keepNvidiaAwake"] = json!(false);
    assert!(controller.set_settings(config).is_ok());
    controller.apply("bat");
    assert_eq!(calls.lock().unwrap().last().unwrap(), "nvidia-smi:0");
    assert_eq!(read_file(&gpu.join("power/control")), "auto");

    // AC enables persistence again (-pm 1)
    controller.apply("ac");
    assert_eq!(calls.lock().unwrap().last().unwrap(), "nvidia-smi:1");

    // Type-C disables persistence (-pm 0)
    controller.apply("type_c");
    assert_eq!(calls.lock().unwrap().last().unwrap(), "nvidia-smi:0");
    assert!(!controller.status()["nvidiaPersistenceEnabled"].as_bool().unwrap());
}
