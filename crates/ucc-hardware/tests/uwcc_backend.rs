// SPDX-License-Identifier: GPL-3.0-or-later

use std::mem::{align_of, size_of};
use std::sync::Arc;
use ucc_hardware::traits::{MockHardwareIo, MockSysfs};
use ucc_hardware::uwcc::ramfan::{flags as ramfan_flags, reg as ramfan_reg};
use ucc_hardware::uwcc::wmi_events::{linux_key, wmi_code};
use ucc_hardware::{
    decode_tachometer_rpm, parse_backend_selection, translate_wmi_event, uwcc_ec_reg, DmiInfo,
    MockPlatformBackend, MockUinputEmitter, MockUwccTransport, PlatformBackend,
    PlatformBackendKind, PlatformBackendSelection, PlatformError, PlatformWrite, RamFanController,
    RamFanCurve, RamFanPoint, SmrwPowerLimits, TuxedoIoBackend, UinputEmitter, UwccBackend,
    UwccEcReg, UwccHotkeyEvent, UwccInfo, UwccPwup, UwccSmrw, UwccTransportWrite, UwccWmiEvent,
    CHASSIS_PROJECT_0X1A, RAMFAN_POINTS, SMRW_POWER_LIMITS_OFFSET, UWCC_IOC_EC_READ,
    UWCC_IOC_EC_WRITE, UWCC_IOC_GET_INFO, UWCC_IOC_PWUP, UWCC_IOC_SMRW, UWCC_MAGIC,
    UWCC_NR_EC_READ, UWCC_NR_EC_WRITE, UWCC_NR_GET_INFO, UWCC_NR_PWUP, UWCC_NR_SMRW,
};

#[test]
fn uapi_structs_and_ioctl_abi_match_contract() {
    assert_eq!(UWCC_MAGIC, b'U');

    assert_eq!(size_of::<UwccEcReg>(), 4);
    assert_eq!(align_of::<UwccEcReg>(), 2);
    assert_eq!(size_of::<UwccSmrw>(), 68);
    assert_eq!(align_of::<UwccSmrw>(), 2);
    assert_eq!(size_of::<UwccPwup>(), 8);
    assert_eq!(align_of::<UwccPwup>(), 4);
    assert_eq!(size_of::<UwccInfo>(), 16);
    assert_eq!(align_of::<UwccInfo>(), 4);
    assert_eq!(size_of::<UwccWmiEvent>(), 16);
    assert_eq!(align_of::<UwccWmiEvent>(), 8);

    // Verify Linux ioctl encoding bitfields: (dir << 30) | (size << 16) | (type << 8) | nr
    let decode = |ioc: u64| -> (u64, u64, u64, u64) {
        let nr = ioc & 0xFF;
        let ty = (ioc >> 8) & 0xFF;
        let sz = (ioc >> 16) & 0x3FFF;
        let dir = (ioc >> 30) & 0x3;
        (dir, ty, nr, sz)
    };

    assert_eq!(
        decode(UWCC_IOC_GET_INFO),
        (2, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_GET_INFO), 16)
    );
    assert_eq!(
        decode(UWCC_IOC_EC_READ),
        (3, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_EC_READ), 4)
    );
    assert_eq!(
        decode(UWCC_IOC_EC_WRITE),
        (1, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_EC_WRITE), 4)
    );
    assert_eq!(
        decode(UWCC_IOC_SMRW),
        (3, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_SMRW), 68)
    );
    assert_eq!(
        decode(UWCC_IOC_PWUP),
        (1, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_PWUP), 8)
    );
}

