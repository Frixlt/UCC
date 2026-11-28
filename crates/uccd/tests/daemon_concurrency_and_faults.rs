// SPDX-License-Identifier: GPL-3.0-or-later

//! Concurrent and fault-injection stress tests for `uccd`:
//! 1. Burst D-Bus commands: concurrent storm of read/write commands with no deadlock or panic
//! 2. Profile switch during slow backend reply (300–3000 ms): generation/epoch ordering
//! 3. Sensor failure at high temperature (>=85–95 °C): fail-safe cooling (never 0% duty)
//! 4. D-Bus owner disconnect while holding power request override: automatic eviction & restore
//! 5. Bounded queues, request maps, disconnect cache, metrics history, and lane backpressure

use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use ucc_config::{
    ConfigManager, PROFILE_ID_HIGH_PERFORMANCE, PROFILE_ID_OFFICE, PROFILE_ID_QUIET,
};
use ucc_core::UccProfile;
use ucc_dbus::{AllowAllAuthorizer, UccdBackend, UccdProxy, DBUS_PATH};
use ucc_hardware::{
    decode_history_binary, default_process_runner, DeviceCapabilities, DeviceVendor, MetricId,
    MetricsHistoryStore, MockPlatformBackend, MockSysfs, PlatformBackend, PlatformBackendKind,
    PlatformCapabilities, PlatformError, PlatformWrite, PowerDeviceController, Sysfs,
    TelemetryConfig, MAX_POINTS_PER_SERIES,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{DaemonRuntime, LidPolicyConfig, LiveUccdBackend, UccdDependencies};

static BUS_SEQ: AtomicUsize = AtomicUsize::new(0);

fn next_bus_name(tag: &str) -> String {
    format!(
        "com.uniwill.uccd.Stress.{tag}.p{}.n{}",
        std::process::id(),
        BUS_SEQ.fetch_add(1, Ordering::SeqCst)
    )
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
            water_cooler_supported: false,
            ctgp_supported: false,
            hwp_dynamic_boost_supported: false,
            tdp_control: true,
            charging_thresholds: true,
            ..Default::default()
        },
        tdp_channels: 2,
        perf_profiles_available: 3,
    }
}

fn seed_mock_sysfs(root: &Path, ac_online: bool) -> Arc<MockSysfs> {
    let sysfs = Arc::new(MockSysfs::new());
    let cpu0 = root.join("sys/devices/system/cpu/cpu0/cpufreq");
    sysfs.set_file(root.join("sys/devices/system/cpu/present"), "0-1");
    sysfs.set_file(cpu0.join("scaling_cur_freq"), "3200000");
    sysfs.set_file(cpu0.join("scaling_governor"), "powersave");
    sysfs.set_file(
        cpu0.join("scaling_available_governors"),
        "performance powersave",
    );
    sysfs.set_file(
        cpu0.join("energy_performance_preference"),
        "balance_performance",
    );
    sysfs.set_file(
        cpu0.join("energy_performance_available_preferences"),
        "default performance balance_performance balance_power power",
    );
    sysfs.set_file(root.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(root.join("sys/class/hwmon/hwmon0/temp1_input"), "55000");
    sysfs.set_file(root.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(
        root.join("sys/class/power_supply/AC/online"),
        if ac_online { "1" } else { "0" },
    );
    let bat = root.join("sys/class/power_supply/BAT0");
    sysfs.set_file(bat.join("type"), "Battery");
    sysfs.set_file(bat.join("charge_control_start_threshold"), "40");
    sysfs.set_file(bat.join("charge_control_end_threshold"), "80");
    sysfs
}

fn build_mock_platform() -> Arc<MockPlatformBackend> {
    let platform = Arc::new(MockPlatformBackend::new(default_uniwill_caps()));
    platform.set_fan_temp(0, 50);
    platform.set_fan_temp(1, 48);
    platform.set_fan_speed(0, 40);
    platform.set_fan_speed(1, 35);
    platform.set_tdp_limits(0, (15, 110));
    platform.set_tdp_limits(1, (20, 130));
    platform.set_tdp_value(0, 55);
    platform.set_tdp_value(1, 75);
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    platform
}

fn build_backend_with_platform(
    root: &Path,
    platform: Arc<dyn PlatformBackend>,
    sysfs: Arc<dyn Sysfs>,
) -> Arc<LiveUccdBackend> {
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
    Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Stress Test Laptop".into(),
        platform,
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
    }))
}

