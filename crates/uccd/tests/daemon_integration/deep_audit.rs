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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_01_charging_rpc_blocked_on_read_never_writes_after_shutdown_deadline() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    *ctrl.read_gate.lock().unwrap() = Some((
        "charge_control_start_threshold".to_string(),
        Arc::clone(&armed),
        std::sync::Mutex::new(Some(started_tx)),
        std::sync::Mutex::new(Some(release_rx)),
    ));

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

    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = format!(
        "com.uniwill.uccd.Deep01.p{}.n{}",
        std::process::id(),
        DEEP_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let proxy = UccdProxy::builder(&conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let call_task = tokio::spawn(async move { proxy.set_charge_end_threshold(85).await });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    runtime.shutdown(Duration::from_millis(70)).await;
    let _ = release_tx.send(());
    armed.store(false, std::sync::atomic::Ordering::SeqCst);

    let rpc_res = call_task.await.unwrap();
    assert_ne!(rpc_res.ok(), Some(true));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));
    let settings_json: serde_json::Value =
        serde_json::from_str(&backend.get_settings_json()).unwrap();
    assert_ne!(settings_json.get("chargeEndThreshold"), Some(&json!(85)));
}

#[tokio::test]
async fn deep_02_saturated_charging_lane_does_not_block_lighting_or_replay_timed_out_requests() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");

    let entered = Arc::new(std::sync::Mutex::new(0_usize));
    let signal = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));

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

    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = format!(
        "com.uniwill.uccd.Deep02.p{}.n{}",
        std::process::id(),
        DEEP_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    // Arm read gate AFTER startup so priming/activation reads do not occupy permits.
    *ctrl.multi_read_gate.lock().unwrap() = Some((
        "charge_control_start_threshold".to_string(),
        Arc::clone(&entered),
        Arc::clone(&signal),
    ));

    // Saturate Charging lane (4 permits).
    let mut stuck = Vec::new();
    for _ in 0..4 {
        let p = UccdProxy::builder(&conn)
            .destination(bus_name.clone())
            .unwrap()
            .path(DBUS_PATH)
            .unwrap()
            .build()
            .await
            .unwrap();
        stuck.push(tokio::spawn(
            async move { p.set_charge_end_threshold(82).await },
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while *entered.lock().unwrap() < 4 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*entered.lock().unwrap(), 4);

    // Independent Lighting setter MUST succeed promptly while all Charging permits are saturated.
    let light_proxy = UccdProxy::builder(&conn)
        .destination(bus_name.as_str())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let t0 = Instant::now();
    let kbd_ok = tokio::time::timeout(
        Duration::from_millis(350),
        light_proxy.set_keyboard_backlight_states_json(
            r#"[{"brightness":30,"red":12,"green":34,"blue":56}]"#,
        ),
    )
    .await
    .expect("Lighting lane must not stall behind saturated Charging lane")
    .unwrap();
    assert!(kbd_ok);
    assert!(t0.elapsed() < Duration::from_millis(350));

    // A queued 5th Charging request must reject at queue timeout instead of replaying after release.
    let extra_proxy = UccdProxy::builder(&conn)
        .destination(bus_name.as_str())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let queued_res = extra_proxy.set_charge_end_threshold(91).await.unwrap();
    assert!(
        !queued_res,
        "timed-out queued request must fail without late replay"
    );

    *signal.0.lock().unwrap() = true;
    signal.1.notify_all();
    for h in stuck {
        let _ = h.await;
    }
    assert_ne!(sysfs.get_file(&end_path).as_deref(), Some("91"));
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_03_stalled_cpu_freq_priming_publishes_dbus_in_degraded_mode_without_duplicate_polls()
{
    let root = tempfile::tempdir().unwrap();
    let ctrl = Arc::new(SysfsGateControl::default());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    *ctrl.read_gate.lock().unwrap() = Some((
        "scaling_cur_freq".to_string(),
        Arc::clone(&armed),
        std::sync::Mutex::new(Some(started_tx)),
        std::sync::Mutex::new(Some(release_rx)),
    ));

    let (backend, _, _, _) =
        build_backend_with_sysfs_gate(root.path(), Arc::clone(&ctrl), |_| {}, |p| p);

    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = format!(
        "com.uniwill.uccd.Deep03.p{}.n{}",
        std::process::id(),
        DEEP_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let backend_for_start = Arc::clone(&backend);
    let conn_for_start = conn.clone();
    let bus_for_start = bus_name.clone();
    let start_task = tokio::spawn(async move {
        DaemonRuntime::start_on_connection(
            conn_for_start,
            bus_for_start,
            backend_for_start,
            Arc::new(AllowAllAuthorizer),
            false,
        )
        .await
    });

    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // startup_prime_timeout() defaults to 1500ms: service must come up in degraded mode even while scaling_cur_freq is blocked.
    let runtime = tokio::time::timeout(Duration::from_millis(2500), start_task)
        .await
        .expect("start_on_connection must publish D-Bus despite hung scaling_cur_freq")
        .unwrap()
        .unwrap();

    assert!(backend.is_prime_degraded());
    let t0 = Instant::now();
    let _snap = backend.poll_telemetry_once();
    assert!(
        t0.elapsed() < Duration::from_millis(100),
        "single-flight poll guard must return cached telemetry immediately instead of queueing behind hung reader"
    );

    let proxy = UccdProxy::builder(&conn)
        .destination(bus_name.as_str())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert!(!proxy.get_device_name().await.unwrap().is_empty());

    armed.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = release_tx.send(());
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_05_concurrent_charging_setters_and_apply_profile_stay_consistent_between_sysfs_and_settings(
) {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *ctrl.gate.lock().unwrap() = Some((
        "charge_control_end_threshold".to_string(),
        "70".to_string(),
        started_tx,
        release_rx,
    ));

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

    let b1 = Arc::clone(&backend);
    let h1 = std::thread::spawn(move || b1.set_charge_end_threshold(70));
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let b2 = Arc::clone(&backend);
    let h2 = std::thread::spawn(move || b2.set_charge_end_threshold(85));
    // Winning 85 finishes without waiting behind gated 70.
    assert!(h2.join().unwrap());
    let _ = release_tx.send(());

    // Superseded 70 reconciles hardware back to 85 and rejects itself (`DEEP-05`).
    assert!(!h1.join().unwrap());
    let settings_val = serde_json::from_str::<serde_json::Value>(&backend.get_settings_json())
        .unwrap()["chargeEndThreshold"]
        .as_i64()
        .unwrap();
    let hw_val = sysfs
        .get_file(&end_path)
        .unwrap()
        .trim()
        .parse::<i64>()
        .unwrap();
    assert_eq!(settings_val, 85);
    assert_eq!(hw_val, 85);

    // Gated direct(70) superseded by ApplyProfile(90) never leaves stale 70 in hardware (`DEEP-05`).
    let (s2_tx, s2_rx) = std::sync::mpsc::channel();
    let (r2_tx, r2_rx) = std::sync::mpsc::channel();
    *ctrl.gate.lock().unwrap() = Some((
        "charge_control_end_threshold".to_string(),
        "70".to_string(),
        s2_tx,
        r2_rx,
    ));
    let b_slow = Arc::clone(&backend);
    let slow_direct = std::thread::spawn(move || b_slow.set_charge_end_threshold(70));
    s2_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let mut prof = default_quiet_profile();
    prof.id = "charge-concurrent".into();
    prof.charging.end_threshold = Some(90);
    let p_json = serde_json::to_string(&prof).unwrap();
    assert!(backend.apply_profile(&p_json));
    let _ = r2_tx.send(());
    assert!(!slow_direct.join().unwrap());
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("90"));

    // Concurrent ungated ApplyProfile + direct setter also leave sysfs & active state consistent.
    let b3 = Arc::clone(&backend);
    let b4 = Arc::clone(&backend);
    let t3 = std::thread::spawn(move || b3.apply_profile(&p_json));
    let t4 = std::thread::spawn(move || b4.set_charge_end_threshold(88));
    let r3 = t3.join().unwrap();
    let r4 = t4.join().unwrap();
    assert!(r3 || r4);
    let final_hw = sysfs
        .get_file(&end_path)
        .unwrap()
        .trim()
        .parse::<i64>()
        .unwrap();
    assert!(final_hw == 88 || final_hw == 90);
}

#[test]
fn deep_06_all_five_direct_charging_setters_reject_noop_writes_and_verify_bracketed_readback() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");
    let ct_path = bat.join("charge_type");
    let prof_avail = bat.join("charging_profile_available");
    let prof_cur = bat.join("charging_profile");
    let prio_avail = bat.join("charging_priority_available");
    let prio_cur = bat.join("charging_priority");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(bat.join("type"), "Battery");
            sysfs.set_file(&start_path, "40");
            sysfs.set_file(&end_path, "80");
            sysfs.set_file(&ct_path, "[Standard] Fast");
            sysfs.set_file(&prof_avail, "balanced stationary");
            sysfs.set_file(&prof_cur, "[balanced] stationary");
            sysfs.set_file(&prio_avail, "performance battery");
            sysfs.set_file(&prio_cur, "[performance] battery");
        },
        |p| p,
    );

    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charge_control_start_threshold".into());
    assert!(!backend.set_charge_start_threshold(50));
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charge_control_end_threshold".into());
    assert!(!backend.set_charge_end_threshold(85));
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charge_type".into());
    assert!(!backend.set_charge_type("Fast"));
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charging_profile".into());
    assert!(!backend.set_charging_profile("stationary"));
    *ctrl.silent_noop_suffix.lock().unwrap() = Some("charging_priority".into());
    assert!(!backend.set_charging_priority("battery"));
    *ctrl.silent_noop_suffix.lock().unwrap() = None;

    let settings: serde_json::Value = serde_json::from_str(&backend.get_settings_json()).unwrap();
    assert_ne!(settings.get("chargeStartThreshold"), Some(&json!(50)));
    assert_ne!(settings.get("chargeEndThreshold"), Some(&json!(85)));
    assert_ne!(settings.get("chargeType"), Some(&json!("Fast")));
    assert_ne!(settings.get("chargingProfile"), Some(&json!("stationary")));
    assert_ne!(settings.get("chargingPriority"), Some(&json!("battery")));
    assert_eq!(sysfs.get_file(&ct_path).as_deref(), Some("[Standard] Fast"));

    // Selecting already-active bracketed token succeeds cleanly.
    assert!(backend.set_charge_type("Standard"));
}

#[test]
fn deep_07_direct_charging_setter_restores_hardware_on_partial_quantized_write_error() {
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

    *ctrl.rewrite_value.lock().unwrap() = Some((
        "charge_control_end_threshold".into(),
        "85".into(),
        "84".into(),
    ));
    assert!(!backend.set_charge_end_threshold(85));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));
    let settings: serde_json::Value = serde_json::from_str(&backend.get_settings_json()).unwrap();
    assert_ne!(settings.get("chargeEndThreshold"), Some(&json!(85)));
}

