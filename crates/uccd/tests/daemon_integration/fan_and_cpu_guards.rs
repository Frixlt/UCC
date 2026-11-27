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

struct RecheckAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    startup_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    slow_auto: bool,
    tdp_fault: bool,
}

impl RecheckAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            startup_gate: std::sync::Mutex::new(None),
            slow_auto: false,
            tdp_fault: false,
        }
    }
}

impl PlatformBackend for RecheckAuditPlatform {
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
        if let Some((started, release)) = self.startup_gate.lock().unwrap().take() {
            let _ = started.send(());
            let _ = release.recv_timeout(Duration::from_secs(4));
        }
        self.inner.set_fan_speed_percent(fan_idx, percent)
    }
    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        if self.slow_auto {
            std::thread::sleep(Duration::from_millis(320));
        }
        self.inner.set_fans_auto()
    }
    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        if self.tdp_fault && idx == 0 {
            return Err(PlatformError::Unavailable);
        }
        self.inner.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(idx)
    }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        if self.tdp_fault && idx == 1 {
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.inner.read_perf_profile()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_recheck_bus_readiness_waits_for_the_initial_telemetry() {
    let root = tempfile::tempdir().unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = RecheckAuditPlatform::new(p);
            *wrapper.startup_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.edge.start";
    let task = tokio::spawn(start_runtime(backend, name, false));
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    let observer = session_conn().await;
    let bus = zbus::fdo::DBusProxy::new(&observer).await.unwrap();
    let has_name = bus.name_has_owner(name.try_into().unwrap()).await.unwrap();
    let early_frequency = if has_name {
        let proxy = uccd_proxy(&observer, name).await;
        tokio::time::timeout(Duration::from_millis(350), proxy.get_cpu_frequency_mhz())
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    release.send(()).unwrap();
    let running = task.await.unwrap();
    let ready_frequency = uccd_proxy(&observer, name)
        .await
        .get_cpu_frequency_mhz()
        .await
        .unwrap();
    running.shutdown(Duration::from_secs(1)).await;
    assert_eq!(ready_frequency, 3200);
    assert!(
        !has_name || early_frequency.is_none() || early_frequency == Some(ready_frequency),
        "name advertised during activation, before={early_frequency:?}, ready={ready_frequency}"
    );
}

#[test]
fn m8_recheck_runtime_shutdown_deadline_also_caps_tokio_runtime_exit() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = RecheckAuditPlatform::new(p);
            wrapper.slow_auto = true;
            Arc::new(wrapper)
        },
    );
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let runtime = tokio_runtime.block_on(start_runtime(backend, "org.test.uccd.edge.exit", false));
    let started = Instant::now();
    tokio_runtime.block_on(runtime.shutdown(Duration::from_millis(25)));
    let returned = started.elapsed();
    drop(tokio_runtime);
    let exited = started.elapsed();
    assert!(
        exited < Duration::from_millis(175),
        "25 ms budget: shutdown future returned in {returned:?}, Tokio runtime exited after {exited:?}"
    );
}

#[tokio::test]
async fn m8_recheck_shutdown_unexports_api_and_releases_the_name_even_after_timeout() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = RecheckAuditPlatform::new(p);
            wrapper.slow_auto = true;
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.edge.shutdown_name";
    let runtime = start_runtime(backend, name, false).await;
    let connection_for_other_services = runtime.connection().clone();
    let unique = connection_for_other_services
        .unique_name()
        .unwrap()
        .to_string();
    runtime.shutdown(Duration::from_millis(25)).await;
    let observer = session_conn().await;
    let bus = zbus::fdo::DBusProxy::new(&observer).await.unwrap();
    let advertised = bus.name_has_owner(name.try_into().unwrap()).await.unwrap();
    let after = uccd_proxy(&observer, &unique).await.get_device_name().await;
    assert!(
        !advertised && after.is_err(),
        "after shutdown, owner advertised={advertised}, direct API call={after:?}"
    );
}

