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

#[allow(unused_imports)]
use super::sysfs_gate_common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn full_27_02_superseded_charging_ctgp_and_rgb_never_overwrite_winner() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let end_path = bat.join("charge_control_end_threshold");
    let start_path = bat.join("charge_control_start_threshold");
    let ctgp_path = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *ctrl.gate.lock().unwrap() = Some((
        "charge_control_end_threshold".to_string(),
        "70".to_string(),
        started_tx,
        release_rx,
    ));

    let (backend, _, sysfs, lighting) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&start_path, "30");
            sysfs.set_file(&end_path, "80");
            sysfs.set_file(&ctgp_path, "0");
        },
        |p| p,
    );

    assert!(backend.save_custom_keyboard_profile(
        "kb-stale",
        "KB Stale Red",
        r#"[{"brightness":255,"red":255,"green":0,"blue":0}]"#,
    ));
    assert!(backend.save_custom_keyboard_profile(
        "kb-winner",
        "KB Winner Green",
        r#"[{"brightness":255,"red":0,"green":255,"blue":0}]"#,
    ));

    let mut stale = default_quiet_profile();
    stale.id = "stale-70".into();
    stale.name = "Stale 70".into();
    stale.charging.end_threshold = Some(70);
    stale.nvidia_ctgp_offset = Some(5);
    stale.keyboard.keyboard_profile_id = "kb-stale".into();

    let mut winner = default_high_performance_profile();
    winner.id = "winner-85".into();
    winner.name = "Winner 85".into();
    winner.charging.end_threshold = Some(85);
    winner.nvidia_ctgp_offset = Some(15);
    winner.keyboard.keyboard_profile_id = "kb-winner".into();

    let b1 = Arc::clone(&backend);
    let stale_json = serde_json::to_string(&stale).unwrap();
    let stale_handle = tokio::task::spawn_blocking(move || b1.apply_profile(&stale_json));

    started_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("stale thread should reach charging threshold gate");

    let winner_json = serde_json::to_string(&winner).unwrap();
    assert!(
        backend.apply_profile(&winner_json),
        "winning profile should commit cleanly"
    );

    let _ = release_tx.send(());
    let _ = stale_handle.await.unwrap();

    let active: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active["id"], "winner-85");
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("85"));
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("15"));
    let kbd_colors =
        ucc_lighting::LightingTransport::read_keyboard_colors(lighting.as_ref()).unwrap();
    assert_eq!(kbd_colors.len(), 1);
    assert_eq!(kbd_colors[0].r, 0);
    assert_eq!(kbd_colors[0].g, 255);
    assert_eq!(kbd_colors[0].b, 0);
}

#[test]
fn full_27_03_apply_profile_fails_and_rolls_back_on_charging_write_error() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&start_path, "40");
            sysfs.set_file(&end_path, "80");
        },
        |p| p,
    );

    let before: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    let mut bad_prof = default_high_performance_profile();
    bad_prof.id = "charge-fail".into();
    bad_prof.name = "Charge Fail".into();
    bad_prof.charging.start_threshold = Some(50);
    bad_prof.charging.end_threshold = Some(90);
    let bad_json = serde_json::to_string(&bad_prof).unwrap();

    // 1. PermissionDenied on charge_control_end_threshold -> fails & rolls back start threshold.
    *ctrl.deny_write_suffix.lock().unwrap() = Some("charge_control_end_threshold".to_string());
    assert!(!backend.apply_profile(&bad_json));
    let after1: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(after1["id"], before["id"]);
    assert_eq!(sysfs.get_file(&start_path).as_deref(), Some("40"));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));
    *ctrl.deny_write_suffix.lock().unwrap() = None;

    // 2. Write-without-effect on charge_control_end_threshold -> readback detects mismatch -> fails & rolls back.
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charge_control_end_threshold".to_string());
    assert!(!backend.apply_profile(&bad_json));
    let after2: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(after2["id"], before["id"]);
    assert_eq!(sysfs.get_file(&start_path).as_deref(), Some("40"));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));
    *ctrl.silent_noop_suffix.lock().unwrap() = None;

    // 3. Missing charging node when battery is present and profile configures end_threshold -> fails.
    let root2 = tempfile::tempdir().unwrap();
    let bat2 = root2.path().join("sys/class/power_supply/BAT0");
    let ctrl2 = Arc::new(SysfsGateControl::default());
    let (backend2, _, _, _) = build_backend_with_sysfs_gate(
        root2.path(),
        ctrl2,
        |sysfs| {
            sysfs.set_file(bat2.join("type"), "Battery");
        },
        |p| p,
    );
    assert!(!backend2.apply_profile(&bad_json));
}

