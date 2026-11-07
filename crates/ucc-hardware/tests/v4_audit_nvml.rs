use std::{fs, path::Path};
use ucc_hardware::{NvidiaPowerGuard, NvmlError, NvmlWrapper};

fn add_gpu(root: &Path, bdf: &str, status: &str) {
    let device = root.join(bdf);
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    fs::write(device.join("power/runtime_status"), status).unwrap();
}

#[test]
fn another_active_gpu_does_not_allow_probing_suspended_target() {
    let dir = tempfile::tempdir().unwrap();
    add_gpu(dir.path(), "0000:02:00.0", "active");
    add_gpu(dir.path(), "0000:03:00.0", "suspended");
    let wrapper = NvmlWrapper::new(NvidiaPowerGuard::new(dir.path())).unwrap();
    assert_eq!(wrapper.get_telemetry(0).unwrap().temperature_c, 42);
    let result = wrapper.get_telemetry(1).map(|t| t.temperature_c);
    assert!(matches!(result, Err(NvmlError::GpuSuspended)), "{result:?}");
}

#[test]
fn transient_device_enumeration_failure_recovers() {
    let dir = tempfile::tempdir().unwrap();
    add_gpu(dir.path(), "0000:02:00.0", "active");
    std::env::set_var("UCC_AUDIT_FAIL_FIRST_NVML_COUNT", "1");
    let wrapper = NvmlWrapper::new(NvidiaPowerGuard::new(dir.path()));
    std::env::remove_var("UCC_AUDIT_FAIL_FIRST_NVML_COUNT");
    let wrapper = wrapper.unwrap();
    assert_eq!(
        wrapper.get_telemetry(0).map(|t| t.temperature_c).unwrap(),
        42
    );
}
