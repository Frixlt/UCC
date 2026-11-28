// SPDX-License-Identifier: GPL-3.0-or-later
//! Reproduction integration tests for confirmed `ucc-hardware` findings on commit `1d07126`:
//! - R04: Missing `power/runtime_status` treated as active NVIDIA GPU -> invokes `nvidia-smi -pm 0`.
//! - R05: `select_cooler` falls back to single candidate with a different MAC when a MAC is saved.
//! - N14: `CpuWorker` permanently disables EPP management on transient `EBUSY` under `performance`.
//! - N15: `WindowsWmiEcBackend::set_fan_speed_percent` leaves `FAN_MANUAL_MODE=1` when duty write fails.
//! - N16: `WindowsWmiEcBackend::set_tdp` truncates `watts > 255` via `as u8` instead of rejecting.
//! - N21: `TuxedoIoBackend::set_perf_profile` lacks sysfs `tuxedo_platform_profile` fallback on ioctl failure.
//! - R20: `cpu/topology.rs` enables `target_arch = "x86"` while calling `core::arch::x86_64::__*cpuid*`.

use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use ucc_core::UccProfile;
use ucc_hardware::ioctl::requests;
use ucc_hardware::traits::MockHardwareIo;
use ucc_hardware::{
    ec_reg, select_cooler, CpuWorker, DeviceProcessRunner, DiscoveredCooler, LctDeviceModel,
    MockSysfs, MockWmiEcTransport, PlatformBackend, PlatformError, PowerDeviceController,
    SavedCoolerIdentity, TuxedoIoBackend, WindowsWmiEcBackend, WmiEcTransport, WmiPlatformLimits,
};

#[test]
fn r04_missing_runtime_status_must_not_invoke_nvidia_smi() {
    let tmp = TempDir::new().unwrap();
    let sys = tmp.path().join("sys");
    let proc = tmp.path().join("proc");

    let pci_dev = sys.join("bus/pci/devices/0000:01:00.0");
    fs::create_dir_all(pci_dev.join("power")).unwrap();
    fs::write(pci_dev.join("vendor"), "0x10de\n").unwrap();
    fs::write(pci_dev.join("class"), "0x030000\n").unwrap();
    fs::write(pci_dev.join("power/control"), "auto\n").unwrap();
    // Deliberately omit `power/runtime_status` to simulate an unreadable/missing sysfs node.

    let nvidia_proc = proc.join("driver/nvidia");
    fs::create_dir_all(&nvidia_proc).unwrap();
    fs::write(nvidia_proc.join("version"), "NVRM version: 550.0\n").unwrap();

    type RecordedCalls = Arc<Mutex<Vec<(String, Vec<String>)>>>;
    let calls: RecordedCalls = Arc::new(Mutex::new(Vec::new()));
    let calls_cb = Arc::clone(&calls);
    let runner: DeviceProcessRunner = Arc::new(move |prog: &str, args: &[&str]| {
        calls_cb.lock().unwrap().push((
            prog.to_string(),
            args.iter().map(|s| (*s).to_string()).collect(),
        ));
        true
    });

    let controller = PowerDeviceController::new(&sys, &proc, runner, tmp.path().join("bin"));
    controller
        .set_settings(json!({
            "enabled": true,
            "keepNvidiaAwake": false,
            "cpu": {},
            "devices": [
                {
                    "type": "pci",
                    "name": "NVIDIA dGPU",
                    "path": "/sys/bus/pci/devices/0000:01:00.0/power/control",
                    "ac_val": "auto",
                    "type_c_val": "auto",
                    "bat_val": "auto"
                }
            ]
        }))
        .unwrap();

    controller.apply("bat");

    let recorded = calls.lock().unwrap();
    let invoked_nvidia_smi_pm0 = recorded
        .iter()
        .any(|(prog, args)| prog == "nvidia-smi" && args == &["-pm", "0"]);
    assert!(
        !invoked_nvidia_smi_pm0,
        "R04: missing power/runtime_status with keepNvidiaAwake=false must not invoke `nvidia-smi -pm 0`, \
         recorded calls: {:?}",
        *recorded
    );
}