#[tokio::test]
async fn m8_recheck_invalid_second_tdp_write_cannot_leave_first_channel_changed() {
    let root = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            p.set_tdp_value(0, 60);
            p.set_tdp_value(1, 85);
            p.set_tdp_value(2, 105);
            let mut wrapper = RecheckAuditPlatform::new(p);
            wrapper.tdp_fault = true;
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.edge.tdp_readback";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let mut profile = default_quiet_profile();
    profile.odm_profile_name = None;
    profile.odm_power_limits = vec![42, 75, 90];
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let actual = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        actual, 60,
        "first channel changed without a readable rollback value"
    );
}

#[tokio::test]
async fn m8_recheck_cpu_io_failure_rolls_back_tdp_and_earlier_cpu_writes() {
    let root = tempfile::tempdir().unwrap();
    let cpu = root.path().join("sys/devices/system/cpu");
    let no_turbo = cpu.join("intel_pstate/no_turbo");
    let epp = cpu.join("cpu0/cpufreq/energy_performance_preference");
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&no_turbo, "0");
            sysfs.set_file(&epp, "balance_power");
            sysfs.set_file(
                cpu.join("cpu0/cpufreq/energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.cpu_fault";
    let runtime = start_runtime(backend, name, false).await;
    for (i, power) in [60, 85, 105].into_iter().enumerate() {
        platform.set_tdp_value(i, power);
    }
    let before = sysfs.get_file(&no_turbo).unwrap();
    assert_eq!(before, "0");
    sysfs.set_write_error(&epp);
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let mut profile = default_quiet_profile();
    profile.odm_profile_name = None;
    profile.odm_power_limits = vec![42, 70, 90];
    profile.cpu.energy_performance_preference = "performance".into();
    profile.cpu.no_turbo = true;
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let after_tdp = (0..3)
        .map(|i| platform.read_tdp(i).unwrap())
        .collect::<Vec<_>>();
    let after_turbo = sysfs.get_file(&no_turbo).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!((after_tdp, after_turbo), (vec![60, 85, 105], before));
}

#[tokio::test]
async fn m8_recheck_shutdown_restores_each_cpu_policy_to_its_own_startup_state() {
    let root = tempfile::tempdir().unwrap();
    let cpus = root.path().join("sys/devices/system/cpu");
    let gov0 = cpus.join("cpu0/cpufreq/scaling_governor");
    let gov1 = cpus.join("cpu1/cpufreq/scaling_governor");
    write_distinct_power_profiles(root.path());
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&gov0, "powersave");
            sysfs.set_file(&gov1, "schedutil");
            for i in 0..2 {
                sysfs.set_file(
                    cpus.join(format!("cpu{i}/cpufreq/scaling_available_governors")),
                    "performance powersave schedutil",
                );
            }
        },
        |p| p,
    );
    let runtime = start_runtime(backend, "org.test.uccd.edge.restore_cpu", false).await;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(sysfs.get_file(&gov0).as_deref(), Some("powersave"));
    assert_eq!(sysfs.get_file(&gov1).as_deref(), Some("schedutil"));
}

#[tokio::test]
async fn m8_recheck_battery_profile_is_retried_after_a_transient_apply_error() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = create_default_settings();
    let ac = default_high_performance_profile();
    let mut bat = default_quiet_profile();
    bat.odm_profile_name = None;
    bat.odm_power_limits = vec![45, 125, 130];
    settings.profiles.insert(bat.id.clone(), bat.clone());
    settings.set_active_profile_for_mode("power_ac", &ac.id);
    settings.set_active_profile_for_mode("power_bat", &bat.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();
    let (backend, platform, sysfs, _) = build_test_backend(root.path());
    let name = "org.test.uccd.edge.profile_retry";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    set_battery_files(&sysfs, root.path());
    backend.reconcile_power_state(false);
    assert_eq!(
        active_profile_id(&proxy.get_active_profile_json().await.unwrap()),
        ac.id
    );
    platform.set_tdp_limits(1, (20, 130));
    backend.reconcile_power_state(false);
    let actual = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        actual, bat.id,
        "profile stayed stale after the hardware recovered"
    );
}

