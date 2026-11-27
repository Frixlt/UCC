// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproduction tests for confirmed `ucc-dbus` findings on commit `1d07126`:
//! - R02: `ApplyProfile` with charging fields must require `AuthAction::ManageHardware`
//! - R22: D-Bus aliases (`/com/uniwill/uccd` and `/de/uniwill/uccd`, primary + legacy) must share one `OffloadLanes` budget
//! - N29: Polkit timeout must use a non-empty `cancellation_id` and invoke `CancelCheckAuthorization`

use serde_json::json;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ucc_dbus::{
    register_uccd_service, AllowAllAuthorizer, AuthAction, DefaultUccdBackend, DenyAllAuthorizer,
    TrackingAuthorizer, UccdBackend, UccdLegacyProxy, UccdProxy, DBUS_PATH, DBUS_PATH_ALT,
};
use zbus::connection::Builder;

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
            .expect("dbus-daemon must be installed to run private D-Bus reproduction tests");
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

struct CountingApplyBackend {
    apply_calls: AtomicUsize,
}

impl UccdBackend for CountingApplyBackend {
    fn apply_profile(&self, _profile_json: &str) -> bool {
        self.apply_calls.fetch_add(1, Ordering::SeqCst);
        true
    }
}

/// R02: Direct charging setters (`SetChargeStartThreshold`) require `AuthAction::ManageHardware`,
/// so `ApplyProfile` containing charging fields (`charging` or `chargeStartThreshold`) must also
/// request `AuthAction::ManageHardware` and refuse to invoke the backend when denied.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r02_apply_profile_with_charging_fields_requires_manage_hardware() {
    let daemon = PrivateDbusDaemon::spawn();
    let allow_service = format!("org.test.uccd.r02_allow_{}", std::process::id());
    let deny_service = format!("org.test.uccd.r02_deny_{}", std::process::id());

    let tracker = Arc::new(TrackingAuthorizer::new(true));
    let server_allow = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(allow_service.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&server_allow, Arc::new(DefaultUccdBackend), tracker.clone())
        .await
        .unwrap();

    let deny_backend = Arc::new(CountingApplyBackend {
        apply_calls: AtomicUsize::new(0),
    });
    let server_deny = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(deny_service.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &server_deny,
        deny_backend.clone(),
        Arc::new(DenyAllAuthorizer),
    )
    .await
    .unwrap();

    let client_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let allow_proxy = UccdProxy::builder(&client_conn)
        .destination(allow_service.as_str())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let deny_proxy = UccdProxy::builder(&client_conn)
        .destination(deny_service.as_str())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    // Baseline: direct charging threshold setter requests ManageHardware.
    let _ = allow_proxy.set_charge_start_threshold(60).await.unwrap();

    // Case 1: nested `charging` object inside ApplyProfile payload.
    let nested_charging_profile = json!({
        "id": "r02_nested_charging",
        "name": "R02 Nested Charging",
        "charging": {
            "startThreshold": 60,
            "endThreshold": 80
        }
    })
    .to_string();
    let _ = allow_proxy
        .apply_profile(&nested_charging_profile)
        .await
        .unwrap();

    // Case 2: flat `chargeStartThreshold` field inside ApplyProfile payload.
    let flat_charging_profile = json!({
        "id": "r02_flat_charging",
        "name": "R02 Flat Charging",
        "chargeStartThreshold": 60,
        "chargeEndThreshold": 80
    })
    .to_string();
    let _ = allow_proxy
        .apply_profile(&flat_charging_profile)
        .await
        .unwrap();

    // Denied path must never reach the backend.
    assert!(
        !deny_proxy
            .apply_profile(&nested_charging_profile)
            .await
            .unwrap(),
        "denied ApplyProfile must return false"
    );
    assert_eq!(
        deny_backend.apply_calls.load(Ordering::SeqCst),
        0,
        "denied ApplyProfile must not invoke backend"
    );

    let recorded = tracker.recorded_actions();
    assert_eq!(
        recorded.len(),
        3,
        "expected 3 authorization checks, got {recorded:?}"
    );
    assert_eq!(
        recorded[0].1,
        AuthAction::ManageHardware,
        "SetChargeStartThreshold must require AuthAction::ManageHardware"
    );
    assert_eq!(
        recorded[1].1,
        AuthAction::ManageHardware,
        "R02: ApplyProfile with nested `charging` fields must require AuthAction::ManageHardware, got {:?}",
        recorded[1].1
    );
    assert_eq!(
        recorded[2].1,
        AuthAction::ManageHardware,
        "R02: ApplyProfile with `chargeStartThreshold` must require AuthAction::ManageHardware, got {:?}",
        recorded[2].1
    );
}

