use std::fs;
use ucc_hardware::nvml::{GpuRuntimeStatus, NvidiaPowerGuard, NVIDIA_PCI_VENDOR_ID};

#[test]
fn test_nvidia_power_guard_suspended() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dev_dir = temp_dir.path().join("0000:01:00.0");
    let power_dir = dev_dir.join("power");
    fs::create_dir_all(&power_dir).unwrap();

    // Create vendor and runtime_status files
    fs::write(
        dev_dir.join("vendor"),
        format!("{}\n", NVIDIA_PCI_VENDOR_ID),
    )
    .unwrap();
    fs::write(power_dir.join("runtime_status"), "suspended\n").unwrap();

    let guard = NvidiaPowerGuard::new(temp_dir.path());
    let devices = guard.find_nvidia_devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        guard.get_runtime_status(&devices[0]),
        GpuRuntimeStatus::Suspended
    );
    assert!(!guard.is_dgpu_awake()); // Guard must block query to preserve sleep!
}

#[test]
fn test_nvidia_power_guard_active() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dev_dir = temp_dir.path().join("0000:01:00.0");
    let power_dir = dev_dir.join("power");
    fs::create_dir_all(&power_dir).unwrap();

    fs::write(
        dev_dir.join("vendor"),
        format!("{}\n", NVIDIA_PCI_VENDOR_ID),
    )
    .unwrap();
    fs::write(power_dir.join("runtime_status"), "active\n").unwrap();

    let guard = NvidiaPowerGuard::new(temp_dir.path());
    let devices = guard.find_nvidia_devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        guard.get_runtime_status(&devices[0]),
        GpuRuntimeStatus::Active
    );
    assert!(guard.is_dgpu_awake()); // Awake, safe to query!
}

#[test]
fn test_nvidia_power_guard_no_nvidia_gpu() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dev_dir = temp_dir.path().join("0000:00:02.0"); // Intel iGPU
    fs::create_dir_all(&dev_dir).unwrap();
    fs::write(dev_dir.join("vendor"), "0x8086\n").unwrap();

    let guard = NvidiaPowerGuard::new(temp_dir.path());
    let devices = guard.find_nvidia_devices().unwrap();
    assert_eq!(devices.len(), 0);
    assert!(!guard.is_dgpu_awake());
}
