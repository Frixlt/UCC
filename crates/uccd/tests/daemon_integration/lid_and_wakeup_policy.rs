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
    let kbd_states =
        json!([{ "brightness": 100, "red": 10, "green": 20, "blue": 255 }]).to_string();
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

    assert!(is_allowed_wakeup_device(
        &lid_dir,
        &dev_root,
        sysfs.as_ref()
    ));
    assert!(is_allowed_wakeup_device(
        &kbd_dir,
        &dev_root,
        sysfs.as_ref()
    ));
    assert!(!is_allowed_wakeup_device(
        &mouse_dir,
        &dev_root,
        sysfs.as_ref()
    ));
    assert!(!is_allowed_wakeup_device(
        &usb_dir,
        &dev_root,
        sysfs.as_ref()
    ));

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
    assert_eq!(
        res,
        WakeupApplyResult {
            enabled: 2,
            disabled: 3,
            failed: 0
        }
    );
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
    assert_eq!(acpi_writes, vec!["XHCI\n".to_string()]);
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
    let (backend, _, _, _) =
        build_custom_backend(root.path(), true, |_| {}, |p| Arc::new(SlowAutoPlatform(p)));
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
            s.set_file(
                cpufreq.join("energy_performance_preference"),
                "balance_power",
            );
            s.set_file(
                cpufreq.join("scaling_available_governors"),
                "performance powersave",
            );
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
    assert_eq!(
        sysfs.get_file(cpufreq.join("scaling_governor")).as_deref(),
        Some("performance")
    );
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
    proxy
        .set_sensor_data_collection_status(false)
        .await
        .unwrap();
    let before_pts = decode_history_binary(&proxy.get_monitor_data_since(0).await.unwrap())
        .unwrap()
        .into_iter()
        .find(|(m, _)| *m == MetricId::CpuTemp)
        .map(|(_, v)| v.len())
        .unwrap_or(0);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let after_pts = decode_history_binary(&proxy.get_monitor_data_since(0).await.unwrap())
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
    let cp_dir = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_profile");
    sysfs.set_file(
        cp_dir.join("charging_profiles_available"),
        "high_capacity balanced stationary",
    );
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
    assert_eq!(
        proxy2.get_current_charging_profile().await.unwrap(),
        "stationary"
    );
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
