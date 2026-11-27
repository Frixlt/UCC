// SPDX-License-Identifier: GPL-3.0-or-later

#[allow(unused_imports)]
use super::common::*;
use super::transactions::TxRegressionPlatform;
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

struct StallingBacklightSysfs {
    inner: Arc<MockSysfs>,
    gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl ucc_hardware::Sysfs for StallingBacklightSysfs {
    fn read(&self, path: &Path) -> std::io::Result<String> {
        if path.ends_with("actual_brightness")
            && self
                .inner
                .get_file(path.with_file_name("brightness"))
                .as_deref()
                == Some("40")
        {
            let gate = self.gate.lock().unwrap().take();
            if let Some((start, release)) = gate {
                start.send(()).unwrap();
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        self.inner.read(path)
    }
    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        self.inner.write(path, content)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        self.inner.read_dir(path)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn followup_01_brightness_verify_failure_does_not_start_rollback_after_deadline() {
    let root = tempfile::tempdir().unwrap();
    let panel = root.path().join("sys/class/backlight/panel0");
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(panel.join("brightness"), "55");
    sysfs.set_file(panel.join("actual_brightness"), "55");
    sysfs.set_file(panel.join("max_brightness"), "100");
    sysfs.set_file(
        root.path()
            .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        root.path()
            .join("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(root.path().join("sys/devices/system/cpu/cpu1/online"), "1");
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "1");

    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let stalling_sysfs: Arc<dyn ucc_hardware::Sysfs> = Arc::new(StallingBacklightSysfs {
        inner: sysfs.clone(),
        gate: std::sync::Mutex::new(Some((started, wait))),
    });
    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));
    let lighting = Arc::new(MockLightingTransport::new());
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: platform.clone(),
        sysfs: stalling_sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(root.path().join("settings.json")),
        sys_root: root.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            root.path().join("sys"),
            root.path().join("proc"),
            ucc_hardware::default_process_runner(),
            root.path().join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));

    let name = "org.test.uccd.reg.followup_01";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut initial = handoff_profile_with_tdps("initial", [55, 80, 95]);
    initial.display.use_brightness = false;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    let mut pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    pending.display.use_brightness = true;
    pending.display.brightness = 40;
    let rpc = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .apply_profile(&serde_json::to_string(&pending).unwrap())
            .await
    });
    handoff_wait_gate(entered).await;
    runtime.shutdown(Duration::from_millis(100)).await;
    platform.clear_journal();
    sysfs.clear_journal();
    release.send(()).unwrap();
    let reply = rpc.await.unwrap().unwrap();
    assert!(!reply);
    let tdp = platform.journal();
    let late_sysfs = sysfs.journal();
    assert!(
        !tdp.iter()
            .any(|w| matches!(w, ucc_hardware::PlatformWrite::SetTdp { .. }))
            && late_sysfs.is_empty(),
        "late rollback writes after shutdown deadline: tdp={tdp:?}, sysfs={late_sysfs:?}"
    );
}

#[tokio::test]
async fn followup_02_failed_odm_without_readback_does_not_leave_a_different_mode() {
    let root = tempfile::tempdir().unwrap();
    let mut odm_only = handoff_profile_with_tdps("odm-only", [55, 80, 95]);
    odm_only.odm_power_limits.clear();
    odm_only.odm_profile_name = Some("performance".into());
    let mut odm_plus_tdp = handoff_profile_with_tdps("odm-tdp", [45, 75, 90]);
    odm_plus_tdp.odm_profile_name = Some("performance".into());

    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.no_odm_readback = true;
            let w = Arc::new(w);
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.followup_02";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    ctl.odm_fail
        .store(true, std::sync::atomic::Ordering::SeqCst);

    assert!(!proxy
        .apply_profile(&serde_json::to_string(&odm_only).unwrap())
        .await
        .unwrap());
    assert_eq!(platform.read_perf_profile().unwrap(), 2);
    assert!(platform.journal().is_empty());

    assert!(!proxy
        .apply_profile(&serde_json::to_string(&odm_plus_tdp).unwrap())
        .await
        .unwrap());
    assert_eq!(platform.read_perf_profile().unwrap(), 2);
    assert!(platform.journal().is_empty());
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn followup_03_successful_edit_is_persisted_if_an_activation_supersedes_it() {
    let root = tempfile::tempdir().unwrap();
    let original = handoff_profile_with_tdps("edited", [55, 80, 95]);
    let mut edited = original.clone();
    edited.odm_power_limits = vec![41, 70, 90];
    let next = handoff_profile_with_tdps("winner", [62, 82, 95]);
    handoff_save_profiles(root.path(), &[&original, &next]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.reg.followup_03";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_active_profile(&original.id).await.unwrap());
    let editing = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .update_custom_profile(&serde_json::to_string(&edited).unwrap())
            .await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy.set_active_profile(&next.id).await.unwrap());
    release.send(()).unwrap();
    let ok = editing.await.unwrap().unwrap();
    let disk = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let actual = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(ok);
    assert_eq!(disk.profiles[&original.id].odm_power_limits[0], 41);
    assert_eq!((active.id.as_str(), actual), (next.id.as_str(), 62));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn followup_04_concurrent_edit_and_activation_agree_on_the_active_profile_revision() {
    let root = tempfile::tempdir().unwrap();
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    let edited = handoff_profile_with_tdps("pending", [55, 80, 95]);
    handoff_save_profiles(root.path(), &[&pending]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.reg.followup_04";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let setting = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .set_active_profile(&pending.id)
            .await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&edited).unwrap())
        .await
        .unwrap());
    release.send(()).unwrap();
    let ok = setting.await.unwrap().unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let disk = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let actual = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(disk.profiles[&edited.id].odm_power_limits[0], 55);
    assert!(ok);
    assert_eq!(
        (active.id.as_str(), active.odm_power_limits[0], actual),
        (edited.id.as_str(), 55, 55)
    );
}