#[test]
fn uwcc_backend_chassis_0x1a_smrw_limits_and_tdp_bounds() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let custom_limits = SmrwPowerLimits {
        pl1_min_w: 18,
        pl1_max_w: 85,
        pl2_min_w: 25,
        pl2_max_w: 120,
        pl4_min_w: 30,
        pl4_max_w: 150,
        ctgp_min_w: 60,
        ctgp_max_w: 140,
        dynamic_boost_min_w: 0,
        dynamic_boost_max_w: 25,
    };
    transport.set_smrw_table(SMRW_POWER_LIMITS_OFFSET, custom_limits.encode());

    let dmi = DmiInfo {
        sys_vendor: "MECHREVO".into(),
        board_name: "YAOSHI Series-X6AR55xY".into(),
        ..Default::default()
    };

    let backend = UwccBackend::new(Arc::clone(&transport) as _, Some(&dmi), None);
    assert_eq!(backend.kind(), PlatformBackendKind::Uwcc);
    assert_eq!(backend.info().project_id, CHASSIS_PROJECT_0X1A);
    assert_eq!(backend.smrw_limits(), custom_limits);

    assert_eq!(backend.read_tdp_limits(0).unwrap(), (18, 85));
    assert_eq!(backend.read_tdp_limits(1).unwrap(), (25, 120));
    assert_eq!(backend.read_tdp_limits(2).unwrap(), (30, 150));
    assert_eq!(backend.read_ctgp_limits(), (60, 140));
    assert_eq!(backend.read_dynamic_boost_limits(), (0, 25));

    backend.set_tdp(0, 65).unwrap();
    assert_eq!(backend.read_tdp(0).unwrap(), 65);
    assert!(matches!(
        backend.set_tdp(0, 95),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_tdp(3, 50),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_ctgp(115).unwrap();
    assert_eq!(transport.get_reg(uwcc_ec_reg::CTGP_WATTS), Some(115));
    assert!(matches!(
        backend.set_ctgp(175),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_dynamic_boost(15).unwrap();
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::DYNAMIC_BOOST_WATTS),
        Some(15)
    );
    assert!(matches!(
        backend.set_dynamic_boost(30),
        Err(PlatformError::InvalidArgument(_))
    ));
}

#[test]
fn uwcc_backend_reads_real_rpm_duty_and_ec_temperatures() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    // CPU RPM = 3250 (0x0CB2): 0x464 = 0x0C, 0x465 = 0xB2
    transport.set_reg(uwcc_ec_reg::CPU_FAN_RPM_HI, 0x0C);
    transport.set_reg(uwcc_ec_reg::CPU_FAN_RPM_LO, 0xB2);
    // GPU RPM = 2900 (0x0B54): 0x46C = 0x0B, 0x46B = 0x54
    transport.set_reg(uwcc_ec_reg::GPU_FAN_RPM_HI, 0x0B);
    transport.set_reg(uwcc_ec_reg::GPU_FAN_RPM_LO, 0x54);
    // Duty readback 0x75B/0x75C (0..=200 raw)
    transport.set_reg(uwcc_ec_reg::CPU_FAN_DUTY_READ, 120);
    transport.set_reg(uwcc_ec_reg::GPU_FAN_DUTY_READ, 90);
    // EC temps 0x043E / 0x044F
    transport.set_reg(uwcc_ec_reg::CPU_TEMP_C, 67);
    transport.set_reg(uwcc_ec_reg::GPU_TEMP_C, 59);

    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    assert_eq!(backend.read_fan_rpm(0).unwrap(), 3250);
    assert_eq!(backend.read_fan_rpm(1).unwrap(), 2900);
    assert_eq!(backend.read_fan_speed_percent(0).unwrap(), 60);
    assert_eq!(backend.read_fan_speed_percent(1).unwrap(), 45);
    assert_eq!(backend.read_fan_temp(0).unwrap(), 67);
    assert_eq!(backend.read_fan_temp(1).unwrap(), 59);

    // Out-of-bounds tachometer reading (> 15000 RPM) is rejected fail-closed
    assert!(matches!(
        decode_tachometer_rpm(0xFF, 0xFF),
        Err(PlatformError::InvalidArgument(_))
    ));

    // Zero temperature reports SensorUnavailable
    transport.set_reg(uwcc_ec_reg::GPU_TEMP_C, 0);
    assert!(matches!(
        backend.read_fan_temp(1),
        Err(PlatformError::SensorUnavailable(1))
    ));
}

