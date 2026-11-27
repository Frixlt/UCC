// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ucc_config::{
    create_default_settings, default_high_performance_profile, default_quiet_profile,
    ConfigManager,
};
use ucc_dbus::{AllowAllAuthorizer, UccdBackend, UccdLegacyProxy, UccdProxy, DBUS_PATH, DBUS_PATH_ALT};
use ucc_hardware::{
    decode_history_binary, default_process_runner, DeviceCapabilities, DeviceVendor, MetricId,
    MockPlatformBackend, MockSysfs, NvidiaPowerGuard, NvmlWrapper, PlatformBackend,
    PlatformBackendKind, PlatformCapabilities, PlatformError, PlatformWrite, PowerDeviceController,
    TelemetryConfig,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{
    apply_wakeup_policy, is_allowed_wakeup_device, DaemonRuntime, LidAction, LidPolicyConfig,
    LiveUccdBackend, UccdDependencies, WakeupApplyResult, WakeupPolicy,
};
use zbus::export::futures_core::Stream;
use zbus::zvariant::OwnedValue;

type CustomBackendParts = (
    Arc<LiveUccdBackend>,
    Arc<MockPlatformBackend>,
    Arc<MockSysfs>,
    Arc<MockLightingTransport>,
);

fn build_custom_backend(
    tmp: &Path,
    explicit_intel_path: bool,
    prepare_sysfs: impl FnOnce(&MockSysfs),
    prepare_platform: impl FnOnce(Arc<MockPlatformBackend>) -> Arc<dyn PlatformBackend>,
) -> CustomBackendParts {
    build_custom_backend_with_sysfs(tmp, explicit_intel_path, None, prepare_sysfs, prepare_platform)
}

fn build_custom_backend_with_sysfs(
    tmp: &Path,
    explicit_intel_path: bool,
    shared_sysfs: Option<Arc<MockSysfs>>,
    prepare_sysfs: impl FnOnce(&MockSysfs),
    prepare_platform: impl FnOnce(Arc<MockPlatformBackend>) -> Arc<dyn PlatformBackend>,
) -> CustomBackendParts {
    let sysfs = shared_sysfs.unwrap_or_else(|| Arc::new(MockSysfs::new()));
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/cpu1/online"), "1");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "58000");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");

    let card = tmp.join("sys/class/drm/card0");
    let rapl = tmp.join("sys/class/powercap/intel-rapl:0:0");
    sysfs.set_file(card.join("gt_act_freq_mhz"), "950");
    sysfs.set_file(card.join("gt_RP0_freq_mhz"), "2200");
    sysfs.set_file(card.join("device/hwmon/hwmon1/temp1_input"), "49000");
    sysfs.set_file(rapl.join("name"), "uncore");
    sysfs.set_file(rapl.join("energy_uj"), "2000000");

    prepare_sysfs(&sysfs);

    let caps = PlatformCapabilities {
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
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_fan_temp(0, 62);
    platform.set_fan_temp(1, 51);
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

    let nvml_library = tmp.join("fixture_nvml.so");
    let nvml = nvml_library.is_file().then(|| {
        Arc::new(
            NvmlWrapper::with_library_path(
                NvidiaPowerGuard::new(tmp.join("sys/bus/pci/devices")),
                &nvml_library,
            )
            .unwrap(),
        )
    });

    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: prepare_platform(platform.clone()),
        sysfs: sysfs.clone(),
        lighting_transport: lighting.clone(),
        config_manager: ConfigManager::with_custom_path(config_path),
        sys_root: tmp.to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp.to_path_buf(),
            intel_card_path: explicit_intel_path.then_some(card),
            nvml,
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

    (backend, platform, sysfs, lighting)
}

fn build_test_backend(
    tmp: &Path,
) -> (
    Arc<LiveUccdBackend>,
    Arc<MockPlatformBackend>,
    Arc<MockSysfs>,
    Arc<MockLightingTransport>,
) {
    build_custom_backend(tmp, true, |_| {}, |p| p)
}

async fn session_conn() -> zbus::Connection {
    zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap()
}

async fn start_runtime(
    backend: Arc<LiveUccdBackend>,
    name: &str,
    workers: bool,
) -> DaemonRuntime {
    DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        backend,
        Arc::new(AllowAllAuthorizer),
        workers,
    )
    .await
    .unwrap()
}

async fn uccd_proxy<'a>(conn: &'a zbus::Connection, name: &'a str) -> UccdProxy<'a> {
    UccdProxy::builder(conn)
        .destination(name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap()
}

fn set_battery_files(sysfs: &MockSysfs, root: &Path) {
    sysfs.set_file(root.join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(root.join("sys/class/power_supply/BAT0/type"), "Battery");
}

fn write_distinct_power_profiles(root: &Path) {
    let mut settings = create_default_settings();
    settings.set_active_profile_for_mode("power_ac", default_high_performance_profile().id);
    settings.set_active_profile_for_mode("power_bat", default_quiet_profile().id);
    ConfigManager::with_custom_path(root.join("settings.json"))
        .save(&settings)
        .unwrap();
}

fn active_profile_id(profile_json: &str) -> String {
    let profile: serde_json::Value = serde_json::from_str(profile_json).unwrap();
    profile["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn daemon_exposes_live_telemetry_fans_lighting_and_restores_auto_on_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, platform, _sysfs, _lighting) = build_test_backend(tmp.path());

    let bus_name = "org.test.uccd.m8.live";
    let server_conn = session_conn().await;
    let client_conn = session_conn().await;

    let runtime = DaemonRuntime::start_on_connection(
        server_conn,
        bus_name,
        Arc::clone(&backend),
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .unwrap();

    let proxy = uccd_proxy(&client_conn, bus_name).await;
    let legacy_proxy = UccdLegacyProxy::builder(&client_conn)
        .destination(bus_name)
        .unwrap()
        .path(DBUS_PATH_ALT)
        .unwrap()
        .build()
        .await
        .unwrap();

    // Device identity & telemetry endpoints serve real data immediately at readiness
    assert_eq!(proxy.get_device_name().await.unwrap(), "Test Laptop");
    assert_eq!(proxy.get_cpu_frequency_mhz().await.unwrap(), 3200);

    let igpu: serde_json::Value =
        serde_json::from_str(&proxy.get_igpu_info_values_json().await.unwrap()).unwrap();
    assert_eq!(igpu["vendor"], "intel");
    assert_eq!(igpu["temp"], 49.0);
    assert_eq!(igpu["coreFrequency"], 950.0);

    let dgpu: serde_json::Value =
        serde_json::from_str(&proxy.get_dgpu_info_values_json().await.unwrap()).unwrap();
    assert_eq!(dgpu["perfLimitReason"], "");
    assert_eq!(dgpu["grClockOffsetMHz"], -999);

    // History codec round-trip via D-Bus GetMonitorDataSince
    proxy.set_monitor_history_horizon(120).await.unwrap();
    assert_eq!(proxy.get_monitor_history_horizon().await.unwrap(), 120);
    let history_bytes = proxy.get_monitor_data_since(0).await.unwrap();
    let decoded = decode_history_binary(&history_bytes).unwrap();
    assert!(decoded
        .iter()
        .any(|(id, pts)| *id == MetricId::CpuTemp && !pts.is_empty()));

    // Profile validation & nested fan data via legacy interface alias (`M8-REVIEW-14`)
    let prof_json = serde_json::to_string(&default_high_performance_profile()).unwrap();
    assert!(proxy.apply_profile(&prof_json).await.unwrap());
    let fan_cpu = legacy_proxy.get_fan_data_cpu().await.unwrap();
    for key in ["temp", "speed"] {
        let nested: HashMap<String, OwnedValue> =
            fan_cpu[key].try_clone().unwrap().try_into().unwrap();
        assert!(i64::try_from(&nested["timestamp"]).unwrap() > 0);
        assert!(i32::try_from(&nested["data"]).unwrap() >= 0);
    }

    // Keyboard & lightbar controllers wired to MockLightingTransport
    let kbd_states = json!([{ "brightness": 100, "red": 10, "green": 20, "blue": 255 }]).to_string();
    assert!(proxy
        .set_keyboard_backlight_states_json(&kbd_states)
        .await
        .unwrap());
    let lb_cfg = json!({
        "managed": true,
        "power_settings": { "ac": { "enabled": true, "brightness_val": 200 } }
    })
    .to_string();
    assert!(proxy.set_lightbar_settings_json(&lb_cfg).await.unwrap());

    // Per-sender power request isolation (via backend) & cooler mode toggle
    assert!(backend.register_power_request_with_sender(":1.999910", "game", "fps", "ac"));
    assert!(!backend.unregister_power_request_with_sender(":1.999999", "game"));
    assert!(backend.unregister_power_request_with_sender(":1.999910", "game"));
    assert!(proxy.set_water_cooler_auto_control(false).await.unwrap());
    assert!(!proxy.is_water_cooler_auto_control_enabled().await.unwrap());

    // Bounded shutdown restores firmware automatic fan control (`M8.3`)
    platform.clear_journal();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        platform.journal().contains(&PlatformWrite::SetFansAuto),
        "shutdown did not restore automatic fan control: {:?}",
        platform.journal()
    );
}

#[test]
fn backend_recovers_corrupt_config_and_enforces_lid_and_wakeup_policies() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("settings.json");
    let bak_path = tmp.path().join("settings.json.bak");

    std::fs::write(&cfg_path, "{ corrupt json !!!").unwrap();
    std::fs::write(
        &bak_path,
        json!({
            "stateMap": { "power_ac": "default_quiet" },
            "chargeStartThreshold": 20,
            "chargeEndThreshold": 80
        })
        .to_string(),
    )
    .unwrap();

    let (backend, _platform, sysfs, _lighting) = build_test_backend(tmp.path());
    assert!(backend.config_recovered_at_startup());
    let recovered_settings: serde_json::Value =
        serde_json::from_str(&backend.get_settings_json()).unwrap();
    assert_eq!(recovered_settings["chargeStartThreshold"], 20);
    assert_eq!(recovered_settings["chargeEndThreshold"], 80);
    // Without battery threshold sysfs nodes, hardware getters return -1 (`M8-REVIEW-12`)
    assert_eq!(backend.get_charge_start_threshold(), -1);
    assert_eq!(backend.get_charge_end_threshold(), -1);

    // Lid AC-unplug debounce (M8.9)
    let mut lid = backend.lid_controller().lock().unwrap();
    let t0 = Instant::now();
    assert_eq!(lid.evaluate_lid_event(t0, true, true), LidAction::Ignore);
    assert_eq!(
        lid.evaluate_lid_event(t0 + Duration::from_secs(1), false, true),
        LidAction::Ignore
    );
    assert_eq!(
        lid.evaluate_lid_event(t0 + Duration::from_secs(4), false, true),
        LidAction::Suspend
    );

    // WakeupPolicy rejects path traversal and enforces lid+keyboard allowlist + /proc/acpi/wakeup tokens (`M8-REVIEW-20`)
    let policy = WakeupPolicy::new(tmp.path());
    assert!(policy
        .set_device_wakeup(
            sysfs.as_ref(),
            "/sys/devices/pci0000:00/../evil/power/wakeup",
            false,
        )
        .is_err());

    let dev_root = tmp.path().join("sys/devices");
    let lid_dir = dev_root.join("platform/PNP0C0D:00");
    let kbd_dir = dev_root.join("platform/i8042/serio0");
    let mouse_dir = dev_root.join("platform/i8042/serio1");
    let usb_dir = dev_root.join("pci0000:00/0000:00:14.0");

    sysfs.set_file(lid_dir.join("power/wakeup"), "disabled");
    sysfs.set_file(kbd_dir.join("description"), "i8042 KBD port");
    sysfs.set_file(kbd_dir.join("power/wakeup"), "disabled");
    sysfs.set_file(mouse_dir.join("description"), "i8042 AUX port");
    sysfs.set_file(mouse_dir.join("power/wakeup"), "enabled");
    sysfs.set_file(usb_dir.join("power/wakeup"), "enabled");

    assert!(is_allowed_wakeup_device(&lid_dir, &dev_root, sysfs.as_ref()));
    assert!(is_allowed_wakeup_device(&kbd_dir, &dev_root, sysfs.as_ref()));
    assert!(!is_allowed_wakeup_device(&mouse_dir, &dev_root, sysfs.as_ref()));
    assert!(!is_allowed_wakeup_device(&usb_dir, &dev_root, sysfs.as_ref()));

    let acpi_wakeup = tmp.path().join("proc/acpi/wakeup");
    sysfs.set_file(
        &acpi_wakeup,
        "Device\tS-state\tStatus   Sysfs node\nLID0\t  S4\t*enabled   platform:PNP0C0D:00\nXHCI\t  S3\t*enabled   pci:0000:00:14.0\n",
    );

    sysfs.clear_journal();
    let res: WakeupApplyResult = apply_wakeup_policy(
        true,
        &tmp.path().join("sys"),
        &tmp.path().join("proc"),
        sysfs.as_ref(),
    );
    assert_eq!(res, WakeupApplyResult { enabled: 2, disabled: 4, failed: 0 });
    assert_eq!(
        sysfs.get_file(lid_dir.join("power/wakeup")).as_deref(),
        Some("enabled\n")
    );
    assert_eq!(
        sysfs.get_file(kbd_dir.join("power/wakeup")).as_deref(),
        Some("enabled\n")
    );
    assert_eq!(
        sysfs.get_file(mouse_dir.join("power/wakeup")).as_deref(),
        Some("disabled\n")
    );
    assert_eq!(
        sysfs.get_file(usb_dir.join("power/wakeup")).as_deref(),
        Some("disabled\n")
    );
    let acpi_writes: Vec<String> = sysfs
        .journal()
        .into_iter()
        .filter(|w| w.path == acpi_wakeup)
        .map(|w| w.content)
        .collect();
    assert_eq!(acpi_writes, vec!["LID0\n".to_string(), "XHCI\n".to_string()]);
}

#[tokio::test]
async fn m8_review_exclusive_ownership_never_writes_hardware() {
    for replaceable in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (backend, platform, _, _) = build_test_backend(root.path());
        let name = if replaceable {
            "org.test.uccd.integ.owner_repl"
        } else {
            "org.test.uccd.integ.owner_strict"
        };
        let owner = session_conn().await;
        if replaceable {
            owner.request_name(name).await.unwrap();
        } else {
            owner
                .request_name_with_flags(name, zbus::fdo::RequestNameFlags::DoNotQueue.into())
                .await
                .unwrap();
        }
        let candidate = session_conn().await;
        let result = DaemonRuntime::start_on_connection(
            candidate.clone(),
            name,
            backend,
            Arc::new(AllowAllAuthorizer),
            true,
        )
        .await;
        assert!(result.is_err());
        let bus = zbus::fdo::DBusProxy::new(&candidate).await.unwrap();
        let actual = bus.get_name_owner(name.try_into().unwrap()).await.unwrap();
        assert_eq!(&actual, owner.unique_name().unwrap());
        assert!(platform.journal().is_empty());
    }
}

struct SlowAutoPlatform(Arc<MockPlatformBackend>);
impl PlatformBackend for SlowAutoPlatform {
    fn kind(&self) -> PlatformBackendKind {
        self.0.kind()
    }
    fn is_available(&self) -> bool {
        self.0.is_available()
    }
    fn capabilities(&self) -> PlatformCapabilities {
        self.0.capabilities()
    }
    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        self.0.read_fan_temp(fan_idx)
    }
    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        self.0.read_fan_speed_percent(fan_idx)
    }
    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        self.0.set_fan_speed_percent(fan_idx, percent)
    }
    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        std::thread::sleep(Duration::from_millis(210));
        self.0.set_fans_auto()
    }
    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        self.0.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.0.read_tdp_limits(idx)
    }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        self.0.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.0.read_perf_profile()
    }

    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        self.0.set_perf_profile(value)
    }
    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
        self.0.set_mode_enable(value)
    }
    fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
        self.0.set_mode(value)
    }
}

