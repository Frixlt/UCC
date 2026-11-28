// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproduction tests for confirmed `uccd` findings on commit `1d07126`:
//! - N01: Lid close on battery with `lid.bat = "suspend"` must invoke logind `Suspend`
//! - N02: `apply_fan_profiles` must accept/normalize fractional duty values from Qt drag
//! - N04: Saving a temporarily applied fan profile must preserve the original revert target
//! - N06: Concurrent `apply_fan_profiles` + `revert_fan_profiles` must not invert lock order
//! - R06: Corrupted `SleepStateStore` JSON file must be recoverable by `save()` and `clear()`
//! - R08: `PidLock::drop` must not unlink after `LOCK_UN`, and `stop_daemon` must fail closed on unreadable `/proc/{pid}/comm`
//! - N30: Concurrent `SaveCustomProfile` calls at 63/64 must respect `MAX_CUSTOM_PROFILES` (<= 64)

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use ucc_config::ConfigManager;
use ucc_core::UccProfile;
use ucc_dbus::{AllowAllAuthorizer, UccdBackend};
use ucc_hardware::{
    default_process_runner, DeviceCapabilities, DeviceVendor, MockPlatformBackend, MockSysfs,
    PlatformBackendKind, PlatformCapabilities, PowerDeviceController, TelemetryConfig,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{
    DaemonRuntime, LidPolicyConfig, LiveUccdBackend, SleepState, SleepStateStore, UccdDependencies,
};
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

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
            .expect("dbus-daemon must be installed to run private D-Bus daemon tests");
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

fn default_uniwill_caps() -> PlatformCapabilities {
    PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            water_cooler_supported: true,
            ctgp_supported: true,
            hwp_dynamic_boost_supported: true,
            tdp_control: true,
            charging_thresholds: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    }
}

fn build_backend(tmp: &Path, ac_online: bool) -> Arc<LiveUccdBackend> {
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/present"), "0-1");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "55000");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(
        tmp.join("sys/class/power_supply/AC/online"),
        if ac_online { "1" } else { "0" },
    );

    let platform = Arc::new(MockPlatformBackend::new(default_uniwill_caps()));
    platform.set_fan_temp(0, 55);
    platform.set_fan_temp(1, 50);
    platform.set_fan_speed(0, 40);
    platform.set_fan_speed(1, 35);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));

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
        zones: 4,
        max_brightness: 255,
    }));

    Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform,
        sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(tmp.join("settings.json")),
        sys_root: tmp.to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp.to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            tmp.join("sys"),
            tmp.join("proc"),
            default_process_runner(),
            tmp.join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_millis(50),
        },
    }))
}

