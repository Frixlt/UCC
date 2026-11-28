// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::{Arc, Mutex};
use ucc_hardware::nvml::windows_nvml_candidate_paths;
use ucc_hardware::windows_display::{
    DisplayModeSpec, MockWindowsDisplayTransport, WindowsDisplayController, WindowsDisplayError,
    WindowsDisplayWrite,
};
use ucc_hardware::windows_lid::{
    parse_lid_power_broadcast, LidSwitchState, MockWindowsLidTransport, PowerBroadcastSetting,
    WindowsLidError, WindowsLidMonitor, DEVICE_NOTIFY_WINDOW_HANDLE, GUID_LIDSWITCH_STATE_CHANGE,
    GUID_LIDSWITCH_STATE_CHANGE_BYTES,
};
use ucc_hardware::windows_power::{
    epp_percent_to_str, epp_str_to_percent, BoostMode, MockWindowsPowerTransport,
    WindowsPowerController, WindowsPowerError, WindowsPowerSource, WindowsPowerWrite,
    GUID_PROCESSOR_FREQUENCY_MAXIMUM, GUID_PROCESSOR_PERF_BOOST_MODE,
    GUID_PROCESSOR_PERF_ENERGY_PREFERENCE, GUID_PROCESSOR_SETTINGS_SUBGROUP,
    GUID_PROCESSOR_THROTTLE_MAXIMUM, GUID_PROCESSOR_THROTTLE_MINIMUM,
};