#[tokio::test]
async fn m8_review_shutdown_budget_covers_slow_firmware_auto() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        Arc::new(SlowAutoPlatform(p))
    });
    let runtime = start_runtime(backend, "org.test.uccd.integ.deadline", false).await;
    let started = Instant::now();
    runtime.shutdown(Duration::from_millis(20)).await;
    assert!(
        started.elapsed() < Duration::from_millis(160),
        "20 ms deadline exceeded: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn m8_review_power_state_profile_reconciliation_cpu_tdp_and_devices() {
    // Startup on battery picks power_bat profile
    let bat_root = tempfile::tempdir().unwrap();
    write_distinct_power_profiles(bat_root.path());
    let (bat_backend, _, _, _) = build_custom_backend(
        bat_root.path(),
        true,
        |sysfs| set_battery_files(sysfs, bat_root.path()),
        |p| p,
    );
    let bat_rt = start_runtime(bat_backend, "org.test.uccd.integ.bat_init", false).await;
    let conn = session_conn().await;
    let bat_proxy = uccd_proxy(&conn, "org.test.uccd.integ.bat_init").await;
    assert_eq!(bat_proxy.get_power_state().await.unwrap(), "power_bat");
    assert_eq!(
        active_profile_id(&bat_proxy.get_active_profile_json().await.unwrap()),
        default_quiet_profile().id
    );
    bat_rt.shutdown(Duration::from_secs(1)).await;

    // Runtime transition + lightbar mode + PowerDeviceController::apply + CPU governor/EPP + TDP atomic reject
    let root = tempfile::tempdir().unwrap();
    write_distinct_power_profiles(root.path());
    let aspm = root.path().join("sys/module/pcie_aspm/parameters/policy");
    std::fs::create_dir_all(aspm.parent().unwrap()).unwrap();
    std::fs::write(&aspm, "performance").unwrap();

    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            let cpufreq = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
            s.set_file(cpufreq.join("scaling_governor"), "powersave");
            s.set_file(cpufreq.join("energy_performance_preference"), "balance_power");
            s.set_file(cpufreq.join("scaling_available_governors"), "performance powersave");
            s.set_file(
                cpufreq.join("energy_performance_available_preferences"),
                "performance balance_power",
            );
        },
        |p| p,
    );

    let name = "org.test.uccd.integ.power_cpu_tdp";
    let rt = start_runtime(backend, name, true).await;
    let proxy = uccd_proxy(&conn, name).await;

    // Out-of-range PL2 rejects entire profile with ZERO TDP writes
    let mut bad_tdp = default_high_performance_profile();
    bad_tdp.id = "review-bad-tdp".into();
    bad_tdp.odm_profile_name = None;
    bad_tdp.odm_power_limits = vec![45, 125, 130];
    platform.clear_journal();
    assert!(!proxy
        .apply_profile(&serde_json::to_string(&bad_tdp).unwrap())
        .await
        .unwrap());
    assert!(platform
        .journal()
        .into_iter()
        .all(|w| !matches!(w, PlatformWrite::SetTdp { .. })));

    // Valid CPU profile updates governor & EPP via Sysfs
    let mut cpu_prof = default_high_performance_profile();
    cpu_prof.cpu.governor = "performance".into();
    cpu_prof.cpu.energy_performance_preference = "performance".into();
    assert!(proxy
        .apply_profile(&serde_json::to_string(&cpu_prof).unwrap())
        .await
        .unwrap());
    let cpufreq = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    assert_eq!(sysfs.get_file(cpufreq.join("scaling_governor")).as_deref(), Some("performance"));
    assert_eq!(
        sysfs
            .get_file(cpufreq.join("energy_performance_preference"))
            .as_deref(),
        Some("performance")
    );

    // PowerDeviceController::apply runs for configured rules
    let rules = json!({"enabled": true, "cpu": {"ac": {"aspm": "powersave"}}, "devices": []});
    assert!(proxy
        .set_power_device_settings_json(&rules.to_string())
        .await
        .unwrap());
    assert_eq!(std::fs::read_to_string(&aspm).unwrap(), "powersave");

    // Switching to battery reconciles active profile and lightbar mode
    set_battery_files(&sysfs, root.path());
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        active_profile_id(&proxy.get_active_profile_json().await.unwrap()),
        default_quiet_profile().id
    );
    let lb_status: serde_json::Value =
        serde_json::from_str(&proxy.get_lightbar_status_json().await.unwrap()).unwrap();
    assert_eq!(lb_status["mode"], "bat");

    rt.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn m8_review_persistence_statemap_fan_profiles_signals_and_hardware_probing() {
    // Persistence rollback on unreadable destination
    let unwritable = tempfile::tempdir().unwrap();
    std::fs::create_dir(unwritable.path().join("settings.json")).unwrap();
    let (u_backend, _, _, _) = build_test_backend(unwritable.path());
    let u_name = "org.test.uccd.integ.unwritable";
    let u_rt = start_runtime(u_backend, u_name, false).await;
    let conn = session_conn().await;
    let u_proxy = uccd_proxy(&conn, u_name).await;
    let mut unsavable = default_high_performance_profile();
    unsavable.id = "review-unsavable".into();
    assert!(!u_proxy
        .save_custom_profile(&serde_json::to_string(&unsavable).unwrap())
        .await
        .unwrap());
    assert!(!u_proxy
        .get_profiles_json()
        .await
        .unwrap()
        .contains("review-unsavable"));
    u_rt.shutdown(Duration::from_secs(1)).await;

    // Normal runtime: StateMap validation, fan payloads, honest display/charging, signals, iGPU autodiscovery, customFanProfiles persistence
    let root = tempfile::tempdir().unwrap();
    write_distinct_power_profiles(root.path());
    let card_dev = root.path().join("sys/class/drm/card0/device");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        false,
        |s| {
            s.set_file(card_dev.join("vendor"), "0x8086");
            s.set_file(card_dev.join("class"), "0x030000");
        },
        |p| p,
    );
    let name = "org.test.uccd.integ.misc";
    let rt = start_runtime(backend, name, true).await;
    let proxy = uccd_proxy(&conn, name).await;

    // Intel iGPU auto-discovery without injected card path
    let igpu: serde_json::Value =
        serde_json::from_str(&proxy.get_igpu_info_values_json().await.unwrap()).unwrap();
    assert_eq!(igpu["vendor"], "intel");
    assert_eq!(igpu["coreFrequency"], 950.0);

    // StateMap rejects unknown profile IDs
    assert!(!proxy
        .set_state_map("power_ac", "missing-profile-id")
        .await
        .unwrap());

    // Without backlight/charging sysfs nodes, setters honestly fail
    assert!(!proxy.set_display_brightness(17).await.unwrap());
    assert_eq!(proxy.get_display_brightness().await.unwrap(), -1);
    assert_eq!(proxy.get_charging_profiles_available().await.unwrap(), "[]");
    assert!(!proxy
        .set_charging_profile("not-a-firmware-profile")
        .await
        .unwrap());
    assert!(!proxy.get_fn_lock_supported().await.unwrap());
    assert!(!proxy.set_webcam(false).await.unwrap());

    // Canonical ApplyFanProfiles payload
    let curves = json!({
        "cpu": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}],
        "gpu": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}]
    });
    assert!(proxy.apply_fan_profiles(&curves.to_string()).await.unwrap());

    // Signal emission on SetActiveProfile
    let mut signals = std::pin::pin!(proxy.receive_profile_changed().await.unwrap());
    assert!(proxy
        .set_active_profile(&default_quiet_profile().id)
        .await
        .unwrap());
    let sig = tokio::time::timeout(
        Duration::from_millis(400),
        std::future::poll_fn(|cx| signals.as_mut().poll_next(cx)),
    )
    .await;
    assert!(matches!(sig, Ok(Some(_))));

    // SaveCustomFanProfile persists across restart with {id, name, json}
    let id = "review-fan-id";
    let label = "Review fan curve";
    let fan_blob = json!({
        "id": id, "name": label,
        "tableCPU": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}]
    })
    .to_string();
    assert!(proxy
        .save_custom_fan_profile(id, label, &fan_blob)
        .await
        .unwrap());

    // Sensor collection off stops periodic CpuTemp history sampling
    proxy.set_sensor_data_collection_status(false).await.unwrap();
    let before_pts =
        decode_history_binary(&proxy.get_monitor_data_since(0).await.unwrap())
            .unwrap()
            .into_iter()
            .find(|(m, _)| *m == MetricId::CpuTemp)
            .map(|(_, v)| v.len())
            .unwrap_or(0);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let after_pts =
        decode_history_binary(&proxy.get_monitor_data_since(0).await.unwrap())
            .unwrap()
            .into_iter()
            .find(|(m, _)| *m == MetricId::CpuTemp)
            .map(|(_, v)| v.len())
            .unwrap_or(0);
    assert_eq!(before_pts, after_pts);

    // Lightbar with positive charging & backlight sysfs checks (`M8-REVIEW-11`, `M8-REVIEW-12`)
    drop(proxy);
    rt.shutdown(Duration::from_secs(1)).await;

    // Positive charging & backlight probing with mocked sysfs
    let bl = root.path().join("sys/class/backlight/intel_backlight");
    sysfs.set_file(bl.join("max_brightness"), "200");
    sysfs.set_file(bl.join("brightness"), "100");
    let cp_dir = root.path().join("sys/devices/platform/tuxedo_keyboard/charging_profile");
    sysfs.set_file(cp_dir.join("charging_profiles_available"), "high_capacity balanced stationary");
    sysfs.set_file(cp_dir.join("charging_profile"), "balanced");

    // Restart and confirm persisted customFanProfiles survived + positive backlight/charging sysfs writes
    let (restarted_backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(bl.join("max_brightness"), "200");
            s.set_file(bl.join("brightness"), "100");
            s.set_file(
                cp_dir.join("charging_profiles_available"),
                "high_capacity balanced stationary",
            );
            s.set_file(cp_dir.join("charging_profile"), "balanced");
        },
        |p| p,
    );
    let rt2 = start_runtime(restarted_backend, name, false).await;
    let proxy2 = uccd_proxy(&conn, name).await;
    let loaded_fans: serde_json::Value =
        serde_json::from_str(&proxy2.get_custom_fan_profiles_json().await.unwrap()).unwrap();
    assert!(loaded_fans
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == id && p["name"] == label && p["json"].is_object()));
    assert!(proxy2.set_display_brightness(50).await.unwrap());
    assert_eq!(proxy2.get_display_brightness().await.unwrap(), 50);
    assert!(proxy2.set_charging_profile("stationary").await.unwrap());
    assert_eq!(proxy2.get_current_charging_profile().await.unwrap(), "stationary");
    rt2.shutdown(Duration::from_secs(1)).await;
    let _ = sysfs;
}

#[tokio::test]
async fn m8_review_disconnected_client_power_request_is_evicted() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| set_battery_files(sysfs, root.path()),
        |p| p,
    );
    let name = "org.test.uccd.integ.noc";
    let runtime = start_runtime(backend, name, true).await;
    let observer = session_conn().await;
    let client = uccd_proxy(&observer, name).await;
    assert_eq!(client.get_power_state().await.unwrap(), "power_bat");

    let app = session_conn().await;
    let app_client = uccd_proxy(&app, name).await;
    assert!(app_client
        .register_power_request("test-app", "integ fixture", "ac")
        .await
        .unwrap());
    assert_eq!(client.get_power_state().await.unwrap(), "power_ac");

    drop(app_client);
    app.close().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.get_power_state().await.unwrap(), "power_bat");
    runtime.shutdown(Duration::from_secs(1)).await;
}

type RecheckStartupGate = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);

struct RecheckAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    startup_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    slow_auto: bool,
    tdp_fault: bool,
}

impl RecheckAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            startup_gate: std::sync::Mutex::new(None),
            slow_auto: false,
            tdp_fault: false,
        }
    }
}

impl PlatformBackend for RecheckAuditPlatform {
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
        if let Some((started, release)) = self.startup_gate.lock().unwrap().take() {
            let _ = started.send(());
            let _ = release.recv_timeout(Duration::from_secs(4));
        }
        self.inner.set_fan_speed_percent(fan_idx, percent)
    }
    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        if self.slow_auto {
            std::thread::sleep(Duration::from_millis(320));
        }
        self.inner.set_fans_auto()
    }
    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        if self.tdp_fault && idx == 0 {
            return Err(PlatformError::Unavailable);
        }
        self.inner.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(idx)
    }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        if self.tdp_fault && idx == 1 {
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
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
async fn m8_recheck_bus_readiness_waits_for_the_initial_telemetry() {
    let root = tempfile::tempdir().unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = RecheckAuditPlatform::new(p);
            *wrapper.startup_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.edge.start";
    let task = tokio::spawn(start_runtime(backend, name, false));
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    let observer = session_conn().await;
    let bus = zbus::fdo::DBusProxy::new(&observer).await.unwrap();
    let has_name = bus.name_has_owner(name.try_into().unwrap()).await.unwrap();
    let early_frequency = if has_name {
        let proxy = uccd_proxy(&observer, name).await;
        tokio::time::timeout(Duration::from_millis(350), proxy.get_cpu_frequency_mhz())
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    release.send(()).unwrap();
    let running = task.await.unwrap();
    let ready_frequency = uccd_proxy(&observer, name)
        .await
        .get_cpu_frequency_mhz()
        .await
        .unwrap();
    running.shutdown(Duration::from_secs(1)).await;
    assert_eq!(ready_frequency, 3200);
    assert!(
        !has_name || early_frequency.is_none() || early_frequency == Some(ready_frequency),
        "name advertised during activation, before={early_frequency:?}, ready={ready_frequency}"
    );
}

#[test]
fn m8_recheck_runtime_shutdown_deadline_also_caps_tokio_runtime_exit() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = RecheckAuditPlatform::new(p);
            wrapper.slow_auto = true;
            Arc::new(wrapper)
        },
    );
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let runtime =
        tokio_runtime.block_on(start_runtime(backend, "org.test.uccd.edge.exit", false));
    let started = Instant::now();
    tokio_runtime.block_on(runtime.shutdown(Duration::from_millis(25)));
    let returned = started.elapsed();
    drop(tokio_runtime);
    let exited = started.elapsed();
    assert!(
        exited < Duration::from_millis(175),
        "25 ms budget: shutdown future returned in {returned:?}, Tokio runtime exited after {exited:?}"
    );
}

#[tokio::test]
async fn m8_recheck_shutdown_unexports_api_and_releases_the_name_even_after_timeout() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = RecheckAuditPlatform::new(p);
            wrapper.slow_auto = true;
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.edge.shutdown_name";
    let runtime = start_runtime(backend, name, false).await;
    let connection_for_other_services = runtime.connection().clone();
    let unique = connection_for_other_services
        .unique_name()
        .unwrap()
        .to_string();
    runtime.shutdown(Duration::from_millis(25)).await;
    let observer = session_conn().await;
    let bus = zbus::fdo::DBusProxy::new(&observer).await.unwrap();
    let advertised = bus.name_has_owner(name.try_into().unwrap()).await.unwrap();
    let after = uccd_proxy(&observer, &unique).await.get_device_name().await;
    assert!(
        !advertised && after.is_err(),
        "after shutdown, owner advertised={advertised}, direct API call={after:?}"
    );
}