#[test]
fn full_27_04_delete_active_custom_profile_preserves_disk_when_fallback_fails() {
    let root = tempfile::tempdir().unwrap();
    let ctrl_platform_slot: Arc<std::sync::Mutex<Option<Arc<AuditFallbackFailPlatform>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let slot_clone = Arc::clone(&ctrl_platform_slot);
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        move |inner| {
            let wrapped = Arc::new(AuditFallbackFailPlatform {
                inner,
                fail_on_perf_profile: std::sync::atomic::AtomicBool::new(false),
            });
            *slot_clone.lock().unwrap() = Some(Arc::clone(&wrapped));
            wrapped
        },
    );
    let wrapper = ctrl_platform_slot.lock().unwrap().take().unwrap();

    let mut custom = default_high_performance_profile();
    custom.id = "cust-active".into();
    custom.name = "Custom Active".into();
    custom.odm_profile_name = Some("performance".into());
    custom.odm_power_limits = vec![45, 70, 105];
    assert!(backend.save_custom_profile(&serde_json::to_string(&custom).unwrap()));

    let mut inactive = default_quiet_profile();
    inactive.id = "cust-inactive".into();
    inactive.name = "Custom Inactive".into();
    assert!(backend.save_custom_profile(&serde_json::to_string(&inactive).unwrap()));

    assert!(backend.set_state_map("power_ac", "cust-active"));
    assert!(backend.set_active_profile("cust-active"));
    assert_eq!(platform.read_tdp(0).unwrap(), 45);

    // When fallback ODM switch fails, DeleteCustomProfile MUST fail and preserve disk & HW state.
    wrapper
        .fail_on_perf_profile
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        !backend.delete_custom_profile("cust-active"),
        "deleting active profile must fail when fallback hardware restore fails"
    );
    let custom_list: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_custom_profiles_json()).unwrap();
    assert!(
        custom_list.iter().any(|p| p["id"] == "cust-active"),
        "deleted profile must stay on disk when fallback apply fails"
    );
    let active_after_fail: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active_after_fail["id"], "cust-active");
    assert_eq!(platform.read_tdp(0).unwrap(), 45);
    let disk_raw = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
    assert!(disk_raw.contains("cust-active"));

    // Deleting an unrelated inactive custom profile succeeds cleanly without touching active profile.
    assert!(backend.delete_custom_profile("cust-inactive"));
    let active_still: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active_still["id"], "cust-active");

    // Once hardware accepts fallback, deleting active profile succeeds and removes it from disk.
    wrapper
        .fail_on_perf_profile
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(backend.delete_custom_profile("cust-active"));
    let active_after_ok: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_ne!(active_after_ok["id"], "cust-active");
    let custom_list_after: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_custom_profiles_json()).unwrap();
    assert!(!custom_list_after.iter().any(|p| p["id"] == "cust-active"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn unmanaged_ctgp_and_keyboard_do_not_inherit_superseded_settings() {
    use ucc_lighting::{LightingTransport, RgbColor};
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let end_path = bat.join("charge_control_end_threshold");
    let start_path = bat.join("charge_control_start_threshold");
    let ctgp_path = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *ctrl.gate.lock().unwrap() = Some((
        "charge_control_end_threshold".to_string(),
        "70".to_string(),
        started_tx,
        release_rx,
    ));

    let (backend, _, sysfs, lighting) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&start_path, "30");
            sysfs.set_file(&end_path, "80");
            sysfs.set_file(&ctgp_path, "0");
        },
        |p| p,
    );

    assert!(backend.save_custom_keyboard_profile(
        "kb-stale",
        "KB Stale Red",
        r#"[{"brightness":255,"red":255,"green":0,"blue":0}]"#,
    ));
    assert!(backend.save_custom_keyboard_profile(
        "kb-winner",
        "KB Winner Green",
        r#"[{"brightness":255,"red":0,"green":255,"blue":0}]"#,
    ));

    let mut stale = default_quiet_profile();
    stale.id = "stale-70".into();
    stale.name = "Stale 70".into();
    stale.charging.end_threshold = Some(70);
    stale.nvidia_ctgp_offset = Some(5);
    stale.keyboard.keyboard_profile_id = "kb-stale".into();

    let mut winner = default_high_performance_profile();
    winner.id = "winner-85".into();
    winner.name = "Winner 85".into();
    winner.charging.end_threshold = Some(85);
    winner.nvidia_ctgp_offset = None;
    winner.keyboard.keyboard_profile_id.clear();
    winner.selected_keyboard_profile = None;
    let before = RgbColor { r: 0, g: 0, b: 255 };
    lighting.write_keyboard_zones(&[before]).unwrap();

    let b1 = Arc::clone(&backend);
    let stale_json = serde_json::to_string(&stale).unwrap();
    let stale_handle = tokio::task::spawn_blocking(move || b1.apply_profile(&stale_json));

    started_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("stale thread should reach charging threshold gate");

    let winner_json = serde_json::to_string(&winner).unwrap();
    assert!(
        backend.apply_profile(&winner_json),
        "winning profile should commit cleanly"
    );

    let _ = release_tx.send(());
    let _ = stale_handle.await.unwrap();

    let active: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active["id"], "winner-85");
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("85"));
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("0"));
    assert_eq!(lighting.read_keyboard_colors().unwrap(), vec![before]);
}

