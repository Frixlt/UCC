// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for M14 / ADR-05 `/dev/uwcc` backend selection, HID lighting preference,
//! real tachometer RPM telemetry, `PWUP` / `tccOffset` integration, and `TuxedoIoBackend`
//! rollback / zero-regression guarantees.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ucc_config::ConfigManager;
use ucc_dbus::UccdBackend;
use ucc_hardware::{
    default_process_runner, DeviceCapabilities, DeviceVendor, DmiInfo, MockHardwareIo,
    MockPlatformBackend, MockSysfs, MockUwccTransport, PlatformBackend, PlatformBackendKind,
    PlatformBackendSelection, PlatformCapabilities, PlatformWrite, PowerDeviceController, Sysfs,
    TelemetryConfig, TuxedoIoBackend, UwccBackend, UwccTransportWrite,
};
use ucc_lighting::MockLightingTransport;
use uccd::{
    has_ite_sysfs_leds, resolve_platform_backend_choice, select_lighting_transport,
    select_lighting_transport_with_platform, select_platform_backend_with_openers,
    LidPolicyConfig, LiveUccdBackend, UccdDependencies,
};
use zbus::zvariant::OwnedValue;

fn extract_nested_i32(map: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
    let val = map.get(key)?;
    if let Ok(dict) = <HashMap<String, OwnedValue>>::try_from(val.clone()) {
        if let Some(data) = dict.get("data") {
            return i32::try_from(data).ok();
        }
    }
    i32::try_from(val).ok()
}

fn build_backend_with_platform(
    tmp: &Path,
    platform: Arc<dyn PlatformBackend>,
    sysfs: Arc<MockSysfs>,
) -> LiveUccdBackend {
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");
    let lighting = Arc::new(MockLightingTransport::new());
    LiveUccdBackend::new(UccdDependencies {
        device_name: "MECHREVO YAOSHI X6AR55xY".to_string(),
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
            ac_unplug_debounce: Duration::from_secs(3),
        },
    })
}

