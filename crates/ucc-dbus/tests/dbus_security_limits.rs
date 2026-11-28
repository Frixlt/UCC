// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ucc_dbus::{
    register_uccd_service, AllowAllAuthorizer, AuthAction, Authorizer, DefaultUccdBackend,
    DenyAllAuthorizer, PolkitAuthorizer, TrackingAuthorizer, UccdBackend, UccdInterface, UccdProxy,
    DBUS_PATH,
};
use zbus::connection::Builder;
use zbus::zvariant::OwnedValue;

struct BlockingWebcamBackend {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    gate: Arc<tokio::sync::Notify>,
}

impl UccdBackend for BlockingWebcamBackend {
    fn set_webcam(&self, _enabled: bool) -> bool {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.max_in_flight.fetch_max(cur, Ordering::SeqCst);
        // Block synchronously on an OS thread until released by test coordinator
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.gate.notified()).await;
        });
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        true
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t_sec_04_offload_concurrency_is_bounded_under_slow_rpc_flood() {
    let backend = Arc::new(BlockingWebcamBackend {
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
        gate: Arc::new(tokio::sync::Notify::new()),
    });
    let bus_name = format!("org.test.uccd.sec04_{}", std::process::id());
    let server = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&server, backend.clone(), Arc::new(AllowAllAuthorizer))
        .await
        .unwrap();

    let client_conn = zbus::Connection::session().await.unwrap();
    let proxy = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let mut handles = Vec::new();
    for _ in 0..40 {
        let p = proxy.clone();
        handles.push(tokio::spawn(async move { p.set_webcam(true).await }));
    }

    tokio::time::sleep(Duration::from_millis(250)).await;
    let peak = backend.max_in_flight.load(Ordering::SeqCst);
    backend.gate.notify_waiters();
    for h in handles {
        let _ = h.await;
    }

    assert!(
        peak <= 16,
        "offload_* spawned {peak} concurrent blocking OS threads on 40 RPCs; concurrency must be bounded (<= 16)"
    );
}

struct PrivateDbusDaemon {
    child: Child,
    address: String,
}

impl PrivateDbusDaemon {
    fn spawn() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon must be installed to run Polkit D-Bus subject test");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("failed reading dbus-daemon address");
        let address = line.trim().to_string();
        assert!(!address.is_empty(), "empty dbus-daemon address");
        Self { child, address }
    }
}

impl Drop for PrivateDbusDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

type PolkitCallRecord = (String, HashMap<String, String>, String, u32);

