// SPDX-License-Identifier: GPL-3.0-or-later

#[allow(unused_imports)]
use super::common::*;
#[allow(unused_imports)]
use serde_json::json;
#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use std::path::Path;
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use ucc_config::{
    create_default_settings, default_high_performance_profile, default_quiet_profile, ConfigManager,
};
#[allow(unused_imports)]
use ucc_dbus::{
    AllowAllAuthorizer, UccdBackend, UccdLegacyProxy, UccdProxy, DBUS_PATH, DBUS_PATH_ALT,
};
#[allow(unused_imports)]
use ucc_hardware::{
    decode_history_binary, default_process_runner, DeviceCapabilities, DeviceVendor, MetricId,
    MockPlatformBackend, MockSysfs, NvidiaPowerGuard, NvmlWrapper, PlatformBackend,
    PlatformBackendKind, PlatformCapabilities, PlatformError, PlatformWrite, PowerDeviceController,
    TelemetryConfig,
};
#[allow(unused_imports)]
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
#[allow(unused_imports)]
use uccd::{
    apply_wakeup_policy, is_allowed_wakeup_device, DaemonRuntime, LidAction, LidPolicyConfig,
    LiveUccdBackend, UccdDependencies, WakeupApplyResult, WakeupPolicy,
};
#[allow(unused_imports)]
use zbus::export::futures_core::Stream;
#[allow(unused_imports)]
use zbus::zvariant::OwnedValue;

pub(crate) type SysfsWriteGate = (
    String,
    String,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

pub(crate) static DEEP_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

pub(crate) type SysfsReadGate = (
    String,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
);

pub(crate) type SysfsMultiReadGate = (
    String,
    Arc<std::sync::Mutex<usize>>,
    Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
);

#[derive(Default)]
pub(crate) struct SysfsGateControl {
    pub(crate) gate: std::sync::Mutex<Option<SysfsWriteGate>>,
    pub(crate) deny_write_suffix: std::sync::Mutex<Option<String>>,
    pub(crate) deny_read_suffix: std::sync::Mutex<Option<String>>,
    pub(crate) silent_noop_suffix: std::sync::Mutex<Option<String>>,
    pub(crate) rewrite_value: std::sync::Mutex<Option<(String, String, String)>>,
    pub(crate) read_gate: std::sync::Mutex<Option<SysfsReadGate>>,
    pub(crate) multi_read_gate: std::sync::Mutex<Option<SysfsMultiReadGate>>,
}

pub(crate) struct AuditSysfsGate {
    pub(crate) inner: Arc<MockSysfs>,
    pub(crate) ctrl: Arc<SysfsGateControl>,
}

impl ucc_hardware::Sysfs for AuditSysfsGate {
    fn read(&self, path: &Path) -> Result<String, std::io::Error> {
        if let Some(ref suffix) = *self.ctrl.deny_read_suffix.lock().unwrap() {
            if path.ends_with(suffix) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated charging read denied",
                ));
            }
        }
        let maybe_multi = {
            let guard = self.ctrl.multi_read_gate.lock().unwrap();
            guard
                .as_ref()
                .filter(|(suf, _, _)| path.ends_with(suf))
                .map(|(_, entered, signal)| (Arc::clone(entered), Arc::clone(signal)))
        };
        if let Some((entered, signal)) = maybe_multi {
            {
                let mut count = entered.lock().unwrap();
                *count += 1;
                signal.1.notify_all();
            }
            let mut open = signal.0.lock().unwrap();
            while !*open {
                let (g, timeout) = signal.1.wait_timeout(open, Duration::from_secs(5)).unwrap();
                open = g;
                if timeout.timed_out() {
                    break;
                }
            }
        }
        let maybe_gate = {
            let guard = self.ctrl.read_gate.lock().unwrap();
            if let Some((ref suf, ref armed, ref started_tx, ref release_rx)) = *guard {
                if path.ends_with(suf) && armed.load(std::sync::atomic::Ordering::SeqCst) {
                    let tx = started_tx.lock().unwrap().take();
                    if let Some(tx) = tx {
                        let _ = tx.send(());
                    }
                    let rx = release_rx.lock().unwrap().take();
                    rx
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(rx) = maybe_gate {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
        self.inner.read(path)
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
        if let Some(ref suffix) = *self.ctrl.deny_write_suffix.lock().unwrap() {
            if path.ends_with(suffix) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated charging write denied",
                ));
            }
        }
        let maybe_gate = {
            let mut guard = self.ctrl.gate.lock().unwrap();
            let matches = guard
                .as_ref()
                .is_some_and(|(suf, val, _, _)| path.ends_with(suf) && content.trim() == val);
            if matches {
                guard.take()
            } else {
                None
            }
        };
        if let Some((_, _, started, release)) = maybe_gate {
            let _ = started.send(());
            let _ = release.recv_timeout(Duration::from_secs(5));
        }
        if let Some(ref suffix) = *self.ctrl.silent_noop_suffix.lock().unwrap() {
            if path.ends_with(suffix) {
                return Ok(());
            }
        }
        if let Some((ref suffix, ref from, ref to)) = *self.ctrl.rewrite_value.lock().unwrap() {
            if path.ends_with(suffix) && content.trim() == from {
                return self.inner.write(path, to);
            }
        }
        self.inner.write(path, content)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
        self.inner.read_dir(path)
    }
}

