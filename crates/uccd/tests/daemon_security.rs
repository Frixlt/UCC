// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use ucc_config::{create_default_settings, ConfigManager};
use ucc_core::{FanPoint, UccProfile};
use ucc_dbus::UccdBackend;
use ucc_hardware::{
    default_process_runner, DeviceCapabilities, DeviceVendor, MockPlatformBackend, MockSysfs,
    PlatformBackend, PlatformBackendKind, PlatformCapabilities, PlatformWrite,
    PowerDeviceController, TelemetryConfig,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{DbusSignalEvent, LidPolicyConfig, LiveUccdBackend, UccdDependencies, WakeupPolicy};

type CustomBackendParts = (
    Arc<LiveUccdBackend>,
    Arc<MockPlatformBackend>,
    Arc<MockSysfs>,
    Arc<MockLightingTransport>,
);

fn build_backend_with_caps(
    tmp: &Path,
    caps: PlatformCapabilities,
    prepare_sysfs: impl FnOnce(&MockSysfs),
    prepare_platform: impl FnOnce(&MockPlatformBackend),
) -> CustomBackendParts {
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/present"), "0-1");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "55000");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");
    prepare_sysfs(&sysfs);

    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_fan_temp(0, 55);
    platform.set_fan_temp(1, 50);
    platform.set_fan_speed(0, 40);
    platform.set_fan_speed(1, 35);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));
    prepare_platform(&platform);

    let lighting = Arc::new(MockLightingTransport::new());
    lighting.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 1,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0],
    }));
    lighting.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 4,
        max_brightness: 255,
    }));

    let config_path = tmp.join("settings.json");
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: platform.clone(),
        sysfs: sysfs.clone(),
        lighting_transport: lighting.clone(),
        config_manager: ConfigManager::with_custom_path(config_path),
        sys_root: tmp.to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp.to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            tmp.join("sys"),
            tmp.join("proc"),
            default_process_runner(),
            tmp.join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));

    (backend, platform, sysfs, lighting)
}

fn default_uniwill_caps() -> PlatformCapabilities {
    PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            water_cooler_supported: true,
            ctgp_supported: true,
            hwp_dynamic_boost_supported: true,
            tdp_control: true,
            charging_thresholds: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    }
}

#[test]
fn t_daemon_01_water_cooler_pump_voltage_protocol_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});

    assert_eq!(
        backend.get_water_cooler_pump_level(),
        -1,
        "unset water cooler pump level must return -1, not 0 (High 11V)"
    );

    assert!(
        !backend.set_water_cooler_pump_voltage(1),
        "discrete code 1 (12V) must be rejected"
    );

    for code in [0, 2, 3, 4] {
        assert!(
            backend.set_water_cooler_pump_voltage(code),
            "discrete code {code} must be accepted"
        );
        assert_eq!(
            backend.get_water_cooler_pump_level(),
            code,
            "pump level must round-trip exact discrete code {code}"
        );
    }
}

#[test]
fn t_daemon_04_manual_wc_controls_disable_auto_control_and_persist() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});
    let mut rx = backend.subscribe_signals();

    assert!(backend.set_water_cooler_fan_speed(75));
    assert!(
        !backend.is_water_cooler_auto_control_enabled(),
        "manual WC fan speed must switch cooler from Auto to Manual"
    );

    let active: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(
        active["fan"]["autoControlWC"], false,
        "manual WC fan control must persist autoControlWC=false on active profile"
    );

    let mut saw_profile_changed = false;
    while let Ok(evt) = rx.try_recv() {
        if matches!(evt, DbusSignalEvent::ProfileChanged { .. }) {
            saw_profile_changed = true;
        }
    }
    assert!(
        saw_profile_changed,
        "manual WC adjustment must emit ProfileChanged"
    );
}