#[tokio::test]
async fn m8_recheck_saving_active_profile_fails_without_changing_live_profile_if_disk_write_fails()
{
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("settings.json")).unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let name = "org.test.uccd.edge.activate_persist";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let before = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let quiet = default_quiet_profile();
    assert_ne!(before, quiet.id);
    let accepted = proxy.set_active_profile(&quiet.id).await.unwrap();
    let after = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        after, before,
        "D-Bus reported failure but activated {after}"
    );
}

#[tokio::test]
async fn m8_recheck_charging_threshold_failure_does_not_leave_hardware_changed() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("settings.json")).unwrap();
    let battery = root.path().join("sys/class/power_supply/BAT0");
    let start = battery.join("charge_control_start_threshold");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(battery.join("type"), "Battery");
            sysfs.set_file(&start, "30");
            sysfs.set_file(battery.join("charge_control_end_threshold"), "80");
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.charge_store";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let accepted = proxy.set_charge_start_threshold(45).await.unwrap();
    let actual = sysfs.get_file(&start);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        actual.as_deref(),
        Some("30"),
        "D-Bus returned failure but the battery threshold changed"
    );
}

#[tokio::test]
async fn m8_recheck_changing_power_device_cpu_policy_reapplies_the_current_profile() {
    let root = tempfile::tempdir().unwrap();
    let governor = root
        .path()
        .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&governor, "performance");
            sysfs.set_file(
                governor.with_file_name("scaling_available_governors"),
                "performance powersave schedutil",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.cpu_overlay";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy
        .set_power_device_settings_json(
            &json!({
                "enabled":true, "cpu": {"ac": {"governor":"schedutil"}}, "devices":[]
            })
            .to_string()
        )
        .await
        .unwrap());
    backend.reconcile_power_state(false);
    let actual = sysfs.get_file(&governor);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(actual.as_deref(), Some("schedutil"));
}

#[tokio::test]
async fn m8_recheck_releasing_a_well_known_name_keeps_power_requests_for_the_connected_client() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            set_battery_files(sysfs, root.path());
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.app_name";
    let runtime = start_runtime(backend, name, false).await;
    let app = session_conn().await;
    let ephemeral_name = "org.test.uccd.edge.client";
    app.request_name(ephemeral_name).await.unwrap();
    let proxy = uccd_proxy(&app, name).await;
    assert!(proxy
        .register_power_request("game", "test", "ac")
        .await
        .unwrap());
    assert_eq!(proxy.get_power_state().await.unwrap(), "power_ac");
    app.release_name(ephemeral_name).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let bus = zbus::fdo::DBusProxy::new(&app).await.unwrap();
    let connected = bus
        .name_has_owner(app.unique_name().unwrap().as_str().try_into().unwrap())
        .await
        .unwrap();
    let actual = proxy.get_power_state().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(connected);
    assert_eq!(
        actual, "power_ac",
        "still-connected sender lost its request after releasing a secondary name"
    );
}

#[tokio::test]
async fn m8_recheck_legacy_usb_c_sources_take_priority_when_mains_is_also_online() {
    let mut states = Vec::new();
    for supply_kind in ["USB", "USB_PD"] {
        let root = tempfile::tempdir().unwrap();
        let (backend, _, _, _) = build_custom_backend(
            root.path(),
            true,
            |sysfs| {
                let usb = root.path().join("sys/class/power_supply/ucsi-source-psy");
                sysfs.set_file(usb.join("type"), supply_kind);
                sysfs.set_file(usb.join("online"), "1");
            },
            |p| p,
        );
        let state = backend.get_power_state();
        states.push(state);
    }
    assert_eq!(
        states,
        ["power_type_c", "power_type_c"],
        "USB and USB_PD with Mains also online"
    );
}