#[test]
fn windows_nvml_candidate_paths_defaults_custom_and_dedup() {
    let defaults = windows_nvml_candidate_paths(None, None);
    assert_eq!(
        defaults,
        vec![
            r"C:\Windows\System32\nvml.dll".to_string(),
            r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll".to_string(),
            r"nvml.dll".to_string(),
        ]
    );

    let blank_fallback = windows_nvml_candidate_paths(Some("  "), Some(""));
    assert_eq!(blank_fallback, defaults);

    let custom = windows_nvml_candidate_paths(Some(r"D:\WinDir\"), Some(r"E:\Apps/"));
    assert_eq!(
        custom,
        vec![
            r"D:\WinDir\System32\nvml.dll".to_string(),
            r"E:\Apps\NVIDIA Corporation\NVSMI\nvml.dll".to_string(),
            r"nvml.dll".to_string(),
        ]
    );
}

#[test]
fn windows_power_epp_mapping_and_ac_dc_isolation() {
    assert_eq!(epp_str_to_percent("performance"), Ok(0));
    assert_eq!(epp_str_to_percent("balance_performance"), Ok(33));
    assert_eq!(epp_str_to_percent("balance_power"), Ok(66));
    assert_eq!(epp_str_to_percent("power"), Ok(100));
    assert!(epp_str_to_percent("unknown_epp").is_err());

    assert_eq!(epp_percent_to_str(0), Ok("performance"));
    assert_eq!(epp_percent_to_str(33), Ok("balance_performance"));
    assert_eq!(epp_percent_to_str(66), Ok("balance_power"));
    assert_eq!(epp_percent_to_str(100), Ok("power"));
    assert!(epp_percent_to_str(101).is_err());

    let transport = Arc::new(MockWindowsPowerTransport::new(true));
    let controller = WindowsPowerController::new(Arc::clone(&transport) as _);

    assert!(controller.is_available());
    assert_eq!(
        controller.read_epp_str(WindowsPowerSource::Ac),
        Ok("balance_performance")
    );
    assert_eq!(
        controller.read_epp_str(WindowsPowerSource::Dc),
        Ok("balance_power")
    );

    controller
        .set_epp_str(WindowsPowerSource::Ac, "performance")
        .unwrap();
    assert_eq!(controller.read_epp_percent(WindowsPowerSource::Ac), Ok(0));
    assert_eq!(controller.read_epp_percent(WindowsPowerSource::Dc), Ok(66));

    assert!(matches!(
        controller.set_epp_percent(WindowsPowerSource::Dc, 101),
        Err(WindowsPowerError::InvalidArgument(_))
    ));
}

#[test]
fn windows_power_throttle_bounds_safe_write_ordering_and_validation() {
    let transport = Arc::new(MockWindowsPowerTransport::new(true));
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MINIMUM,
        10,
    );
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MAXIMUM,
        50,
    );
    let controller = WindowsPowerController::new(Arc::clone(&transport) as _);

    // Raising min (60) above current max (50) must write MAXIMUM first, then MINIMUM.
    controller
        .set_throttle_bounds(WindowsPowerSource::Ac, 60, 80)
        .unwrap();
    assert_eq!(
        controller.read_throttle_bounds(WindowsPowerSource::Ac),
        Ok((60, 80))
    );
    assert_eq!(
        transport.journal(),
        vec![
            WindowsPowerWrite::WriteValue {
                source: WindowsPowerSource::Ac,
                subgroup: GUID_PROCESSOR_SETTINGS_SUBGROUP,
                setting: GUID_PROCESSOR_THROTTLE_MAXIMUM,
                value: 80,
            },
            WindowsPowerWrite::WriteValue {
                source: WindowsPowerSource::Ac,
                subgroup: GUID_PROCESSOR_SETTINGS_SUBGROUP,
                setting: GUID_PROCESSOR_THROTTLE_MINIMUM,
                value: 60,
            },
            WindowsPowerWrite::ApplyActiveScheme,
        ]
    );

    // Lowering max (40) below current min (60) must write MINIMUM first, then MAXIMUM.
    transport.clear_journal();
    controller
        .set_throttle_bounds(WindowsPowerSource::Ac, 10, 40)
        .unwrap();
    assert_eq!(
        controller.read_throttle_bounds(WindowsPowerSource::Ac),
        Ok((10, 40))
    );
    assert_eq!(
        transport.journal(),
        vec![
            WindowsPowerWrite::WriteValue {
                source: WindowsPowerSource::Ac,
                subgroup: GUID_PROCESSOR_SETTINGS_SUBGROUP,
                setting: GUID_PROCESSOR_THROTTLE_MINIMUM,
                value: 10,
            },
            WindowsPowerWrite::WriteValue {
                source: WindowsPowerSource::Ac,
                subgroup: GUID_PROCESSOR_SETTINGS_SUBGROUP,
                setting: GUID_PROCESSOR_THROTTLE_MAXIMUM,
                value: 40,
            },
            WindowsPowerWrite::ApplyActiveScheme,
        ]
    );

    // Invalid ranges rejected without touching transport.
    transport.clear_journal();
    assert!(matches!(
        controller.set_throttle_bounds(WindowsPowerSource::Ac, 70, 50),
        Err(WindowsPowerError::InvalidArgument(_))
    ));
    assert!(matches!(
        controller.set_throttle_bounds(WindowsPowerSource::Ac, 0, 101),
        Err(WindowsPowerError::InvalidArgument(_))
    ));
    assert!(transport.journal().is_empty());
}

#[test]
fn windows_power_frequency_boost_and_unavailability() {
    let transport = Arc::new(MockWindowsPowerTransport::new(true));
    let controller = WindowsPowerController::new(Arc::clone(&transport) as _);

    controller
        .set_max_frequency_mhz(WindowsPowerSource::Ac, 3400)
        .unwrap();
    assert_eq!(
        controller.read_max_frequency_mhz(WindowsPowerSource::Ac),
        Ok(3400)
    );
    assert_eq!(
        transport.get_value(
            WindowsPowerSource::Ac,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_FREQUENCY_MAXIMUM,
        ),
        Some(3400)
    );

    // 0 means unlimited.
    controller
        .set_max_frequency_mhz(WindowsPowerSource::Ac, 0)
        .unwrap();
    assert_eq!(
        controller.read_max_frequency_mhz(WindowsPowerSource::Ac),
        Ok(0)
    );

    controller
        .set_boost_mode(WindowsPowerSource::Dc, BoostMode::EfficientEnabled)
        .unwrap();
    assert_eq!(
        controller.read_boost_mode(WindowsPowerSource::Dc),
        Ok(BoostMode::EfficientEnabled)
    );
    assert_eq!(
        transport.get_value(
            WindowsPowerSource::Dc,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_PERF_BOOST_MODE,
        ),
        Some(3)
    );

    controller
        .set_turbo_enabled(WindowsPowerSource::Dc, false)
        .unwrap();
    assert_eq!(
        controller.read_boost_mode(WindowsPowerSource::Dc),
        Ok(BoostMode::Disabled)
    );
    controller
        .set_turbo_enabled(WindowsPowerSource::Dc, true)
        .unwrap();
    assert_eq!(
        controller.read_boost_mode(WindowsPowerSource::Dc),
        Ok(BoostMode::Aggressive)
    );

    assert_eq!(GUID_PROCESSOR_PERF_ENERGY_PREFERENCE.data1, 0x3668_7F9E);

    transport.set_available(false);
    assert!(!controller.is_available());
    assert_eq!(
        controller.read_epp_percent(WindowsPowerSource::Ac),
        Err(WindowsPowerError::Unavailable)
    );
}