struct BlockingWebcamBackend {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    gate: Arc<tokio::sync::Notify>,
}

impl UccdBackend for BlockingWebcamBackend {
    fn set_webcam(&self, _enabled: bool) -> bool {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.max_in_flight.fetch_max(cur, Ordering::SeqCst);
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

/// R22: `register_uccd_service` exports primary and legacy interfaces on both `/com/uniwill/uccd`
/// and `/de/uniwill/uccd`. All 4 alias endpoints must share a single `OffloadLanes` budget
/// (`OffloadLane::Display` = 4 permits) rather than multiplying concurrency 4x.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r22_dbus_aliases_share_single_offload_lanes_budget() {
    let daemon = PrivateDbusDaemon::spawn();
    let backend = Arc::new(BlockingWebcamBackend {
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
        gate: Arc::new(tokio::sync::Notify::new()),
    });
    let bus_name = format!("org.test.uccd.r22_{}", std::process::id());
    let server = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&server, backend.clone(), Arc::new(AllowAllAuthorizer))
        .await
        .unwrap();

    let client_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let primary_main = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let legacy_main = UccdLegacyProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let primary_alt = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH_ALT)
        .unwrap()
        .build()
        .await
        .unwrap();
    let legacy_alt = UccdLegacyProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH_ALT)
        .unwrap()
        .build()
        .await
        .unwrap();

    // Dispatch 4 concurrent `SetWebcam` calls (`OffloadLane::Display`, limit 4) per alias endpoint.
    let mut handles = Vec::with_capacity(16);
    for _ in 0..4 {
        let p1 = primary_main.clone();
        handles.push(tokio::spawn(async move { p1.set_webcam(true).await }));
        let p2 = legacy_main.clone();
        handles.push(tokio::spawn(async move { p2.set_webcam(true).await }));
        let p3 = primary_alt.clone();
        handles.push(tokio::spawn(async move { p3.set_webcam(true).await }));
        let p4 = legacy_alt.clone();
        handles.push(tokio::spawn(async move { p4.set_webcam(true).await }));
    }

    tokio::time::sleep(Duration::from_millis(250)).await;
    let peak = backend.max_in_flight.load(Ordering::SeqCst);
    backend.gate.notify_waiters();
    for h in handles {
        let _ = h.await;
    }

    assert!(
        peak <= 4,
        "R22: aliases across {DBUS_PATH} and {DBUS_PATH_ALT} spawned {peak} concurrent Display-lane ops; expected shared budget <= 4"
    );
}

/// N29: `crates/ucc-dbus/src/auth.rs` must pass a non-empty `cancellation_id` to Polkit
/// `CheckAuthorization` and invoke `CancelCheckAuthorization` when the authorization check times out.
#[test]
fn n29_polkit_timeout_sends_cancel_check_authorization() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let auth_path = manifest_dir.join("src/auth.rs");
    let source = std::fs::read_to_string(&auth_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", auth_path.display()));

    assert!(
        !source.contains("let cancellation_id = \"\";"),
        "N29: auth.rs must generate a non-empty cancellation_id instead of hardcoding empty string"
    );
    assert!(
        source.contains("cancel_check_authorization")
            || source.contains("CancelCheckAuthorization"),
        "N29: auth.rs must define and invoke CancelCheckAuthorization on Polkit timeout"
    );
}