#[test]
fn uwcc_backend_tcc_offset_and_pwup() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);

    backend.set_tcc_offset(15).unwrap();
    assert_eq!(backend.read_tcc_offset().unwrap(), 15);
    assert_eq!(transport.get_reg(uwcc_ec_reg::TCC_OFFSET_C), Some(15));

    backend.set_tcc_offset(63).unwrap();
    assert_eq!(backend.read_tcc_offset().unwrap(), 63);
    assert!(matches!(
        backend.set_tcc_offset(64),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_pwup(true, 2).unwrap();
    backend.set_pwup(false, 0).unwrap();
    assert!(matches!(
        backend.set_pwup(false, 1),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_pwup(true, 4),
        Err(PlatformError::InvalidArgument(_))
    ));

    let journal = transport.journal();
    assert!(journal.contains(&UwccTransportWrite::Pwup {
        enable: true,
        level: 2,
    }));
    assert!(journal.contains(&UwccTransportWrite::Pwup {
        enable: false,
        level: 0,
    }));
}

#[test]
fn ramfan_15_encodes_16_point_curve_sets_cume_ufme_and_restores_on_drop() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let curve = RamFanCurve::balanced_default();

    {
        let controller = RamFanController::new(Arc::clone(&transport) as _);
        controller.apply_curve(&curve).unwrap();
        assert!(controller.is_custom_active());

        for i in 0..RAMFAN_POINTS {
            let offset = i as u16;
            assert_eq!(
                transport.get_reg(ramfan_reg::CPU_TEMP_BASE + offset),
                Some(curve.cpu[i].temp_c)
            );
            assert_eq!(
                transport.get_reg(ramfan_reg::CPU_DUTY_BASE + offset),
                Some(curve.cpu[i].duty_percent * 2)
            );
            assert_eq!(
                transport.get_reg(ramfan_reg::GPU_TEMP_BASE + offset),
                Some(curve.gpu[i].temp_c)
            );
            assert_eq!(
                transport.get_reg(ramfan_reg::GPU_DUTY_BASE + offset),
                Some(curve.gpu[i].duty_percent * 2)
            );
        }

        assert_eq!(
            transport.get_reg(ramfan_reg::CTRL),
            Some(ramfan_flags::CUME | ramfan_flags::UFME)
        );
        assert_eq!(transport.get_reg(ramfan_reg::COMMIT), Some(0x01));

        controller.set_fan_boost(true).unwrap();
        assert_eq!(
            transport.get_reg(ramfan_reg::CTRL),
            Some(ramfan_flags::CUME | ramfan_flags::UFME | ramfan_flags::FAN_BOOST)
        );
        // Dropping `controller` must automatically restore firmware auto mode!
    }

    assert_eq!(transport.get_reg(ramfan_reg::CTRL), Some(0x00));
    assert_eq!(transport.get_reg(ramfan_reg::COMMIT), Some(0x00));

    // Non-monotonic curve must be rejected before writing EC RAM
    let mut bad_curve = RamFanCurve::balanced_default();
    bad_curve.cpu[5] = RamFanPoint::new(bad_curve.cpu[4].temp_c - 1, bad_curve.cpu[4].duty_percent);
    assert!(matches!(
        bad_curve.validate(),
        Err(PlatformError::InvalidArgument(_))
    ));

    // Explicit `set_fans_auto` on `UwccBackend` also restores RamFan 1.5 and manual mode
    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    backend.apply_ramfan_curve(&curve).unwrap();
    backend.set_fans_auto().unwrap();
    assert_eq!(transport.get_reg(ramfan_reg::CTRL), Some(0x00));
    assert_eq!(transport.get_reg(uwcc_ec_reg::FAN_MANUAL_MODE), Some(0x00));
}

#[test]
fn uwcc_backend_charge_thresholds_ec_switches_and_lightbar_power() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);

    // Charging thresholds: valid 20 <= start < end <= 100
    backend.set_charge_thresholds(50, 80).unwrap();
    assert_eq!(backend.read_charge_thresholds().unwrap(), (50, 80));
    assert!(matches!(
        backend.set_charge_thresholds(80, 80),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_charge_thresholds(15, 80),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_charge_thresholds(60, 105),
        Err(PlatformError::InvalidArgument(_))
    ));

    // Fn Lock, Win Lock, USB sleep charging
    backend.set_fn_lock(true).unwrap();
    assert!(backend.read_fn_lock().unwrap());
    assert_eq!(transport.get_reg(uwcc_ec_reg::FN_LOCK), Some(1));

    backend.set_win_lock(true).unwrap();
    assert!(backend.read_win_lock().unwrap());
    assert_eq!(transport.get_reg(uwcc_ec_reg::WIN_LOCK), Some(1));

    backend.set_usb_sleep_charging(true).unwrap();
    assert!(backend.read_usb_sleep_charging().unwrap());
    assert_eq!(transport.get_reg(uwcc_ec_reg::USB_SLEEP_CHARGING), Some(1));

    // Lightbar EC power rails 0x767 / 0x768
    backend.set_lightbar_power(false).unwrap();
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::LIGHTBAR_POWER_PRIMARY),
        Some(0)
    );
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::LIGHTBAR_POWER_SECONDARY),
        Some(0)
    );
    backend.set_lightbar_power(true).unwrap();
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::LIGHTBAR_POWER_PRIMARY),
        Some(1)
    );
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::LIGHTBAR_POWER_SECONDARY),
        Some(1)
    );
}