#[tokio::test]
async fn rejected_settings_save_restores_ctgp_and_keyboard_state() {
    use ucc_lighting::{LightingTransport, RgbColor};
    let root = tempfile::tempdir().unwrap();
    let ctgp_path = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");
    let (backend, _, sysfs, lighting) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::new(SysfsGateControl::default()),
        |sysfs| sysfs.set_file(&ctgp_path, "0"),
        |p| p,
    );
    let before = RgbColor { r: 0, g: 0, b: 255 };
    lighting.write_keyboard_zones(&[before]).unwrap();
    assert!(backend.save_custom_keyboard_profile(
        "red-kb",
        "Red KB",
        r#"[{"brightness":255,"red":255,"green":0,"blue":0}]"#,
    ));
    let mut attempted = default_quiet_profile();
    attempted.id = "unsavable-profile".to_string();
    attempted.odm_profile_name = None;
    attempted.nvidia_ctgp_offset = Some(5);
    attempted.keyboard.keyboard_profile_id = "red-kb".into();
    assert!(backend.save_custom_profile(&serde_json::to_string(&attempted).unwrap()));
    let cfg = root.path().join("settings.json");
    std::fs::remove_file(&cfg).unwrap();
    std::fs::create_dir(&cfg).unwrap();

    assert!(!backend.set_active_profile(&attempted.id));
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("0"));
    assert_eq!(lighting.read_keyboard_colors().unwrap(), vec![before]);
}

