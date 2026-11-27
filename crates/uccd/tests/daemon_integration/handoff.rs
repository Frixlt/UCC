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

struct HandoffAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    auto_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    reject_second_tdp: std::sync::atomic::AtomicBool,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl HandoffAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            auto_gate: std::sync::Mutex::new(None),
            reject_second_tdp: std::sync::atomic::AtomicBool::new(false),
            tdp_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for HandoffAuditPlatform {
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
        self.inner.read_fan_temp(fan_idx)
    }
    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        self.inner.read_fan_speed_percent(fan_idx)
    }
    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        self.inner.set_fan_speed_percent(fan_idx, percent)
    }
    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        let gate = self.auto_gate.lock().unwrap().take();
        if let Some((started, release)) = gate {
            let _ = started.send(());
            let _ = release.recv_timeout(Duration::from_secs(8));
        }
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
        if idx == 1
            && self
                .reject_second_tdp
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_tdp(idx, watts)
    }
    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_perf_profile(value)
    }
    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
        self.inner.set_mode_enable(value)
    }
    fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_mode(value)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_handoff_two_successful_dbus_profile_switches_leave_latest_hardware_and_disk_in_agreement(
) {
    let root = tempfile::tempdir().unwrap();
    let older = handoff_profile_with_tdps("older", [41, 70, 90]);
    let newer = handoff_profile_with_tdps("newer", [55, 80, 95]);
    handoff_save_profiles(root.path(), &[&older, &newer]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = HandoffAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.handoff.profile_race";
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
    let _ = release.send(());
    let first_ok = first.await.unwrap();
    let second_ok = second.unwrap().unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let saved = ConfigManager::load_from_path(&root.path().join("settings.json"))
        .unwrap()
        .state_map["power_ac"]
        .clone();
    let tdp = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(first_ok && second_ok);
    assert_eq!(
        (active, saved, tdp),
        (newer.id.clone(), newer.id, 55),
        "older RPC resumed after newer success and replaced active profile and TDP"
    );
}

#[test]
fn m8_handoff_offloaded_hardware_rpc_does_not_write_subsequent_tdp_or_hold_up_runtime_exit() {
    let root = tempfile::tempdir().unwrap();
    let profile = handoff_profile_with_tdps("pending-profile", [41, 70, 90]);
    handoff_save_profiles(root.path(), &[&profile]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = HandoffAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.handoff.pending_shutdown";
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let runtime = tokio_runtime.block_on(start_runtime(backend.clone(), name, false));
    let _in_flight_call = tokio_runtime.spawn(async move {
        let conn = session_conn().await;
        let _ = uccd_proxy(&conn, name)
            .await
            .set_active_profile(&profile.id)
            .await;
    });
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    tokio_runtime.block_on(runtime.shutdown(Duration::from_millis(40)));
    let after_shutdown = started.elapsed();
    platform.clear_journal();
    let release_thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(270));
        let _ = release.send(());
    });
    drop(tokio_runtime);
    let runtime_exit = started.elapsed();
    release_thread.join().unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let writes = platform.journal();
    assert!(
        runtime_exit < Duration::from_millis(200)
            && !writes
                .iter()
                .any(|w| matches!(w, PlatformWrite::SetTdp { index: 1 | 2, .. }))
            && !backend.get_active_profile_json().contains("pending-profile"),
        "40 ms shutdown returned at {after_shutdown:?}; Tokio exited at {runtime_exit:?}; late writes={writes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_handoff_published_daemon_does_not_acknowledge_a_profile_it_loses_during_activation() {
    let root = tempfile::tempdir().unwrap();
    let name = "org.test.uccd.handoff.mutations";
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (old, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = HandoffAuditPlatform::new(p);
            *wrapper.auto_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let old = start_runtime(old, name, false).await;
    old.shutdown(Duration::from_millis(35)).await;
    handoff_wait_gate(entered).await;
    let (next, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |_| platform.clone());
    let next = tokio::spawn(start_runtime(next, name, false));
    let conn = session_conn().await;
    handoff_wait_name(&conn, name).await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut profile = default_quiet_profile();
    profile.id = "accepted-before-activation".into();
    profile.odm_profile_name = None;
    profile.odm_power_limits.clear();
    let body = serde_json::to_string(&profile).unwrap();
    let accepted_now =
        tokio::time::timeout(Duration::from_millis(350), proxy.apply_profile(&body)).await;
    let _ = release.send(());
    let next = next.await.unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    next.shutdown(Duration::from_secs(1)).await;
    assert!(
        !matches!(accepted_now, Ok(Ok(true))) || active == profile.id,
        "D-Bus acknowledged {accepted_now:?} during handoff, activation replaced it with {active}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_handoff_predecessor_cannot_write_fan_auto_after_successor_handoff_timeout() {
    let root = tempfile::tempdir().unwrap();
    let name = "org.test.uccd.handoff.timeout";
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (old, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = HandoffAuditPlatform::new(p);
            *wrapper.auto_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let old = start_runtime(old, name, false).await;
    old.shutdown(Duration::from_millis(35)).await;
    handoff_wait_gate(entered).await;
    let (next, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |_| platform.clone());
    let next_task = tokio::spawn(start_runtime(next, name, false));
    // Predecessor holds set_fans_auto() beyond the old 5s window? Verify successor still waits
    // (150ms here keeps suite fast; predecessor release runs AFTER we assert successor has not activated).
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!next_task.is_finished());
    let _ = release.send(());
    let next_rt = next_task.await.unwrap();
    platform.clear_journal();
    tokio::time::sleep(Duration::from_millis(60)).await;
    let writes = platform.journal();
    next_rt.shutdown(Duration::from_secs(1)).await;
    assert!(!writes.contains(&PlatformWrite::SetFansAuto));
}

#[tokio::test]
async fn m8_handoff_odm_write_is_not_left_applied_when_tdp_fails_on_a_backend_without_readback() {
    let root = tempfile::tempdir().unwrap();
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(HandoffAuditPlatform::new(p));
            control_tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let control = control_rx.recv().unwrap();
    let name = "org.test.uccd.handoff.odm_readback";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut no_odm = default_quiet_profile();
    no_odm.odm_profile_name = None;
    no_odm.odm_power_limits.clear();
    assert!(proxy
        .apply_profile(&serde_json::to_string(&no_odm).unwrap())
        .await
        .unwrap());
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    control
        .reject_second_tdp
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut profile = handoff_profile_with_tdps("fault-after-odm", [42, 70, 90]);
    profile.odm_profile_name = Some("overboost".into());
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let actual = platform.read_perf_profile().unwrap();
    let journal = platform.journal();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        actual, 2,
        "hardware without read_perf_profile kept a failed plan's ODM: {journal:?}"
    );
}

#[tokio::test]
async fn m8_handoff_nvidia_hardware_default_and_offset_survive_daemon_restart() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("nvml_limit_write");
    let c_source = root.path().join("audit_nvml.c");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stand-fixtures/nvml.c");
    let source = std::fs::read_to_string(fixture).unwrap().replace(
        "unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) { return 0; }",
        &format!(
            "unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) {{ FILE *f = fopen({:?}, \"w\"); if (!f) return 1; fprintf(f, \"%u\", limit); fclose(f); return 0; }}",
            log.as_os_str()
        ),
    );
    let source = source.replace(
        "unsigned nvmlDeviceGetEnforcedPowerLimit(void *dev, unsigned *out) { *out = 50000; return 0; }",
        &format!(
            "unsigned nvmlDeviceGetEnforcedPowerLimit(void *dev, unsigned *out) {{ FILE *f=fopen({:?}, \"r\"); *out=50000; if (f) {{ (void)fscanf(f, \"%u\", out); fclose(f); }} return 0; }}",
            log.as_os_str()
        ),
    );
    assert!(source.contains("fprintf(f") && source.contains("fscanf(f"));
    std::fs::write(&c_source, format!("#include <stdio.h>\n{source}")).unwrap();
    let result = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(root.path().join("fixture_nvml.so"))
        .arg(c_source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let gpu = root.path().join("sys/bus/pci/devices/0000:01:00.0");
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
    let build = || {
        build_custom_backend(
            root.path(),
            true,
            |sysfs| {
                for (path, data) in nodes {
                    sysfs.set_file(gpu.join(path), data);
                }
            },
            |p| p,
        )
        .0
    };
    let name = "org.test.uccd.handoff.ctgp_restart";
    let runtime = start_runtime(build(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert_eq!(
        proxy
            .get_nvidia_power_ctrl_default_power_limit()
            .await
            .unwrap(),
        50
    );
    assert!(proxy.set_nvidia_power_offset(-5).await.unwrap());
    let first = std::fs::read_to_string(&log).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    let runtime = start_runtime(build(), name, false).await;
    let proxy = uccd_proxy(&conn, name).await;
    let default_after_restart = proxy
        .get_nvidia_power_ctrl_default_power_limit()
        .await
        .unwrap();
    assert!(proxy.set_nvidia_power_offset(-5).await.unwrap());
    let second = std::fs::read_to_string(&log).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(first, "45000");
    assert_eq!((default_after_restart, second.as_str()), (50, "45000"));
}

type HandoffAuthGate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
struct HandoffDelayedControl(std::sync::Mutex<Option<HandoffAuthGate>>);

impl ucc_dbus::Authorizer for HandoffDelayedControl {
    fn check_auth<'a>(
        &'a self,
        _conn: &'a zbus::Connection,
        _sender: Option<&'a str>,
        action: ucc_dbus::AuthAction,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        let gate = if action == ucc_dbus::AuthAction::Control {
            self.0.lock().unwrap().take()
        } else {
            None
        };
        Box::pin(async move {
            if let Some((entered, resume)) = gate {
                let _ = entered.send(());
                return resume.await.is_ok();
            }
            true
        })
    }
}

#[tokio::test]
async fn m8_handoff_power_request_cannot_be_installed_after_its_sender_disconnected_during_auth() {
    let root = tempfile::tempdir().unwrap();
    let name = "org.test.uccd.handoff.auth_disconnect";
    let (backend, _, sysfs, _) = build_test_backend(root.path());
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(
        root.path().join("sys/class/power_supply/BAT0/type"),
        "Battery",
    );
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let runtime = DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        backend,
        Arc::new(HandoffDelayedControl(std::sync::Mutex::new(Some((
            started, wait,
        ))))),
        false,
    )
    .await
    .unwrap();

    let observer = session_conn().await;
    let observer_api = uccd_proxy(&observer, name).await;
    assert_eq!(observer_api.get_power_state().await.unwrap(), "power_bat");
    let app = session_conn().await;
    let app_id = app.unique_name().unwrap().to_string();
    let request = tokio::spawn({
        let app = app.clone();
        async move {
            uccd_proxy(&app, name)
                .await
                .register_power_request("pending", "check", "ac")
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), entered)
        .await
        .unwrap()
        .unwrap();
    app.close().await.unwrap();
    let bus = zbus::fdo::DBusProxy::new(&observer).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while bus
            .name_has_owner(app_id.as_str().try_into().unwrap())
            .await
            .unwrap()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = release.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), request).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let decision = observer_api.get_power_policy_json().await.unwrap();
    let state = observer_api.get_power_state().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        state, "power_bat",
        "request from disconnected {app_id} applied: {decision}"
    );
}