#[tokio::test]
async fn m8_recheck_invalid_second_tdp_write_cannot_leave_first_channel_changed() {
    let root = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            p.set_tdp_value(0, 60);
            p.set_tdp_value(1, 85);
            p.set_tdp_value(2, 105);
            let mut wrapper = RecheckAuditPlatform::new(p);
            wrapper.tdp_fault = true;
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.edge.tdp_readback";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let mut profile = default_quiet_profile();
    profile.odm_profile_name = None;
    profile.odm_power_limits = vec![42, 75, 90];
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let actual = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        actual, 60,
        "first channel changed without a readable rollback value"
    );
}

#[tokio::test]
async fn m8_recheck_cpu_io_failure_rolls_back_tdp_and_earlier_cpu_writes() {
    let root = tempfile::tempdir().unwrap();
    let cpu = root.path().join("sys/devices/system/cpu");
    let no_turbo = cpu.join("intel_pstate/no_turbo");
    let epp = cpu.join("cpu0/cpufreq/energy_performance_preference");
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&no_turbo, "0");
            sysfs.set_file(&epp, "balance_power");
            sysfs.set_file(
                cpu.join("cpu0/cpufreq/energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.cpu_fault";
    let runtime = start_runtime(backend, name, false).await;
    for (i, power) in [60, 85, 105].into_iter().enumerate() {
        platform.set_tdp_value(i, power);
    }
    let before = sysfs.get_file(&no_turbo).unwrap();
    assert_eq!(before, "0");
    sysfs.set_write_error(&epp);
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let mut profile = default_quiet_profile();
    profile.odm_profile_name = None;
    profile.odm_power_limits = vec![42, 70, 90];
    profile.cpu.energy_performance_preference = "performance".into();
    profile.cpu.no_turbo = true;
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let after_tdp = (0..3)
        .map(|i| platform.read_tdp(i).unwrap())
        .collect::<Vec<_>>();
    let after_turbo = sysfs.get_file(&no_turbo).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!((after_tdp, after_turbo), (vec![60, 85, 105], before));
}

#[tokio::test]
async fn m8_recheck_shutdown_restores_each_cpu_policy_to_its_own_startup_state() {
    let root = tempfile::tempdir().unwrap();
    let cpus = root.path().join("sys/devices/system/cpu");
    let gov0 = cpus.join("cpu0/cpufreq/scaling_governor");
    let gov1 = cpus.join("cpu1/cpufreq/scaling_governor");
    write_distinct_power_profiles(root.path());
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&gov0, "powersave");
            sysfs.set_file(&gov1, "schedutil");
            for i in 0..2 {
                sysfs.set_file(
                    cpus.join(format!("cpu{i}/cpufreq/scaling_available_governors")),
                    "performance powersave schedutil",
                );
            }
        },
        |p| p,
    );
    let runtime = start_runtime(backend, "org.test.uccd.edge.restore_cpu", false).await;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(sysfs.get_file(&gov0).as_deref(), Some("powersave"));
    assert_eq!(sysfs.get_file(&gov1).as_deref(), Some("schedutil"));
}

#[tokio::test]
async fn m8_recheck_battery_profile_is_retried_after_a_transient_apply_error() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = create_default_settings();
    let ac = default_high_performance_profile();
    let mut bat = default_quiet_profile();
    bat.odm_profile_name = None;
    bat.odm_power_limits = vec![45, 125, 130];
    settings.profiles.insert(bat.id.clone(), bat.clone());
    settings.set_active_profile_for_mode("power_ac", &ac.id);
    settings.set_active_profile_for_mode("power_bat", &bat.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();
    let (backend, platform, sysfs, _) = build_test_backend(root.path());
    let name = "org.test.uccd.edge.profile_retry";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    set_battery_files(&sysfs, root.path());
    backend.reconcile_power_state(false);
    assert_eq!(
        active_profile_id(&proxy.get_active_profile_json().await.unwrap()),
        ac.id
    );
    platform.set_tdp_limits(1, (20, 130));
    backend.reconcile_power_state(false);
    let actual = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        actual, bat.id,
        "profile stayed stale after the hardware recovered"
    );
}

#[tokio::test]
async fn m8_recheck_saving_active_profile_fails_without_changing_live_profile_if_disk_write_fails() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("settings.json")).unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let name = "org.test.uccd.edge.activate_persist";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let before = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let quiet = default_quiet_profile();
    assert_ne!(before, quiet.id);
    let accepted = proxy.set_active_profile(&quiet.id).await.unwrap();
    let after = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        after, before,
        "D-Bus reported failure but activated {after}"
    );
}

#[tokio::test]
async fn m8_recheck_charging_threshold_failure_does_not_leave_hardware_changed() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("settings.json")).unwrap();
    let battery = root.path().join("sys/class/power_supply/BAT0");
    let start = battery.join("charge_control_start_threshold");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(battery.join("type"), "Battery");
            sysfs.set_file(&start, "30");
            sysfs.set_file(battery.join("charge_control_end_threshold"), "80");
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.charge_store";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let accepted = proxy.set_charge_start_threshold(45).await.unwrap();
    let actual = sysfs.get_file(&start);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        actual.as_deref(),
        Some("30"),
        "D-Bus returned failure but the battery threshold changed"
    );
}

#[tokio::test]
async fn m8_recheck_changing_power_device_cpu_policy_reapplies_the_current_profile() {
    let root = tempfile::tempdir().unwrap();
    let governor = root
        .path()
        .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&governor, "performance");
            sysfs.set_file(
                governor.with_file_name("scaling_available_governors"),
                "performance powersave schedutil",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.cpu_overlay";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy
        .set_power_device_settings_json(
            &json!({
                "enabled":true, "cpu": {"ac": {"governor":"schedutil"}}, "devices":[]
            })
            .to_string()
        )
        .await
        .unwrap());
    backend.reconcile_power_state(false);
    let actual = sysfs.get_file(&governor);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(actual.as_deref(), Some("schedutil"));
}

#[tokio::test]
async fn m8_recheck_releasing_a_well_known_name_keeps_power_requests_for_the_connected_client() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            set_battery_files(sysfs, root.path());
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.app_name";
    let runtime = start_runtime(backend, name, false).await;
    let app = session_conn().await;
    let ephemeral_name = "org.test.uccd.edge.client";
    app.request_name(ephemeral_name).await.unwrap();
    let proxy = uccd_proxy(&app, name).await;
    assert!(proxy
        .register_power_request("game", "test", "ac")
        .await
        .unwrap());
    assert_eq!(proxy.get_power_state().await.unwrap(), "power_ac");
    app.release_name(ephemeral_name).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let bus = zbus::fdo::DBusProxy::new(&app).await.unwrap();
    let connected = bus
        .name_has_owner(app.unique_name().unwrap().as_str().try_into().unwrap())
        .await
        .unwrap();
    let actual = proxy.get_power_state().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(connected);
    assert_eq!(
        actual, "power_ac",
        "still-connected sender lost its request after releasing a secondary name"
    );
}

#[tokio::test]
async fn m8_recheck_legacy_usb_c_sources_take_priority_when_mains_is_also_online() {
    let mut states = Vec::new();
    for supply_kind in ["USB", "USB_PD"] {
        let root = tempfile::tempdir().unwrap();
        let (backend, _, _, _) = build_custom_backend(
            root.path(),
            true,
            |sysfs| {
                let usb = root.path().join("sys/class/power_supply/ucsi-source-psy");
                sysfs.set_file(usb.join("type"), supply_kind);
                sysfs.set_file(usb.join("online"), "1");
            },
            |p| p,
        );
        let state = backend.get_power_state();
        states.push(state);
    }
    assert_eq!(
        states,
        ["power_type_c", "power_type_c"],
        "USB and USB_PD with Mains also online"
    );
}

#[tokio::test]
async fn m8_recheck_intel_igpu_discovery_does_not_pick_the_first_intel_discrete_card() {
    let root = tempfile::tempdir().unwrap();
    let drm = root.path().join("sys/class/drm");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        false,
        |sysfs| {
            for (card, device, clock) in [("card0", "56a0", "650"), ("card1", "46a6", "950")] {
                let dir = drm.join(card);
                sysfs.set_file(dir.join("device/vendor"), "0x8086");
                sysfs.set_file(dir.join("device/class"), "0x030000");
                sysfs.set_file(dir.join("device/device"), format!("0x{device}"));
                sysfs.set_file(
                    dir.join("device/uevent"),
                    format!("PCI_ID=8086:{}\n", device.to_uppercase()),
                );
                sysfs.set_file(dir.join("gt_act_freq_mhz"), clock);
            }
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.igpu";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let actual: serde_json::Value =
        serde_json::from_str(&proxy.get_igpu_info_values_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        actual["coreFrequency"], 950.0,
        "discrete card reported as iGPU: {actual}"
    );
}

#[tokio::test]
async fn m8_recheck_disappearing_backlight_entry_does_not_hide_working_panel() {
    let root = tempfile::tempdir().unwrap();
    let backlight = root.path().join("sys/class/backlight");
    let brightness = backlight.join("panel1/brightness");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(backlight.join("panel0/brightness"), "5");
            sysfs.set_read_error(backlight.join("panel0/max_brightness"));
            sysfs.set_file(backlight.join("panel1/max_brightness"), "100");
            sysfs.set_file(&brightness, "55");
        },
        |p| p,
    );
    let accepted = backend.set_display_brightness(40);
    assert!(
        accepted && sysfs.get_file(brightness).as_deref() == Some("40"),
        "unreadable first entry hid the valid second entry"
    );
}

#[tokio::test]
async fn m8_recheck_supported_nvidia_offset_reaches_the_hardware_controller() {
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
    assert!(source.contains("fprintf(f"));
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
    ];
    std::fs::create_dir_all(gpu.join("power")).unwrap();
    for (path, data) in nodes {
        std::fs::write(gpu.join(path), data).unwrap();
    }
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            for (path, data) in nodes {
                sysfs.set_file(gpu.join(path), data);
            }
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.ctgp";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy.get_nvidia_power_ctrl_available().await.unwrap());
    let accepted = proxy.set_nvidia_power_offset(-5).await.unwrap();
    let write = std::fs::read_to_string(&log).ok();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        accepted && write.is_some(),
        "D-Bus accepted={accepted}, NVML write={write:?}"
    );
}

#[test]
fn m8_recheck_independent_concurrent_config_edits_survive_all_successful_saves() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("settings.json");
    let mut settings = create_default_settings();
    settings
        .extra
        .insert("audit_padding".into(), json!("x".repeat(1_048_576)));
    ConfigManager::with_custom_path(file.clone())
        .save(&settings)
        .unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let gate = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        let mut tasks = vec![];
        for i in 0..8 {
            let gate = gate.clone();
            let backend = backend.clone();
            tasks.push(scope.spawn(move || {
                let mut profile = default_quiet_profile();
                profile.id = format!("edge-saved-{i}");
                let body = serde_json::to_string(&profile).unwrap();
                gate.wait();
                backend.save_custom_profile(&body)
            }));
        }
        assert!(tasks.into_iter().all(|t| t.join().unwrap()));
    });
    let disk = ConfigManager::load_from_path(&file).unwrap();
    let on_disk = disk
        .profiles
        .keys()
        .filter(|k| k.starts_with("edge-saved-"))
        .count();
    let in_memory = (0..8)
        .filter(|i| backend.get_profiles_json().contains(&format!("edge-saved-{i}")))
        .count();
    assert_eq!(
        (on_disk, in_memory),
        (8, 8),
        "all eight updates returned success"
    );
}

#[tokio::test]
async fn m8_recheck_explicit_cpu_core_selection_is_applied_by_the_live_daemon() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, sysfs, _) = build_test_backend(root.path());
    let cpu1 = root.path().join("sys/devices/system/cpu/cpu1/online");
    let name = "org.test.uccd.edge.cpu_selection";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let mut profile = default_quiet_profile();
    profile.cpu.enabled_cores = Some(vec![0]);
    profile.cpu.online_cores = None;
    profile.odm_profile_name = None;
    profile.odm_power_limits.clear();
    profile.validate().unwrap();
    assert_eq!(sysfs.get_file(&cpu1).as_deref(), Some("1"));
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let cpu_online = sysfs.get_file(cpu1);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(accepted);
    assert_eq!(
        cpu_online.as_deref(),
        Some("0"),
        "explicit CPU set was ignored"
    );
}

#[tokio::test]
async fn m8_recheck_fan_profile_saved_with_outer_name_remains_usable_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let name = "org.test.uccd.edge.fan_save";
    let id = "fan-outer-name";
    let payload = json!({
        "id": id,
        "tableCPU": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}],
        "tableGPU": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}]
    })
    .to_string();
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy
        .save_custom_fan_profile(id, "Outer name", &payload)
        .await
        .unwrap());
    let apply_id = json!({"fanProfileId": id}).to_string();
    assert!(proxy.apply_fan_profiles(&apply_id).await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = start_runtime(backend, name, false).await;
    let proxy = uccd_proxy(&observer, name).await;
    let listed = proxy
        .get_custom_fan_profiles_json()
        .await
        .unwrap()
        .contains(id);
    let accepted = proxy.apply_fan_profiles(&apply_id).await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        listed && accepted,
        "saved curve is listed={listed}, apply by ID succeeds={accepted}"
    );
}

#[tokio::test]
async fn m8_recheck_webcam_status_delegates_to_platform_backend() {
    let root = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut caps = p.capabilities();
            caps.device.webcam_switch = true;
            p.set_capabilities(caps);
            p.set_webcam(false).unwrap();
            p
        },
    );
    let name = "org.test.uccd.edge.webcam";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy.webcam_sw_available().await.unwrap());
    assert!(proxy.set_webcam(false).await.unwrap());
    assert!(!proxy.get_webcam_sw_status().await.unwrap());
    assert!(proxy.set_webcam(true).await.unwrap());
    let status = proxy.get_webcam_sw_status().await.unwrap();
    let writes = platform.journal();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(status);
    assert!(writes.contains(&PlatformWrite::SetWebcam(false)));
    assert!(writes.contains(&PlatformWrite::SetWebcam(true)));
}

struct ProdAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    startup_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    slow_auto: bool,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl ProdAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            startup_gate: std::sync::Mutex::new(None),
            slow_auto: false,
            tdp_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for ProdAuditPlatform {
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
        if let Some((started, release)) = self.startup_gate.lock().unwrap().take() {
            let _ = started.send(());
            let _ = release.recv_timeout(Duration::from_secs(4));
        }
        self.inner.set_fan_speed_percent(fan_idx, percent)
    }
    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        if self.slow_auto {
            std::thread::sleep(Duration::from_millis(320));
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
                return Err(PlatformError::Unavailable);
            }
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
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

struct PrivatePolicyBus(std::process::Child);