#[tokio::test]
async fn charging_token_bracket_check_rejects_unselected_option() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let charge_type_path = bat.join("charge_type");
    let prof_avail_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profiles_available");
    let prof_cur_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profile");
    let prio_avail_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_priority/charging_prios_available");
    let prio_cur_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_priority/charging_prio");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, _sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&charge_type_path, "[Standard] Fast");
            sysfs.set_file(&prof_avail_path, "high_capacity balanced stationary");
            sysfs.set_file(&prof_cur_path, "[high_capacity] balanced stationary");
            sysfs.set_file(&prio_avail_path, "performance battery");
            sysfs.set_file(&prio_cur_path, "[performance] battery");
        },
        |p| p,
    );

    // 1. charge_type unchanged bracketed token must fail without publishing profile.
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charge_type".to_string());
    let mut p_ct = default_quiet_profile();
    p_ct.id = "fast-ct".into();
    p_ct.charging.charge_type = Some(ucc_core::ChargeType::Fast);
    assert!(
        !backend.apply_profile(&serde_json::to_string(&p_ct).unwrap()),
        "charge_type [Standard] Fast must reject Fast"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&backend.get_active_profile_json()).unwrap()
            ["id"],
        "unapplied"
    );

    // 2. charging_profile unchanged bracketed token must fail.
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charging_profile".to_string());
    let mut p_prof = default_quiet_profile();
    p_prof.id = "stationary-prof".into();
    p_prof.charging.charging_profile = Some("stationary".into());
    assert!(
        !backend.apply_profile(&serde_json::to_string(&p_prof).unwrap()),
        "charging_profile [high_capacity] stationary must reject stationary"
    );

    // 3. charging_priority unchanged bracketed token must fail.
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charging_prio".to_string());
    let mut p_prio = default_quiet_profile();
    p_prio.id = "bat-prio".into();
    p_prio.charging.charging_priority = Some("battery".into());
    assert!(
        !backend.apply_profile(&serde_json::to_string(&p_prio).unwrap()),
        "charging_priority [performance] battery must reject battery"
    );
}

#[tokio::test]
async fn charging_quantized_readback_failure_restores_all_thresholds() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&start_path, "40");
            sysfs.set_file(&end_path, "80");
        },
        |p| p,
    );

    // Case A: start first (50 < 80), then end 85 quantized to 84 -> both restore to (40, 80).
    *ctrl.rewrite_value.lock().unwrap() = Some((
        "charge_control_end_threshold".to_string(),
        "85".to_string(),
        "84".to_string(),
    ));
    let mut p_up = default_quiet_profile();
    p_up.id = "quant-up".into();
    p_up.charging.start_threshold = Some(50);
    p_up.charging.end_threshold = Some(85);
    assert!(!backend.apply_profile(&serde_json::to_string(&p_up).unwrap()));
    assert_eq!(sysfs.get_file(&start_path).as_deref(), Some("40"));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));

    // Case B: end-first ordering (start=85 >= cur_end=80), end writes 95, start 85 quantized to 84 -> both restore to (40, 80).
    *ctrl.rewrite_value.lock().unwrap() = Some((
        "charge_control_start_threshold".to_string(),
        "85".to_string(),
        "84".to_string(),
    ));
    let mut p_end_first = default_quiet_profile();
    p_end_first.id = "quant-end-first".into();
    p_end_first.charging.start_threshold = Some(85);
    p_end_first.charging.end_threshold = Some(95);
    assert!(!backend.apply_profile(&serde_json::to_string(&p_end_first).unwrap()));
    assert_eq!(sysfs.get_file(&start_path).as_deref(), Some("40"));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));
}