#[test]
fn deep_08_apply_profile_refuses_charging_write_when_baseline_readback_is_unreadable() {
    let root = tempfile::tempdir().unwrap();
    let bat = root.path().join("sys/class/power_supply/BAT0");
    let start_path = bat.join("charge_control_start_threshold");
    let end_path = bat.join("charge_control_end_threshold");

    let ctrl = Arc::new(SysfsGateControl::default());
    *ctrl.deny_read_suffix.lock().unwrap() = Some("charge_control_end_threshold".to_string());

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

    let mut p = default_quiet_profile();
    p.id = "unreadable-end".into();
    p.charging.start_threshold = Some(50);
    p.charging.end_threshold = Some(85);
    assert!(!backend.apply_profile(&serde_json::to_string(&p).unwrap()));
    assert_eq!(sysfs.get_file(&start_path).as_deref(), Some("40"));
    assert_eq!(sysfs.get_file(&end_path).as_deref(), Some("80"));
}

#[test]
fn deep_09_ctgp_direct_setter_rejects_noop_sysfs_and_nvml_readback() {
    let root = tempfile::tempdir().unwrap();
    let ctgp_path = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(&ctgp_path, "0");
        },
        |p| p,
    );

    *ctrl.silent_noop_suffix.lock().unwrap() = Some("ctgp_offset".into());
    assert!(!backend.set_nvidia_power_offset(5));
    *ctrl.silent_noop_suffix.lock().unwrap() = None;
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("0"));
    assert_eq!(backend.nvidia_power_offset(), None);

    // Also verify NVML power limit readback mismatch detection end-to-end (`DEEP-09`).
    let nvml_root = tempfile::tempdir().unwrap();
    let c_source = nvml_root.path().join("nvml_noop_limit.c");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stand-fixtures/nvml.c");
    let source = format!(
        "{}\nunsigned nvmlDeviceGetPowerManagementLimit(void *dev, unsigned *limit) {{ if (limit) *limit = 50000; return 0; }}\n",
        std::fs::read_to_string(fixture).unwrap()
    );
    std::fs::write(&c_source, source).unwrap();
    let so_path = nvml_root.path().join("fixture_nvml.so");
    let cc = std::process::Command::new("cc")
        .arg("-shared")
        .arg("-fPIC")
        .arg("-o")
        .arg(&so_path)
        .arg(&c_source)
        .status()
        .unwrap();
    assert!(cc.success());
    let gpu = nvml_root.path().join("sys/bus/pci/devices/0000:01:00.0");
    let nodes = [
        ("vendor", "0x10de"),
        ("class", "0x030000"),
        ("power/runtime_status", "active"),
        ("power/control", "on"),
    ];
    std::fs::create_dir_all(gpu.join("power")).unwrap();
    for (path, data) in nodes {
        std::fs::write(gpu.join(path), data).unwrap();
    }
    let (nvml_backend, _, _, _) = build_custom_backend(
        nvml_root.path(),
        true,
        |sysfs| {
            for (path, data) in nodes {
                sysfs.set_file(gpu.join(path), data);
            }
        },
        |p| p,
    );
    assert!(!nvml_backend.set_nvidia_power_offset(-5));
    assert_eq!(nvml_backend.nvidia_power_offset(), None);
}

