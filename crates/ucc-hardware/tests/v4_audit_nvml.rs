use std::process::Command;
use std::{fs, path::Path, path::PathBuf, sync::Mutex};
use ucc_hardware::{NvidiaPowerGuard, NvmlError, NvmlWrapper};

static NVML_AUDIT_MUTEX: Mutex<()> = Mutex::new(());

fn compile_mock_nvml(dir: &Path) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("../../stand-fixtures/nvml.c");
    let so_path = dir.join("libnvidia-ml.so.1");
    let status = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&src)
        .arg("-o")
        .arg(&so_path)
        .status()
        .expect("cc must be available to compile mock NVML library");
    assert!(status.success(), "failed to compile mock NVML library");
    so_path
}

fn add_gpu(root: &Path, bdf: &str, status: &str) {
    let device = root.join(bdf);
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    fs::write(device.join("power/runtime_status"), status).unwrap();
}

#[test]
fn another_active_gpu_does_not_allow_probing_suspended_target() {
    let _guard = NVML_AUDIT_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_mock_nvml(lib_dir.path());
    let dir = tempfile::tempdir().unwrap();
    add_gpu(dir.path(), "0000:02:00.0", "active");
    add_gpu(dir.path(), "0000:03:00.0", "suspended");
    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    assert_eq!(wrapper.get_telemetry(0).unwrap().temperature_c, 42);
    let result = wrapper.get_telemetry(1).map(|t| t.temperature_c);
    assert!(matches!(result, Err(NvmlError::GpuSuspended)), "{result:?}");
}

#[test]
fn transient_device_enumeration_failure_recovers() {
    let _guard = NVML_AUDIT_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_mock_nvml(lib_dir.path());
    let dir = tempfile::tempdir().unwrap();
    add_gpu(dir.path(), "0000:02:00.0", "active");
    std::env::set_var("UCC_AUDIT_FAIL_FIRST_NVML_COUNT", "1");
    let wrapper = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path);
    std::env::remove_var("UCC_AUDIT_FAIL_FIRST_NVML_COUNT");
    let wrapper = wrapper.unwrap();
    assert_eq!(
        wrapper.get_telemetry(0).map(|t| t.temperature_c).unwrap(),
        42
    );
}
