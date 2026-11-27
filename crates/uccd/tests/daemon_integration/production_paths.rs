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

struct ProdAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    startup_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    slow_auto: bool,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl ProdAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            startup_gate: std::sync::Mutex::new(None),
            slow_auto: false,
            tdp_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for ProdAuditPlatform {
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
        self.inner.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(idx)
    }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        if idx == 0 && watts == 41 {
            let gate = self.tdp_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
                return Err(PlatformError::Unavailable);
            }
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

struct PrivatePolicyBus(std::process::Child);

impl Drop for PrivatePolicyBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn m8_prod_daemon_starts_under_the_packaged_system_bus_policy() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let root = tempfile::tempdir().unwrap();
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../uccd/com.uniwill.uccd.conf");
    let service_policy = root.path().join("service.conf");
    std::fs::write(
        &service_policy,
        std::fs::read_to_string(source)
            .unwrap()
            .replace("user=\"root\"", &format!("user=\"{}\"", uid.trim())),
    )
    .unwrap();
    let config = root.path().join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"
        <busconfig>
          <listen>unix:path={}/bus</listen>
          <auth>EXTERNAL</auth>
          <policy context="default">
            <allow user="*"/>
            <deny own="*"/>
            <allow send_destination="*"/>
            <allow receive_sender="*"/>
          </policy>
          <include>{}</include>
        </busconfig>
    "#,
            root.path().display(),
            service_policy.display()
        ),
    )
    .unwrap();
    let mut daemon = PrivatePolicyBus(
        Command::new("dbus-daemon")
            .args(["--nofork", "--print-address=1", "--config-file"])
            .arg(config)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    std::io::BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    assert!(!address.is_empty(), "private policy bus failed to start");

    let connection = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
    let name = ucc_dbus::DBUS_SERVICE;
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    let reply = connection
        .request_name_with_flags(name, flags)
        .await
        .unwrap();
    assert_eq!(reply, zbus::fdo::RequestNameReply::PrimaryOwner);
    assert!(connection.release_name(name).await.unwrap());

    let (backend, _, _, _) = build_test_backend(root.path());
    let runtime = DaemonRuntime::start_on_connection(
        connection,
        name,
        backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .expect("packaged system bus policy must allow uccd to start");

    let observer = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
    let proxy = uccd_proxy(&observer, name).await;
    assert_eq!(proxy.get_device_name().await.unwrap(), "Test Laptop");
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_prod_startup_exclusive_name_blocks_rival_and_loser_never_writes_fans() {
    let root1 = tempfile::tempdir().unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (winner_backend, _, _, _) = build_custom_backend(
        root1.path(),
        true,
        |_| {},
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.startup_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.production.startup_race_exclusive";
    let first = tokio::spawn(DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        winner_backend,
        Arc::new(AllowAllAuthorizer),
        false,
    ));
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();

    // While winner is inside activate_hardware(), it ALREADY owns `name` exclusively:
    // any second candidate must fail immediately with ZERO fan writes.
    let root2 = tempfile::tempdir().unwrap();
    let (loser_backend, loser_platform, _, _) = build_test_backend(root2.path());
    let rival_err = DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        loser_backend,
        Arc::new(AllowAllAuthorizer),
        false,
    )
    .await
    .is_err();
    let loser_writes: Vec<_> = loser_platform
        .journal()
        .into_iter()
        .filter(|w| {
            matches!(
                w,
                PlatformWrite::SetFanSpeed { .. } | PlatformWrite::SetFansAuto
            )
        })
        .collect();
    let _ = release.send(());
    let winner = first.await.unwrap().unwrap();
    winner.shutdown(Duration::from_secs(1)).await;

    assert!(rival_err && loser_writes.is_empty());
}

#[tokio::test]
async fn m8_prod_restarted_daemon_is_not_overwritten_by_timed_out_predecessor_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let name = "org.test.uccd.production.restart";
    let (old, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = ProdAuditPlatform::new(p);
            wrapper.slow_auto = true;
            Arc::new(wrapper)
        },
    );
    let old = start_runtime(old, name, false).await;
    old.shutdown(Duration::from_millis(25)).await;
    let (replacement, _, _, _) =
        build_custom_backend(root.path(), true, |_| {}, |_| platform.clone());
    let replacement = start_runtime(replacement, name, false).await;
    platform.clear_journal();
    tokio::time::sleep(Duration::from_millis(390)).await;
    let writes = platform.journal();
    replacement.shutdown(Duration::from_secs(1)).await;
    assert!(
        !writes.contains(&PlatformWrite::SetFansAuto),
        "predecessor reset fan control after successor had acquired and primed {name}: {writes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m8_prod_failed_profile_switch_does_not_rollback_a_newer_successful_switch() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = create_default_settings();
    let mut failed = default_quiet_profile();
    failed.id = "gate-profile".into();
    failed.odm_profile_name = None;
    failed.odm_power_limits = vec![41, 70, 90];
    let mut good = failed.clone();
    good.id = "newer-profile".into();
    good.odm_power_limits = vec![55, 80, 95];
    settings.profiles.insert(failed.id.clone(), failed.clone());
    settings.profiles.insert(good.id.clone(), good.clone());
    let file = root.path().join("settings.json");
    ConfigManager::with_custom_path(file.clone())
        .save(&settings)
        .unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.production.concurrent_profile";
    let runtime = start_runtime(backend, name, false).await;
    let old = runtime.backend().clone();
    let first_call = std::thread::spawn(move || old.set_active_profile(&failed.id));
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let newer_result =
        tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&good.id)).await;
    let _ = release.send(());
    let failed_accepted = first_call.join().unwrap();
    let newer_accepted = newer_result.unwrap().unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let saved = ConfigManager::load_from_path(&file).unwrap().state_map["power_ac"].clone();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!failed_accepted && newer_accepted);
    assert_eq!(
        (active, saved),
        (good.id.clone(), good.id),
        "failed older RPC clobbered successful newer assignment"
    );
}