#[test]
fn deep_10_and_21_apply_profile_aborts_on_ctgp_write_error_and_unreadable_baseline() {
    let root = tempfile::tempdir().unwrap();
    let ctgp_path = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");

    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, platform, sysfs, _) = build_backend_with_sysfs_gate(
        root.path(),
        Arc::clone(&ctrl),
        |sysfs| {
            sysfs.set_file(&ctgp_path, "0");
        },
        |p| p,
    );

    // First commit a known baseline profile.
    let mut base = default_quiet_profile();
    base.id = "ctgp-base".into();
    base.odm_profile_name = Some("power_save".into());
    base.nvidia_ctgp_offset = Some(0);
    assert!(backend.apply_profile(&serde_json::to_string(&base).unwrap()));
    let before_odm = platform.read_perf_profile().unwrap();

    // Write EIO on ctgp_offset must fail ApplyProfile and roll back earlier phases (`DEEP-10`).
    *ctrl.deny_write_suffix.lock().unwrap() = Some("ctgp_offset".into());
    let mut broken = default_high_performance_profile();
    broken.id = "ctgp-write-errno".into();
    broken.odm_profile_name = Some("enthusiast".into());
    broken.nvidia_ctgp_offset = Some(10);
    assert!(!backend.apply_profile(&serde_json::to_string(&broken).unwrap()));
    *ctrl.deny_write_suffix.lock().unwrap() = None;
    let active: serde_json::Value =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(active["id"], "ctgp-base");
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("0"));
    assert_eq!(platform.read_perf_profile().unwrap(), before_odm);

    // Unreadable cTGP baseline must reject the write before mutating sysfs (`DEEP-21`).
    *ctrl.deny_read_suffix.lock().unwrap() = Some("ctgp_offset".into());
    assert!(!backend.apply_profile(&serde_json::to_string(&broken).unwrap()));
    *ctrl.deny_read_suffix.lock().unwrap() = None;
    assert_eq!(sysfs.get_file(&ctgp_path).as_deref(), Some("0"));
}

