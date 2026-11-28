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
    // GPU RPM = 2900 (0x0B54): 0x46C = 0x0B, 0x46D = 0x54
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

    // EC 0x0743 is EC_ADDR_CTGP_DB_CTRL (0x07 on live X6AR55xY); set_tcc_offset must NEVER clobber it
    transport.set_reg(uwcc_ec_reg::TCC_OFFSET_C, 0x07);
    assert!(matches!(
        backend.set_tcc_offset(15),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        backend.read_tcc_offset(),
        Err(PlatformError::Unsupported(_))
    ));
    assert_eq!(transport.get_reg(uwcc_ec_reg::TCC_OFFSET_C), Some(0x07));
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

    // Charge threshold (0x07B9 EC_ADDR_CHARGE_CTRL, bits 6..=0) preserves bit 7
    transport.set_reg(uwcc_ec_reg::CHARGE_CTRL, 0x80);
    backend.set_charge_thresholds(50, 80).unwrap();
    assert_eq!(transport.get_reg(uwcc_ec_reg::CHARGE_CTRL), Some(0x80 | 80));
    assert_eq!(backend.read_charge_thresholds().unwrap(), (75, 80));
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

    // Fn Lock: RMW on BIT(3) of 0x07A4 preserves unrelated bits
    transport.set_reg(uwcc_ec_reg::FN_LOCK, 0x05);
    backend.set_fn_lock(true).unwrap();
    assert!(backend.read_fn_lock().unwrap());
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::FN_LOCK),
        Some(0x05 | uwcc_ec_reg::FN_LOCK_BIT)
    );

    // Win Lock: reads BIT(0) of 0x0768, toggles via BIT(0) of 0x0767
    transport.set_reg(uwcc_ec_reg::WIN_LOCK, 0x00);
    transport.set_reg(uwcc_ec_reg::WIN_LOCK_TRIGGER, 0x10);
    backend.set_win_lock(true).unwrap();
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::WIN_LOCK_TRIGGER),
        Some(0x10 | uwcc_ec_reg::WIN_LOCK_BIT)
    );
    transport.set_reg(uwcc_ec_reg::WIN_LOCK, uwcc_ec_reg::WIN_LOCK_BIT);
    assert!(backend.read_win_lock().unwrap());

    backend.set_usb_sleep_charging(true).unwrap();
    assert!(backend.read_usb_sleep_charging().unwrap());
    assert_eq!(transport.get_reg(uwcc_ec_reg::USB_SLEEP_CHARGING), Some(1));

    // Lightbar AC control RMW on 0x0748 BIT(0) — MUST NEVER touch 0x0767/0x0768 (Super key lock!)
    transport.set_reg(uwcc_ec_reg::LIGHTBAR_AC_CTRL, 0x80);
    transport.set_reg(uwcc_ec_reg::WIN_LOCK_TRIGGER, 0x00);
    backend.set_lightbar_power(true).unwrap();
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::LIGHTBAR_AC_CTRL),
        Some(0x80 | uwcc_ec_reg::LIGHTBAR_APP_EXISTS_BIT)
    );
    assert_eq!(transport.get_reg(uwcc_ec_reg::WIN_LOCK_TRIGGER), Some(0x00));
    backend.set_lightbar_power(false).unwrap();
    assert_eq!(transport.get_reg(uwcc_ec_reg::LIGHTBAR_AC_CTRL), Some(0x80));
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
        MuxActiveGpu, MuxBootMode, UwccGpuPower, UwccMuxState, UwccTransport, UWCC_FLAG_HAS_EC,
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

#[test]
fn uwcc_backend_upstream_0x1a_register_map_rmw_and_250w_fallback() {
    // Verify exact upstream uniwill-acpi.c addresses
    assert_eq!(uwcc_ec_reg::PROJECT_ID, 0x0740);
    assert_eq!(uwcc_ec_reg::GPU_FAN_RPM_HI, 0x046C);
    assert_eq!(uwcc_ec_reg::GPU_FAN_RPM_LO, 0x046D);
    assert_eq!(uwcc_ec_reg::PL1_WATTS, 0x0783);
    assert_eq!(uwcc_ec_reg::PL2_WATTS, 0x0784);
    assert_eq!(uwcc_ec_reg::PL4_WATTS, 0x0785);
    assert_eq!(uwcc_ec_reg::FN_LOCK, 0x07A4);
    assert_eq!(uwcc_ec_reg::CHARGE_CTRL, 0x07B9);
    assert_eq!(uwcc_ec_reg::LIGHTBAR_AC_CTRL, 0x0748);

    // Fallback chassis_0x1a_default() must accept MECHREVO X6AR55xY [210, 210, 250] W TDP
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    backend.set_tdp(0, 210).unwrap();
    backend.set_tdp(1, 210).unwrap();
    backend.set_tdp(2, 250).unwrap();
    assert_eq!(transport.get_reg(uwcc_ec_reg::PL1_WATTS), Some(210));
    assert_eq!(transport.get_reg(uwcc_ec_reg::PL2_WATTS), Some(210));
    assert_eq!(transport.get_reg(uwcc_ec_reg::PL4_WATTS), Some(250));

    // FAN_MANUAL_MODE (0x0751) must RMW BIT(7) without clobbering lower level bits (0x05)
    transport.set_reg(uwcc_ec_reg::FAN_MANUAL_MODE, 0x05);
    backend.set_fan_speed_percent(0, 60).unwrap();
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::FAN_MANUAL_MODE),
        Some(0x05 | uwcc_ec_reg::FAN_MODE_USER_BIT)
    );
    backend.set_fans_auto().unwrap();
    assert_eq!(transport.get_reg(uwcc_ec_reg::FAN_MANUAL_MODE), Some(0x05));
}

