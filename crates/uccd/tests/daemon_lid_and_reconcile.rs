// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for v2 daemon & D-Bus fixes:
//! - `V2-P1-01`: No infinite 3x/200ms profile reapply loop when `state_map` has `None`
//! - `V2-P1-02`: `LidController` AC-unplug tracking via `reconcile_power_state`, per-profile
//!   `display.lid_close_action` override, and dynamic `should_inhibit_lid_switch`
//! - `V2-P1-03`: Explicit ACPI wakeup allowlist preserving lid & internal keyboard devices
//! - User Bug #2: Water cooler LED `RgbTarget::Unset` restores default white + BLE packet on lid open
//! - `V2-P2-02`: `prepare_sleep` recovers from corrupted JSON in a safe owner-only snapshot
//! - `V2-P2-05`: Clearing active power state via `SetStateMap` / `SetBatchStateMap` triggers fallback
//! - `V2-P2-07`: `apply_cpu_defaults_snapshot` orders min before max when lowering max and tolerates EPP `EBUSY`

use serde_json::json;
use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ucc_config::ConfigManager;
use ucc_core::UccProfile;
use ucc_dbus::UccdBackend;
use ucc_hardware::{
    default_process_runner, CoolerPacketEncoder, DeviceCapabilities, DeviceVendor,
    MockPlatformBackend, MockSysfs, PlatformBackendKind, PlatformCapabilities,
    PowerDeviceController, RgbState, RgbTarget, Sysfs, TelemetryConfig, CMD_RGB,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{
    apply_wakeup_policy, prepare_sleep, LidController, LidPolicyConfig, LidSourceAction,
    LidSourcePolicy, LiveUccdBackend, SleepOperations, SleepState, SleepStateStore,
    UccdDependencies, WakeupApplyResult,
};

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

fn build_backend(
    tmp: &Path,
    sysfs: Arc<dyn Sysfs>,
) -> (Arc<LiveUccdBackend>, Arc<MockPlatformBackend>) {
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

    let config_path = tmp.join("settings.json");
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: platform.clone(),
        sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(config_path),
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
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));
    (backend, platform)
}

fn seed_basic_sysfs(tmp: &Path, sysfs: &MockSysfs) {
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/present"), "0-1");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "55000");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");
}

#[test]
fn v2_p1_01_none_state_map_fallback_does_not_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(MockSysfs::new());
    seed_basic_sysfs(tmp.path(), &sysfs);
    let (backend, platform) = build_backend(tmp.path(), sysfs.clone());
    backend.activate_hardware();

    // Clear AC state_map entry so `get_active_profile_for_mode_with_fallback("ac", Some("ac"))` is None.
    assert!(backend.set_state_map("ac", ""));

    platform.clear_journal();
    backend.reconcile_power_state(false);
    assert!(
        !backend.consume_mode_reapply_pending(),
        "V2-P1-01: fallback commit when state_map has None must not leave mode_reapply_pending=true"
    );

    // Subsequent 200ms reconcile ticks must NOT re-apply the profile endlessly.
    platform.clear_journal();
    backend.reconcile_power_state(false);
    backend.reconcile_power_state(false);
    assert!(
        platform.journal().is_empty(),
        "V2-P1-01: subsequent reconcile ticks must not loop-reapply profile: {:?}",
        platform.journal()
    );
}