#[test]
fn deep_11_apply_profile_rolls_back_keyboard_brightness_and_colors_on_rgb_write_error() {
    use ucc_lighting::{LightingTransport, RgbColor};

    let root = tempfile::tempdir().unwrap();
    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, _, lighting) =
        build_backend_with_sysfs_gate(root.path(), Arc::clone(&ctrl), |_| {}, |p| p);

    assert!(backend.save_custom_keyboard_profile(
        "kb-blue",
        "KB Blue",
        r#"[{"brightness":50,"red":0,"green":0,"blue":255}]"#,
    ));
    assert!(backend.save_custom_keyboard_profile(
        "kb-red",
        "KB Red",
        r#"[{"brightness":255,"red":255,"green":0,"blue":0}]"#,
    ));
    assert!(backend.set_keyboard_backlight_states_json(
        r#"{"keyboardProfileId":"kb-blue","states":[{"brightness":50,"red":0,"green":0,"blue":255}]}"#,
    ));

    lighting.set_fail_on_keyboard_zones(true);
    let mut bad = default_high_performance_profile();
    bad.id = "rgb-write-errno".into();
    bad.keyboard.keyboard_profile_id = "kb-red".into();
    bad.selected_keyboard_profile = Some("kb-red".into());
    assert!(!backend.apply_profile(&serde_json::to_string(&bad).unwrap()));
    lighting.set_fail_on_keyboard_zones(false);

    assert_eq!(lighting.read_keyboard_brightness_state(), Some(50));
    assert_eq!(
        lighting.read_keyboard_colors().unwrap(),
        vec![RgbColor { r: 0, g: 0, b: 255 }]
    );
}