#[test]
fn uwcc_backend_toggles_only_usb_uvc_rgb_ir_webcam_over_mock_sysfs() {
    let sysfs = Arc::new(MockSysfs::new());

    // Combined USB RGB+IR UVC webcam at 1-6 (interfaces 1-6:1.0 RGB, 1-6:1.2 IR both class 0e)
    sysfs.set_file("/sys/bus/usb/devices/1-6/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/1-6/idVendor", "04f2");
    sysfs.set_file("/sys/bus/usb/devices/1-6/idProduct", "b751");
    sysfs.set_file(
        "/sys/bus/usb/devices/1-6/product",
        "Integrated RGB+IR Camera",
    );
    sysfs.set_file("/sys/bus/usb/devices/1-6:1.0/bInterfaceClass", "0e");
    sysfs.set_file("/sys/bus/usb/devices/1-6:1.1/bInterfaceClass", "0e");
    sysfs.set_file("/sys/bus/usb/devices/1-6:1.2/bInterfaceClass", "0e");

    // Non-camera USB HID keyboard at 1-4 (class 03) — MUST NEVER be touched!
    sysfs.set_file("/sys/bus/usb/devices/1-4/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/1-4/idVendor", "048d");
    sysfs.set_file("/sys/bus/usb/devices/1-4/idProduct", "600b");
    sysfs.set_file("/sys/bus/usb/devices/1-4:1.0/bInterfaceClass", "03");

    // Root hub usb1 — MUST NEVER be touched!
    sysfs.set_file("/sys/bus/usb/devices/usb1/authorized", "1");

    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let backend = UwccBackend::new(
        Arc::clone(&transport) as _,
        None,
        Some(Arc::clone(&sysfs) as _),
    );

    assert!(backend.read_webcam().unwrap());
    backend.set_webcam(false).unwrap();

    // Simulate Linux `usb_deauthorize_device` unregistering child UVC interfaces from sysfs.
    sysfs.remove_file("/sys/bus/usb/devices/1-6:1.0/bInterfaceClass");
    sysfs.remove_file("/sys/bus/usb/devices/1-6:1.1/bInterfaceClass");
    sysfs.remove_file("/sys/bus/usb/devices/1-6:1.2/bInterfaceClass");

    assert!(!backend.read_webcam().unwrap());
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/1-6/authorized")
            .as_deref(),
        Some("0")
    );
    // Verify non-camera devices remain untouched
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/1-4/authorized")
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/usb1/authorized")
            .as_deref(),
        Some("1")
    );
    assert!(sysfs
        .journal()
        .iter()
        .all(|w| w.path.to_string_lossy().contains("1-6/authorized")));

    backend.set_webcam(true).unwrap();
    assert!(backend.read_webcam().unwrap());
}