#[test]
fn uwcc_backend_decodes_12byte_dsdt_bx90_smrw_and_merges_0x07a7_ceiling() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    // Exact 12-byte BX90 table from MECHREVO YAOSHI X6AR55xY DSDT.dsl:
    // PL1=205W, PL2=205W, PL4=250W, step=5, DB=0..25W, max_gpu_temp=87C, base_tgp=150W, tail=FF*4
    let mut bx90 = [0u8; 64];
    bx90[..12].copy_from_slice(&[
        0xCD, 0xCD, 0xFA, 0x05, 0x00, 0x19, 0x57, 0x96, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    transport.set_smrw_table(SMRW_POWER_LIMITS_OFFSET, bx90);
    // Live EC 0x07A7..=0x07A9 factory ceiling (0xD2 = 210W)
    transport.set_reg(uwcc_ec_reg::TDP_FACTORY_PL1_MAX, 0xD2);
    transport.set_reg(uwcc_ec_reg::TDP_FACTORY_PL2_MAX, 0xD2);
    transport.set_reg(uwcc_ec_reg::TDP_FACTORY_PL4_MAX, 0xD2);

    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    assert_eq!(backend.read_tdp_limits(0).unwrap(), (15, 210));
    assert_eq!(backend.read_tdp_limits(1).unwrap(), (20, 210));
    assert_eq!(backend.read_tdp_limits(2).unwrap(), (25, 250));
    assert_eq!(backend.read_ctgp_limits(), (55, 175));
    assert_eq!(backend.read_dynamic_boost_limits(), (0, 25));

    // Profile frixit-ac [210, 210, 250] W applies cleanly without failing
    backend.set_tdp(0, 210).unwrap();
    backend.set_tdp(1, 210).unwrap();
    backend.set_tdp(2, 250).unwrap();
}

#[test]
fn uapi_gpu_power_and_mux_ioctl_abi_match_kernel_header() {
    use ucc_hardware::{
        UwccGpuPower, UwccMuxState, UWCC_IOC_GPU_POWER, UWCC_IOC_MUX_GET, UWCC_IOC_MUX_SET,
        UWCC_NR_GPU_POWER, UWCC_NR_MUX_GET, UWCC_NR_MUX_SET,
    };

    assert_eq!(size_of::<UwccGpuPower>(), 8);
    assert_eq!(align_of::<UwccGpuPower>(), 4);
    assert_eq!(size_of::<UwccMuxState>(), 8);
    assert_eq!(align_of::<UwccMuxState>(), 4);

    let decode = |ioc: u64| -> (u64, u64, u64, u64) {
        let nr = ioc & 0xFF;
        let ty = (ioc >> 8) & 0xFF;
        let sz = (ioc >> 16) & 0x3FFF;
        let dir = (ioc >> 30) & 0x3;
        (dir, ty, nr, sz)
    };

    assert_eq!(
        decode(UWCC_IOC_GPU_POWER),
        (1, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_GPU_POWER), 8)
    );
    assert_eq!(
        decode(UWCC_IOC_MUX_GET),
        (2, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_MUX_GET), 8)
    );
    assert_eq!(
        decode(UWCC_IOC_MUX_SET),
        (1, u64::from(UWCC_MAGIC), u64::from(UWCC_NR_MUX_SET), 8)
    );
}

#[test]
fn uwcc_backend_smrw_fallback_on_corrupt_or_unavailable_table() {
    use ucc_hardware::{UwccTransport, UWCC_SMRW_MAX_LEN};

    let expected = SmrwPowerLimits::chassis_0x1a_default();

    // 1. Corrupt SMRW table with min > max must fall back to chassis_0x1a_default()
    let corrupt_transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut corrupt_limits = SmrwPowerLimits::chassis_0x1a_default();
    corrupt_limits.pl1_min_w = 120;
    corrupt_limits.pl1_max_w = 60;
    corrupt_transport.set_smrw_table(SMRW_POWER_LIMITS_OFFSET, corrupt_limits.encode());
    let corrupt_backend = UwccBackend::new(corrupt_transport, None, None);
    assert_eq!(corrupt_backend.smrw_limits(), expected);

    // 2. Zeroed SMRW buffer must fall back to chassis_0x1a_default()
    let zero_transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    zero_transport.set_smrw_table(SMRW_POWER_LIMITS_OFFSET, [0u8; UWCC_SMRW_MAX_LEN]);
    let zero_backend = UwccBackend::new(zero_transport, None, None);
    assert_eq!(zero_backend.smrw_limits(), expected);

    // 3. Unavailable transport during probe must fall back to chassis_0x1a_default()
    let unavail_transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    unavail_transport.set_available(false);
    let unavail_backend = UwccBackend::new(unavail_transport, None, None);
    assert_eq!(unavail_backend.smrw_limits(), expected);

    // 4. Available transport whose smrw() explicitly returns Err(Unavailable)
    struct FailingSmrwTransport(MockUwccTransport);
    impl UwccTransport for FailingSmrwTransport {
        fn is_available(&self) -> bool {
            self.0.is_available()
        }
        fn get_info(&self) -> Result<UwccInfo, PlatformError> {
            self.0.get_info()
        }
        fn ec_read(&self, addr: u16) -> Result<u8, PlatformError> {
            self.0.ec_read(addr)
        }
        fn ec_write(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
            self.0.ec_write(addr, val)
        }
        fn smrw(&self, _req: UwccSmrw) -> Result<UwccSmrw, PlatformError> {
            Err(PlatformError::Unavailable)
        }
        fn pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
            self.0.pwup(enable, level)
        }
        fn poll_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
            self.0.poll_event()
        }
    }

    let failing_smrw = Arc::new(FailingSmrwTransport(
        MockUwccTransport::chassis_0x1a_default(),
    ));
    let failing_backend = UwccBackend::new(failing_smrw, None, None);
    assert_eq!(failing_backend.smrw_limits(), expected);
}