#[test]
fn windows_display_refresh_rate_switching_and_brightness() {
    let current = DisplayModeSpec {
        width: 2560,
        height: 1600,
        refresh_hz: 165,
        bits_per_pel: 32,
    };
    let modes = vec![
        DisplayModeSpec {
            width: 2560,
            height: 1600,
            refresh_hz: 60,
            bits_per_pel: 32,
        },
        DisplayModeSpec {
            width: 2560,
            height: 1600,
            refresh_hz: 165,
            bits_per_pel: 32,
        },
        DisplayModeSpec {
            width: 1920,
            height: 1080,
            refresh_hz: 240,
            bits_per_pel: 32,
        },
        DisplayModeSpec {
            width: 2560,
            height: 1600,
            refresh_hz: 120,
            bits_per_pel: 16,
        },
    ];

    let transport = Arc::new(MockWindowsDisplayTransport::new(current, modes));
    let controller = WindowsDisplayController::new(Arc::clone(&transport) as _);

    assert!(controller.is_available());
    assert_eq!(controller.supported_refresh_rates(None), Ok(vec![60, 165]));

    // Switching to supported 60 Hz preserves resolution and color depth.
    let updated = controller.set_refresh_rate(None, 60).unwrap();
    assert_eq!(
        updated,
        DisplayModeSpec {
            width: 2560,
            height: 1600,
            refresh_hz: 60,
            bits_per_pel: 32,
        }
    );
    assert_eq!(controller.current_mode(None), Ok(updated));

    // 240 Hz is only available at 1920x1080; 120 Hz is only available at 16bpp -> both rejected.
    assert_eq!(
        controller.set_refresh_rate(None, 240),
        Err(WindowsDisplayError::UnsupportedRefreshRate {
            requested_hz: 240,
            width: 2560,
            height: 1600,
            bits_per_pel: 32,
        })
    );
    assert_eq!(
        controller.set_refresh_rate(None, 120),
        Err(WindowsDisplayError::UnsupportedRefreshRate {
            requested_hz: 120,
            width: 2560,
            height: 1600,
            bits_per_pel: 32,
        })
    );
    assert!(matches!(
        controller.set_refresh_rate(None, 0),
        Err(WindowsDisplayError::InvalidArgument(_))
    ));

    // Brightness validation and clamping.
    assert_eq!(controller.read_brightness(), Ok(80));
    controller.set_brightness(45).unwrap();
    assert_eq!(controller.read_brightness(), Ok(45));
    assert!(matches!(
        controller.set_brightness(101),
        Err(WindowsDisplayError::InvalidArgument(_))
    ));
    assert_eq!(controller.set_brightness_clamped(150), Ok(100));
    assert_eq!(controller.read_brightness(), Ok(100));

    assert_eq!(
        transport.journal(),
        vec![
            WindowsDisplayWrite::SetRefreshRate {
                device_name: None,
                refresh_hz: 60,
            },
            WindowsDisplayWrite::SetBrightness { percent: 45 },
            WindowsDisplayWrite::SetBrightness { percent: 100 },
        ]
    );

    transport.set_available(false);
    assert!(!controller.is_available());
    assert_eq!(
        controller.read_brightness(),
        Err(WindowsDisplayError::Unavailable)
    );
}

