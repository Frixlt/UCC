use std::fs;
use std::path::Path;
use std::process::Command;
use ucc_hardware::nvml::{NvidiaPowerGuard, NvmlWrapper};

#[test]
fn gpu_telemetry_recovers_after_device_wakes() {
    let lib_dir = tempfile::tempdir().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stand-fixtures/nvml.c");
    let so_path = lib_dir.path().join("libnvidia-ml.so.1");
    let status_cc = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&src)
        .arg("-o")
        .arg(&so_path)
        .status()
        .unwrap();
    assert!(status_cc.success());

    let dir = tempfile::tempdir().unwrap();
    let device = dir.path().join("0000:02:00.0");
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    let status = device.join("power/runtime_status");
    fs::write(&status, "suspended").unwrap();
    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    fs::write(&status, "active").unwrap();
    let temp = wrapper
        .get_telemetry(0)
        .map(|data| data.temperature_c)
        .unwrap();
    assert_eq!(temp, 42, "Telemetry recovered from mock NVML: temp={temp}");
}
