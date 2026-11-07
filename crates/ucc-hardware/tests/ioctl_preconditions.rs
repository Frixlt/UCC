use ucc_hardware::detection::DeviceVendor;
use ucc_hardware::ioctl::{
    read_uniwill_fan_speed, read_uniwill_fan_temp, read_uniwill_tdp_limits, requests,
    set_clevo_fan_speed, set_clevo_fans_auto, set_perf_profile, set_uniwill_fan_speed,
    set_uniwill_fans_auto, set_uniwill_mode, set_uniwill_mode_enable, set_uniwill_tdp, IoctlError,
};
use ucc_hardware::traits::{HardwareIoWrite, MockHardwareIo};

#[test]
fn test_exact_pointer_sized_ioctl_encoding() {
    // Verified 64-bit Linux kernel ioctl codes with 8-byte pointer size encoding
    assert_eq!(requests::R_MOD_VERSION, 0x8008ec00);
    assert_eq!(requests::R_HWCHECK_CL, 0x8008ec05);
    assert_eq!(requests::R_HWCHECK_UW, 0x8008ec06);

    // Clevo interface
    assert_eq!(requests::R_CL_HW_IF_STR, 0x8008ed00);
    assert_eq!(requests::R_CL_FANINFO1, 0x8008ed10);
    assert_eq!(requests::W_CL_FANSPEED, 0x4008ee10);
    assert_eq!(requests::W_CL_FANAUTO, 0x4008ee11);
    assert_eq!(requests::W_CL_PERF_PROFILE, 0x4008ee15);

    // Uniwill interface
    assert_eq!(requests::R_UW_HW_IF_STR, 0x8008ef00);
    assert_eq!(requests::R_UW_MODEL_ID, 0x8008ef01);
    assert_eq!(requests::R_UW_FANSPEED, 0x8008ef10);
    assert_eq!(requests::W_UW_FANSPEED, 0x4008f010);
    assert_eq!(requests::W_UW_FANSPEED2, 0x4008f011);
    assert_eq!(requests::W_UW_MODE, 0x4008f012);
    assert_eq!(requests::W_UW_MODE_ENABLE, 0x4008f013);
    assert_eq!(requests::W_UW_FANAUTO, 0x0000f014); // _IO, size 0
    assert_eq!(requests::W_UW_TDP0, 0x4008f015);
    assert_eq!(requests::W_UW_TDP1, 0x4008f016);
    assert_eq!(requests::W_UW_TDP2, 0x4008f017);
    assert_eq!(requests::W_UW_PERF_PROF, 0x4008f018);
}

#[test]
fn test_clevo_fan_speed_preconditions_and_fail_closed() {
    let mock = MockHardwareIo::new();

    // 1. Valid bounds: fan_idx 0..=2, percent 0..=100
    assert!(set_clevo_fan_speed(&mock, 0, 0).is_ok());
    assert!(set_clevo_fan_speed(&mock, 1, 50).is_ok());
    assert!(set_clevo_fan_speed(&mock, 2, 100).is_ok());

    let j = mock.journal();
    assert_eq!(j.len(), 3);
    assert!(matches!(
        j[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_CL_FANSPEED,
            val: 0,
            ..
        }
    ));
    assert!(matches!(
        j[1],
        HardwareIoWrite::WriteI32 {
            request: requests::W_CL_FANSPEED,
            val: 306, // (1 << 8) | 50
            ..
        }
    ));
    assert!(matches!(
        j[2],
        HardwareIoWrite::WriteI32 {
            request: requests::W_CL_FANSPEED,
            val: 612, // (2 << 8) | 100
            ..
        }
    ));

    mock.clear_journal();

    // 2. Precondition failure: fan_idx out of bounds (>= 3)
    let res = set_clevo_fan_speed(&mock, 3, 50);
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty(), "Fail-closed: no write on invalid fan_idx");

    // 3. Precondition failure: percent out of bounds (> 100)
    let res = set_clevo_fan_speed(&mock, 0, 101);
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty(), "Fail-closed: no write on invalid percent");
}

#[test]
fn test_clevo_fans_auto() {
    let mock = MockHardwareIo::new();
    assert!(set_clevo_fans_auto(&mock).is_ok());

    let j = mock.journal();
    assert_eq!(j.len(), 1);
    assert!(matches!(
        j[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_CL_FANAUTO,
            val: 0x0f, // 1 | (1<<1) | (1<<2) | (1<<3)
            ..
        }
    ));
}