struct MockLogindManager {
    lid_closed: Arc<AtomicBool>,
    inhibit_calls: Arc<AtomicUsize>,
    suspend_calls: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl MockLogindManager {
    #[zbus(property, name = "LidClosed")]
    fn lid_closed(&self) -> bool {
        self.lid_closed.load(Ordering::SeqCst)
    }

    async fn inhibit(
        &self,
        _what: String,
        _who: String,
        _why: String,
        _mode: String,
    ) -> zbus::fdo::Result<OwnedFd> {
        self.inhibit_calls.fetch_add(1, Ordering::SeqCst);
        let file = std::fs::File::open("/dev/null")
            .map_err(|e| zbus::fdo::Error::Failed(format!("open /dev/null failed: {e}")))?;
        Ok(OwnedFd::from(std::os::fd::OwnedFd::from(file)))
    }

    async fn suspend(&self, _interactive: bool) -> zbus::fdo::Result<()> {
        self.suspend_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn hibernate(&self, _interactive: bool) -> zbus::fdo::Result<()> {
        Ok(())
    }

    async fn power_off(&self, _interactive: bool) -> zbus::fdo::Result<()> {
        Ok(())
    }

    async fn lock_sessions(&self) -> zbus::fdo::Result<()> {
        Ok(())
    }
}

/// N01: When `lid.bat = "suspend"` is configured and the laptop is on battery, closing the lid
/// must invoke `org.freedesktop.login1.Manager.Suspend` instead of holding `handle-lid-switch`
/// block inhibitor without executing the configured suspend action.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n01_lid_close_on_battery_invokes_logind_suspend() {
    let daemon = PrivateDbusDaemon::spawn();
    let lid_closed = Arc::new(AtomicBool::new(false));
    let inhibit_calls = Arc::new(AtomicUsize::new(0));
    let suspend_calls = Arc::new(AtomicUsize::new(0));

    let login_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at(
            "/org/freedesktop/login1",
            MockLogindManager {
                lid_closed: Arc::clone(&lid_closed),
                inhibit_calls: Arc::clone(&inhibit_calls),
                suspend_calls: Arc::clone(&suspend_calls),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), false);
    assert!(
        backend.set_power_device_settings_json(
            &json!({
                "lid": {
                    "ac": "ignore",
                    "type_c": "ignore",
                    "bat": "suspend"
                }
            })
            .to_string(),
        ),
        "failed to configure lid.bat = suspend"
    );

    let daemon_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let bus_name = format!("org.test.uccd.n01_{}", std::process::id());
    let runtime = DaemonRuntime::start_on_connection(
        daemon_conn,
        bus_name,
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        true,
    )
    .await
    .unwrap();

    // Allow logind monitor loop to acquire inhibitor and subscribe to LidClosed signals.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Emit LidClosed = true on mock org.freedesktop.login1.Manager.
    lid_closed.store(true, Ordering::SeqCst);
    let iface_ref = login_conn
        .object_server()
        .interface::<_, MockLogindManager>("/org/freedesktop/login1")
        .await
        .unwrap();
    let emitter = SignalEmitter::new(&login_conn, "/org/freedesktop/login1").unwrap();
    iface_ref
        .get()
        .await
        .lid_closed_changed(&emitter)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    runtime.shutdown(Duration::from_secs(2)).await;

    assert!(
        inhibit_calls.load(Ordering::SeqCst) >= 1,
        "expected uccd to acquire handle-lid-switch inhibitor when lid policy is configured"
    );
    assert!(
        suspend_calls.load(Ordering::SeqCst) >= 1,
        "N01: closing lid on battery with lid.bat=\"suspend\" must invoke logind Suspend, got {} calls",
        suspend_calls.load(Ordering::SeqCst)
    );
}

/// N02: `apply_fan_profiles` must accept/normalize fractional duty values produced by dragging
/// control points in Qt `FanCurveEditorWidget` (e.g., `54.910714285714285` at 60 °C).
#[test]
fn n02_apply_fan_profiles_accepts_float_duty_from_qt_drag() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), true);

    let float_payload =
        r#"{"cpu":[{"temp":60,"speed":54.910714285714285}],"gpu":[{"temp":60,"speed":50}]}"#;
    let integer_control = r#"{"cpu":[{"temp":60,"speed":55}],"gpu":[{"temp":60,"speed":50}]}"#;

    assert!(
        backend.apply_fan_profiles(integer_control),
        "integer control payload should be accepted"
    );
    assert!(
        backend.apply_fan_profiles(float_payload),
        "N02: apply_fan_profiles must accept/normalize fractional duty values from Qt drag"
    );
}