#[test]
fn uwcc_backend_ctgp_and_db_respect_pwbt_pwdb_flags_and_fail_writes() {
    use ucc_hardware::{
        UwccTransport, UWCC_FLAG_HAS_EC, UWCC_FLAG_HAS_PWBT, UWCC_FLAG_HAS_PWDB,
        UWCC_GPU_POWER_OP_BASE_TGP, UWCC_GPU_POWER_OP_DYNAMIC_BOOST,
    };

    // Without UWCC_FLAG_HAS_PWBT / UWCC_FLAG_HAS_PWDB, only EC registers are written
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut info = transport.get_info().unwrap();
    info.flags = UWCC_FLAG_HAS_EC;
    transport.set_info(info);

    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);
    transport.clear_journal();

    backend.set_ctgp(120).unwrap();
    backend.set_dynamic_boost(20).unwrap();
    assert_eq!(transport.get_reg(uwcc_ec_reg::CTGP_WATTS), Some(120));
    assert_eq!(
        transport.get_reg(uwcc_ec_reg::DYNAMIC_BOOST_WATTS),
        Some(20)
    );

    let journal = transport.journal();
    assert!(!journal
        .iter()
        .any(|w| matches!(w, UwccTransportWrite::GpuPower { .. })));

    // Only UWCC_FLAG_HAS_PWBT enabled: cTGP dispatches BASE_TGP, DB does not dispatch DYNAMIC_BOOST
    let pwbt_only = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut pwbt_info = pwbt_only.get_info().unwrap();
    pwbt_info.flags = UWCC_FLAG_HAS_EC | UWCC_FLAG_HAS_PWBT;
    pwbt_only.set_info(pwbt_info);

    let pwbt_backend = UwccBackend::new(Arc::clone(&pwbt_only) as _, None, None);
    pwbt_only.clear_journal();
    pwbt_backend.set_ctgp(130).unwrap();
    pwbt_backend.set_dynamic_boost(15).unwrap();
    let pwbt_journal = pwbt_only.journal();
    assert!(pwbt_journal.contains(&UwccTransportWrite::GpuPower {
        op: UWCC_GPU_POWER_OP_BASE_TGP,
        value: 130,
    }));
    assert!(!pwbt_journal.iter().any(|w| matches!(
        w,
        UwccTransportWrite::GpuPower {
            op: UWCC_GPU_POWER_OP_DYNAMIC_BOOST,
            ..
        }
    )));

    // Only UWCC_FLAG_HAS_PWDB enabled: DB dispatches DYNAMIC_BOOST, cTGP does not dispatch BASE_TGP
    let pwdb_only = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut pwdb_info = pwdb_only.get_info().unwrap();
    pwdb_info.flags = UWCC_FLAG_HAS_EC | UWCC_FLAG_HAS_PWDB;
    pwdb_only.set_info(pwdb_info);

    let pwdb_backend = UwccBackend::new(Arc::clone(&pwdb_only) as _, None, None);
    pwdb_only.clear_journal();
    pwdb_backend.set_ctgp(125).unwrap();
    pwdb_backend.set_dynamic_boost(25).unwrap();
    let pwdb_journal = pwdb_only.journal();
    assert!(pwdb_journal.contains(&UwccTransportWrite::GpuPower {
        op: UWCC_GPU_POWER_OP_DYNAMIC_BOOST,
        value: 25,
    }));
    assert!(!pwdb_journal.iter().any(|w| matches!(
        w,
        UwccTransportWrite::GpuPower {
            op: UWCC_GPU_POWER_OP_BASE_TGP,
            ..
        }
    )));

    // With set_fail_writes(true), both set_ctgp and set_dynamic_boost fail closed
    let fail_transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let fail_backend = UwccBackend::new(Arc::clone(&fail_transport) as _, None, None);
    fail_transport.clear_journal();
    fail_transport.set_fail_writes(true);

    assert!(matches!(
        fail_backend.set_ctgp(120),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        fail_backend.set_dynamic_boost(20),
        Err(PlatformError::Unavailable)
    ));
    assert!(fail_transport.journal().is_empty());
}