fn build_default_backend(
    root: &Path,
    ac_online: bool,
) -> (Arc<LiveUccdBackend>, Arc<MockPlatformBackend>, Arc<MockSysfs>) {
    let sysfs = seed_mock_sysfs(root, ac_online);
    let platform = build_mock_platform();
    let backend = build_backend_with_platform(root, platform.clone(), sysfs.clone());
    (backend, platform, sysfs)
}

async fn build_proxy(conn: &zbus::Connection, bus_name: String) -> UccdProxy<'static> {
    UccdProxy::builder(conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap()
}

/// Platform wrapper injecting a configurable latency (300–3000 ms) on a specific target TDP write
/// so tests can deterministically race a newer profile switch against an in-flight slow write.
struct SlowTdpPlatform {
    inner: Arc<MockPlatformBackend>,
    armed: Arc<AtomicBool>,
    target_watts: i32,
    delay: Duration,
    entered: Arc<(Mutex<bool>, Condvar)>,
}

impl PlatformBackend for SlowTdpPlatform {
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
        self.inner.set_fans_auto()
    }

    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
        self.inner.read_tdp(index)
    }

    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(index)
    }

    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
        if index == 0
            && watts == self.target_watts
            && self.armed.swap(false, Ordering::SeqCst)
        {
            {
                let mut flag = self.entered.0.lock().unwrap();
                *flag = true;
                self.entered.1.notify_all();
            }
            std::thread::sleep(self.delay);
        }
        self.inner.set_tdp(index, watts)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        self.inner.set_perf_profile(profile_code)
    }

    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
        self.inner.set_mode_enable(enable)
    }

    fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
        self.inner.set_mode(mode)
    }

    fn read_webcam(&self) -> Result<bool, PlatformError> {
        self.inner.read_webcam()
    }

    fn set_webcam(&self, enabled: bool) -> Result<(), PlatformError> {
        self.inner.set_webcam(enabled)
    }
}

/// Platform wrapper blocking `set_tdp` on a release gate to exercise bounded D-Bus lane
/// backpressure.
struct BlockingProfilePlatform {
    inner: Arc<MockPlatformBackend>,
    armed: Arc<AtomicBool>,
    entered: Arc<Mutex<usize>>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl PlatformBackend for BlockingProfilePlatform {
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
        self.inner.set_fans_auto()
    }

    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
        self.inner.read_tdp(index)
    }

    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(index)
    }

    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
        if index == 0 && self.armed.load(Ordering::SeqCst) {
            {
                let mut count = self.entered.lock().unwrap();
                *count += 1;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut open = self.release.0.lock().unwrap();
            while !*open {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let (guard, timeout) = self.release.1.wait_timeout(open, remaining).unwrap();
                open = guard;
                if timeout.timed_out() {
                    break;
                }
            }
        }
        self.inner.set_tdp(index, watts)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        self.inner.set_perf_profile(profile_code)
    }

    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
        self.inner.set_mode_enable(enable)
    }

    fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
        self.inner.set_mode(mode)
    }

    fn read_webcam(&self) -> Result<bool, PlatformError> {
        self.inner.read_webcam()
    }

    fn set_webcam(&self, enabled: bool) -> Result<(), PlatformError> {
        self.inner.set_webcam(enabled)
    }
}