#[test]
fn windows_lid_parser_lifecycle_and_deduplicated_callbacks() {
    assert_eq!(
        GUID_LIDSWITCH_STATE_CHANGE.to_le_bytes(),
        GUID_LIDSWITCH_STATE_CHANGE_BYTES
    );

    // Binary parser for DWORD payloads.
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &0u32.to_le_bytes()),
        Some(LidSwitchState::Closed)
    );
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &1u32.to_le_bytes()),
        Some(LidSwitchState::Open)
    );
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &2u32.to_le_bytes()),
        None
    );
    assert_eq!(
        parse_lid_power_broadcast(&[0u8; 16], &1u32.to_le_bytes()),
        None
    );
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[1, 0]),
        None
    );

    // Full POWERBROADCAST_SETTING frame parsing.
    let mut raw_open = Vec::new();
    raw_open.extend_from_slice(&GUID_LIDSWITCH_STATE_CHANGE_BYTES);
    raw_open.extend_from_slice(&4u32.to_le_bytes());
    raw_open.extend_from_slice(&1u32.to_le_bytes());
    let parsed = PowerBroadcastSetting::parse(&raw_open).unwrap();
    assert_eq!(parsed.data_length, 4);
    assert_eq!(parsed.as_lid_state(), Some(LidSwitchState::Open));
    assert!(PowerBroadcastSetting::parse(&raw_open[..19]).is_none());
    assert!(PowerBroadcastSetting::parse(&raw_open[..22]).is_none());

    // Registration lifecycle, deduplicated callbacks, and RAII unregister on Drop.
    let transport = Arc::new(MockWindowsLidTransport::new(true));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let registered_handle = {
        let monitor = WindowsLidMonitor::new(Arc::clone(&transport) as _);
        let observed_cb = Arc::clone(&observed);
        monitor.on_state_change(move |state| {
            observed_cb
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(state);
        });

        let handle = monitor
            .register(0x1234, DEVICE_NOTIFY_WINDOW_HANDLE)
            .unwrap();
        assert!(monitor.is_registered());
        assert_eq!(monitor.handle(), Some(handle));
        assert_eq!(
            monitor.register(0x1234, DEVICE_NOTIFY_WINDOW_HANDLE),
            Err(WindowsLidError::AlreadyRegistered)
        );

        assert_eq!(
            monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &0u32.to_le_bytes()),
            Some(LidSwitchState::Closed)
        );
        // Duplicate Closed must be deduplicated.
        assert_eq!(
            monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &0u32.to_le_bytes()),
            None
        );
        assert_eq!(
            monitor.handle_raw_broadcast(&raw_open),
            Some(LidSwitchState::Open)
        );
        // Duplicate Open must be deduplicated.
        assert_eq!(monitor.handle_raw_broadcast(&raw_open), None);
        assert_eq!(
            monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &0u32.to_le_bytes()),
            Some(LidSwitchState::Closed)
        );

        assert_eq!(monitor.current_state(), Some(LidSwitchState::Closed));
        assert_eq!(
            monitor.transitions(),
            vec![
                LidSwitchState::Closed,
                LidSwitchState::Open,
                LidSwitchState::Closed,
            ]
        );
        handle
    };

    // Monitor drop must have unregistered the handle via RAII.
    assert!(transport.active_handles().is_empty());
    assert_eq!(transport.unregistered_handles(), vec![registered_handle]);
    assert_eq!(
        *observed.lock().unwrap_or_else(|e| e.into_inner()),
        vec![
            LidSwitchState::Closed,
            LidSwitchState::Open,
            LidSwitchState::Closed,
        ]
    );
}

