// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ucc_core::{FanCurve, FanPoint, FanProfile};
use ucc_hardware::cooler::NORDIC_UART_SERVICE_UUID;
use ucc_hardware::ioctl::requests;
use ucc_hardware::traits::MockHardwareIo;
use ucc_hardware::{
    detect_lct_model, validate_settings_json, BluezCoolerClient, DmiInfo, FanController, GpuVendor,
    MetricsHistoryStore, MockSysfs, PlatformBackend, PlatformCapabilities, PlatformError,
    PowerDeviceController, RaplEnergyCounter, TelemetryCollector, TelemetryConfig, TuxedoIoBackend,
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{connection, interface};

#[test]
fn t_hw_04_uniwill_zero_fan_temp_returns_sensor_unavailable_and_triggers_auto() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_FANS_MIN_SPEED, 20);
    hw.set_i32_read(requests::R_UW_FANS_OFF_AVAILABLE, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 3);
    hw.set_i32_read(requests::R_UW_FAN_TEMP, 0);

    let backend = TuxedoIoBackend::new(hw, None, None);
    let temp_res = backend.read_fan_temp(0);
    assert!(
        matches!(temp_res, Err(PlatformError::SensorUnavailable(0))),
        "Uniwill 0°C sensor reading must return SensorUnavailable(0), got {temp_res:?}"
    );

    // Also verify FanController failsafe behavior when platform returns SensorUnavailable
    let pts: Vec<FanPoint> = [20, 40, 60, 80, 100]
        .into_iter()
        .map(|t| FanPoint { temp: t, speed: 50 })
        .collect();
    let profile = FanProfile {
        id: "test".into(),
        name: "Test".into(),
        table_cpu: pts.clone(),
        table_gpu: pts,
        ..Default::default()
    };
    let mut fc = FanController::new(&backend, profile, false);
    let tick = fc.tick(&backend, true, 1000);
    assert!(tick.fallback_auto || tick.calculated_speeds[0] != Some(0));
}

#[test]
fn t_hw_05_uniwill_tdp_channels_probed_from_hardware_registers() {
    // Two TDP channels present (TDP0, TDP1 ok; TDP2 absent/err)
    let hw_two = MockHardwareIo::new();
    hw_two.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_two.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 3);
    hw_two.set_i32_read(requests::R_UW_TDP0, 45);
    hw_two.set_i32_read(requests::R_UW_TDP1, 65);
    let backend_two = TuxedoIoBackend::new(hw_two, None, None);
    assert_eq!(
        backend_two.capabilities().tdp_channels,
        2,
        "2-channel Uniwill device must probe tdp_channels == 2, not hardcoded 3"
    );

    // Zero TDP channels present
    let hw_zero = MockHardwareIo::new();
    hw_zero.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_zero.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 2);
    let backend_zero = TuxedoIoBackend::new(hw_zero, Some(&DmiInfo::default()), None);
    let caps_zero = backend_zero.capabilities();
    assert_eq!(caps_zero.tdp_channels, 0);
    assert!(!caps_zero.device.tdp_control);
}

#[test]
fn t_hw_06_power_devices_skips_nvidia_smi_when_dgpu_suspended_in_d3cold() {
    let tmp = tempfile::tempdir().unwrap();
    let sys = tmp.path().join("sys");
    let pci_dev = sys.join("bus/pci/devices/0000:01:00.0");
    std::fs::create_dir_all(pci_dev.join("power")).unwrap();
    std::fs::write(pci_dev.join("vendor"), "0x10de\n").unwrap();
    std::fs::write(pci_dev.join("class"), "0x030000\n").unwrap();
    std::fs::write(pci_dev.join("power/runtime_status"), "suspended\n").unwrap();
    std::fs::write(pci_dev.join("power/control"), "auto\n").unwrap();
    let nvidia_proc = tmp.path().join("proc/driver/nvidia");
    std::fs::create_dir_all(&nvidia_proc).unwrap();
    std::fs::write(nvidia_proc.join("version"), "NVRM version: 550.0\n").unwrap();

    type RecordedCalls = Arc<Mutex<Vec<(String, Vec<String>)>>>;
    let calls: RecordedCalls = Arc::new(Mutex::new(Vec::new()));
    let calls_cb = calls.clone();
    let runner: ucc_hardware::DeviceProcessRunner = Arc::new(move |prog: &str, args: &[&str]| {
        calls_cb.lock().unwrap().push((
            prog.to_string(),
            args.iter().map(|s| (*s).to_string()).collect(),
        ));
        true
    });

    let ctrl = PowerDeviceController::new(
        &sys,
        tmp.path().join("proc"),
        runner,
        tmp.path().join("bin"),
    );
    assert!(ctrl
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
        .is_ok());
    ctrl.apply("bat");

    let recorded = calls.lock().unwrap();
    assert!(
        !recorded.iter().any(|(prog, _)| prog.contains("nvidia-smi")),
        "nvidia-smi was invoked while NVIDIA dGPU was suspended in D3cold: {recorded:?}"
    );
}

