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

pub(crate) struct TxRegressionPlatform {
    pub(crate) inner: Arc<MockPlatformBackend>,
    pub(crate) mutate_before_error: bool,
    pub(crate) gated_channel: usize,
    pub(crate) reject_second_tdp: std::sync::atomic::AtomicBool,
    pub(crate) omit_tdp1_read: std::sync::atomic::AtomicBool,
    pub(crate) odm_fail: std::sync::atomic::AtomicBool,
    pub(crate) no_odm_readback: bool,
    pub(crate) odm_read_error_at: std::sync::atomic::AtomicUsize,
    pub(crate) reject_second_41: bool,
    pub(crate) calls_41: std::sync::atomic::AtomicUsize,
    pub(crate) tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    pub(crate) read_tdp0_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    pub(crate) restore_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
    pub(crate) stale_rollback_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl TxRegressionPlatform {
    pub(crate) fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            mutate_before_error: false,
            gated_channel: 0,
            reject_second_tdp: std::sync::atomic::AtomicBool::new(false),
            omit_tdp1_read: std::sync::atomic::AtomicBool::new(false),
            odm_fail: std::sync::atomic::AtomicBool::new(false),
            no_odm_readback: false,
            odm_read_error_at: std::sync::atomic::AtomicUsize::new(0),
            reject_second_41: false,
            calls_41: std::sync::atomic::AtomicUsize::new(0),
            tdp_gate: std::sync::Mutex::new(None),
            read_tdp0_gate: std::sync::Mutex::new(None),
            restore_gate: std::sync::Mutex::new(None),
            stale_rollback_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for TxRegressionPlatform {
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
        if idx == 0 {
            let gate = self.read_tdp0_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if idx == 1
            && self
                .omit_tdp1_read
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(PlatformError::Unavailable);
        }
        self.inner.read_tdp(idx)
    }
    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.inner.read_tdp_limits(idx)
    }
    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        if idx == 2 && watts == 95 {
            let gate = self.stale_rollback_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if self.reject_second_41
            && idx == 0
            && watts == 41
            && self
                .calls_41
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 1
        {
            return Err(PlatformError::Unavailable);
        }
        if idx == 1 && watts == 80 {
            let gate = self.restore_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if idx == self.gated_channel && watts == [41, 70, 90][idx] {
            let gate = self.tdp_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                let _ = started.send(());
                let _ = release.recv_timeout(Duration::from_secs(4));
            }
        }
        if idx == 1
            && self
                .reject_second_tdp
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            if self.mutate_before_error {
                self.inner.set_tdp(idx, watts)?;
            }
            return Err(PlatformError::Unavailable);
        }
        self.inner.set_tdp(idx, watts)
    }
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        use std::sync::atomic::Ordering::SeqCst;
        #[allow(deprecated)]
        let error_now = self
            .odm_read_error_at
            .fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1))
            == Ok(1);
        if error_now {
            return Err(PlatformError::Unsupported(
                "profile sysfs disappeared after preflight",
            ));
        }
        if self.no_odm_readback {
            return Err(PlatformError::Unsupported("ODM readback unsupported"));
        }
        self.inner.read_perf_profile()
    }

    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_perf_profile(value)?;
        if self
            .odm_fail
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            for (i, w) in [45, 70, 105].into_iter().enumerate() {
                self.inner.set_tdp(i, w)?;
            }
            return Err(PlatformError::Unavailable);
        }
        Ok(())
    }
    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
        self.inner.set_mode_enable(value)
    }
    fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
        self.inner.set_mode(value)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_winning_profile_does_not_inherit_losing_uncommitted_tdp() {
    let root = tempfile::tempdir().unwrap();
    let old = handoff_profile_with_tdps("older-writes", [41, 70, 90]);
    let mut new = default_quiet_profile();
    new.id = "no-tdp-override".into();
    new.odm_profile_name = None;
    new.odm_power_limits.clear();
    handoff_save_profiles(root.path(), &[&old, &new]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.gated_channel = 1;
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.tx_reg.uncommitted";
    let runtime = start_runtime(backend, name, false).await;
    platform.set_tdp_value(0, 65);
    let older = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .set_active_profile(&old.id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let newer_ok = tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&new.id))
        .await
        .unwrap()
        .unwrap();
    let _ = release.send(());
    let _ = older.await.unwrap();
    let actual = platform.read_tdp(0).unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        newer_ok && active == new.id && actual == 65,
        "actual PL1={actual}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_deleted_profile_cannot_be_committed_by_concurrent_set_active() {
    let root = tempfile::tempdir().unwrap();
    let pending = handoff_profile_with_tdps("pending-removal", [41, 70, 90]);
    handoff_save_profiles(root.path(), &[&pending]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let w = TxRegressionPlatform::new(p);
            *w.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(w)
        },
    );
    let name = "org.test.uccd.tx_reg.delete";
    let runtime = start_runtime(backend, name, false).await;
    let id = pending.id.clone();
    let set = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .set_active_profile(&id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let deleted = proxy.delete_custom_profile(&pending.id).await.unwrap();
    let _ = release.send(());
    let accepted = set.await.unwrap();
    let s = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(deleted && !accepted && !s.profiles.contains_key(&pending.id));
    assert_ne!(s.state_map["power_ac"], pending.id);
}

