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

struct LifecycleAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    fan_read_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    panic_on_auto: std::sync::atomic::AtomicBool,
    reject_restore: std::sync::atomic::AtomicBool,
    reset_tdps_on_odm_change: std::sync::atomic::AtomicBool,
    odm_changed: std::sync::atomic::AtomicBool,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl LifecycleAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            fan_read_gate: std::sync::Mutex::new(None),
            panic_on_auto: std::sync::atomic::AtomicBool::new(false),
            reject_restore: std::sync::atomic::AtomicBool::new(false),
            reset_tdps_on_odm_change: std::sync::atomic::AtomicBool::new(false),
            odm_changed: std::sync::atomic::AtomicBool::new(false),
            tdp_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for LifecycleAuditPlatform {
    fn kind(&self) -> PlatformBackendKind {
        self.inner.kind()
    }
    fn is_available(&self) -> bool {
        self.inner.is_available()
    }
    fn capabilities(&self) -> PlatformCapabilities {
        self.inner.capabilities()
    }
    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        let gate = self.fan_read_gate.lock().unwrap().take();
        if let Some((started, release)) = gate {
            let _ = started.send(());
            let _ = release.recv_timeout(Duration::from_secs(5));
        }
        self.inner.read_fan_temp(fan_idx)
    }
    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        self.inner.read_fan_speed_percent(fan_idx)
    }
    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        self.inner.set_fan_speed_percent(fan_idx, percent)
    }
    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        assert!(
            !self.panic_on_auto.load(std::sync::atomic::Ordering::SeqCst),
            "mock cleanup panicked before handoff acknowledgement"
        );
        self.inner.set_fans_auto()
    }
    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        self.inner.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(idx)
    }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        if idx == 0 && watts == 41 {
            let gate = self.tdp_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        use std::sync::atomic::Ordering::SeqCst;
        if (idx == 1
            && watts == 70
            && self.reset_tdps_on_odm_change.load(SeqCst)
            && self.odm_changed.load(SeqCst))
            || (idx == 0 && watts == 55 && self.reject_restore.load(SeqCst))
        {
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
    }

    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_perf_profile(value)?;
        if self
            .reset_tdps_on_odm_change
            .load(std::sync::atomic::Ordering::SeqCst)
            && value == 3
        {
            self.odm_changed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            for (idx, watts) in [60, 90, 110].into_iter().enumerate() {
                self.inner.set_tdp_value(idx, watts);
            }
        }
        Ok(())
    }
    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
        self.inner.set_mode_enable(value)
    }
    fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_mode(value)
    }
}

