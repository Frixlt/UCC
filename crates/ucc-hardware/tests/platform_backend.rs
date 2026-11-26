use std::sync::Arc;
use ucc_hardware::ioctl::requests;
use ucc_hardware::traits::{HardwareIoWrite, MockHardwareIo};
use ucc_hardware::{
    DeviceVendor, DmiInfo, MockPlatformBackend, PlatformBackend, PlatformBackendKind,
    PlatformError, PlatformWrite, TuxedoIoBackend,
};

#[test]
fn tuxedo_io_backend_uniwill_reads_and_writes_through_seam() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_FANS_MIN_SPEED, 20);
    hw.set_i32_read(requests::R_UW_FANS_OFF_AVAILABLE, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 3);
    hw.set_i32_read(requests::R_UW_FAN_TEMP, 58);
    hw.set_i32_read(requests::R_UW_FAN_TEMP2, 52);
    hw.set_i32_read(requests::R_UW_FANSPEED, 100);
    hw.set_i32_read(requests::R_UW_TDP0, 65);
    hw.set_i32_read(requests::R_UW_TDP0_MIN, 15);
    hw.set_i32_read(requests::R_UW_TDP0_MAX, 115);

    let dmi = DmiInfo {
        sys_vendor: "MECHREVO".into(),
        board_name: "YAOSHI Series-X6AR55xY".into(),
        board_version: "Standard".into(),
        product_sku: "0001".into(),
        ..Default::default()
    };

    let backend = TuxedoIoBackend::new(hw, Some(&dmi), None);
    let caps = backend.capabilities();
    assert_eq!(backend.kind(), PlatformBackendKind::TuxedoIo);
    assert_eq!(caps.device.vendor, DeviceVendor::Uniwill);
    assert_eq!(caps.device.number_of_fans, 2);
    assert_eq!(caps.tdp_channels, 3);
    assert_eq!(caps.perf_profiles_available, 3);
    assert!(caps.device.water_cooler_supported);

    assert_eq!(backend.read_fan_temp(0).unwrap(), 58);
    assert_eq!(backend.read_fan_temp(1).unwrap(), 52);
    assert_eq!(backend.read_fan_speed_percent(0).unwrap(), 50);
    assert_eq!(backend.read_tdp(0).unwrap(), 65);
    assert_eq!(backend.read_tdp_limits(0).unwrap(), (15, 115));

    backend.set_fan_speed_percent(0, 75).unwrap();
    backend.set_tdp(0, 80).unwrap();
    backend.set_perf_profile(2).unwrap();
    backend.set_mode_enable(true).unwrap();
    backend.set_mode(1).unwrap();
    backend.set_fans_auto().unwrap();

    // Out-of-bounds TDP is rejected fail-closed before touching ioctl
    assert!(backend.set_tdp(0, 140).is_err());

    let journal = backend.hardware_io().journal();
    assert!(journal.iter().any(|w| matches!(
        w,
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_FANSPEED,
            val: 150,
            ..
        }
    )));
    assert!(journal.iter().any(|w| matches!(
        w,
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_TDP0,
            val: 80,
            ..
        }
    )));
    assert!(journal.iter().any(|w| matches!(
        w,
        HardwareIoWrite::CallNone {
            request: requests::W_UW_FANAUTO,
            ..
        }
    )));
}

#[test]
fn tuxedo_io_backend_clevo_handles_faninfo_packing_and_rejects_tdp() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_CL, 1);
    // raw temp = 63 (bits 16..23), speed raw = 128 (~50%)
    hw.set_i32_read(requests::R_CL_FANINFO1, (63 << 16) | 128);
    hw.set_i32_read(requests::R_CL_FANINFO2, (50 << 16) | 64);
    hw.set_i32_read(requests::R_CL_FANINFO3, 1 << 16); // <=1 => SensorUnavailable

    let backend = TuxedoIoBackend::new(hw, None, None);
    let caps = backend.capabilities();
    assert_eq!(caps.device.vendor, DeviceVendor::Clevo);
    assert_eq!(caps.tdp_channels, 0);

    assert_eq!(backend.read_fan_temp(0).unwrap(), 63);
    assert_eq!(backend.read_fan_speed_percent(0).unwrap(), 50);
    assert!(matches!(
        backend.read_fan_temp(2),
        Err(PlatformError::SensorUnavailable(2))
    ));
    assert!(matches!(
        backend.read_tdp(0),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        backend.set_tdp(0, 45),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        backend.set_mode_enable(true),
        Err(PlatformError::Unsupported(_))
    ));

    backend.set_fan_speed_percent(0, 60).unwrap();
    backend.set_fans_auto().unwrap();
}