#[test]
fn test_uniwill_fan_speed_preconditions_and_fail_closed() {
    let mock = MockHardwareIo::new();

    // 1. Valid: fan 0 (CPU)
    assert!(set_uniwill_fan_speed(&mock, 0, 60).is_ok());
    // Valid: fan 1 (GPU)
    assert!(set_uniwill_fan_speed(&mock, 1, 80).is_ok());

    let j = mock.journal();
    assert_eq!(j.len(), 2);
    assert!(matches!(
        j[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_FANSPEED,
            val: 60,
            ..
        }
    ));
    assert!(matches!(
        j[1],
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_FANSPEED2,
            val: 80,
            ..
        }
    ));

    mock.clear_journal();

    // 2. Precondition failure: fan index >= 2
    let res = set_uniwill_fan_speed(&mock, 2, 50);
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty());

    // 3. Precondition failure: percentage > 100
    let res = set_uniwill_fan_speed(&mock, 0, 150);
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty());

    // 4. Auto fan control via call_none
    assert!(set_uniwill_fans_auto(&mock).is_ok());
    let j2 = mock.journal();
    assert_eq!(j2.len(), 1);
    assert!(matches!(
        j2[0],
        HardwareIoWrite::CallNone {
            request: requests::W_UW_FANAUTO,
            ..
        }
    ));
}

#[test]
fn test_uniwill_tdp_limits_validation() {
    let mock = MockHardwareIo::new();

    // 1. Valid TDP within explicit hardware boundary (45W..115W)
    assert!(set_uniwill_tdp(&mock, 0, 75, Some((45, 115))).is_ok());
    assert!(matches!(
        mock.journal()[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_TDP0,
            val: 75,
            ..
        }
    ));
    mock.clear_journal();

    // 2. Precondition failure: wattage < min
    let res = set_uniwill_tdp(&mock, 0, 40, Some((45, 115)));
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty());

    // 3. Precondition failure: wattage > max
    let res = set_uniwill_tdp(&mock, 0, 120, Some((45, 115)));
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty());

    // 4. Precondition failure: invalid TDP index (> 2)
    let res = set_uniwill_tdp(&mock, 3, 75, Some((45, 115)));
    assert!(matches!(res, Err(IoctlError::PreconditionFailed(_))));
    assert!(mock.journal().is_empty());

    // 5. Physical safe envelope bounds (0..=500W) when no explicit limits provided
    assert!(set_uniwill_tdp(&mock, 1, 150, None).is_ok());
    assert!(matches!(
        set_uniwill_tdp(&mock, 1, -5, None),
        Err(IoctlError::PreconditionFailed(_))
    ));
    assert!(matches!(
        set_uniwill_tdp(&mock, 1, 550, None),
        Err(IoctlError::PreconditionFailed(_))
    ));
}

#[test]
fn test_perf_profile_and_modes() {
    let mock = MockHardwareIo::new();

    // Mode enable
    assert!(set_uniwill_mode_enable(&mock, true).is_ok());
    assert!(matches!(
        mock.journal()[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_MODE_ENABLE,
            val: 1,
            ..
        }
    ));
    mock.clear_journal();

    // Negative mode index rejected
    assert!(matches!(
        set_uniwill_mode(&mock, -1),
        Err(IoctlError::PreconditionFailed(_))
    ));
    assert!(mock.journal().is_empty());

    // Perf profiles per vendor
    assert!(set_perf_profile(&mock, DeviceVendor::Clevo, 2).is_ok());
    assert!(matches!(
        mock.journal()[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_CL_PERF_PROFILE,
            val: 2,
            ..
        }
    ));
    mock.clear_journal();

    assert!(set_perf_profile(&mock, DeviceVendor::Uniwill, 3).is_ok());
    assert!(matches!(
        mock.journal()[0],
        HardwareIoWrite::WriteI32 {
            request: requests::W_UW_PERF_PROF,
            val: 3,
            ..
        }
    ));
    mock.clear_journal();

    assert!(matches!(
        set_perf_profile(&mock, DeviceVendor::Unsupported, 1),
        Err(IoctlError::PreconditionFailed(_))
    ));
    assert!(mock.journal().is_empty());
}

#[test]
fn test_read_helpers_and_device_unavailable_guarantees() {
    let mock = MockHardwareIo::new();

    // Setup mock reads
    mock.set_i32_read(requests::R_UW_FANSPEED, 2500);
    mock.set_i32_read(requests::R_UW_FAN_TEMP, 65);
    mock.set_i32_read(requests::R_UW_TDP0_MIN, 35);
    mock.set_i32_read(requests::R_UW_TDP0_MAX, 125);

    assert_eq!(read_uniwill_fan_speed(&mock, 0).unwrap(), 2500);
    assert_eq!(read_uniwill_fan_temp(&mock, 0).unwrap(), 65);
    assert_eq!(read_uniwill_tdp_limits(&mock, 0).unwrap(), (35, 125));

    // Device unavailable check (FT-F01-03)
    mock.set_available(false);

    assert!(matches!(
        set_clevo_fan_speed(&mock, 0, 50),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        set_clevo_fans_auto(&mock),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        set_uniwill_fan_speed(&mock, 0, 50),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        set_uniwill_fans_auto(&mock),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        set_uniwill_tdp(&mock, 0, 50, None),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        read_uniwill_fan_speed(&mock, 0),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        read_uniwill_fan_temp(&mock, 0),
        Err(IoctlError::DeviceUnavailable)
    ));
    assert!(matches!(
        read_uniwill_tdp_limits(&mock, 0),
        Err(IoctlError::DeviceUnavailable)
    ));

    // Absolutely zero writes recorded when device unavailable
    assert!(mock.journal().is_empty());
}