#[test]
fn windows_nvapi_candidate_path_uses_trusted_system32() {
    use ucc_hardware::nvml::windows_nvapi_candidate_path;

    assert_eq!(
        windows_nvapi_candidate_path(None),
        r"C:\Windows\System32\nvapi64.dll"
    );
    assert_eq!(
        windows_nvapi_candidate_path(Some("   ")),
        r"C:\Windows\System32\nvapi64.dll"
    );
    assert_eq!(
        windows_nvapi_candidate_path(Some(r"D:\WinOS\")),
        r"D:\WinOS\System32\nvapi64.dll"
    );
}

#[test]
fn windows_power_error_paths_invalid_raw_and_write_failures() {
    use ucc_hardware::windows_power::{PowerGuid, WindowsPowerTransport};

    let transport = Arc::new(MockWindowsPowerTransport::new(true));
    let controller = WindowsPowerController::new(Arc::clone(&transport) as _);

    // Seed AC throttle bounds to (10, 50) so both write-ordering branches can be exercised.
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MINIMUM,
        10,
    );
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MAXIMUM,
        50,
    );

    // Inject write failures and verify all mutating operations fail closed without journaling.
    transport.set_fail_writes(true);
    assert_eq!(
        controller.set_throttle_bounds(WindowsPowerSource::Ac, 60, 80),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        controller.set_throttle_bounds(WindowsPowerSource::Ac, 5, 40),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        controller.set_epp_percent(WindowsPowerSource::Ac, 50),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        controller.set_epp_str(WindowsPowerSource::Dc, "power"),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        controller.set_max_frequency_mhz(WindowsPowerSource::Ac, 3200),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        controller.set_boost_mode(WindowsPowerSource::Ac, BoostMode::Enabled),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        controller.set_turbo_enabled(WindowsPowerSource::Dc, false),
        Err(WindowsPowerError::Unavailable)
    );
    assert_eq!(
        transport.apply_active_scheme(),
        Err(WindowsPowerError::Unavailable)
    );
    assert!(transport.journal().is_empty());
    assert_eq!(
        controller.read_throttle_bounds(WindowsPowerSource::Ac),
        Ok((10, 50))
    );

    // Verify apply_active_scheme failure propagates even when write_value_index succeeds.
    struct FailOnApplyTransport {
        inner: MockWindowsPowerTransport,
    }
    impl WindowsPowerTransport for FailOnApplyTransport {
        fn is_available(&self) -> bool {
            self.inner.is_available()
        }
        fn read_value_index(
            &self,
            source: WindowsPowerSource,
            subgroup: PowerGuid,
            setting: PowerGuid,
        ) -> Result<u32, WindowsPowerError> {
            self.inner.read_value_index(source, subgroup, setting)
        }
        fn write_value_index(
            &self,
            source: WindowsPowerSource,
            subgroup: PowerGuid,
            setting: PowerGuid,
            value: u32,
        ) -> Result<(), WindowsPowerError> {
            self.inner
                .write_value_index(source, subgroup, setting, value)
        }
        fn apply_active_scheme(&self) -> Result<(), WindowsPowerError> {
            Err(WindowsPowerError::Win32Error {
                api: "PowerSetActiveScheme",
                code: 5,
            })
        }
    }

    let apply_fail_controller = WindowsPowerController::new(Arc::new(FailOnApplyTransport {
        inner: MockWindowsPowerTransport::new(true),
    }));
    assert_eq!(
        apply_fail_controller.set_throttle_bounds(WindowsPowerSource::Ac, 20, 80),
        Err(WindowsPowerError::Win32Error {
            api: "PowerSetActiveScheme",
            code: 5,
        })
    );
    assert_eq!(
        apply_fail_controller.set_epp_percent(WindowsPowerSource::Ac, 25),
        Err(WindowsPowerError::Win32Error {
            api: "PowerSetActiveScheme",
            code: 5,
        })
    );

    // Invalid raw values seeded in mock transport must return Unsupported.
    transport.set_fail_writes(false);
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_PERF_ENERGY_PREFERENCE,
        101,
    );
    assert!(matches!(
        controller.read_epp_percent(WindowsPowerSource::Ac),
        Err(WindowsPowerError::Unsupported(_))
    ));
    assert!(matches!(
        controller.read_epp_str(WindowsPowerSource::Ac),
        Err(WindowsPowerError::Unsupported(_))
    ));

    // Invalid raw min throttle (> 100).
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MINIMUM,
        101,
    );
    assert!(matches!(
        controller.read_throttle_bounds(WindowsPowerSource::Ac),
        Err(WindowsPowerError::Unsupported(_))
    ));

    // Invalid raw max throttle (> 100) with valid min.
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MINIMUM,
        5,
    );
    transport.seed_value(
        WindowsPowerSource::Ac,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MAXIMUM,
        150,
    );
    assert!(matches!(
        controller.read_throttle_bounds(WindowsPowerSource::Ac),
        Err(WindowsPowerError::Unsupported(_))
    ));

    // Invalid raw BoostMode value (> 4).
    assert_eq!(BoostMode::from_raw(5), None);
    assert_eq!(BoostMode::from_raw(u32::MAX), None);
    transport.seed_value(
        WindowsPowerSource::Dc,
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_PERF_BOOST_MODE,
        9,
    );
    assert!(matches!(
        controller.read_boost_mode(WindowsPowerSource::Dc),
        Err(WindowsPowerError::Unsupported(_))
    ));

    // Unseeded setting lookup returns Unsupported.
    let unknown_guid = PowerGuid::new(0x1111_2222, 0x3333, 0x4444, [0; 8]);
    assert!(matches!(
        transport.read_value_index(
            WindowsPowerSource::Ac,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            unknown_guid,
        ),
        Err(WindowsPowerError::Unsupported(_))
    ));
}

