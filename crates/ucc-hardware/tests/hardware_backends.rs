use serde_json::json;
use std::{fs, os::unix::fs::symlink, sync::Arc};
use ucc_core::UccProfile;
use ucc_hardware::{CpuController, CpuWorker, PowerDeviceController};

#[test]
fn whitelisted_device_path_cannot_write_via_external_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let sys = dir.path().join("sys");
    let target = sys.join("bus/pci/devices/0000:02:00.0/power/control");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "original").unwrap();
    symlink(&outside, target).unwrap();
    let c = PowerDeviceController::new(
        &sys,
        dir.path().join("proc"),
        Arc::new(|_, _| true),
        dir.path().join("bin"),
    );
    c.set_settings(json!({"enabled":true,"devices":[{
        "type":"pci","path":"/sys/bus/pci/devices/0000:02:00.0/power/control",
        "name":"GPU", "ac_val":"on", "type_c_val":"auto", "bat_val":"auto"
    }]}))
    .unwrap();
    c.apply("ac");
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "original",
        "status={}",
        c.status()
    );
}

fn cpu_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("cpu0/cpufreq")).unwrap();
    for node in ["present", "possible"] {
        fs::write(dir.path().join(node), "0").unwrap();
    }
    dir
}

#[test]
fn cpu_request_with_missing_frequency_nodes_is_not_reported_applied_and_healthy() {
    let dir = cpu_fixture();
    let mut worker = CpuWorker::new(dir.path());
    let mut profile = UccProfile::new("p", "Profile");
    profile.cpu.max_freq_khz = Some(2_000_000);
    let apply = worker.apply_cpu_profile(&profile);
    assert!(
        apply.is_err() || !worker.validate_cpu_freq(&profile),
        "no frequency nodes, yet apply={apply:?} and readback healthy"
    );
}

#[test]
fn snapping_max_frequency_cannot_exceed_no_turbo_ceiling() {
    let dir = cpu_fixture();
    fs::create_dir(dir.path().join("intel_pstate")).unwrap();
    fs::write(dir.path().join("intel_pstate/no_turbo"), "1").unwrap();
    for (path, value) in [
        ("cpuinfo_min_freq", "400000"),
        ("cpuinfo_max_freq", "3000000"),
        ("scaling_min_freq", "400000"),
        ("scaling_max_freq", "3000000"),
        ("scaling_available_frequencies", "1800000 2100000"),
        ("base_frequency", "2000000"),
    ] {
        fs::write(dir.path().join("cpu0/cpufreq").join(path), value).unwrap();
    }
    let c = CpuController::new(dir.path());
    let max = c.compute_effective_max_freq(&c.cores[0], Some(2_000_000), false);
    assert!(max.is_none_or(|f| f <= 2_000_000), "chosen={max:?}");
}