#[test]
fn t_daemon_05_wakeup_policy_no_double_sys_proc_join_and_wired_to_power_devices() {
    let tmp = tempfile::tempdir().unwrap();
    let sys_dir = tmp.path().join("sys");
    let proc_dir = tmp.path().join("proc");
    let acpi_wakeup = proc_dir.join("acpi/wakeup");
    let dev_pw = sys_dir.join("devices/pci0000:00/0000:00:14.0/power/wakeup");
    std::fs::create_dir_all(acpi_wakeup.parent().unwrap()).unwrap();
    std::fs::create_dir_all(dev_pw.parent().unwrap()).unwrap();
    std::fs::write(
        &acpi_wakeup,
        "Device\tS-state\tStatus\tSysfs node\nXHC\tS3\t*enabled\tpci:0000:00:14.0\n",
    )
    .unwrap();
    std::fs::write(&dev_pw, "enabled\n").unwrap();

    let policy = WakeupPolicy::with_sys_proc(&sys_dir, &proc_dir);
    let sysfs = ucc_hardware::RealSysfs;
    let res = policy.apply(true, &sysfs);
    assert!(
        res.disabled >= 2,
        "WakeupPolicy::with_sys_proc must inspect <sys_dir>/devices and <proc_dir>/acpi/wakeup directly without double /sys/sys or /proc/proc; got {res:?}"
    );
    let updated_pw = std::fs::read_to_string(&dev_pw).unwrap();
    assert_eq!(updated_pw.trim(), "disabled");
}

#[test]
fn t_daemon_08_named_fan_profile_preset_resolution_and_temp_curve_isolation() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});

    let cpu_pts = vec![
        FanPoint {
            temp: 40,
            speed: 30,
        },
        FanPoint {
            temp: 80,
            speed: 70,
        },
    ];
    let gpu_pts = vec![
        FanPoint {
            temp: 40,
            speed: 25,
        },
        FanPoint {
            temp: 80,
            speed: 65,
        },
    ];
    assert!(backend.set_fan_profile_dgpu(&serde_json::to_string(&gpu_pts).unwrap()));
    assert!(backend.set_fan_profile_cpu(&serde_json::to_string(&cpu_pts).unwrap()));

    // Verify custom profile with named preset and empty tables works and SetActiveProfile clears temporary curves
    let mut prof = UccProfile::new("named_fan", "Named Fan");
    prof.fan.use_control = true;
    prof.fan.fan_profile = "Balanced".to_string();
    prof.fan.table_cpu.clear();
    prof.fan.table_gpu.clear();
    assert!(backend.add_custom_profile(&serde_json::to_string(&prof).unwrap()));
    assert!(backend.set_active_profile("named_fan"));

    let active: UccProfile = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert!(
        !active.fan.table_cpu.is_empty(),
        "empty fan tables with named preset 'Balanced' must resolve to preset curve rather than falling back empty"
    );
}

#[test]
fn t_daemon_09_startup_charging_settings_and_sysfs_thresholds() {
    let tmp = tempfile::tempdir().unwrap();
    let bat_dir = tmp.path().join("sys/class/power_supply/BAT0");
    let mut cfg = create_default_settings();
    cfg.profiles.values_mut().for_each(|p| {
        p.charging.start_threshold = Some(40);
        p.charging.end_threshold = Some(80);
    });
    let config_path = tmp.path().join("settings.json");
    ConfigManager::with_custom_path(&config_path)
        .save(&cfg)
        .unwrap();

    let (backend, _, sysfs, _) = build_backend_with_caps(
        tmp.path(),
        default_uniwill_caps(),
        |s| {
            s.set_file(bat_dir.join("type"), "Battery");
            s.set_file(bat_dir.join("charge_control_start_threshold"), "0");
            s.set_file(bat_dir.join("charge_control_end_threshold"), "100");
        },
        |_| {},
    );

    backend.activate_hardware();
    assert_eq!(
        sysfs
            .get_file(bat_dir.join("charge_control_start_threshold"))
            .as_deref()
            .map(str::trim),
        Some("40"),
        "activate_hardware must apply saved global charging start threshold"
    );
    assert_eq!(
        sysfs
            .get_file(bat_dir.join("charge_control_end_threshold"))
            .as_deref()
            .map(str::trim),
        Some("80"),
        "activate_hardware must apply saved global charging end threshold"
    );
}

#[test]
fn t_daemon_10_unsupported_vendor_runs_passive_mode_zero_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut caps = PlatformCapabilities::default();
    caps.device.vendor = DeviceVendor::Unsupported;
    let (backend, platform, sysfs, _) = build_backend_with_caps(tmp.path(), caps, |_| {}, |_| {});
    sysfs.clear_journal();
    platform.clear_journal();

    backend.activate_hardware();
    assert!(
        platform.journal().is_empty(),
        "Unsupported device must not issue platform hardware writes: {:?}",
        platform.journal()
    );
    assert!(
        sysfs.journal().is_empty(),
        "Unsupported device must run in passive D-Bus mode with 0 sysfs writes: {:?}",
        sysfs.journal()
    );
}

