// SPDX-License-Identifier: GPL-3.0-or-later

#[allow(unused_imports)]
use super::common::*;
#[allow(unused_imports)]
use serde_json::json;
#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use std::path::Path;
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use ucc_config::{
    create_default_settings, default_high_performance_profile, default_quiet_profile, ConfigManager,
};
#[allow(unused_imports)]
use ucc_dbus::{
    AllowAllAuthorizer, UccdBackend, UccdLegacyProxy, UccdProxy, DBUS_PATH, DBUS_PATH_ALT,
};
#[allow(unused_imports)]
use ucc_hardware::{
    decode_history_binary, default_process_runner, DeviceCapabilities, DeviceVendor, MetricId,
    MockPlatformBackend, MockSysfs, NvidiaPowerGuard, NvmlWrapper, PlatformBackend,
    PlatformBackendKind, PlatformCapabilities, PlatformError, PlatformWrite, PowerDeviceController,
    TelemetryConfig,
};
#[allow(unused_imports)]
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
#[allow(unused_imports)]
use uccd::{
    apply_wakeup_policy, is_allowed_wakeup_device, DaemonRuntime, LidAction, LidPolicyConfig,
    LiveUccdBackend, UccdDependencies, WakeupApplyResult, WakeupPolicy,
};
#[allow(unused_imports)]
use zbus::export::futures_core::Stream;
#[allow(unused_imports)]
use zbus::zvariant::OwnedValue;

#[tokio::test]
async fn m8_10_lid_close_blanks_and_restores_keyboard_and_wc_led_in_ram_only() {
    use ucc_dbus::UccdBackend;
    use ucc_hardware::{CoolerPacketEncoder, RgbState, RgbTarget};

    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, lighting) = build_test_backend(root.path());
    let name = "org.test.uccd.m8_10_lid_lighting";
    let runtime = start_runtime(Arc::clone(&backend), name, false).await;

    let initial_kb = json!([{
        "brightness": 180,
        "red": 12,
        "green": 140,
        "blue": 255,
        "mode": 0
    }])
    .to_string();
    assert!(backend.set_keyboard_backlight_states_json(&initial_kb));
    let before_settings = std::fs::read_to_string(root.path().join("settings.json")).unwrap();

    // Configure both lid blanking toggles and connect water cooler with an active LED color.
    let pd_both = json!({
        "enabled": false,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "screen_off",
            "type_c": "screen_off",
            "bat": "suspend",
            "wakeOnlyLidAndKeyboard": true,
            "turnOffKeyboardBacklightOnLidClose": true,
            "turnOffWaterCoolerLedOnLidClose": true
        }
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_both));
    backend.set_cooler_connected_state(true);
    assert!(backend.set_water_cooler_led_color(200, 30, 90, 0));
    let _ = drain_cooler_packets(&backend, 5).await;

    let after_cfg_settings = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
    let _ = before_settings;

    // 1. Close lid: keyboard brightness becomes 0 and WC LED turns off without altering settings.json.
    backend.on_lid_state_changed(true);
    assert!(backend.is_lid_closed());
    let kb_while_closed: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_while_closed[0]["brightness"].as_u64(), Some(0));
    assert_eq!(backend.cooler_desired_rgb(), RgbTarget::Off);
    let pkts = drain_cooler_packets(&backend, 5).await;
    assert!(
        pkts.contains(&CoolerPacketEncoder::turn_off(ucc_hardware::CMD_RGB)),
        "closing lid must enqueue TurnOffLed packet when WC LED blanking is enabled: {pkts:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("settings.json")).unwrap(),
        after_cfg_settings,
        "lid close blanking must live in RAM only and never mutate settings.json"
    );

    // 2. While lid stays closed, explicit LED/keyboard updates stay blanked on hardware but update RAM snapshot.
    let updated_kb = json!([{
        "brightness": 220,
        "red": 255,
        "green": 0,
        "blue": 100,
        "mode": 0
    }])
    .to_string();
    assert!(backend.set_keyboard_backlight_states_json(&updated_kb));
    let kb_still_dark: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_still_dark[0]["brightness"].as_u64(), Some(0));

    // 3. Reopen lid: previous keyboard states (updated RAM brightness 220) and WC LED color restore.
    backend.on_lid_state_changed(false);
    assert!(!backend.is_lid_closed());
    let kb_reopened: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_reopened[0]["brightness"].as_u64(), Some(220));
    assert_eq!(
        backend.cooler_desired_rgb(),
        RgbTarget::Color {
            red: 200,
            green: 30,
            blue: 90,
            state: RgbState::Static
        }
    );
    let _ = lighting;

    // 4. Disable both toggles: closing lid leaves keyboard & WC LED untouched.
    let pd_off = json!({
        "enabled": false,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "screen_off",
            "type_c": "screen_off",
            "bat": "suspend",
            "wakeOnlyLidAndKeyboard": true,
            "turnOffKeyboardBacklightOnLidClose": false,
            "turnOffWaterCoolerLedOnLidClose": false
        }
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_off));
    backend.on_lid_state_changed(true);
    let kb_unchanged: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_unchanged[0]["brightness"].as_u64(), Some(220));
    assert_ne!(backend.cooler_desired_rgb(), RgbTarget::Off);

    runtime.shutdown(Duration::from_secs(2)).await;
}