#[tokio::test]
async fn m8_tx_failing_tdp_write_after_mutation_is_rolled_back() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("baseline", [55, 80, 95]);
    let target = handoff_profile_with_tdps("failing", [43, 75, 95]);
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut w = TxRegressionPlatform::new(p);
            w.mutate_before_error = true;
            let w = Arc::new(w);
            tx.send(w.clone()).unwrap();
            w
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.tx_reg.mutate_err";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    ctl.reject_second_tdp
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let ok = proxy
        .apply_profile(&serde_json::to_string(&target).unwrap())
        .await
        .unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let actual = platform.read_tdp(1).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(active.odm_power_limits[1], actual as u32);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_drained_shutdown_rolls_back_cancelled_tdp_write() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("baseline", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
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
    let name = "org.test.uccd.tx_reg.drained_shutdown";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    let pending_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .apply_profile(&serde_json::to_string(&pending).unwrap())
            .await
    });
    handoff_wait_gate(entered).await;
    let shutdown = tokio::spawn(runtime.shutdown(Duration::from_secs(2)));
    let bus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while bus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let _ = release.send(());
    let _ = pending_call.await;
    shutdown.await.unwrap();
    let actual: Vec<_> = (0..3).map(|i| platform.read_tdp(i).unwrap()).collect();
    assert_eq!(actual, vec![55, 80, 95]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_tx_power_change_during_set_active_clears_applied_state_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let ac = handoff_profile_with_tdps("custom-ac", [41, 70, 90]);
    let bat = handoff_profile_with_tdps("custom-bat", [45, 125, 130]);
    let mut cfg = create_default_settings();
    cfg.profiles.insert(ac.id.clone(), ac.clone());
    cfg.profiles.insert(bat.id.clone(), bat.clone());
    cfg.set_active_profile_for_mode("power_bat", &bat.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&cfg)
        .unwrap();
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
    let name = "org.test.uccd.tx_reg.power_retry";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let task = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .set_active_profile(&ac.id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    sysfs.set_file(root.path().join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(
        root.path().join("sys/class/power_supply/BAT0/type"),
        "Battery",
    );
    backend.reconcile_power_state(false);
    let _ = release.send(());
    let _ = task.await.unwrap();
    platform.set_tdp_limits(1, (20, 130));
    backend.reconcile_power_state(false);
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let state = proxy.get_power_state().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!(
        (state.as_str(), active.as_str()),
        ("power_bat", bat.id.as_str())
    );
}

#[tokio::test]
async fn m8_tx_display_brightness_failure_aborts_profile_apply() {
    let root = tempfile::tempdir().unwrap();
    let node = root.path().join("sys/class/backlight/panel0/brightness");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(&node, "55");
            s.set_file(node.with_file_name("max_brightness"), "100");
        },
        |p| p,
    );
    let name = "org.test.uccd.tx_reg.brightness";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    sysfs.set_file(&node, "55");
    sysfs.set_write_error(&node);
    let mut p = default_quiet_profile();
    p.id = "req-brightness".into();
    p.display.use_brightness = true;
    p.display.brightness = 40;
    p.odm_profile_name = None;
    p.odm_power_limits.clear();
    let ok = proxy
        .apply_profile(&serde_json::to_string(&p).unwrap())
        .await
        .unwrap();
    let actual = proxy.get_display_brightness().await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        !ok,
        "apply_profile unexpectedly succeeded (actual={actual})"
    );
    assert_eq!(actual, 55, "brightness mutated despite write error");
}

