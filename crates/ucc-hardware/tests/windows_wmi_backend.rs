// SPDX-License-Identifier: GPL-3.0-or-later

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use ucc_hardware::windows_wmi::{
    decode_ec_read_byte, encode_ec_read_arg, encode_ec_write_arg, feature_requirement,
    feature_support, AcpiWmiCommand, DriverRequirement, WmiFeature, WINDOWS_WMI_CAPABILITY_MATRIX,
    WMI_CLASS_ACPI_TEST, WMI_EC_READ_FLAG, WMI_METHOD_GET_SET_ULONG, WMI_METHOD_ID_EC_RW,
    WMI_NAMESPACE,
};
use ucc_hardware::{
    ec_reg, MockWmiEcTransport, PlatformBackend, PlatformBackendKind, PlatformError,
    WindowsWmiEcBackend, WmiEcOp, WmiEcTransport, WmiPlatformLimits,
};

#[test]
fn acpi_wmi_command_64bit_encoding_decoding_round_trips() {
    assert_eq!(WMI_NAMESPACE, r"root\WMI");
    assert_eq!(WMI_CLASS_ACPI_TEST, "AcpiTest_MULong");
    assert_eq!(WMI_METHOD_GET_SET_ULONG, "GetSetULong");

    let read_cmd = AcpiWmiCommand::ReadEcByte {
        addr: ec_reg::CPU_TEMP_C,
    };
    let read_arg = read_cmd.encode_arg();
    assert_eq!(read_arg, encode_ec_read_arg(ec_reg::CPU_TEMP_C));
    assert_eq!(read_arg, u64::from(ec_reg::CPU_TEMP_C) | WMI_EC_READ_FLAG);
    assert_eq!(AcpiWmiCommand::decode_arg(read_arg), read_cmd);
    assert_eq!(read_cmd.method_id(), WMI_METHOD_ID_EC_RW);

    let write_cmd = AcpiWmiCommand::WriteEcByte {
        addr: ec_reg::CPU_FAN_DUTY,
        val: 0xC8,
    };
    let write_arg = write_cmd.encode_arg();
    assert_eq!(write_arg, encode_ec_write_arg(ec_reg::CPU_FAN_DUTY, 0xC8));
    assert_eq!(write_arg, u64::from(ec_reg::CPU_FAN_DUTY) | (0xC8u64 << 16));
    assert_eq!(write_arg & WMI_EC_READ_FLAG, 0);
    assert_eq!(AcpiWmiCommand::decode_arg(write_arg), write_cmd);

    // Upper status bits from firmware return values must be masked out.
    assert_eq!(decode_ec_read_byte(0xFFFF_DEAD_BEEF_004A), 0x4A);
    assert_eq!(decode_ec_read_byte(0x0000_0000_0000_0000), 0x00);
    assert_eq!(decode_ec_read_byte(0x0000_0000_0000_00FF), 0xFF);

    // Boundary register addresses (0x0000 and 0xFFFF) round-trip cleanly.
    for addr in [0x0000u16, 0x0740, 0x07B9, 0x1809, 0xFFFF] {
        let r = AcpiWmiCommand::ReadEcByte { addr };
        assert_eq!(AcpiWmiCommand::decode_arg(r.encode_arg()), r);
        for val in [0x00u8, 0x01, 0x7F, 0xFF] {
            let w = AcpiWmiCommand::WriteEcByte { addr, val };
            assert_eq!(AcpiWmiCommand::decode_arg(w.encode_arg()), w);
        }
    }
}