impl Drop for PrivatePolicyBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn m8_prod_daemon_starts_under_the_packaged_system_bus_policy() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let root = tempfile::tempdir().unwrap();
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../uccd/com.uniwill.uccd.conf");
    let service_policy = root.path().join("service.conf");
    std::fs::write(
        &service_policy,
        std::fs::read_to_string(source)
            .unwrap()
            .replace("user=\"root\"", &format!("user=\"{}\"", uid.trim())),
    )
    .unwrap();
    let config = root.path().join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"
        <busconfig>
          <listen>unix:path={}/bus</listen>
          <auth>EXTERNAL</auth>
          <policy context="default">
            <allow user="*"/>
            <deny own="*"/>
            <allow send_destination="*"/>
            <allow receive_sender="*"/>
          </policy>
          <include>{}</include>
        </busconfig>
    "#,
            root.path().display(),
            service_policy.display()
        ),
    )
    .unwrap();
    let mut daemon = PrivatePolicyBus(
        Command::new("dbus-daemon")
            .args(["--nofork", "--print-address=1", "--config-file"])
            .arg(config)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    std::io::BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    assert!(!address.is_empty(), "private policy bus failed to start");

    let connection = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
    let name = ucc_dbus::DBUS_SERVICE;
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    let reply = connection.request_name_with_flags(name, flags).await.unwrap();
    assert_eq!(reply, zbus::fdo::RequestNameReply::PrimaryOwner);
    assert!(connection.release_name(name).await.unwrap());

    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = DaemonRuntime::start_on_connection(
        connection,
        name,
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .expect("packaged system bus policy must allow uccd to start");

    let observer = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
    let proxy = uccd_proxy(&observer, name).await;
    assert_eq!(proxy.get_device_name().await.unwrap(), "Test Laptop");
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_prod_startup_exclusive_name_blocks_rival_and_loser_never_writes_fans() {
    let root1 = tempfile::tempdir().unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (winner_backend, _, _, _) = build_custom_backend(
        root1.path(),
        true,
        |_| {},
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.startup_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.production.startup_race_exclusive";
    let first = tokio::spawn(DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        winner_backend,
        Arc::new(AllowAllAuthorizer),
        false,
    ));
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();

    // While winner is inside activate_hardware(), it ALREADY owns `name` exclusively:
    // any second candidate must fail immediately with ZERO fan writes.
    let root2 = tempfile::tempdir().unwrap();
    let (loser_backend, loser_platform, _, _) = build_test_backend(root2.path());
    let rival_err = DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        loser_backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .is_err();
    let loser_writes: Vec<_> = loser_platform
        .journal()
        .into_iter()
        .filter(|w| {
            matches!(
                w,
                PlatformWrite::SetFanSpeed { .. } | PlatformWrite::SetFansAuto
            )
        })
        .collect();
    let _ = release.send(());
    let winner = first.await.unwrap().unwrap();
    winner.shutdown(Duration::from_secs(1)).await;

    assert!(rival_err && loser_writes.is_empty());
}

#[tokio::test]
async fn m8_prod_restarted_daemon_is_not_overwritten_by_timed_out_predecessor_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let name = "org.test.uccd.production.restart";
    let (old, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = ProdAuditPlatform::new(p);
            wrapper.slow_auto = true;
            Arc::new(wrapper)
        },
    );
    let old = start_runtime(old, name, false).await;
    old.shutdown(Duration::from_millis(25)).await;
    let (replacement, _, _, _) =
        build_custom_backend(root.path(), true, |_| {}, |_| platform.clone());
    let replacement = start_runtime(replacement, name, false).await;
    platform.clear_journal();
    tokio::time::sleep(Duration::from_millis(390)).await;
    let writes = platform.journal();
    replacement.shutdown(Duration::from_secs(1)).await;
    assert!(
        !writes.contains(&PlatformWrite::SetFansAuto),
        "predecessor reset fan control after successor had acquired and primed {name}: {writes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m8_prod_failed_profile_switch_does_not_rollback_a_newer_successful_switch() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = create_default_settings();
    let mut failed = default_quiet_profile();
    failed.id = "gate-profile".into();
    failed.odm_profile_name = None;
    failed.odm_power_limits = vec![41, 70, 90];
    let mut good = failed.clone();
    good.id = "newer-profile".into();
    good.odm_power_limits = vec![55, 80, 95];
    settings.profiles.insert(failed.id.clone(), failed.clone());
    settings.profiles.insert(good.id.clone(), good.clone());
    let file = root.path().join("settings.json");
    ConfigManager::with_custom_path(file.clone())
        .save(&settings)
        .unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.production.concurrent_profile";
    let runtime = start_runtime(backend, name, false).await;
    let old = runtime.backend().clone();
    let first_call = std::thread::spawn(move || old.set_active_profile(&failed.id));
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let newer_result =
        tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&good.id)).await;
    let _ = release.send(());
    let failed_accepted = first_call.join().unwrap();
    let newer_accepted = newer_result.unwrap().unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let saved = ConfigManager::load_from_path(&file).unwrap().state_map["power_ac"].clone();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!failed_accepted && newer_accepted);
    assert_eq!(
        (active, saved),
        (good.id.clone(), good.id),
        "failed older RPC clobbered successful newer assignment"
    );
}

#[tokio::test]
async fn m8_prod_cpu_node_without_readback_is_not_modified_if_profile_later_fails() {
    let root = tempfile::tempdir().unwrap();
    let cpus = root.path().join("sys/devices/system/cpu");
    let no_turbo = cpus.join("intel_pstate/no_turbo");
    let epp = cpus.join("cpu0/cpufreq/energy_performance_preference");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&no_turbo, "0");
            sysfs.set_file(&epp, "balance_power");
            sysfs.set_file(
                epp.with_file_name("energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.production.cpu_readback";
    let runtime = start_runtime(backend, name, false).await;
    let before = sysfs.get_file(&no_turbo).unwrap();
    assert_eq!(before, "0");
    sysfs.set_read_error(&no_turbo);
    sysfs.set_write_error(&epp);
    let mut target = default_quiet_profile();
    target.odm_profile_name = None;
    target.odm_power_limits.clear();
    target.cpu.no_turbo = true;
    target.cpu.energy_performance_preference = "performance".into();
    let conn = session_conn().await;
    let accepted = uccd_proxy(&conn, name)
        .await
        .apply_profile(&serde_json::to_string(&target).unwrap())
        .await
        .unwrap();
    let after = sysfs.get_file(&no_turbo).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        after, before,
        "no_turbo was written without a readable rollback value"
    );
}

#[tokio::test]
async fn m8_prod_odm_switch_has_rollback_when_current_profile_does_not_name_odm_mode() {
    let root = tempfile::tempdir().unwrap();
    let cpufreq = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    let epp = cpufreq.join("energy_performance_preference");
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&epp, "balance_power");
            sysfs.set_file(
                epp.with_file_name("energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.production.odm_rollback";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut profile = default_quiet_profile();
    profile.odm_profile_name = None;
    profile.odm_power_limits.clear();
    profile.cpu.energy_performance_preference = "balance_power".into();
    assert!(proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap());
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();

    // Case 1: CPU failure before ODM write keeps ODM untouched.
    sysfs.set_write_error(&epp);
    profile.odm_profile_name = Some("overboost".into());
    profile.cpu.energy_performance_preference = "performance".into();
    assert!(!proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap());
    let writes_case1: Vec<_> = platform
        .journal()
        .into_iter()
        .filter(|w| matches!(w, PlatformWrite::SetPerfProfile(_)))
        .collect();
    assert!(writes_case1.is_empty());

    // Case 2: Subsequent TDP failure after ODM write rolls ODM back to hardware readback `2`.
    let root2 = tempfile::tempdir().unwrap();
    let cpufreq2 = root2.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    let epp2 = cpufreq2.join("energy_performance_preference");
    let (backend2, platform2, _, _) = build_custom_backend(
        root2.path(),
        true,
        |sysfs| {
            sysfs.set_file(&epp2, "balance_power");
            sysfs.set_file(
                epp2.with_file_name("energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name2 = "org.test.uccd.production.odm_rollback_tdp";
    let runtime2 = start_runtime(backend2, name2, false).await;
    let proxy2 = uccd_proxy(&conn, name2).await;
    let mut profile2 = default_quiet_profile();
    profile2.odm_profile_name = None;
    profile2.odm_power_limits.clear();
    profile2.cpu.energy_performance_preference = "balance_power".into();
    assert!(proxy2
        .apply_profile(&serde_json::to_string(&profile2).unwrap())
        .await
        .unwrap());
    platform2.set_perf_profile(2).unwrap();
    platform2.clear_journal();
    profile2.odm_profile_name = Some("overboost".into());
    profile2.cpu.energy_performance_preference = "performance".into();
    profile2.odm_power_limits = vec![41, 70, 90];
    let _ = release.send(());
    let accepted = proxy2
        .apply_profile(&serde_json::to_string(&profile2).unwrap())
        .await
        .unwrap();
    let _ = entered.try_recv();
    let writes_case2: Vec<_> = platform2
        .journal()
        .into_iter()
        .filter(|w| matches!(w, PlatformWrite::SetPerfProfile(_)))
        .collect();
    runtime.shutdown(Duration::from_secs(1)).await;
    runtime2.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(writes_case2.last(), Some(&PlatformWrite::SetPerfProfile(2)));
}

#[tokio::test]
async fn m8_prod_nvidia_ctgp_sysfs_fallback_is_available_without_nvml() {
    let root = tempfile::tempdir().unwrap();
    let expected = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&expected, "0");
        },
        |p| p,
    );
    let name = "org.test.uccd.production.ctgp_sysfs";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let advertised = proxy.get_nvidia_power_ctrl_available().await.unwrap();
    let accepted = proxy.set_nvidia_power_offset(-5).await.unwrap();
    let after = sysfs.get_file(&expected).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        advertised && accepted && after == "-5",
        "NVML absent, sysfs available: advertised={advertised}, accepted={accepted}, offset={after}"
    );
}

#[tokio::test]
async fn m8_prod_cpu_overlay_retries_after_transient_sysfs_write_failure() {
    let root = tempfile::tempdir().unwrap();
    let governor = root
        .path()
        .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&governor, "performance");
            sysfs.set_file(
                governor.with_file_name("scaling_available_governors"),
                "performance powersave",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.production.overlay_retry";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let available = governor.with_file_name("scaling_available_governors");
    sysfs.set_file(&available, "performance");
    let config = json!({"enabled":true, "cpu":{"ac":{"governor":"powersave"}}, "devices":[]});
    assert!(proxy
        .set_power_device_settings_json(&config.to_string())
        .await
        .unwrap());
    assert_eq!(sysfs.get_file(&governor).as_deref(), Some("performance"));
    sysfs.set_file(&available, "performance powersave");
    backend.reconcile_power_state(false);
    let actual = sysfs.get_file(&governor);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        actual.as_deref(),
        Some("powersave"),
        "a newly available overlay is never retried"
    );
}

#[tokio::test]
async fn m8_prod_charging_priority_uses_the_installed_driver_attribute_names() {
    let root = tempfile::tempdir().unwrap();
    let attrs = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_priority");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(
                attrs.join("charging_prios_available"),
                "charge_battery performance",
            );
            sysfs.set_file(attrs.join("charging_prio"), "charge_battery");
        },
        |p| p,
    );
    let name = "org.test.uccd.production.charge_prio";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let supported = proxy.get_charging_priorities_available().await.unwrap();
    let accepted = proxy.set_charging_priority("performance").await.unwrap();
    let actual = sysfs.get_file(attrs.join("charging_prio"));
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        supported.contains("performance") && accepted && actual.as_deref() == Some("performance"),
        "advertised={supported}, accepted={accepted}, driver={actual:?}"
    );
}

#[tokio::test]
async fn m8_prod_repeating_the_same_nvidia_offset_does_not_compound_it() {
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
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            for (path, data) in nodes {
                sysfs.set_file(gpu.join(path), data);
            }
        },
        |p| p,
    );

    let name = "org.test.uccd.production.ctgp_repeat";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_nvidia_power_offset(-5).await.unwrap());
    let first = std::fs::read_to_string(&log).unwrap();
    backend.poll_telemetry_once();
    assert!(proxy.set_nvidia_power_offset(-5).await.unwrap());
    let second = std::fs::read_to_string(&log).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(first, "45000");
    assert_eq!(
        second, first,
        "same requested offset after polling changed the applied absolute limit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m8_prod_unrelated_dbus_reads_remain_responsive_while_profile_io_is_stalled() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = create_default_settings();
    let mut profile = default_quiet_profile();
    profile.id = "stalled-profile".into();
    profile.odm_profile_name = None;
    profile.odm_power_limits = vec![41, 70, 90];
    settings
        .profiles
        .insert(profile.id.clone(), profile.clone());
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.production.stalled_rpc";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let read_proxy = uccd_proxy(&observer, name).await;
    let first = tokio::spawn(async move {
        let conn = session_conn().await;
        uccd_proxy(&conn, name)
            .await
            .set_active_profile(&profile.id)
            .await
            .unwrap()
    });
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    let read = tokio::time::timeout(Duration::from_millis(350), read_proxy.get_device_name()).await;
    let _ = release.send(());
    assert!(!first.await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        matches!(read, Ok(Ok(ref n)) if n == "Test Laptop"),
        "independent GetDeviceName was blocked: {read:?}"
    );
}

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

fn handoff_profile_with_tdps(id: &str, watts: [u32; 3]) -> ucc_core::UccProfile {
    let mut profile = default_quiet_profile();
    profile.id = id.into();
    profile.odm_profile_name = None;
    profile.odm_power_limits = watts.to_vec();
    profile
}

fn handoff_save_profiles(root: &Path, profiles: &[&ucc_core::UccProfile]) {
    let mut config = create_default_settings();
    for p in profiles {
        config.profiles.insert(p.id.clone(), (*p).clone());
    }
    ConfigManager::with_custom_path(root.join("settings.json"))
        .save(&config)
        .unwrap();
}

async fn handoff_wait_gate(entered: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
}