#[test]
fn tuxedo_io_backend_unsupported_fails_closed() {
    let hw = MockHardwareIo::new();
    let backend = TuxedoIoBackend::new(hw, None, None);
    assert_eq!(backend.kind(), PlatformBackendKind::Unsupported);
    assert!(matches!(
        backend.read_fan_temp(0),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        backend.set_fans_auto(),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn mock_platform_backend_records_journal_and_validates_limits() {
    let mock: Arc<MockPlatformBackend> = Arc::new(MockPlatformBackend::uniwill_default());
    let dyn_backend: &dyn PlatformBackend = mock.as_ref();

    assert_eq!(dyn_backend.kind(), PlatformBackendKind::Mock);
    assert!(dyn_backend.is_available());

    dyn_backend.set_fan_speed_percent(0, 60).unwrap();
    dyn_backend.set_tdp(1, 90).unwrap();
    dyn_backend.set_fans_auto().unwrap();

    assert!(matches!(
        dyn_backend.set_tdp(1, 200),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        dyn_backend.set_fan_speed_percent(5, 50),
        Err(PlatformError::InvalidArgument(_))
    ));

    assert_eq!(
        mock.journal(),
        vec![
            PlatformWrite::SetFanSpeed {
                fan_idx: 0,
                percent: 60
            },
            PlatformWrite::SetTdp {
                index: 1,
                watts: 90
            },
            PlatformWrite::SetFansAuto,
        ]
    );
}

#[test]
fn perf_profile_rejects_negative_and_out_of_range_codes() {
    let mock = MockPlatformBackend::uniwill_default();
    // Uniwill wire codes are 1-based (`1..=3` when 3 profiles available)
    assert!(matches!(
        mock.set_perf_profile(0),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        mock.set_perf_profile(4),
        Err(PlatformError::InvalidArgument(_))
    ));
    mock.set_perf_profile(1).unwrap();
    mock.set_perf_profile(3).unwrap();

    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 2);
    let tuxedo = TuxedoIoBackend::new(hw, None, None);
    assert!(matches!(
        tuxedo.set_perf_profile(0),
        Err(PlatformError::InvalidArgument(_))
    ));
    assert!(matches!(
        tuxedo.set_perf_profile(3),
        Err(PlatformError::InvalidArgument(_))
    ));
    tuxedo.set_perf_profile(1).unwrap();
    tuxedo.set_perf_profile(2).unwrap();
}

#[test]
fn clevo_rejects_a_profile_code_outside_its_four_known_codes() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_CL, 1);
    let backend = TuxedoIoBackend::new(hw, None, None);
    let result = backend.set_perf_profile(4);
    assert!(
        result.is_err() && backend.hardware_io().journal().is_empty(),
        "{result:?} {:?}",
        backend.hardware_io().journal()
    );
    for valid in 0..=3 {
        backend.set_perf_profile(valid).unwrap();
    }
}

#[test]
fn uniwill_uses_wire_codes_instead_of_zero_based_ordinals() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 3);
    let backend = TuxedoIoBackend::new(hw, None, None);
    let zero = backend.set_perf_profile(0);
    let overboost = backend.set_perf_profile(3);
    assert!(
        zero.is_err() && overboost.is_ok(),
        "undefined code 0: {zero:?}, overboost code 3: {overboost:?}"
    );

    // Unknown/insufficient profile count (< 2) fails closed
    let hw_low = MockHardwareIo::new();
    hw_low.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_low.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 1);
    let backend_low = TuxedoIoBackend::new(hw_low, None, None);
    assert!(matches!(
        backend_low.set_perf_profile(1),
        Err(PlatformError::Unsupported(_))
    ));
}