#[test]
fn windows_display_brightness_clamping_and_refresh_error_paths() {
    let current = DisplayModeSpec {
        width: 1920,
        height: 1080,
        refresh_hz: 60,
        bits_per_pel: 32,
    };
    let modes = vec![
        DisplayModeSpec {
            width: 1920,
            height: 1080,
            refresh_hz: 144,
            bits_per_pel: 32,
        },
        DisplayModeSpec {
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            bits_per_pel: 32,
        },
        // Duplicate and zero-Hz entries must be deduplicated/filtered out.
        DisplayModeSpec {
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            bits_per_pel: 32,
        },
        DisplayModeSpec {
            width: 1920,
            height: 1080,
            refresh_hz: 0,
            bits_per_pel: 32,
        },
    ];

    let transport = Arc::new(MockWindowsDisplayTransport::new(current, modes));
    let controller = WindowsDisplayController::new(Arc::clone(&transport) as _);

    assert_eq!(
        controller.supported_refresh_rates(Some(r"\\.\DISPLAY1")),
        Ok(vec![60, 144])
    );

    // Out-of-range firmware brightness (> 100) is clamped on read.
    transport.seed_brightness(145);
    assert_eq!(controller.read_brightness(), Ok(100));
    transport.seed_brightness(0);
    assert_eq!(controller.read_brightness(), Ok(0));

    // Strict set_brightness rejects > 100 without touching transport.
    assert!(matches!(
        controller.set_brightness(101),
        Err(WindowsDisplayError::InvalidArgument(_))
    ));
    assert!(matches!(
        controller.set_brightness(u8::MAX),
        Err(WindowsDisplayError::InvalidArgument(_))
    ));
    assert!(transport.journal().is_empty());

    // Clamped brightness accepts > 100 and caps at 100.
    assert_eq!(controller.set_brightness_clamped(0), Ok(0));
    assert_eq!(controller.set_brightness_clamped(255), Ok(100));
    assert_eq!(controller.read_brightness(), Ok(100));

    // Refresh rate error paths: 0 Hz -> InvalidArgument, unsupported rate -> UnsupportedRefreshRate.
    assert!(matches!(
        controller.set_refresh_rate(Some(r"\\.\DISPLAY1"), 0),
        Err(WindowsDisplayError::InvalidArgument(_))
    ));
    assert_eq!(
        controller.set_refresh_rate(Some(r"\\.\DISPLAY1"), 120),
        Err(WindowsDisplayError::UnsupportedRefreshRate {
            requested_hz: 120,
            width: 1920,
            height: 1080,
            bits_per_pel: 32,
        })
    );

    // Write failures fail closed for both brightness and refresh rate without mutating state.
    transport.clear_journal();
    transport.set_fail_writes(true);
    assert_eq!(
        controller.set_brightness(50),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.set_brightness_clamped(50),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.set_refresh_rate(Some(r"\\.\DISPLAY1"), 144),
        Err(WindowsDisplayError::Unavailable)
    );
    assert!(transport.journal().is_empty());
    assert_eq!(controller.current_mode(None), Ok(current));

    // Clearing write failure allows named-device refresh switch.
    transport.set_fail_writes(false);
    let applied = controller
        .set_refresh_rate(Some(r"\\.\DISPLAY1"), 144)
        .unwrap();
    assert_eq!(applied.refresh_hz, 144);
    assert_eq!(
        transport.journal(),
        vec![WindowsDisplayWrite::SetRefreshRate {
            device_name: Some(r"\\.\DISPLAY1".to_string()),
            refresh_hz: 144,
        }]
    );

    // Unavailable transport fails closed across reads and writes.
    transport.set_available(false);
    assert_eq!(
        controller.current_mode(None),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.enumerate_modes(None),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.supported_refresh_rates(None),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.set_refresh_rate(None, 60),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.read_brightness(),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.set_brightness(50),
        Err(WindowsDisplayError::Unavailable)
    );
    assert_eq!(
        controller.set_brightness_clamped(50),
        Err(WindowsDisplayError::Unavailable)
    );

    // On Windows, also verify Win32DisplayTransport WMI brightness bridge delegation and clamping.
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicU8, Ordering};
        use ucc_hardware::windows_display::{
            Win32DisplayTransport, WindowsDisplayTransport, WmiBrightnessBridge,
        };

        struct StubWmiBrightness {
            level: AtomicU8,
        }
        impl WmiBrightnessBridge for StubWmiBrightness {
            fn read_wmi_brightness(&self) -> Result<u8, WindowsDisplayError> {
                Ok(self.level.load(Ordering::SeqCst))
            }
            fn write_wmi_brightness(&self, percent: u8) -> Result<(), WindowsDisplayError> {
                self.level.store(percent, Ordering::SeqCst);
                Ok(())
            }
        }

        let wmi_transport =
            Win32DisplayTransport::with_wmi_brightness(Box::new(StubWmiBrightness {
                level: AtomicU8::new(135),
            }));
        assert_eq!(wmi_transport.read_brightness(), Ok(100));
        wmi_transport.write_brightness(180).unwrap();
        assert_eq!(wmi_transport.read_brightness(), Ok(100));
        wmi_transport.write_brightness(42).unwrap();
        assert_eq!(wmi_transport.read_brightness(), Ok(42));
    }
}

