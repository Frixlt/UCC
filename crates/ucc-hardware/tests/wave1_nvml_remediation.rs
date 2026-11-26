use libloading::Library;
use std::{fs, path::Path, path::PathBuf, process::Command, sync::Mutex};
use ucc_hardware::{NvidiaPowerGuard, NvmlError, NvmlWrapper};

static NVML_REMEDIATION_MUTEX: Mutex<()> = Mutex::new(());

fn gpu(root: &Path, bdf: &str, status: &str) -> PathBuf {
    let dev = root.join(bdf);
    fs::create_dir_all(dev.join("power")).unwrap();
    fs::write(dev.join("vendor"), "0x10de").unwrap();
    fs::write(dev.join("class"), "0x030000").unwrap();
    fs::write(dev.join("power/runtime_status"), status).unwrap();
    dev
}

#[test]
fn hotplug_does_not_change_the_selected_active_pci_device() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mock = tmp.path().join("driver.so");
    let source = tmp.path().join("driver.c");
    let original = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"),
    )
    .unwrap();
    let source_text = original
        .replace(
            "static _Atomic unsigned bdf_calls;",
            "static _Atomic unsigned bdf_calls; static _Atomic unsigned selected_new_gpu;",
        )
        .replace(
            "++bdf_calls;",
            "++bdf_calls; selected_new_gpu = !strcmp(id, \"0000:02:00.0\");",
        )
        + "\nunsigned nvmlAuditSelectedNewGpu(void) { return selected_new_gpu; }\n";
    fs::write(&source, source_text).unwrap();
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(&source)
        .arg("-o")
        .arg(&mock)
        .status()
        .unwrap()
        .success());
    let lib = unsafe { Library::new(&mock).unwrap() };

    let pci = tmp.path().join("pci");
    let orig = gpu(&pci, "0000:03:00.0", "suspended");
    let incoming = gpu(&tmp.path().join("incoming"), "0000:02:00.0", "active");
    let target = pci.join("0000:02:00.0");
    let wrapper = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), &mock).unwrap();
    fs::write(orig.join("power/runtime_status"), "active").unwrap();
    std::env::set_var("UCC_E16_FIXTURE_HOTPLUG_FROM", incoming);
    std::env::set_var("UCC_E16_FIXTURE_HOTPLUG_TO", &target);
    let result = wrapper.get_telemetry(0).map(|sample| sample.temperature_c);
    std::env::remove_var("UCC_E16_FIXTURE_HOTPLUG_FROM");
    std::env::remove_var("UCC_E16_FIXTURE_HOTPLUG_TO");
    assert!(target.exists());
    let selected_new_gpu =
        unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditSelectedNewGpu").unwrap()() };
    assert_eq!(
        selected_new_gpu, 0,
        "initial index 0 was 03:00.0: {result:?}"
    );
}

#[test]
fn index_only_driver_rejects_multiple_gpus_even_when_all_active() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let so_path = lib_dir.path().join("libnvidia-ml.so.1");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&src)
        .arg("-o")
        .arg(&so_path)
        .status()
        .unwrap()
        .success());

    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "active");
    gpu(dir.path(), "0000:03:00.0", "active");

    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    let tel_res = wrapper.get_telemetry(0).map(|t| t.temperature_c);
    let set_res = wrapper.set_power_limit(0, 40_000);
    assert!(
        matches!(tel_res, Err(NvmlError::NvmlCall(_))),
        "index-only telemetry on multi-GPU must fail closed: {tel_res:?}"
    );
    assert!(
        matches!(set_res, Err(NvmlError::NvmlCall(_))),
        "index-only setter on multi-GPU must fail closed: {set_res:?}"
    );
}

#[test]
fn bdf_lookup_allows_explicit_target_when_another_gpu_is_suspended() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let so_path = lib_dir.path().join("libnvidia-ml.so.1");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(&src)
        .arg("-o")
        .arg(&so_path)
        .status()
        .unwrap()
        .success());

    let dir = tempfile::tempdir().unwrap();
    gpu(dir.path(), "0000:02:00.0", "suspended");
    gpu(dir.path(), "0000:03:00.0", "active");

    let wrapper =
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(dir.path()), &so_path).unwrap();
    assert!(matches!(
        wrapper.get_telemetry_by_bdf("0000:02:00.0").err(),
        Some(NvmlError::GpuSuspended)
    ));
    assert_eq!(
        wrapper
            .get_telemetry_by_bdf("0000:03:00.0")
            .unwrap()
            .temperature_c,
        42
    );
    wrapper.set_power_limit_by_bdf("0000:03:00.0", 40_000).unwrap();
}

#[test]
fn index_only_driver_requires_an_unambiguous_singleton_in_both_views() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pci = dir.path().join("pci");
    let device = pci.join("0000:03:00.0");
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    fs::write(device.join("power/runtime_status"), "active").unwrap();

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let modified = dir.path().join("driver.c");
    let source = fs::read_to_string(fixture).unwrap();
    fs::write(
        &modified,
        source
            .replace(
                "static _Atomic unsigned sleeping_probes;",
                "static _Atomic unsigned sleeping_probes; static _Atomic unsigned index_probes; unsigned nvmlAuditIndexProbes(void) { return index_probes; }",
            )
            .replace(
                "unsigned nvmlDeviceGetHandleByIndex_v2(unsigned index, void **out) {",
                "unsigned nvmlDeviceGetHandleByIndex_v2(unsigned index, void **out) { ++index_probes;",
            ),
    )
    .unwrap();
    let mock = dir.path().join("fake.so");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(modified)
        .arg("-o")
        .arg(&mock)
        .status()
        .unwrap()
        .success());
    let lib = unsafe { Library::new(&mock).unwrap() };
    let nvml = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), &mock).unwrap();
    let result = nvml.get_telemetry_by_bdf("0000:03:00.0");
    assert_eq!(nvml.device_count(), 2);
    let index_probes = unsafe {
        lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditIndexProbes")
            .unwrap()()
    };
    assert!(
        result.is_err() && index_probes == 0,
        "driver reports two devices, requested BDF is unverified, index probes={index_probes}, returned sample={}",
        result.is_ok()
    );
}