#[tokio::test]
async fn m8_recheck_intel_igpu_discovery_does_not_pick_the_first_intel_discrete_card() {
    let root = tempfile::tempdir().unwrap();
    let drm = root.path().join("sys/class/drm");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        false,
        |sysfs| {
            for (card, device, clock) in [("card0", "56a0", "650"), ("card1", "46a6", "950")] {
                let dir = drm.join(card);
                sysfs.set_file(dir.join("device/vendor"), "0x8086");
                sysfs.set_file(dir.join("device/class"), "0x030000");
                sysfs.set_file(dir.join("device/device"), format!("0x{device}"));
                sysfs.set_file(
                    dir.join("device/uevent"),
                    format!("PCI_ID=8086:{}\n", device.to_uppercase()),
                );
                sysfs.set_file(dir.join("gt_act_freq_mhz"), clock);
            }
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.igpu";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let actual: serde_json::Value =
        serde_json::from_str(&proxy.get_igpu_info_values_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        actual["coreFrequency"], 950.0,
        "discrete card reported as iGPU: {actual}"
    );
}

#[tokio::test]
async fn m8_recheck_disappearing_backlight_entry_does_not_hide_working_panel() {
    let root = tempfile::tempdir().unwrap();
    let backlight = root.path().join("sys/class/backlight");
    let brightness = backlight.join("panel1/brightness");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(backlight.join("panel0/brightness"), "5");
            sysfs.set_read_error(backlight.join("panel0/max_brightness"));
            sysfs.set_file(backlight.join("panel1/max_brightness"), "100");
            sysfs.set_file(&brightness, "55");
        },
        |p| p,
    );
    let accepted = backend.set_display_brightness(40);
    assert!(
        accepted && sysfs.get_file(brightness).as_deref() == Some("40"),
        "unreadable first entry hid the valid second entry"
    );
}

#[tokio::test]
async fn m8_recheck_supported_nvidia_offset_reaches_the_hardware_controller() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("nvml_limit_write");
    let c_source = root.path().join("audit_nvml.c");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stand-fixtures/nvml.c");
    let source = std::fs::read_to_string(fixture).unwrap().replace(
        "unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) { return 0; }",
        &format!(
            "unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) {{ FILE *f = fopen({:?}, \"w\"); if (!f) return 1; fprintf(f, \"%u\", limit); fclose(f); return 0; }}",
            log.as_os_str()
        ),
    );
    assert!(source.contains("fprintf(f"));
    std::fs::write(&c_source, format!("#include <stdio.h>\n{source}")).unwrap();
    let result = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(root.path().join("fixture_nvml.so"))
        .arg(c_source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let gpu = root.path().join("sys/bus/pci/devices/0000:01:00.0");
    let nodes = [
        ("vendor", "0x10de"),
        ("class", "0x030000"),
        ("power/runtime_status", "active"),
    ];
    std::fs::create_dir_all(gpu.join("power")).unwrap();
    for (path, data) in nodes {
        std::fs::write(gpu.join(path), data).unwrap();
    }
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            for (path, data) in nodes {
                sysfs.set_file(gpu.join(path), data);
            }
        },
        |p| p,
    );
    let name = "org.test.uccd.edge.ctgp";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy.get_nvidia_power_ctrl_available().await.unwrap());
    let accepted = proxy.set_nvidia_power_offset(-5).await.unwrap();
    let write = std::fs::read_to_string(&log).ok();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        accepted && write.is_some(),
        "D-Bus accepted={accepted}, NVML write={write:?}"
    );
}