#[tokio::test]
async fn m8_prod_cpu_node_without_readback_is_not_modified_if_profile_later_fails() {
    let root = tempfile::tempdir().unwrap();
    let cpus = root.path().join("sys/devices/system/cpu");
    let no_turbo = cpus.join("intel_pstate/no_turbo");
    let epp = cpus.join("cpu0/cpufreq/energy_performance_preference");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&no_turbo, "0");
            sysfs.set_file(&epp, "balance_power");
            sysfs.set_file(
                epp.with_file_name("energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.production.cpu_readback";
    let runtime = start_runtime(backend, name, false).await;
    let before = sysfs.get_file(&no_turbo).unwrap();
    assert_eq!(before, "0");
    sysfs.set_read_error(&no_turbo);
    sysfs.set_write_error(&epp);
    let mut target = default_quiet_profile();
    target.odm_profile_name = None;
    target.odm_power_limits.clear();
    target.cpu.no_turbo = true;
    target.cpu.energy_performance_preference = "performance".into();
    let conn = session_conn().await;
    let accepted = uccd_proxy(&conn, name)
        .await
        .apply_profile(&serde_json::to_string(&target).unwrap())
        .await
        .unwrap();
    let after = sysfs.get_file(&no_turbo).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(
        after, before,
        "no_turbo was written without a readable rollback value"
    );
}

#[tokio::test]
async fn m8_prod_odm_switch_has_rollback_when_current_profile_does_not_name_odm_mode() {
    let root = tempfile::tempdir().unwrap();
    let cpufreq = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    let epp = cpufreq.join("energy_performance_preference");
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&epp, "balance_power");
            sysfs.set_file(
                epp.with_file_name("energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.production.odm_rollback";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut profile = default_quiet_profile();
    profile.odm_profile_name = None;
    profile.odm_power_limits.clear();
    profile.cpu.energy_performance_preference = "balance_power".into();
    assert!(proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap());
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();

    // Case 1: CPU failure before ODM write keeps ODM untouched.
    sysfs.set_write_error(&epp);
    profile.odm_profile_name = Some("overboost".into());
    profile.cpu.energy_performance_preference = "performance".into();
    assert!(!proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap());
    let writes_case1: Vec<_> = platform
        .journal()
        .into_iter()
        .filter(|w| matches!(w, PlatformWrite::SetPerfProfile(_)))
        .collect();
    assert!(writes_case1.is_empty());

    // Case 2: Subsequent TDP failure after ODM write rolls ODM back to hardware readback `2`.
    let root2 = tempfile::tempdir().unwrap();
    let cpufreq2 = root2.path().join("sys/devices/system/cpu/cpu0/cpufreq");
    let epp2 = cpufreq2.join("energy_performance_preference");
    let (backend2, platform2, _, _) = build_custom_backend(
        root2.path(),
        true,
        |sysfs| {
            sysfs.set_file(&epp2, "balance_power");
            sysfs.set_file(
                epp2.with_file_name("energy_performance_available_preferences"),
                "balance_power performance",
            );
        },
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name2 = "org.test.uccd.production.odm_rollback_tdp";
    let runtime2 = start_runtime(backend2, name2, false).await;
    let proxy2 = uccd_proxy(&conn, name2).await;
    let mut profile2 = default_quiet_profile();
    profile2.odm_profile_name = None;
    profile2.odm_power_limits.clear();
    profile2.cpu.energy_performance_preference = "balance_power".into();
    assert!(proxy2
        .apply_profile(&serde_json::to_string(&profile2).unwrap())
        .await
        .unwrap());
    platform2.set_perf_profile(2).unwrap();
    platform2.clear_journal();
    profile2.odm_profile_name = Some("overboost".into());
    profile2.cpu.energy_performance_preference = "performance".into();
    profile2.odm_power_limits = vec![41, 70, 90];
    let _ = release.send(());
    let accepted = proxy2
        .apply_profile(&serde_json::to_string(&profile2).unwrap())
        .await
        .unwrap();
    let _ = entered.try_recv();
    let writes_case2: Vec<_> = platform2
        .journal()
        .into_iter()
        .filter(|w| matches!(w, PlatformWrite::SetPerfProfile(_)))
        .collect();
    runtime.shutdown(Duration::from_secs(1)).await;
    runtime2.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!(writes_case2.last(), Some(&PlatformWrite::SetPerfProfile(2)));
}

#[tokio::test]
async fn m8_prod_nvidia_ctgp_sysfs_fallback_is_available_without_nvml() {
    let root = tempfile::tempdir().unwrap();
    let expected = root
        .path()
        .join("sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(&expected, "0");
        },
        |p| p,
    );
    let name = "org.test.uccd.production.ctgp_sysfs";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let advertised = proxy.get_nvidia_power_ctrl_available().await.unwrap();
    let accepted = proxy.set_nvidia_power_offset(-5).await.unwrap();
    let after = sysfs.get_file(&expected).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        advertised && accepted && after == "-5",
        "NVML absent, sysfs available: advertised={advertised}, accepted={accepted}, offset={after}"
    );
}

#[tokio::test]
async fn m8_prod_cpu_overlay_retries_after_transient_sysfs_write_failure() {
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
                "performance powersave",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.production.overlay_retry";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let available = governor.with_file_name("scaling_available_governors");
    sysfs.set_file(&available, "performance");
    let config = json!({"enabled":true, "cpu":{"ac":{"governor":"powersave"}}, "devices":[]});
    assert!(proxy
        .set_power_device_settings_json(&config.to_string())
        .await
        .unwrap());
    assert_eq!(sysfs.get_file(&governor).as_deref(), Some("performance"));
    sysfs.set_file(&available, "performance powersave");
    backend.reconcile_power_state(false);
    let actual = sysfs.get_file(&governor);
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        actual.as_deref(),
        Some("powersave"),
        "a newly available overlay is never retried"
    );
}