#[test]
fn t_daemon_11_read_rpcs_avoid_unwrap_lock_poison_panics() {
    let backend_src =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/backend.rs"))
            .unwrap();
    let unwrap_count = backend_src.matches(".unwrap()").count();
    assert!(
        unwrap_count <= 5,
        "crates/uccd/src/backend.rs still has {unwrap_count} raw .unwrap() calls on locks/state that cause cascading panics after worker unwind (ISSUE-19)"
    );
}

#[test]
fn t_daemon_13_signals_emitted_for_mode_reapply_and_power_source_change() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, sysfs, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});
    backend.activate_hardware();
    let mut rx = backend.subscribe_signals();

    // Switch AC offline -> BAT without changing pd_rev
    sysfs.set_file(tmp.path().join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(
        tmp.path().join("sys/class/power_supply/BAT0/type"),
        "Battery",
    );
    backend.reconcile_power_state(false);

    let mut saw_pd = false;
    while let Ok(evt) = rx.try_recv() {
        if matches!(evt, DbusSignalEvent::PowerDeviceStatusChanged { .. }) {
            saw_pd = true;
        }
    }
    assert!(
        saw_pd,
        "PowerDeviceStatusChanged signal must be emitted on AC/Battery switch even when pd_rev is unchanged"
    );
}

#[test]
fn t_daemon_15_custom_profile_crud_empty_id_builtin_collision_and_delete_reconcile() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});
    backend.activate_hardware();

    // 1. Empty ID must auto-generate UUID/ID
    let mut no_id = UccProfile::new("", "Generated Profile");
    no_id.odm_power_limits = vec![42, 70, 90];
    assert!(
        backend.add_custom_profile(&serde_json::to_string(&no_id).unwrap()),
        "AddCustomProfile with empty id must auto-generate id"
    );

    // 2. Built-in name collision must be rejected
    let dup_name = UccProfile::new("not_builtin_id", "Quiet");
    assert!(
        !backend.add_custom_profile(&serde_json::to_string(&dup_name).unwrap()),
        "AddCustomProfile colliding with built-in profile name 'Quiet' must be rejected"
    );

    // 3. Delete currently active custom profile reconciles hardware back to remaining valid profile
    let mut custom = UccProfile::new("del_reconcile", "Temp Custom");
    custom.odm_power_limits = vec![33, 60, 80];
    assert!(backend.add_custom_profile(&serde_json::to_string(&custom).unwrap()));
    assert!(backend.set_active_profile("del_reconcile"));
    assert_eq!(platform.read_tdp(0).unwrap(), 33);

    assert!(backend.delete_custom_profile("del_reconcile"));
    let active_after: UccProfile =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_ne!(active_after.id, "del_reconcile");
    assert_ne!(
        platform.read_tdp(0).unwrap(),
        33,
        "Deleting active custom profile must immediately reconcile hardware"
    );
}

#[test]
fn t_daemon_16_keyboard_wrapped_object_payload_and_profile_keyboard_rgb_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, lighting) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});

    let wrapped = json!({
        "keyboardProfileId": "kb_test",
        "states": [{
            "mode": 0,
            "brightness": 180,
            "red": 12,
            "green": 34,
            "blue": 56
        }]
    });
    assert!(
        backend.set_keyboard_backlight_states_json(&wrapped.to_string()),
        "SetKeyboardBacklightStatesJSON must accept {{keyboardProfileId, states}} object payload from GUI/tray"
    );

    assert!(backend.save_custom_keyboard_profile(
        "kb_prof_rgb",
        "GreenGlow",
        &json!([{
            "mode": 0,
            "brightness": 150,
            "red": 0,
            "green": 255,
            "blue": 128
        }])
        .to_string()
    ));

    let mut prof = UccProfile::new("with_kb", "With KB");
    prof.selected_keyboard_profile = Some("kb_prof_rgb".into());
    lighting.clear_ops();
    assert!(backend.apply_profile(&serde_json::to_string(&prof).unwrap()));
    assert!(
        !lighting.ops().is_empty(),
        "Applying profile with selectedKeyboardProfile must write keyboard backlight states to hardware"
    );
}