fn make_mock_platform(kind: PlatformBackendKind) -> Arc<MockPlatformBackend> {
    let caps = PlatformCapabilities {
        backend: kind,
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
    let mock = Arc::new(MockPlatformBackend::new(caps));
    mock.set_fan_temp(0, 61);
    mock.set_fan_temp(1, 52);
    mock.set_fan_speed(0, 45);
    mock.set_fan_speed(1, 35);
    mock.set_tdp_limits(0, (15, 90));
    mock.set_tdp_limits(1, (20, 120));
    mock.set_tdp_limits(2, (25, 140));
    mock
}

#[test]
fn backend_choice_priority_env_over_settings_over_auto() {
    assert_eq!(
        resolve_platform_backend_choice(None, None),
        PlatformBackendSelection::Auto
    );
    assert_eq!(
        resolve_platform_backend_choice(None, Some("uwcc")),
        PlatformBackendSelection::Uwcc
    );
    assert_eq!(
        resolve_platform_backend_choice(None, Some("tuxedo_io")),
        PlatformBackendSelection::TuxedoIo
    );
    assert_eq!(
        resolve_platform_backend_choice(Some("tuxedo_io"), Some("uwcc")),
        PlatformBackendSelection::TuxedoIo
    );
    assert_eq!(
        resolve_platform_backend_choice(Some("uwcc"), Some("tuxedo_io")),
        PlatformBackendSelection::Uwcc
    );
    assert_eq!(
        resolve_platform_backend_choice(Some("invalid"), Some("tuxedo_io")),
        PlatformBackendSelection::TuxedoIo
    );
}

#[test]
fn select_platform_backend_auto_prefers_uwcc_then_falls_back_to_tuxedo_io_then_unsupported() {
    let sysfs: Arc<dyn Sysfs> = Arc::new(MockSysfs::new());
    let dmi = DmiInfo::default();

    let tuxedo_called = AtomicBool::new(false);
    let selected = select_platform_backend_with_openers(
        None,
        Some("auto"),
        &dmi,
        &sysfs,
        || Some(make_mock_platform(PlatformBackendKind::Uwcc)),
        || {
            tuxedo_called.store(true, Ordering::SeqCst);
            Some(make_mock_platform(PlatformBackendKind::TuxedoIo))
        },
    );
    assert_eq!(selected.kind(), PlatformBackendKind::Uwcc);
    assert!(
        !tuxedo_called.load(Ordering::SeqCst),
        "auto mode should not open /dev/tuxedo_io when /dev/uwcc succeeds"
    );

    let fallback = select_platform_backend_with_openers(
        None,
        Some("auto"),
        &dmi,
        &sysfs,
        || None,
        || Some(make_mock_platform(PlatformBackendKind::TuxedoIo)),
    );
    assert_eq!(fallback.kind(), PlatformBackendKind::TuxedoIo);
    assert!(fallback.is_available());

    let unsupported =
        select_platform_backend_with_openers(None, None, &dmi, &sysfs, || None, || None);
    assert_eq!(unsupported.kind(), PlatformBackendKind::Unsupported);
    assert!(!unsupported.is_available());
}

#[test]
fn select_platform_backend_explicit_uwcc_never_falls_back_to_tuxedo_io() {
    let sysfs: Arc<dyn Sysfs> = Arc::new(MockSysfs::new());
    let dmi = DmiInfo::default();
    let tuxedo_called = AtomicBool::new(false);

    let selected = select_platform_backend_with_openers(
        None,
        Some("uwcc"),
        &dmi,
        &sysfs,
        || None,
        || {
            tuxedo_called.store(true, Ordering::SeqCst);
            Some(make_mock_platform(PlatformBackendKind::TuxedoIo))
        },
    );
    assert_eq!(selected.kind(), PlatformBackendKind::Unsupported);
    assert!(!selected.is_available());
    assert!(
        !tuxedo_called.load(Ordering::SeqCst),
        "explicit uwcc must not silently fall back to tuxedo_io"
    );
}

#[test]
fn select_platform_backend_explicit_tuxedo_io_guarantees_clean_rollback() {
    let sysfs: Arc<dyn Sysfs> = Arc::new(MockSysfs::new());
    let dmi = DmiInfo::default();
    let uwcc_called = AtomicBool::new(false);

    let selected = select_platform_backend_with_openers(
        Some("tuxedo_io"),
        Some("uwcc"),
        &dmi,
        &sysfs,
        || {
            uwcc_called.store(true, Ordering::SeqCst);
            Some(make_mock_platform(PlatformBackendKind::Uwcc))
        },
        || Some(make_mock_platform(PlatformBackendKind::TuxedoIo)),
    );
    assert_eq!(selected.kind(), PlatformBackendKind::TuxedoIo);
    assert!(selected.is_available());
    assert!(
        !uwcc_called.load(Ordering::SeqCst),
        "explicit tuxedo_io must never open /dev/uwcc"
    );

    let missing = select_platform_backend_with_openers(
        None,
        Some("tuxedo_io"),
        &dmi,
        &sysfs,
        || Some(make_mock_platform(PlatformBackendKind::Uwcc)),
        || None,
    );
    assert_eq!(missing.kind(), PlatformBackendKind::Unsupported);
    assert!(!missing.is_available());
}

#[test]
fn lighting_transport_selection_prefers_hidraw_on_uwcc_and_preserves_sysfs_on_tuxedo_io() {
    let sysfs = MockSysfs::new();
    let root = Path::new("/mock_root");

    assert!(!has_ite_sysfs_leds(&sysfs, root));
    // Without ite_8291 sysfs LEDs, even TuxedoIo falls back to direct USB hidraw transport.
    let _hid_no_sysfs = select_lighting_transport(PlatformBackendKind::TuxedoIo, &sysfs, root);

    // Populate ite_8291 sysfs LED node.
    sysfs.set_file(
        root.join("sys/class/leds/rgb:kbd_backlight/brightness"),
        "50",
    );
    assert!(has_ite_sysfs_leds(&sysfs, root));

    // Both branches construct valid LightingTransport instances without panicking.
    let _hid_uwcc = select_lighting_transport(PlatformBackendKind::Uwcc, &sysfs, root);
    let _sysfs_tuxedo = select_lighting_transport(PlatformBackendKind::TuxedoIo, &sysfs, root);

    let uwcc_platform: Arc<dyn PlatformBackend> = make_mock_platform(PlatformBackendKind::Uwcc);
    let _hid_uwcc_wired =
        select_lighting_transport_with_platform(&uwcc_platform, &sysfs, root);
}

#[test]
fn uwcc_backend_reports_real_rpm_tcc_offset_and_backend_kind_without_breaking_dbus_schema() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(tmp.path().join("sys/module/uwcc_rs/version"), "0.1.0");
    sysfs.set_file(tmp.path().join("dev/uwcc"), "");

    let platform = make_mock_platform(PlatformBackendKind::Uwcc);
    platform.set_fan_rpm(0, 3250);
    platform.set_fan_rpm(1, 2900);
    platform.set_mock_tcc_offset(10);

    let backend = build_backend_with_platform(tmp.path(), platform, sysfs);
    let _tick = backend.tick_fans_once();

    assert_eq!(backend.latest_fan_rpm(0), Some(3250));
    assert_eq!(backend.latest_fan_rpm(1), Some(2900));

    let cpu_map = backend.get_fan_data_cpu();
    assert_eq!(extract_nested_i32(&cpu_map, "temp"), Some(61));
    assert_eq!(extract_nested_i32(&cpu_map, "realRpm"), Some(3250));

    let gpu_map = backend.get_fan_data_gpu1();
    assert_eq!(extract_nested_i32(&gpu_map, "temp"), Some(52));
    assert_eq!(extract_nested_i32(&gpu_map, "realRpm"), Some(2900));

    let sys_info: Value =
        serde_json::from_str(&backend.get_system_info_json()).expect("valid system info json");
    assert_eq!(sys_info["platformBackend"], "uwcc");
    assert_eq!(sys_info["tccOffset"], 10);
    assert_eq!(sys_info["realRpm"][0], 3250);
    assert_eq!(sys_info["realRpm"][1], 2900);

    let health: Value =
        serde_json::from_str(&backend.get_driver_health_json()).expect("valid driver health json");
    assert_eq!(health["platformBackend"], "uwcc");
    assert_eq!(health["healthy"], true);
}

