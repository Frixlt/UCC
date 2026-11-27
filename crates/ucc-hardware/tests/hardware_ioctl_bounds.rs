use std::fs;
use ucc_core::UccProfile;
use ucc_hardware::{
    requests, set_uniwill_tdp, CpuWorker, HardwareIo, IoctlError, MockHardwareIo, TuxedoIoDevice,
};

#[test]
fn unknown_integer_read_cannot_reach_ioctl() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let dev = TuxedoIoDevice::open(file.path()).unwrap();
    // BLKGETSIZE64 expects u64* and is unrelated to /dev/tuxedo_io.
    // The call here uses only the temporary regular file.
    let res = HardwareIo::read_i32(&dev, 0x80081272);
    assert!(
        matches!(res, Err(IoctlError::PreconditionFailed(_))),
        "{res:?}"
    );
}

#[test]
fn string_ioctl_with_high_bits_cannot_bypass_prevalidation() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let dev = TuxedoIoDevice::open(file.path()).unwrap();
    let res = HardwareIo::read_i32(&dev, requests::R_MOD_VERSION | (1_u64 << 32));
    assert!(
        matches!(res, Err(IoctlError::PreconditionFailed(_))),
        "{res:?}"
    );
}

#[test]
fn caller_limits_cannot_override_smaller_driver_tdp_limits() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_UW_TDP0_MIN, 35);
    hw.set_i32_read(requests::R_UW_TDP0_MAX, 160);
    let res = set_uniwill_tdp(&hw, 0, 500, Some((1, 500)));
    assert!(
        res.is_err() && hw.journal().is_empty(),
        "{res:?}, {:?}",
        hw.journal()
    );
}

#[test]
fn cpu_apply_propagates_hwp_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    for node in ["possible", "present"] {
        fs::write(dir.path().join(node), "0").unwrap();
    }
    fs::create_dir_all(dir.path().join("intel_pstate/hwp_dynamic_boost")).unwrap();
    let mut worker = CpuWorker::new(dir.path());
    let mut profile = UccProfile::new("p", "P");
    profile.cpu.hwp_dynamic_boost = true;
    let res = worker.apply_cpu_profile(&profile);
    assert!(
        res.is_err(),
        "HWP write to a directory was reported as {res:?}"
    );
}