/// Scenario 1: Concurrent storm of read/write D-Bus commands from multiple tasks must complete
/// without deadlock or panic, return valid responses, and leave a consistent final state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_1_burst_dbus_commands_no_deadlock_and_consistent_final_state() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _) = build_default_backend(root.path(), true);
    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("burst");
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let mut handles = Vec::new();

    // 6 concurrent reader tasks hammering profile, status, policy, settings, and fan queries.
    for reader_idx in 0..6 {
        let proxy = build_proxy(&conn, bus_name.clone()).await;
        handles.push(tokio::spawn(async move {
            for iter in 0..15 {
                let active_raw = proxy.get_active_profile_json().await.unwrap();
                let active: Value = serde_json::from_str(&active_raw).unwrap();
                assert!(
                    active["id"].as_str().is_some_and(|s| !s.is_empty()),
                    "reader {reader_idx} iter {iter}: invalid active profile JSON"
                );

                let profiles_raw = proxy.get_profiles_json().await.unwrap();
                let profiles: Value = serde_json::from_str(&profiles_raw).unwrap();
                assert!(profiles.as_array().is_some_and(|a| !a.is_empty()));

                let state = proxy.get_power_state().await.unwrap();
                assert!(
                    matches!(
                        state.as_str(),
                        "power_ac" | "power_bat" | "power_type_c" | "power_wc"
                    ),
                    "unexpected power state: {state}"
                );

                let policy_raw = proxy.get_power_policy_json().await.unwrap();
                let policy: Value = serde_json::from_str(&policy_raw).unwrap();
                assert!(policy["current_mode"].is_string());

                let settings_raw = proxy.get_settings_json().await.unwrap();
                let settings: Value = serde_json::from_str(&settings_raw).unwrap();
                assert!(settings.is_object());

                let fan_cpu = proxy.get_fan_data_cpu().await.unwrap();
                assert!(fan_cpu.contains_key("temp"));
                assert!(fan_cpu.contains_key("speed"));
            }
        }));
    }

    // 3 concurrent profile & lighting writer tasks.
    let profile_ids = [
        PROFILE_ID_OFFICE,
        PROFILE_ID_QUIET,
        PROFILE_ID_HIGH_PERFORMANCE,
    ];
    for writer_idx in 0..3 {
        let proxy = build_proxy(&conn, bus_name.clone()).await;
        handles.push(tokio::spawn(async move {
            for iter in 0..10 {
                let target = profile_ids[(writer_idx + iter) % profile_ids.len()];
                let _ = proxy.set_active_profile(target).await.unwrap();
                let lb_json = json!({
                    "managed": false,
                    "mode": "solid",
                    "brightness": 50 + ((writer_idx * 10 + iter) % 50)
                })
                .to_string();
                let _ = proxy.set_lightbar_settings_json(&lb_json).await.unwrap();
            }
        }));
    }

    // 3 concurrent policy & app power request writer tasks.
    let modes = ["ac", "bat", "auto"];
    for policy_idx in 0..3 {
        let proxy = build_proxy(&conn, bus_name.clone()).await;
        handles.push(tokio::spawn(async move {
            let app_name = format!("burst-app-{policy_idx}");
            for iter in 0..10 {
                let mode = modes[(policy_idx + iter) % modes.len()];
                let _ = proxy.set_manual_power_mode(mode).await.unwrap();
                let _ = proxy
                    .register_power_request(&app_name, "burst stress", "ac")
                    .await
                    .unwrap();
                let _ = proxy.unregister_power_request(&app_name).await.unwrap();
            }
        }));
    }

    let storm = async {
        for handle in handles {
            handle.await.expect("burst worker task panicked");
        }
    };
    tokio::time::timeout(Duration::from_secs(15), storm)
        .await
        .expect("burst D-Bus command storm timed out (possible deadlock)");

    // Verify deterministic, consistent final state after the concurrent storm settles.
    let verify_proxy = build_proxy(&conn, bus_name).await;
    assert!(verify_proxy.set_manual_power_mode("auto").await.unwrap());
    assert!(verify_proxy
        .set_active_profile(PROFILE_ID_OFFICE)
        .await
        .unwrap());

    assert_eq!(verify_proxy.get_power_state().await.unwrap(), "power_ac");
    let final_active: Value =
        serde_json::from_str(&verify_proxy.get_active_profile_json().await.unwrap()).unwrap();
    assert_eq!(final_active["id"].as_str(), Some(PROFILE_ID_OFFICE));

    let final_policy: Value =
        serde_json::from_str(&verify_proxy.get_power_policy_json().await.unwrap()).unwrap();
    assert_eq!(final_policy["manual_mode"].as_str(), Some("auto"));
    assert!(final_policy["registered_apps"]
        .as_array()
        .is_some_and(|apps| apps.is_empty()));

    runtime.shutdown(Duration::from_secs(2)).await;
}