#[test]
fn m8_recheck_independent_concurrent_config_edits_survive_all_successful_saves() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("settings.json");
    let mut settings = create_default_settings();
    settings
        .extra
        .insert("audit_padding".into(), json!("x".repeat(1_048_576)));
    ConfigManager::with_custom_path(file.clone())
        .save(&settings)
        .unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let gate = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        let mut tasks = vec![];
        for i in 0..8 {
            let gate = gate.clone();
            let backend = backend.clone();
            tasks.push(scope.spawn(move || {
                let mut profile = default_quiet_profile();
                profile.id = format!("edge-saved-{i}");
                let body = serde_json::to_string(&profile).unwrap();
                gate.wait();
                backend.save_custom_profile(&body)
            }));
        }
        assert!(tasks.into_iter().all(|t| t.join().unwrap()));
    });
    let disk = ConfigManager::load_from_path(&file).unwrap();
    let on_disk = disk
        .profiles
        .keys()
        .filter(|k| k.starts_with("edge-saved-"))
        .count();
    let in_memory = (0..8)
        .filter(|i| {
            backend
                .get_profiles_json()
                .contains(&format!("edge-saved-{i}"))
        })
        .count();
    assert_eq!(
        (on_disk, in_memory),
        (8, 8),
        "all eight updates returned success"
    );
}

#[tokio::test]
async fn m8_recheck_explicit_cpu_core_selection_is_applied_by_the_live_daemon() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, sysfs, _) = build_test_backend(root.path());
    let cpu1 = root.path().join("sys/devices/system/cpu/cpu1/online");
    let name = "org.test.uccd.edge.cpu_selection";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    let mut profile = default_quiet_profile();
    profile.cpu.enabled_cores = Some(vec![0]);
    profile.cpu.online_cores = None;
    profile.odm_profile_name = None;
    profile.odm_power_limits.clear();
    profile.validate().unwrap();
    assert_eq!(sysfs.get_file(&cpu1).as_deref(), Some("1"));
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let cpu_online = sysfs.get_file(cpu1);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(accepted);
    assert_eq!(
        cpu_online.as_deref(),
        Some("0"),
        "explicit CPU set was ignored"
    );
}

#[tokio::test]
async fn m8_recheck_fan_profile_saved_with_outer_name_remains_usable_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_test_backend(root.path());
    let name = "org.test.uccd.edge.fan_save";
    let id = "fan-outer-name";
    let payload = json!({
        "id": id,
        "tableCPU": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}],
        "tableGPU": [{"temp": 45, "speed": 40}, {"temp": 85, "speed": 80}]
    })
    .to_string();
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy
        .save_custom_fan_profile(id, "Outer name", &payload)
        .await
        .unwrap());
    let apply_id = json!({"fanProfileId": id}).to_string();
    assert!(proxy.apply_fan_profiles(&apply_id).await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = start_runtime(backend, name, false).await;
    let proxy = uccd_proxy(&observer, name).await;
    let listed = proxy
        .get_custom_fan_profiles_json()
        .await
        .unwrap()
        .contains(id);
    let accepted = proxy.apply_fan_profiles(&apply_id).await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        listed && accepted,
        "saved curve is listed={listed}, apply by ID succeeds={accepted}"
    );
}

#[tokio::test]
async fn m8_recheck_webcam_status_delegates_to_platform_backend() {
    let root = tempfile::tempdir().unwrap();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut caps = p.capabilities();
            caps.device.webcam_switch = true;
            p.set_capabilities(caps);
            p.set_webcam(false).unwrap();
            p
        },
    );
    let name = "org.test.uccd.edge.webcam";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let proxy = uccd_proxy(&observer, name).await;
    assert!(proxy.webcam_sw_available().await.unwrap());
    assert!(proxy.set_webcam(false).await.unwrap());
    assert!(!proxy.get_webcam_sw_status().await.unwrap());
    assert!(proxy.set_webcam(true).await.unwrap());
    let status = proxy.get_webcam_sw_status().await.unwrap();
    let writes = platform.journal();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(status);
    assert!(writes.contains(&PlatformWrite::SetWebcam(false)));
    assert!(writes.contains(&PlatformWrite::SetWebcam(true)));
}
