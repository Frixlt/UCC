use libloading::Library;
use std::process::Command;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier, Mutex},
};
use ucc_hardware::{set_uniwill_tdp, MockHardwareIo, NvidiaPowerGuard, NvmlWrapper, SysfsRoot};

static TEST_MUTEX: Mutex<()> = Mutex::new(());

fn compile_stand_nvml(dir: &Path) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("../../stand-fixtures/nvml.c");
    let so_path = dir.join("libnvidia-ml.so.1");
    let status = Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&src)
        .arg("-o")
        .arg(&so_path)
        .status()
        .expect("cc must be available");
    assert!(status.success(), "failed to compile stand-fixtures/nvml.c");
    so_path
}

fn gpu(root: &Path, id: &str, status: &str) -> std::path::PathBuf {
    let dev = root.join(id);
    fs::create_dir_all(dev.join("power")).unwrap();
    fs::write(dev.join("vendor"), "0x10de").unwrap();
    fs::write(dev.join("class"), "0x030000").unwrap();
    let node = dev.join("power/runtime_status");
    fs::write(&node, status).unwrap();
    node
}

#[test]
fn nvml_index_order_is_not_an_environment_override() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_stand_nvml(lib_dir.path());
    let lib = unsafe { Library::new(&so_path).unwrap() };
    let count = unsafe {
        lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditSleepingProbes")
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "active");
    gpu(dir.path(), "0000:03:00.0", "suspended");
    // Only the fake driver knows its actual device order.
    assert!(std::env::var_os("UCC_AUDIT_REVERSED_NVML_INDEX").is_none());
    std::env::set_var("UCC_E1_FIXTURE_REVERSED_ORDER", "1");
    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    let _ = wrapper.get_telemetry(0);
    std::env::remove_var("UCC_E1_FIXTURE_REVERSED_ORDER");
    assert_eq!(unsafe { count() }, 0, "handle requested for suspended BDF");
}

#[test]
fn concurrent_first_queries_leave_no_nvml_initialization_reference() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let so_path = compile_stand_nvml(lib_dir.path());
    let lib = unsafe { Library::new(&so_path).unwrap() };
    let refs = unsafe {
        lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditRefCount")
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    let status = gpu(dir.path(), "0000:02:00.0", "suspended");
    let wrapper = Arc::new(
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap(),
    );
    fs::write(status, "active").unwrap();
    std::env::set_var("UCC_E1_FIXTURE_SLOW_INIT", "1");
    let gate = Arc::new(Barrier::new(3));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let worker = wrapper.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                worker.get_telemetry(0).unwrap().temperature_c
            })
        })
        .collect();
    gate.wait();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), 42);
    }
    std::env::remove_var("UCC_E1_FIXTURE_SLOW_INIT");
    drop(wrapper);
    assert_eq!(
        unsafe { refs() },
        0,
        "concurrent init left an unmatched reference"
    );
}

#[test]
fn caller_tdp_range_does_not_bypass_unavailable_hardware_limits() {
    let hw = MockHardwareIo::new();
    let result = set_uniwill_tdp(&hw, 0, 500, Some((1, 500)));
    assert!(
        result.is_err() && hw.journal().is_empty(),
        "{result:?}, {:?}",
        hw.journal()
    );
}

#[test]
fn ancestor_replacement_cannot_truncate_an_external_file() {
    let _lock = TEST_MUTEX.lock().unwrap();
    if std::env::var_os("UCC_PRELOAD_CHILD").is_none() {
        let preload_dir = tempfile::tempdir().unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/open-ancestor.c");
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
                "ancestor_replacement_cannot_truncate_an_external_file",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "preload child failed for ancestor_replacement_cannot_truncate_an_external_file"
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let parent = root.join("sub");
    fs::create_dir_all(&parent).unwrap();
    fs::write(parent.join("node"), "inside").unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let external_file = outside.join("node");
    fs::write(&external_file, "must survive").unwrap();
    std::env::set_var("UCC_E1_SWAP_ANCESTOR", &parent);
    std::env::set_var("UCC_E1_SWAP_OPEN_PATH", parent.join("node"));
    std::env::set_var("UCC_E1_SWAP_DESTINATION", &outside);
    let result = SysfsRoot::custom(root).write_string("/sub/node", "changed");
    for key in [
        "UCC_E1_SWAP_ANCESTOR",
        "UCC_E1_SWAP_OPEN_PATH",
        "UCC_E1_SWAP_DESTINATION",
    ] {
        std::env::remove_var(key);
    }
    assert!(
        fs::symlink_metadata(&parent)
            .map(|m| m.is_symlink())
            .unwrap_or(false),
        "LD_PRELOAD open-ancestor fixture must swap parent to a symlink"
    );
    assert!(result.is_err(), "swapped symlink write must be rejected: {result:?}");
    assert_eq!(
        fs::read_to_string(external_file).unwrap(),
        "must survive",
        "result={result:?}"
    );
}