#[tokio::test]
async fn m8_tx_preview_mode_blocks_brightness_and_hardware_writes() {
    if std::env::var_os("UCC_TEST_PREVIEW_REG_CHILD").is_none() {
        let out = tokio::task::spawn_blocking(|| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "m8_tx_preview_mode_blocks_brightness_and_hardware_writes",
                    "--nocapture",
                ])
                .env("UCC_TEST_PREVIEW_REG_CHILD", "1")
                .env("UCC_READ_ONLY_PREVIEW", "1")
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(out.status.success(), "{out:?}");
        return;
    }
    assert!(ucc_config::is_read_only_preview());
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .join("sys/class/backlight/intel_backlight/brightness");
    let (backend, platform, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |sys| {
            sys.set_file(&path, "55");
            sys.set_file(
                root.path()
                    .join("sys/class/backlight/intel_backlight/max_brightness"),
                "100",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.tx_reg.preview";
    let runtime = start_runtime(backend.clone(), name, false).await;
    backend.activate_hardware();
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    sysfs.clear_journal();
    platform.clear_journal();
    let ok = proxy.set_display_brightness(40).await.unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(sysfs.get_file(&path).as_deref(), Some("55"));
    assert!(sysfs.journal().is_empty() && platform.journal().is_empty());
}

#[tokio::test]
async fn m8_tx_lightbar_worker_ticks_at_frame_cadence() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, lighting) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.tx_reg.lightbar_cadence";
    let runtime = start_runtime(backend, name, true).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let cfg = serde_json::json!({
        "managed": true,
        "silent_mode": false,
        "welcome_anim": false,
        "power_settings": {
            "ac": { "enabled": true, "brightness_val": 200, "mode": "Rainbow", "color_mode": [255, 0, 0] },
            "type_c": { "enabled": true, "brightness_val": 200, "mode": "Rainbow", "color_mode": [255, 0, 0] },
            "bat": { "enabled": true, "brightness_val": 200, "mode": "Rainbow", "color_mode": [255, 0, 0] }
        }
    });
    assert!(proxy
        .set_lightbar_settings_json(&cfg.to_string())
        .await
        .unwrap());
    lighting.clear_ops();
    assert!(proxy.trigger_lightbar_animation(true).await.unwrap());
    tokio::time::sleep(Duration::from_millis(120)).await;
    let ops = lighting.ops().len();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(
        ops >= 3,
        "expected 20 ms frame cadence (>=3 writes in 120 ms), got {ops}"
    );
}