struct PwmRampBacklightSysfs {
    inner: Arc<MockSysfs>,
    bl_dir: PathBuf,
    ramp_written_at: std::sync::Mutex<Option<std::time::Instant>>,
}

impl ucc_hardware::Sysfs for PwmRampBacklightSysfs {
    fn read(&self, path: &Path) -> std::io::Result<String> {
        if path == self.bl_dir.join("actual_brightness") {
            if let Some(at) = *self.ramp_written_at.lock().unwrap() {
                // Keep actual_brightness lagging at 55 for 20 ms (>4ms old timeout, <250ms new timeout),
                // then converge to the written target brightness.
                if at.elapsed() < Duration::from_millis(20) {
                    return Ok("55\n".to_string());
                }
                return self.inner.read(&self.bl_dir.join("brightness"));
            }
        }
        self.inner.read(path)
    }

    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        if path == self.bl_dir.join("brightness") && content.trim() == "40" {
            *self.ramp_written_at.lock().unwrap() = Some(std::time::Instant::now());
        }
        self.inner.write(path, content)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
}

#[test]
fn t_daemon_17_backlight_pwm_ramp_lag_does_not_rollback_profile_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let bl = tmp.path().join("sys/class/backlight/intel_backlight");
    let raw_sysfs = Arc::new(MockSysfs::new());
    raw_sysfs.set_file(
        tmp.path()
            .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    raw_sysfs.set_file(tmp.path().join("sys/devices/system/cpu/present"), "0-1");
    raw_sysfs.set_file(tmp.path().join("sys/class/hwmon/hwmon0/name"), "coretemp");
    raw_sysfs.set_file(
        tmp.path().join("sys/class/hwmon/hwmon0/temp1_input"),
        "55000",
    );
    raw_sysfs.set_file(tmp.path().join("sys/class/power_supply/AC/type"), "Mains");
    raw_sysfs.set_file(tmp.path().join("sys/class/power_supply/AC/online"), "1");
    raw_sysfs.set_file(bl.join("max_brightness"), "100\n");
    raw_sysfs.set_file(bl.join("brightness"), "90\n");
    raw_sysfs.set_file(bl.join("actual_brightness"), "90\n");

    let ramp_sysfs: Arc<dyn ucc_hardware::Sysfs> = Arc::new(PwmRampBacklightSysfs {
        inner: raw_sysfs.clone(),
        bl_dir: bl.clone(),
        ramp_written_at: std::sync::Mutex::new(None),
    });

    let platform = Arc::new(MockPlatformBackend::new(default_uniwill_caps()));
    platform.set_fan_temp(0, 55);
    platform.set_fan_temp(1, 50);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));
    let lighting = Arc::new(MockLightingTransport::new());
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: platform.clone(),
        sysfs: ramp_sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(tmp.path().join("settings.json")),
        sys_root: tmp.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            tmp.path().join("sys"),
            tmp.path().join("proc"),
            default_process_runner(),
            tmp.path().join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));
    backend.activate_hardware();

    // Simulate PWM ramp lag: set brightness to 40, actual_brightness stays 55 for 20ms (>4ms old timeout)
    let mut prof = UccProfile::new("pwm_lag", "PWM Lag");
    prof.odm_power_limits = vec![65, 90, 110];
    prof.display.use_brightness = true;
    prof.display.brightness = 40;
    assert!(backend.apply_profile(&serde_json::to_string(&prof).unwrap()));
    assert_eq!(platform.read_tdp(0).unwrap(), 65);
}

#[test]
fn t_daemon_18_two_profile_uniwill_clamps_overboost_startup_default() {
    let tmp = tempfile::tempdir().unwrap();
    let mut caps = default_uniwill_caps();
    caps.perf_profiles_available = 2; // only indices 0, 1 exist; "overboost" is index 2!
    let (backend, platform, _, _) = build_backend_with_caps(tmp.path(), caps, |_| {}, |_| {});

    backend.activate_hardware();
    let odm_writes: Vec<i32> = platform
        .journal()
        .into_iter()
        .filter_map(|w| match w {
            PlatformWrite::SetPerfProfile(idx) => Some(idx),
            _ => None,
        })
        .collect();
    assert!(
        !odm_writes.is_empty() && odm_writes.iter().all(|&idx| idx < 2),
        "2-profile Uniwill must clamp startup 'overboost' default to valid index (< 2); writes={odm_writes:?}"
    );
}