#[test]
fn uniwill_profile_with_charge_type_succeeds_when_bat0_lacks_charge_nodes_and_hydrates_kbd() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let prof_avail_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profiles_available");
    let prof_cur_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profile");
    let kbd_leds = root
        .path()
        .join("sys/bus/hid/drivers/tuxedo-keyboard-ite/0003:048D:600B.0001/leds");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&prof_avail_path, "high_capacity balanced stationary");
            sysfs.set_file(&prof_cur_path, "[balanced] high_capacity stationary");
            for id in 0..4_u32 {
                let name = if id == 0 {
                    "rgb:kbd_backlight".to_string()
                } else {
                    format!("rgb:kbd_backlight_{id}")
                };
                let z = kbd_leds.join(name);
                sysfs.set_file(z.join("max_brightness"), "50");
                sysfs.set_file(z.join("brightness"), "32");
                sysfs.set_file(z.join("multi_intensity"), "180 0 220");
            }
        },
        |p| p,
    );

    let mut prof = default_quiet_profile();
    prof.id = "uniwill-ac".into();
    prof.charging.charge_type = Some(ucc_core::ChargeType::Standard);
    prof.charging.charging_profile = Some("high_capacity".into());
    assert!(
        backend.apply_profile(&serde_json::to_string(&prof).unwrap()),
        "legacy profile with chargeType=Standard must succeed when BAT0 has no charge_type sysfs node"
    );
    assert_eq!(
        sysfs.get_file(&prof_cur_path).as_deref(),
        Some("high_capacity")
    );
}