async fn handoff_wait_name(conn: &zbus::Connection, name: &str) {
    let bus = zbus::fdo::DBusProxy::new(conn).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !bus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_handoff_two_successful_dbus_profile_switches_leave_latest_hardware_and_disk_in_agreement()
{
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
async fn m8_life_failed_cleanup_handoff_returns_explicit_error_and_allows_retry_without_spinning()
{
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
    let second =
        tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&invalid.id)).await;
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
    let reported_pl1 = active_val["odmPowerLimits"]["tdpValues"][0].as_i64().unwrap();
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
async fn m8_life_extreme_nvidia_offset_returns_false_without_panicking_and_keeps_dbus_responsive()
{
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

struct CommitAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    reject_restore: std::sync::atomic::AtomicBool,
    reject_second_tdp: std::sync::atomic::AtomicBool,
    clamp_pl1: bool,
    hide_pl1_readback: bool,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl CommitAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            reject_restore: std::sync::atomic::AtomicBool::new(false),
            reject_second_tdp: std::sync::atomic::AtomicBool::new(false),
            clamp_pl1: false,
            hide_pl1_readback: false,
            tdp_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for CommitAuditPlatform {
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
    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        if idx == 0 && self.hide_pl1_readback {
            return Err(PlatformError::Unavailable);
        }
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
        if idx == 1 && self.reject_second_tdp.load(SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        if idx == 0 && watts == 55 {
            if self.reject_restore.load(SeqCst) {
                return Err(PlatformError::Unavailable);
            }
            if self.clamp_pl1 {
                return self.inner.set_tdp(idx, 50);
            }
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
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
async fn m8_commit_superseded_write_restores_channel_unmanaged_by_newer_profile() {
    let root = tempfile::tempdir().unwrap();
    let old = handoff_profile_with_tdps("manage-tdp", [41, 70, 90]);
    let mut next = default_quiet_profile();
    next.id = "leave-tdp-alone".into();
    next.odm_profile_name = None;
    next.odm_power_limits.clear();
    handoff_save_profiles(root.path(), &[&old, &next]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = CommitAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.commit_reg.omitted";
    let runtime = start_runtime(backend, name, false).await;
    platform.set_tdp_value(0, 65);
    let older = tokio::spawn(async move {
        let conn = session_conn().await;
        uccd_proxy(&conn, name)
            .await
            .set_active_profile(&old.id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let newer =
        tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&next.id)).await;
    let _ = release.send(());
    let older_ok = older.await.unwrap();
    let newer_ok = newer.unwrap().unwrap();
    let actual = platform.read_tdp(0).unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(older_ok && newer_ok);
    assert_eq!(active, next.id);
    assert_eq!(actual, 65);
}

#[tokio::test]
async fn m8_commit_failed_rollback_syncs_active_readback() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("known", [55, 80, 95]);
    let bad = handoff_profile_with_tdps("fails-during-write", [41, 70, 90]);
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(CommitAuditPlatform::new(p));
            tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let platform_control = rx.recv().unwrap();
    let name = "org.test.uccd.commit_reg.rollback";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    platform_control
        .reject_second_tdp
        .store(true, std::sync::atomic::Ordering::SeqCst);
    platform_control
        .reject_restore
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&bad).unwrap())
        .await
        .unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let readback = platform.read_tdp(0).unwrap() as u32;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!((readback, active.odm_power_limits[0]), (41, 41));
}

#[tokio::test]
async fn m8_commit_ordinary_apply_rejects_clamped_or_unavailable_readback() {
    for hide_readback in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let requested = handoff_profile_with_tdps("would-be-clamped", [55, 80, 95]);
        let (backend, platform, _, _) = build_custom_backend(
            root.path(),
            true,
            |_| {},
            |p| {
                let mut wrapper = CommitAuditPlatform::new(p);
                wrapper.clamp_pl1 = !hide_readback;
                wrapper.hide_pl1_readback = hide_readback;
                Arc::new(wrapper)
            },
        );
        let name = if hide_readback {
            "org.test.uccd.commit_reg.missing_readback"
        } else {
            "org.test.uccd.commit_reg.clamp"
        };
        let runtime = start_runtime(backend, name, false).await;
        let conn = session_conn().await;
        let proxy = uccd_proxy(&conn, name).await;
        if !hide_readback {
            let reply = proxy
                .apply_profile(&serde_json::to_string(&requested).unwrap())
                .await
                .unwrap();
            assert!(!reply);
            assert_eq!(platform.read_tdp(0).unwrap(), 90);
            let active: ucc_core::UccProfile =
                serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
            assert_ne!(active.id, requested.id);
        } else {
            // Even baseline check cannot confirm PL1 when read_tdp fails -> rejected cleanly.
            let reply = proxy
                .apply_profile(&serde_json::to_string(&requested).unwrap())
                .await
                .unwrap();
            assert!(!reply);
        }
        runtime.shutdown(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn m8_commit_startup_does_not_claim_unapplied_profile_and_recovers_later() {
    let root = tempfile::tempdir().unwrap();
    let profile = handoff_profile_with_tdps("bad-powered-profile", [45, 125, 130]);
    let mut config = create_default_settings();
    config.profiles.insert(profile.id.clone(), profile.clone());
    config.set_active_profile_for_mode("power_ac", &profile.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&config)
        .unwrap();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.commit_reg.unapplied_start";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let pl2_before = platform.read_tdp(1).unwrap();
    assert_ne!(active, profile.id);
    assert_eq!(pl2_before, 120);

    // Raise hardware PL2 ceiling so background reconciliation can now apply the configured profile.
    platform.set_tdp_limits(1, (20, 130));
    backend.reconcile_power_state(false);
    let active_after = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let pl2_after = platform.read_tdp(1).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!((active_after, pl2_after), (profile.id, 125));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_commit_successor_captures_cpu_defaults_only_after_predecessor_restores() {
    let root = tempfile::tempdir().unwrap();
    let mut pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    pending.cpu.governor = "performance".into();
    handoff_save_profiles(root.path(), &[&pending]);
    let governor = root
        .path()
        .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (old, old_platform, shared, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(&governor, "powersave");
            s.set_file(
                governor.with_file_name("scaling_available_governors"),
                "powersave performance",
            );
        },
        |p| {
            let wrapper = CommitAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.commit_reg.cpu_handoff";
    let old = start_runtime(old, name, false).await;
    let request = tokio::spawn(async move {
        let conn = session_conn().await;
        let _ = uccd_proxy(&conn, name)
            .await
            .set_active_profile(&pending.id)
            .await;
    });
    handoff_wait_gate(entered).await;
    assert_eq!(shared.get_file(&governor).as_deref(), Some("performance"));
    old.shutdown(Duration::from_millis(30)).await;
    let (next, _, _, _) = build_custom_backend_with_sysfs(
        root.path(),
        true,
        Some(shared.clone()),
        |_| {},
        |_| old_platform,
    );
    let next = tokio::spawn(start_runtime(next, name, false));
    let conn = session_conn().await;
    handoff_wait_name(&conn, name).await;
    let _ = release.send(());
    let _ = request.await;
    let next = next.await.unwrap();
    next.shutdown(Duration::from_secs(1)).await;
    assert_eq!(shared.get_file(&governor).as_deref(), Some("powersave"));
}

#[tokio::test]
async fn m8_commit_failed_batch_and_single_state_map_do_not_persist_changes() {
    let root = tempfile::tempdir().unwrap();
    let invalid = handoff_profile_with_tdps("cannot-apply", [45, 125, 130]);
    handoff_save_profiles(root.path(), &[&invalid]);
    let settings = root.path().join("settings.json");
    let initial = ConfigManager::load_from_path(&settings).unwrap().state_map;
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.commit_reg.state_map";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    let batch = serde_json::json!({"power_ac": invalid.id, "power_bat": invalid.id}).to_string();
    let batch_ok = proxy.set_batch_state_map(&batch).await.unwrap();
    let single_ok = proxy.set_state_map("power_ac", &invalid.id).await.unwrap();
    let saved = ConfigManager::load_from_path(&settings).unwrap().state_map;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!batch_ok && !single_ok);
    assert_eq!(saved, initial);
}

struct TxRegressionPlatform {
    inner: Arc<MockPlatformBackend>,
    mutate_before_error: bool,
    gated_channel: usize,
    reject_second_tdp: std::sync::atomic::AtomicBool,
    omit_tdp1_read: std::sync::atomic::AtomicBool,
    odm_fail: std::sync::atomic::AtomicBool,
    no_odm_readback: bool,
    odm_read_error_at: std::sync::atomic::AtomicUsize,
    reject_second_41: bool,
    calls_41: std::sync::atomic::AtomicUsize,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    read_tdp0_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    restore_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    stale_rollback_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl TxRegressionPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            mutate_before_error: false,
            gated_channel: 0,
            reject_second_tdp: std::sync::atomic::AtomicBool::new(false),
            omit_tdp1_read: std::sync::atomic::AtomicBool::new(false),
            odm_fail: std::sync::atomic::AtomicBool::new(false),
            no_odm_readback: false,
            odm_read_error_at: std::sync::atomic::AtomicUsize::new(0),
            reject_second_41: false,
            calls_41: std::sync::atomic::AtomicUsize::new(0),
            tdp_gate: std::sync::Mutex::new(None),
            read_tdp0_gate: std::sync::Mutex::new(None),
            restore_gate: std::sync::Mutex::new(None),
            stale_rollback_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for TxRegressionPlatform {
    fn kind(&self) -> PlatformBackendKind { self.inner.kind() }
    fn is_available(&self) -> bool { self.inner.is_available() }
    fn capabilities(&self) -> PlatformCapabilities { self.inner.capabilities() }
    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> { self.inner.read_fan_temp(fan_idx) }
    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> { self.inner.read_fan_speed_percent(fan_idx) }
    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> { self.inner.set_fan_speed_percent(fan_idx, percent) }
    fn set_fans_auto(&self) -> Result<(), PlatformError> { self.inner.set_fans_auto() }
    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        if idx == 0 {
            let gate = self.read_tdp0_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if idx == 1 && self.omit_tdp1_read.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        self.inner.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> { self.inner.read_tdp_limits(idx) }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        if idx == 2 && watts == 95 {
            let gate = self.stale_rollback_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if self.reject_second_41
            && idx == 0
            && watts == 41
            && self
                .calls_41
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 1
        {
            return Err(PlatformError::Unavailable);
        }
        if idx == 1 && watts == 80 {
            let gate = self.restore_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if idx == self.gated_channel && watts == [41, 70, 90][idx] {
            let gate = self.tdp_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if idx == 1 && self.reject_second_tdp.load(std::sync::atomic::Ordering::SeqCst) {
            if self.mutate_before_error {
                self.inner.set_tdp(idx, watts)?;
            }
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        use std::sync::atomic::Ordering::SeqCst;
        #[allow(deprecated)]
        let error_now =
            self.odm_read_error_at
                .fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1))
                == Ok(1);
        if error_now {
            return Err(PlatformError::Unsupported(
                "profile sysfs disappeared after preflight",
            ));
        }
        if self.no_odm_readback {
            return Err(PlatformError::Unsupported("ODM readback unsupported"));
        }
        self.inner.read_perf_profile()
    }

    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_perf_profile(value)?;
        if self.odm_fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
            for (i, w) in [45, 70, 105].into_iter().enumerate() {
                self.inner.set_tdp(i, w)?;
            }
            return Err(PlatformError::Unavailable);
        }
        Ok(())
    }
    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> { self.inner.set_mode_enable(value) }
    fn set_mode(&self, value: i32) -> Result<(), PlatformError> { self.inner.set_mode(value) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_winning_profile_does_not_inherit_losing_uncommitted_tdp() {
    let root = tempfile::tempdir().unwrap();
    let old = handoff_profile_with_tdps("older-writes", [41, 70, 90]);
    let mut new = default_quiet_profile();
    new.id = "no-tdp-override".into();
    new.odm_profile_name = None;
    new.odm_power_limits.clear();
    handoff_save_profiles(root.path(), &[&old, &new]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.gated_channel = 1;
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.tx_reg.uncommitted";
    let runtime = start_runtime(backend, name, false).await;
    platform.set_tdp_value(0, 65);
    let older = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.set_active_profile(&old.id).await.unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let newer_ok = tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&new.id))
        .await
        .unwrap()
        .unwrap();
    let _ = release.send(());
    let _ = older.await.unwrap();
    let actual = platform.read_tdp(0).unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(newer_ok && active == new.id && actual == 65, "actual PL1={actual}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_deleted_profile_cannot_be_committed_by_concurrent_set_active() {
    let root = tempfile::tempdir().unwrap();
    let pending = handoff_profile_with_tdps("pending-removal", [41, 70, 90]);
    handoff_save_profiles(root.path(), &[&pending]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.tx_reg.delete";
    let runtime = start_runtime(backend, name, false).await;
    let id = pending.id.clone();
    let set = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.set_active_profile(&id).await.unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let deleted = proxy.delete_custom_profile(&pending.id).await.unwrap();
    let _ = release.send(());
    let accepted = set.await.unwrap();
    let s = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(deleted && !accepted && !s.profiles.contains_key(&pending.id));
    assert_ne!(s.state_map["power_ac"], pending.id);
}

#[tokio::test]
async fn m8_tx_failing_tdp_write_after_mutation_is_rolled_back() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("baseline", [55, 80, 95]);
    let target = handoff_profile_with_tdps("failing", [43, 75, 95]);
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.mutate_before_error = true;
            let w = Arc::new(w);
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.tx_reg.mutate_err";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.apply_profile(&serde_json::to_string(&initial).unwrap()).await.unwrap());
    ctl.reject_second_tdp.store(true, std::sync::atomic::Ordering::SeqCst);
    let ok = proxy.apply_profile(&serde_json::to_string(&target).unwrap()).await.unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let actual = platform.read_tdp(1).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(active.odm_power_limits[1], actual as u32);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_drained_shutdown_rolls_back_cancelled_tdp_write() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("baseline", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.tx_reg.drained_shutdown";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.apply_profile(&serde_json::to_string(&initial).unwrap()).await.unwrap());
    let pending_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.apply_profile(&serde_json::to_string(&pending).unwrap()).await
    });
    handoff_wait_gate(entered).await;
    let shutdown = tokio::spawn(runtime.shutdown(Duration::from_secs(2)));
    let bus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while bus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let _ = release.send(());
    let _ = pending_call.await;
    shutdown.await.unwrap();
    let actual: Vec<_> = (0..3).map(|i| platform.read_tdp(i).unwrap()).collect();
    assert_eq!(actual, vec![55, 80, 95]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_power_change_during_set_active_clears_applied_state_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let ac = handoff_profile_with_tdps("custom-ac", [41, 70, 90]);
    let bat = handoff_profile_with_tdps("custom-bat", [45, 125, 130]);
    let mut cfg = create_default_settings();
    cfg.profiles.insert(ac.id.clone(), ac.clone());
    cfg.profiles.insert(bat.id.clone(), bat.clone());
    cfg.set_active_profile_for_mode("power_bat", &bat.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&cfg)
        .unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.tx_reg.power_retry";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let task = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.set_active_profile(&ac.id).await.unwrap()
    });
    handoff_wait_gate(entered).await;
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(root.path().join("sys/class/power_supply/BAT0/type"), "Battery");
    backend.reconcile_power_state(false);
    let _ = release.send(());
    let _ = task.await.unwrap();
    platform.set_tdp_limits(1, (20, 130));
    backend.reconcile_power_state(false);
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let state = proxy.get_power_state().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!((state.as_str(), active.as_str()), ("power_bat", bat.id.as_str()));
}

#[tokio::test]
async fn m8_tx_display_brightness_failure_aborts_profile_apply() {
    let root = tempfile::tempdir().unwrap();
    let node = root.path().join("sys/class/backlight/panel0/brightness");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(&node, "55");
            s.set_file(node.with_file_name("max_brightness"), "100");
        },
        |p| p,
    );
    let name = "org.test.uccd.tx_reg.brightness";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    sysfs.set_file(&node, "55");
    sysfs.set_write_error(&node);
    let mut p = default_quiet_profile();
    p.id = "req-brightness".into();
    p.display.use_brightness = true;
    p.display.brightness = 40;
    p.odm_profile_name = None;
    p.odm_power_limits.clear();
    let ok = proxy.apply_profile(&serde_json::to_string(&p).unwrap()).await.unwrap();
    let actual = proxy.get_display_brightness().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok, "apply_profile unexpectedly succeeded (actual={actual})");
    assert_eq!(actual, 55, "brightness mutated despite write error");
}

#[tokio::test]
async fn m8_tx_preview_mode_blocks_brightness_and_hardware_writes() {
    if std::env::var_os("UCC_TEST_PREVIEW_REG_CHILD").is_none() {
        let out = tokio::task::spawn_blocking(|| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "m8_tx_preview_mode_blocks_brightness_and_hardware_writes",
                    "--nocapture",
                ])
                .env("UCC_TEST_PREVIEW_REG_CHILD", "1")
                .env("UCC_READ_ONLY_PREVIEW", "1")
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(out.status.success(), "{out:?}");
        return;
    }
    assert!(ucc_config::is_read_only_preview());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sys/class/backlight/intel_backlight/brightness");
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sys| {
            sys.set_file(&path, "55");
            sys.set_file(root.path().join("sys/class/backlight/intel_backlight/max_brightness"), "100");
        },
        |p| p,
    );
    let name = "org.test.uccd.tx_reg.preview";
    let runtime = start_runtime(backend.clone(), name, false).await;
    backend.activate_hardware();
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    sysfs.clear_journal();
    platform.clear_journal();
    let ok = proxy.set_display_brightness(40).await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(sysfs.get_file(&path).as_deref(), Some("55"));
    assert!(sysfs.journal().is_empty() && platform.journal().is_empty());
}

#[tokio::test]
async fn m8_tx_lightbar_worker_ticks_at_frame_cadence() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, lighting) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.tx_reg.lightbar_cadence";
    let runtime = start_runtime(backend, name, true).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let cfg = serde_json::json!({
        "managed": true,
        "silent_mode": false,
        "welcome_anim": false,
        "power_settings": {
            "ac": { "enabled": true, "brightness_val": 200, "mode": "Rainbow", "color_mode": [255, 0, 0] },
            "type_c": { "enabled": true, "brightness_val": 200, "mode": "Rainbow", "color_mode": [255, 0, 0] },
            "bat": { "enabled": true, "brightness_val": 200, "mode": "Rainbow", "color_mode": [255, 0, 0] }
        }
    });
    assert!(proxy.set_lightbar_settings_json(&cfg.to_string()).await.unwrap());
    lighting.clear_ops();
    assert!(proxy.trigger_lightbar_animation(true).await.unwrap());
    tokio::time::sleep(Duration::from_millis(120)).await;
    let ops = lighting.ops().len();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(ops >= 3, "expected 20 ms frame cadence (>=3 writes in 120 ms), got {ops}");
}

#[tokio::test]
async fn m8_tx_legacy_dbus_contracts_and_fan_catalog() {
    let root = tempfile::tempdir().unwrap();
    let custom = handoff_profile_with_tdps("one-custom", [45, 80, 95]);
    handoff_save_profiles(root.path(), &[&custom]);
    let fn_lock = root.path().join("sys/devices/platform/tuxedo_keyboard/fn_lock");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sys| {
            let tree = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
            sys.set_file(tree.join("cpuinfo_min_freq"), "800000");
            sys.set_file(tree.join("cpuinfo_max_freq"), "4200000");
            sys.set_file(&fn_lock, "0");
            sys.set_write_error(&fn_lock);
        },
        |p| p,
    );
    let name = "org.test.uccd.tx_reg.contracts";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    assert!(!proxy.set_display_refresh_rate("eDP-1", 120).await.unwrap());
    assert!(proxy.set_fn_lock_status(true).await.is_err());
    let sys_info: serde_json::Value = serde_json::from_str(&proxy.get_system_info_json().await.unwrap()).unwrap();
    assert_eq!(sys_info["laptopModel"], "Test Laptop");
    assert!(sys_info["cpuModel"].is_string() && sys_info["dGpuModel"].is_string() && sys_info["iGpuModel"].is_string());

    let kb_data = serde_json::json!({"color": "#123456"});
    assert!(proxy.save_custom_keyboard_profile("rgb-mine", "Mine", &kb_data.to_string()).await.unwrap());
    let kb_list: serde_json::Value = serde_json::from_str(&proxy.get_custom_keyboard_profiles_json().await.unwrap()).unwrap();
    assert_eq!(kb_list, serde_json::json!([{"id": "rgb-mine", "name": "Mine", "json": kb_data}]));

    let freq_limits: serde_json::Value = serde_json::from_str(&proxy.get_cpu_frequency_limits_json().await.unwrap()).unwrap();
    assert_eq!((freq_limits["min"].as_i64(), freq_limits["max"].as_i64()), (Some(800000), Some(4200000)));

    let custom_list: Vec<serde_json::Value> = serde_json::from_str(&proxy.get_custom_profiles_json().await.unwrap()).unwrap();
    let custom_ids: Vec<_> = custom_list.iter().filter_map(|p| p["id"].as_str()).collect();
    assert_eq!(custom_ids, ["one-custom"]);

    let fan_list: Vec<serde_json::Value> = serde_json::from_str(&proxy.get_fan_profile_names().await.unwrap()).unwrap();
    let silent: serde_json::Value = serde_json::from_str(&proxy.get_fan_profile("Silent [Built-in]").await.unwrap()).unwrap();
    assert_eq!(fan_list.len(), 5);
    assert!(fan_list.iter().any(|p| p["name"] == "Silent [Built-in]"));
    assert!(silent["tableCPU"].as_array().is_some_and(|a| !a.is_empty()));

    let topo: Vec<serde_json::Value> = serde_json::from_str(&proxy.get_cpu_topology_json().await.unwrap()).unwrap();
    let health: serde_json::Value = serde_json::from_str(&proxy.get_driver_health_json().await.unwrap()).unwrap();
    assert_eq!(topo.len(), 2);
    assert_eq!(health["healthy"], true);

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn recheck_01_winning_profile_without_tdp_overrides_ignores_in_flight_loser_write() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("before", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("loser", [41, 70, 90]);
    let mut winner = initial.clone();
    winner.id = "winner-without-tdp".to_string();
    winner.odm_power_limits.clear();
    let (sample_started, sample_entered) = std::sync::mpsc::channel();
    let (release_sample, sample_wait) = std::sync::mpsc::channel();
    let (writer_started, writer_entered) = std::sync::mpsc::channel();
    let (release_writer, writer_wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        let mut wrapper = TxRegressionPlatform::new(p);
        wrapper.gated_channel = 1;
        *wrapper.tdp_gate.lock().unwrap() = Some((writer_started, writer_wait));
        let wrapper = Arc::new(wrapper);
        tx.send(wrapper.clone()).unwrap();
        wrapper
    });
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.recheck_01";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.apply_profile(&serde_json::to_string(&initial).unwrap()).await.unwrap());
    *ctl.read_tdp0_gate.lock().unwrap() = Some((sample_started, sample_wait));
    let winner_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.apply_profile(&serde_json::to_string(&winner).unwrap()).await
    });
    handoff_wait_gate(sample_entered).await;
    let old_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.apply_profile(&serde_json::to_string(&pending).unwrap()).await
    });
    handoff_wait_gate(writer_entered).await;
    assert_eq!(platform.read_tdp(0).unwrap(), 41);
    release_sample.send(()).unwrap();
    let winner_ok = winner_call.await.unwrap().unwrap();
    release_writer.send(()).unwrap();
    let old_ok = old_call.await.unwrap().unwrap();
    let final_pl1 = platform.read_tdp(0).unwrap();
    let active: serde_json::Value = serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(winner_ok && old_ok);
    assert_eq!(active["id"], "winner-without-tdp");
    assert_eq!(final_pl1, 55);
}