/// Scenario 2: Profile switch arriving mid-flight during a slow backend reply (1000 ms, within the
/// 300–3000 ms window) must win via commit-epoch ordering so the stale profile never overwrites
/// the newer profile, while unrelated read RPCs stay responsive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_2_profile_switch_during_slow_backend_preserves_newer_profile() {
    let root = tempfile::tempdir().unwrap();
    let sysfs = seed_mock_sysfs(root.path(), true);
    let inner_platform = build_mock_platform();
    let armed = Arc::new(AtomicBool::new(false));
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let slow_delay = Duration::from_millis(1000);

    let slow_platform: Arc<dyn PlatformBackend> = Arc::new(SlowTdpPlatform {
        inner: Arc::clone(&inner_platform),
        armed: Arc::clone(&armed),
        target_watts: 45,
        delay: slow_delay,
        entered: Arc::clone(&entered),
    });
    let backend = build_backend_with_platform(root.path(), slow_platform, sysfs);

    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("slow_profile");
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let setup_proxy = build_proxy(&conn, bus_name.clone()).await;

    let mut slow_profile = UccProfile::new("slow_profile_a", "Slow Profile A");
    slow_profile.odm_profile_name = Some("power_save".to_string());
    slow_profile.odm_power_limits = vec![45, 65];
    assert!(setup_proxy
        .add_custom_profile(&serde_json::to_string(&slow_profile).unwrap())
        .await
        .unwrap());

    let mut fast_profile = UccProfile::new("fast_profile_b", "Fast Profile B");
    fast_profile.odm_profile_name = Some("overboost".to_string());
    fast_profile.odm_power_limits = vec![85, 105];
    assert!(setup_proxy
        .add_custom_profile(&serde_json::to_string(&fast_profile).unwrap())
        .await
        .unwrap());

    // Pre-build proxies before launching `slow_task` so proxy setup never eats into `slow_delay`.
    let slow_proxy = build_proxy(&conn, bus_name.clone()).await;
    let read_proxy = build_proxy(&conn, bus_name.clone()).await;
    let fast_proxy = build_proxy(&conn, bus_name.clone()).await;

    // Arm the 1000 ms delay exclusively for `slow_profile_a` (TDP channel 0 == 45W).
    armed.store(true, Ordering::SeqCst);

    let slow_started = Instant::now();
    let slow_task =
        tokio::spawn(async move { slow_proxy.set_active_profile("slow_profile_a").await });

    // Wait deterministically until `slow_profile_a` is mid-flight inside `set_tdp`.
    let entered_clone = Arc::clone(&entered);
    let entered_ok = tokio::task::spawn_blocking(move || {
        let mut flag = entered_clone.0.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !*flag {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (guard, timeout) = entered_clone.1.wait_timeout(flag, remaining).unwrap();
            flag = guard;
            if timeout.timed_out() {
                return *flag;
            }
        }
        true
    })
    .await
    .unwrap();
    assert!(
        entered_ok,
        "slow profile transaction did not reach platform set_tdp in time"
    );

    // 1. Verify the daemon stays responsive to read RPCs while the slow backend call is in flight.
    let read_start = Instant::now();
    assert_eq!(
        read_proxy.get_device_name().await.unwrap(),
        "Stress Test Laptop"
    );
    assert_eq!(read_proxy.get_power_state().await.unwrap(), "power_ac");
    assert!(
        read_start.elapsed() < Duration::from_millis(500),
        "read RPCs blocked behind slow backend profile switch ({:?})",
        read_start.elapsed()
    );

    // 2. Issue the newer profile switch mid-flight while `slow_profile_a` is still sleeping.
    assert!(
        fast_proxy
            .set_active_profile("fast_profile_b")
            .await
            .unwrap(),
        "newer mid-flight profile switch must succeed"
    );

    // 3. Wait for the stale slow transaction and verify it did NOT overwrite `fast_profile_b`.
    let slow_result = tokio::time::timeout(Duration::from_secs(5), slow_task)
        .await
        .expect("slow profile task timed out")
        .expect("slow profile task panicked")
        .expect("slow profile D-Bus call failed");
    assert!(
        slow_started.elapsed() >= Duration::from_millis(300),
        "expected slow backend delay in 300..=3000 ms window, got {:?}",
        slow_started.elapsed()
    );
    assert!(
        slow_result,
        "superseded slow profile should reconcile cleanly rather than erroring"
    );

    let active: UccProfile =
        serde_json::from_str(&fast_proxy.get_active_profile_json().await.unwrap()).unwrap();
    assert_eq!(
        active.id, "fast_profile_b",
        "stale slow profile must NOT overwrite newer committed profile"
    );
    assert_eq!(
        inner_platform.read_tdp(0).unwrap(),
        85,
        "stale 45W TDP write must be reconciled back to newer 85W target"
    );
    assert_eq!(
        inner_platform.read_tdp(1).unwrap(),
        105,
        "newer 105W TDP channel 1 target must remain intact"
    );
    assert_eq!(
        inner_platform.read_perf_profile().unwrap(),
        3,
        "newer ODM perf profile (overboost = 3) must remain active"
    );

    runtime.shutdown(Duration::from_secs(2)).await;
}