#[test]
fn r05_select_cooler_must_not_pick_different_mac_when_mac_saved() {
    let candidate = DiscoveredCooler {
        device_path: "/org/bluez/hci0/dev_11_22_33_44_55_66".to_string(),
        address: "11:22:33:44:55:66".to_string(),
        name: "LCT21001".to_string(),
        model: LctDeviceModel::Lct21001,
        tx_char_path: None,
        rx_char_path: None,
        connected: false,
    };
    let saved = SavedCoolerIdentity {
        address: Some("AA:BB:CC:DD:EE:FF".to_string()),
        name: None,
    };

    let selected = select_cooler(std::slice::from_ref(&candidate), Some(&saved));
    assert_eq!(
        selected, None,
        "R05: select_cooler must return None when saved MAC does not match and no saved name exists"
    );
}

fn create_single_core_intel_pstate_fixture(root: &Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("present"), "0").unwrap();
    fs::write(root.join("possible"), "0").unwrap();
    fs::write(root.join("online"), "0").unwrap();

    let pstate = root.join("intel_pstate");
    fs::create_dir_all(&pstate).unwrap();
    fs::write(pstate.join("no_turbo"), "0").unwrap();
    fs::write(pstate.join("hwp_dynamic_boost"), "0").unwrap();

    let cpufreq = root.join("cpu0/cpufreq");
    fs::create_dir_all(&cpufreq).unwrap();
    fs::write(cpufreq.join("scaling_driver"), "intel_pstate").unwrap();
    fs::write(cpufreq.join("scaling_governor"), "performance").unwrap();
    fs::write(
        cpufreq.join("scaling_available_governors"),
        "performance powersave",
    )
    .unwrap();
    fs::write(cpufreq.join("energy_performance_preference"), "performance").unwrap();
    fs::write(
        cpufreq.join("energy_performance_available_preferences"),
        "default performance balance_performance balance_power power",
    )
    .unwrap();
    fs::write(cpufreq.join("cpuinfo_min_freq"), "800000").unwrap();
    fs::write(cpufreq.join("cpuinfo_max_freq"), "4800000").unwrap();
    fs::write(cpufreq.join("scaling_min_freq"), "800000").unwrap();
    fs::write(cpufreq.join("scaling_max_freq"), "4800000").unwrap();
}