#[test]
fn ev_whql_vs_driverless_capability_matrix_and_guards() {
    assert_eq!(WINDOWS_WMI_CAPABILITY_MATRIX.len(), 13);

    let driverless = [
        WmiFeature::FanControl,
        WmiFeature::FanBoost,
        WmiFeature::TempSensors,
        WmiFeature::PerfProfile,
        WmiFeature::TdpPl1Pl2Pl4,
        WmiFeature::CtgpDynamicBoost,
        WmiFeature::ChargeThresholds,
        WmiFeature::FnLock,
        WmiFeature::WebcamSwitch,
        WmiFeature::LightbarPower,
    ];
    let ev_whql = [
        WmiFeature::OemMethodPwup,
        WmiFeature::OemMethodSmrw,
        WmiFeature::RawEcArbitraryIo,
    ];

    let transport = Arc::new(MockWmiEcTransport::new(true));
    let backend = WindowsWmiEcBackend::new(transport, WmiPlatformLimits::default());

    for feat in driverless {
        assert_eq!(
            feature_requirement(feat),
            DriverRequirement::DriverlessWmiAcpi
        );
        let entry = feature_support(feat).expect("driverless feature must be in matrix");
        assert_eq!(entry.requirement, DriverRequirement::DriverlessWmiAcpi);
        assert!(entry.transport_path.contains("wmiacpi.sys"));
        assert!(backend.check_feature_support(feat).is_ok());
    }

    for feat in ev_whql {
        assert_eq!(
            feature_requirement(feat),
            DriverRequirement::SignedOemKmdfEvWhql
        );
        let entry = feature_support(feat).expect("EV/WHQL feature must be in matrix");
        assert_eq!(entry.requirement, DriverRequirement::SignedOemKmdfEvWhql);
        assert!(entry.transport_path.contains("UWACPIDriver.sys"));
        assert!(matches!(
            backend.check_feature_support(feat),
            Err(PlatformError::Unsupported(_))
        ));
    }

    assert!(matches!(
        backend.invoke_oem_pwup(1),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        backend.read_oem_smrw_table(),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn windows_wmi_backend_reads_and_writes_ec_within_bounds() {
    let transport = Arc::new(MockWmiEcTransport::new(true));
    transport.set_reg(ec_reg::CPU_TEMP_C, 61);
    transport.set_reg(ec_reg::GPU_TEMP_C, 54);

    let backend =
        WindowsWmiEcBackend::new(Arc::clone(&transport) as _, WmiPlatformLimits::default());
    assert_eq!(backend.kind(), PlatformBackendKind::WindowsWmi);
    assert!(backend.is_available());
    assert_eq!(backend.read_fan_temp(0).unwrap(), 61);
    assert_eq!(backend.read_fan_temp(1).unwrap(), 54);
    assert!(matches!(
        backend.read_fan_temp(2),
        Err(PlatformError::InvalidArgument(_))
    ));

    // Zero temperature reading maps to SensorUnavailable.
    transport.set_reg(ec_reg::CPU_TEMP_C, 0);
    assert!(matches!(
        backend.read_fan_temp(0),
        Err(PlatformError::SensorUnavailable(0))
    ));
    transport.set_reg(ec_reg::GPU_TEMP_C, 0);
    assert!(matches!(
        backend.read_fan_temp(1),
        Err(PlatformError::SensorUnavailable(1))
    ));

    backend.set_fan_speed_percent(0, 50).unwrap();
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(1));
    assert_eq!(transport.get_reg(ec_reg::CPU_FAN_DUTY), Some(100));
    assert_eq!(backend.read_fan_speed_percent(0).unwrap(), 50);

    backend.set_fan_speed_percent(1, 100).unwrap();
    assert_eq!(transport.get_reg(ec_reg::GPU_FAN_DUTY), Some(200));
    assert_eq!(backend.read_fan_speed_percent(1).unwrap(), 100);

    assert!(matches!(
        backend.set_fan_speed_percent(0, 101),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_fan_speed_percent(2, 50),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_fans_auto().unwrap();
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(0));

    backend.set_perf_profile(3).unwrap();
    assert_eq!(backend.read_perf_profile().unwrap(), 3);
    assert!(matches!(
        backend.set_perf_profile(0),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_perf_profile(4),
        Err(PlatformError::InvalidArgument(_))
    ));

    // TDP PL1/PL2/PL4 bounds enforcement.
    assert_eq!(backend.read_tdp_limits(0).unwrap(), (15, 90));
    assert_eq!(backend.read_tdp_limits(1).unwrap(), (20, 120));
    assert_eq!(backend.read_tdp_limits(2).unwrap(), (25, 140));
    assert!(matches!(
        backend.read_tdp_limits(3),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_tdp(0, 45).unwrap();
    backend.set_tdp(1, 80).unwrap();
    backend.set_tdp(2, 110).unwrap();
    assert_eq!(backend.read_tdp(0).unwrap(), 45);
    assert_eq!(backend.read_tdp(1).unwrap(), 80);
    assert_eq!(backend.read_tdp(2).unwrap(), 110);
    assert!(matches!(
        backend.set_tdp(0, 14),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_tdp(0, 200),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_tdp(3, 50),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_mode_enable(true).unwrap();
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(1));
    backend.set_mode(2).unwrap();
    assert_eq!(transport.get_reg(ec_reg::PERF_MODE), Some(2));
    assert!(matches!(
        backend.set_mode(-1),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_mode(256),
        Err(PlatformError::InvalidArgument(_))
    ));
}

#[test]
fn fan_duty_write_failure_rolls_back_manual_mode_only_when_previously_auto() {
    let transport = Arc::new(MockWmiEcTransport::new(true));
    transport.set_reg(ec_reg::FAN_MANUAL_MODE, 0);
    transport.set_fail_write_reg(Some(ec_reg::CPU_FAN_DUTY));

    let backend =
        WindowsWmiEcBackend::new(Arc::clone(&transport) as _, WmiPlatformLimits::default());

    let err = backend.set_fan_speed_percent(0, 75);
    assert!(matches!(err, Err(PlatformError::Unavailable)));
    // Because FAN_MANUAL_MODE was 0 prior to the call, rollback must restore 0.
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(0));

    // When FAN_MANUAL_MODE was already 1, a failed duty write must leave it at 1.
    transport.set_reg(ec_reg::FAN_MANUAL_MODE, 1);
    let err2 = backend.set_fan_speed_percent(0, 80);
    assert!(matches!(err2, Err(PlatformError::Unavailable)));
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(1));
}

#[test]
fn extended_controls_charge_thresholds_switches_boost_and_ctgp() {
    let transport = Arc::new(MockWmiEcTransport::new(true));
    let backend =
        WindowsWmiEcBackend::new(Arc::clone(&transport) as _, WmiPlatformLimits::default());

    // Charge thresholds validation: bounds 20..=100 and start < end.
    assert!(matches!(
        backend.set_charge_thresholds(19, 80),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_charge_thresholds(80, 101),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_charge_thresholds(80, 80),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        backend.set_charge_thresholds(90, 70),
        Err(PlatformError::InvalidArgument(_))
    ));

    backend.set_charge_thresholds(40, 80).unwrap();
    assert_eq!(transport.get_reg(ec_reg::CHARGE_START_THRESHOLD), Some(40));
    assert_eq!(transport.get_reg(ec_reg::CHARGE_END_THRESHOLD), Some(80));
    assert_eq!(backend.read_charge_thresholds().unwrap(), (40, 80));

    // Rollback start threshold if writing end threshold fails.
    transport.set_fail_write_reg(Some(ec_reg::CHARGE_END_THRESHOLD));
    assert!(matches!(
        backend.set_charge_thresholds(50, 90),
        Err(PlatformError::Unavailable)
    ));
    assert_eq!(transport.get_reg(ec_reg::CHARGE_START_THRESHOLD), Some(40));
    assert_eq!(transport.get_reg(ec_reg::CHARGE_END_THRESHOLD), Some(80));
    transport.set_fail_write_reg(None);

    // Fn-Lock.
    backend.set_fn_lock(true).unwrap();
    assert_eq!(transport.get_reg(ec_reg::FN_LOCK), Some(1));
    assert!(backend.read_fn_lock().unwrap());
    backend.set_fn_lock(false).unwrap();
    assert!(!backend.read_fn_lock().unwrap());

    // Webcam switch (both helper methods and PlatformBackend trait methods).
    backend.set_webcam_switch(true).unwrap();
    assert_eq!(transport.get_reg(ec_reg::WEBCAM_SWITCH), Some(1));
    assert!(backend.read_webcam_switch().unwrap());
    assert!(backend.read_webcam().unwrap());
    backend.set_webcam(false).unwrap();
    assert_eq!(transport.get_reg(ec_reg::WEBCAM_SWITCH), Some(0));
    assert!(!backend.read_webcam().unwrap());

    // Lightbar power.
    backend.set_lightbar_power(true).unwrap();
    assert_eq!(transport.get_reg(ec_reg::LIGHTBAR_POWER), Some(1));
    assert!(backend.read_lightbar_power().unwrap());
    backend.set_lightbar_power(false).unwrap();
    assert!(!backend.read_lightbar_power().unwrap());

    // Fan boost.
    backend.set_fan_boost(true).unwrap();
    assert_eq!(transport.get_reg(ec_reg::FAN_BOOST), Some(1));
    assert!(backend.read_fan_boost().unwrap());
    backend.set_fan_boost(false).unwrap();
    assert!(!backend.read_fan_boost().unwrap());

    // cTGP offset & Dynamic Boost.
    backend.set_ctgp_offset_watts(15).unwrap();
    assert_eq!(transport.get_reg(ec_reg::CTGP_OFFSET_WATTS), Some(15));
    assert_eq!(backend.read_ctgp_offset_watts().unwrap(), 15);

    backend.set_dynamic_boost_watts(25).unwrap();
    assert_eq!(transport.get_reg(ec_reg::DYNAMIC_BOOST_WATTS), Some(25));
    assert_eq!(backend.read_dynamic_boost_watts().unwrap(), 25);
}

#[test]
fn fault_injection_unavailable_transport_and_poisoned_mutex_recovery() {
    let transport = Arc::new(MockWmiEcTransport::new(false));
    let backend =
        WindowsWmiEcBackend::new(Arc::clone(&transport) as _, WmiPlatformLimits::default());

    assert!(!backend.is_available());
    assert!(matches!(
        backend.read_fan_temp(0),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        backend.set_tdp(0, 45),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        backend.set_charge_thresholds(40, 80),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        transport.call_wmi_method(WMI_METHOD_ID_EC_RW, encode_ec_read_arg(ec_reg::CPU_TEMP_C)),
        Err(PlatformError::Unavailable)
    ));

    // Re-enable transport and verify WMI method recording works.
    transport.set_available(true);
    let read_arg = encode_ec_read_arg(ec_reg::CPU_TEMP_C);
    assert_eq!(
        transport
            .call_wmi_method(WMI_METHOD_ID_EC_RW, read_arg)
            .unwrap(),
        0
    );
    assert!(transport.ops().contains(&WmiEcOp::CallMethod {
        method_id: WMI_METHOD_ID_EC_RW,
        arg: read_arg,
    }));

    // Poison the internal register mutex and verify panic-free recovery.
    transport.set_reg(ec_reg::CPU_TEMP_C, 58);
    let poison_result = catch_unwind(AssertUnwindSafe(|| {
        transport.update_regs(|regs| {
            regs.insert(ec_reg::GPU_TEMP_C, 49);
            panic!("intentional mutex poison");
        });
    }));
    assert!(poison_result.is_err());

    // Subsequent reads and writes must recover the poisoned lock without panicking.
    assert_eq!(backend.read_fan_temp(0).unwrap(), 58);
    assert_eq!(backend.read_fan_temp(1).unwrap(), 49);
    backend.set_fan_boost(true).unwrap();
    assert!(backend.read_fan_boost().unwrap());
}