/// Scenario 3: Sensor read errors, out-of-range readings, and NaN telemetry injected right after
/// high temperature (>=85–95 °C) must trigger fail-safe firmware-auto cooling and NEVER drop fan
/// duty to 0%.
#[tokio::test]
async fn stress_3_sensor_failure_at_high_temperature_triggers_failsafe_cooling() {
    let root = tempfile::tempdir().unwrap();
    let (backend, platform, sysfs) = build_default_backend(root.path(), true);
    backend.activate_hardware();
    assert!(backend.set_active_profile(PROFILE_ID_HIGH_PERFORMANCE));

    // Drive sustained high temperatures (92 °C CPU, 88 °C GPU) and confirm active cooling.
    platform.set_fan_temp(0, 92);
    platform.set_fan_temp(1, 88);
    let mut high_tick = backend.tick_fans_once();
    for _ in 0..3 {
        high_tick = backend.tick_fans_once();
    }
    assert!(!high_tick.fallback_auto);
    assert_eq!(high_tick.temps_c, vec![Some(92), Some(88)]);
    let cpu_duty = high_tick.speeds_percent[0].expect("CPU fan duty must be applied at 92 °C");
    let gpu_duty = high_tick.speeds_percent[1].expect("GPU fan duty must be applied at 88 °C");
    assert!(
        cpu_duty >= 60 && gpu_duty >= 60,
        "expected aggressive cooling at >=88 °C, got CPU={cpu_duty}% GPU={gpu_duty}%"
    );

    // Sub-case A: Single-channel sensor read failure right after 92 °C must fall back to auto
    // and NEVER command 0% duty.
    platform.clear_journal();
    platform.set_fan_temp_error(0);
    let fail_tick_a = backend.tick_fans_once();
    assert!(
        fail_tick_a.fallback_auto,
        "sensor read error at high temp must engage fail-safe firmware auto"
    );
    assert!(fail_tick_a.temps_c[0].is_none());
    let journal_a = platform.journal();
    assert!(
        journal_a.contains(&PlatformWrite::SetFansAuto),
        "expected SetFansAuto write on sensor error, journal={journal_a:?}"
    );
    assert!(
        !journal_a
            .iter()
            .any(|w| matches!(w, PlatformWrite::SetFanSpeed { percent: 0, .. })),
        "fail-safe cooling must NEVER command 0% fan duty: {journal_a:?}"
    );

    // Sub-case B: Recover to 95 °C / 90 °C, then inject out-of-range (-1 °C) + missing GPU sensor.
    platform.set_fan_temp(0, 95);
    platform.set_fan_temp(1, 90);
    let recovered_tick = backend.tick_fans_once();
    assert!(!recovered_tick.fallback_auto);

    platform.clear_journal();
    platform.set_fan_temp(0, -1);
    platform.set_fan_temp_error(1);
    let fail_tick_b = backend.tick_fans_once();
    assert!(
        fail_tick_b.fallback_auto,
        "out-of-range/missing sensor readings after 95 °C must engage fail-safe firmware auto"
    );
    let journal_b = platform.journal();
    assert!(journal_b.contains(&PlatformWrite::SetFansAuto));
    assert!(
        !journal_b
            .iter()
            .any(|w| matches!(w, PlatformWrite::SetFanSpeed { percent: 0, .. })),
        "fail-safe cooling must NEVER command 0% fan duty on out-of-range sensor: {journal_b:?}"
    );

    // Sub-case C: Transient EC write failure during fail-safe must retry `set_fans_auto`.
    platform.set_fan_temp(0, 91);
    platform.set_fan_temp(1, 87);
    assert!(!backend.tick_fans_once().fallback_auto);

    platform.clear_journal();
    platform.set_fan_temp_error(0);
    platform.set_fail_writes(true);
    let failed_auto_tick = backend.tick_fans_once();
    assert!(
        !failed_auto_tick.fallback_auto,
        "fallback_auto must remain false while SetFansAuto hardware write fails"
    );

    platform.set_fail_writes(false);
    let retried_auto_tick = backend.tick_fans_once();
    assert!(
        retried_auto_tick.fallback_auto,
        "subsequent tick must retry SetFansAuto once hardware writes recover"
    );
    assert!(platform.journal().contains(&PlatformWrite::SetFansAuto));
    assert!(!platform
        .journal()
        .iter()
        .any(|w| matches!(w, PlatformWrite::SetFanSpeed { percent: 0, .. })));

    // Sub-case D: Non-numeric / NaN sysfs temperature reading must not panic telemetry.
    sysfs.set_file(root.path().join("sys/class/hwmon/hwmon0/temp1_input"), "NaN");
    let tel = backend.poll_telemetry_once();
    assert!(
        tel.cpu.temp_c.is_none(),
        "NaN sysfs temperature must yield None rather than invalid reading"
    );
}