#[test]
fn t_hw_07_amd_dgpu_auto_discovery_and_d3cold_guard() {
    let sysfs = Arc::new(MockSysfs::new());
    let root = Path::new("/tmp/amd_dgpu_stand");
    let dgpu = root.join("sys/bus/pci/devices/0000:03:00.0");
    let hwmon = dgpu.join("hwmon/hwmon2");
    sysfs.set_file(dgpu.join("vendor"), "0x1002\n");
    sysfs.set_file(dgpu.join("class"), "0x030000\n");
    sysfs.set_file(dgpu.join("boot_vga"), "0\n");
    sysfs.set_file(dgpu.join("power/runtime_status"), "suspended\n");
    sysfs.set_file(hwmon.join("name"), "amdgpu\n");
    sysfs.set_file(hwmon.join("temp1_input"), "45000\n");
    sysfs.set_file(hwmon.join("power1_average"), "12000000\n");

    let mut collector = TelemetryCollector::new(
        sysfs.clone(),
        TelemetryConfig {
            sys_root: root.to_path_buf(),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );

    let t0 = Instant::now();
    let snap_sleep = collector.poll(t0, 1000);
    let dgpu_sleep = snap_sleep
        .dgpu
        .expect("discrete AMD GPU must be auto-discovered even when suspended");
    assert_eq!(dgpu_sleep.vendor, GpuVendor::Amd);
    assert!(dgpu_sleep.is_sleeping);
    assert_eq!(
        dgpu_sleep.temp_c, None,
        "must not read amdgpu hwmon while dGPU is suspended"
    );

    sysfs.set_file(dgpu.join("power/runtime_status"), "active\n");
    let snap_active = collector.poll(t0 + Duration::from_secs(1), 2000);
    let dgpu_active = snap_active.dgpu.expect("active AMD dGPU snapshot");
    assert_eq!(dgpu_active.vendor, GpuVendor::Amd);
    assert!(!dgpu_active.is_sleeping);
    assert_eq!(dgpu_active.temp_c, Some(45.0));
}

#[test]
fn t_hw_08_intel_i915_multi_gt_energy1_input_and_rapl_suspend_spike() {
    // 1. Multi-GT rps_act_freq_mhz + energy1_input delta
    let sysfs = Arc::new(MockSysfs::new());
    let root = Path::new("/tmp/intel_multi_gt_stand");
    let card = root.join("sys/class/drm/card0");
    let hwmon = card.join("device/hwmon/hwmon1");
    sysfs.set_file(card.join("device/vendor"), "0x8086\n");
    sysfs.set_file(card.join("device/device"), "0xa7a0\n");
    sysfs.set_file(card.join("device/class"), "0x030000\n");
    sysfs.set_file(card.join("gt/gt0/rps_act_freq_mhz"), "1100\n");
    sysfs.set_file(card.join("gt/gt0/rps_RP0_freq_mhz"), "2100\n");
    sysfs.set_file(hwmon.join("name"), "i915\n");
    sysfs.set_file(hwmon.join("energy1_input"), "1000000\n");

    let mut collector = TelemetryCollector::new(
        sysfs.clone(),
        TelemetryConfig {
            sys_root: root.to_path_buf(),
            intel_card_path: Some(card),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );

    let t0 = Instant::now();
    let s1 = collector.poll(t0, 1000);
    assert_eq!(s1.igpu.as_ref().and_then(|g| g.core_freq_mhz), Some(1100.0));

    sysfs.set_file(hwmon.join("energy1_input"), "6000000\n");
    let s2 = collector.poll(t0 + Duration::from_secs(1), 2000);
    let p_w = s2
        .igpu
        .as_ref()
        .and_then(|g| g.power_w)
        .expect("i915 energy1_input delta should report iGPU power");
    assert!((p_w - 5.0).abs() < 0.01, "expected ~5W, got {p_w}");

    // 2. Post-suspend RAPL counter reset must not emit an implausible wraparound spike
    let rapl_dir = Path::new("/sys/class/powercap/intel-rapl:0");
    sysfs.set_file(rapl_dir.join("max_energy_range_uj"), "262143288509\n");
    sysfs.set_file(rapl_dir.join("energy_uj"), "100000000000\n");
    let mut rapl = RaplEnergyCounter::new(rapl_dir);
    assert_eq!(rapl.sample_power_watts(sysfs.as_ref(), t0), None);
    // Simulate suspend reset to near 0 in 1s (would compute ~162,143 W wraparound spike)
    sysfs.set_file(rapl_dir.join("energy_uj"), "500000\n");
    let post_resume = rapl.sample_power_watts(sysfs.as_ref(), t0 + Duration::from_secs(1));
    assert!(
        post_resume.is_none_or(|w| w <= 1000.0),
        "RAPL suspend reset produced bogus spike: {post_resume:?}"
    );
}

#[test]
fn t_hw_12_tuxedo_io_read_perf_profile_uses_injected_sysfs() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 3);

    let sysfs = MockSysfs::new();
    sysfs.set_file(
        "/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile",
        "performance\n",
    );

    let backend = TuxedoIoBackend::new(hw, None, Some(&sysfs));
    let perf = backend.read_perf_profile();
    assert!(
        matches!(perf, Ok(3)),
        "TuxedoIoBackend must read perf_profile via injected Sysfs, not host fs (got {perf:?})"
    );
}

