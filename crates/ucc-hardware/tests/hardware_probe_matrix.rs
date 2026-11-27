use serde_json::json;
use std::{fs, os::unix::fs::symlink};
use ucc_core::UccProfile;
use ucc_hardware::{
    requests, set_clevo_fan_speed, set_uniwill_fan_speed, set_uniwill_tdp, CpuWorker, HardwareIo,
    HardwareIoWrite, IoctlError, MockHardwareIo, PowerDeviceController, SysfsRoot, TuxedoIoDevice,
};

#[test]
fn safe_trait_rejects_incompatible_ioctl_before_syscall() {
    // Ordinary temporary file. ENOTTY proves the malformed call reached ioctl;
    // this test never opens a hardware device.
    let file = tempfile::NamedTempFile::new().unwrap();
    let device = TuxedoIoDevice::open(file.path()).unwrap();
    let result = HardwareIo::read_i32(&device, requests::R_MOD_VERSION);
    assert!(
        matches!(result, Err(IoctlError::PreconditionFailed(_))),
        "{result:?}"
    );
}

#[test]
fn clevo_payload_preserves_other_fans() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_CL_FANINFO1, 0x1411);
    hw.set_i32_read(requests::R_CL_FANINFO3, 0x1433);
    set_clevo_fan_speed(&hw, 1, 100).unwrap();
    assert!(
        matches!(
            hw.journal().as_slice(),
            [HardwareIoWrite::WriteI32 {
                request: requests::W_CL_FANSPEED,
                val: 0x33ff11,
                ..
            }]
        ),
        "{:?}",
        hw.journal()
    );
}

#[test]
fn uniwill_full_speed_is_raw_200() {
    let hw = MockHardwareIo::new();
    set_uniwill_fan_speed(&hw, 1, 100).unwrap();
    assert!(
        matches!(
            hw.journal().as_slice(),
            [HardwareIoWrite::WriteI32 {
                request: requests::W_UW_FANSPEED2,
                val: 200,
                ..
            }]
        ),
        "{:?}",
        hw.journal()
    );
}

#[test]
fn uniwill_tdp_rejects_missing_device_limits() {
    let hw = MockHardwareIo::new();
    assert!(set_uniwill_tdp(&hw, 0, 500, None).is_err());
    assert!(hw.journal().is_empty());
}

#[test]
fn short_device_path_is_rejected_without_panicking() {
    let mut bad = Vec::new();
    for bus in ["pci", "usb"] {
        let config = json!({"devices":[{
            "type":bus, "path":format!("/sys/bus/{bus}/devices/power/control"),
            "name":"malformed", "ac_val":"on", "bat_val":"auto", "type_c_val":"auto"
        }]});
        let result = std::panic::catch_unwind(|| PowerDeviceController::validate(&config));
        if !matches!(result, Ok(Err(_))) {
            bad.push(bus);
        }
    }
    assert!(
        bad.is_empty(),
        "validator failed to reject malformed paths normally: {bad:?}"
    );
}

#[test]
fn sysfs_root_rejects_symlink_escape() {
    let dir = tempfile::tempdir().unwrap();
    let root_path = dir.path().join("fixture");
    fs::create_dir(&root_path).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "before").unwrap();
    symlink(&outside, root_path.join("node")).unwrap();
    let root = SysfsRoot::custom(root_path);
    let result = root.write_string("/node", "changed");
    assert_eq!(fs::read_to_string(outside).unwrap(), "before", "{result:?}");
}

fn cpu_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let cpufreq = dir.path().join("cpu0/cpufreq");
    fs::create_dir_all(&cpufreq).unwrap();
    for name in ["present", "possible"] {
        fs::write(dir.path().join(name), "0").unwrap();
    }
    fs::write(
        cpufreq.join("scaling_available_governors"),
        "powersave performance",
    )
    .unwrap();
    fs::write(cpufreq.join("scaling_governor"), "powersave").unwrap();
    dir
}

#[test]
fn cpu_apply_reports_governor_write_error() {
    let dir = cpu_fixture();
    let governor = dir.path().join("cpu0/cpufreq/scaling_governor");
    fs::remove_file(&governor).unwrap();
    fs::create_dir(&governor).unwrap(); // Deterministic EISDIR, even under root.
    let mut cpu = CpuWorker::new(dir.path());
    let mut profile = UccProfile::new("p", "Profile");
    profile.cpu.governor = "performance".into();
    let result = cpu.apply_cpu_profile(&profile);
    assert!(
        result.is_err(),
        "failed governor write was reported as {result:?}"
    );
}

#[test]
fn cpu_worker_detects_external_governor_override() {
    let dir = cpu_fixture();
    let mut cpu = CpuWorker::new(dir.path());
    let mut profile = UccProfile::new("p", "Profile");
    profile.cpu.governor = "performance".into();
    cpu.apply_cpu_profile(&profile).unwrap();
    let governor = dir.path().join("cpu0/cpufreq/scaling_governor");
    assert_eq!(fs::read_to_string(&governor).unwrap(), "performance");
    fs::write(&governor, "powersave").unwrap();
    assert!(
        !cpu.validate_cpu_freq(&profile),
        "external override is reported as healthy"
    );
}