#[tokio::test]
async fn recheck_02_failed_odm_only_read_preserves_tdp_channel_identity() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("known-limits", [55, 80, 95]);
    let mut target = handoff_profile_with_tdps("odm-only", [55, 80, 95]);
    target.odm_power_limits.clear();
    target.odm_profile_name = Some("performance".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        let wrapper = Arc::new(TxRegressionPlatform::new(p));
        tx.send(wrapper.clone()).unwrap();
        wrapper
    });
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.recheck_02";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.apply_profile(&serde_json::to_string(&initial).unwrap()).await.unwrap());
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    ctl.omit_tdp1_read.store(true, std::sync::atomic::Ordering::SeqCst);
    ctl.odm_fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let ok = proxy.apply_profile(&serde_json::to_string(&target).unwrap()).await.unwrap();
    let actual: Vec<_> = (0..3).map(|i| platform.read_tdp(i).unwrap()).collect();
    let writes = platform.journal();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(actual, vec![55, 80, 95]);
    assert!(writes.is_empty(), "pre-validation should fail with zero hardware writes: {writes:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn recheck_03_shutdown_deadline_stops_unstarted_rollback_writes() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("before", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (rollback_started, rollback_entered) = std::sync::mpsc::channel();
    let (release_rollback, rollback_wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        let mut wrapper = TxRegressionPlatform::new(p);
        wrapper.gated_channel = 1;
        *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
        let wrapper = Arc::new(wrapper);
        tx.send(wrapper.clone()).unwrap();
        wrapper
    });
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.recheck_03";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.apply_profile(&serde_json::to_string(&initial).unwrap()).await.unwrap());
    *ctl.restore_gate.lock().unwrap() = Some((rollback_started, rollback_wait));
    let pending_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.apply_profile(&serde_json::to_string(&pending).unwrap()).await
    });
    handoff_wait_gate(entered).await;
    let bus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    let shutdown = tokio::spawn(runtime.shutdown(Duration::from_millis(500)));
    tokio::time::timeout(Duration::from_secs(3), async {
        while bus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    release.send(()).unwrap();
    handoff_wait_gate(rollback_entered).await;
    shutdown.await.unwrap();
    platform.clear_journal();
    release_rollback.send(()).unwrap();
    let _ = pending_call.await.unwrap();
    let late = platform.journal();
    assert!(!late.iter().any(|w| matches!(w, ucc_hardware::PlatformWrite::SetTdp { index: 0, .. })),
        "late writes after deadline: {late:?}");
}

#[tokio::test]
async fn recheck_04_brightness_apply_rejects_mismatched_actual_brightness_and_restores_request() {
    let root = tempfile::tempdir().unwrap();
    let panel = root.path().join("sys/class/backlight/panel0");
    let (backend, _, sysfs, _) = build_custom_backend(root.path(), true, |s| {
        s.set_file(panel.join("brightness"), "55");
        s.set_file(panel.join("actual_brightness"), "55");
        s.set_file(panel.join("max_brightness"), "100");
    }, |p| p);
    let name = "org.test.uccd.reg.recheck_04";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut profile = handoff_profile_with_tdps("brightness", [55, 80, 95]);
    profile.display.use_brightness = true;
    profile.display.brightness = 40;
    sysfs.set_file(panel.join("brightness"), "55");
    sysfs.set_file(panel.join("actual_brightness"), "55");
    let ok = proxy.apply_profile(&serde_json::to_string(&profile).unwrap()).await.unwrap();
    let actual = proxy.get_display_brightness().await.unwrap();
    let restored = sysfs.get_file(panel.join("brightness"));
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(actual, 55);
    assert_eq!(restored.as_deref(), Some("55"));
}

#[tokio::test]
async fn recheck_05_editing_active_custom_profile_reapplies_hardware_and_inactive_does_not() {
    let root = tempfile::tempdir().unwrap();
    let active_custom = handoff_profile_with_tdps("edit-active", [55, 80, 95]);
    let inactive_custom = handoff_profile_with_tdps("edit-inactive", [35, 65, 85]);
    handoff_save_profiles(root.path(), &[&active_custom, &inactive_custom]);
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.reg.recheck_05";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    assert!(proxy.set_active_profile(&active_custom.id).await.unwrap());
    let mut edited_inactive = inactive_custom.clone();
    edited_inactive.odm_power_limits[0] = 30;
    assert!(proxy.update_custom_profile(&serde_json::to_string(&edited_inactive).unwrap()).await.unwrap());
    assert_eq!(platform.read_tdp(0).unwrap(), 55, "editing inactive profile must not mutate hardware");

    let mut edited_active = active_custom.clone();
    edited_active.odm_power_limits[0] = 45;
    assert!(proxy.update_custom_profile(&serde_json::to_string(&edited_active).unwrap()).await.unwrap());
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    assert_eq!((active.odm_power_limits[0], platform.read_tdp(0).unwrap()), (45, 45));

    // Failing hardware update (out-of-limits TDP) must NOT persist stale edit to disk or active RAM.
    let mut invalid_hw = edited_active.clone();
    invalid_hw.odm_power_limits[0] = 999;
    assert!(!proxy.update_custom_profile(&serde_json::to_string(&invalid_hw).unwrap()).await.unwrap());
    let saved = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    assert_eq!(saved.profiles[&active_custom.id].odm_power_limits[0], 45);
    assert_eq!(platform.read_tdp(0).unwrap(), 45);

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn recheck_06_custom_profile_mutators_reject_builtin_ids() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.reg.recheck_06";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    let mut builtin = default_quiet_profile();
    builtin.name = "Tampered Built-in".into();
    let body = serde_json::to_string(&builtin).unwrap();
    assert!(!proxy.add_custom_profile(&body).await.unwrap());
    assert!(!proxy.save_custom_profile(&body).await.unwrap());
    assert!(!proxy.update_custom_profile(&body).await.unwrap());
    assert!(!proxy.delete_custom_profile(&builtin.id).await.unwrap());

    let missing = handoff_profile_with_tdps("does-not-exist", [45, 75, 95]);
    assert!(!proxy.update_custom_profile(&serde_json::to_string(&missing).unwrap()).await.unwrap());

    let all: Vec<ucc_core::UccProfile> =
        serde_json::from_str(&proxy.get_profiles_json().await.unwrap()).unwrap();
    assert_eq!(all.iter().find(|p| p.id == builtin.id).unwrap().name, default_quiet_profile().name);
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn recheck_08_prime_display_modes_x11_and_yuv420_report_real_system_state() {
    let root = tempfile::tempdir().unwrap();
    let drm = root.path().join("sys/class/drm/card0-eDP-1");
    let sess = root.path().join("run/systemd/sessions/2");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(root.path().join("etc/prime-discrete"), "on\n");
            s.set_file(drm.join("status"), "connected\n");
            s.set_file(drm.join("modes"), "2560x1600@240\n2560x1600@60\n1920x1200\n");
            s.set_file(drm.join("mode"), "2560x1600@240\n");
            s.set_file(drm.join("force_yuv420_output"), "0\n");
            s.set_file(sess.clone(), "STATE=active\nACTIVE=1\nTYPE=wayland\n");
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.recheck_08";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    assert_eq!(proxy.get_prime_state().await.unwrap(), "dGPU");
    sysfs.set_file(root.path().join("etc/prime-discrete"), "off\n");
    assert_eq!(proxy.get_prime_state().await.unwrap(), "iGPU");
    sysfs.set_file(root.path().join("etc/prime-discrete"), "on-demand\n");
    assert_eq!(proxy.get_prime_state().await.unwrap(), "on-demand");

    assert!(!proxy.get_is_x11().await.unwrap());
    sysfs.set_file(sess, "STATE=active\nACTIVE=1\nTYPE=x11\n");
    assert!(proxy.get_is_x11().await.unwrap());
    assert!(proxy.get_force_yuv420_output_switch_available().await.unwrap());

    let modes: serde_json::Value =
        serde_json::from_str(&proxy.get_display_modes_json().await.unwrap()).unwrap();
    assert_eq!(modes["displayName"], "eDP-1");
    assert_eq!(modes["activeMode"]["xResolution"], 2560);
    assert_eq!(modes["activeMode"]["yResolution"], 1600);
    assert_eq!(modes["activeMode"]["refreshRates"], serde_json::json!([240]));
    assert_eq!(modes["displayModes"].as_array().unwrap().len(), 2);
    assert_eq!(modes["displayModes"][1]["xResolution"], 1920);
    assert_eq!(modes["displayModes"][1]["refreshRates"], serde_json::json!([]));

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn recheck_09_system_info_detects_igpu_and_dgpu_from_pci_sysfs_without_telemetry() {
    let root = tempfile::tempdir().unwrap();
    let igpu = root.path().join("sys/bus/pci/devices/0000:00:02.0");
    let dgpu = root.path().join("sys/bus/pci/devices/0000:01:00.0");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(igpu.join("class"), "0x030000\n");
            s.set_file(igpu.join("vendor"), "0x8086\n");
            s.set_file(igpu.join("device"), "0xa7a0\n");
            s.set_file(dgpu.join("class"), "0x030000\n");
            s.set_file(dgpu.join("vendor"), "0x10de\n");
            s.set_file(dgpu.join("device"), "0x28e0\n");
            s.set_file(
                root.path().join("usr/share/hwdata/pci.ids"),
                "10de  NVIDIA Corporation\n\t28e0  AD106M [GeForce RTX 4060 Max-Q / Mobile]\n8086  Intel Corporation\n\ta7a0  Raptor Lake-P [Iris Xe Graphics]\n",
            );
            s.set_file(root.path().join("proc/cpuinfo"), "processor\t: 0\nmodel name\t: Fixture Core i7\n");
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.recheck_09";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    proxy.set_sensor_data_collection_status(false).await.unwrap();

    let info: serde_json::Value =
        serde_json::from_str(&proxy.get_system_info_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;

    assert_eq!(info["cpuModel"], "Fixture Core i7");
    assert_eq!(info["iGpuModel"], "Intel Iris Xe Graphics");
    assert_eq!(info["dGpuModel"], "NVIDIA GeForce RTX 4060 Max-Q / Mobile");
}

struct StallingBacklightSysfs {
    inner: Arc<MockSysfs>,
    gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl ucc_hardware::Sysfs for StallingBacklightSysfs {
    fn read(&self, path: &Path) -> std::io::Result<String> {
        if path.ends_with("actual_brightness")
            && self.inner.get_file(path.with_file_name("brightness")).as_deref() == Some("40")
        {
            let gate = self.gate.lock().unwrap().take();
            if let Some((start, release)) = gate {
                start.send(()).unwrap();
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        self.inner.read(path)
    }
    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        self.inner.write(path, content)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        self.inner.read_dir(path)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn followup_01_brightness_verify_failure_does_not_start_rollback_after_deadline() {
    let root = tempfile::tempdir().unwrap();
    let panel = root.path().join("sys/class/backlight/panel0");
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(panel.join("brightness"), "55");
    sysfs.set_file(panel.join("actual_brightness"), "55");
    sysfs.set_file(panel.join("max_brightness"), "100");
    sysfs.set_file(root.path().join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"), "3200000");
    sysfs.set_file(root.path().join("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq"), "3200000");
    sysfs.set_file(root.path().join("sys/devices/system/cpu/cpu1/online"), "1");
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "1");

    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let stalling_sysfs: Arc<dyn ucc_hardware::Sysfs> = Arc::new(StallingBacklightSysfs {
        inner: sysfs.clone(),
        gate: std::sync::Mutex::new(Some((started, wait))),
    });
    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));
    let lighting = Arc::new(MockLightingTransport::new());
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: platform.clone(),
        sysfs: stalling_sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(root.path().join("settings.json")),
        sys_root: root.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            root.path().join("sys"),
            root.path().join("proc"),
            ucc_hardware::default_process_runner(),
            root.path().join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));

    let name = "org.test.uccd.reg.followup_01";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut initial = handoff_profile_with_tdps("initial", [55, 80, 95]);
    initial.display.use_brightness = false;
    assert!(proxy.apply_profile(&serde_json::to_string(&initial).unwrap()).await.unwrap());
    let mut pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    pending.display.use_brightness = true;
    pending.display.brightness = 40;
    let rpc = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.apply_profile(&serde_json::to_string(&pending).unwrap()).await
    });
    handoff_wait_gate(entered).await;
    runtime.shutdown(Duration::from_millis(100)).await;
    platform.clear_journal();
    sysfs.clear_journal();
    release.send(()).unwrap();
    let reply = rpc.await.unwrap().unwrap();
    assert!(!reply);
    let tdp = platform.journal();
    let late_sysfs = sysfs.journal();
    assert!(
        !tdp.iter().any(|w| matches!(w, ucc_hardware::PlatformWrite::SetTdp { .. }))
            && late_sysfs.is_empty(),
        "late rollback writes after shutdown deadline: tdp={tdp:?}, sysfs={late_sysfs:?}"
    );
}

#[tokio::test]
async fn followup_02_failed_odm_without_readback_does_not_leave_a_different_mode() {
    let root = tempfile::tempdir().unwrap();
    let mut odm_only = handoff_profile_with_tdps("odm-only", [55, 80, 95]);
    odm_only.odm_power_limits.clear();
    odm_only.odm_profile_name = Some("performance".into());
    let mut odm_plus_tdp = handoff_profile_with_tdps("odm-tdp", [45, 75, 90]);
    odm_plus_tdp.odm_profile_name = Some("performance".into());

    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        let mut w = TxRegressionPlatform::new(p);
        w.no_odm_readback = true;
        let w = Arc::new(w);
        tx.send(w.clone()).unwrap();
        w
    });
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.followup_02";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    ctl.odm_fail.store(true, std::sync::atomic::Ordering::SeqCst);

    assert!(!proxy.apply_profile(&serde_json::to_string(&odm_only).unwrap()).await.unwrap());
    assert_eq!(platform.read_perf_profile().unwrap(), 2);
    assert!(platform.journal().is_empty());

    assert!(!proxy.apply_profile(&serde_json::to_string(&odm_plus_tdp).unwrap()).await.unwrap());
    assert_eq!(platform.read_perf_profile().unwrap(), 2);
    assert!(platform.journal().is_empty());
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn followup_03_successful_edit_is_persisted_if_an_activation_supersedes_it() {
    let root = tempfile::tempdir().unwrap();
    let original = handoff_profile_with_tdps("edited", [55, 80, 95]);
    let mut edited = original.clone();
    edited.odm_power_limits = vec![41, 70, 90];
    let next = handoff_profile_with_tdps("winner", [62, 82, 95]);
    handoff_save_profiles(root.path(), &[&original, &next]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        let w = TxRegressionPlatform::new(p);
        *w.tdp_gate.lock().unwrap() = Some((started, wait));
        Arc::new(w)
    });
    let name = "org.test.uccd.reg.followup_03";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_active_profile(&original.id).await.unwrap());
    let editing = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.update_custom_profile(&serde_json::to_string(&edited).unwrap()).await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy.set_active_profile(&next.id).await.unwrap());
    release.send(()).unwrap();
    let ok = editing.await.unwrap().unwrap();
    let disk = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let actual = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(ok);
    assert_eq!(disk.profiles[&original.id].odm_power_limits[0], 41);
    assert_eq!((active.id.as_str(), actual), (next.id.as_str(), 62));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn followup_04_concurrent_edit_and_activation_agree_on_the_active_profile_revision() {
    let root = tempfile::tempdir().unwrap();
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    let edited = handoff_profile_with_tdps("pending", [55, 80, 95]);
    handoff_save_profiles(root.path(), &[&pending]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| {
        let w = TxRegressionPlatform::new(p);
        *w.tdp_gate.lock().unwrap() = Some((started, wait));
        Arc::new(w)
    });
    let name = "org.test.uccd.reg.followup_04";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let setting = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name).await.set_active_profile(&pending.id).await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy.update_custom_profile(&serde_json::to_string(&edited).unwrap()).await.unwrap());
    release.send(()).unwrap();
    let ok = setting.await.unwrap().unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let disk = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let actual = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(disk.profiles[&edited.id].odm_power_limits[0], 55);
    assert!(ok);
    assert_eq!((active.id.as_str(), active.odm_power_limits[0], actual), (edited.id.as_str(), 55, 55));
}

