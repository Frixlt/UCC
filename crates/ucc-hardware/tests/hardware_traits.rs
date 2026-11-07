use std::path::Path;
use std::time::Duration;
use ucc_hardware::ioctl::requests;
use ucc_hardware::*;

#[test]
fn test_mock_hardware_io_write_journal_and_reads() {
    let hw = MockHardwareIo::new();
    assert!(hw.is_available());

    // Configure reads
    hw.set_i32_read(requests::R_UW_FANS_MIN_SPEED, 25);
    hw.set_i32_read(requests::R_UW_FANS_OFF_AVAILABLE, 1);
    hw.set_str_read(requests::R_MOD_VERSION, "0.3.0");

    assert_eq!(hw.read_i32(requests::R_UW_FANS_MIN_SPEED).unwrap(), 25);
    assert_eq!(hw.read_i32(requests::R_UW_FANS_OFF_AVAILABLE).unwrap(), 1);
    assert_eq!(hw.read_str(requests::R_MOD_VERSION, 20).unwrap(), "0.3.0");

    // Initially no writes in journal
    assert!(hw.journal().is_empty());

    // Perform writes
    hw.write_i32(requests::W_UW_FANSPEED, 70).unwrap();
    hw.call_none(requests::W_UW_FANAUTO).unwrap();

    let journal = hw.journal();
    assert_eq!(journal.len(), 2);
    match &journal[0] {
        HardwareIoWrite::WriteI32 { request, val, .. } => {
            assert_eq!(*request, requests::W_UW_FANSPEED);
            assert_eq!(*val, 70);
        }
        _ => panic!("Expected WriteI32"),
    }
    match &journal[1] {
        HardwareIoWrite::CallNone { request, .. } => {
            assert_eq!(*request, requests::W_UW_FANAUTO);
        }
        _ => panic!("Expected CallNone"),
    }

    // Test error injection
    hw.set_error(requests::W_UW_FANSPEED);
    assert!(hw.write_i32(requests::W_UW_FANSPEED, 80).is_err());
    // Journal should not have recorded the failed write
    assert_eq!(hw.journal().len(), 2);

    // Test unavailable hardware
    hw.set_available(false);
    assert!(!hw.is_available());
    assert!(matches!(
        hw.read_i32(requests::R_UW_FANS_MIN_SPEED),
        Err(IoctlError::DeviceUnavailable)
    ));
}

#[test]
fn test_mock_sysfs_write_journal_and_isolation() {
    let sysfs = MockSysfs::new();
    let dmi_vendor_path = Path::new("/sys/class/dmi/id/sys_vendor");
    let governor_path = Path::new("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");

    // File not found initially
    assert!(!sysfs.exists(dmi_vendor_path));
    assert!(sysfs.read(dmi_vendor_path).is_err());

    // Set file
    sysfs.set_file(dmi_vendor_path, "TUXEDO");
    assert!(sysfs.exists(dmi_vendor_path));
    assert_eq!(sysfs.read(dmi_vendor_path).unwrap(), "TUXEDO");

    // Write to sysfs node
    assert!(sysfs.journal().is_empty());
    sysfs.write(governor_path, "performance").unwrap();
    assert_eq!(sysfs.read(governor_path).unwrap(), "performance");

    let journal = sysfs.journal();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].path, governor_path);
    assert_eq!(journal[0].content, "performance");

    // Test write error injection (permission denied simulation)
    sysfs.set_write_error(governor_path);
    assert!(sysfs.write(governor_path, "powersave").is_err());
    // Journal still has only 1 successful write
    assert_eq!(sysfs.journal().len(), 1);
}

#[test]
fn test_mock_process_runner_journal_and_matching() {
    let runner = MockProcessRunner::new();

    runner.set_response(
        "prime-select",
        &["query"],
        ProcessOutput::success("on-demand\n"),
    );
    runner.set_response(
        "prime-select",
        &["nvidia"],
        ProcessOutput::success("Set graphics profile to nvidia\n"),
    );

    let q = runner.run("prime-select", &["query"]).unwrap();
    assert_eq!(q.status_code, 0);
    assert_eq!(q.stdout.trim(), "on-demand");

    let set = runner.run("prime-select", &["nvidia"]).unwrap();
    assert_eq!(set.status_code, 0);

    let journal = runner.journal();
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].program, "prime-select");
    assert_eq!(journal[0].args, vec!["query"]);
    assert_eq!(journal[1].args, vec!["nvidia"]);

    // Unconfigured command fails
    assert!(runner.run("unexpected_cmd", &[]).is_err());
}

#[test]
fn test_mock_clock_determinism() {
    let clock = MockClock::new();
    let t0 = clock.now();
    assert_eq!(clock.elapsed_since(t0), Duration::from_secs(0));

    clock.advance(Duration::from_millis(500));
    assert_eq!(clock.elapsed_since(t0), Duration::from_millis(500));

    clock.advance(Duration::from_secs(2));
    assert_eq!(clock.elapsed_since(t0), Duration::from_millis(2500));
}

#[test]
fn test_mock_gpu_backend_and_write_journal() {
    let gpu = MockGpuBackend::new();
    assert!(!gpu.is_suspended());

    gpu.set_telemetry(GpuTelemetry {
        temperature_c: 55,
        fan_speed_percent: Some(40),
        power_usage_mw: Some(35000),
        memory_used_bytes: Some(1024 * 1024 * 512),
        memory_total_bytes: Some(1024 * 1024 * 8192),
        gpu_utilization_percent: Some(25),
        memory_utilization_percent: Some(10),
        is_suspended: false,
    });

    let telem = gpu.get_telemetry().unwrap();
    assert_eq!(telem.temperature_c, 55);
    assert_eq!(telem.power_usage_mw, Some(35000));

    // Power mutations
    gpu.set_power_limit(115).unwrap();
    gpu.set_power_offset(15).unwrap();

    let journal = gpu.journal();
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0], GpuWrite::SetPowerLimit { watts: 115, timestamp: journal[0].timestamp() });
    assert_eq!(journal[1], GpuWrite::SetPowerOffset { offset_watts: 15, timestamp: journal[1].timestamp() });

    // D3cold suspended GPU blocks queries
    gpu.set_suspended(true);
    assert!(gpu.is_suspended());
    assert!(matches!(gpu.get_telemetry(), Err(NvmlError::GpuSuspended)));
    assert!(matches!(gpu.set_power_limit(100), Err(NvmlError::GpuSuspended)));
}

trait TimestampExtract {
    fn timestamp(&self) -> std::time::Instant;
}
impl TimestampExtract for GpuWrite {
    fn timestamp(&self) -> std::time::Instant {
        match self {
            Self::SetPowerLimit { timestamp, .. } => *timestamp,
            Self::SetPowerOffset { timestamp, .. } => *timestamp,
        }
    }
}