#[test]
fn n14_cpu_worker_ebusy_on_performance_must_not_permanently_disable_epp() {
    // 1. Verify the `libc::EBUSY` branch in `CpuWorker::apply_cpu_profile` does not permanently
    //    flip `no_epp_write_quirk = true` on a transient EBUSY (e.g. under `performance` governor).
    let worker_src_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cpu/worker.rs");
    let worker_src = fs::read_to_string(&worker_src_path).unwrap_or_else(|e| {
        panic!("Failed to read {}: {e}", worker_src_path.display());
    });

    if let Some(ebusy_idx) = worker_src.find("libc::EBUSY") {
        let after_ebusy = &worker_src[ebusy_idx..];
        if let Some(open_brace) = after_ebusy.find('{') {
            let mut depth = 0usize;
            let mut block_end = after_ebusy.len();
            for (i, ch) in after_ebusy[open_brace..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            block_end = open_brace + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let ebusy_block = &after_ebusy[open_brace..block_end];
            assert!(
                !ebusy_block.contains("no_epp_write_quirk = true"),
                "N14: transient EBUSY on EPP write (e.g. under `performance` governor) must not \
                 permanently set `no_epp_write_quirk = true`; found in EBUSY handler:\n{ebusy_block}"
            );
        }
    }

    // 2. Verify runtime transition on `CpuWorker`: applying `performance` then `powersave` with
    //    `balance_power` EPP succeeds and writes the requested EPP under `powersave`.
    let tmp = TempDir::new().unwrap();
    create_single_core_intel_pstate_fixture(tmp.path());

    let mut worker = CpuWorker::new(tmp.path());

    let mut perf_profile = UccProfile::new("perf", "Performance");
    perf_profile.cpu.governor = "performance".to_string();
    perf_profile.cpu.energy_performance_preference = "balance_power".to_string();
    worker.apply_cpu_profile(&perf_profile).unwrap();

    let mut powersave_profile = UccProfile::new("save", "Powersave");
    powersave_profile.cpu.governor = "powersave".to_string();
    powersave_profile.cpu.energy_performance_preference = "balance_power".to_string();
    worker.apply_cpu_profile(&powersave_profile).unwrap();

    assert!(
        !worker.no_epp_write_quirk,
        "N14: CpuWorker must keep EPP management enabled after switching to powersave"
    );
    let epp_on_disk = fs::read_to_string(
        tmp.path()
            .join("cpu0/cpufreq/energy_performance_preference"),
    )
    .unwrap();
    assert_eq!(
        epp_on_disk.trim(),
        "balance_power",
        "N14: EPP must be written as `balance_power` once governor is `powersave`"
    );
    assert!(worker.validate_cpu_freq(&powersave_profile));
}

struct FailingDutyWmiTransport {
    regs: Mutex<BTreeMap<u16, u8>>,
    fail_reg: u16,
}

impl FailingDutyWmiTransport {
    fn new(fail_reg: u16) -> Self {
        let mut regs = BTreeMap::new();
        regs.insert(ec_reg::FAN_MANUAL_MODE, 0);
        Self {
            regs: Mutex::new(regs),
            fail_reg,
        }
    }

    fn get_reg(&self, addr: u16) -> Option<u8> {
        self.regs.lock().unwrap().get(&addr).copied()
    }
}

impl WmiEcTransport for FailingDutyWmiTransport {
    fn is_available(&self) -> bool {
        true
    }

    fn read_ec_byte(&self, addr: u16) -> Result<u8, PlatformError> {
        Ok(self.regs.lock().unwrap().get(&addr).copied().unwrap_or(0))
    }

    fn write_ec_byte(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
        if addr == self.fail_reg {
            return Err(PlatformError::Unavailable);
        }
        self.regs.lock().unwrap().insert(addr, val);
        Ok(())
    }

    fn call_wmi_method(&self, _method_id: u32, _arg: u64) -> Result<u64, PlatformError> {
        Ok(0)
    }
}

#[test]
fn n15_windows_wmi_failed_duty_write_restores_auto_mode() {
    let transport = Arc::new(FailingDutyWmiTransport::new(ec_reg::CPU_FAN_DUTY));
    let backend =
        WindowsWmiEcBackend::new(Arc::clone(&transport) as _, WmiPlatformLimits::default());

    let res = backend.set_fan_speed_percent(0, 60);
    assert!(
        res.is_err(),
        "Expected CPU fan duty write failure to propagate as Err"
    );
    assert_eq!(
        transport.get_reg(ec_reg::FAN_MANUAL_MODE),
        Some(0),
        "N15: failed duty write must restore FAN_MANUAL_MODE to 0 (auto) instead of leaving it at 1"
    );
}

#[test]
fn n16_windows_wmi_tdp_above_255_rejected_not_truncated() {
    let transport = Arc::new(MockWmiEcTransport::new(true));
    let limits = WmiPlatformLimits {
        tdp_limits: [(15, 300), (20, 300), (25, 300)],
        ..WmiPlatformLimits::default()
    };
    let backend = WindowsWmiEcBackend::new(Arc::clone(&transport) as _, limits);

    let res = backend.set_tdp(0, 256);
    assert!(
        res.is_err(),
        "N16: set_tdp(0, 256) exceeds 8-bit EC register width and must return Err"
    );
    assert_eq!(
        transport.get_reg(ec_reg::PL1_WATTS),
        None,
        "N16: rejected 256 W TDP must not truncate to 0 and write PL1_WATTS"
    );
}

#[test]
fn n21_tuxedo_io_backend_falls_back_to_platform_profile_sysfs_on_write() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 3);
    hw.set_error(requests::W_UW_PERF_PROF);

    let sysfs = Arc::new(MockSysfs::new());
    let profile_path =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile");
    sysfs.set_file(profile_path, "balanced\n");

    let backend = TuxedoIoBackend::with_sysfs(hw, None, Arc::clone(&sysfs) as _);
    assert_eq!(
        backend.read_perf_profile().unwrap(),
        2,
        "Initial sysfs platform_profile `balanced` should map to Uniwill code 2"
    );

    let write_res = backend.set_perf_profile(3);
    assert!(
        write_res.is_ok(),
        "N21: set_perf_profile must fall back to tuxedo_platform_profile sysfs when ioctl fails, \
         got {write_res:?}"
    );
    assert!(
        sysfs.journal().iter().any(|w| w.path == profile_path),
        "N21: fallback must write to {}",
        profile_path.display()
    );
    assert_eq!(
        backend.read_perf_profile().unwrap(),
        3,
        "N21: readback after sysfs fallback write must report profile code 3"
    );
}