#[test]
fn v2_p1_02_lid_controller_updates_ac_online_on_reconcile_and_debounce() {
    let policy = LidSourcePolicy {
        ac: LidSourceAction::Ignore,
        type_c: LidSourceAction::Ignore,
        bat: LidSourceAction::Suspend,
        ..Default::default()
    };
    let mut ctrl = LidController::new(LidPolicyConfig {
        ignore_lid_on_ac: true,
        ac_unplug_debounce: Duration::from_secs(3),
    });

    let t0 = Instant::now();
    ctrl.update_power_source(t0, true);
    // Unplug AC at t1 = t0 + 1s
    let t1 = t0 + Duration::from_secs(1);
    ctrl.update_power_source(t1, false);

    // Closing lid 1s after unplug (within 3s debounce) -> Ignore
    let t_soon = t1 + Duration::from_secs(1);
    assert_eq!(
        ctrl.action_for_source(&policy, "bat", t_soon, false, true),
        LidSourceAction::Ignore
    );

    // Closing lid 10s after unplug (well past 3s debounce) -> Suspend!
    let t_later = t1 + Duration::from_secs(10);
    let _ = ctrl.evaluate_lid_event(t_later, false, true);
    assert_eq!(
        ctrl.action_for_source(&policy, "bat", t_later, false, true),
        LidSourceAction::Suspend,
        "V2-P1-02: unplugging AC earlier must not re-arm 3s debounce on first lid close"
    );

    // Also verify end-to-end through LiveUccdBackend::reconcile_power_state:
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(MockSysfs::new());
    seed_basic_sysfs(tmp.path(), &sysfs);
    let (backend, _) = build_backend(tmp.path(), sysfs.clone());
    backend.activate_hardware();

    // Start on AC, then unplug AC and run reconcile_power_state (updates lid_controller).
    backend.reconcile_power_state(false);
    sysfs.set_file(tmp.path().join("sys/class/power_supply/AC/online"), "0");
    backend.reconcile_power_state(false);
    // Immediately closing lid within 3s of unplug is debounced to Ignore.
    assert_eq!(backend.evaluate_lid_close_action(), LidSourceAction::Ignore);
}

#[test]
fn v2_p1_02_profile_lid_close_action_override_and_inhibitor_predicate() {
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(MockSysfs::new());
    seed_basic_sysfs(tmp.path(), &sysfs);
    let (backend, _) = build_backend(tmp.path(), sysfs.clone());
    backend.activate_hardware();

    // Initially neither powerDevices.lid nor active_profile.display.lid_close_action is set.
    assert!(!backend.should_inhibit_lid_switch());

    // Configure powerDevices.lid -> ac="ignore", bat="suspend".
    let pd_lid = json!({
        "enabled": false,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "ignore",
            "type_c": "ignore",
            "bat": "suspend"
        }
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_lid));
    assert!(backend.should_inhibit_lid_switch());
    assert_eq!(backend.evaluate_lid_close_action(), LidSourceAction::Ignore);

    // Apply a profile with explicit `display.lid_close_action = Some("hibernate")`:
    // it overrides powerDevices.lid.ac ("ignore").
    let mut prof: UccProfile =
        serde_json::from_str(&backend.get_active_profile_json()).expect("valid active profile");
    prof.id = "custom-lid-profile".to_string();
    prof.name = "Custom Lid Profile".to_string();
    prof.display.lid_close_action = Some("hibernate".to_string());
    let prof_json = serde_json::to_string(&prof).unwrap();
    assert!(backend.save_custom_profile(&prof_json));
    assert!(backend.set_active_profile("custom-lid-profile"));
    assert_eq!(
        backend.evaluate_lid_close_action(),
        LidSourceAction::Hibernate,
        "V2-P1-02: active_profile.display.lid_close_action must override powerDevices.lid.<source>"
    );

    // Even if powerDevices.lid is removed, the profile's lid_close_action still requires inhibitor.
    let pd_no_lid = json!({
        "enabled": false,
        "keepNvidiaAwake": false
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_no_lid));
    assert!(backend.should_inhibit_lid_switch());

    // Clearing profile lid_close_action back to empty releases inhibitor requirement.
    prof.display.lid_close_action = Some(String::new());
    let prof_json = serde_json::to_string(&prof).unwrap();
    assert!(backend.save_custom_profile(&prof_json));
    assert!(backend.set_active_profile("custom-lid-profile"));
    assert!(!backend.should_inhibit_lid_switch());
}