#[tokio::test]
async fn m8_tx_legacy_dbus_contracts_and_fan_catalog() {
    let root = tempfile::tempdir().unwrap();
    let custom = handoff_profile_with_tdps("one-custom", [45, 80, 95]);
    handoff_save_profiles(root.path(), &[&custom]);
    let fn_lock = root
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/fn_lock");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |sys| {
            let tree = root.path().join("sys/devices/system/cpu/cpu0/cpufreq");
            sys.set_file(tree.join("cpuinfo_min_freq"), "800000");
            sys.set_file(tree.join("cpuinfo_max_freq"), "4200000");
            sys.set_file(&fn_lock, "0");
            sys.set_write_error(&fn_lock);
        },
        |p| p,
    );
    let name = "org.test.uccd.tx_reg.contracts";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    assert!(!proxy.set_display_refresh_rate("eDP-1", 120).await.unwrap());
    assert!(proxy.set_fn_lock_status(true).await.is_err());
    let sys_info: serde_json::Value =
        serde_json::from_str(&proxy.get_system_info_json().await.unwrap()).unwrap();
    assert_eq!(sys_info["laptopModel"], "Test Laptop");
    assert!(
        sys_info["cpuModel"].is_string()
            && sys_info["dGpuModel"].is_string()
            && sys_info["iGpuModel"].is_string()
    );

    let kb_data = serde_json::json!({"color": "#123456"});
    assert!(proxy
        .save_custom_keyboard_profile("rgb-mine", "Mine", &kb_data.to_string())
        .await
        .unwrap());
    let kb_list: serde_json::Value =
        serde_json::from_str(&proxy.get_custom_keyboard_profiles_json().await.unwrap()).unwrap();
    assert_eq!(
        kb_list,
        serde_json::json!([{"id": "rgb-mine", "name": "Mine", "json": kb_data}])
    );

    let freq_limits: serde_json::Value =
        serde_json::from_str(&proxy.get_cpu_frequency_limits_json().await.unwrap()).unwrap();
    assert_eq!(
        (freq_limits["min"].as_i64(), freq_limits["max"].as_i64()),
        (Some(800000), Some(4200000))
    );

    let custom_list: Vec<serde_json::Value> =
        serde_json::from_str(&proxy.get_custom_profiles_json().await.unwrap()).unwrap();
    let custom_ids: Vec<_> = custom_list
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(custom_ids, ["one-custom"]);

    let fan_list: Vec<serde_json::Value> =
        serde_json::from_str(&proxy.get_fan_profile_names().await.unwrap()).unwrap();
    let silent: serde_json::Value =
        serde_json::from_str(&proxy.get_fan_profile("Silent [Built-in]").await.unwrap()).unwrap();
    assert_eq!(fan_list.len(), 5);
    assert!(fan_list.iter().any(|p| p["name"] == "Silent [Built-in]"));
    assert!(silent["tableCPU"].as_array().is_some_and(|a| !a.is_empty()));

    let topo: Vec<serde_json::Value> =
        serde_json::from_str(&proxy.get_cpu_topology_json().await.unwrap()).unwrap();
    let health: serde_json::Value =
        serde_json::from_str(&proxy.get_driver_health_json().await.unwrap()).unwrap();
    assert_eq!(topo.len(), 2);
    assert_eq!(health["healthy"], true);

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn recheck_01_winning_profile_without_tdp_overrides_ignores_in_flight_loser_write() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("before", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("loser", [41, 70, 90]);
    let mut winner = initial.clone();
    winner.id = "winner-without-tdp".to_string();
    winner.odm_power_limits.clear();
    let (sample_started, sample_entered) = std::sync::mpsc::channel();
    let (release_sample, sample_wait) = std::sync::mpsc::channel();
    let (writer_started, writer_entered) = std::sync::mpsc::channel();
    let (release_writer, writer_wait) = std::sync::mpsc::channel();
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let mut wrapper = TxRegressionPlatform::new(p);
            wrapper.gated_channel = 1;
            *wrapper.tdp_gate.lock().unwrap() = Some((writer_started, writer_wait));
            let wrapper = Arc::new(wrapper);
            tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.recheck_01";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    *ctl.read_tdp0_gate.lock().unwrap() = Some((sample_started, sample_wait));
    let winner_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .apply_profile(&serde_json::to_string(&winner).unwrap())
            .await
    });
    handoff_wait_gate(sample_entered).await;
    let old_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .apply_profile(&serde_json::to_string(&pending).unwrap())
            .await
    });
    handoff_wait_gate(writer_entered).await;
    assert_eq!(platform.read_tdp(0).unwrap(), 41);
    release_sample.send(()).unwrap();
    let winner_ok = winner_call.await.unwrap().unwrap();
    release_writer.send(()).unwrap();
    let old_ok = old_call.await.unwrap().unwrap();
    let final_pl1 = platform.read_tdp(0).unwrap();
    let active: serde_json::Value =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(winner_ok && old_ok);
    assert_eq!(active["id"], "winner-without-tdp");
    assert_eq!(final_pl1, 55);
}

