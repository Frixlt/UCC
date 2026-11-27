// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproduction tests for confirmed `ucc-cli` findings on commit `1d07126`:
//! - R07: `charging set-thresholds` must filter `< 0` baseline sentinels and inspect/report rollback failure
//! - N20: `monitor --json` must output `null` (not `0`) for unavailable GPU metrics
//! - R24: CLI write/Polkit commands must allow >1500ms interactive Polkit authorization window

use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use ucc_dbus::{register_uccd_service, AllowAllAuthorizer, UccdBackend};
use zbus::connection::Builder;
use zbus::zvariant::OwnedValue;

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
            .expect("dbus-daemon must be installed to run CLI reproduction tests");
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

fn unique_service_name(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("org.test.uccd.repro_{tag}_{}_{}", std::process::id(), nanos)
}

fn run_cli_on_bus(address: &str, bus_name: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ucc-cli"))
        .args(args)
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .env("UCCD_DBUS_SERVICE", bus_name)
        .output()
        .expect("failed to spawn ucc-cli binary")
}

struct ChargingRollbackMockBackend {
    prev_end: i32,
    written_ends: Mutex<Vec<i32>>,
    fail_second_end_write: bool,
}

impl UccdBackend for ChargingRollbackMockBackend {
    fn get_charge_start_threshold(&self) -> i32 {
        -1
    }

    fn get_charge_end_threshold(&self) -> i32 {
        self.prev_end
    }

    fn set_charge_end_threshold(&self, value: i32) -> bool {
        let mut w = self.written_ends.lock().unwrap();
        w.push(value);
        if self.fail_second_end_write && w.len() >= 2 {
            return false;
        }
        value >= 0
    }

    fn set_charge_start_threshold(&self, _value: i32) -> bool {
        // Force rollback path after end threshold succeeds.
        false
    }
}

/// R07: `charging.rs` must filter `< 0` sentinel baselines (never attempting to roll back to `-1`)
/// and must check the rollback `Result<bool>` and report when rollback fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r07_charging_set_thresholds_handles_minus_one_and_reports_rollback_failure() {
    let daemon = PrivateDbusDaemon::spawn();

    // Case 1: baseline end threshold is sentinel -1 -> CLI must NOT call SetChargeEndThreshold(-1).
    let minus_one_backend = Arc::new(ChargingRollbackMockBackend {
        prev_end: -1,
        written_ends: Mutex::new(Vec::new()),
        fail_second_end_write: false,
    });
    let svc_sentinel = unique_service_name("r07_sentinel");
    let _conn1 = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(svc_sentinel.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &_conn1,
        Arc::clone(&minus_one_backend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .unwrap();

    let out1 = run_cli_on_bus(
        &daemon.address,
        &svc_sentinel,
        &["charging", "set-thresholds", "60", "85"],
    );
    assert!(
        !out1.status.success(),
        "expected non-zero exit when start setter fails"
    );
    let writes1 = minus_one_backend.written_ends.lock().unwrap().clone();
    assert!(
        !writes1.contains(&-1),
        "R07: CLI must filter < 0 sentinel baseline and never send SetChargeEndThreshold(-1); writes={writes1:?}"
    );

    // Case 2: valid baseline (70), end set to 85 succeeds, start fails, rollback to 70 returns false ->
    // CLI stderr must explicitly report rollback failure.
    let fail_rb_backend = Arc::new(ChargingRollbackMockBackend {
        prev_end: 70,
        written_ends: Mutex::new(Vec::new()),
        fail_second_end_write: true,
    });
    let svc_rb = unique_service_name("r07_rollback");
    let _conn2 = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(svc_rb.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &_conn2,
        Arc::clone(&fail_rb_backend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .unwrap();

    let out2 = run_cli_on_bus(
        &daemon.address,
        &svc_rb,
        &["charging", "set-thresholds", "60", "85"],
    );
    assert!(!out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr).to_ascii_lowercase();
    assert!(
        stderr2.contains("rollback failed") || stderr2.contains("failed to roll back"),
        "R07: CLI must check rollback Result<bool> and report rollback failure on stderr, got: {stderr2}"
    );

    // Source verification: charging.rs must not discard rollback result with `let _ =`.
    let charging_src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/commands/charging.rs"),
    )
    .expect("read charging.rs");
    assert!(
        !charging_src.contains(
            "let _ = crate::client::with_rpc_timeout(\n                            client.proxy().set_charge_end_threshold(prev)"
        ),
        "R07: charging.rs must inspect rollback Result<bool> instead of discarding it"
    );
}

struct SleepingGpuMockBackend;

impl UccdBackend for SleepingGpuMockBackend {
    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("speed".to_string(), OwnedValue::from(45_i32)),
            ("temp".to_string(), OwnedValue::from(58_i32)),
            ("timestamp".to_string(), OwnedValue::from(1_i64)),
        ])
    }

    // Return empty GPU data so GpuMetrics temperature/frequency/power/fan_percent remain `None`.
    fn get_fan_data_gpu1(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }

    fn get_dgpu_info_values_json(&self) -> String {
        "{}".to_string()
    }
}

/// N20: `monitor --json` must preserve `null` for unavailable (`None`) GPU metrics instead of
/// coercing `None` to `0` via `.unwrap_or(0)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n20_monitor_json_outputs_null_for_unavailable_gpu_metrics() {
    let daemon = PrivateDbusDaemon::spawn();
    let svc = unique_service_name("n20_monitor");
    let _conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(svc.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &_conn,
        Arc::new(SleepingGpuMockBackend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .unwrap();

    let out = run_cli_on_bus(&daemon.address, &svc, &["--json", "monitor", "-n", "1"]);
    assert!(
        out.status.success(),
        "monitor --json -n 1 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first_line = stdout
        .lines()
        .next()
        .expect("monitor must emit a JSON line");
    let row: Value = serde_json::from_str(first_line)
        .unwrap_or_else(|e| panic!("invalid monitor JSON '{first_line}': {e}"));

    assert!(
        row["gpu"]["temperature"].is_null(),
        "N20: unavailable GPU temperature must be JSON null, got {:?}",
        row["gpu"]["temperature"]
    );
    assert!(
        row["gpu"]["frequency"].is_null(),
        "N20: unavailable GPU frequency must be JSON null, got {:?}",
        row["gpu"]["frequency"]
    );
}

struct SlowPolkitWriteMockBackend;

impl UccdBackend for SlowPolkitWriteMockBackend {
    fn set_active_profile(&self, _profile_id: &str) -> bool {
        // Simulate interactive Polkit approval taking 1.8s (> 1.5s read timeout, << 30s Polkit window).
        std::thread::sleep(Duration::from_millis(1800));
        true
    }
}

/// R24: Write / Polkit-authenticated CLI commands (`profile set`) must not use the 1500ms read
/// timeout (`DEFAULT_RPC_TIMEOUT`), which cuts off interactive Polkit prompts (up to 30s).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r24_cli_write_rpc_timeout_allows_polkit_interaction() {
    let daemon = PrivateDbusDaemon::spawn();
    let svc = unique_service_name("r24_polkit_timeout");
    let _conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(svc.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &_conn,
        Arc::new(SlowPolkitWriteMockBackend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .unwrap();

    let out = run_cli_on_bus(&daemon.address, &svc, &["profile", "set", "default"]);
    assert!(
        out.status.success(),
        "R24: `ucc-cli profile set` timed out at 1500ms instead of allowing Polkit interaction window; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
