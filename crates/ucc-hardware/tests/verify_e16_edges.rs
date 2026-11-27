use libloading::Library;
use std::process::Command;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use ucc_hardware::{NvidiaPowerGuard, NvmlWrapper, SysfsRoot};

static E16_TEST_MUTEX: Mutex<()> = Mutex::new(());

fn compile_extra_nvml(dir: &Path, with_bdf: bool) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("../../extra-fixtures/nvml.c");
    let so_path = dir.join("libnvidia-ml.so.1");
    let mut cmd = Command::new("cc");
    if with_bdf {
        cmd.arg("-DAUDIT_WITH_BDF");
    }
    let status = cmd
        .args(["-shared", "-fPIC"])
        .arg(&src)
        .arg("-o")
        .arg(&so_path)
        .status()
        .expect("cc must be available");
    assert!(status.success(), "failed to compile extra-fixtures/nvml.c");
    so_path
}

fn gpu(root: &Path, bdf: &str, status: &str) -> PathBuf {
    let device = root.join(bdf);
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    fs::write(device.join("power/runtime_status"), status).unwrap();
    device
}

fn counter(lib: &Library, symbol: &[u8]) -> u32 {
    unsafe { lib.get::<unsafe extern "C" fn() -> u32>(symbol).unwrap()() }
}

#[test]
fn index_only_driver_does_not_guess_reversed_order() {
    let _lock = E16_TEST_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_extra_nvml(lib_dir.path(), false);
    let lib = unsafe { Library::new(&so_path).unwrap() };
    assert!(unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditBdfCalls") }.is_err());
    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "active");
    gpu(dir.path(), "0000:03:00.0", "suspended");
    std::env::set_var("UCC_E16_FIXTURE_FORWARD_ORDER", "1");
    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    let result = wrapper.get_telemetry(0).map(|sample| sample.temperature_c);
    std::env::remove_var("UCC_E16_FIXTURE_FORWARD_ORDER");
    assert_eq!(counter(&lib, b"nvmlAuditSleepingProbes"), 0, "{result:?}");
}

#[test]
fn bdf_lookup_failure_never_writes_to_an_unidentified_gpu() {
    let _lock = E16_TEST_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_extra_nvml(lib_dir.path(), true);
    let lib = unsafe { Library::new(&so_path).unwrap() };
    assert_eq!(counter(&lib, b"nvmlAuditBdfCalls"), 0);
    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "active");
    gpu(dir.path(), "0000:03:00.0", "suspended");
    std::env::set_var("UCC_E16_FIXTURE_FORWARD_ORDER", "1");
    std::env::set_var("UCC_E16_FIXTURE_REFUSE_BDF", "1");
    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    let result = wrapper.set_power_limit(0, 45_000);
    std::env::remove_var("UCC_E16_FIXTURE_FORWARD_ORDER");
    std::env::remove_var("UCC_E16_FIXTURE_REFUSE_BDF");
    assert_eq!(counter(&lib, b"nvmlAuditBdfCalls"), 1);
    assert!(
        result.is_err() && counter(&lib, b"nvmlAuditSleepingProbes") == 0,
        "{result:?}"
    );
}

#[test]
fn bdf_lookup_keeps_the_identity_checked_before_enumeration() {
    let _lock = E16_TEST_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_extra_nvml(lib_dir.path(), true);
    let lib = unsafe { Library::new(&so_path).unwrap() };
    assert_eq!(counter(&lib, b"nvmlAuditBdfCalls"), 0);
    let dir = tempfile::tempdir().unwrap();
    let pci = dir.path().join("pci");
    let incoming = dir.path().join("incoming");
    let active = gpu(&pci, "0000:03:00.0", "suspended");
    let new_gpu = gpu(&incoming, "0000:02:00.0", "suspended");
    let target = pci.join("0000:02:00.0");
    let wrapper = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), &so_path).unwrap();
    fs::write(active.join("power/runtime_status"), "active").unwrap();
    std::env::set_var("UCC_E16_FIXTURE_HOTPLUG_FROM", &new_gpu);
    std::env::set_var("UCC_E16_FIXTURE_HOTPLUG_TO", &target);
    let result = wrapper.get_telemetry(0).map(|sample| sample.temperature_c);
    std::env::remove_var("UCC_E16_FIXTURE_HOTPLUG_FROM");
    std::env::remove_var("UCC_E16_FIXTURE_HOTPLUG_TO");
    assert!(
        target.exists(),
        "fixture must insert a new PCI device during enumeration"
    );
    assert_eq!(counter(&lib, b"nvmlAuditSleepingProbes"), 0, "{result:?}");
}

#[test]
fn root_replacement_cannot_authorize_an_external_file() {
    let _lock = E16_TEST_MUTEX.lock().unwrap();
    if std::env::var_os("UCC_PRELOAD_CHILD").is_none() {
        let preload_dir = tempfile::tempdir().unwrap();
        let src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/open-ancestor.c");
        let so_path = preload_dir.path().join("open-ancestor.so");
        let cc_status = Command::new("cc")
            .args(["-shared", "-fPIC"])
            .arg(&src)
            .args(["-ldl", "-o"])
            .arg(&so_path)
            .status()
            .expect("cc must be available");
        assert!(cc_status.success(), "failed to compile open-ancestor.c");

        let exe = std::env::current_exe().unwrap();
        let status = Command::new(exe)
            .env("LD_PRELOAD", &so_path)
            .env("UCC_PRELOAD_CHILD", "1")
            .args([
                "--exact",
                "root_replacement_cannot_authorize_an_external_file",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "preload child failed for root_replacement_cannot_authorize_an_external_file"
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(root.join("node"), "inside").unwrap();
    let external = outside.join("node");
    fs::write(&external, "outside original").unwrap();
    std::env::set_var("UCC_E1_SWAP_ANCESTOR", &root);
    std::env::set_var("UCC_E1_SWAP_OPEN_PATH", root.join("node"));
    std::env::set_var("UCC_E1_SWAP_DESTINATION", &outside);
    let result = SysfsRoot::custom(&root).write_string("/node", "changed");
    for key in [
        "UCC_E1_SWAP_ANCESTOR",
        "UCC_E1_SWAP_OPEN_PATH",
        "UCC_E1_SWAP_DESTINATION",
    ] {
        std::env::remove_var(key);
    }
    assert!(
        fs::symlink_metadata(&root)
            .map(|m| m.is_symlink())
            .unwrap_or(false),
        "LD_PRELOAD open-ancestor fixture must swap root to a symlink"
    );
    assert!(
        result.is_err(),
        "swapped symlink write must be rejected: {result:?}"
    );
    assert_eq!(
        fs::read_to_string(external).unwrap(),
        "outside original",
        "{result:?}"
    );
}
