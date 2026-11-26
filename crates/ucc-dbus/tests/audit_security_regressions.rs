// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ucc_dbus::{
    register_uccd_service, AllowAllAuthorizer, AuthAction, Authorizer, DefaultUccdBackend,
    DenyAllAuthorizer, PolkitAuthorizer, TrackingAuthorizer, UccdBackend, UccdInterface,
    UccdProxy, DBUS_PATH,
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
    assert!(auth.check_auth(&caller_conn, Some(&unique), AuthAction::Read).await);
    assert!(
        auth.check_auth(&caller_conn, Some(&unique), AuthAction::Control)
            .await
    );

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 2, "expected 2 Polkit checks, got {recorded:?}");

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
    assert_eq!(recorded.len(), 5, "unexpected auth call count: {recorded:?}");
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
