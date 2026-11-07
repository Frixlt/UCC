use libloading::Library;
use std::{fs, path::Path, sync::Mutex};
use ucc_hardware::{NvidiaPowerGuard, NvmlWrapper};

static NVML_TEST_MUTEX: Mutex<()> = Mutex::new(());

fn gpu(root: &Path, id: &str, status: &str) {
    let dev = root.join(id);
    fs::create_dir_all(dev.join("power")).unwrap();
    fs::write(dev.join("vendor"), "0x10de").unwrap();
    fs::write(dev.join("class"), "0x030000").unwrap();
    fs::write(dev.join("power/runtime_status"), status).unwrap();
}

#[test]
fn startup_retry_does_not_leak_nvml_init_reference() {
    let _guard = NVML_TEST_MUTEX.lock().unwrap();
    let lib = unsafe { Library::new("libnvidia-ml.so.1").unwrap() };
    let refs = unsafe {
        lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditRefCount")
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "active");
    std::env::set_var("UCC_AUDIT_FAIL_FIRST_NVML_COUNT", "1");
    let wrapper = NvmlWrapper::new(NvidiaPowerGuard::new(dir.path())).unwrap();
    std::env::remove_var("UCC_AUDIT_FAIL_FIRST_NVML_COUNT");
    assert_eq!(wrapper.get_telemetry(0).unwrap().temperature_c, 42);
    drop(wrapper);
    assert_eq!(
        unsafe { refs() },
        0,
        "successful init without matching shutdown"
    );
}

#[test]
fn gpu_guard_does_not_assume_nvml_enumeration_matches_sorted_bdf() {
    let _guard = NVML_TEST_MUTEX.lock().unwrap();
    let lib = unsafe { Library::new("libnvidia-ml.so.1").unwrap() };
    let count = unsafe {
        lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditSleepingProbes")
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "active");
    gpu(dir.path(), "0000:03:00.0", "suspended");
    std::env::set_var("UCC_AUDIT_REVERSED_NVML_INDEX", "1");
    let wrapper = NvmlWrapper::new(NvidiaPowerGuard::new(dir.path())).unwrap();
    let _ = wrapper.get_telemetry(0);
    std::env::remove_var("UCC_AUDIT_REVERSED_NVML_INDEX");
    assert_eq!(
        unsafe { count() },
        0,
        "NVML attempted to obtain a handle to the sleeping device"
    );
}