#[tokio::test]
async fn m8_prod_charging_priority_uses_the_installed_driver_attribute_names() {
    let root = tempfile::tempdir().unwrap();
    let attrs = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/charging_priority");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sysfs| {
            sysfs.set_file(
                attrs.join("charging_prios_available"),
                "charge_battery performance",
            );
            sysfs.set_file(attrs.join("charging_prio"), "charge_battery");
        },
        |p| p,
    );
    let name = "org.test.uccd.production.charge_prio";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let supported = proxy.get_charging_priorities_available().await.unwrap();
    let accepted = proxy.set_charging_priority("performance").await.unwrap();
    let actual = sysfs.get_file(attrs.join("charging_prio"));
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        supported.contains("performance") && accepted && actual.as_deref() == Some("performance"),
        "advertised={supported}, accepted={accepted}, driver={actual:?}"
    );
}

#[tokio::test]
async fn m8_prod_repeating_the_same_nvidia_offset_does_not_compound_it() {
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
    let source = source.replace(
        "unsigned nvmlDeviceGetEnforcedPowerLimit(void *dev, unsigned *out) { *out = 50000; return 0; }",
        &format!(
            "unsigned nvmlDeviceGetEnforcedPowerLimit(void *dev, unsigned *out) {{ FILE *f=fopen({:?}, \"r\"); *out=50000; if (f) {{ (void)fscanf(f, \"%u\", out); fclose(f); }} return 0; }}",
            log.as_os_str()
        ),
    );
    assert!(source.contains("fprintf(f") && source.contains("fscanf(f"));
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
        ("power/control", "on"),
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

    let name = "org.test.uccd.production.ctgp_repeat";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_nvidia_power_offset(-5).await.unwrap());
    let first = std::fs::read_to_string(&log).unwrap();
    backend.poll_telemetry_once();
    assert!(proxy.set_nvidia_power_offset(-5).await.unwrap());
    let second = std::fs::read_to_string(&log).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(first, "45000");
    assert_eq!(
        second, first,
        "same requested offset after polling changed the applied absolute limit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m8_prod_unrelated_dbus_reads_remain_responsive_while_profile_io_is_stalled() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = create_default_settings();
    let mut profile = default_quiet_profile();
    profile.id = "stalled-profile".into();
    profile.odm_profile_name = None;
    profile.odm_power_limits = vec![41, 70, 90];
    settings
        .profiles
        .insert(profile.id.clone(), profile.clone());
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&settings)
        .unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = ProdAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.production.stalled_rpc";
    let runtime = start_runtime(backend, name, false).await;
    let observer = session_conn().await;
    let read_proxy = uccd_proxy(&observer, name).await;
    let first = tokio::spawn(async move {
        let conn = session_conn().await;
        uccd_proxy(&conn, name)
            .await
            .set_active_profile(&profile.id)
            .await
            .unwrap()
    });
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    let read = tokio::time::timeout(Duration::from_millis(350), read_proxy.get_device_name()).await;
    let _ = release.send(());
    assert!(!first.await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        matches!(read, Ok(Ok(ref n)) if n == "Test Laptop"),
        "independent GetDeviceName was blocked: {read:?}"
    );
}