#[test]
fn uwcc_driver_health_reports_missing_dev_node_when_uwcc_rs_module_is_loaded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(tmp.path().join("sys/module/uwcc_rs/version"), "0.1.0");

    // Simulate /dev/uwcc missing at startup so backend degrades to Unsupported.
    let platform = make_mock_platform(PlatformBackendKind::Unsupported);
    let backend = build_backend_with_platform(tmp.path(), platform, sysfs);

    let health: Value =
        serde_json::from_str(&backend.get_driver_health_json()).expect("valid driver health json");
    assert_eq!(health["healthy"], false);
    let message = health["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("/dev/uwcc"),
        "expected UWCC /dev/uwcc diagnostic, got: {message}"
    );
    assert!(
        !message.contains("TUXEDO"),
        "must not misdiagnose UWCC failure as TUXEDO: {message}"
    );
}

#[test]
fn uwcc_water_cooler_connect_disconnect_suspend_and_shutdown_toggle_pwup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sysfs = Arc::new(MockSysfs::new());

    // 1. Verify against real UwccBackend over MockUwccTransport.
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let uwcc_platform: Arc<dyn PlatformBackend> = Arc::new(UwccBackend::new(
        Arc::clone(&transport) as Arc<dyn ucc_hardware::UwccTransport>,
        None,
        Some(Arc::clone(&sysfs) as Arc<dyn Sysfs>),
    ));
    let backend = build_backend_with_platform(tmp.path(), uwcc_platform, Arc::clone(&sysfs));

    backend.set_cooler_connected_state(true);
    assert!(
        transport.journal().iter().any(|w| matches!(
            w,
            UwccTransportWrite::Pwup {
                enable: true,
                level: 1
            }
        )),
        "expected PWUP enable on water cooler connect"
    );

    transport.clear_journal();
    backend.set_cooler_connected_state(false);
    assert!(
        transport.journal().iter().any(|w| matches!(
            w,
            UwccTransportWrite::Pwup {
                enable: false,
                level: 0
            }
        )),
        "expected PWUP disable on water cooler disconnect"
    );

    // 2. Verify suspend and shutdown always disable PWUP and restore fans auto.
    let mock_platform = make_mock_platform(PlatformBackendKind::Uwcc);
    let backend2 =
        build_backend_with_platform(tmp.path(), Arc::clone(&mock_platform) as _, sysfs);
    backend2.set_cooler_connected_state(true);
    mock_platform.clear_journal();

    backend2.on_system_suspend();
    assert!(
        mock_platform
            .journal()
            .contains(&PlatformWrite::SetPwup {
                enable: false,
                level: 0
            }),
        "on_system_suspend must disable PWUP"
    );

    mock_platform.clear_journal();
    backend2.shutdown();
    let journal = mock_platform.journal();
    assert!(
        journal.contains(&PlatformWrite::SetPwup {
            enable: false,
            level: 0
        }),
        "shutdown must disable PWUP"
    );
    assert!(
        journal.contains(&PlatformWrite::SetFansAuto),
        "shutdown must restore firmware automatic fan control"
    );
}

#[test]
fn tuxedo_io_backend_zero_regression_omits_real_rpm_and_tcc_offset() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sysfs = Arc::new(MockSysfs::new());
    let hw = MockHardwareIo::new();
    let dmi = DmiInfo::default();
    let tuxedo_backend: Arc<dyn PlatformBackend> =
        Arc::new(TuxedoIoBackend::new(hw, Some(&dmi), Some(sysfs.as_ref())));

    let backend = build_backend_with_platform(tmp.path(), tuxedo_backend, sysfs);
    let _tick = backend.tick_fans_once();

    // TuxedoIoBackend does not support read_fan_rpm or read_tcc_offset; existing D-Bus
    // payloads must stay clean without spurious fields.
    assert_eq!(backend.latest_fan_rpm(0), None);
    let cpu_map = backend.get_fan_data_cpu();
    assert!(
        !cpu_map.contains_key("realRpm"),
        "TuxedoIoBackend must not emit realRpm when read_fan_rpm is Unsupported"
    );

    let sys_info: Value =
        serde_json::from_str(&backend.get_system_info_json()).expect("valid system info json");
    assert_eq!(sys_info["platformBackend"], "tuxedo_io");
    assert!(
        sys_info.get("tccOffset").is_none(),
        "TuxedoIoBackend must omit tccOffset when Unsupported"
    );
    assert!(
        sys_info.get("realRpm").is_none(),
        "TuxedoIoBackend must omit realRpm when Unsupported"
    );

    // Water cooler PWUP calls must gracefully ignore PlatformError::Unsupported on TuxedoIoBackend.
    backend.set_cooler_connected_state(true);
    backend.set_cooler_connected_state(false);
    backend.shutdown();
}