#[tokio::test]
async fn recheck_02_failed_odm_only_read_preserves_tdp_channel_identity() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("known-limits", [55, 80, 95]);
    let mut target = handoff_profile_with_tdps("odm-only", [55, 80, 95]);
    target.odm_power_limits.clear();
    target.odm_profile_name = Some("performance".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(TxRegressionPlatform::new(p));
            tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.recheck_02";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    platform.set_perf_profile(2).unwrap();
    platform.clear_journal();
    ctl.omit_tdp1_read
        .store(true, std::sync::atomic::Ordering::SeqCst);
    ctl.odm_fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let ok = proxy
        .apply_profile(&serde_json::to_string(&target).unwrap())
        .await
        .unwrap();
    let actual: Vec<_> = (0..3).map(|i| platform.read_tdp(i).unwrap()).collect();
    let writes = platform.journal();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(actual, vec![55, 80, 95]);
    assert!(
        writes.is_empty(),
        "pre-validation should fail with zero hardware writes: {writes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn recheck_03_shutdown_deadline_stops_unstarted_rollback_writes() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("before", [55, 80, 95]);
    let pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
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
            let mut wrapper = TxRegressionPlatform::new(p);
            wrapper.gated_channel = 1;
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            let wrapper = Arc::new(wrapper);
            tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let ctl = rx.recv().unwrap();
    let name = "org.test.uccd.reg.recheck_03";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    *ctl.restore_gate.lock().unwrap() = Some((rollback_started, rollback_wait));
    let pending_call = tokio::spawn(async move {
        let c = session_conn().await;
        uccd_proxy(&c, name)
            .await
            .apply_profile(&serde_json::to_string(&pending).unwrap())
            .await
    });
    handoff_wait_gate(entered).await;
    let bus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    let shutdown = tokio::spawn(runtime.shutdown(Duration::from_millis(500)));
    tokio::time::timeout(Duration::from_secs(3), async {
        while bus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    release.send(()).unwrap();
    handoff_wait_gate(rollback_entered).await;
    shutdown.await.unwrap();
    platform.clear_journal();
    release_rollback.send(()).unwrap();
    let _ = pending_call.await.unwrap();
    let late = platform.journal();
    assert!(
        !late
            .iter()
            .any(|w| matches!(w, ucc_hardware::PlatformWrite::SetTdp { index: 0, .. })),
        "late writes after deadline: {late:?}"
    );
}

#[tokio::test]
async fn recheck_04_brightness_apply_rejects_mismatched_actual_brightness_and_restores_request() {
    let root = tempfile::tempdir().unwrap();
    let panel = root.path().join("sys/class/backlight/panel0");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(panel.join("brightness"), "55");
            s.set_file(panel.join("actual_brightness"), "55");
            s.set_file(panel.join("max_brightness"), "100");
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.recheck_04";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let mut profile = handoff_profile_with_tdps("brightness", [55, 80, 95]);
    profile.display.use_brightness = true;
    profile.display.brightness = 40;
    sysfs.set_file(panel.join("brightness"), "55");
    sysfs.set_file(panel.join("actual_brightness"), "55");
    let ok = proxy
        .apply_profile(&serde_json::to_string(&profile).unwrap())
        .await
        .unwrap();
    let actual = proxy.get_display_brightness().await.unwrap();
    let restored = sysfs.get_file(panel.join("brightness"));
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!ok);
    assert_eq!(actual, 55);
    assert_eq!(restored.as_deref(), Some("55"));
}

#[tokio::test]
async fn recheck_05_editing_active_custom_profile_reapplies_hardware_and_inactive_does_not() {
    let root = tempfile::tempdir().unwrap();
    let active_custom = handoff_profile_with_tdps("edit-active", [55, 80, 95]);
    let inactive_custom = handoff_profile_with_tdps("edit-inactive", [35, 65, 85]);
    handoff_save_profiles(root.path(), &[&active_custom, &inactive_custom]);
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.reg.recheck_05";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    assert!(proxy.set_active_profile(&active_custom.id).await.unwrap());
    let mut edited_inactive = inactive_custom.clone();
    edited_inactive.odm_power_limits[0] = 30;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&edited_inactive).unwrap())
        .await
        .unwrap());
    assert_eq!(
        platform.read_tdp(0).unwrap(),
        55,
        "editing inactive profile must not mutate hardware"
    );

    let mut edited_active = active_custom.clone();
    edited_active.odm_power_limits[0] = 45;
    assert!(proxy
        .update_custom_profile(&serde_json::to_string(&edited_active).unwrap())
        .await
        .unwrap());
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    assert_eq!(
        (active.odm_power_limits[0], platform.read_tdp(0).unwrap()),
        (45, 45)
    );

    // Failing hardware update (out-of-limits TDP) must NOT persist stale edit to disk or active RAM.
    let mut invalid_hw = edited_active.clone();
    invalid_hw.odm_power_limits[0] = 999;
    assert!(!proxy
        .update_custom_profile(&serde_json::to_string(&invalid_hw).unwrap())
        .await
        .unwrap());
    let saved = ConfigManager::load_from_path(&root.path().join("settings.json")).unwrap();
    assert_eq!(saved.profiles[&active_custom.id].odm_power_limits[0], 45);
    assert_eq!(platform.read_tdp(0).unwrap(), 45);

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn recheck_06_custom_profile_mutators_reject_builtin_ids() {
    let root = tempfile::tempdir().unwrap();
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.reg.recheck_06";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    let mut builtin = default_quiet_profile();
    builtin.name = "Tampered Built-in".into();
    let body = serde_json::to_string(&builtin).unwrap();
    assert!(!proxy.add_custom_profile(&body).await.unwrap());
    assert!(!proxy.save_custom_profile(&body).await.unwrap());
    assert!(!proxy.update_custom_profile(&body).await.unwrap());
    assert!(!proxy.delete_custom_profile(&builtin.id).await.unwrap());

    let missing = handoff_profile_with_tdps("does-not-exist", [45, 75, 95]);
    assert!(!proxy
        .update_custom_profile(&serde_json::to_string(&missing).unwrap())
        .await
        .unwrap());

    let all: Vec<ucc_core::UccProfile> =
        serde_json::from_str(&proxy.get_profiles_json().await.unwrap()).unwrap();
    assert_eq!(
        all.iter().find(|p| p.id == builtin.id).unwrap().name,
        default_quiet_profile().name
    );
    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn recheck_08_prime_display_modes_x11_and_yuv420_report_real_system_state() {
    let root = tempfile::tempdir().unwrap();
    let drm = root.path().join("sys/class/drm/card0-eDP-1");
    let sess = root.path().join("run/systemd/sessions/2");
    let (backend, _, sysfs, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(root.path().join("etc/prime-discrete"), "on\n");
            s.set_file(drm.join("status"), "connected\n");
            s.set_file(
                drm.join("modes"),
                "2560x1600@240\n2560x1600@60\n1920x1200\n",
            );
            s.set_file(drm.join("mode"), "2560x1600@240\n");
            s.set_file(drm.join("force_yuv420_output"), "0\n");
            s.set_file(sess.clone(), "STATE=active\nACTIVE=1\nTYPE=wayland\n");
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.recheck_08";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    assert_eq!(proxy.get_prime_state().await.unwrap(), "dGPU");
    sysfs.set_file(root.path().join("etc/prime-discrete"), "off\n");
    assert_eq!(proxy.get_prime_state().await.unwrap(), "iGPU");
    sysfs.set_file(root.path().join("etc/prime-discrete"), "on-demand\n");
    assert_eq!(proxy.get_prime_state().await.unwrap(), "on-demand");

    assert!(!proxy.get_is_x11().await.unwrap());
    sysfs.set_file(sess, "STATE=active\nACTIVE=1\nTYPE=x11\n");
    assert!(proxy.get_is_x11().await.unwrap());
    assert!(proxy
        .get_force_yuv420_output_switch_available()
        .await
        .unwrap());

    let modes: serde_json::Value =
        serde_json::from_str(&proxy.get_display_modes_json().await.unwrap()).unwrap();
    assert_eq!(modes["displayName"], "eDP-1");
    assert_eq!(modes["activeMode"]["xResolution"], 2560);
    assert_eq!(modes["activeMode"]["yResolution"], 1600);
    assert_eq!(
        modes["activeMode"]["refreshRates"],
        serde_json::json!([240])
    );
    assert_eq!(modes["displayModes"].as_array().unwrap().len(), 2);
    assert_eq!(modes["displayModes"][1]["xResolution"], 1920);
    assert_eq!(
        modes["displayModes"][1]["refreshRates"],
        serde_json::json!([])
    );

    runtime.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn recheck_09_system_info_detects_igpu_and_dgpu_from_pci_sysfs_without_telemetry() {
    let root = tempfile::tempdir().unwrap();
    let igpu = root.path().join("sys/bus/pci/devices/0000:00:02.0");
    let dgpu = root.path().join("sys/bus/pci/devices/0000:01:00.0");
    let (backend, _, _, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(igpu.join("class"), "0x030000\n");
            s.set_file(igpu.join("vendor"), "0x8086\n");
            s.set_file(igpu.join("device"), "0xa7a0\n");
            s.set_file(dgpu.join("class"), "0x030000\n");
            s.set_file(dgpu.join("vendor"), "0x10de\n");
            s.set_file(dgpu.join("device"), "0x28e0\n");
            s.set_file(
                root.path().join("usr/share/hwdata/pci.ids"),
                "10de  NVIDIA Corporation\n\t28e0  AD106M [GeForce RTX 4060 Max-Q / Mobile]\n8086  Intel Corporation\n\ta7a0  Raptor Lake-P [Iris Xe Graphics]\n",
            );
            s.set_file(
                root.path().join("proc/cpuinfo"),
                "processor\t: 0\nmodel name\t: Fixture Core i7\n",
            );
        },
        |p| p,
    );
    let name = "org.test.uccd.reg.recheck_09";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    proxy
        .set_sensor_data_collection_status(false)
        .await
        .unwrap();

    let info: serde_json::Value =
        serde_json::from_str(&proxy.get_system_info_json().await.unwrap()).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;

    assert_eq!(info["cpuModel"], "Fixture Core i7");
    assert_eq!(info["iGpuModel"], "Intel Iris Xe Graphics");
    assert_eq!(info["dGpuModel"], "NVIDIA GeForce RTX 4060 Max-Q / Mobile");
}