#[tokio::test]
async fn followup_05_drm_resolution_only_data_does_not_claim_refresh_rate_or_active_mode() {
    let root = tempfile::tempdir().unwrap();
    let connector = root.path().join("sys/class/drm/card0-eDP-1");
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |s| {
        s.set_file(connector.join("status"), "connected\n");
        s.set_file(connector.join("enabled"), "enabled\n");
        s.set_file(connector.join("modes"), "2560x1600\n2560x1600\n1920x1200\n");
    }, |p| p);
    let name = "org.test.uccd.reg.followup_05";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let modes: serde_json::Value =
        serde_json::from_str(&proxy.get_display_modes_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(modes["displayName"], "eDP-1");
    assert_eq!(modes["activeMode"]["xResolution"], 0);
    assert_eq!(modes["activeMode"]["refreshRates"], serde_json::json!([]));
    assert_eq!(modes["displayModes"].as_array().unwrap().len(), 2);
    assert!(modes["displayModes"].as_array().unwrap().iter().all(|m| m["refreshRates"] == serde_json::json!([])));
}

#[tokio::test]
async fn followup_06_configured_ycbcr_workaround_uses_driver_debugfs_node() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = serde_json::to_value(create_default_settings()).unwrap();
    cfg["ycbcr420Workaround"] = serde_json::json!([{"HDMI-A-1": false}, {"DP-1": true}]);
    let cfg: ucc_config::UccSettings = serde_json::from_value(cfg).unwrap();
    ConfigManager::with_custom_path(root.path().join("settings.json")).save(&cfg).unwrap();
    let (backend, _, sysfs, _) = build_custom_backend(root.path(), true, |s| {
        s.set_file(root.path().join("sys/kernel/debug/dri/0/eDP-1/force_yuv420_output"), "0\n");
    }, |p| p);
    let name = "org.test.uccd.reg.followup_06";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    // Neither configured HDMI-A-1 on card 0 nor DP-1 on card 1 exists yet, even though unconfigured eDP-1 does.
    assert!(!proxy.get_force_yuv420_output_switch_available().await.unwrap());
    sysfs.set_file(root.path().join("sys/kernel/debug/dri/1/DP-1/force_yuv420_output"), "0\n");
    assert!(proxy.get_force_yuv420_output_switch_available().await.unwrap());

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn followup_07_second_integrated_pci_gpu_does_not_fill_discrete_slot() {
    let root = tempfile::tempdir().unwrap();
    let pci = root.path().join("sys/bus/pci/devices");
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |s| {
        for (bdf, product) in [("0000:00:02.0", "Integrated One"), ("0001:00:02.0", "Integrated Two")] {
            let dev = pci.join(bdf);
            s.set_file(dev.join("class"), "0x030000");
            s.set_file(dev.join("vendor"), "0x8086");
            s.set_file(dev.join("product_name"), product);
        }
    }, |p| p);
    let name = "org.test.uccd.reg.followup_07";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let info: serde_json::Value =
        serde_json::from_str(&proxy.get_system_info_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(info["iGpuModel"], "Integrated One");
    assert_eq!(info["dGpuModel"], "");
}

#[tokio::test]
async fn m8_outcomes_odm_read_error_after_preflight_restores_baseline() {
    use std::sync::atomic::Ordering::SeqCst;
    let root = tempfile::tempdir().unwrap();
    let mut target = handoff_profile_with_tdps("new-odm-mode", [55, 80, 95]);
    target.odm_power_limits.clear();
    target.odm_profile_name = Some("performance".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = Arc::new(TxRegressionPlatform::new(p));
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.outcomes_odm_disappeared";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    ctl.odm_read_error_at.store(2, SeqCst);
    ctl.odm_fail.store(true, SeqCst);
    let ok = proxy
        .apply_profile(&serde_json::to_string(&target).unwrap())
        .await
        .unwrap();
    let final_mode = platform.read_perf_profile().unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(final_mode, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_failed_reapply_of_superseded_edit_does_not_persist() {
    let root = tempfile::tempdir().unwrap();
    let original = handoff_profile_with_tdps("edited", [55, 80, 95]);
    let old_edit = handoff_profile_with_tdps("edited", [41, 70, 90]);
    let new_edit = handoff_profile_with_tdps("edited", [62, 82, 105]);
    handoff_save_profiles(root.path(), &[&original]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.reject_second_41 = true;
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.reg.outcomes_failed_reapply";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_active_profile(&original.id).await.unwrap());
    let pending = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .update_custom_profile(&serde_json::to_string(&old_edit).unwrap())
            .await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&new_edit).unwrap())
        .await
        .unwrap());
    release.send(()).unwrap();
    let ok = pending.await.unwrap().unwrap();
    let active_before: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let disk_before = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let hardware_before = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;

    let (restarted, restarted_hw, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let runtime = start_runtime(restarted, name, false).await;
    let after = restarted_hw.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;

    assert!(!ok);
    assert_eq!(active_before.odm_power_limits[0], 62);
    assert_eq!(hardware_before, 62);
    assert_eq!(
        (disk_before.profiles[&original.id].odm_power_limits[0], after),
        (62, 62)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_stale_revision_rollback_does_not_overwrite_winner() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("initial", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    let replacement = handoff_profile_with_tdps("pending", [91, 95, 105]);
    let winner = handoff_profile_with_tdps("winner", [62, 82, 105]);
    handoff_save_profiles(root.path(), &[&initial, &pending, &winner]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (rollback_started, rollback_entered) = std::sync::mpsc::channel();
    let (release_rollback, rollback_wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = Arc::new(TxRegressionPlatform::new(p));
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.outcomes_stale_undo";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_active_profile(&initial.id).await.unwrap());
    let setting = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .set_active_profile(&pending.id)
            .await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&replacement).unwrap())
        .await
        .unwrap());
    *ctl.stale_rollback_gate.lock().unwrap() = Some((rollback_started, rollback_wait));
    release.send(()).unwrap();
    handoff_wait_gate(rollback_entered).await;
    assert!(proxy.set_active_profile(&winner.id).await.unwrap());
    assert_eq!(platform.read_tdp(0).unwrap(), 62);
    release_rollback.send(()).unwrap();
    let losing_ok = setting.await.unwrap().unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let disk = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let actual = (0..3)
        .map(|i| platform.read_tdp(i).unwrap())
        .collect::<Vec<_>>();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!losing_ok);
    assert_eq!(active.id, winner.id);
    assert_eq!(disk.state_map["power_ac"], winner.id);
    assert_eq!(actual, [62, 82, 105]);
}

struct CpuRestoreGateSysfs {
    sysfs: Arc<MockSysfs>,
    read_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    restored: std::sync::mpsc::Sender<()>,
}

impl ucc_hardware::Sysfs for CpuRestoreGateSysfs {
    fn read(&self, path: &Path) -> std::io::Result<String> {
        if path.ends_with("no_turbo") {
            let gate = self.read_gate.lock().unwrap().take();
            if let Some((start, release)) = gate {
                start.send(()).unwrap();
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        self.sysfs.read(path)
    }
    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        self.sysfs.write(path, content)?;
        if path.ends_with("no_turbo") && content.trim() == "0" {
            let _ = self.restored.send(());
        }
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.sysfs.exists(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        self.sysfs.read_dir(path)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_no_new_cpu_restore_write_after_shutdown_deadline() {
    let root = tempfile::tempdir().unwrap();
    let nt = root.path().join("sys/devices/system/cpu/intel_pstate/no_turbo");
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (restored, completion) = std::sync::mpsc::channel();
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        root.path()
            .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        root.path().join("sys/class/power_supply/AC/type"),
        "Mains",
    );
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "1");
    sysfs.set_file(&nt, "0");
    let gated_sysfs: Arc<dyn ucc_hardware::Sysfs> = {
        let w = Arc::new(CpuRestoreGateSysfs {
            sysfs: sysfs.clone(),
            read_gate: std::sync::Mutex::new(None),
            restored,
        });
        tx.send(w.clone()).unwrap();
        w
    };
    let ctl = rx.recv().unwrap();
    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    let lighting = Arc::new(MockLightingTransport::new());
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform,
        sysfs: gated_sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(root.path().join("settings.json")),
        sys_root: root.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            root.path().join("sys"),
            root.path().join("proc"),
            ucc_hardware::default_process_runner(),
            root.path().join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));
    let name = "org.test.uccd.reg.outcomes_shutdown_cpu";
    let runtime = start_runtime(backend, name, false).await;
    sysfs.set_file(&nt, "1");
    *ctl.read_gate.lock().unwrap() = Some((started, wait));
    let shutdown = tokio::spawn(runtime.shutdown(Duration::from_millis(100)));
    handoff_wait_gate(entered).await;
    shutdown.await.unwrap();
    while completion.try_recv().is_ok() {}
    sysfs.clear_journal();
    release.send(()).unwrap();
    let wrote = tokio::task::spawn_blocking(move || {
        completion.recv_timeout(Duration::from_millis(500)).is_ok()
    })
    .await
    .unwrap();
    assert!(
        !wrote,
        "shutdown cleanup started new writes after budget expired: {:?}",
        sysfs.journal()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_power_worker_reapplies_edited_battery_profile() {
    let root = tempfile::tempdir().unwrap();
    let ac_profile = handoff_profile_with_tdps("ac", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("battery", [41, 70, 90]);
    let edited = handoff_profile_with_tdps("battery", [62, 82, 105]);
    let mut cfg = create_default_settings();
    for profile in [&ac_profile, &pending] {
        cfg.profiles.insert(profile.id.clone(), profile.clone());
    }
    cfg.set_active_profile_for_mode("power_ac", &ac_profile.id);
    cfg.set_active_profile_for_mode("power_bat", &pending.id);
    let config = root.path().join("settings.json");
    ConfigManager::with_custom_path(&config).save(&cfg).unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.reg.outcomes_worker_revision";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert_eq!(platform.read_tdp(0).unwrap(), 55);
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(
        root.path().join("sys/class/power_supply/BAT0/type"),
        "Battery",
    );
    let worker = tokio::task::spawn_blocking({
        let backend = backend.clone();
        move || backend.reconcile_power_state(false)
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&edited).unwrap())
        .await
        .unwrap());
    release.send(()).unwrap();
    worker.await.unwrap();
    backend.reconcile_power_state(false);
    let actual = platform.read_tdp(0).unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let saved = ConfigManager::load_from_path(&config).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(saved.profiles[&edited.id].odm_power_limits[0], 62);
    assert_eq!((active.odm_power_limits[0], actual), (62, 62));
}


#[tokio::test]
async fn failed_cpu_restore_is_carried_to_the_next_daemon() {
    let root = tempfile::tempdir().unwrap();
    let no_turbo = root.path().join("sys/devices/system/cpu/intel_pstate/no_turbo");
    let mut config = create_default_settings();
    config.cpu_settings_enabled = Some(false);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&config)
        .unwrap();
    let (first, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| s.set_file(&no_turbo, "0"),
        |p| p,
    );
    let name = "org.test.uccd.reg.failed_cpu_cleanup_handoff";
    let runtime = start_runtime(first, name, false).await;
    sysfs.set_file(&no_turbo, "1");
    sysfs.set_write_error(&no_turbo);
    runtime.shutdown(Duration::from_secs(2)).await;
    assert_eq!(sysfs.get_file(&no_turbo).as_deref(), Some("1"));

    sysfs.fail_writes.lock().unwrap().remove(&no_turbo);
    let (second, _, _, _) = build_custom_backend_with_sysfs(
        root.path(),
        true,
        Some(sysfs.clone()),
        |_| {},
        |p| p,
    );
    let runtime = start_runtime(second, name, false).await;
    let restored = sysfs.get_file(&no_turbo);
    runtime.shutdown(Duration::from_secs(2)).await;
    assert_eq!(restored.as_deref(), Some("0"));
}

#[tokio::test]
async fn m8_10_lid_close_blanks_and_restores_keyboard_and_wc_led_in_ram_only() {
    use ucc_dbus::UccdBackend;
    use ucc_hardware::{CoolerPacketEncoder, RgbState, RgbTarget};

    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, lighting) = build_test_backend(root.path());
    let name = "org.test.uccd.m8_10_lid_lighting";
    let runtime = start_runtime(Arc::clone(&backend), name, false).await;

    let initial_kb = json!([{
        "brightness": 180,
        "red": 12,
        "green": 140,
        "blue": 255,
        "mode": 0
    }])
    .to_string();
    assert!(backend.set_keyboard_backlight_states_json(&initial_kb));
    let before_settings = std::fs::read_to_string(root.path().join("settings.json")).unwrap();

    // Configure both lid blanking toggles and connect water cooler with an active LED color.
    let pd_both = json!({
        "enabled": false,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "screen_off",
            "type_c": "screen_off",
            "bat": "suspend",
            "wakeOnlyLidAndKeyboard": true,
            "turnOffKeyboardBacklightOnLidClose": true,
            "turnOffWaterCoolerLedOnLidClose": true
        }
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_both));
    backend.set_cooler_connected_state(true);
    assert!(backend.set_water_cooler_led_color(200, 30, 90, 0));
    let _ = drain_cooler_packets(&backend, 5).await;

    let after_cfg_settings = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
    let _ = before_settings;

    // 1. Close lid: keyboard brightness becomes 0 and WC LED turns off without altering settings.json.
    backend.on_lid_state_changed(true);
    assert!(backend.is_lid_closed());
    let kb_while_closed: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_while_closed[0]["brightness"].as_u64(), Some(0));
    assert_eq!(backend.cooler_desired_rgb(), RgbTarget::Off);
    let pkts = drain_cooler_packets(&backend, 5).await;
    assert!(
        pkts.contains(&CoolerPacketEncoder::turn_off(ucc_hardware::CMD_RGB)),
        "closing lid must enqueue TurnOffLed packet when WC LED blanking is enabled: {pkts:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("settings.json")).unwrap(),
        after_cfg_settings,
        "lid close blanking must live in RAM only and never mutate settings.json"
    );

    // 2. While lid stays closed, explicit LED/keyboard updates stay blanked on hardware but update RAM snapshot.
    let updated_kb = json!([{
        "brightness": 220,
        "red": 255,
        "green": 0,
        "blue": 100,
        "mode": 0
    }])
    .to_string();
    assert!(backend.set_keyboard_backlight_states_json(&updated_kb));
    let kb_still_dark: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_still_dark[0]["brightness"].as_u64(), Some(0));

    // 3. Reopen lid: previous keyboard states (updated RAM brightness 220) and WC LED color restore.
    backend.on_lid_state_changed(false);
    assert!(!backend.is_lid_closed());
    let kb_reopened: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_reopened[0]["brightness"].as_u64(), Some(220));
    assert_eq!(
        backend.cooler_desired_rgb(),
        RgbTarget::Color {
            red: 200,
            green: 30,
            blue: 90,
            state: RgbState::Static
        }
    );
    let _ = lighting;

    // 4. Disable both toggles: closing lid leaves keyboard & WC LED untouched.
    let pd_off = json!({
        "enabled": false,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "screen_off",
            "type_c": "screen_off",
            "bat": "suspend",
            "wakeOnlyLidAndKeyboard": true,
            "turnOffKeyboardBacklightOnLidClose": false,
            "turnOffWaterCoolerLedOnLidClose": false
        }
    })
    .to_string();
    assert!(backend.set_power_device_settings_json(&pd_off));
    backend.on_lid_state_changed(true);
    let kb_unchanged: Vec<serde_json::Value> =
        serde_json::from_str(&backend.get_keyboard_backlight_states_json()).unwrap();
    assert_eq!(kb_unchanged[0]["brightness"].as_u64(), Some(220));
    assert_ne!(backend.cooler_desired_rgb(), RgbTarget::Off);

    runtime.shutdown(Duration::from_secs(2)).await;
}