#[test]
fn wmi_event_translation_and_mock_uinput_emission() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 100,
        code: wmi_code::FN_LOCK_TOGGLE,
        payload: 1,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 200,
        code: wmi_code::WIN_LOCK_TOGGLE,
        payload: 1,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 300,
        code: wmi_code::TOUCHPAD_TOGGLE,
        payload: 0,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 400,
        code: wmi_code::PERF_MODE_CYCLE,
        payload: 2,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 500,
        code: wmi_code::KBD_BRIGHTNESS_UP,
        payload: 0,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 600,
        code: wmi_code::KBD_BRIGHTNESS_DOWN,
        payload: 0,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 700,
        code: wmi_code::WEBCAM_TOGGLE,
        payload: 0,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 800,
        code: wmi_code::FAN_BOOST_TOGGLE,
        payload: 1,
    });
    transport.push_event(UwccWmiEvent {
        timestamp_ns: 900,
        code: 0xDEAD,
        payload: 0,
    });

    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    let emitter = MockUinputEmitter::new();

    let mut decoded = Vec::new();
    while let Some(raw_evt) = backend.poll_wmi_event().unwrap() {
        decoded.push(emitter.handle_wmi_event(raw_evt).unwrap());
    }

    assert_eq!(
        decoded,
        vec![
            UwccHotkeyEvent::FnLockToggle,
            UwccHotkeyEvent::WinLockToggle,
            UwccHotkeyEvent::TouchpadToggle,
            UwccHotkeyEvent::PerformanceModeCycle,
            UwccHotkeyEvent::KeyboardBrightnessUp,
            UwccHotkeyEvent::KeyboardBrightnessDown,
            UwccHotkeyEvent::WebcamToggle,
            UwccHotkeyEvent::FanBoostToggle,
            UwccHotkeyEvent::Unknown(0xDEAD),
        ]
    );
    assert_eq!(
        emitter.emitted_keys(),
        vec![
            linux_key::KEY_FN_ESC,
            linux_key::KEY_PROG3,
            linux_key::KEY_TOUCHPAD_TOGGLE,
            linux_key::KEY_PROG1,
            linux_key::KEY_KBDILLUMUP,
            linux_key::KEY_KBDILLUMDOWN,
            linux_key::KEY_CAMERA,
            linux_key::KEY_PROG2,
        ]
    );

    // Direct translation sanity check
    assert_eq!(
        translate_wmi_event(UwccWmiEvent {
            timestamp_ns: 0,
            code: 0x1234,
            payload: 0,
        }),
        UwccHotkeyEvent::Unknown(0x1234)
    );
}