#[tokio::test]
async fn followup_05_drm_resolution_only_data_does_not_claim_refresh_rate_or_active_mode() {
    let root = tempfile::tempdir().unwrap();
    let connector = root.path().join("sys/class/drm/card0-eDP-1");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(connector.join("status"), "connected\n");
            s.set_file(connector.join("enabled"), "enabled\n");
            s.set_file(connector.join("modes"), "2560x1600\n2560x1600\n1920x1200\n");
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.followup_05";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let modes: serde_json::Value =
        serde_json::from_str(&proxy.get_display_modes_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(modes["displayName"], "eDP-1");
    assert_eq!(modes["activeMode"]["xResolution"], 0);
    assert_eq!(modes["activeMode"]["refreshRates"], serde_json::json!([]));
    assert_eq!(modes["displayModes"].as_array().unwrap().len(), 2);
    assert!(modes["displayModes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["refreshRates"] == serde_json::json!([])));
}

#[tokio::test]
async fn followup_06_configured_ycbcr_workaround_uses_driver_debugfs_node() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = serde_json::to_value(create_default_settings()).unwrap();
    cfg["ycbcr420Workaround"] = serde_json::json!([{"HDMI-A-1": false}, {"DP-1": true}]);
    let cfg: ucc_config::UccSettings = serde_json::from_value(cfg).unwrap();
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&cfg)
        .unwrap();
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(
                root.path()
                    .join("sys/kernel/debug/dri/0/eDP-1/force_yuv420_output"),
                "0\n",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.followup_06";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    // Neither configured HDMI-A-1 on card 0 nor DP-1 on card 1 exists yet, even though unconfigured eDP-1 does.
    assert!(!proxy
        .get_force_yuv420_output_switch_available()
        .await
        .unwrap());
    sysfs.set_file(
        root.path()
            .join("sys/kernel/debug/dri/1/DP-1/force_yuv420_output"),
        "0\n",
    );
    assert!(proxy
        .get_force_yuv420_output_switch_available()
        .await
        .unwrap());

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn followup_07_second_integrated_pci_gpu_does_not_fill_discrete_slot() {
    let root = tempfile::tempdir().unwrap();
    let pci = root.path().join("sys/bus/pci/devices");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            for (bdf, product) in [
                ("0000:00:02.0", "Integrated One"),
                ("0001:00:02.0", "Integrated Two"),
            ] {
                let dev = pci.join(bdf);
                s.set_file(dev.join("class"), "0x030000");
                s.set_file(dev.join("vendor"), "0x8086");
                s.set_file(dev.join("product_name"), product);
            }
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.followup_07";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let info: serde_json::Value =
        serde_json::from_str(&proxy.get_system_info_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(info["iGpuModel"], "Integrated One");
    assert_eq!(info["dGpuModel"], "");
}

#[tokio::test]
async fn m8_outcomes_odm_read_error_after_preflight_restores_baseline() {
    use std::sync::atomic::Ordering::SeqCst;
    let root = tempfile::tempdir().unwrap();
    let mut target = handoff_profile_with_tdps("new-odm-mode", [55, 80, 95]);
    target.odm_power_limits.clear();
    target.odm_profile_name = Some("performance".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = Arc::new(TxRegressionPlatform::new(p));
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.outcomes_odm_disappeared";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    ctl.odm_read_error_at.store(2, SeqCst);
    ctl.odm_fail.store(true, SeqCst);
    let ok = proxy
        .apply_profile(&serde_json::to_string(&target).unwrap())
        .await
        .unwrap();
    let final_mode = platform.read_perf_profile().unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(final_mode, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_failed_reapply_of_superseded_edit_does_not_persist() {
    let root = tempfile::tempdir().unwrap();
    let original = handoff_profile_with_tdps("edited", [55, 80, 95]);
    let old_edit = handoff_profile_with_tdps("edited", [41, 70, 90]);
    let new_edit = handoff_profile_with_tdps("edited", [62, 82, 105]);
    handoff_save_profiles(root.path(), &[&original]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.reject_second_41 = true;
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.reg.outcomes_failed_reapply";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_active_profile(&original.id).await.unwrap());
    let pending = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .update_custom_profile(&serde_json::to_string(&old_edit).unwrap())
            .await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&new_edit).unwrap())
        .await
        .unwrap());
    release.send(()).unwrap();
    let ok = pending.await.unwrap().unwrap();
    let active_before: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let disk_before = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let hardware_before = platform.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;

    let (restarted, restarted_hw, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let runtime = start_runtime(restarted, name, false).await;
    let after = restarted_hw.read_tdp(0).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;

    assert!(!ok);
    assert_eq!(active_before.odm_power_limits[0], 62);
    assert_eq!(hardware_before, 62);
    assert_eq!(
        (
            disk_before.profiles[&original.id].odm_power_limits[0],
            after
        ),
        (62, 62)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_stale_revision_rollback_does_not_overwrite_winner() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("initial", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    let replacement = handoff_profile_with_tdps("pending", [91, 95, 105]);
    let winner = handoff_profile_with_tdps("winner", [62, 82, 105]);
    handoff_save_profiles(root.path(), &[&initial, &pending, &winner]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (rollback_started, rollback_entered) = std::sync::mpsc::channel();
    let (release_rollback, rollback_wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = Arc::new(TxRegressionPlatform::new(p));
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.outcomes_stale_undo";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy.set_active_profile(&initial.id).await.unwrap());
    let setting = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .set_active_profile(&pending.id)
            .await
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&replacement).unwrap())
        .await
        .unwrap());
    *ctl.stale_rollback_gate.lock().unwrap() = Some((rollback_started, rollback_wait));
    release.send(()).unwrap();
    handoff_wait_gate(rollback_entered).await;
    assert!(proxy.set_active_profile(&winner.id).await.unwrap());
    assert_eq!(platform.read_tdp(0).unwrap(), 62);
    release_rollback.send(()).unwrap();
    let losing_ok = setting.await.unwrap().unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let disk = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    let actual = (0..3)
        .map(|i| platform.read_tdp(i).unwrap())
        .collect::<Vec<_>>();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!losing_ok);
    assert_eq!(active.id, winner.id);
    assert_eq!(disk.state_map["power_ac"], winner.id);
    assert_eq!(actual, [62, 82, 105]);
}

struct CpuRestoreGateSysfs {
    sysfs: Arc<MockSysfs>,
    read_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    restored: std::sync::mpsc::Sender<()>,
}

impl ucc_hardware::Sysfs for CpuRestoreGateSysfs {
    fn read(&self, path: &Path) -> std::io::Result<String> {
        if path.ends_with("no_turbo") {
            let gate = self.read_gate.lock().unwrap().take();
            if let Some((start, release)) = gate {
                start.send(()).unwrap();
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        self.sysfs.read(path)
    }
    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        self.sysfs.write(path, content)?;
        if path.ends_with("no_turbo") && content.trim() == "0" {
            let _ = self.restored.send(());
        }
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.sysfs.exists(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        self.sysfs.read_dir(path)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_no_new_cpu_restore_write_after_shutdown_deadline() {
    let root = tempfile::tempdir().unwrap();
    let nt = root
        .path()
        .join("sys/devices/system/cpu/intel_pstate/no_turbo");
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (restored, completion) = std::sync::mpsc::channel();
    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        root.path()
            .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "1");
    sysfs.set_file(&nt, "0");
    let gated_sysfs: Arc<dyn ucc_hardware::Sysfs> = {
        let w = Arc::new(CpuRestoreGateSysfs {
            sysfs: sysfs.clone(),
            read_gate: std::sync::Mutex::new(None),
            restored,
        });
        tx.send(w.clone()).unwrap();
        w
    };
    let ctl = rx.recv().unwrap();
    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    let lighting = Arc::new(MockLightingTransport::new());
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform,
        sysfs: gated_sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(root.path().join("settings.json")),
        sys_root: root.path().to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: root.path().to_path_buf(),
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            root.path().join("sys"),
            root.path().join("proc"),
            ucc_hardware::default_process_runner(),
            root.path().join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));
    let name = "org.test.uccd.reg.outcomes_shutdown_cpu";
    let runtime = start_runtime(backend, name, false).await;
    sysfs.set_file(&nt, "1");
    *ctl.read_gate.lock().unwrap() = Some((started, wait));
    let shutdown = tokio::spawn(runtime.shutdown(Duration::from_millis(100)));
    handoff_wait_gate(entered).await;
    shutdown.await.unwrap();
    while completion.try_recv().is_ok() {}
    sysfs.clear_journal();
    release.send(()).unwrap();
    let wrote = tokio::task::spawn_blocking(move || {
        completion.recv_timeout(Duration::from_millis(500)).is_ok()
    })
    .await
    .unwrap();
    assert!(
        !wrote,
        "shutdown cleanup started new writes after budget expired: {:?}",
        sysfs.journal()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_outcomes_power_worker_reapplies_edited_battery_profile() {
    let root = tempfile::tempdir().unwrap();
    let ac_profile = handoff_profile_with_tdps("ac", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("battery", [41, 70, 90]);
    let edited = handoff_profile_with_tdps("battery", [62, 82, 105]);
    let mut cfg = create_default_settings();
    for profile in [&ac_profile, &pending] {
        cfg.profiles.insert(profile.id.clone(), profile.clone());
    }
    cfg.set_active_profile_for_mode("power_ac", &ac_profile.id);
    cfg.set_active_profile_for_mode("power_bat", &pending.id);
    let config = root.path().join("settings.json");
    ConfigManager::with_custom_path(&config).save(&cfg).unwrap();
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.reg.outcomes_worker_revision";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert_eq!(platform.read_tdp(0).unwrap(), 55);
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(
        root.path().join("sys/class/power_supply/BAT0/type"),
        "Battery",
    );
    let worker = tokio::task::spawn_blocking({
        let backend = backend.clone();
        move || backend.reconcile_power_state(false)
    });
    handoff_wait_gate(entered).await;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&edited).unwrap())
        .await
        .unwrap());
    release.send(()).unwrap();
    worker.await.unwrap();
    backend.reconcile_power_state(false);
    let actual = platform.read_tdp(0).unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let saved = ConfigManager::load_from_path(&config).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(saved.profiles[&edited.id].odm_power_limits[0], 62);
    assert_eq!((active.odm_power_limits[0], actual), (62, 62));
}

#[tokio::test]
async fn failed_cpu_restore_is_carried_to_the_next_daemon() {
    let root = tempfile::tempdir().unwrap();
    let no_turbo = root
        .path()
        .join("sys/devices/system/cpu/intel_pstate/no_turbo");
    let mut config = create_default_settings();
    config.cpu_settings_enabled = Some(false);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&config)
        .unwrap();
    let (first, _, sysfs, _) =
        build_custom_backend(root.path(), true, |s| s.set_file(&no_turbo, "0"), |p| p);
    let name = "org.test.uccd.reg.failed_cpu_cleanup_handoff";
    let runtime = start_runtime(first, name, false).await;
    sysfs.set_file(&no_turbo, "1");
    sysfs.set_write_error(&no_turbo);
    runtime.shutdown(Duration::from_secs(2)).await;
    assert_eq!(sysfs.get_file(&no_turbo).as_deref(), Some("1"));

    sysfs.fail_writes.lock().unwrap().remove(&no_turbo);
    let (second, _, _, _) =
        build_custom_backend_with_sysfs(root.path(), true, Some(sysfs.clone()), |_| {}, |p| p);
    let runtime = start_runtime(second, name, false).await;
    let restored = sysfs.get_file(&no_turbo);
    runtime.shutdown(Duration::from_secs(2)).await;
    assert_eq!(restored.as_deref(), Some("0"));
}
