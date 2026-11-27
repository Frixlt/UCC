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