#[test]
fn t_hw_01_cpu_freq_ordering_reset_amd_boost_and_present_core_count() {
    let tmp = tempfile::tempdir().unwrap();
    let cpu0 = tmp.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    let (backend, _, sysfs, _) = build_backend_with_caps(
        tmp.path(),
        default_uniwill_caps(),
        |s| {
            s.set_file(tmp.path().join("sys/devices/system/cpu/present"), "0-7");
            for c in 0..8 {
                let on = if c < 4 { "1" } else { "0" };
                s.set_file(
                    tmp.path()
                        .join(format!("sys/devices/system/cpu/cpu{c}/online")),
                    on,
                );
            }
            s.set_file(cpu0.join("cpuinfo_min_freq"), "400000");
            s.set_file(cpu0.join("cpuinfo_max_freq"), "4500000");
            s.set_file(cpu0.join("scaling_min_freq"), "2500000");
            s.set_file(cpu0.join("scaling_max_freq"), "4000000");
            s.set_file(
                cpu0.join("scaling_available_governors"),
                "powersave performance",
            );
            s.set_file(cpu0.join("scaling_governor"), "powersave");
            s.set_file(tmp.path().join("sys/devices/system/cpu/cpufreq/boost"), "1");
        },
        |_| {},
    );

    assert_eq!(
        backend.get_cpu_core_count(),
        8,
        "GetCpuCoreCount must report present CPUs (8), not merely online count (4)"
    );

    // Lower max below old_min (1800000 < 2500000): must lower min BEFORE lowering max
    let mut prof = UccProfile::new("cpu_low", "CPU Low");
    prof.cpu.min_freq_khz = Some(1_000_000);
    prof.cpu.max_freq_khz = Some(1_800_000);
    prof.cpu.no_turbo = true;
    sysfs.clear_journal();
    assert!(backend.apply_profile(&serde_json::to_string(&prof).unwrap()));

    let min_pos = sysfs
        .journal()
        .iter()
        .position(|w| w.path.ends_with("scaling_min_freq"));
    let max_pos = sysfs
        .journal()
        .iter()
        .position(|w| w.path.ends_with("scaling_max_freq"));
    assert!(
        matches!((min_pos, max_pos), (Some(a), Some(b)) if a < b),
        "when new_max < old_min, scaling_min_freq must be written BEFORE scaling_max_freq: {:?}",
        sysfs.journal()
    );
    assert_eq!(
        sysfs
            .get_file(tmp.path().join("sys/devices/system/cpu/cpufreq/boost"))
            .as_deref()
            .map(str::trim),
        Some("0"),
        "AMD cpufreq/boost must be updated when noTurbo=true"
    );

    // Reset limits back to None -> restore hardware cpuinfo_min_freq / cpuinfo_max_freq
    let mut prof_reset = UccProfile::new("cpu_reset", "CPU Reset");
    prof_reset.cpu.min_freq_khz = None;
    prof_reset.cpu.max_freq_khz = None;
    assert!(backend.apply_profile(&serde_json::to_string(&prof_reset).unwrap()));
    assert_eq!(
        sysfs
            .get_file(cpu0.join("scaling_min_freq"))
            .as_deref()
            .map(str::trim),
        Some("400000")
    );
    assert_eq!(
        sysfs
            .get_file(cpu0.join("scaling_max_freq"))
            .as_deref()
            .map(str::trim),
        Some("4500000")
    );
}

#[test]
fn t_hw_07_no_dgpu_returns_neutral_unknown_vendor() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});
    let info: Value = serde_json::from_str(&backend.get_dgpu_info_values_json()).unwrap();
    assert_ne!(
        info.get("vendor").and_then(|v| v.as_str()),
        Some("nvidia"),
        "System without NVIDIA dGPU must not fabricate vendor=\"nvidia\": {info}"
    );
}