struct MockPolkitAuthority {
    calls: Arc<Mutex<Vec<PolkitCallRecord>>>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl MockPolkitAuthority {
    async fn check_authorization(
        &self,
        subject: (String, HashMap<String, OwnedValue>),
        action_id: String,
        _details: HashMap<String, String>,
        flags: u32,
        _cancellation_id: String,
    ) -> (bool, bool, HashMap<String, String>) {
        let mut details_str = HashMap::new();
        for (k, v) in subject.1 {
            details_str.insert(k, format!("{v:?}"));
        }
        self.calls
            .lock()
            .unwrap()
            .push((subject.0, details_str, action_id, flags));
        (true, false, HashMap::new())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_sec_05_polkit_uses_system_bus_name_subject_and_non_interactive_read() {
    let daemon = PrivateDbusDaemon::spawn();
    let calls: Arc<Mutex<Vec<PolkitCallRecord>>> = Arc::new(Mutex::new(Vec::new()));

    let _authority_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("org.freedesktop.PolicyKit1")
        .unwrap()
        .serve_at(
            "/org/freedesktop/PolicyKit1/Authority",
            MockPolkitAuthority {
                calls: calls.clone(),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let caller_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let unique = caller_conn
        .unique_name()
        .expect("unique bus name")
        .as_str()
        .to_string();

    let auth = PolkitAuthorizer::new();
    assert!(
        auth.check_auth(&caller_conn, Some(&unique), AuthAction::Read)
            .await
    );
    assert!(
        auth.check_auth(&caller_conn, Some(&unique), AuthAction::Control)
            .await
    );

    let recorded = calls.lock().unwrap();
    assert_eq!(
        recorded.len(),
        2,
        "expected 2 Polkit checks, got {recorded:?}"
    );

    let (read_kind, read_details, read_action, read_flags) = &recorded[0];
    assert_eq!(read_action, "com.uniwill.uccd.read");
    assert_eq!(
        read_kind, "system-bus-name",
        "Polkit must use race-free system-bus-name subject instead of {read_kind}"
    );
    assert!(
        read_details
            .get("name")
            .map(|v| v.contains(&unique))
            .unwrap_or(false),
        "system-bus-name subject must include unique sender '{unique}', got {read_details:?}"
    );
    assert_eq!(
        *read_flags, 0,
        "AuthAction::Read must NOT pass AllowUserInteraction (0x1); got flags={read_flags:#x}"
    );

    let (ctrl_kind, _, ctrl_action, ctrl_flags) = &recorded[1];
    assert_eq!(ctrl_action, "com.uniwill.uccd.control");
    assert_eq!(ctrl_kind, "system-bus-name");
    assert_eq!(*ctrl_flags, 0x1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_sec_06_read_auth_denial_returns_clean_sentinel_values() {
    let bus_name = format!("org.test.uccd.sec06_{}", std::process::id());
    let server = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &server,
        Arc::new(DefaultUccdBackend),
        Arc::new(DenyAllAuthorizer),
    )
    .await
    .unwrap();

    let client_conn = zbus::Connection::session().await.unwrap();
    let proxy = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    assert_eq!(
        proxy.get_display_brightness().await.unwrap(),
        -1,
        "denied GetDisplayBrightness must return -1 sentinel rather than 0%"
    );
    assert_eq!(
        proxy.get_charging_profiles_available().await.unwrap(),
        "[]",
        "denied JSON array getter must return [] rather than empty string"
    );
    assert_eq!(
        proxy.get_charging_priorities_available().await.unwrap(),
        "[]"
    );
    assert_eq!(
        proxy.get_charge_start_available_thresholds().await.unwrap(),
        "[]"
    );
    assert_eq!(
        proxy.get_charge_end_available_thresholds().await.unwrap(),
        "[]"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_sec_07_auth_action_matrix_for_privileged_and_wc_methods() {
    let bus_name = format!("org.test.uccd.sec07_{}", std::process::id());
    let tracker = Arc::new(TrackingAuthorizer::new(true));
    let server = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&server, Arc::new(DefaultUccdBackend), tracker.clone())
        .await
        .unwrap();

    let client_conn = zbus::Connection::session().await.unwrap();
    let proxy = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    // Test methods aligned with C++ / test_control_authorization.py vs privileged TDP/fan bypass:
    let _ = proxy.set_water_cooler_pump_voltage(2).await.unwrap();
    let _ = proxy.turn_off_water_cooler_fan().await.unwrap();
    let _ = proxy.turn_off_water_cooler_pump().await.unwrap();
    let _ = proxy.set_nvidia_power_offset(10).await.unwrap();

    let mut priv_profile = ucc_core::UccProfile::new("custom_tdp", "Custom TDP");
    priv_profile.odm_power_limits = vec![45, 70, 90];
    priv_profile.fan.use_control = false;
    let _ = proxy
        .apply_profile(&serde_json::to_string(&priv_profile).unwrap())
        .await
        .unwrap();

    let recorded = tracker.recorded_actions();
    assert_eq!(
        recorded.len(),
        5,
        "unexpected auth call count: {recorded:?}"
    );
    assert_eq!(
        recorded[0].1,
        AuthAction::Control,
        "SetWaterCoolerPumpVoltage should use AuthAction::Control"
    );
    assert_eq!(
        recorded[1].1,
        AuthAction::Control,
        "TurnOffWaterCoolerFan should use AuthAction::Control"
    );
    assert_eq!(
        recorded[2].1,
        AuthAction::Control,
        "TurnOffWaterCoolerPump should use AuthAction::Control"
    );
    assert_eq!(
        recorded[3].1,
        AuthAction::Control,
        "SetNVIDIAPowerOffset should use AuthAction::Control"
    );
    assert_eq!(
        recorded[4].1,
        AuthAction::ManageHardware,
        "ApplyProfile with custom TDP or fan.useControl=false must require AuthAction::ManageHardware"
    );
}

#[test]
fn t_sec_08_dbus_policy_conf_allows_legacy_alias_and_restricts_method_call() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let conf_path = manifest.join("../../uccd/com.uniwill.uccd.conf");
    let content = std::fs::read_to_string(&conf_path).expect("read com.uniwill.uccd.conf");

    assert!(
        content.contains("send_interface=\"de.uniwill.uccd\"")
            || content.contains("send_destination=\"de.uniwill.uccd\""),
        "com.uniwill.uccd.conf must allow legacy de.uniwill.uccd interface per DIFF-09"
    );
    assert!(
        content.contains("send_type=\"method_call\""),
        "client <allow send_destination=... rules must restrict send_type=\"method_call\""
    );
    assert!(
        !content.contains("<policy context=\"default\">\n                <allow receive_sender=\"com.uniwill.uccd\""),
        "default policy should not blindly broadcast signals via unrestricted receive_sender"
    );
}

// Re-export check helper to keep UccdInterface directly constructible
#[allow(dead_code)]
fn _ensure_interface_type_constructible() -> UccdInterface<DefaultUccdBackend> {
    UccdInterface::new(DefaultUccdBackend)
}

struct ReadEnabledBackend;

impl UccdBackend for ReadEnabledBackend {
    fn get_display_brightness(&self) -> i32 {
        77
    }
}

struct MockScopedPolkitAuthority {
    gated_sender: String,
    gate: Arc<tokio::sync::Notify>,
    in_flight_for_gated: Arc<AtomicUsize>,
    max_in_flight_for_gated: Arc<AtomicUsize>,
    innocent_calls: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl MockScopedPolkitAuthority {
    async fn check_authorization(
        &self,
        subject: (String, HashMap<String, OwnedValue>),
        _action_id: String,
        _details: HashMap<String, String>,
        _flags: u32,
        _cancellation_id: String,
    ) -> (bool, bool, HashMap<String, String>) {
        let is_gated = subject
            .1
            .get("name")
            .is_some_and(|v| format!("{v:?}").contains(&self.gated_sender));
        if is_gated {
            let cur = self.in_flight_for_gated.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight_for_gated
                .fetch_max(cur, Ordering::SeqCst);
            let _ = tokio::time::timeout(Duration::from_secs(2), self.gate.notified()).await;
            self.in_flight_for_gated.fetch_sub(1, Ordering::SeqCst);
        } else {
            self.innocent_calls.fetch_add(1, Ordering::SeqCst);
        }
        (true, false, HashMap::new())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t_sec_ipc03_per_sender_read_load_bounded_without_starving_other_clients() {
    let daemon = PrivateDbusDaemon::spawn();
    let abusive_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let abusive_sender = abusive_conn
        .unique_name()
        .expect("abusive client unique name")
        .to_string();
    let innocent_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let gate = Arc::new(tokio::sync::Notify::new());
    let in_flight_for_gated = Arc::new(AtomicUsize::new(0));
    let max_in_flight_for_gated = Arc::new(AtomicUsize::new(0));
    let innocent_calls = Arc::new(AtomicUsize::new(0));

    let _authority_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("org.freedesktop.PolicyKit1")
        .unwrap()
        .serve_at(
            "/org/freedesktop/PolicyKit1/Authority",
            MockScopedPolkitAuthority {
                gated_sender: abusive_sender,
                gate: Arc::clone(&gate),
                in_flight_for_gated: Arc::clone(&in_flight_for_gated),
                max_in_flight_for_gated: Arc::clone(&max_in_flight_for_gated),
                innocent_calls: Arc::clone(&innocent_calls),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let server_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("com.uniwill.uccd")
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &server_conn,
        Arc::new(ReadEnabledBackend),
        Arc::new(PolkitAuthorizer::new()),
    )
    .await
    .unwrap();

    let abusive_proxy = UccdProxy::builder(&abusive_conn)
        .destination("com.uniwill.uccd")
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let innocent_proxy = UccdProxy::builder(&innocent_conn)
        .destination("com.uniwill.uccd")
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let mut handles = Vec::new();
    for _ in 0..16 {
        let p = abusive_proxy.clone();
        handles.push(tokio::spawn(async move {
            p.get_display_brightness().await.unwrap_or(-1) == 77
        }));
    }

    tokio::time::sleep(Duration::from_millis(150)).await;

    let innocent_val = tokio::time::timeout(
        Duration::from_millis(750),
        innocent_proxy.get_display_brightness(),
    )
    .await
    .expect("innocent client read must not starve while abusive client saturates reads")
    .expect("innocent client RPC succeeds");
    assert_eq!(innocent_val, 77, "innocent client read must succeed");
    assert_eq!(innocent_calls.load(Ordering::SeqCst), 1);

    let peak = max_in_flight_for_gated.load(Ordering::SeqCst);
    assert!(
        peak <= 8,
        "per-sender Polkit read admission allowed {peak} concurrent calls (expected <= 8)"
    );

    gate.notify_waiters();
    let mut allowed = 0usize;
    let mut rejected = 0usize;
    for h in handles {
        if h.await.unwrap() {
            allowed += 1;
        } else {
            rejected += 1;
        }
    }
    assert!(
        allowed <= 8,
        "allowed abusive calls={allowed}, expected <= 8"
    );
    assert!(
        rejected >= 8,
        "expected excess abusive calls rejected (>= 8), got rejected={rejected}"
    );

    // SenderAuthGuard must release slots on drop so subsequent calls succeed.
    assert_eq!(
        abusive_proxy.get_display_brightness().await.unwrap(),
        77,
        "per-sender slot must recover after inflight reads complete"
    );
}

struct MockFastPolkitAuthority {
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl MockFastPolkitAuthority {
    async fn check_authorization(
        &self,
        _subject: (String, HashMap<String, OwnedValue>),
        _action_id: String,
        _details: HashMap<String, String>,
        _flags: u32,
        _cancellation_id: String,
    ) -> (bool, bool, HashMap<String, String>) {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(cur, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(2)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        (true, false, HashMap::new())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t_sec_ipc03_fast_gui_reconnect_burst_of_32_reads_succeeds_without_false_denial() {
    let daemon = PrivateDbusDaemon::spawn();
    let gui_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let _authority_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("org.freedesktop.PolicyKit1")
        .unwrap()
        .serve_at(
            "/org/freedesktop/PolicyKit1/Authority",
            MockFastPolkitAuthority {
                in_flight: Arc::clone(&in_flight),
                max_in_flight: Arc::clone(&max_in_flight),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let server_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("com.uniwill.uccd")
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &server_conn,
        Arc::new(ReadEnabledBackend),
        Arc::new(PolkitAuthorizer::new()),
    )
    .await
    .unwrap();

    let proxy = UccdProxy::builder(&gui_conn)
        .destination("com.uniwill.uccd")
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let mut handles = Vec::new();
    for _ in 0..32 {
        let p = proxy.clone();
        handles.push(tokio::spawn(async move {
            p.get_display_brightness().await.unwrap_or(-1)
        }));
    }

    let mut succeeded = 0usize;
    for h in handles {
        if h.await.unwrap() == 77 {
            succeeded += 1;
        }
    }
    assert_eq!(
        succeeded, 32,
        "all 32 rapid reconnect reads must succeed through bounded queuing"
    );
    let peak = max_in_flight.load(Ordering::SeqCst);
    assert!(
        peak <= 8,
        "per-sender queued burst must still cap concurrency at <= 8, got {peak}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_sec_ipc03_oversized_json_payloads_rejected_before_auth_and_backend() {
    let auth = Arc::new(TrackingAuthorizer::new(true));
    let bus_name = format!("org.test.uccd.ipc03_json_{}", std::process::id());
    let server = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&server, Arc::new(DefaultUccdBackend), auth.clone())
        .await
        .unwrap();

    let client_conn = zbus::Connection::session().await.unwrap();
    let proxy = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let oversized = format!("{{\"pad\":\"{}\"}}", "a".repeat(300 * 1024));
    assert!(
        !proxy.apply_profile(&oversized).await.unwrap(),
        "oversized ApplyProfile must return false"
    );
    assert!(
        !proxy.save_custom_profile(&oversized).await.unwrap(),
        "oversized SaveCustomProfile must return false"
    );
    assert!(
        !proxy
            .set_power_device_settings_json(&oversized)
            .await
            .unwrap(),
        "oversized SetPowerDeviceSettingsJSON must return false"
    );
    assert!(
        !proxy.set_fan_profile("custom", &oversized).await.unwrap(),
        "oversized SetFanProfile must return false"
    );
    assert!(
        !proxy.set_batch_state_map(&oversized).await.unwrap(),
        "oversized SetBatchStateMap must return false"
    );

    assert!(
        auth.recorded_actions().is_empty(),
        "oversized JSON payloads must be rejected before invoking authorizer/backend, got: {:?}",
        auth.recorded_actions()
    );
}

struct LaneIsolationBackend {
    charging_in_flight: AtomicUsize,
    charging_gate: Arc<tokio::sync::Notify>,
    display_calls: AtomicUsize,
    lighting_calls: AtomicUsize,
}

impl UccdBackend for LaneIsolationBackend {
    fn set_charge_start_threshold(&self, _value: i32) -> bool {
        self.charging_in_flight.fetch_add(1, Ordering::SeqCst);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.charging_gate.notified())
                .await;
        });
        self.charging_in_flight.fetch_sub(1, Ordering::SeqCst);
        true
    }

    fn set_display_brightness(&self, _brightness: i32) -> bool {
        self.display_calls.fetch_add(1, Ordering::SeqCst);
        true
    }

    fn trigger_lightbar_animation(&self, _force: bool) -> bool {
        self.lighting_calls.fetch_add(1, Ordering::SeqCst);
        true
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offload_lane_isolation_across_independent_lanes() {
    let daemon = PrivateDbusDaemon::spawn();
    let backend = Arc::new(LaneIsolationBackend {
        charging_in_flight: AtomicUsize::new(0),
        charging_gate: Arc::new(tokio::sync::Notify::new()),
        display_calls: AtomicUsize::new(0),
        lighting_calls: AtomicUsize::new(0),
    });

    let server_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("com.uniwill.uccd")
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&server_conn, backend.clone(), Arc::new(AllowAllAuthorizer))
        .await
        .unwrap();

    let client_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let proxy = UccdProxy::builder(&client_conn)
        .destination("com.uniwill.uccd")
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    // Saturate the Charging lane (capacity = 4) with 6 blocked calls.
    let mut charging_handles = Vec::new();
    for _ in 0..6 {
        let p = proxy.clone();
        charging_handles.push(tokio::spawn(async move {
            p.set_charge_start_threshold(40).await
        }));
    }

    // Wait until all 4 Charging lane permits are occupied.
    for _ in 0..100 {
        if backend.charging_in_flight.load(Ordering::SeqCst) == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        backend.charging_in_flight.load(Ordering::SeqCst),
        4,
        "Charging lane must be saturated with 4 in-flight calls"
    );

    // Independent Display and Lighting lanes must complete promptly without blocking.
    let display_ok = tokio::time::timeout(
        Duration::from_millis(400),
        proxy.set_display_brightness(80),
    )
    .await
    .expect("Display lane must not be blocked by saturated Charging lane")
    .expect("SetDisplayBrightness RPC should succeed");
    assert!(display_ok);

    let lighting_ok = tokio::time::timeout(
        Duration::from_millis(400),
        proxy.trigger_lightbar_animation(true),
    )
    .await
    .expect("Lighting lane must not be blocked by saturated Charging lane")
    .expect("TriggerLightbarAnimation RPC should succeed");
    assert!(lighting_ok);

    assert_eq!(backend.display_calls.load(Ordering::SeqCst), 1);
    assert_eq!(backend.lighting_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        backend.charging_in_flight.load(Ordering::SeqCst),
        4,
        "Charging lane should still be blocked while Display and Lighting completed"
    );

    // Release blocked Charging lane workers cleanly.
    backend.charging_gate.notify_waiters();
    for handle in charging_handles {
        let _ = handle.await;
    }
}