#[test]
fn custom_fan_profile_accepts_float_pump_temperatures_from_real_settings() {
    let root = tempfile::tempdir().unwrap();
    let settings_path = root.path().join("settings.json");
    let custom_fan_id = "frixit-tcc-l287pgdbmxpmks8k9ai";
    let settings_json = serde_json::json!({
        "stateMap": { "power_ac": "frixit-ac" },
        "profiles": {
            "frixit-ac": default_high_performance_profile()
        },
        "customFanProfiles": {
            custom_fan_id: {
                "id": custom_fan_id,
                "name": "Custom Pump Curve",
                "json": {
                    "tableCPU": [{ "temp": 0, "speed": 10 }, { "temp": 100, "speed": 100 }],
                    "tableGPU": [{ "temp": 0, "speed": 10 }, { "temp": 100, "speed": 100 }],
                    "tableWaterCoolerFan": [{ "temp": 20, "speed": 7 }, { "temp": 100, "speed": 100 }],
                    "tablePump": [
                        { "temp": 20, "speed": 1 },
                        { "temp": 65.01210653753026, "speed": 2 },
                        { "temp": 79.93088739546089, "speed": 3 }
                    ]
                }
            }
        }
    });
    std::fs::write(&settings_path, settings_json.to_string()).unwrap();

    let (backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let raw = backend.get_fan_profile(custom_fan_id);
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["id"], custom_fan_id);
    assert_eq!(parsed["tablePump"][1]["temp"], 65);
    assert_eq!(parsed["tablePump"][2]["temp"], 80);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_activation_applies_nvidia_ctgp_and_clears_pending_for_keyboard_rpc() {
    use ucc_dbus::UccdBackend;

    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let prof_avail_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profiles_available");
    let prof_cur_path = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile/charging_profile");
    let ctgp_path = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");

    let mut prof = default_quiet_profile();
    prof.id = "ctgp-ac".into();
    prof.name = "cTGP AC".into();
    prof.odm_profile_name = None;
    prof.odm_power_limits.clear();
    prof.nvidia_ctgp_offset = Some(95);

    let mut settings = create_default_settings();
    settings.profiles.insert(prof.id.clone(), prof.clone());
    settings.set_active_profile_for_mode("power_ac", prof.id.clone());
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, sysfs, lighting) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&prof_avail_path, "high_capacity balanced stationary");
            sysfs.set_file(&prof_cur_path, "[balanced] high_capacity stationary");
            sysfs.set_file(&ctgp_path, "0");
        },
        |_| {
            let caps = DeviceCapabilities {
                vendor: ucc_hardware::DeviceVendor::Uniwill,
                ctgp_supported: true,
                ..Default::default()
            };
            Arc::new(MockPlatformBackend::new(PlatformCapabilities {
                backend: PlatformBackendKind::TuxedoIo,
                device: caps,
                tdp_channels: 0,
                perf_profiles_available: 0,
            }))
        },
    );
    lighting.set_keyboard_caps(Some(ucc_lighting::KeyboardCapabilities {
        zones: 2,
        max_brightness: 50,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1],
    }));

    let runtime = start_runtime(
        Arc::clone(&backend),
        "org.test.uccd.startup_ctgp_and_kbd",
        false,
    )
    .await;

    let active_json: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active_json["id"], "ctgp-ac");
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("95"));
    assert!(backend.set_keyboard_backlight_states_json(
        r#"[{"brightness":31,"red":12,"green":34,"blue":56},{"brightness":31,"red":12,"green":34,"blue":56}]"#
    ));

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[test]
fn performance_governor_epp_ebusy_does_not_trigger_repeated_reapply() {
    struct EbusyEppSysfs {
        inner: Arc<MockSysfs>,
    }

    impl ucc_hardware::Sysfs for EbusyEppSysfs {
        fn read(&self, path: &Path) -> Result<String, std::io::Error> {
            self.inner.read(path)
        }

        fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
            if path.file_name().and_then(|n| n.to_str()) == Some("energy_performance_preference") {
                return Err(std::io::Error::from_raw_os_error(nix::libc::EBUSY));
            }
            self.inner.write(path, content)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn read_dir(&self, path: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
            self.inner.read_dir(path)
        }
    }

    let root = tempfile::tempdir().unwrap();
    let cpu0_fq = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    let inner = Arc::new(MockSysfs::new());
    inner.set_file(root.path().join("sys/devices/system/cpu/present"), "0");
    inner.set_file(cpu0_fq.join("scaling_governor"), "performance");
    inner.set_file(
        cpu0_fq.join("scaling_available_governors"),
        "performance powersave",
    );
    inner.set_file(cpu0_fq.join("energy_performance_preference"), "default");
    inner.set_file(
        cpu0_fq.join("energy_performance_available_preferences"),
        "default performance balance_performance power",
    );
    inner.set_file(cpu0_fq.join("cpuinfo_min_freq"), "800000");
    inner.set_file(cpu0_fq.join("cpuinfo_max_freq"), "5400000");
    inner.set_file(cpu0_fq.join("scaling_min_freq"), "800000");
    inner.set_file(cpu0_fq.join("scaling_max_freq"), "5400000");
    inner.set_file(root.path().join("sys/class/power_supply/AC/type"), "Mains");
    inner.set_file(root.path().join("sys/class/power_supply/AC/online"), "1");

    let mut perf_prof = default_high_performance_profile();
    perf_prof.id = "perf-ac".into();
    perf_prof.odm_profile_name = None;
    perf_prof.odm_power_limits.clear();
    perf_prof.cpu.governor = "performance".into();
    perf_prof.cpu.energy_performance_preference = "performance".into();

    let mut settings = create_default_settings();
    settings.profiles.insert(perf_prof.id.clone(), perf_prof);
    settings.set_active_profile_for_mode("power_ac", "perf-ac");
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();

    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: ucc_hardware::DeviceVendor::Uniwill,
            number_of_fans: 2,
            ..Default::default()
        },
        tdp_channels: 0,
        perf_profiles_available: 0,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    let lighting = Arc::new(ucc_lighting::MockLightingTransport::new());
    let sysfs: Arc<dyn ucc_hardware::Sysfs> = Arc::new(EbusyEppSysfs {
        inner: Arc::clone(&inner),
    });
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "PerfTest".into(),
        platform,
        sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(root.path().join("settings.json")),
        sys_root: root.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::default(),
        lid_policy: LidPolicyConfig::default(),
    }));

    backend.reconcile_power_state(true);
    let active_json: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active_json["id"], "perf-ac");

    inner.clear_journal();
    backend.reconcile_power_state(false);
    let extra_cpu_writes: Vec<_> = inner
        .journal()
        .into_iter()
        .filter(|w| w.path.to_string_lossy().contains("cpufreq"))
        .collect();
    assert!(
        extra_cpu_writes.is_empty(),
        "EPP EBUSY under performance governor must not keep reapplying CPU sysfs: {extra_cpu_writes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn ta_04_startup_state_and_no_silent_rejection_during_activation() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let health_before: serde_json::Value =
        serde_json::from_str(&backend.get_driver_health_json()).unwrap();
    assert_eq!(health_before["startupState"], "ready");

    let runtime = start_runtime(
        Arc::clone(&backend),
        "org.test.uccd.ta04_startup_state",
        false,
    )
    .await;
    let health_ready: serde_json::Value =
        serde_json::from_str(&backend.get_driver_health_json()).unwrap();
    assert_eq!(health_ready["startupState"], "ready");

    // Simulate active startup window while a caller mutates a setting: must wait & succeed instead of silent false.
    backend.set_activation_pending(true);
    let b_setter = Arc::clone(&backend);
    let setter = tokio::task::spawn_blocking(move || b_setter.set_water_cooler_fan_speed(42));
    tokio::time::sleep(Duration::from_millis(60)).await;
    backend.set_activation_pending(false);
    assert!(
        setter.await.unwrap(),
        "TA-04: mutation arriving during active startup must wait and succeed cleanly"
    );

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn ta_07_wc_connect_without_power_wc_mapping_preserves_ac_profile() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = create_default_settings();
    let mut ac_prof = default_high_performance_profile();
    ac_prof.id = "frixit-ac".to_string();
    ac_prof.name = "max perfomance".to_string();
    ac_prof.odm_profile_name = None;
    ac_prof.odm_power_limits = vec![45, 65, 85];
    cfg.profiles.insert(ac_prof.id.clone(), ac_prof.clone());
    cfg.state_map.insert("power_ac".to_string(), ac_prof.id.clone());
    cfg.state_map.remove("power_wc");
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&cfg)
        .unwrap();

    let (backend, _, _, _) = build_test_backend(root.path());
    backend.activate_hardware_and_prime();

    backend.set_cooler_connected_state(true);
    backend.reconcile_power_state(false);
    let active: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(
        active["id"], "frixit-ac",
        "TA-07: missing power_wc mapping must fall back to physical power_ac profile, not High Performance"
    );
}