#[test]
fn deep_12_direct_keyboard_rgb_setter_rolls_back_on_settings_save_failure() {
    use ucc_lighting::{LightingTransport, RgbColor};

    let root = tempfile::tempdir().unwrap();
    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, _, lighting) =
        build_backend_with_sysfs_gate(root.path(), Arc::clone(&ctrl), |_| {}, |p| p);

    let blue = RgbColor { r: 0, g: 0, b: 255 };
    assert!(
        backend.set_keyboard_backlight_states_json(
            r#"[{"brightness":50,"red":0,"green":0,"blue":255}]"#,
        )
    );
    assert_eq!(lighting.read_keyboard_colors().unwrap(), vec![blue]);

    let cfg = root.path().join("settings.json");
    std::fs::remove_file(&cfg).unwrap();
    std::fs::create_dir(&cfg).unwrap();

    assert!(!backend.set_keyboard_backlight_states_json(
        r#"[{"brightness":255,"red":255,"green":0,"blue":0}]"#,
    ));
    assert_eq!(lighting.read_keyboard_brightness_state(), Some(50));
    assert_eq!(lighting.read_keyboard_colors().unwrap(), vec![blue]);
}

#[test]
fn deep_13_lightbar_configure_error_returns_false_and_preserves_previous_settings() {
    let root = tempfile::tempdir().unwrap();
    let ctrl = Arc::new(SysfsGateControl::default());
    let (backend, _, _, lighting) =
        build_backend_with_sysfs_gate(root.path(), Arc::clone(&ctrl), |_| {}, |p| p);

    assert!(backend.set_lightbar_settings_json(
        &serde_json::json!({
            "managed": true,
            "welcome_anim": false,
            "mode": "solid",
            "power_settings": {
                "ac": {
                    "enabled": true,
                    "brightness": 40,
                    "color": {"r": 0, "g": 200, "b": 0}
                }
            }
        })
        .to_string(),
    ));
    let prev_settings = backend.get_lightbar_settings_json();

    lighting.set_fail_on_lightbar_write(true);
    assert!(!backend.set_lightbar_settings_json(
        &serde_json::json!({
            "managed": true,
            "welcome_anim": false,
            "mode": "solid",
            "power_settings": {
                "ac": {
                    "enabled": true,
                    "brightness": 100,
                    "color": {"r": 255, "g": 0, "b": 0}
                }
            }
        })
        .to_string(),
    ));
    lighting.set_fail_on_lightbar_write(false);

    let status: serde_json::Value =
        serde_json::from_str(&backend.get_lightbar_status_json()).unwrap();
    assert_eq!(status["ok"], serde_json::json!(false));
    assert!(!status["error"].as_str().unwrap_or("").is_empty());
    assert_eq!(backend.get_lightbar_settings_json(), prev_settings);
}