/// Scenario 4: Ephemeral D-Bus clients holding power mode overrides under unique bus names must
/// be automatically evicted on disconnect (`NameOwnerChanged`), restoring the expected effective
/// power state without leaking stale owners.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stress_4_dbus_owner_disconnect_evicts_override_and_restores_power_state() {
    let root = tempfile::tempdir().unwrap();
    // Start on battery so client requests visibly elevate effective power state.
    let (backend, _, _) = build_default_backend(root.path(), false);
    let bus_name = next_bus_name("owner_disconnect");
    let host_conn = zbus::Connection::session().await.unwrap();
    let runtime = DaemonRuntime::start_on_connection(
        host_conn.clone(),
        bus_name.clone(),
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let observer = build_proxy(&host_conn, bus_name.clone()).await;
    assert_eq!(
        observer.get_power_state().await.unwrap(),
        "power_bat",
        "baseline physical battery supply must start in power_bat"
    );

    // Client A acquires an AC override; Client B acquires a Type-C override.
    let client_a_conn = zbus::Connection::session().await.unwrap();
    let client_a_owner = client_a_conn.unique_name().unwrap().as_str().to_string();
    let proxy_a = build_proxy(&client_a_conn, bus_name.clone()).await;

    let client_b_conn = zbus::Connection::session().await.unwrap();
    let client_b_owner = client_b_conn.unique_name().unwrap().as_str().to_string();
    let proxy_b = build_proxy(&client_b_conn, bus_name.clone()).await;

    assert!(proxy_a
        .register_power_request("render-job", "GPU rendering", "ac")
        .await
        .unwrap());
    assert!(proxy_b
        .register_power_request("sync-job", "Large upload", "type_c")
        .await
        .unwrap());

    assert_eq!(observer.get_power_state().await.unwrap(), "power_ac");
    let active_policy: Value =
        serde_json::from_str(&observer.get_power_policy_json().await.unwrap()).unwrap();
    let active_apps = active_policy["registered_apps"].as_array().unwrap();
    assert_eq!(active_apps.len(), 2);
    assert!(active_apps
        .iter()
        .any(|r| r["owner"] == client_a_owner && r["name"] == "render-job"));
    assert!(active_apps
        .iter()
        .any(|r| r["owner"] == client_b_owner && r["name"] == "sync-job"));

    // Disconnect Client A while Client B stays connected: effective state must step down to type_c.
    drop(proxy_a);
    drop(client_a_conn);

    let deadline_a = Instant::now() + Duration::from_secs(3);
    let mut stepped_down = false;
    while Instant::now() < deadline_a {
        let state = observer.get_power_state().await.unwrap();
        let policy: Value =
            serde_json::from_str(&observer.get_power_policy_json().await.unwrap()).unwrap();
        let apps = policy["registered_apps"].as_array().unwrap();
        if state == "power_type_c"
            && apps.len() == 1
            && apps[0]["owner"] == client_b_owner
            && !apps.iter().any(|r| r["owner"] == client_a_owner)
        {
            stepped_down = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        stepped_down,
        "disconnecting client A must evict its AC override and step down to client B's power_type_c"
    );

    // Late registration attempting to reuse departed Client A's unique bus name must be rejected.
    assert!(
        !backend.register_power_request_with_sender(
            &client_a_owner,
            "late-stale-request",
            "must be rejected after disconnect",
            "ac",
        ),
        "disconnected unique bus name must not be allowed to leak back into power requests"
    );

    // Disconnect Client B: all overrides must clear and physical battery power_bat restored.
    drop(proxy_b);
    drop(client_b_conn);

    let deadline_b = Instant::now() + Duration::from_secs(3);
    let mut fully_restored = false;
    while Instant::now() < deadline_b {
        let state = observer.get_power_state().await.unwrap();
        let policy: Value =
            serde_json::from_str(&observer.get_power_policy_json().await.unwrap()).unwrap();
        if state == "power_bat"
            && policy["current_mode"] == "bat"
            && policy["registered_apps"]
                .as_array()
                .is_some_and(|a| a.is_empty())
        {
            fully_restored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        fully_restored,
        "disconnecting last client must restore power_bat with empty request table"
    );

    runtime.shutdown(Duration::from_secs(2)).await;
}

/// Scenario 5: Hammering app power requests, disconnected-sender tracking, metrics history, and
/// D-Bus offload command lanes beyond capacity must remain strictly bounded with backpressure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_5_bounded_queues_requests_history_and_lane_backpressure() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _) = build_default_backend(root.path(), true);
    backend.activate_hardware();

    // 1. PowerPolicy request caps: max 32 per owner, max 256 total.
    let mut owner0_accepted = 0usize;
    for i in 0..40 {
        if backend.register_power_request_with_sender(
            ":1.2000",
            &format!("app-{i}"),
            "cap test",
            "ac",
        ) {
            owner0_accepted += 1;
        }
    }
    assert_eq!(
        owner0_accepted, 32,
        "single owner must be strictly capped at 32 requests"
    );
    // Updating an existing request key for a full owner must still succeed without growing count.
    assert!(backend.register_power_request_with_sender(
        ":1.2000",
        "app-0",
        "updated reason",
        "type_c",
    ));

    let mut total_accepted = owner0_accepted;
    for owner_idx in 1..10 {
        let owner = format!(":1.{}", 2000 + owner_idx);
        for req_idx in 0..32 {
            if backend.register_power_request_with_sender(
                &owner,
                &format!("app-{req_idx}"),
                "global cap test",
                "ac",
            ) {
                total_accepted += 1;
            }
        }
    }
    assert_eq!(
        total_accepted, 256,
        "global power request table must be strictly capped at 256 entries"
    );
    let policy: Value = serde_json::from_str(&backend.get_power_policy_json()).unwrap();
    assert_eq!(policy["registered_apps"].as_array().unwrap().len(), 256);

    // Evicting one full owner frees 32 slots cleanly.
    backend.on_dbus_client_disconnected(":1.2000");
    let policy_after_evict: Value =
        serde_json::from_str(&backend.get_power_policy_json()).unwrap();
    assert_eq!(
        policy_after_evict["registered_apps"]
            .as_array()
            .unwrap()
            .len(),
        224
    );

    // 2. Disconnected-sender cache must remain bounded (FIFO cap of 1024).
    for i in 0..1200 {
        backend.on_dbus_client_disconnected(&format!(":1.{}", 10_000 + i));
    }
    assert!(
        !backend.register_power_request_with_sender(":1.11199", "recent", "blocked", "ac"),
        "most recent disconnected sender must still be tracked"
    );
    assert!(
        backend.register_power_request_with_sender(":1.10000", "evicted-old", "allowed", "ac"),
        "oldest disconnected sender beyond 1024 FIFO bound must have been evicted"
    );

    // 3. Metrics history horizon clamping and per-series ring-buffer bound.
    backend.set_monitor_history_horizon(5);
    assert_eq!(
        backend.get_monitor_history_horizon(),
        MetricsHistoryStore::MIN_HORIZON_S
    );
    backend.set_monitor_history_horizon(99_999);
    assert_eq!(
        backend.get_monitor_history_horizon(),
        MetricsHistoryStore::MAX_HORIZON_S
    );

    let history = MetricsHistoryStore::new();
    history.set_horizon_seconds(60);
    // Push non-finite values alongside 12,500 valid samples within the 60s window.
    history.push(MetricId::CpuTemp, 1_000_000, f64::NAN);
    history.push(MetricId::CpuTemp, 1_000_001, f64::INFINITY);
    for i in 0..12_500_i64 {
        history.push(MetricId::CpuTemp, 1_000_000 + i, 50.0 + (i % 40) as f64);
    }
    let encoded_cap = history.query_since_binary(0);
    let decoded_cap = decode_history_binary(&encoded_cap).unwrap();
    assert_eq!(decoded_cap.len(), 1);
    assert_eq!(decoded_cap[0].0, MetricId::CpuTemp);
    assert_eq!(
        decoded_cap[0].1.len(),
        MAX_POINTS_PER_SERIES,
        "history series must remain strictly capped at MAX_POINTS_PER_SERIES"
    );
    assert!(decoded_cap[0].1.iter().all(|pt| pt.value.is_finite()));

    // Advance timestamp beyond the 60 s horizon and confirm expired points are evicted.
    history.push(MetricId::CpuTemp, 1_200_000, 62.0);
    let encoded_horizon = history.query_since_binary(0);
    let decoded_horizon = decode_history_binary(&encoded_horizon).unwrap();
    assert_eq!(
        decoded_horizon[0].1.len(),
        1,
        "samples older than the 60s horizon must be evicted"
    );

    // 4. Bounded D-Bus offload command lane backpressure (Profile lane cap = 4 permits).
    let lane_root = tempfile::tempdir().unwrap();
    let lane_sysfs = seed_mock_sysfs(lane_root.path(), true);
    let lane_inner = build_mock_platform();
    let armed = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Mutex::new(0usize));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let blocking_platform: Arc<dyn PlatformBackend> = Arc::new(BlockingProfilePlatform {
        inner: Arc::clone(&lane_inner),
        armed: Arc::clone(&armed),
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    });
    let lane_backend = build_backend_with_platform(lane_root.path(), blocking_platform, lane_sysfs);
    let conn = zbus::Connection::session().await.unwrap();
    let bus_name = next_bus_name("bounded_lane");
    let runtime = DaemonRuntime::start_on_connection(
        conn.clone(),
        bus_name.clone(),
        lane_backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let mut prof = UccProfile::new("lane_stress_prof", "Lane Stress");
    prof.odm_power_limits = vec![60, 80];
    let prof_json = serde_json::to_string(&prof).unwrap();

    armed.store(true, Ordering::SeqCst);

    // Saturate all 4 permits of `OffloadLane::Profile`.
    let mut saturated = Vec::new();
    for _ in 0..4 {
        let p = build_proxy(&conn, bus_name.clone()).await;
        let payload = prof_json.clone();
        saturated.push(tokio::spawn(async move { p.apply_profile(&payload).await }));
    }

    let wait_deadline = Instant::now() + Duration::from_secs(3);
    while *entered.lock().unwrap() < 4 && Instant::now() < wait_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        *entered.lock().unwrap(),
        4,
        "expected all 4 Profile lane slots to be occupied"
    );

    // Independent Read lane must stay immediately responsive while Profile lane is saturated.
    let read_proxy = build_proxy(&conn, bus_name.clone()).await;
    let t_read = Instant::now();
    assert_eq!(
        read_proxy.get_device_name().await.unwrap(),
        "Stress Test Laptop"
    );
    assert!(
        t_read.elapsed() < Duration::from_millis(1000),
        "Read lane starved by saturated Profile lane ({:?})",
        t_read.elapsed()
    );

    // Hammer overflow requests on the saturated Profile lane: must fail-fast (`false`).
    let mut overflow_tasks = Vec::new();
    for _ in 0..4 {
        let p = build_proxy(&conn, bus_name.clone()).await;
        let payload = prof_json.clone();
        overflow_tasks.push(tokio::spawn(async move {
            p.apply_profile(&payload).await.unwrap()
        }));
    }
    for task in overflow_tasks {
        let accepted = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("overflow call hung instead of failing fast with backpressure")
            .expect("overflow task panicked");
        assert!(
            !accepted,
            "excess command beyond bounded Profile lane capacity must be rejected"
        );
    }

    // Release blocked profile tasks and verify clean teardown.
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    for h in saturated {
        let ok = tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("saturated task timed out")
            .expect("saturated task panicked")
            .expect("saturated apply_profile D-Bus call failed");
        assert!(ok, "saturated apply_profile must succeed once unblocked");
    }
    runtime.shutdown(Duration::from_secs(2)).await;
}