#[test]
fn ta_22_startup_activation_does_not_duplicate_charging_writes_or_fan_ticks() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let end_path = bat.join("charge_control_end_threshold");

    let mut cfg = create_default_settings();
    cfg.profiles.values_mut().for_each(|p| {
        p.charging.start_threshold = Some(40);
        p.charging.end_threshold = Some(80);
    });
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&cfg)
        .unwrap();

    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        root.path().join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        root.path().join("sys/class/power_supply/AC/type"),
        "Mains",
    );
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "1");
    sysfs.set_file(bat.join("type"), "Battery");
    sysfs.set_file(bat.join("charge_control_start_threshold"), "0");
    sysfs.set_file(&end_path, "100");

    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            charging_thresholds: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_fan_temp(0, 65);
    platform.set_fan_temp(1, 65);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));

    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "DedupTest".into(),
        platform: platform.clone(),
        sysfs: sysfs.clone(),
        lighting_transport: Arc::new(MockLightingTransport::new()),
        config_manager: ConfigManager::with_custom_path(root.path().join("settings.json")),
        sys_root: root.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::default(),
        lid_policy: LidPolicyConfig::default(),
    }));
    platform.clear_journal();
    sysfs.clear_journal();

    backend.activate_hardware_and_prime();

    let end_writes = sysfs
        .journal()
        .into_iter()
        .filter(|w| w.path == end_path)
        .count();
    let fan0_writes = platform
        .journal()
        .into_iter()
        .filter(|w| matches!(w, PlatformWrite::SetFanSpeed { fan_idx: 0, .. }))
        .count();
    assert_eq!(
        end_writes, 1,
        "TA-22: startup must write charge_control_end_threshold once, not twice"
    );
    assert_eq!(
        fan0_writes, 1,
        "TA-22: startup must run initial fan tick once, not twice"
    );
}