/// N04: Starting from `fan-balanced`, temporarily applying `audit-temporary-fan`, saving
/// `audit-temporary-fan`, and calling `revert_fan_profiles` must restore active fan profile
/// back to `fan-balanced`.
#[test]
fn n04_save_temporary_fan_profile_preserves_revert_target() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), true);

    // Ensure baseline custom fan profile `fan-balanced` and temporary preset `audit-temporary-fan` exist.
    let balanced_curve = json!({
        "cpu": [{"temp": 40, "speed": 25}, {"temp": 80, "speed": 75}],
        "gpu": [{"temp": 40, "speed": 25}, {"temp": 80, "speed": 75}]
    })
    .to_string();
    let temp_curve = json!({
        "cpu": [{"temp": 40, "speed": 35}, {"temp": 80, "speed": 90}],
        "gpu": [{"temp": 40, "speed": 35}, {"temp": 80, "speed": 90}]
    })
    .to_string();

    assert!(backend.save_custom_fan_profile("fan-balanced", "Fan Balanced", &balanced_curve));
    assert!(backend.save_custom_fan_profile(
        "audit-temporary-fan",
        "Audit Temporary Fan",
        &temp_curve
    ));

    // Apply a profile whose committed fan profile is `fan-balanced`.
    let mut base_profile = UccProfile::new("n04_base", "N04 Base");
    base_profile.fan.fan_profile = "fan-balanced".to_string();
    assert!(backend.apply_profile(&serde_json::to_string(&base_profile).unwrap()));

    let before: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(before["fan"]["fanProfile"].as_str(), Some("fan-balanced"));

    // Temporarily apply `audit-temporary-fan`, then save edits to `audit-temporary-fan`.
    let apply_temp = json!({
        "fanProfileId": "audit-temporary-fan",
        "cpu": [{"temp": 40, "speed": 40}, {"temp": 80, "speed": 95}],
        "gpu": [{"temp": 40, "speed": 40}, {"temp": 80, "speed": 95}]
    })
    .to_string();
    assert!(backend.apply_fan_profiles(&apply_temp));

    let edited_temp_curve = json!({
        "cpu": [{"temp": 40, "speed": 45}, {"temp": 80, "speed": 98}],
        "gpu": [{"temp": 40, "speed": 45}, {"temp": 80, "speed": 98}]
    })
    .to_string();
    assert!(backend.save_custom_fan_profile(
        "audit-temporary-fan",
        "Audit Temporary Fan",
        &edited_temp_curve
    ));

    // Reverting temporary fan profiles must return active fan profile to `fan-balanced`.
    assert!(backend.revert_fan_profiles());
    let after: Value = serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    assert_eq!(
        after["fan"]["fanProfile"].as_str(),
        Some("fan-balanced"),
        "N04: revert_fan_profiles must restore original fan profile 'fan-balanced', got {:?}",
        after["fan"]["fanProfile"]
    );
}