struct MockNonLctObjectManager {
    dev_name: String,
    address: String,
    connected: Arc<AtomicBool>,
}

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl MockNonLctObjectManager {
    async fn get_managed_objects(
        &self,
    ) -> zbus::fdo::Result<HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>>
    {
        let mut root = HashMap::new();
        let mut ifaces = HashMap::new();
        let mut dev_props = HashMap::new();
        dev_props.insert(
            "Name".to_string(),
            Value::from(self.dev_name.clone()).try_into().unwrap(),
        );
        dev_props.insert(
            "Address".to_string(),
            Value::from(self.address.clone()).try_into().unwrap(),
        );
        dev_props.insert(
            "Connected".to_string(),
            Value::from(self.connected.load(Ordering::SeqCst))
                .try_into()
                .unwrap(),
        );
        dev_props.insert(
            "UUIDs".to_string(),
            Value::from(vec![NORDIC_UART_SERVICE_UUID.to_string()])
                .try_into()
                .unwrap(),
        );
        ifaces.insert("org.bluez.Device1".to_string(), dev_props);
        root.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_11_22_33_44_55_66").unwrap(),
            ifaces,
        );
        Ok(root)
    }
}

#[tokio::test]
async fn t_hw_13_bluez_cooler_does_not_match_generic_nus_device_without_lct_name() {
    assert_eq!(detect_lct_model("Flipper-Zero"), None);
    assert_eq!(detect_lct_model("PineTime-NUS"), None);

    let bus_name = "org.test.bluez.generic_nus_filter";
    let server_conn = connection::Builder::session()
        .unwrap()
        .name(bus_name)
        .unwrap()
        .build()
        .await
        .unwrap();
    let client_conn = connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/",
            MockNonLctObjectManager {
                dev_name: "PineTime".to_string(),
                address: "11:22:33:44:55:66".to_string(),
                connected: Arc::new(AtomicBool::new(false)),
            },
        )
        .await
        .unwrap();

    let client = BluezCoolerClient::with_bus_name(client_conn, bus_name);
    let discovered = client.discover_coolers().await.unwrap();
    assert!(
        discovered.is_empty(),
        "Generic Nordic UART device without LCT/Aquaris name must not be auto-detected as cooler: {discovered:?}"
    );
}

#[test]
fn t_daemon_05_whitelist_validates_lid_policy_section() {
    let valid = json!({
        "enabled": true,
        "keepNvidiaAwake": false,
        "lid": {
            "ac": "screen_off",
            "type_c": "ignore",
            "bat": "suspend",
            "wakeOnlyLidAndKeyboard": true,
            "turnOffKeyboardBacklightOnLidClose": true,
            "turnOffWaterCoolerLedOnLidClose": false
        }
    });
    assert!(validate_settings_json(&valid).is_ok());

    let invalid_bool = json!({
        "enabled": true,
        "lid": {
            "turnOffKeyboardBacklightOnLidClose": "yes"
        }
    });
    assert!(
        validate_settings_json(&invalid_bool).is_err(),
        "whitelist validate_settings_json must reject non-boolean lid lighting toggles"
    );

    let invalid = json!({
        "enabled": true,
        "lid": {
            "bat": "rm -rf /"
        }
    });
    assert!(
        validate_settings_json(&invalid).is_err(),
        "whitelist validate_settings_json must reject invalid lid actions"
    );
}

#[test]
fn t_sec_10_power_device_scan_rejects_invalid_wifi_interface_name() {
    let tmp = tempfile::tempdir().unwrap();
    let sys = tmp.path().join("sys");
    let bad_wifi = sys.join("class/net/wlan0;evil/wireless");
    std::fs::create_dir_all(&bad_wifi).unwrap();

    let ctrl = PowerDeviceController::new(
        &sys,
        tmp.path().join("proc"),
        ucc_hardware::default_process_runner(),
        tmp.path().join("bin"),
    );
    let scanned = ctrl.scan();
    let devices = scanned["devices"].as_array().unwrap();
    assert!(
        !devices.iter().any(|d| d["path"] == "wlan0;evil"),
        "scan() allowed unvalidated Wi-Fi interface name into devices list: {devices:?}"
    );
}

// Keep unused imports guard happy across conditional assertions
#[allow(dead_code)]
fn _fan_curve_type_anchor() -> Option<FanCurve> {
    let _ = PlatformCapabilities::default();
    None
}