fn bdf_fixture_with_failures(dir: &Path) -> (NvmlWrapper, Library) {
    let pci = dir.join("pci");
    let device = pci.join("0000:03:00.0");
    fs::create_dir_all(device.join("power")).unwrap();
    fs::write(device.join("vendor"), "0x10de").unwrap();
    fs::write(device.join("class"), "0x030000").unwrap();
    fs::write(device.join("power/runtime_status"), "active").unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let modified = dir.join("driver.c");
    let src = fs::read_to_string(fixture).unwrap();
    let src = src
        .replace("*out = 42; return 0;", "(void)out; return 999;")
        .replace(
            "*min = 10000; *max = 50000; return 0;",
            "(void)min; (void)max; return 999;",
        )
        .replace(
            "unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) { return 0; }",
            "static _Atomic unsigned writes; unsigned nvmlAuditWrites(void) { return writes; } unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) { ++writes; return 0; }",
        );
    fs::write(&modified, src).unwrap();
    let library = dir.join("fake.so");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(modified)
        .arg("-o")
        .arg(&library)
        .status()
        .unwrap()
        .success());
    let lib = unsafe { Library::new(&library).unwrap() };
    let nvml = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), library).unwrap();
    (nvml, lib)
}

#[test]
fn driver_sensor_failure_is_not_reported_as_zero_degrees() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (nvml, _) = bdf_fixture_with_failures(dir.path());
    let result = nvml
        .get_telemetry_by_bdf("0000:03:00.0")
        .map(|sample| sample.temperature_c);
    assert_ne!(
        result.ok(),
        Some(0),
        "mock returns an error from temperature getter"
    );
}

#[test]
fn power_write_requires_known_driver_constraints() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (nvml, lib) = bdf_fixture_with_failures(dir.path());
    let result = nvml.set_power_limit_by_bdf("0000:03:00.0", 45_000);
    let writes = unsafe {
        lib.get::<unsafe extern "C" fn() -> u32>(b"nvmlAuditWrites")
            .unwrap()()
    };
    assert!(
        result.is_err() && writes == 0,
        "{result:?}, writes={writes}"
    );
}

#[test]
fn index_only_rechecks_count_after_driver_topology_changes() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pci = dir.path().join("pci");
    let dev = pci.join("0000:03:00.0");
    fs::create_dir_all(dev.join("power")).unwrap();
    fs::write(dev.join("vendor"), "0x10de").unwrap();
    fs::write(dev.join("class"), "0x030000").unwrap();
    fs::write(dev.join("power/runtime_status"), "active").unwrap();

    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let src = dir.path().join("mock.c");
    let text = fs::read_to_string(template).unwrap();
    fs::write(
        &src,
        format!(
            "static unsigned live_count = 1;\nvoid mock_add_device(void) {{ live_count = 2; }}\n{}",
            text.replace("*out = 2;", "*out = live_count;")
        ),
    )
    .unwrap();
    let so = dir.path().join("mock.so");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(src)
        .arg("-o")
        .arg(&so)
        .status()
        .unwrap()
        .success());
    let lib = unsafe { Library::new(&so).unwrap() };
    let nvml = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(pci), so).unwrap();
    assert_eq!(nvml.device_count(), 1);
    unsafe { lib.get::<extern "C" fn()>(b"mock_add_device").unwrap()() };
    let sample = nvml.get_telemetry_by_bdf("0000:03:00.0");
    assert!(
        sample.is_err(),
        "sysfs has one GPU, NVML now has two, cached count={} and temperature={:?}",
        nvml.device_count(),
        sample.ok().map(|s| s.temperature_c)
    );
}

#[test]
fn power_sensor_failure_does_not_produce_a_zero_sample() {
    let _lock = NVML_REMEDIATION_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pci = dir.path().join("pci");
    let dev = pci.join("0000:03:00.0");
    fs::create_dir_all(dev.join("power")).unwrap();
    fs::write(dev.join("vendor"), "0x10de").unwrap();
    fs::write(dev.join("class"), "0x030000").unwrap();
    fs::write(dev.join("power/runtime_status"), "active").unwrap();

    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let src = dir.path().join("mock.c");
    let text = fs::read_to_string(template)
        .unwrap()
        .replace("*out = 10000; return 0;", "(void)out; return 999;");
    fs::write(&src, text).unwrap();
    let so = dir.path().join("mock.so");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(src)
        .arg("-o")
        .arg(&so)
        .status()
        .unwrap()
        .success());

    let nvml = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(pci), so).unwrap();
    let value = nvml
        .get_telemetry_by_bdf("0000:03:00.0")
        .map(|s| s.power_usage_mw);
    assert_ne!(
        value.ok(),
        Some(0),
        "mock returns an error from power getter"
    );

    // Also verify Option-typed snapshot keeps valid temperature while marking failed power as None
    let snap = nvml.get_telemetry_snapshot_by_bdf("0000:03:00.0").unwrap();
    assert_eq!(snap.temperature_c, Some(42));
    assert_eq!(snap.power_usage_mw, None);
}