#[test]
fn t_sec_01_uccd_main_refuses_silent_session_bus_and_tmp_pid() {
    let main_src =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
            .unwrap();
    assert!(
        !main_src.contains("\"/tmp/uccd.pid\""),
        "uccd must use $XDG_RUNTIME_DIR/uccd.pid fallback instead of world-writable /tmp/uccd.pid"
    );
    assert!(
        !main_src.contains("starts_with(\"ucc\")"),
        "stop_daemon must verify exact comm == \"uccd\" rather than starts_with(\"ucc\")"
    );

    // Spawn compiled uccd with unreachable system bus and verify it does NOT fall back to session bus
    let out = Command::new("timeout")
        .args(["3s", env!("CARGO_BIN_EXE_uccd"), "--start"])
        .env(
            "DBUS_SYSTEM_BUS_ADDRESS",
            "unix:path=/nonexistent/uccd_audit_test.sock",
        )
        .env_remove("UCC_ALLOW_SESSION_BUS")
        .output()
        .expect("spawn uccd");
    assert!(
        !out.status.success(),
        "uccd --start must fail closed when system D-Bus is unreachable rather than falling back to session bus"
    );
}

#[test]
fn t_sec_11_custom_profile_payload_and_count_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});

    let mut huge = UccProfile::new("huge", "Huge");
    huge.description = "A".repeat(2 * 1024 * 1024);
    assert!(
        !backend.add_custom_profile(&serde_json::to_string(&huge).unwrap()),
        "AddCustomProfile must reject oversized (>1 MiB) profile payloads"
    );
}

#[test]
fn t_daemon_19_get_settings_json_includes_hwp_dynamic_boost_supported() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});
    let settings: Value = serde_json::from_str(&backend.get_settings_json()).unwrap();
    assert_eq!(
        settings
            .get("hwpDynamicBoostSupported")
            .and_then(|v| v.as_bool()),
        Some(true),
        "GetSettingsJSON must include live hwpDynamicBoostSupported flag"
    );
}

#[test]
fn t_daemon_20_lightbar_reconfigure_resets_animation_timer() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});

    assert!(backend.set_lightbar_settings_json(
        &json!({
            "managed": true,
            "welcome_anim": false,
            "power_settings": {
                "ac": { "enabled": true, "brightness_val": 200, "mode": "Pulse (stays on)", "color_mode": [255, 0, 0] }
            }
        })
        .to_string()
    ));
    assert!(backend.trigger_lightbar_animation(true));
    assert!(backend.set_lightbar_settings_json(&json!({ "managed": false }).to_string()));
    let status: Value = serde_json::from_str(&backend.get_lightbar_status_json()).unwrap();
    assert_eq!(
        status["animating"], false,
        "Reconfiguring lightbar to unmanaged must reset animating state"
    );
}

#[test]
fn ta_02_rejected_wc_fan_or_pump_command_keeps_auto_control_active() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});

    assert!(backend.enable_water_cooler(true));
    assert!(backend.is_water_cooler_auto_control_enabled());

    for invalid_fan in [-1, 101, 255, 256] {
        assert!(
            !backend.set_water_cooler_fan_speed(invalid_fan),
            "out-of-range WC fan speed {invalid_fan} must be rejected"
        );
        assert!(
            backend.is_water_cooler_auto_control_enabled(),
            "rejected WC fan speed {invalid_fan} must NOT disable Auto mode"
        );
    }

    assert!(!backend.set_water_cooler_pump_voltage(1)); // 12V forbidden
    assert!(
        backend.is_water_cooler_auto_control_enabled(),
        "rejected 12V pump voltage must NOT disable Auto mode"
    );

    let active: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active["fan"]["autoControlWC"], true);

    platform.set_fan_temp(0, 95);
    platform.set_fan_temp(1, 95);
    let _ = backend.tick_fans_once();
    assert!(
        backend.get_water_cooler_fan_speed() > 0,
        "Auto WC loop must still drive fan speed at 95°C after rejected manual requests"
    );
    assert!(
        backend.get_water_cooler_pump_level() >= 0,
        "Auto WC loop must still drive pump voltage at 95°C after rejected manual requests"
    );
}

