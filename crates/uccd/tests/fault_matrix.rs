// SPDX-License-Identifier: GPL-3.0-or-later

//! Dedicated 7-case private-bus fault matrix for `uccd` (`M8.7`):
//! 1. Daemon restart & settings/bus re-ownership handoff
//! 2. Corrupt primary config recovery from backup
//! 3. Unavailable/unsupported hardware passive mode (zero writes)
//! 4. D-Bus client disconnect eviction & client reconnection
//! 5. Slow Polkit/authorizer non-starvation across concurrent callers
//! 6. Charging lane queue overload vs isolated Lighting lane (`DEEP-02`)
//! 7. Sleep state handoff + shutdown deadline preemption (`DEEP-01`)

use serde_json::{json, Value};
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use ucc_config::{create_default_settings, ConfigManager};
use ucc_dbus::{AllowAllAuthorizer, AuthAction, Authorizer, UccdBackend, UccdProxy, DBUS_PATH};
use ucc_hardware::{
    default_process_runner, DeviceCapabilities, DeviceVendor, MockPlatformBackend, MockSysfs,
    PlatformBackendKind, PlatformCapabilities, PowerDeviceController, Sysfs, TelemetryConfig,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{
    DaemonRuntime, LidPolicyConfig, LiveUccdBackend, SleepState, SleepStateStore, UccdDependencies,
};

static BUS_SEQ: AtomicUsize = AtomicUsize::new(0);

fn next_bus_name(tag: &str) -> String {
    format!(
        "com.uniwill.uccd.FaultMatrix.{tag}.p{}.n{}",
        std::process::id(),
        BUS_SEQ.fetch_add(1, Ordering::SeqCst)
    )
}

fn build_backend_with_sysfs(
    root: &Path,
    vendor: DeviceVendor,
    sysfs: Arc<dyn Sysfs>,
) -> (Arc<LiveUccdBackend>, Arc<MockPlatformBackend>) {
    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor,
            number_of_fans: if vendor == DeviceVendor::Unsupported {
                0
            } else {
                2
            },
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: vendor != DeviceVendor::Unsupported,
            water_cooler_supported: false,
            ctgp_supported: false,
            hwp_dynamic_boost_supported: false,
            tdp_control: vendor != DeviceVendor::Unsupported,
            ..Default::default()
        },
        tdp_channels: if vendor == DeviceVendor::Unsupported {
            0
        } else {
            2
        },
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_fan_temp(0, 50);
    platform.set_fan_speed(0, 40);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 110));

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
        zones: 2,
        max_brightness: 255,
    }));

    let card = root.join("sys/class/drm/card0");
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Fault Matrix Test Laptop".into(),
        platform: platform.clone(),
        sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(root.join("settings.json")),
        sys_root: root.to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.to_path_buf(),
            intel_card_path: Some(card),
            nvml: None,
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            root.join("sys"),
            root.join("proc"),
            default_process_runner(),
            root.join("bin"),
        ),
        lid_policy: LidPolicyConfig::default(),
    }));
    (backend, platform)
}

fn seed_basic_sysfs(root: &Path) -> Arc<MockSysfs> {
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        root.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "2800000",
    );
    sysfs.set_file(root.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(root.join("sys/class/power_supply/AC/online"), "1");
    let bat = root.join("sys/class/power_supply/BAT0");
    sysfs.set_file(bat.join("type"), "Battery");
    sysfs.set_file(bat.join("charge_control_start_threshold"), "40");
    sysfs.set_file(bat.join("charge_control_end_threshold"), "80");
    sysfs
}