async fn drain_cooler_packets(backend: &LiveUccdBackend, max_rounds: usize) -> Vec<[u8; 8]> {
    let mut out = Vec::new();
    for _ in 0..max_rounds {
        tokio::time::sleep(Duration::from_millis(90)).await;
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

#[tokio::test]
async fn m5_9_startup_auto_enables_wc_scan_when_power_wc_profile_requests_it() {
    use ucc_dbus::UccdBackend;

    let root = tempfile::tempdir().unwrap();
    let mut ac_profile = default_high_performance_profile();
    ac_profile.id = "ac-no-wc".to_string();
    ac_profile.name = "AC No WC".to_string();
    ac_profile.fan.enable_water_cooler = false;

    let mut wc_profile = default_high_performance_profile();
    wc_profile.id = "wc-auto-scan".to_string();
    wc_profile.name = "WC Auto Scan".to_string();
    wc_profile.fan.enable_water_cooler = true;

    let mut settings = create_default_settings();
    settings.profiles.insert(ac_profile.id.clone(), ac_profile.clone());
    settings.profiles.insert(wc_profile.id.clone(), wc_profile.clone());
    settings.set_active_profile_for_mode("power_ac", ac_profile.id);
    settings.set_active_profile_for_mode("power_wc", wc_profile.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();

    let (backend, _, _, _) = build_test_backend(root.path());
    assert!(
        backend.is_water_cooler_enabled(),
        "WC scanning must auto-enable at startup when power_wc profile enables water cooler"
    );
}

#[tokio::test]
async fn m5_9_manual_wc_fan_and_pump_profile_apply_and_reconnect_replay() {
    use ucc_dbus::UccdBackend;
    use ucc_hardware::{CoolerPacketEncoder, PumpVoltage};

    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = start_runtime(Arc::clone(&backend), "org.test.uccd.m5_9_manual", false).await;

    let mut manual_profile = default_high_performance_profile();
    manual_profile.fan.enable_water_cooler = true;
    manual_profile.fan.auto_control_wc = false;
    manual_profile.fan.manual_fan_speed = Some(65);
    manual_profile.fan.manual_pump_voltage = Some(2); // discrete code 2 == 7V

    assert!(backend.apply_profile(&serde_json::to_string(&manual_profile).unwrap()));
    assert!(!backend.is_water_cooler_auto_control_enabled());
    assert_eq!(backend.get_water_cooler_fan_speed(), 65);
    assert_eq!(backend.get_water_cooler_pump_level(), 2);

    backend.set_cooler_connected_state(true);
    let pkts = drain_cooler_packets(&backend, 4).await;
    assert!(
        pkts.contains(&CoolerPacketEncoder::fan_speed(65).unwrap()),
        "manual fan speed 65% must be flushed on connect: {pkts:?}"
    );
    assert!(
        pkts.contains(&CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap()),
        "manual pump voltage V7 must be flushed on connect: {pkts:?}"
    );

    // Simulate transient BLE drop + debounced reconnect replay.
    backend.set_cooler_connected_state(false);
    backend.set_cooler_connected_state(true);
    backend.replay_active_cooler_profile();
    let replayed = drain_cooler_packets(&backend, 4).await;
    assert!(
        replayed.contains(&CoolerPacketEncoder::fan_speed(65).unwrap()),
        "manual fan speed 65% must replay after reconnect: {replayed:?}"
    );
    assert!(
        replayed.contains(&CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap()),
        "manual pump voltage V7 must replay after reconnect: {replayed:?}"
    );

    runtime.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn m5_9_auto_wc_curve_and_aquaris_led_restore_across_suspend_resume() {
    use ucc_core::{FanPoint, PumpPoint};
    use ucc_dbus::UccdBackend;
    use ucc_hardware::{CoolerPacketEncoder, PumpVoltage, RgbState, RgbTarget};

    let root = tempfile::tempdir().unwrap();
    let aquaris_path = root.path().join("aquaris.json");
    std::fs::write(
        &aquaris_path,
        json!({
            "address": "AA:BB:CC:DD:EE:FF",
            "name": "LCT21001",
            "red": 10,
            "green": 180,
            "blue": 240,
            "ledMode": 1
        })
        .to_string(),
    )
    .unwrap();

    let (backend, _, _, _) = build_test_backend(root.path());
    backend.load_aquaris_saved_state_from(&aquaris_path);
    let runtime = start_runtime(Arc::clone(&backend), "org.test.uccd.m5_9_resume", false).await;

    let mut auto_profile = default_high_performance_profile();
    auto_profile.fan.enable_water_cooler = true;
    auto_profile.fan.auto_control_wc = true;
    auto_profile.fan.table_water_cooler_fan = vec![
        FanPoint { temp: 40, speed: 35 },
        FanPoint { temp: 60, speed: 75 },
        FanPoint { temp: 80, speed: 95 },
    ];
    auto_profile.fan.table_pump = vec![
        PumpPoint { temp: 40, speed: 1 }, // V7
        PumpPoint { temp: 60, speed: 2 }, // V8
        PumpPoint { temp: 80, speed: 3 }, // V11
    ];
    assert!(backend.apply_profile(&serde_json::to_string(&auto_profile).unwrap()));

    // Enter suspend: flushing must stay blocked even if targets were queued.
    backend.on_system_suspend();
    assert!(backend.drain_cooler_commands().is_empty());

    // Resume + reconnect: saved aquaris.json RGB and Auto curve targets (at 62C -> 77% fan? check interpolation) restore.
    backend.on_system_resume();
    backend.set_cooler_connected_state(true);

    assert_eq!(
        backend.cooler_desired_rgb(),
        RgbTarget::Color {
            red: 10,
            green: 180,
            blue: 240,
            state: RgbState::Breathe,
        }
    );
    let fan_pct = backend.get_water_cooler_fan_speed();
    assert!(
        (75..=95).contains(&fan_pct),
        "auto WC curve fan at 62C must be evaluated in 75..=95%, got {fan_pct}"
    );
    assert_eq!(
        backend.get_water_cooler_pump_level(),
        3,
        "auto WC pump curve at 62C must select discrete code 3 (8V)"
    );

    let pkts = drain_cooler_packets(&backend, 5).await;
    assert!(
        pkts.contains(&CoolerPacketEncoder::fan_speed(fan_pct as u8).unwrap()),
        "resumed auto WC fan speed packet missing: {pkts:?}"
    );
    assert!(
        pkts.contains(&CoolerPacketEncoder::pump_voltage(PumpVoltage::V8).unwrap()),
        "resumed auto WC pump packet missing: {pkts:?}"
    );
    assert!(
        pkts.contains(&CoolerPacketEncoder::rgb(10, 180, 240, RgbState::Breathe)),
        "saved aquaris.json LED packet must restore on resume reconnect: {pkts:?}"
    );

    runtime.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn full_27_01_sleep_state_survives_resume_until_daemon_consumes_it() {
    use std::os::unix::fs::MetadataExt;
    use ucc_dbus::UccdBackend;
    use uccd::{resume_sleep, SleepOperations, SleepState, SleepStateStore};

    struct StartSucceedsImmediateOps;
    impl SleepOperations for StartSucceedsImmediateOps {
        fn is_daemon_active(&self) -> Result<bool, String> {
            Ok(true)
        }
        fn capture_runtime_state(&self) -> Result<SleepState, String> {
            unreachable!()
        }
        fn stop_daemon(&self) -> Result<(), String> {
            unreachable!()
        }
        fn start_daemon(&self) -> Result<(), String> {
            Ok(())
        }
    }

    struct StartFailsOps;
    impl SleepOperations for StartFailsOps {
        fn is_daemon_active(&self) -> Result<bool, String> {
            Ok(true)
        }
        fn capture_runtime_state(&self) -> Result<SleepState, String> {
            unreachable!()
        }
        fn stop_daemon(&self) -> Result<(), String> {
            unreachable!()
        }
        fn start_daemon(&self) -> Result<(), String> {
            Err("simulated start failure".to_string())
        }
    }

    let root = tempfile::tempdir().unwrap();
    let run_ucc = root.path().join("run/ucc");
    std::fs::create_dir_all(&run_ucc).unwrap();
    let uid = std::fs::metadata(root.path()).unwrap().uid();
    let state_path = run_ucc.join("sleep-state.json");
    let store = SleepStateStore::new(&state_path, uid);

    // 1. Active snapshot survives resume_sleep even when start_daemon fails or returns immediately.
    let expected = SleepState {
        was_active: true,
        manual_mode: "type_c".to_string(),
        cooler_enabled: true,
    };
    store.save(&expected).unwrap();
    assert!(resume_sleep(&store, &StartFailsOps).is_err());
    assert_eq!(store.load().unwrap(), Some(expected.clone()));
    resume_sleep(&store, &StartSucceedsImmediateOps).unwrap();
    assert_eq!(store.load().unwrap(), Some(expected.clone()));
    // Repeated resume before daemon consumes it preserves the snapshot.
    resume_sleep(&store, &StartSucceedsImmediateOps).unwrap();
    assert_eq!(store.load().unwrap(), Some(expected));

    // 2. When DaemonRuntime starts, it consumes and applies the snapshot and clears the file only afterward.
    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = start_runtime(Arc::clone(&backend), "org.test.uccd.full_27_01", false).await;
    assert!(backend.is_water_cooler_enabled());
    let policy: serde_json::Value = serde_json::from_str(&backend.get_power_policy_json()).unwrap();
    assert_eq!(policy["manual_mode"], "type_c");
    assert_eq!(
        store.load().unwrap(),
        None,
        "snapshot must be cleared once daemon activation consumes it"
    );
    runtime.shutdown(Duration::from_secs(1)).await;

    // 3. Inactive snapshot (was_active=false) is cleared directly by resume_sleep.
    store
        .save(&SleepState {
            was_active: false,
            manual_mode: "bat".to_string(),
            cooler_enabled: false,
        })
        .unwrap();
    resume_sleep(&store, &StartSucceedsImmediateOps).unwrap();
    assert_eq!(store.load().unwrap(), None);
}