#[test]
fn v2_p1_03_acpi_wakeup_allowlist_preserves_lid_and_keyboard() {
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(MockSysfs::new());
    let acpi_wakeup = tmp.path().join("proc/acpi/wakeup");
    sysfs.set_file(
        &acpi_wakeup,
        concat!(
            "Device\tS-state\tStatus   Sysfs node\n",
            "LID\t  S4\t*enabled   platform:PNP0C0D:00\n",
            "LID0\t  S4\t*enabled   platform:PNP0C0D:00\n",
            "LID1\t  S4\t*enabled   platform:PNP0C0D:01\n",
            "PS2K\t  S3\t*enabled   pnp:00:05\n",
            "KBC\t  S3\t*enabled   pnp:00:06\n",
            "KBC0\t  S3\t*enabled   pnp:00:07\n",
            "KBD\t  S3\t*enabled   pnp:00:08\n",
            "KBD0\t  S3\t*enabled   pnp:00:09\n",
            "XHCI\t  S3\t*enabled   pci:0000:00:14.0\n",
            "PEG0\t  S4\t*enabled   pci:0000:00:01.0\n",
            "GLAN\t  S4\t*disabled  pci:0000:00:1f.6\n",
        ),
    );

    let res: WakeupApplyResult = apply_wakeup_policy(
        true,
        &tmp.path().join("sys"),
        &tmp.path().join("proc"),
        sysfs.as_ref(),
    );
    assert_eq!(
        res,
        WakeupApplyResult {
            enabled: 0,
            disabled: 2,
            failed: 0,
        }
    );
    let acpi_writes: Vec<String> = sysfs
        .journal()
        .into_iter()
        .filter(|w| w.path == acpi_wakeup)
        .map(|w| w.content)
        .collect();
    assert_eq!(
        acpi_writes,
        vec!["XHCI\n".to_string(), "PEG0\n".to_string()],
        "V2-P1-03: allowlisted lid/keyboard ACPI devices must never be toggled off"
    );
}

fn drain_cooler_packets(backend: &LiveUccdBackend, max_rounds: usize) -> Vec<[u8; 8]> {
    let mut out = Vec::new();
    for round in 0..max_rounds {
        if round > 0 {
            std::thread::sleep(Duration::from_millis(90));
        }
        let batch = backend.drain_cooler_commands();
        if batch.is_empty() {
            if !out.is_empty() {
                break;
            }
        } else {
            out.extend(batch);
        }
    }
    out
}

#[test]
fn user_bug_2_wc_led_unset_restores_default_white_and_enqueues_ble_packet() {
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(MockSysfs::new());
    seed_basic_sysfs(tmp.path(), &sysfs);
    let (backend, _) = build_backend(tmp.path(), sysfs.clone());
    backend.activate_hardware();

    let pd_wc_off = json!({
        "enabled": false,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "ignore",
            "type_c": "ignore",
            "bat": "suspend",
            "turnOffKeyboardBacklightOnLidClose": false,
            "turnOffWaterCoolerLedOnLidClose": true
        }
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_wc_off));

    backend.set_cooler_connected_state(true);
    assert_eq!(backend.cooler_desired_rgb(), RgbTarget::Unset);

    // Close lid -> turns off WC LED and queues TurnOffLed packet.
    assert!(backend.on_lid_state_changed(true));
    assert_eq!(backend.cooler_desired_rgb(), RgbTarget::Off);
    let off_pkts = drain_cooler_packets(&backend, 8);
    assert!(off_pkts.contains(&CoolerPacketEncoder::turn_off(CMD_RGB)));

    // Open lid -> must NOT leave hardware dark with Unset; restores default static white and queues RGB packet.
    assert!(backend.on_lid_state_changed(false));
    assert_eq!(
        backend.cooler_desired_rgb(),
        RgbTarget::Color {
            red: 255,
            green: 255,
            blue: 255,
            state: RgbState::Static,
        }
    );
    let restore_pkts = drain_cooler_packets(&backend, 8);
    assert!(
        restore_pkts.contains(&CoolerPacketEncoder::rgb(255, 255, 255, RgbState::Static)),
        "User Bug #2: lid open must enqueue an explicit RGB packet to wake BLE loop"
    );
}

struct MockSleepOps {
    active: bool,
    stopped: Mutex<bool>,
}

impl SleepOperations for MockSleepOps {
    fn is_daemon_active(&self) -> Result<bool, String> {
        Ok(self.active)
    }

    fn capture_runtime_state(&self) -> Result<SleepState, String> {
        Ok(SleepState {
            was_active: true,
            manual_mode: "ac".to_string(),
            cooler_enabled: true,
        })
    }

    fn stop_daemon(&self) -> Result<(), String> {
        *self.stopped.lock().unwrap() = true;
        Ok(())
    }