#[test]
fn r20_cpu_topology_x86_cfg_does_not_hardcode_x86_64_cpuid() {
    let topology_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cpu/topology.rs");
    let source = fs::read_to_string(&topology_path).unwrap_or_else(|e| {
        panic!("Failed to read {}: {e}", topology_path.display());
    });

    let code_without_line_comments: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    let enables_x86_32 = code_without_line_comments.contains(r#"target_arch = "x86""#);
    let calls_x86_64_cpuid = code_without_line_comments
        .contains("core::arch::x86_64::__get_cpuid_max")
        || code_without_line_comments.contains("core::arch::x86_64::__cpuid_count");
    let calls_x86_32_cpuid = code_without_line_comments
        .contains("core::arch::x86::__get_cpuid_max")
        || code_without_line_comments.contains("core::arch::x86::__cpuid_count");

    assert!(
        !(enables_x86_32 && calls_x86_64_cpuid && !calls_x86_32_cpuid),
        "R20: {} includes target_arch = \"x86\" in cfg while calling core::arch::x86_64 CPUID \
         intrinsics without a 32-bit core::arch::x86 branch",
        topology_path.display()
    );
}

#[test]
fn r21_ble_write_value_has_explicit_timeout_and_tx_proxy_cleanup() {
    let bluez_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bluez_cooler.rs");
    let source = fs::read_to_string(&bluez_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", bluez_path.display()));

    // 1. `write_packet` must wrap `WriteValue` in an explicit `tokio::time::timeout`.
    let wp_start = source
        .find("pub async fn write_packet")
        .expect("bluez_cooler.rs must define `pub async fn write_packet`");
    let wp_end = source[wp_start..]
        .find("pub async fn flush_controller")
        .map(|off| wp_start + off)
        .unwrap_or(source.len());
    let wp_body = &source[wp_start..wp_end];
    assert!(
        wp_body.contains("timeout("),
        "R21: BluezGattSession::write_packet must wrap GATT WriteValue in an explicit tokio::time::timeout"
    );

    // 2. If constructing `tx_proxy` fails after `Connect` / `StartNotify`, `connect_cooler` must
    //    disconnect `dev_proxy` instead of leaking a connected BLE session.
    let tx_start = source
        .find("let tx_proxy =")
        .expect("bluez_cooler.rs must construct `tx_proxy`");
    let tx_end = source[tx_start..]
        .find("Ok(BluezGattSession")
        .map(|off| tx_start + off)
        .unwrap_or(source.len());
    let tx_section = &source[tx_start..tx_end];
    assert!(
        tx_section.contains("Disconnect"),
        "R21: connect_cooler must disconnect dev_proxy when tx_proxy construction fails"
    );
}

#[test]
fn p0_05_sysfs_root_production_enforces_sys_proc_prefix_and_ignores_env() {
    use ucc_hardware::SysfsRoot;

    struct EnvGuard(Option<String>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("UCC_SYSFS_ROOT", v),
                None => std::env::remove_var("UCC_SYSFS_ROOT"),
            }
        }
    }

    let tmp = TempDir::new().unwrap();
    let _guard = EnvGuard(std::env::var("UCC_SYSFS_ROOT").ok());
    std::env::set_var("UCC_SYSFS_ROOT", tmp.path());

    let prod = SysfsRoot::production();
    assert_eq!(
        prod.root_path(),
        Path::new("/"),
        "P0-05: SysfsRoot::production() must ignore UCC_SYSFS_ROOT and stay rooted at `/`"
    );
    assert!(
        prod.resolve("/etc/passwd").is_err(),
        "P0-05: SysfsRoot::production() must reject paths outside /sys and /proc"
    );
    assert!(
        prod.resolve("/tmp/ucc_escape").is_err(),
        "P0-05: SysfsRoot::production() must reject /tmp paths"
    );

    // Verify `read_string` does not blindly call `fs::read_to_string(&full_path)` without symlink protection.
    let sysfs_src_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sysfs.rs");
    let sysfs_src = fs::read_to_string(&sysfs_src_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", sysfs_src_path.display()));
    let read_start = sysfs_src
        .find("pub fn read_string")
        .expect("sysfs.rs must define `pub fn read_string`");
    let read_end = sysfs_src[read_start..]
        .find("pub fn write_string")
        .map(|off| read_start + off)
        .unwrap_or(sysfs_src.len());
    let read_body = &sysfs_src[read_start..read_end];
    assert!(
        read_body.contains("O_NOFOLLOW")
            || read_body.contains("symlink_metadata")
            || read_body.contains("/proc/self/fd/"),
        "P0-05: SysfsRoot::read_string must enforce symlink/descriptor boundary checks instead of raw fs::read_to_string"
    );
}

#[test]
fn p0_07_nvapi_filters_null_gpu_handles() {
    let nvml_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/nvml.rs");
    let source = fs::read_to_string(&nvml_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", nvml_path.display()));

    let handles_start = source
        .find("self.gpu_handles =")
        .expect("nvml.rs must populate `self.gpu_handles`");
    let handles_end = source[handles_start..]
        .find(';')
        .map(|off| handles_start + off + 1)
        .unwrap_or(source.len());
    let handles_stmt = &source[handles_start..handles_end];

    let filters_at_enum = handles_stmt.contains("is_null()");
    let checks_before_bus = source
        .find("fn find_handle_by_bdf")
        .map(|start| {
            let section = &source[start..source.len().min(start + 1500)];
            section.contains("is_null()")
        })
        .unwrap_or(false);

    assert!(
        filters_at_enum || checks_before_bus,
        "P0-07: nvml.rs must filter `!p.is_null()` before storing NVAPI GPU handles or calling fn_bus/fn_voltage"
    );
}

#[test]
fn m_sec_08_resolve_trusted_program_fails_closed_no_path_fallback() {
    use std::os::unix::fs::PermissionsExt;
    use ucc_hardware::default_process_runner;

    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("pwned.marker");
    let evil_bin = tmp.path().join("evil_helper");
    fs::write(
        &evil_bin,
        format!(
            "#!/bin/sh\necho executed > \"{}\"\nexit 0\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&evil_bin, fs::Permissions::from_mode(0o755)).unwrap();

    let runner = default_process_runner();
    let ok = runner(evil_bin.to_str().unwrap(), &[]);
    assert!(
        !ok && !marker.exists(),
        "M-SEC-08: default_process_runner must reject arbitrary /tmp/... executable paths"
    );

    // Source verification: resolve_trusted_program must fail closed (Option) with no bare PATH fallback.
    let ctrl_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/devices/controller.rs");
    let source = fs::read_to_string(&ctrl_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", ctrl_path.display()));
    let fn_start = source
        .find("fn resolve_trusted_program")
        .expect("devices/controller.rs must define resolve_trusted_program");
    let fn_end = source[fn_start..]
        .find("pub fn default_process_runner")
        .map(|off| fn_start + off)
        .unwrap_or(source.len());
    let fn_body = &source[fn_start..fn_end];
    assert!(
        !fn_body.contains("if program.contains('/') {\n        return program;\n    }"),
        "M-SEC-08: resolve_trusted_program must not blindly accept arbitrary slash-containing paths"
    );
    assert!(
        fn_body.contains("Option<"),
        "M-SEC-08: resolve_trusted_program must return Option and fail closed instead of falling back to untrusted PATH"
    );
}

#[test]
fn p0_cooler_manual_overheat_failsafe_triggers_blocks_turnoff_and_releases() {
    use std::time::Instant;
    use ucc_core::{FanPoint, FanProfile};
    use ucc_hardware::cooler::{
        CoolerController, CoolerError, CoolerMode, CoolerPacketEncoder, FanTarget,
        MockBleTransport, PumpTarget, PumpVoltage, BLE_WRITE_GAP, OVERHEAT_RELEASE_TEMP_C,
        OVERHEAT_TRIGGER_TEMP_C,
    };

    let profile = FanProfile {
        table_water_cooler_fan: vec![FanPoint::new(40, 30), FanPoint::new(80, 80)],
        ..Default::default()
    };
    let transport = MockBleTransport::new(true);
    let mut ctrl = CoolerController::new();
    ctrl.on_connected();

    // Put cooler in Manual mode with low fan and low pump.
    ctrl.set_manual_fan_speed(20).unwrap();
    ctrl.set_manual_pump_voltage(PumpVoltage::V7).unwrap();
    assert_eq!(ctrl.mode(), CoolerMode::Manual);
    let mut now = Instant::now();
    ctrl.flush_ready(&transport, now).unwrap();
    now += BLE_WRITE_GAP;
    ctrl.flush_ready(&transport, now).unwrap();
    transport.clear();

    // Below trigger (89C): overheat override remains inactive.
    ctrl.apply_profile_temperature(&profile, OVERHEAT_TRIGGER_TEMP_C - 1)
        .unwrap();
    assert!(!ctrl.is_overheat_override_active());
    assert_eq!(ctrl.desired_fan_target(), FanTarget::Speed(20));

    // At trigger (90C): overheat override latches EVEN in Manual mode and enqueues max cooling.
    ctrl.apply_profile_temperature(&profile, OVERHEAT_TRIGGER_TEMP_C)
        .unwrap();
    assert!(ctrl.is_overheat_override_active());
    assert_eq!(ctrl.mode(), CoolerMode::Manual);
    assert_eq!(ctrl.desired_fan_target(), FanTarget::Speed(100));
    assert_eq!(
        ctrl.desired_pump_target(),
        PumpTarget::Voltage(PumpVoltage::V11)
    );

    now += BLE_WRITE_GAP;
    assert_eq!(ctrl.flush_ready(&transport, now).unwrap(), 1);
    now += BLE_WRITE_GAP;
    assert_eq!(ctrl.flush_ready(&transport, now).unwrap(), 1);
    let pkts = transport.packets();
    assert_eq!(
        pkts,
        vec![
            CoolerPacketEncoder::fan_speed(100).unwrap(),
            CoolerPacketEncoder::pump_voltage(PumpVoltage::V11).unwrap(),
        ]
    );

    // While overheat override is active, turn-offs and reductions below max must be blocked.
    assert_eq!(
        ctrl.turn_off_manual_fan(),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.turn_off_manual_pump(),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.turn_off_fan(),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.turn_off_pump(),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.set_manual_fan_speed(50),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.set_manual_pump_voltage(PumpVoltage::V8),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.set_fan_speed(30),
        Err(CoolerError::OverheatProtectionActive)
    );
    assert_eq!(
        ctrl.set_pump_voltage(PumpVoltage::V7),
        Err(CoolerError::OverheatProtectionActive)
    );

    // Hysteresis: at exactly OVERHEAT_RELEASE_TEMP_C (80C), override must remain active.
    ctrl.apply_profile_temperature(&profile, OVERHEAT_RELEASE_TEMP_C)
        .unwrap();
    assert!(ctrl.is_overheat_override_active());

    // Below release (79C): override clears and restores previous Manual targets.
    transport.clear();
    ctrl.apply_profile_temperature(&profile, OVERHEAT_RELEASE_TEMP_C - 1)
        .unwrap();
    assert!(!ctrl.is_overheat_override_active());
    assert_eq!(ctrl.desired_fan_target(), FanTarget::Speed(20));
    assert_eq!(
        ctrl.desired_pump_target(),
        PumpTarget::Voltage(PumpVoltage::V7)
    );
    now += BLE_WRITE_GAP;
    assert_eq!(ctrl.flush_ready(&transport, now).unwrap(), 1);
    now += BLE_WRITE_GAP;
    assert_eq!(ctrl.flush_ready(&transport, now).unwrap(), 1);
    assert_eq!(
        transport.packets(),
        vec![
            CoolerPacketEncoder::fan_speed(20).unwrap(),
            CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap(),
        ]
    );

    // Manual turn-off is allowed again once cooled below release threshold.
    assert!(ctrl.turn_off_manual_fan().is_ok());

    // Invalid/out-of-range sensor temperature also latches overheat protection.
    ctrl.apply_profile_temperature(&profile, -1).unwrap();
    assert!(ctrl.is_overheat_override_active());
    assert_eq!(ctrl.desired_fan_target(), FanTarget::Speed(100));
    assert_eq!(
        ctrl.desired_pump_target(),
        PumpTarget::Voltage(PumpVoltage::V11)
    );
}

#[test]
fn p1_cpu_controller_rolls_back_on_partial_core_failure() {
    use std::os::unix::fs::PermissionsExt;
    use ucc_hardware::cpu::CpuController;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(root.join("present"), "0-1").unwrap();
    fs::write(root.join("possible"), "0-1").unwrap();

    for idx in 0..2 {
        let cpu = root.join(format!("cpu{idx}"));
        let cpufreq = cpu.join("cpufreq");
        fs::create_dir_all(&cpufreq).unwrap();
        if idx > 0 {
            fs::write(cpu.join("online"), "1").unwrap();
        }
        fs::write(cpufreq.join("cpuinfo_min_freq"), "800000").unwrap();
        fs::write(cpufreq.join("cpuinfo_max_freq"), "4800000").unwrap();
        fs::write(cpufreq.join("scaling_min_freq"), "800000").unwrap();
        fs::write(cpufreq.join("scaling_max_freq"), "4800000").unwrap();
        fs::write(cpufreq.join("scaling_governor"), "powersave").unwrap();
        fs::write(
            cpufreq.join("scaling_available_governors"),
            "performance powersave",
        )
        .unwrap();
    }

    // Make cpu1/cpufreq/scaling_max_freq read-only to force partial failure on core 1.
    let cpu1_max = root.join("cpu1/cpufreq/scaling_max_freq");
    fs::set_permissions(&cpu1_max, fs::Permissions::from_mode(0o444)).unwrap();

    let ctrl = CpuController::new(root);
    let res = ctrl.set_governor_scaling_max_frequency(Some(3200000));
    // Restore permissions before asserting so TempDir cleans up cleanly.
    fs::set_permissions(&cpu1_max, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        res.is_err(),
        "Write to read-only cpu1 scaling_max_freq must fail"
    );
    assert_eq!(
        ctrl.cores[0].read_scaling_max_freq(),
        Some(4800000),
        "Core 0 scaling_max_freq must be rolled back to 4800000 after core 1 failure"
    );

    // Verify set_enabled_cores rolls back earlier core mutations in reverse order.
    fs::write(root.join("cpu1/online"), "0").unwrap();
    let mut ops: Vec<(u32, bool)> = Vec::new();
    let err = ctrl.set_enabled_cores(&[0, 1], |core, enable| {
        ops.push((core.core_index, enable));
        false
    });
    assert!(err.is_err());
    assert_eq!(ops, vec![(1, true)]);
}

#[test]
fn p1_windows_power_set_throttle_bounds_rolls_back_on_partial_failure() {
    use ucc_hardware::windows_power::{
        PowerGuid, WindowsPowerController, WindowsPowerError, WindowsPowerSource,
        WindowsPowerTransport, GUID_PROCESSOR_THROTTLE_MAXIMUM, GUID_PROCESSOR_THROTTLE_MINIMUM,
    };

    struct FailOnSetting {
        values: Mutex<BTreeMap<PowerGuid, u32>>,
        fail_setting: PowerGuid,
    }
    impl WindowsPowerTransport for FailOnSetting {
        fn is_available(&self) -> bool {
            true
        }
        fn read_value_index(
            &self,
            _source: WindowsPowerSource,
            _subgroup: PowerGuid,
            setting: PowerGuid,
        ) -> Result<u32, WindowsPowerError> {
            Ok(*self.values.lock().unwrap().get(&setting).unwrap_or(&0))
        }
        fn write_value_index(
            &self,
            _source: WindowsPowerSource,
            _subgroup: PowerGuid,
            setting: PowerGuid,
            value: u32,
        ) -> Result<(), WindowsPowerError> {
            if setting == self.fail_setting {
                return Err(WindowsPowerError::Unavailable);
            }
            self.values.lock().unwrap().insert(setting, value);
            Ok(())
        }
        fn apply_active_scheme(&self) -> Result<(), WindowsPowerError> {
            Ok(())
        }
    }

    let mut init = BTreeMap::new();
    init.insert(GUID_PROCESSOR_THROTTLE_MINIMUM, 10);
    init.insert(GUID_PROCESSOR_THROTTLE_MAXIMUM, 90);
    let transport = Arc::new(FailOnSetting {
        values: Mutex::new(init),
        fail_setting: GUID_PROCESSOR_THROTTLE_MAXIMUM,
    });
    let ctrl = WindowsPowerController::new(Arc::clone(&transport) as _);

    assert!(ctrl
        .set_throttle_bounds(WindowsPowerSource::Ac, 30, 80)
        .is_err());
    assert_eq!(
        *transport
            .values
            .lock()
            .unwrap()
            .get(&GUID_PROCESSOR_THROTTLE_MINIMUM)
            .unwrap(),
        10,
        "Minimum throttle must be rolled back to 10 when maximum throttle write fails"
    );
}

#[test]
fn p1_fan_controller_write_failsafe_backoff_auto_recovers_and_reads_measured_speed() {
    use ucc_core::{FanPoint, FanProfile};
    use ucc_hardware::fan_controller::{FanController, WRITE_FAILSAFE_INITIAL_BACKOFF_MS};
    use ucc_hardware::platform::{
        MockPlatformBackend, PlatformBackendKind, PlatformCapabilities, PlatformWrite,
    };

    struct MeasuredSpeedBackend {
        inner: MockPlatformBackend,
        measured: [u8; 2],
    }

    impl PlatformBackend for MeasuredSpeedBackend {
        fn kind(&self) -> PlatformBackendKind {
            self.inner.kind()
        }
        fn is_available(&self) -> bool {
            self.inner.is_available()
        }
        fn capabilities(&self) -> PlatformCapabilities {
            self.inner.capabilities()
        }
        fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
            self.inner.read_fan_temp(fan_idx)
        }
        fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
            Ok(self.measured[fan_idx])
        }
        fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
            self.inner.set_fan_speed_percent(fan_idx, percent)
        }
        fn set_fans_auto(&self) -> Result<(), PlatformError> {
            self.inner.set_fans_auto()
        }
        fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
            self.inner.read_tdp(idx)
        }
        fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
            self.inner.read_tdp_limits(idx)
        }
        fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
            self.inner.set_tdp(idx, watts)
        }
        fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
            self.inner.set_perf_profile(value)
        }
        fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
            self.inner.set_mode_enable(value)
        }
        fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
            self.inner.set_mode(value)
        }
    }

    let inner = MockPlatformBackend::uniwill_default();
    inner.set_fan_temp(0, 70);
    inner.set_fan_temp(1, 70);
    let backend = MeasuredSpeedBackend {
        inner,
        measured: [62, 58],
    };

    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 60)],
        table_gpu: vec![FanPoint::new(40, 60)],
        ..Default::default()
    };

    let mut ctrl = FanController::new(&backend, profile, false);

    // Normal tick reads measured fan speed via read_fan_speed_percent (not raw target 60).
    let snap = ctrl.tick(&backend, true, 1_000);
    assert!(!snap.fallback_auto);
    assert_eq!(snap.speeds_percent, vec![Some(62), Some(58)]);

    // Simulate 2 consecutive write failures -> enters write failsafe auto.
    backend.inner.set_fail_writes(true);
    let s1 = ctrl.tick(&backend, true, 2_000);
    assert!(!s1.fallback_auto);
    let _s2 = ctrl.tick(&backend, true, 3_000);
    backend.inner.set_fail_writes(false);

    // Before backoff expires (3_000 + 5_000 = 8_000), controller stays in failsafe auto.
    let s3 = ctrl.tick(&backend, true, 4_000);
    assert!(s3.fallback_auto);
    assert!(backend
        .inner
        .journal()
        .contains(&PlatformWrite::SetFansAuto));

    // After backoff expires, controller automatically probes and recovers manual control.
    let s4 = ctrl.tick(&backend, true, 3_000 + WRITE_FAILSAFE_INITIAL_BACKOFF_MS);
    assert!(
        !s4.fallback_auto,
        "FanController must auto-recover from write_failsafe_active after backoff elapses"
    );
    assert_eq!(s4.speeds_percent, vec![Some(62), Some(58)]);
}

#[tokio::test]
async fn p1_fan_worker_drop_synchronously_restores_fans_auto() {
    use std::time::Duration;
    use ucc_core::{FanPoint, FanProfile};
    use ucc_hardware::fan_controller::{spawn_fan_worker, FanWorkerConfig};
    use ucc_hardware::platform::{MockPlatformBackend, PlatformWrite};

    let backend = Arc::new(MockPlatformBackend::uniwill_default());
    backend.set_fan_temp(0, 65);
    backend.set_fan_temp(1, 60);
    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 60)],
        table_gpu: vec![FanPoint::new(40, 50)],
        ..Default::default()
    };

    let worker = spawn_fan_worker(
        Arc::clone(&backend) as _,
        FanWorkerConfig {
            profile,
            fan_control_enabled: true,
            same_speed: false,
            temporary_curves: None,
        },
        Duration::from_millis(15),
    );
    for _ in 0..50 {
        if worker.latest_snapshot().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    backend.clear_journal();

    // Drop without calling shutdown().await -> Drop must synchronously call set_fans_auto().
    drop(worker);
    assert!(
        backend.journal().contains(&PlatformWrite::SetFansAuto),
        "Dropping FanWorkerHandle without shutdown() must synchronously call set_fans_auto()"
    );
}