#[tokio::test]
async fn m8_7_case_1_restart_reacquires_bus_and_reloads_persisted_state() {
    let root = tempfile::tempdir().unwrap();
    let sysfs = seed_basic_sysfs(root.path());
    let bus_name = next_bus_name("restart");
    let conn = zbus::Connection::session().await.unwrap();

    let (b1, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, sysfs.clone());
    let rt1 = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        b1,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();
    let p1 = UccdProxy::builder(&conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert!(p1.set_charge_end_threshold(85).await.unwrap());
    rt1.shutdown(Duration::from_secs(1)).await;

    let (b2, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, sysfs);
    let rt2 = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        b2,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();
    let p2 = UccdProxy::builder(&conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let settings: Value = serde_json::from_str(&p2.get_settings_json().await.unwrap()).unwrap();
    assert_eq!(settings["chargeEndThreshold"], json!(85));
    rt2.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn m8_7_case_2_corrupt_config_recovers_from_backup() {
    let root = tempfile::tempdir().unwrap();
    let sysfs = seed_basic_sysfs(root.path());

    // Save twice so primary has 88 and .bak holds 77.
    let cfg = ConfigManager::with_custom_path(root.path().join("settings.json"));
    let mut s = create_default_settings();
    s.charge_end_threshold = Some(77);
    cfg.save(&s).unwrap();
    s.charge_end_threshold = Some(88);
    cfg.save(&s).unwrap();

    // Corrupt primary settings.json; .bak still holds 77.
    std::fs::write(root.path().join("settings.json"), "{ invalid json !!!").unwrap();

    let (backend, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, sysfs);
    let loaded: Value = serde_json::from_str(&backend.get_settings_json()).unwrap();
    assert_eq!(loaded["chargeEndThreshold"], json!(77));
}

#[tokio::test]
async fn m8_7_case_3_unavailable_device_runs_passive_with_zero_writes() {
    let root = tempfile::tempdir().unwrap();
    let sysfs = seed_basic_sysfs(root.path());
    let (backend, platform) =
        build_backend_with_sysfs(root.path(), DeviceVendor::Unsupported, sysfs.clone());
    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("unsupported");
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let proxy = UccdProxy::builder(&conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert!(!proxy.is_device_supported().await.unwrap());
    assert!(proxy.odm_profiles_available().await.unwrap().is_empty());
    assert_eq!(proxy.odm_power_limits_json().await.unwrap(), "[]");
    assert!(!proxy.set_webcam(true).await.unwrap());
    assert!(!proxy.set_temp_profile("missing-profile").await.unwrap());
    assert!(platform.journal().is_empty());
    assert!(sysfs.journal().is_empty());
    assert_eq!(
        sysfs
            .get_file(
                root.path()
                    .join("sys/class/power_supply/BAT0/charge_control_end_threshold")
            )
            .as_deref(),
        Some("80")
    );
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn m8_7_case_4_dbus_client_disconnect_evicts_power_request_and_allows_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let sysfs = seed_basic_sysfs(root.path());
    let (backend, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, sysfs);
    let bus_name = next_bus_name("client_drop");
    let host_conn = zbus::Connection::session().await.unwrap();
    let runtime = DaemonRuntime::start_on_connection(
        host_conn,
        bus_name.clone(),
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let ephemeral_conn = zbus::Connection::session().await.unwrap();
    let eph_proxy = UccdProxy::builder(&ephemeral_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert!(eph_proxy
        .register_power_request("transient-game", "fault test", "ac")
        .await
        .unwrap());
    let registered_policy: Value =
        serde_json::from_str(&eph_proxy.get_power_policy_json().await.unwrap()).unwrap();
    assert!(registered_policy["registered_apps"]
        .as_array()
        .is_some_and(|a| a.iter().any(|r| r["name"] == "transient-game")));
    drop(eph_proxy);
    drop(ephemeral_conn);

    // Reconnect from a fresh client and verify the disconnected client's request is evicted.
    let fresh_conn = zbus::Connection::session().await.unwrap();
    let fresh_proxy = UccdProxy::builder(&fresh_conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut evicted = false;
    while Instant::now() < deadline {
        let policy: Value =
            serde_json::from_str(&fresh_proxy.get_power_policy_json().await.unwrap()).unwrap();
        if policy["registered_apps"]
            .as_array()
            .is_some_and(|a| a.is_empty())
        {
            evicted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(evicted, "disconnected client power request was not evicted");
    runtime.shutdown(Duration::from_secs(1)).await;
}

struct SlowControlAuthorizer;

impl Authorizer for SlowControlAuthorizer {
    fn check_auth<'a>(
        &'a self,
        _conn: &'a zbus::Connection,
        _sender: Option<&'a str>,
        action: AuthAction,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            if action == AuthAction::Control {
                tokio::time::sleep(Duration::from_millis(350)).await;
            }
            true
        })
    }
}

#[tokio::test]
async fn m8_7_case_5_slow_authorization_does_not_block_concurrent_read_rpcs() {
    let root = tempfile::tempdir().unwrap();
    let sysfs = seed_basic_sysfs(root.path());
    let (backend, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, sysfs);
    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("slow_auth");
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        backend,
        Arc::new(SlowControlAuthorizer),
        false,
    )
    .await
    .unwrap();

    let ctrl_proxy = UccdProxy::builder(&conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let read_proxy = UccdProxy::builder(&conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let slow_task = tokio::spawn(async move { ctrl_proxy.set_charge_end_threshold(75).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let t0 = Instant::now();
    assert!(!read_proxy.get_device_name().await.unwrap().is_empty());
    assert!(
        t0.elapsed() < Duration::from_millis(150),
        "read RPC blocked behind slow control auth ({:?})",
        t0.elapsed()
    );
    assert!(slow_task.await.unwrap().unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
}

struct BlockingChargeSysfs {
    inner: Arc<MockSysfs>,
    armed: Arc<AtomicBool>,
    entered: Arc<Mutex<usize>>,
    signal: Arc<(Mutex<bool>, Condvar)>,
}

impl Sysfs for BlockingChargeSysfs {
    fn read(&self, path: &Path) -> Result<String, std::io::Error> {
        if self.armed.load(Ordering::SeqCst) && path.ends_with("charge_control_start_threshold") {
            {
                let mut g = self.entered.lock().unwrap();
                *g += 1;
                self.signal.1.notify_all();
            }
            let mut open = self.signal.0.lock().unwrap();
            while !*open {
                let (g, timeout) = self
                    .signal
                    .1
                    .wait_timeout(open, Duration::from_secs(4))
                    .unwrap();
                open = g;
                if timeout.timed_out() {
                    break;
                }
            }
        }
        self.inner.read(path)
    }
    fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
        self.inner.write(path, content)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
        self.inner.read_dir(path)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m8_7_case_6_queue_overload_rejects_excess_without_starving_independent_lanes() {
    let root = tempfile::tempdir().unwrap();
    let raw_sysfs = seed_basic_sysfs(root.path());
    let armed = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Mutex::new(0_usize));
    let signal = Arc::new((Mutex::new(false), Condvar::new()));
    let blocking: Arc<dyn Sysfs> = Arc::new(BlockingChargeSysfs {
        inner: Arc::clone(&raw_sysfs),
        armed: Arc::clone(&armed),
        entered: Arc::clone(&entered),
        signal: Arc::clone(&signal),
    });
    let (backend, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, blocking);
    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("overload");
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();
    armed.store(true, Ordering::SeqCst);

    let mut sat = Vec::new();
    for _ in 0..4 {
        let p = UccdProxy::builder(&conn)
            .destination(bus_name.clone())
            .unwrap()
            .path(DBUS_PATH)
            .unwrap()
            .build()
            .await
            .unwrap();
        sat.push(tokio::spawn(
            async move { p.set_charge_end_threshold(83).await },
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while *entered.lock().unwrap() < 4 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*entered.lock().unwrap(), 4);

    let light_proxy = UccdProxy::builder(&conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let t0 = Instant::now();
    assert!(light_proxy
        .set_lightbar_settings_json(&json!({ "managed": false, "mode": "solid" }).to_string(),)
        .await
        .unwrap());
    assert!(t0.elapsed() < Duration::from_millis(250));

    let overflow_proxy = UccdProxy::builder(&conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    assert!(!overflow_proxy.set_charge_end_threshold(92).await.unwrap());

    *signal.0.lock().unwrap() = true;
    signal.1.notify_all();
    for h in sat {
        let _ = h.await;
    }
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m8_7_case_7_sleep_handoff_and_shutdown_deadline_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let sysfs: Arc<dyn Sysfs> = seed_basic_sysfs(root.path());
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let sleep_path = root.path().join("sleep-state.json");
    let uid = unsafe { nix::libc::geteuid() };
    let store = SleepStateStore::new(&sleep_path, uid);
    store
        .save(&SleepState {
            was_active: true,
            manual_mode: "ac".to_string(),
            cooler_enabled: true,
        })
        .unwrap();

    let (backend, _) = build_backend_with_sysfs(root.path(), DeviceVendor::Uniwill, sysfs);
    assert!(backend.load_sleep_state_from(&store));

    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("shutdown");
    let runtime = DaemonRuntime::start_on_connection(
        conn,
        bus_name,
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    // Shutdown with bounded deadline completes promptly and blocks late hardware writes.
    let t0 = Instant::now();
    runtime.shutdown(Duration::from_millis(120)).await;
    assert!(t0.elapsed() < Duration::from_millis(600));
    assert!(!backend.set_charge_end_threshold(71));
}