#[test]
fn m8_life_blocked_periodic_fan_reader_does_not_hold_tokio_runtime_open() {
    let root = tempfile::tempdir().unwrap();
    let (ctl, listen) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(LifecycleAuditPlatform::new(p));
            ctl.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let control = listen.recv().unwrap();
    let name = "org.test.uccd.life.periodic_stop";
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let runtime = rt.block_on(start_runtime(backend, name, true));
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *control.fan_read_gate.lock().unwrap() = Some((started, wait));
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    rt.block_on(runtime.shutdown(Duration::from_millis(40)));
    let returned = started.elapsed();
    let gate = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(260));
        let _ = release.send(());
    });
    drop(rt);
    let exited = started.elapsed();
    gate.join().unwrap();
    assert!(
        exited < Duration::from_millis(210),
        "budget=40 ms, shutdown returned={returned:?}, Tokio exited={exited:?}"
    );
    // Give the detached shutdown thread time to complete cleanup before asserting SetFansAuto ran.
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        platform.journal().contains(&PlatformWrite::SetFansAuto),
        "detached shutdown did not restore auto fan control after blocked periodic tick exited"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_life_failed_cleanup_handoff_returns_explicit_error_and_allows_retry_without_spinning() {
    let root = tempfile::tempdir().unwrap();
    let (ctl, listen) = std::sync::mpsc::channel();
    let (old, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(LifecycleAuditPlatform::new(p));
            ctl.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let control = listen.recv().unwrap();
    let name = "org.test.uccd.life.closed_watch";
    let old = start_runtime(old, name, false).await;
    control
        .panic_on_auto
        .store(true, std::sync::atomic::Ordering::SeqCst);
    old.shutdown(Duration::from_millis(200)).await;

    let (next, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let first_attempt = tokio::time::timeout(
        Duration::from_millis(800),
        DaemonRuntime::start_on_connection(
            session_conn().await,
            name,
            next,
            Arc::new(AllowAllAuthorizer),
            false,
        ),
    )
    .await
    .expect("first retry hung/spun instead of failing explicitly on predecessor panic");
    assert!(
        first_attempt.is_err(),
        "successor unexpectedly succeeded despite panicked predecessor cleanup"
    );

    let bus = zbus::fdo::DBusProxy::new(&session_conn().await)
        .await
        .unwrap();
    assert!(
        !bus.name_has_owner(name.try_into().unwrap()).await.unwrap(),
        "failed start_on_connection leaked bus ownership"
    );

    let (retry_backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let retry_rt = tokio::time::timeout(
        Duration::from_millis(800),
        DaemonRuntime::start_on_connection(
            session_conn().await,
            name,
            retry_backend,
            Arc::new(AllowAllAuthorizer),
            false,
        ),
    )
    .await
    .expect("second retry hung on stale shutdown registry entry")
    .expect("second retry should succeed cleanly after failed predecessor record cleared");
    retry_rt.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_life_failing_newer_request_does_not_cancel_older_request() {
    let root = tempfile::tempdir().unwrap();
    let older = handoff_profile_with_tdps("older", [41, 70, 90]);
    let invalid = handoff_profile_with_tdps("invalid", [50, 125, 130]);
    handoff_save_profiles(root.path(), &[&older, &invalid]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = LifecycleAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.life.invalid_newer";
    let runtime = start_runtime(backend, name, false).await;
    let first = tokio::spawn(async move {
        let conn = session_conn().await;
        uccd_proxy(&conn, name)
            .await
            .set_active_profile(&older.id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let second = tokio::time::timeout(
        Duration::from_secs(2),
        proxy.set_active_profile(&invalid.id),
    )
    .await;
    let _ = release.send(());
    assert!(!second.unwrap().unwrap());
    let first_accepted = first.await.unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let saved = ConfigManager::load_from_path(&root.path().join("settings.json"))
        .unwrap()
        .state_map["power_ac"]
        .clone();
    let tdps = (0..3)
        .map(|i| platform.read_tdp(i).unwrap())
        .collect::<Vec<_>>();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        (first_accepted, active, saved, tdps),
        (true, "older".into(), "older".into(), vec![41, 70, 90])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_life_failed_hardware_reconciliation_reports_failure_and_reflects_readback() {
    let root = tempfile::tempdir().unwrap();
    let older = handoff_profile_with_tdps("older", [41, 70, 90]);
    let newer = handoff_profile_with_tdps("newer", [55, 80, 95]);
    handoff_save_profiles(root.path(), &[&older, &newer]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(LifecycleAuditPlatform::new(p));
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            control_tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let control = control_rx.recv().unwrap();
    let name = "org.test.uccd.life.reapply_failure";
    let runtime = start_runtime(backend, name, false).await;
    let first = tokio::spawn(async move {
        let conn = session_conn().await;
        uccd_proxy(&conn, name)
            .await
            .set_active_profile(&older.id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let second =
        tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&newer.id)).await;
    control
        .reject_restore
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = release.send(());
    assert!(second.unwrap().unwrap());
    let older_accepted = first.await.unwrap();
    let active_val: serde_json::Value =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let reported_pl1 = active_val["odmPowerLimits"]["tdpValues"][0]
        .as_i64()
        .unwrap();
    let hw_pl1 = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        !older_accepted && hw_pl1 == 41 && reported_pl1 == 41,
        "older_accepted={older_accepted}, hw_pl1={hw_pl1}, reported_pl1={reported_pl1}"
    );
}

#[tokio::test]
async fn m8_life_failed_second_tdp_pass_after_odm_does_not_confirm_profile() {
    let root = tempfile::tempdir().unwrap();
    let (ctl, listen) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(LifecycleAuditPlatform::new(p));
            ctl.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let control = listen.recv().unwrap();
    let name = "org.test.uccd.life.odm_second_pass";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    platform.set_perf_profile(2).unwrap();
    platform.set_tdp_value(0, 50);
    platform.set_tdp_value(1, 60);
    platform.set_tdp_value(2, 70);
    platform.clear_journal();
    control
        .reset_tdps_on_odm_change
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut profile = handoff_profile_with_tdps("second-pass", [42, 70, 95]);
    profile.odm_profile_name = Some("overboost".into());
    let ok = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let limits = (0..3)
        .map(|i| platform.read_tdp(i).unwrap())
        .collect::<Vec<_>>();
    let journal = platform.journal();
    assert!(
        journal.contains(&PlatformWrite::SetPerfProfile(3)),
        "fixture did not enter ODM phase: {journal:?}"
    );
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        !ok && active != "second-pass" && limits == vec![50, 60, 70],
        "ApplyProfile={ok}; active={active}; limits={limits:?}"
    );
}

#[tokio::test]
async fn m8_life_extreme_nvidia_offset_returns_false_without_panicking_and_keeps_dbus_responsive() {
    let root = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stand-fixtures/nvml.c");
    let compiled = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(root.path().join("fixture_nvml.so"))
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let gpu = root.path().join("sys/bus/pci/devices/0000:01:00.0");
    let nodes = [
        ("vendor", "0x10de"),
        ("class", "0x030000"),
        ("power/runtime_status", "active"),
        ("power/control", "on"),
    ];
    std::fs::create_dir_all(gpu.join("power")).unwrap();
    for (node, value) in nodes {
        std::fs::write(gpu.join(node), value).unwrap();
    }
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            for (node, value) in nodes {
                sysfs.set_file(gpu.join(node), value);
            }
        },
        |p| p,
    );
    let name = "org.test.uccd.life.offset_range";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.get_nvidia_power_ctrl_available().await.unwrap());
    assert!(
        proxy
            .get_nvidia_power_ctrl_default_power_limit()
            .await
            .unwrap()
            > 0
    );
    let max_outcome = tokio::time::timeout(
        Duration::from_millis(500),
        proxy.set_nvidia_power_offset(i32::MAX),
    )
    .await;
    let min_outcome = tokio::time::timeout(
        Duration::from_millis(500),
        proxy.set_nvidia_power_offset(i32::MIN),
    )
    .await;
    let alive = proxy.get_device_name().await;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        matches!(max_outcome, Ok(Ok(false))) && matches!(min_outcome, Ok(Ok(false))),
        "max={max_outcome:?}, min={min_outcome:?}"
    );
    assert_eq!(alive.as_deref(), Ok("Test Laptop"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn ta_03_sigterm_during_startup_activation_runs_hardware_cleanup_and_releases_bus() {
    let root = tempfile::tempdir().unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let slow = default_high_performance_profile();
    let mut cfg = create_default_settings();
    let mut prof = slow.clone();
    prof.odm_profile_name = None;
    prof.odm_power_limits = vec![41, 70, 90];
    cfg.profiles.insert(prof.id.clone(), prof.clone());
    cfg.set_active_profile_for_mode("power_ac", &prof.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&cfg)
        .unwrap();

    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = LifecycleAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );

    let name = "org.test.uccd.life.startup_sigterm";
    let conn = session_conn().await;
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let start_task = tokio::spawn(DaemonRuntime::start_on_connection_cancellable(
        conn.clone(),
        name,
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        true,
        async move {
            let _ = stop_rx.await;
        },
    ));

    // Wait until activate_hardware is actively blocked inside TDP programming.
    handoff_wait_gate(entered).await;
    platform.clear_journal();
    let _ = stop_tx.send(());
    let _ = release.send(());

    let res = tokio::time::timeout(Duration::from_secs(3), start_task)
        .await
        .expect("start_on_connection_cancellable did not abort in time")
        .expect("join failed")
        .expect("startup abort returned D-Bus error");
    assert!(res.is_none(), "expected Ok(None) after startup cancellation");

    let bus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    assert!(
        !bus.name_has_owner(name.try_into().unwrap()).await.unwrap(),
        "bus name must be released after startup cancellation"
    );
    assert!(
        backend.was_hardware_activated(),
        "expected hardware activation flag before abort"
    );
    assert!(
        platform.journal().contains(&PlatformWrite::SetFansAuto),
        "startup cancellation must restore firmware auto fan control"
    );
}