    fn start_daemon(&self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn v2_p2_02_prepare_sleep_recovers_from_corrupted_safe_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("ucc-sleep");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let snap_path = dir.join("sleep-state.json");
    std::fs::write(&snap_path, b"{corrupt json!!!").unwrap();
    std::fs::set_permissions(&snap_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let uid = unsafe { nix::libc::geteuid() };
    let store = SleepStateStore::new(&snap_path, uid);
    let ops = MockSleepOps {
        active: true,
        stopped: Mutex::new(false),
    };

    prepare_sleep(&store, &ops)
        .expect("V2-P2-02: prepare_sleep must recover from corrupted JSON in a safe file");
    assert!(*ops.stopped.lock().unwrap());
    let loaded = store.load().unwrap().expect("overwritten valid snapshot");
    assert!(loaded.was_active);
    assert_eq!(loaded.manual_mode, "ac");
    assert!(loaded.cooler_enabled);

    // Unsafe permissions (0o644) must still be rejected.
    std::fs::set_permissions(&snap_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        prepare_sleep(&store, &ops).is_err(),
        "V2-P2-02: unsafe permissions must still fail prepare_sleep"
    );
}

#[test]
fn v2_p2_05_set_state_map_and_batch_clear_active_state_triggers_reconcile() {
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(MockSysfs::new());
    seed_basic_sysfs(tmp.path(), &sysfs);
    let (backend, platform) = build_backend(tmp.path(), sysfs.clone());
    backend.activate_hardware();

    // First reconcile settles AC state.
    backend.reconcile_power_state(false);
    platform.clear_journal();
    backend.reconcile_power_state(false);
    assert!(platform.journal().is_empty());

    // Clear active state ("ac") via SetStateMap -> must invalidate last_applied_profile_state
    // so next reconcile_power_state triggers fallback immediately.
    assert!(backend.set_state_map("ac", ""));
    platform.clear_journal();
    backend.reconcile_power_state(false);
    assert!(
        !platform.journal().is_empty(),
        "V2-P2-05: SetStateMap clearing active state must trigger immediate fallback reconcile"
    );

    // Re-bind AC and settle, then clear via SetBatchStateMap.
    let active_id = serde_json::from_str::<UccProfile>(&backend.get_active_profile_json())
        .unwrap()
        .id;
    assert!(backend.set_state_map("ac", &active_id));
    backend.reconcile_power_state(false);
    platform.clear_journal();
    assert!(backend.set_batch_state_map(&json!({ "ac": "" }).to_string()));
    backend.reconcile_power_state(false);
    assert!(
        !platform.journal().is_empty(),
        "V2-P2-05: SetBatchStateMap clearing active state must trigger immediate fallback reconcile"
    );
}

struct StrictCpufreqSysfs {
    inner: MockSysfs,
    ebusy_paths: Mutex<HashSet<PathBuf>>,
}

impl StrictCpufreqSysfs {
    fn new() -> Self {
        Self {
            inner: MockSysfs::new(),
            ebusy_paths: Mutex::new(HashSet::new()),
        }
    }

    fn set_ebusy(&self, path: impl AsRef<Path>) {
        self.ebusy_paths
            .lock()
            .unwrap()
            .insert(path.as_ref().to_path_buf());
    }
}

impl Sysfs for StrictCpufreqSysfs {
    fn read(&self, path: &Path) -> Result<String, std::io::Error> {
        self.inner.read(path)
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
        if self.ebusy_paths.lock().unwrap().contains(path) {
            return Err(std::io::Error::from_raw_os_error(nix::libc::EBUSY));
        }
        // Enforce kernel cpufreq invariant: scaling_min_freq <= scaling_max_freq.
        if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
            if fname == "scaling_max_freq" {
                if let (Ok(new_max), Some(cur_min)) = (
                    content.trim().parse::<u32>(),
                    path.parent()
                        .and_then(|p| self.inner.get_file(p.join("scaling_min_freq")))
                        .and_then(|v| v.trim().parse::<u32>().ok()),
                ) {
                    if new_max < cur_min {
                        return Err(std::io::Error::from_raw_os_error(nix::libc::EINVAL));
                    }
                }
            } else if fname == "scaling_min_freq" {
                if let (Ok(new_min), Some(cur_max)) = (
                    content.trim().parse::<u32>(),
                    path.parent()
                        .and_then(|p| self.inner.get_file(p.join("scaling_max_freq")))
                        .and_then(|v| v.trim().parse::<u32>().ok()),
                ) {
                    if new_min > cur_max {
                        return Err(std::io::Error::from_raw_os_error(nix::libc::EINVAL));
                    }
                }
            }
        }
        self.inner.write(path, content)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
        self.inner.read_dir(path)
    }
}

#[test]
fn v2_p2_07_restore_cpu_defaults_orders_min_before_max_and_tolerates_epp_ebusy() {
    let tmp = tempfile::tempdir().unwrap();
    let sysfs = Arc::new(StrictCpufreqSysfs::new());
    seed_basic_sysfs(tmp.path(), &sysfs.inner);

    // Seed two CPUs with initial defaults: min=800000, max=2000000.
    for cpu in ["cpu0", "cpu1"] {
        let base = tmp.path().join("sys/devices/system/cpu").join(cpu);
        let cf = base.join("cpufreq");
        if cpu == "cpu1" {
            sysfs.inner.set_file(base.join("online"), "1");
        }
        sysfs
            .inner
            .set_file(cf.join("scaling_available_governors"), "powersave performance");
        sysfs.inner.set_file(cf.join("scaling_governor"), "powersave");
        sysfs.inner.set_file(
            cf.join("energy_performance_available_preferences"),
            "performance balance_performance power",
        );
        sysfs
            .inner
            .set_file(cf.join("energy_performance_preference"), "balance_performance");
        sysfs.inner.set_file(cf.join("cpuinfo_min_freq"), "800000");
        sysfs.inner.set_file(cf.join("cpuinfo_max_freq"), "4500000");
        sysfs.inner.set_file(cf.join("scaling_min_freq"), "800000");
        sysfs.inner.set_file(cf.join("scaling_max_freq"), "2000000");
    }

    let (backend, _) = build_backend(tmp.path(), sysfs.clone());
    backend.activate_hardware();

    // Apply a profile that raises min=3000000, max=4000000 (so cur_min=3000000 > default_max=2000000)
    // and sets governor=performance, epp=performance.
    let mut prof: UccProfile =
        serde_json::from_str(&backend.get_active_profile_json()).unwrap();
    prof.id = "high-freq-prof".to_string();
    prof.name = "High Freq".to_string();
    prof.cpu.governor = "performance".to_string();
    prof.cpu.energy_performance_preference = "performance".to_string();
    prof.cpu.min_freq_khz = Some(3_000_000);
    prof.cpu.max_freq_khz = Some(4_000_000);
    assert!(backend.save_custom_profile(&serde_json::to_string(&prof).unwrap()));
    assert!(backend.set_active_profile("high-freq-prof"));

    assert_eq!(
        sysfs
            .inner
            .get_file(tmp.path().join("sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq"))
            .as_deref(),
        Some("3000000")
    );

    // Simulate EBUSY on cpu0's EPP during shutdown restore: must NOT abort cpu0 freq restore or cpu1 restore!
    sysfs.set_ebusy(
        tmp.path()
            .join("sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"),
    );

    backend.shutdown();

    for cpu in ["cpu0", "cpu1"] {
        let cf = tmp
            .path()
            .join("sys/devices/system/cpu")
            .join(cpu)
            .join("cpufreq");
        assert_eq!(
            sysfs.inner.get_file(cf.join("scaling_min_freq")).as_deref(),
            Some("800000"),
            "V2-P2-07: {cpu} scaling_min_freq must be restored before lowering max"
        );
        assert_eq!(
            sysfs.inner.get_file(cf.join("scaling_max_freq")).as_deref(),
            Some("2000000"),
            "V2-P2-07: {cpu} scaling_max_freq must be restored without EINVAL"
        );
        assert_eq!(
            sysfs.inner.get_file(cf.join("scaling_governor")).as_deref(),
            Some("powersave"),
            "V2-P2-07: {cpu} governor must be restored"
        );
    }
    assert_eq!(
        sysfs
            .inner
            .get_file(
                tmp.path()
                    .join("sys/devices/system/cpu/cpu1/cpufreq/energy_performance_preference")
            )
            .as_deref(),
        Some("balance_performance"),
        "V2-P2-07: cpu1 EPP must still be restored even after cpu0 EPP returned EBUSY"
    );
}
