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

struct CommitAuditPlatform {
    inner: Arc<MockPlatformBackend>,
    reject_restore: std::sync::atomic::AtomicBool,
    reject_second_tdp: std::sync::atomic::AtomicBool,
    clamp_pl1: bool,
    hide_pl1_readback: bool,
    tdp_gate: std::sync::Mutex<Option<RecheckStartupGate>>,
}

impl CommitAuditPlatform {
    fn new(inner: Arc<MockPlatformBackend>) -> Self {
        Self {
            inner,
            reject_restore: std::sync::atomic::AtomicBool::new(false),
            reject_second_tdp: std::sync::atomic::AtomicBool::new(false),
            clamp_pl1: false,
            hide_pl1_readback: false,
            tdp_gate: std::sync::Mutex::new(None),
        }
    }
}

impl PlatformBackend for CommitAuditPlatform {
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
        if idx == 0 && self.hide_pl1_readback {
            return Err(PlatformError::Unavailable);
        }
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
            }
        }
        use std::sync::atomic::Ordering::SeqCst;
        if idx == 1 && self.reject_second_tdp.load(SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        if idx == 0 && watts == 55 {
            if self.reject_restore.load(SeqCst) {
                return Err(PlatformError::Unavailable);
            }
            if self.clamp_pl1 {
                return self.inner.set_tdp(idx, 50);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_commit_superseded_write_restores_channel_unmanaged_by_newer_profile() {
    let root = tempfile::tempdir().unwrap();
    let old = handoff_profile_with_tdps("manage-tdp", [41, 70, 90]);
    let mut next = default_quiet_profile();
    next.id = "leave-tdp-alone".into();
    next.odm_profile_name = None;
    next.odm_power_limits.clear();
    handoff_save_profiles(root.path(), &[&old, &next]);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = CommitAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.commit_reg.omitted";
    let runtime = start_runtime(backend, name, false).await;
    platform.set_tdp_value(0, 65);
    let older = tokio::spawn(async move {
        let conn = session_conn().await;
        uccd_proxy(&conn, name)
            .await
            .set_active_profile(&old.id)
            .await
            .unwrap()
    });
    handoff_wait_gate(entered).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let newer =
        tokio::time::timeout(Duration::from_secs(2), proxy.set_active_profile(&next.id)).await;
    let _ = release.send(());
    let older_ok = older.await.unwrap();
    let newer_ok = newer.unwrap().unwrap();
    let actual = platform.read_tdp(0).unwrap();
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(older_ok && newer_ok);
    assert_eq!(active, next.id);
    assert_eq!(actual, 65);
}

#[tokio::test]
async fn m8_commit_failed_rollback_syncs_active_readback() {
    let root = tempfile::tempdir().unwrap();
    let initial = handoff_profile_with_tdps("known", [55, 80, 95]);
    let bad = handoff_profile_with_tdps("fails-during-write", [41, 70, 90]);
    let (tx, rx) = std::sync::mpsc::channel();
    let (backend, platform, _, _) = build_custom_backend(
        root.path(),
        true,
        |_| {},
        |p| {
            let wrapper = Arc::new(CommitAuditPlatform::new(p));
            tx.send(wrapper.clone()).unwrap();
            wrapper
        },
    );
    let platform_control = rx.recv().unwrap();
    let name = "org.test.uccd.commit_reg.rollback";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    assert!(proxy
        .apply_profile(&serde_json::to_string(&initial).unwrap())
        .await
        .unwrap());
    platform_control
        .reject_second_tdp
        .store(true, std::sync::atomic::Ordering::SeqCst);
    platform_control
        .reject_restore
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let accepted = proxy
        .apply_profile(&serde_json::to_string(&bad).unwrap())
        .await
        .unwrap();
    let active: ucc_core::UccProfile =
        serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
    let readback = platform.read_tdp(0).unwrap() as u32;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!accepted);
    assert_eq!((readback, active.odm_power_limits[0]), (41, 41));
}

#[tokio::test]
async fn m8_commit_ordinary_apply_rejects_clamped_or_unavailable_readback() {
    for hide_readback in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let requested = handoff_profile_with_tdps("would-be-clamped", [55, 80, 95]);
        let (backend, platform, _, _) = build_custom_backend(
            root.path(),
            true,
            |_| {},
            |p| {
                let mut wrapper = CommitAuditPlatform::new(p);
                wrapper.clamp_pl1 = !hide_readback;
                wrapper.hide_pl1_readback = hide_readback;
                Arc::new(wrapper)
            },
        );
        let name = if hide_readback {
            "org.test.uccd.commit_reg.missing_readback"
        } else {
            "org.test.uccd.commit_reg.clamp"
        };
        let runtime = start_runtime(backend, name, false).await;
        let conn = session_conn().await;
        let proxy = uccd_proxy(&conn, name).await;
        if !hide_readback {
            let reply = proxy
                .apply_profile(&serde_json::to_string(&requested).unwrap())
                .await
                .unwrap();
            assert!(!reply);
            assert_eq!(platform.read_tdp(0).unwrap(), 90);
            let active: ucc_core::UccProfile =
                serde_json::from_str(&proxy.get_active_profile_json().await.unwrap()).unwrap();
            assert_ne!(active.id, requested.id);
        } else {
            // Even baseline check cannot confirm PL1 when read_tdp fails -> rejected cleanly.
            let reply = proxy
                .apply_profile(&serde_json::to_string(&requested).unwrap())
                .await
                .unwrap();
            assert!(!reply);
        }
        runtime.shutdown(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn m8_commit_startup_does_not_claim_unapplied_profile_and_recovers_later() {
    let root = tempfile::tempdir().unwrap();
    let profile = handoff_profile_with_tdps("bad-powered-profile", [45, 125, 130]);
    let mut config = create_default_settings();
    config.profiles.insert(profile.id.clone(), profile.clone());
    config.set_active_profile_for_mode("power_ac", &profile.id);
    ConfigManager::with_custom_path(root.path().join("settings.json"))
        .save(&config)
        .unwrap();
    let (backend, platform, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.commit_reg.unapplied_start";
    let runtime = start_runtime(backend.clone(), name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;
    let active = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let pl2_before = platform.read_tdp(1).unwrap();
    assert_ne!(active, profile.id);
    assert_eq!(pl2_before, 120);

    // Raise hardware PL2 ceiling so background reconciliation can now apply the configured profile.
    platform.set_tdp_limits(1, (20, 130));
    backend.reconcile_power_state(false);
    let active_after = active_profile_id(&proxy.get_active_profile_json().await.unwrap());
    let pl2_after = platform.read_tdp(1).unwrap();
    runtime.shutdown(Duration::from_secs(1)).await;
    assert_eq!((active_after, pl2_after), (profile.id, 125));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn m8_commit_successor_captures_cpu_defaults_only_after_predecessor_restores() {
    let root = tempfile::tempdir().unwrap();
    let mut pending = handoff_profile_with_tdps("pending", [41, 70, 90]);
    pending.cpu.governor = "performance".into();
    handoff_save_profiles(root.path(), &[&pending]);
    let governor = root
        .path()
        .join("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let (started, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (old, old_platform, shared, _) = build_custom_backend(
        root.path(),
        true,
        |s| {
            s.set_file(&governor, "powersave");
            s.set_file(
                governor.with_file_name("scaling_available_governors"),
                "powersave performance",
            );
        },
        |p| {
            let wrapper = CommitAuditPlatform::new(p);
            *wrapper.tdp_gate.lock().unwrap() = Some((started, wait));
            Arc::new(wrapper)
        },
    );
    let name = "org.test.uccd.commit_reg.cpu_handoff";
    let old = start_runtime(old, name, false).await;
    let request = tokio::spawn(async move {
        let conn = session_conn().await;
        let _ = uccd_proxy(&conn, name)
            .await
            .set_active_profile(&pending.id)
            .await;
    });
    handoff_wait_gate(entered).await;
    assert_eq!(shared.get_file(&governor).as_deref(), Some("performance"));
    old.shutdown(Duration::from_millis(30)).await;
    let (next, _, _, _) = build_custom_backend_with_sysfs(
        root.path(),
        true,
        Some(shared.clone()),
        |_| {},
        |_| old_platform,
    );
    let next = tokio::spawn(start_runtime(next, name, false));
    let conn = session_conn().await;
    handoff_wait_name(&conn, name).await;
    let _ = release.send(());
    let _ = request.await;
    let next = next.await.unwrap();
    next.shutdown(Duration::from_secs(1)).await;
    assert_eq!(shared.get_file(&governor).as_deref(), Some("powersave"));
}

#[tokio::test]
async fn m8_commit_failed_batch_and_single_state_map_do_not_persist_changes() {
    let root = tempfile::tempdir().unwrap();
    let invalid = handoff_profile_with_tdps("cannot-apply", [45, 125, 130]);
    handoff_save_profiles(root.path(), &[&invalid]);
    let settings = root.path().join("settings.json");
    let initial = ConfigManager::load_from_path(&settings).unwrap().state_map;
    let (backend, _, _, _) = build_custom_backend(root.path(), true, |_| {}, |p| p);
    let name = "org.test.uccd.commit_reg.state_map";
    let runtime = start_runtime(backend, name, false).await;
    let conn = session_conn().await;
    let proxy = uccd_proxy(&conn, name).await;

    let batch = serde_json::json!({"power_ac": invalid.id, "power_bat": invalid.id}).to_string();
    let batch_ok = proxy.set_batch_state_map(&batch).await.unwrap();
    let single_ok = proxy.set_state_map("power_ac", &invalid.id).await.unwrap();
    let saved = ConfigManager::load_from_path(&settings).unwrap().state_map;
    runtime.shutdown(Duration::from_secs(1)).await;
    assert!(!batch_ok && !single_ok);
    assert_eq!(saved, initial);
}