#[test]
fn windows_lid_guid_wire_layout_and_event_transitions() {
    use std::mem::size_of;
    use ucc_hardware::windows_lid::{LidGuid, DEVICE_NOTIFY_SERVICE_HANDLE};
    use ucc_hardware::windows_power::PowerGuid;

    // Verify 16-byte repr(C) GUID wire layouts.
    assert_eq!(size_of::<LidGuid>(), 16);
    assert_eq!(size_of::<PowerGuid>(), 16);
    assert_eq!(DEVICE_NOTIFY_WINDOW_HANDLE, 0);
    assert_eq!(DEVICE_NOTIFY_SERVICE_HANDLE, 1);

    // {BA3E0F4D-B817-4094-A2D1-D56379E6A0F3} in Windows little-endian wire layout.
    let expected_lid_guid_bytes: [u8; 16] = [
        0x4D, 0x0F, 0x3E, 0xBA, 0x17, 0xB8, 0x94, 0x40, 0xA2, 0xD1, 0xD5, 0x63, 0x79, 0xE6, 0xA0,
        0xF3,
    ];
    assert_eq!(
        GUID_LIDSWITCH_STATE_CHANGE.to_le_bytes(),
        expected_lid_guid_bytes
    );
    assert_eq!(GUID_LIDSWITCH_STATE_CHANGE_BYTES, expected_lid_guid_bytes);

    // {54533251-82BE-4824-96C1-47B60B740D00} in Windows little-endian wire layout.
    let expected_proc_subgroup_bytes: [u8; 16] = [
        0x51, 0x32, 0x53, 0x54, 0xBE, 0x82, 0x24, 0x48, 0x96, 0xC1, 0x47, 0xB6, 0x0B, 0x74, 0x0D,
        0x00,
    ];
    assert_eq!(
        GUID_PROCESSOR_SETTINGS_SUBGROUP.to_le_bytes(),
        expected_proc_subgroup_bytes
    );

    // Raw DWORD and single-byte payload decoding including unknown states.
    assert_eq!(LidSwitchState::from_raw(0), Some(LidSwitchState::Closed));
    assert_eq!(LidSwitchState::from_raw(1), Some(LidSwitchState::Open));
    assert_eq!(LidSwitchState::from_raw(2), None);
    assert_eq!(LidSwitchState::from_raw(u32::MAX), None);
    assert_eq!(LidSwitchState::Closed.as_raw(), 0);
    assert_eq!(LidSwitchState::Open.as_raw(), 1);

    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[0]),
        Some(LidSwitchState::Closed)
    );
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[1]),
        Some(LidSwitchState::Open)
    );
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[2]),
        None
    );
    assert_eq!(
        parse_lid_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[]),
        None
    );

    // Registration error paths and explicit unregister.
    let transport = Arc::new(MockWindowsLidTransport::new(true));
    let monitor = WindowsLidMonitor::new(Arc::clone(&transport) as _);
    assert!(!monitor.is_registered());
    assert_eq!(monitor.current_state(), None);
    assert_eq!(monitor.unregister(), Err(WindowsLidError::NotRegistered));
    assert!(matches!(
        monitor.register(0, DEVICE_NOTIFY_WINDOW_HANDLE),
        Err(WindowsLidError::InvalidArgument(_))
    ));

    transport.set_fail_register(true);
    assert_eq!(
        monitor.register(0x5555, DEVICE_NOTIFY_SERVICE_HANDLE),
        Err(WindowsLidError::Unavailable)
    );
    transport.set_fail_register(false);

    let handle = monitor
        .register(0x5555, DEVICE_NOTIFY_SERVICE_HANDLE)
        .unwrap();
    assert!(monitor.is_registered());
    assert_eq!(monitor.handle(), Some(handle));

    // Event transitions: Open -> duplicate Open -> unknown/non-lid -> Closed -> duplicate Closed -> Open.
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_cb = Arc::clone(&observed);
    monitor.on_state_change(move |state| {
        observed_cb
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(state);
    });

    assert_eq!(
        monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &1u32.to_le_bytes()),
        Some(LidSwitchState::Open)
    );
    // Duplicate Open via 1-byte payload is suppressed.
    assert_eq!(
        monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[1]),
        None
    );
    // Unknown state code and unrelated GUID must not alter state or fire callbacks.
    assert_eq!(
        monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &99u32.to_le_bytes()),
        None
    );
    assert_eq!(
        monitor.handle_power_broadcast(&expected_proc_subgroup_bytes, &0u32.to_le_bytes()),
        None
    );
    assert_eq!(monitor.current_state(), Some(LidSwitchState::Open));

    // Close via 1-byte payload, then duplicate Close via 4-byte payload.
    assert_eq!(
        monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &[0]),
        Some(LidSwitchState::Closed)
    );
    assert_eq!(
        monitor.handle_power_broadcast(&GUID_LIDSWITCH_STATE_CHANGE_BYTES, &0u32.to_le_bytes()),
        None
    );

    // Open via raw POWERBROADCAST_SETTING frame with trailing padding bytes.
    let mut raw_open = Vec::new();
    raw_open.extend_from_slice(&GUID_LIDSWITCH_STATE_CHANGE_BYTES);
    raw_open.extend_from_slice(&4u32.to_le_bytes());
    raw_open.extend_from_slice(&1u32.to_le_bytes());
    raw_open.extend_from_slice(&[0xAA, 0xBB]);
    assert_eq!(
        monitor.handle_raw_broadcast(&raw_open),
        Some(LidSwitchState::Open)
    );

    let expected_seq = vec![
        LidSwitchState::Open,
        LidSwitchState::Closed,
        LidSwitchState::Open,
    ];
    assert_eq!(monitor.current_state(), Some(LidSwitchState::Open));
    assert_eq!(monitor.transitions(), expected_seq);
    assert_eq!(
        *observed.lock().unwrap_or_else(|e| e.into_inner()),
        expected_seq
    );

    monitor.unregister().unwrap();
    assert!(!monitor.is_registered());
    assert_eq!(monitor.handle(), None);
    assert!(transport.active_handles().is_empty());
    assert_eq!(transport.unregistered_handles(), vec![handle]);
}