#[test]
fn ta_08_ta_09_temporary_wc_curves_drive_wc_auto_and_revert_restores_fan_profile_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) =
        build_backend_with_caps(tmp.path(), default_uniwill_caps(), |_| {}, |_| {});
    let mut rx = backend.subscribe_signals();

    assert!(backend.enable_water_cooler(true));
    backend.set_cooler_connected_state(true);
    let initial_active: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    let orig_fp_id = initial_active["fan"]["fanProfile"]
        .as_str()
        .unwrap()
        .to_string();

    platform.set_fan_temp(0, 60);
    platform.set_fan_temp(1, 60);

    // TA-08: Apply temporary WC fan & pump curves and verify WC Auto controller follows them
    let temp_json = json!({
        "waterCoolerFan": [{"temp": 20, "speed": 90}, {"temp": 100, "speed": 90}],
        "pump": [{"temp": 20, "speed": 3}, {"temp": 100, "speed": 3}],
        "fanProfileId": "fan-freezy"
    })
    .to_string();
    assert!(backend.apply_fan_profiles(&temp_json));
    assert_eq!(
        backend.get_water_cooler_fan_speed(),
        90,
        "TA-08: temporary WC fan curve must drive WC Auto fan speed"
    );
    assert_eq!(
        backend.get_water_cooler_pump_level(),
        0, // discrete 0 = 11V
        "TA-08: temporary WC pump curve must drive WC Auto pump voltage"
    );

    // Reconnect replay also retains temporary WC curves while active
    backend.replay_active_cooler_profile();
    assert_eq!(backend.get_water_cooler_fan_speed(), 90);
    assert_eq!(backend.get_water_cooler_pump_level(), 0);

    let switched: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(switched["fan"]["fanProfile"], "fan-freezy");

    while rx.try_recv().is_ok() {}

    // TA-09: RevertFanProfiles restores original fanProfile ID, emits ProfileChanged, and reverts WC Auto targets
    assert!(backend.revert_fan_profiles());
    let reverted: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(
        reverted["fan"]["fanProfile"].as_str(),
        Some(orig_fp_id.as_str()),
        "TA-09: RevertFanProfiles must restore original fanProfile ID"
    );
    assert_ne!(
        backend.get_water_cooler_fan_speed(),
        90,
        "TA-08: RevertFanProfiles must restore WC fan target to active profile curve"
    );

    let mut emitted_orig_fp = false;
    while let Ok(evt) = rx.try_recv() {
        if let DbusSignalEvent::ProfileChanged { fan_profile_id, .. } = evt {
            if fan_profile_id == orig_fp_id {
                emitted_orig_fp = true;
            }
        }
    }
    assert!(
        emitted_orig_fp,
        "TA-09: RevertFanProfiles must emit ProfileChanged with restored fanProfile ID"
    );
}

#[cfg(unix)]
#[test]
fn sleep_state_store_rejects_symlink_and_oversized_file() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    use uccd::{SleepState, SleepStateStore};

    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("ucc-sleep");
    std::fs::create_dir(&state_dir).unwrap();
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let uid = unsafe { nix::libc::geteuid() };

    // 1. Symlink at snapshot path must be rejected by both load() and save().
    let target_path = state_dir.join("real-target.json");
    let original_target = r#"{"version":1,"wasActive":true,"manualMode":"ac","coolerEnabled":false}"#;
    std::fs::write(&target_path, original_target).unwrap();
    std::fs::set_permissions(&target_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let symlink_path = state_dir.join("sleep-state.json");
    symlink(&target_path, &symlink_path).unwrap();

    let symlink_store = SleepStateStore::new(&symlink_path, uid);
    assert!(
        symlink_store.load().is_err(),
        "SleepStateStore::load must reject symlink snapshot"
    );
    assert!(
        symlink_store.save(&SleepState::default()).is_err(),
        "SleepStateStore::save must reject symlink snapshot"
    );
    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        original_target,
        "SleepStateStore::save must not follow symlink or modify target file"
    );

    // 2. Oversized regular file (> 4096 bytes) must be rejected by both load() and save().
    let oversized_path = state_dir.join("sleep-state-oversized.json");
    std::fs::write(&oversized_path, vec![b' '; 4097]).unwrap();
    std::fs::set_permissions(&oversized_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let oversized_store = SleepStateStore::new(&oversized_path, uid);
    assert!(
        oversized_store.load().is_err(),
        "SleepStateStore::load must reject oversized (>4096-byte) snapshot"
    );
    assert!(
        oversized_store.save(&SleepState::default()).is_err(),
        "SleepStateStore::save must reject oversized (>4096-byte) snapshot"
    );
}
