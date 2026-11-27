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

    assert_eq!(
        GUID_PROCESSOR_PERF_ENERGY_PREFERENCE.data1,
        0x3668_7F9E
    );

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

        let handle = monitor.register(0x1234, DEVICE_NOTIFY_WINDOW_HANDLE).unwrap();
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