#[test]
fn platform_backend_selection_parser_and_extended_mock_backend() {
    assert_eq!(
        parse_backend_selection("auto").unwrap(),
        PlatformBackendSelection::Auto
    );
    assert_eq!(
        parse_backend_selection(" UWCC ").unwrap(),
        PlatformBackendSelection::Uwcc
    );
    assert_eq!(
        parse_backend_selection("tuxedo_io").unwrap(),
        PlatformBackendSelection::TuxedoIo
    );
    assert_eq!(
        parse_backend_selection("tuxedo-io").unwrap(),
        PlatformBackendSelection::TuxedoIo
    );
    assert!(matches!(
        parse_backend_selection("invalid"),
        Err(PlatformError::InvalidArgument(_))
    ));

    // Extended MockPlatformBackend supports all new M14 optional methods + journal
    let mock = MockPlatformBackend::uniwill_default();
    mock.set_fan_rpm(0, 3100);
    assert_eq!(mock.read_fan_rpm(0).unwrap(), 3100);

    mock.set_tcc_offset(10).unwrap();
    assert_eq!(mock.read_tcc_offset().unwrap(), 10);

    mock.set_pwup(true, 1).unwrap();
    mock.set_lightbar_power(false).unwrap();
    assert!(!mock.mock_lightbar_power());

    mock.set_fn_lock(true).unwrap();
    assert!(mock.read_fn_lock().unwrap());
    mock.set_win_lock(true).unwrap();
    assert!(mock.read_win_lock().unwrap());
    mock.set_usb_sleep_charging(true).unwrap();
    assert!(mock.read_usb_sleep_charging().unwrap());
    mock.set_charge_thresholds(45, 85).unwrap();
    assert_eq!(mock.read_charge_thresholds().unwrap(), (45, 85));
    mock.set_fan_boost(true).unwrap();
    assert!(mock.mock_fan_boost());

    let curve = RamFanCurve::balanced_default();
    mock.apply_ramfan_curve(&curve).unwrap();
    assert_eq!(mock.mock_ramfan_curve(), Some(curve.clone()));

    mock.push_wmi_event(UwccWmiEvent {
        timestamp_ns: 42,
        code: wmi_code::WEBCAM_TOGGLE,
        payload: 1,
    });
    assert_eq!(
        mock.poll_wmi_event().unwrap().map(|e| e.code),
        Some(wmi_code::WEBCAM_TOGGLE)
    );

    let journal = mock.journal();
    assert!(journal.contains(&PlatformWrite::SetTccOffset(10)));
    assert!(journal.contains(&PlatformWrite::SetPwup {
        enable: true,
        level: 1,
    }));
    assert!(journal.contains(&PlatformWrite::SetLightbarPower(false)));
    assert!(journal.contains(&PlatformWrite::SetFnLock(true)));
    assert!(journal.contains(&PlatformWrite::SetWinLock(true)));
    assert!(journal.contains(&PlatformWrite::SetUsbSleepCharging(true)));
    assert!(journal.contains(&PlatformWrite::SetChargeThresholds { start: 45, end: 85 }));
    assert!(journal.contains(&PlatformWrite::SetFanBoost(true)));
    assert!(journal.contains(&PlatformWrite::ApplyRamFanCurve(Box::new(curve))));

    // Existing TuxedoIoBackend continues to compile and returns Unsupported for optional M14 hooks
    let tuxedo = TuxedoIoBackend::new(MockHardwareIo::new(), None, None);
    assert!(matches!(
        tuxedo.read_fan_rpm(0),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        tuxedo.read_tcc_offset(),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn uwcc_backend_dispatches_npcf_pwbt_pwdb_for_175w_nvidia_unlock_and_mux_switch() {
    use ucc_hardware::{
        MuxActiveGpu, MuxBootMode, UwccGpuPower, UwccMuxState, UWCC_FLAG_HAS_EC,
        UWCC_GPU_POWER_OP_BASE_TGP, UWCC_GPU_POWER_OP_DYNAMIC_BOOST, UWCC_MUX_ACTIVE_DGPU,
        UWCC_MUX_MODE_DGPU_ONLY,
    };

    assert_eq!(std::mem::size_of::<UwccGpuPower>(), 8);
    assert_eq!(std::mem::size_of::<UwccMuxState>(), 8);

    // Configure an RTX 5080 Mobile 175W (150W Base TGP + 25W Dynamic Boost) SMRW table.
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut rtx5080_limits = SmrwPowerLimits::chassis_0x1a_default();
    rtx5080_limits.ctgp_max_w = 150;
    rtx5080_limits.dynamic_boost_max_w = 25;
    transport.set_smrw_table(SMRW_POWER_LIMITS_OFFSET, rtx5080_limits.encode());

    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    backend.set_ctgp(150).unwrap();
    backend.set_dynamic_boost(25).unwrap();

    let journal = transport.journal();
    assert!(journal.contains(&UwccTransportWrite::GpuPower {
        op: UWCC_GPU_POWER_OP_BASE_TGP,
        value: 150,
    }));
    assert!(journal.contains(&UwccTransportWrite::GpuPower {
        op: UWCC_GPU_POWER_OP_DYNAMIC_BOOST,
        value: 25,
    }));

    // Verify hardware MUX read (MXDS/MXDM) and reboot-safe BIOS boot mode update (MXDM).
    let initial_mux = backend.read_mux_state().unwrap();
    assert_eq!(initial_mux.active_gpu, MuxActiveGpu::Igpu);
    assert_eq!(initial_mux.boot_mode, MuxBootMode::Dynamic);

    backend.set_mux_boot_mode(MuxBootMode::DgpuOnly).unwrap();
    assert!(transport.journal().contains(&UwccTransportWrite::MuxSet {
        boot_mode: UWCC_MUX_MODE_DGPU_ONLY,
    }));

    transport.set_mux_state(UwccMuxState {
        active_gpu: UWCC_MUX_ACTIVE_DGPU,
        boot_mode: UWCC_MUX_MODE_DGPU_ONLY,
    });
    let after_reboot_mux = backend.read_mux_state().unwrap();
    assert_eq!(after_reboot_mux.active_gpu, MuxActiveGpu::Dgpu);
    assert_eq!(after_reboot_mux.boot_mode, MuxBootMode::DgpuOnly);

    assert!(matches!(
        backend.set_mux_boot_mode(MuxBootMode::Unknown),
        Err(PlatformError::InvalidArgument(_))
    ));

    // Systems without UWCC_FLAG_HAS_MUX report Unsupported cleanly.
    let no_mux_transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut info = no_mux_transport.get_info().unwrap();
    info.flags = UWCC_FLAG_HAS_EC;
    no_mux_transport.set_info(info);
    let no_mux_backend = UwccBackend::new(no_mux_transport, None, None);
    assert!(matches!(
        no_mux_backend.read_mux_state(),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        no_mux_backend.set_mux_boot_mode(MuxBootMode::Dynamic),
        Err(PlatformError::Unsupported(_))
    ));
}