async fn drain_cooler_packets(backend: &LiveUccdBackend, max_rounds: usize) -> Vec<[u8; 8]> {
    let mut out = Vec::new();
    for _ in 0..max_rounds {
        tokio::time::sleep(Duration::from_millis(90)).await;
        let batch = backend.drain_cooler_commands();
        if batch.is_empty() {
            if !out.is_empty() {
                break;
            }
        } else {
            out.extend(batch);
        }
    }
    out
}

#[tokio::test]
async fn m5_9_startup_auto_enables_wc_scan_when_power_wc_profile_requests_it() {
    use ucc_dbus::UccdBackend;

    let root = tempfile::tempdir().unwrap();
    let mut ac_profile = default_high_performance_profile();
    ac_profile.id = "ac-no-wc".to_string();
    ac_profile.name = "AC No WC".to_string();
    ac_profile.fan.enable_water_cooler = false;

    let mut wc_profile = default_high_performance_profile();
    wc_profile.id = "wc-auto-scan".to_string();
    wc_profile.name = "WC Auto Scan".to_string();
    wc_profile.fan.enable_water_cooler = true;

    let mut settings = create_default_settings();
    settings
        .profiles
        .insert(ac_profile.id.clone(), ac_profile.clone());
    settings
        .profiles
        .insert(wc_profile.id.clone(), wc_profile.clone());
    settings.set_active_profile_for_mode("power_ac", ac_profile.id);
    settings.set_active_profile_for_mode("power_wc", wc_profile.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();

    let (backend, _, _, _) = build_test_backend(root.path());
    assert!(
        backend.is_water_cooler_enabled(),
        "WC scanning must auto-enable at startup when power_wc profile enables water cooler"
    );
}

#[tokio::test]
async fn m5_9_manual_wc_fan_and_pump_profile_apply_and_reconnect_replay() {
    use ucc_dbus::UccdBackend;
    use ucc_hardware::{CoolerPacketEncoder, PumpVoltage};

    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = start_runtime(Arc::clone(&backend), "org.test.uccd.m5_9_manual", false).await;

    let mut manual_profile = default_high_performance_profile();
    manual_profile.fan.enable_water_cooler = true;
    manual_profile.fan.auto_control_wc = false;
    manual_profile.fan.manual_fan_speed = Some(65);
    manual_profile.fan.manual_pump_voltage = Some(2); // discrete code 2 == 7V

    assert!(backend.apply_profile(&serde_json::to_string(&manual_profile).unwrap()));
    assert!(!backend.is_water_cooler_auto_control_enabled());
    assert_eq!(backend.get_water_cooler_fan_speed(), 65);
    assert_eq!(backend.get_water_cooler_pump_level(), 2);

    backend.set_cooler_connected_state(true);
    let pkts = drain_cooler_packets(&backend, 4).await;
    assert!(
        pkts.contains(&CoolerPacketEncoder::fan_speed(65).unwrap()),
        "manual fan speed 65% must be flushed on connect: {pkts:?}"
    );
    assert!(
        pkts.contains(&CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap()),
        "manual pump voltage V7 must be flushed on connect: {pkts:?}"
    );

    // Simulate transient BLE drop + debounced reconnect replay.
    backend.set_cooler_connected_state(false);
    backend.set_cooler_connected_state(true);
    backend.replay_active_cooler_profile();
    let replayed = drain_cooler_packets(&backend, 4).await;
    assert!(
        replayed.contains(&CoolerPacketEncoder::fan_speed(65).unwrap()),
        "manual fan speed 65% must replay after reconnect: {replayed:?}"
    );
    assert!(
        replayed.contains(&CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap()),
        "manual pump voltage V7 must replay after reconnect: {replayed:?}"
    );

    runtime.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn m5_9_auto_wc_curve_and_aquaris_led_restore_across_suspend_resume() {
    use ucc_core::{FanPoint, PumpPoint};
    use ucc_dbus::UccdBackend;
    use ucc_hardware::{CoolerPacketEncoder, PumpVoltage, RgbState, RgbTarget};

    let root = tempfile::tempdir().unwrap();
    let aquaris_path = root.path().join("aquaris.json");
    std::fs::write(
        &aquaris_path,
        json!({
            "address": "AA:BB:CC:DD:EE:FF",
            "name": "LCT21001",
            "red": 10,
            "green": 180,
            "blue": 240,
            "ledMode": 1
        })
        .to_string(),
    )
    .unwrap();

    let (backend, _, _, _) = build_test_backend(root.path());
    backend.load_aquaris_saved_state_from(&aquaris_path);
    let runtime = start_runtime(Arc::clone(&backend), "org.test.uccd.m5_9_resume", false).await;

    let mut auto_profile = default_high_performance_profile();
    auto_profile.fan.enable_water_cooler = true;
    auto_profile.fan.auto_control_wc = true;
    auto_profile.fan.table_water_cooler_fan = vec![
        FanPoint {
            temp: 40,
            speed: 35,
        },
        FanPoint {
            temp: 60,
            speed: 75,
        },
        FanPoint {
            temp: 80,
            speed: 95,
        },
    ];
    auto_profile.fan.table_pump = vec![
        PumpPoint { temp: 40, speed: 1 }, // V7
        PumpPoint { temp: 60, speed: 2 }, // V8
        PumpPoint { temp: 80, speed: 3 }, // V11
    ];
    assert!(backend.apply_profile(&serde_json::to_string(&auto_profile).unwrap()));

    // Enter suspend: flushing must stay blocked even if targets were queued.
    backend.on_system_suspend();
    assert!(backend.drain_cooler_commands().is_empty());

    // Resume + reconnect: saved aquaris.json RGB and Auto curve targets (at 62C -> 77% fan? check interpolation) restore.
    backend.on_system_resume();
    backend.set_cooler_connected_state(true);

    assert_eq!(
        backend.cooler_desired_rgb(),
        RgbTarget::Color {
            red: 10,
            green: 180,
            blue: 240,
            state: RgbState::Breathe,
        }
    );
    let fan_pct = backend.get_water_cooler_fan_speed();
    assert!(
        (75..=95).contains(&fan_pct),
        "auto WC curve fan at 62C must be evaluated in 75..=95%, got {fan_pct}"
    );
    assert_eq!(
        backend.get_water_cooler_pump_level(),
        3,
        "auto WC pump curve at 62C must select discrete code 3 (8V)"
    );

    let pkts = drain_cooler_packets(&backend, 5).await;
    assert!(
        pkts.contains(&CoolerPacketEncoder::fan_speed(fan_pct as u8).unwrap()),
        "resumed auto WC fan speed packet missing: {pkts:?}"
    );
    assert!(
        pkts.contains(&CoolerPacketEncoder::pump_voltage(PumpVoltage::V8).unwrap()),
        "resumed auto WC pump packet missing: {pkts:?}"
    );
    assert!(
        pkts.contains(&CoolerPacketEncoder::rgb(10, 180, 240, RgbState::Breathe)),
        "saved aquaris.json LED packet must restore on resume reconnect: {pkts:?}"
    );

    runtime.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn full_27_01_sleep_state_survives_resume_until_daemon_consumes_it() {
    use std::os::unix::fs::MetadataExt;
    use ucc_dbus::UccdBackend;
    use uccd::{resume_sleep, SleepOperations, SleepState, SleepStateStore};

    struct StartSucceedsImmediateOps;
    impl SleepOperations for StartSucceedsImmediateOps {
        fn is_daemon_active(&self) -> Result<bool, String> {
            Ok(true)
        }
        fn capture_runtime_state(&self) -> Result<SleepState, String> {
            unreachable!()
        }
        fn stop_daemon(&self) -> Result<(), String> {
            unreachable!()
        }
        fn start_daemon(&self) -> Result<(), String> {
            Ok(())
        }
    }

    struct StartFailsOps;
    impl SleepOperations for StartFailsOps {
        fn is_daemon_active(&self) -> Result<bool, String> {
            Ok(true)
        }
        fn capture_runtime_state(&self) -> Result<SleepState, String> {
            unreachable!()
        }
        fn stop_daemon(&self) -> Result<(), String> {
            unreachable!()
        }
        fn start_daemon(&self) -> Result<(), String> {
            Err("simulated start failure".to_string())
        }
    }

    let root = tempfile::tempdir().unwrap();
    let run_ucc = root.path().join("run/ucc");
    std::fs::create_dir_all(&run_ucc).unwrap();
    let uid = std::fs::metadata(root.path()).unwrap().uid();
    let state_path = run_ucc.join("sleep-state.json");
    let store = SleepStateStore::new(&state_path, uid);

    // 1. Active snapshot survives resume_sleep even when start_daemon fails or returns immediately.
    let expected = SleepState {
        was_active: true,
        manual_mode: "type_c".to_string(),
        cooler_enabled: true,
    };
    store.save(&expected).unwrap();
    assert!(resume_sleep(&store, &StartFailsOps).is_err());
    assert_eq!(store.load().unwrap(), Some(expected.clone()));
    resume_sleep(&store, &StartSucceedsImmediateOps).unwrap();
    assert_eq!(store.load().unwrap(), Some(expected.clone()));
    // Repeated resume before daemon consumes it preserves the snapshot.
    resume_sleep(&store, &StartSucceedsImmediateOps).unwrap();
    assert_eq!(store.load().unwrap(), Some(expected));

    // 2. When DaemonRuntime starts, it consumes and applies the snapshot and clears the file only afterward.
    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = start_runtime(Arc::clone(&backend), "org.test.uccd.full_27_01", false).await;
    assert!(backend.is_water_cooler_enabled());
    let policy: serde_json::Value = serde_json::from_str(&backend.get_power_policy_json()).unwrap();
    assert_eq!(policy["manual_mode"], "type_c");
    assert_eq!(
        store.load().unwrap(),
        None,
        "snapshot must be cleared once daemon activation consumes it"
    );
    runtime.shutdown(Duration::from_secs(1)).await;

    // 3. Inactive snapshot (was_active=false) is cleared directly by resume_sleep.
    store
        .save(&SleepState {
            was_active: false,
            manual_mode: "bat".to_string(),
            cooler_enabled: false,
        })
        .unwrap();
    resume_sleep(&store, &StartSucceedsImmediateOps).unwrap();
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn apply_wakeup_policy_skips_symlink_loops_and_non_dirs() {
    use ucc_hardware::RealSysfs;

    let tmp = tempfile::tempdir().unwrap();
    let pci_dev = tmp.path().join("sys/devices/pci0000:00/0000:00:14.0");
    std::fs::create_dir_all(pci_dev.join("power")).unwrap();
    std::fs::write(pci_dev.join("power/wakeup"), "enabled\n").unwrap();
    std::fs::write(pci_dev.join("modalias"), "pci:dummy\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("..", pci_dev.join("device")).unwrap();

    let started = Instant::now();
    let policy = WakeupPolicy::with_sys_proc(tmp.path().join("sys"), tmp.path().join("proc"));
    let res = policy.apply(true, &RealSysfs);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "wakeup walk should skip symlink loops rapidly, took {:?}",
        started.elapsed()
    );
    assert_eq!(res.disabled, 1);
    assert_eq!(
        std::fs::read_to_string(pci_dev.join("power/wakeup"))
            .unwrap()
            .trim(),
        "disabled"
    );
}