#[test]
fn uwcc_backend_manual_fan_speed_bounds_and_unavailable_transport() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);

    // Valid fan indices (0 = CPU, 1 = GPU) across boundary percentages 0, 50, 100
    for (percent, expected_raw) in [(0u8, 0u8), (50, 100), (100, 200)] {
        backend.set_fan_speed_percent(0, percent).unwrap();
        assert_eq!(
            transport.get_reg(uwcc_ec_reg::CPU_FAN_DUTY_WRITE),
            Some(expected_raw)
        );
        assert_ne!(
            transport
                .get_reg(uwcc_ec_reg::FAN_MANUAL_MODE)
                .unwrap_or_default()
                & uwcc_ec_reg::FAN_MODE_USER_BIT,
            0
        );

        backend.set_fan_speed_percent(1, percent).unwrap();
        assert_eq!(
            transport.get_reg(uwcc_ec_reg::GPU_FAN_DUTY_WRITE),
            Some(expected_raw)
        );
    }

    // Invalid fan index 2 and out-of-range percentage 101 rejected before EC write
    transport.clear_journal();
    assert!(matches!(
        backend.set_fan_speed_percent(2, 50),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_fan_speed_percent(0, 101),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_fan_speed_percent(1, 101),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(transport.journal().is_empty());

    // Disconnected/unavailable transport fails closed on reads and writes
    transport.set_available(false);
    assert!(!backend.is_available());
    assert!(matches!(
        backend.set_fan_speed_percent(0, 50),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        backend.set_fans_auto(),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        backend.read_fan_speed_percent(0),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        backend.read_fan_rpm(0),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        backend.read_fan_temp(0),
        Err(PlatformError::Unavailable)
    ));
}

#[test]
fn ramfan_validation_matrix_and_fan_boost_transitions() {
    use ucc_hardware::RAMFAN_MAX_TEMP_C;

    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let controller = RamFanController::new(Arc::clone(&transport) as _);
    transport.clear_journal();

    // 1. Duty > 100 rejected on CPU and GPU
    let mut bad_cpu_duty = RamFanCurve::balanced_default();
    bad_cpu_duty.cpu[RAMFAN_POINTS - 1].duty_percent = 101;
    assert!(matches!(
        controller.apply_curve(&bad_cpu_duty),
        Err(PlatformError::InvalidArgument(_))
    ));

    let mut bad_gpu_duty = RamFanCurve::balanced_default();
    bad_gpu_duty.gpu[RAMFAN_POINTS - 1].duty_percent = 101;
    assert!(matches!(
        controller.apply_curve(&bad_gpu_duty),
        Err(PlatformError::InvalidArgument(_))
    ));

    // 2. Temp > 115 C rejected on CPU and GPU
    let mut bad_cpu_temp = RamFanCurve::balanced_default();
    bad_cpu_temp.cpu[RAMFAN_POINTS - 1].temp_c = RAMFAN_MAX_TEMP_C + 1;
    assert!(matches!(
        controller.apply_curve(&bad_cpu_temp),
        Err(PlatformError::InvalidArgument(_))
    ));

    let mut bad_gpu_temp = RamFanCurve::balanced_default();
    bad_gpu_temp.gpu[RAMFAN_POINTS - 1].temp_c = RAMFAN_MAX_TEMP_C + 1;
    assert!(matches!(
        controller.apply_curve(&bad_gpu_temp),
        Err(PlatformError::InvalidArgument(_))
    ));

    // 3. Non-monotonic duty rejected on CPU and GPU
    let mut non_mono_cpu_duty = RamFanCurve::balanced_default();
    non_mono_cpu_duty.cpu[6].duty_percent = non_mono_cpu_duty.cpu[5].duty_percent - 1;
    assert!(matches!(
        controller.apply_curve(&non_mono_cpu_duty),
        Err(PlatformError::InvalidArgument(_))
    ));

    let mut non_mono_gpu_duty = RamFanCurve::balanced_default();
    non_mono_gpu_duty.gpu[6].duty_percent = non_mono_gpu_duty.gpu[5].duty_percent - 1;
    assert!(matches!(
        controller.apply_curve(&non_mono_gpu_duty),
        Err(PlatformError::InvalidArgument(_))
    ));

    assert!(transport.journal().is_empty());
    assert!(!controller.is_custom_active());

    // 4. Fan boost transitions WITHOUT active custom curve
    controller.set_fan_boost(true).unwrap();
    assert!(controller.is_fan_boost_enabled());
    assert!(!controller.is_custom_active());
    assert_eq!(
        transport.get_reg(ramfan_reg::CTRL),
        Some(ramfan_flags::FAN_BOOST)
    );
    assert_eq!(transport.get_reg(ramfan_reg::COMMIT), Some(0x01));

    controller.set_fan_boost(false).unwrap();
    assert!(!controller.is_fan_boost_enabled());
    assert_eq!(transport.get_reg(ramfan_reg::CTRL), Some(0x00));
    assert_eq!(transport.get_reg(ramfan_reg::COMMIT), Some(0x00));

    // 5. Fan boost transitions WITH active custom curve
    let valid_curve = RamFanCurve::balanced_default();
    controller.apply_curve(&valid_curve).unwrap();
    assert!(controller.is_custom_active());

    controller.set_fan_boost(true).unwrap();
    assert!(controller.is_fan_boost_enabled());
    assert_eq!(
        transport.get_reg(ramfan_reg::CTRL),
        Some(ramfan_flags::CUME | ramfan_flags::UFME | ramfan_flags::FAN_BOOST)
    );
    assert_eq!(transport.get_reg(ramfan_reg::COMMIT), Some(0x01));

    controller.set_fan_boost(false).unwrap();
    assert!(!controller.is_fan_boost_enabled());
    assert!(controller.is_custom_active());
    assert_eq!(
        transport.get_reg(ramfan_reg::CTRL),
        Some(ramfan_flags::CUME | ramfan_flags::UFME)
    );
    assert_eq!(transport.get_reg(ramfan_reg::COMMIT), Some(0x01));

    // Unavailable transport fails closed
    transport.set_available(false);
    assert!(matches!(
        controller.set_fan_boost(true),
        Err(PlatformError::Unavailable)
    ));
}

#[test]
fn usb_webcam_vid_pid_filter_multi_cam_and_errors() {
    use ucc_hardware::UsbWebcamController;

    let sysfs = Arc::new(MockSysfs::new());

    // Camera 1: integrated RGB+IR webcam at 1-6
    sysfs.set_file("/sys/bus/usb/devices/1-6/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/1-6/idVendor", "04f2");
    sysfs.set_file("/sys/bus/usb/devices/1-6/idProduct", "b751");
    sysfs.set_file("/sys/bus/usb/devices/1-6/product", "Integrated Camera");
    sysfs.set_file("/sys/bus/usb/devices/1-6:1.0/bInterfaceClass", "0e");

    // Camera 2: external USB UVC webcam at 2-3
    sysfs.set_file("/sys/bus/usb/devices/2-3/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/2-3/idVendor", "046d");
    sysfs.set_file("/sys/bus/usb/devices/2-3/idProduct", "0825");
    sysfs.set_file("/sys/bus/usb/devices/2-3/product", "External UVC Cam");
    sysfs.set_file("/sys/bus/usb/devices/2-3:1.0/bInterfaceClass", "0E");

    // Non-camera HID device at 1-4
    sysfs.set_file("/sys/bus/usb/devices/1-4/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/1-4/idVendor", "048d");
    sysfs.set_file("/sys/bus/usb/devices/1-4/idProduct", "600b");
    sysfs.set_file("/sys/bus/usb/devices/1-4:1.0/bInterfaceClass", "03");

    // Unfiltered controller discovers and toggles both cameras
    let all_cams = UsbWebcamController::new(Arc::clone(&sysfs) as _);
    let discovered = all_cams.discover_webcams().unwrap();
    assert_eq!(discovered.len(), 2);
    assert_eq!(discovered[0].vendor_id, "04f2");
    assert_eq!(discovered[0].product_id, "b751");
    assert_eq!(discovered[1].vendor_id, "046d");
    assert_eq!(discovered[1].product_id, "0825");

    all_cams.set_enabled(false).unwrap();
    assert!(!all_cams.is_enabled().unwrap());
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/1-6/authorized")
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/2-3/authorized")
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/1-4/authorized")
            .as_deref(),
        Some("1")
    );

    all_cams.set_enabled(true).unwrap();
    assert!(all_cams.is_enabled().unwrap());

    // VID/PID filter (case-insensitive) restricts discovery and toggling to matching camera
    let filtered =
        UsbWebcamController::new(Arc::clone(&sysfs) as _).with_vid_pid_filter("04F2", "B751");
    let matched = filtered.discover_webcams().unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].vendor_id, "04f2");

    filtered.set_enabled(false).unwrap();
    assert!(!filtered.is_enabled().unwrap());
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/1-6/authorized")
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        sysfs
            .get_file("/sys/bus/usb/devices/2-3/authorized")
            .as_deref(),
        Some("1")
    );

    // Non-matching VID/PID filter yields Unsupported on read/write
    let no_match =
        UsbWebcamController::new(Arc::clone(&sysfs) as _).with_vid_pid_filter("dead", "beef");
    assert!(no_match.discover_webcams().unwrap().is_empty());
    assert!(matches!(
        no_match.is_enabled(),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        no_match.set_enabled(false),
        Err(PlatformError::Unsupported(_))
    ));

    // Missing /sys/bus/usb/devices root returns Unsupported
    let empty_sysfs = Arc::new(MockSysfs::new());
    let missing_root = UsbWebcamController::new(empty_sysfs);
    assert!(matches!(
        missing_root.discover_webcams(),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        missing_root.is_enabled(),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        missing_root.set_enabled(true),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn wmi_uinput_mock_emitter_error_propagation_and_unknown_codes() {
    let emitter = MockUinputEmitter::new();
    emitter.set_fail(true);

    // Known WMI codes attempt key emission and propagate Unavailable when failing
    let known_evt = UwccWmiEvent {
        timestamp_ns: 1000,
        code: wmi_code::FN_LOCK_TOGGLE,
        payload: 1,
    };
    assert!(matches!(
        emitter.handle_wmi_event(known_evt),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        emitter.emit_hotkey(UwccHotkeyEvent::WebcamToggle),
        Err(PlatformError::Unavailable)
    ));
    assert!(emitter.emitted_keys().is_empty());

    // Unknown WMI codes do not emit keys, so they succeed cleanly even when set_fail(true)
    let unknown_evt = UwccWmiEvent {
        timestamp_ns: 2000,
        code: 0xBEEF,
        payload: 0,
    };
    assert_eq!(
        emitter.handle_wmi_event(unknown_evt).unwrap(),
        UwccHotkeyEvent::Unknown(0xBEEF)
    );
    assert!(!emitter
        .emit_hotkey(UwccHotkeyEvent::Unknown(0xCAFE))
        .unwrap());
    assert!(emitter.emitted_keys().is_empty());

    // Recovering emitter resumes normal key emission
    emitter.set_fail(false);
    let webcam_evt = UwccWmiEvent {
        timestamp_ns: 3000,
        code: wmi_code::WEBCAM_TOGGLE,
        payload: 1,
    };
    assert_eq!(
        emitter.handle_wmi_event(webcam_evt).unwrap(),
        UwccHotkeyEvent::WebcamToggle
    );
    assert_eq!(emitter.emitted_keys(), vec![linux_key::KEY_CAMERA]);
    emitter.clear();
    assert!(emitter.emitted_keys().is_empty());
}

#[test]
fn uwcc_backend_ignores_0xff_unpopulated_ec_tdp_ceiling_sentinel() {
    use ucc_hardware::UWCC_SMRW_MAX_LEN;
    let transport = Arc::new(MockUwccTransport::new());
    // Native 12-byte INOU0000 BX90 table: PL1=205W, PL2=205W, PL4=250W.
    let mut bx90 = [0u8; UWCC_SMRW_MAX_LEN];
    bx90[..12].copy_from_slice(&[
        0xCD, 0xCD, 0xFA, 0x05, 0x00, 0x19, 0x57, 0x96, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    transport.set_smrw_table(0, bx90);
    // Simulate an SKU where EC 0x07A7..=0x07A9 are unprogrammed (0xFF sentinel).
    transport.set_reg(uwcc_ec_reg::TDP_FACTORY_PL1_MAX, 0xFF);
    transport.set_reg(uwcc_ec_reg::TDP_FACTORY_PL2_MAX, 0xFF);
    transport.set_reg(uwcc_ec_reg::TDP_FACTORY_PL4_MAX, 0xFF);

    let backend = UwccBackend::new(transport, None, None);
    let limits = backend.smrw_limits();
    assert_eq!(limits.pl1_max_w, 205);
    assert_eq!(limits.pl2_max_w, 205);
    assert_eq!(limits.pl4_max_w, 250);
}

#[test]
fn uwcc_backend_decodes_12byte_dsdt_g2x9_smrw_without_fallback() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use ucc_hardware::{UwccTransport, UWCC_SMRW_MAX_LEN};

    struct CountingSmrwTransport {
        inner: MockUwccTransport,
        calls_over_12: AtomicUsize,
    }

    impl UwccTransport for CountingSmrwTransport {
        fn is_available(&self) -> bool {
            self.inner.is_available()
        }
        fn get_info(&self) -> Result<ucc_hardware::UwccInfo, PlatformError> {
            self.inner.get_info()
        }
        fn ec_read(&self, addr: u16) -> Result<u8, PlatformError> {
            self.inner.ec_read(addr)
        }
        fn ec_write(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
            self.inner.ec_write(addr, val)
        }
        fn smrw(&self, req: UwccSmrw) -> Result<UwccSmrw, PlatformError> {
            if req.len > 12 {
                self.calls_over_12.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.smrw(req)
        }
        fn pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
            self.inner.pwup(enable, level)
        }
        fn set_gpu_power(&self, op: u32, watts: u32) -> Result<(), PlatformError> {
            self.inner.set_gpu_power(op, watts)
        }
        fn poll_event(&self) -> Result<Option<ucc_hardware::UwccWmiEvent>, PlatformError> {
            self.inner.poll_event()
        }
        fn mux_get(&self) -> Result<ucc_hardware::UwccMuxState, PlatformError> {
            self.inner.mux_get()
        }
        fn mux_set(&self, boot_mode: u32) -> Result<(), PlatformError> {
            self.inner.mux_set(boot_mode)
        }
    }

    let inner = MockUwccTransport::chassis_0x1a_default();
    let mut g2x9 = [0u8; UWCC_SMRW_MAX_LEN];
    g2x9[..12].copy_from_slice(&[
        0xD2, 0xD2, 0xD2, 0x0A, 0x46, 0x19, 0x57, 0x50, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    inner.set_smrw_table(SMRW_POWER_LIMITS_OFFSET, g2x9);

    let counting = Arc::new(CountingSmrwTransport {
        inner,
        calls_over_12: AtomicUsize::new(0),
    });
    let backend = UwccBackend::new(Arc::clone(&counting) as Arc<dyn UwccTransport>, None, None);
    let limits = backend.smrw_limits();

    assert_eq!(counting.calls_over_12.load(Ordering::SeqCst), 0);
    assert_eq!(limits.pl1_max_w, 210);
    assert_eq!(limits.pl2_max_w, 210);
    assert_eq!(limits.pl4_max_w, 210);
    assert_eq!(limits.ctgp_min_w, 70);
    assert_eq!(limits.ctgp_max_w, 175);
    assert_eq!(limits.dynamic_boost_min_w, 0);
    assert_eq!(limits.dynamic_boost_max_w, 25);
}

#[test]
fn uwcc_12byte_smrw_rejects_excessive_base_tgp_and_ctgp_over_byte_ceiling() {
    // base_tgp_max = data[4] (200) + data[7] (52) = 252 > 250, ctgp_max = 252 <= 255 -> rejected.
    let over_base = [
        0xD2, 0xD2, 0xD2, 0x0A, 200, 0, 0x57, 52, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    assert!(SmrwPowerLimits::decode_inou0000_12byte(&over_base).is_none());

    // base_tgp_max = 220 <= 250, but ctgp_max = 220 + 50 = 270 > u8::MAX (255) -> rejected.
    let over_ctgp = [
        0xD2, 0xD2, 0xD2, 0x0A, 120, 50, 0x57, 100, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    assert!(SmrwPowerLimits::decode_inou0000_12byte(&over_ctgp).is_none());
}

#[test]
fn uwcc_backend_charging_profile_rmw_ctgp_and_lid_logo_ec_sequence() {
    use ucc_hardware::UwccTransport;

    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    transport.set_reg(0x0767, 0x01); // WIN_LOCK_TRIGGER must remain untouched
    transport.set_reg(0x07A6, 0x89);
    transport.set_reg(0x07AB, 0x05); // bit0=exists, bit2=preserved
    transport.set_reg(0x07C5, 0x48);

    let backend = UwccBackend::new(Arc::clone(&transport) as Arc<dyn UwccTransport>, None, None);
    let platform: &dyn PlatformBackend = &backend;

    // Uninitialized EC 0x0744 == 0 returns Unavailable; valid write reads back cleanly.
    assert!(platform.read_ctgp_limits().is_some());
    assert!(platform.read_ctgp_watts().is_err());
    platform.set_ctgp_watts(150).unwrap();
    assert_eq!(platform.read_ctgp_watts().unwrap(), 150);

    // Charging profile RMW preserves unrelated bits (0x81) and round-trips all 3 modes.
    assert_eq!(
        platform.charging_profiles_available(),
        vec!["high_capacity", "balanced", "stationary"]
    );
    assert_eq!(platform.read_charging_profile().unwrap(), "high_capacity");

    platform.set_charging_profile("balanced").unwrap();
    assert_eq!(transport.get_reg(0x07A6), Some(0x99));
    assert_eq!(platform.read_charging_profile().unwrap(), "balanced");

    platform.set_charging_profile("stationary").unwrap();
    assert_eq!(transport.get_reg(0x07A6), Some(0xA9));
    assert_eq!(platform.read_charging_profile().unwrap(), "stationary");

    platform.set_charging_profile("high_capacity").unwrap();
    assert_eq!(transport.get_reg(0x07A6), Some(0x89));
    assert!(platform.set_charging_profile("invalid_mode").is_err());

    // Unrecognized battery mode bits return Unavailable rather than misreporting high_capacity.
    transport.set_reg(0x07A6, 0x38);
    assert!(platform.read_charging_profile().is_err());

    // Lid logo EC sequence preserves low bits in 0x07AB, skips 0x0767 clobber, and commits 0x078C.
    assert!(platform.has_lid_logo_ec());
    platform.set_lid_logo_ec(true, 0, 255, 128, 80).unwrap();
    assert_eq!(transport.get_reg(0x07AB), Some(0x25));
    platform.set_lid_logo_ec(true, 0, 255, 128, 128).unwrap();
    assert_eq!(transport.get_reg(0x07AB), Some(0x45));
    platform.set_lid_logo_ec(true, 0, 255, 128, 255).unwrap();
    assert_eq!(transport.get_reg(0x0767), Some(0x01));
    assert_eq!(transport.get_reg(0x07C5), Some(0x68));
    assert_eq!(transport.get_reg(0x0769), Some(0));
    assert_eq!(transport.get_reg(0x076A), Some(50));
    assert_eq!(transport.get_reg(0x076B), Some(25));
    assert_eq!(transport.get_reg(0x07AB), Some(0x85));
    assert_eq!(transport.get_reg(0x078C), Some(0x10));

    platform.set_lid_logo_ec(false, 0, 0, 0, 0).unwrap();
    assert_eq!(transport.get_reg(0x07AB), Some(0x05));
    assert_eq!(transport.get_reg(0x078C), Some(0x10));

    // Hardware without LOGO_EXISTS_BIT rejects set_lid_logo_ec without forging bit 0.
    transport.set_reg(0x07AB, 0x00);
    assert!(!platform.has_lid_logo_ec());
    assert!(platform.set_lid_logo_ec(true, 255, 0, 0, 255).is_err());
    assert_eq!(transport.get_reg(0x07AB), Some(0x00));
}

#[test]
fn uwcc_backend_api_version_and_capability_flags_enforcement() {
    use ucc_hardware::{
        UwccTransport, UWCC_API_VERSION, UWCC_FLAG_HAS_EC, UWCC_FLAG_HAS_PWUP, UWCC_FLAG_HAS_SMRW,
        UWCC_FLAG_HAS_WMI_EVENTS,
    };

    // 1. API version mismatch disables availability, capabilities, and operations.
    let bad_ver = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut info = bad_ver.get_info().unwrap();
    info.api_version = UWCC_API_VERSION + 1;
    bad_ver.set_info(info);
    let bad_backend = UwccBackend::new(bad_ver, None, None);
    assert!(!bad_backend.is_available());
    assert!(!bad_backend.capabilities().device.fan_control);
    assert_eq!(bad_backend.capabilities().tdp_channels, 0);
    assert!(matches!(
        bad_backend.read_fan_temp(0),
        Err(PlatformError::Unsupported(_))
    ));

    // 2. Missing EC/PWUP/WMI_EVENTS flags gate capabilities and methods fail-closed.
    let no_ec = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut info = no_ec.get_info().unwrap();
    info.flags = UWCC_FLAG_HAS_SMRW; // No EC, PWUP, or WMI_EVENTS
    no_ec.set_info(info);
    let no_ec_backend = UwccBackend::new(no_ec, None, None);
    assert!(no_ec_backend.is_available());
    assert!(!no_ec_backend.capabilities().device.fan_control);
    assert!(!no_ec_backend.capabilities().device.tdp_control);
    assert_eq!(no_ec_backend.capabilities().device.number_of_fans, 0);
    assert!(matches!(
        no_ec_backend.read_fan_temp(0),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        no_ec_backend.set_pwup(true, 1),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        no_ec_backend.poll_wmi_event(),
        Err(PlatformError::Unsupported(_))
    ));

    // 3. EC-only flags enable EC while gating PWUP and WMI_EVENTS.
    let ec_only = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let mut info = ec_only.get_info().unwrap();
    info.flags = UWCC_FLAG_HAS_EC;
    ec_only.set_info(info);
    let ec_only_backend = UwccBackend::new(ec_only, None, None);
    assert!(ec_only_backend.capabilities().device.fan_control);
    assert!(ec_only_backend.read_fan_temp(0).is_ok());
    assert!(matches!(
        ec_only_backend.set_pwup(true, 1),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        ec_only_backend.poll_wmi_event(),
        Err(PlatformError::Unsupported(_))
    ));

    let _ = (UWCC_FLAG_HAS_PWUP, UWCC_FLAG_HAS_WMI_EVENTS);
}

#[test]
fn uwcc_set_fans_auto_clears_manual_mode_even_when_ramfan_restore_fails() {
    use ucc_hardware::UwccTransport;

    struct FailRamFanCtrlTransport(MockUwccTransport);
    impl UwccTransport for FailRamFanCtrlTransport {
        fn is_available(&self) -> bool {
            self.0.is_available()
        }
        fn get_info(&self) -> Result<UwccInfo, PlatformError> {
            self.0.get_info()
        }
        fn ec_read(&self, addr: u16) -> Result<u8, PlatformError> {
            self.0.ec_read(addr)
        }
        fn ec_write(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
            if addr == ramfan_reg::CTRL {
                return Err(PlatformError::Unavailable);
            }
            self.0.ec_write(addr, val)
        }
        fn smrw(&self, req: UwccSmrw) -> Result<UwccSmrw, PlatformError> {
            self.0.smrw(req)
        }
        fn pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
            self.0.pwup(enable, level)
        }
        fn poll_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
            self.0.poll_event()
        }
    }

    let inner = MockUwccTransport::chassis_0x1a_default();
    inner.set_reg(
        uwcc_ec_reg::FAN_MANUAL_MODE,
        0x03 | uwcc_ec_reg::FAN_MODE_USER_BIT,
    );
    let transport = Arc::new(FailRamFanCtrlTransport(inner));
    let backend = UwccBackend::new(Arc::clone(&transport) as _, None, None);

    // Must return Err because ramfan restore failed, YET still clear FAN_MODE_USER_BIT!
    assert!(backend.set_fans_auto().is_err());
    assert_eq!(
        transport.0.get_reg(uwcc_ec_reg::FAN_MANUAL_MODE),
        Some(0x03)
    );
}

#[test]
fn ramfan_diff_write_cache_skips_unchanged_and_invalidates_on_reset_or_error() {
    let transport = Arc::new(MockUwccTransport::chassis_0x1a_default());
    let ctrl = RamFanController::new(Arc::clone(&transport) as _);
    let mut curve = RamFanCurve::balanced_default();

    // First apply writes all 64 curve registers + CTRL + COMMIT = 66 writes.
    transport.clear_journal();
    ctrl.apply_curve(&curve).unwrap();
    assert_eq!(transport.journal().len(), 66);

    // Re-applying identical curve performs 0 EC writes.
    transport.clear_journal();
    ctrl.apply_curve(&curve).unwrap();
    assert_eq!(transport.journal().len(), 0);

    // Modifying a single breakpoint duty writes only that 1 register + CTRL + COMMIT = 3 writes.
    curve.cpu[RAMFAN_POINTS - 1].duty_percent = 99;
    transport.clear_journal();
    ctrl.apply_curve(&curve).unwrap();
    assert_eq!(transport.journal().len(), 3);

    // restore_firmware_auto invalidates cache so next apply writes all 66 registers again.
    ctrl.restore_firmware_auto().unwrap();
    transport.clear_journal();
    ctrl.apply_curve(&curve).unwrap();
    assert_eq!(transport.journal().len(), 66);

    // Write failure invalidates cache so subsequent apply writes all 66 registers again.
    curve.gpu[RAMFAN_POINTS - 1].duty_percent = 98;
    transport.set_fail_writes(true);
    assert!(ctrl.apply_curve(&curve).is_err());
    transport.set_fail_writes(false);
    transport.clear_journal();
    ctrl.apply_curve(&curve).unwrap();
    assert_eq!(transport.journal().len(), 66);
}

#[test]
fn webcam_ttl_cache_and_bounded_sysfs_journal_and_d3cold_temp_guard() {
    use std::time::Duration;
    use ucc_hardware::ioctl::requests;
    use ucc_hardware::sysfs::{SysfsWriteJournal, DEFAULT_MAX_JOURNAL_ENTRIES};
    use ucc_hardware::uwcc::UsbWebcamController;

    // 1. Webcam TTL cache skips redundant sysfs scans within TTL and rescans after invalidation.
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file("/sys/bus/usb/devices/1-6/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/1-6/idVendor", "04f2");
    sysfs.set_file("/sys/bus/usb/devices/1-6/idProduct", "b751");
    sysfs.set_file("/sys/bus/usb/devices/1-6:1.0/bInterfaceClass", "0e");

    let webcam =
        UsbWebcamController::new(Arc::clone(&sysfs) as _).with_cache_ttl(Duration::from_secs(60));
    assert_eq!(webcam.discover_webcams().unwrap().len(), 1);

    // Adding a second camera is not seen while within TTL, then seen after invalidation.
    sysfs.set_file("/sys/bus/usb/devices/1-7/authorized", "1");
    sysfs.set_file("/sys/bus/usb/devices/1-7/idVendor", "04f2");
    sysfs.set_file("/sys/bus/usb/devices/1-7/idProduct", "b752");
    sysfs.set_file("/sys/bus/usb/devices/1-7:1.0/bInterfaceClass", "0e");
    assert_eq!(webcam.discover_webcams().unwrap().len(), 1);
    webcam.invalidate_cache();
    assert_eq!(webcam.discover_webcams().unwrap().len(), 2);

    // 2. Bounded SysfsWriteJournal evicts oldest entries FIFO at cap.
    let journal = SysfsWriteJournal::with_max_entries(4);
    for i in 0..10 {
        journal.record(format!("/sys/test/{i}"), format!("{i}"), true);
    }
    assert_eq!(journal.count(), 4);
    let payloads: Vec<String> = journal.entries().into_iter().map(|e| e.payload).collect();
    assert_eq!(payloads, vec!["6", "7", "8", "9"]);
    assert_eq!(
        SysfsWriteJournal::new().max_entries(),
        DEFAULT_MAX_JOURNAL_ENTRIES
    );

    // 3. TuxedoIoBackend rejects D3cold/sentinel temperatures > 150 C.
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_FAN_TEMP, 255);
    let backend = TuxedoIoBackend::new(hw, None, None);
    assert!(matches!(
        backend.read_fan_temp(0),
        Err(PlatformError::SensorUnavailable(0))
    ));
}
