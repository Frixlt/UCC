use std::fs;
use ucc_hardware::nvml::{NvidiaPowerGuard, NvmlWrapper};

#[test]
fn gpu_telemetry_recovers_after_device_wakes() {
    let dir = tempfile::tempdir().unwrap();
    let device = dir.path().join("0000:02:00.0");
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    let status = device.join("power/runtime_status");
    fs::write(&status, "suspended").unwrap();
    let wrapper = NvmlWrapper::new(NvidiaPowerGuard::new(dir.path())).unwrap();
    fs::write(&status, "active").unwrap();
    let temp = wrapper.get_telemetry(0).map(|data| data.temperature_c).unwrap();
    assert!(temp == 42 || temp > 0, "Telemetry recovered: temp={temp}");
}