pub(crate) struct AuditFallbackFailPlatform {
    pub(crate) inner: Arc<MockPlatformBackend>,
    pub(crate) fail_on_perf_profile: std::sync::atomic::AtomicBool,
}

impl PlatformBackend for AuditFallbackFailPlatform {
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
        self.inner.read_fan_speed_percent(fan_idx)
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
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
    }
    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        if self
            .fail_on_perf_profile
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_perf_profile(value)
    }
    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
        self.inner.set_mode_enable(value)
    }
    fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_mode(value)
    }
}

pub(crate) fn build_backend_with_sysfs_gate(
    tmp: &Path,
    ctrl: Arc<SysfsGateControl>,
    prepare_sysfs: impl FnOnce(&MockSysfs),
    prepare_platform: impl FnOnce(Arc<MockPlatformBackend>) -> Arc<dyn PlatformBackend>,
) -> CustomBackendParts {
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/cpu1/online"), "1");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "58000");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");

    let card = tmp.join("sys/class/drm/card0");
    let rapl = tmp.join("sys/class/powercap/intel-rapl:0:0");
    sysfs.set_file(card.join("gt_act_freq_mhz"), "950");
    sysfs.set_file(card.join("gt_RP0_freq_mhz"), "2200");
    sysfs.set_file(card.join("device/hwmon/hwmon1/temp1_input"), "49000");
    sysfs.set_file(rapl.join("name"), "uncore");
    sysfs.set_file(rapl.join("energy_uj"), "2000000");
    prepare_sysfs(&sysfs);

    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            water_cooler_supported: true,
            ctgp_supported: true,
            hwp_dynamic_boost_supported: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_fan_temp(0, 62);
    platform.set_fan_temp(1, 51);
    platform.set_fan_speed(0, 40);
    platform.set_fan_speed(1, 35);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));

    let lighting = Arc::new(MockLightingTransport::new());
    lighting.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 1,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0],
    }));
    lighting.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 4,
        max_brightness: 255,
    }));

    let gated_sysfs: Arc<dyn ucc_hardware::Sysfs> = Arc::new(AuditSysfsGate {
        inner: Arc::clone(&sysfs),
        ctrl,
    });
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: prepare_platform(platform.clone()),
        sysfs: gated_sysfs,
        lighting_transport: lighting.clone(),
        config_manager: ConfigManager::with_custom_path(tmp.join("settings.json")),
        sys_root: tmp.to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp.to_path_buf(),
            intel_card_path: Some(card),
            nvml: None,
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            tmp.join("sys"),
            tmp.join("proc"),
            default_process_runner(),
            tmp.join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));

    (backend, platform, sysfs, lighting)
}