/// N06: Concurrent `apply_fan_profiles` and `revert_fan_profiles` must complete without deadlock
/// (no AB-BA lock inversion via Rust 2021 `if let` temporary `MutexGuard` retention in `cooling.rs`).
#[test]
fn n06_apply_and_revert_fan_profiles_do_not_invert_lock_order() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cooling_path = manifest_dir.join("src/backend/cooling.rs");
    let cooling_src = std::fs::read_to_string(&cooling_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", cooling_path.display()));

    // Source guard: Rust 2021 `if let Some(...) = self.lock_mutex(&self.temporary_saved_fan_profile_id).take()`
    // holds the MutexGuard across the entire `if let` body while `active_profile` is acquired inside.
    assert!(
        !cooling_src
            .contains("if let Some(prev) = self.lock_mutex(&self.temporary_saved_fan_profile_id)")
            && !cooling_src.contains(
                "if let Some(saved) = self.lock_mutex(&self.temporary_saved_fan_profile_id)"
            )
            && !cooling_src.contains(
                "if let Some(orig) = self.lock_mutex(&self.temporary_saved_fan_profile_id)"
            ),
        "N06: cooling.rs must not retain temporary_saved_fan_profile_id MutexGuard across `if let` body while locking active_profile"
    );

    // Concurrent stress test with a hard watchdog deadline so any deadlock fails deterministically.
    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), true);
    let payload = json!({
        "fanProfileId": "Balanced",
        "cpu": [{"temp": 45, "speed": 30}, {"temp": 85, "speed": 85}],
        "gpu": [{"temp": 45, "speed": 30}, {"temp": 85, "speed": 85}]
    })
    .to_string();

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let b_worker = Arc::clone(&backend);
    std::thread::spawn(move || {
        let barrier = Arc::new(Barrier::new(2));
        let b1 = Arc::clone(&b_worker);
        let bar1 = Arc::clone(&barrier);
        let p1 = payload.clone();
        let t1 = std::thread::spawn(move || {
            bar1.wait();
            for _ in 0..100 {
                let _ = b1.apply_fan_profiles(&p1);
            }
        });

        let b2 = Arc::clone(&b_worker);
        let bar2 = Arc::clone(&barrier);
        let t2 = std::thread::spawn(move || {
            bar2.wait();
            for _ in 0..100 {
                let _ = b2.revert_fan_profiles();
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        let _ = done_tx.send(());
    });

    done_rx.recv_timeout(Duration::from_secs(5)).expect(
        "N06: concurrent apply_fan_profiles + revert_fan_profiles deadlocked past 5s deadline",
    );
}

/// R06: A corrupted/truncated JSON sleep snapshot owned by the expected UID must not permanently
/// block `SleepStateStore::save()` or `SleepStateStore::clear()`.
#[test]
fn r06_corrupted_sleep_state_file_can_be_overwritten_and_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("ucc-sleep");
    std::fs::create_dir(&state_dir).unwrap();
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let uid = unsafe { nix::libc::geteuid() };
    let snapshot_path = state_dir.join("sleep-state.json");
    let store = SleepStateStore::new(&snapshot_path, uid);

    // Write truncated JSON with valid 0600 permissions and current UID ownership.
    std::fs::write(&snapshot_path, r#"{"was_active": tru"#).unwrap();
    std::fs::set_permissions(&snapshot_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    // `save()` must overwrite the corrupted regular file instead of failing on `self.load()?`.
    let fresh = SleepState {
        was_active: true,
        manual_mode: "ac".to_string(),
        cooler_enabled: false,
    };
    let save_res = store.save(&fresh);
    assert!(
        save_res.is_ok(),
        "R06: SleepStateStore::save() must recover from corrupted JSON, got: {:?}",
        save_res
    );
    assert_eq!(store.load().unwrap(), Some(fresh));

    // Re-corrupt and verify `clear()` can remove the corrupted file cleanly.
    std::fs::write(&snapshot_path, r#"{"was_active": tru"#).unwrap();
    std::fs::set_permissions(&snapshot_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let clear_res = store.clear();
    assert!(
        clear_res.is_ok(),
        "R06: SleepStateStore::clear() must remove corrupted JSON file, got: {:?}",
        clear_res
    );
    assert!(
        !snapshot_path.exists(),
        "R06: corrupted sleep snapshot must be removed after clear()"
    );
}

/// R08: `PidLock::drop` must unlink before releasing `LOCK_UN` (avoiding race against a new lock
/// holder), and `stop_daemon` must fail closed rather than sending `SIGTERM` when `/proc/{pid}/comm`
/// cannot be read.
#[test]
fn r08_pidlock_and_stop_daemon_fail_closed_on_unreadable_comm() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main_path = manifest_dir.join("src/main.rs");
    let source = std::fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", main_path.display()));

    // Check PidLock::drop ordering: must not call LOCK_UN before fs::remove_file(&self.path).
    let drop_start = source
        .find("impl Drop for PidLock")
        .expect("impl Drop for PidLock must exist in crates/uccd/src/main.rs");
    let drop_section = &source[drop_start..source.len().min(drop_start + 500)];
    if let (Some(unlock_pos), Some(remove_pos)) = (
        drop_section.find("LOCK_UN"),
        drop_section.find("remove_file"),
    ) {
        assert!(
            remove_pos < unlock_pos,
            "R08: PidLock::drop must unlink PID file before releasing flock(LOCK_UN)"
        );
    }

    // Check stop_daemon: reading `/proc/{pid}/comm` must fail closed (return non-zero on Err),
    // never silently fall through via `if let Ok(comm) = fs::read_to_string(...)`.
    let stop_start = source
        .find("fn stop_daemon")
        .expect("fn stop_daemon must exist in crates/uccd/src/main.rs");
    let stop_section = &source[stop_start..];
    assert!(
        !stop_section.contains("if let Ok(comm) = fs::read_to_string(format!(\"/proc/{pid}/comm\"))"),
        "R08: stop_daemon must fail closed when /proc/{{pid}}/comm is unreadable instead of using `if let Ok(comm)`"
    );
}

/// N30: With 63 custom profiles already stored, two concurrent `SaveCustomProfile` calls with
/// distinct new IDs must allow at most 1 to succeed so total custom profiles never exceed 64.
#[test]
fn n30_concurrent_save_custom_profile_respects_max_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), true);

    for i in 0..63 {
        let prof = UccProfile::new(format!("n30_seed_{i}"), format!("N30 Seed {i}"));
        assert!(
            backend.save_custom_profile(&serde_json::to_string(&prof).unwrap()),
            "seeding custom profile {i} should succeed"
        );
    }

    let prof_a = serde_json::to_string(&UccProfile::new("n30_race_a", "N30 Race A")).unwrap();
    let prof_b = serde_json::to_string(&UccProfile::new("n30_race_b", "N30 Race B")).unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let b1 = Arc::clone(&backend);
    let bar1 = Arc::clone(&barrier);
    let t1 = std::thread::spawn(move || {
        bar1.wait();
        b1.save_custom_profile(&prof_a)
    });

    let b2 = Arc::clone(&backend);
    let bar2 = Arc::clone(&barrier);
    let t2 = std::thread::spawn(move || {
        bar2.wait();
        b2.save_custom_profile(&prof_b)
    });

    let ok1 = t1.join().unwrap();
    let ok2 = t2.join().unwrap();
    let succeeded = usize::from(ok1) + usize::from(ok2);

    let custom_json = backend.get_custom_profiles_json();
    let custom_list: Vec<Value> = serde_json::from_str(&custom_json).unwrap();

    assert!(
        succeeded <= 1,
        "N30: at most 1 of 2 concurrent SaveCustomProfile calls at 63/64 may succeed, got {succeeded}"
    );
    assert!(
        custom_list.len() <= 64,
        "N30: custom profile count exceeded MAX_CUSTOM_PROFILES (64): {}",
        custom_list.len()
    );

    // Deterministic guard against TOCTOU check-outside-write-lock in profile_tx.rs:
    // the MAX_CUSTOM_PROFILES cap must be enforced inside `update_settings(...)` before insert.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let profile_tx_path = manifest_dir.join("src/backend/profile_tx.rs");
    let source = std::fs::read_to_string(&profile_tx_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", profile_tx_path.display()));
    let fn_start = source
        .find("fn persist_or_reapply_custom_profile")
        .or_else(|| source.find("fn save_custom_profile"))
        .expect(
            "profile_tx.rs must reference persist_or_reapply_custom_profile or save_custom_profile",
        );
    let update_pos = source[fn_start..]
        .find(".update_settings(|s|")
        .expect("save_custom_profile must call update_settings");
    let update_block =
        &source[fn_start + update_pos..source.len().min(fn_start + update_pos + 600)];
    assert!(
        update_block.contains("MAX_CUSTOM_PROFILES"),
        "N30: MAX_CUSTOM_PROFILES must be re-checked atomically inside update_settings write lock"
    );
}

/// R03: `GetPrimeState` must offload subprocess waiting so a slow `prime-select` helper (sleeping
/// up to 2s) does not stall the Tokio async worker or block concurrent independent D-Bus RPCs.
#[tokio::test(flavor = "current_thread")]
async fn r03_get_prime_state_does_not_block_tokio_async_worker() {
    use std::time::Instant;
    use ucc_dbus::{UccdProxy, DBUS_PATH};

    let daemon = PrivateDbusDaemon::spawn();
    let tmp = tempfile::tempdir().unwrap();

    // Place a slow fake `prime-select` helper under `<sys_root>/usr/bin/prime-select`.
    let bin_dir = tmp.path().join("usr/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let prime_bin = bin_dir.join("prime-select");
    std::fs::write(&prime_bin, "#!/bin/sh\nsleep 0.8\necho nvidia\n").unwrap();
    std::fs::set_permissions(&prime_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let backend = build_backend(tmp.path(), true);
    let daemon_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let bus_name = format!("org.test.uccd.r03_{}", std::process::id());
    let runtime = DaemonRuntime::start_on_connection(
        daemon_conn,
        bus_name.clone(),
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        true,
    )
    .await
    .unwrap();

    let client_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let proxy = UccdProxy::builder(&client_conn)
        .destination(bus_name.clone())
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();

    let prime_proxy = proxy.clone();
    let prime_task = tokio::spawn(async move { prime_proxy.get_prime_state().await });

    // Give the server a moment to enter `GetPrimeState`.
    tokio::time::sleep(Duration::from_millis(60)).await;

    let probe_start = Instant::now();
    let power_state = proxy.get_power_state().await.unwrap();
    let probe_elapsed = probe_start.elapsed();
    assert!(!power_state.is_empty());

    let prime_state = prime_task.await.unwrap().unwrap();
    runtime.shutdown(Duration::from_secs(2)).await;

    assert_eq!(prime_state, "dGPU");
    assert!(
        probe_elapsed < Duration::from_millis(400),
        "R03: GetPrimeState blocked the single Tokio async worker for {:?} while waiting on prime-select",
        probe_elapsed
    );

    // Source guard: `ucc-dbus/src/interface.rs` must offload `get_prime_state` rather than calling it inline.
    let interface_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ucc-dbus/src/interface.rs");
    let iface_src = std::fs::read_to_string(&interface_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", interface_path.display()));
    let fn_start = iface_src
        .find("pub async fn get_prime_state")
        .expect("get_prime_state must exist in ucc-dbus/src/interface.rs");
    let fn_block = &iface_src[fn_start..iface_src.len().min(fn_start + 700)];
    assert!(
        !fn_block.contains("self.backend.get_prime_state()")
            || fn_block.contains("spawn_blocking")
            || fn_block.contains("offload"),
        "R03: interface.rs `get_prime_state` must offload blocking prime-select execution off the Tokio async worker"
    );
}

/// P0-02: `lighting_power.rs` must not hold `lid_saved_keyboard_states` or `lid_saved_wc_rgb`
/// `MutexGuard` across an `if let` body while acquiring `keyboard_ctrl` / `cooler_controller`
/// (AB-BA lock inversion with `on_lid_state_changed`).
#[test]
fn p0_02_lighting_power_lid_and_restore_no_abba_deadlock() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lp_path = manifest_dir.join("src/backend/lighting_power.rs");
    let lp_src = std::fs::read_to_string(&lp_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lp_path.display()));

    // Source guard: Rust 2021 `if let Some(...) = self.lock_mutex(&self.lid_saved_*).take()`
    // retains the MutexGuard across the entire `if let` block while acquiring keyboard_ctrl/cooler_controller.
    assert!(
        !lp_src.contains("if let Some(states) = self.lock_mutex(&self.lid_saved_keyboard_states)"),
        "P0-02: restore_lid_lighting must drop lid_saved_keyboard_states MutexGuard before locking keyboard_ctrl"
    );
    assert!(
        !lp_src.contains("= self.lock_mutex(&self.lid_saved_wc_rgb).take()")
            || !lp_src.contains("if let Some(RgbTarget::Color"),
        "P0-02: restore_lid_lighting must drop lid_saved_wc_rgb MutexGuard before locking cooler_controller"
    );

    // Concurrent stress test with a hard 5s watchdog deadline so any AB-BA deadlock fails deterministically.
    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), true);
    assert!(backend.set_power_device_settings_json(
        &json!({
            "lid": {
                "turnOffKeyboardBacklightOnLidClose": true,
                "turnOffWaterCoolerLedOnLidClose": true
            }
        })
        .to_string(),
    ));
    assert!(backend.set_keyboard_backlight_states_json(
        &json!([{"brightness": 180, "red": 255, "green": 128, "blue": 64}]).to_string(),
    ));

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let b_worker = Arc::clone(&backend);
    std::thread::spawn(move || {
        let barrier = Arc::new(Barrier::new(2));
        let b1 = Arc::clone(&b_worker);
        let bar1 = Arc::clone(&barrier);
        let t1 = std::thread::spawn(move || {
            bar1.wait();
            for _ in 0..100 {
                b1.on_lid_state_changed(true);
                b1.on_lid_state_changed(false);
            }
        });

        let b2 = Arc::clone(&b_worker);
        let bar2 = Arc::clone(&barrier);
        let t2 = std::thread::spawn(move || {
            bar2.wait();
            for _ in 0..100 {
                b2.restore_lid_lighting();
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        let _ = done_tx.send(());
    });

    done_rx.recv_timeout(Duration::from_secs(5)).expect(
        "P0-02: concurrent on_lid_state_changed + restore_lid_lighting deadlocked past 5s deadline",
    );
}

#[test]
fn water_cooler_led_mode_and_pump_bounds_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = build_backend(tmp.path(), true);

    // Valid LED modes (0=Static, 1=Breathe, 2=Colorful, 3=BreatheColor) with valid RGB bounds.
    for mode in 0..=3 {
        assert!(
            backend.set_water_cooler_led_color(64, 128, 255, mode),
            "valid LED mode {mode} with in-range RGB must be accepted"
        );
    }

    // Unknown/out-of-range LED mode falls back safely to Static (returns true) when RGB is valid,
    // whereas out-of-range RGB channels (< 0 or > 255) must be rejected.
    assert!(backend.set_water_cooler_led_color(10, 20, 30, -1));
    assert!(backend.set_water_cooler_led_color(10, 20, 30, 99));
    assert!(!backend.set_water_cooler_led_color(-1, 128, 128, 0));
    assert!(!backend.set_water_cooler_led_color(128, 256, 128, 1));
    assert!(!backend.set_water_cooler_led_color(128, 128, 300, 2));
    assert!(!backend.set_water_cooler_led_color(-5, 300, -1, 99));

    // Pump voltage bounds: discrete code 1 (V12) is hardware-forbidden and must always be rejected,
    // along with out-of-range codes (< 0 or > 4).
    assert!(
        !backend.set_water_cooler_pump_voltage(1),
        "forbidden pump voltage code 1 (V12) must be rejected"
    );
    assert_eq!(backend.get_water_cooler_pump_level(), -1);

    for invalid_code in [-1, 1, 5, 99] {
        assert!(
            !backend.set_water_cooler_pump_voltage(invalid_code),
            "invalid/forbidden pump voltage code {invalid_code} must be rejected"
        );
    }

    // Allowed discrete codes: 0 (V11), 2 (V7), 3 (V8), 4 (Off).
    for valid_code in [0, 2, 3, 4] {
        assert!(
            backend.set_water_cooler_pump_voltage(valid_code),
            "valid pump voltage code {valid_code} must be accepted"
        );
        assert_eq!(backend.get_water_cooler_pump_level(), valid_code);
    }

    // Re-verify forbidden V12 (1) does not clobber the last valid pump level (4).
    assert!(!backend.set_water_cooler_pump_voltage(1));
    assert_eq!(backend.get_water_cooler_pump_level(), 4);
}
