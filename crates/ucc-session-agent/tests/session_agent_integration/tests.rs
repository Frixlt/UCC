// SPDX-License-Identifier: GPL-3.0-or-later

#[allow(unused_imports)]
use super::harness::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

#[tokio::test]
async fn full_27_05_driver_alert_and_power_source_notifications() {
    let (bus_dir, bus_addr, mut _dbus_child) = spawn_isolated_dbus().await;
    let runtime_dir = bus_dir.path().join("runtime");
    let drm_dir = bus_dir.path().join("drm");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&drm_dir).unwrap();

    let notif_state = Arc::new(FakeNotificationsState::default());
    let _notif_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            FakeNotifications(Arc::clone(&notif_state)),
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let uccd_state = Arc::new(FakeUccdState::new("ac", 240, false));
    uccd_state.healthy.store(false, Ordering::SeqCst);
    let svc_name = "org.test.uccd.session.alerts";
    let uccd_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name(svc_name)
        .unwrap()
        .serve_at("/com/uniwill/uccd", FakeUccd(Arc::clone(&uccd_state)))
        .unwrap()
        .build()
        .await
        .unwrap();

    let exe = env!("CARGO_BIN_EXE_ucc-session-agent");
    let mut agent = Command::new(exe)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus_addr)
        .env("UCCD_DBUS_SERVICE", svc_name)
        .env("UCC_LID_BUS_SESSION", "1")
        .env("UCC_DRM_ROOT", &drm_dir)
        .env("UCC_HEALTH_POLL_MS", "150")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("KDE_SESSION_VERSION")
        .env_remove("XDG_CURRENT_DESKTOP")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // 1. Driver fault triggers Notify.
    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state.notifies.lock().unwrap().iter().any(|n| {
                n.app_name == "ucc-rusted"
                    && n.summary == "UCC Driver Alert"
                    && n.body.contains("tuxedo_io missing")
            })
        })
        .await,
        "expected driver fault desktop notification"
    );
    let driver_notif_id = notif_state
        .notifies
        .lock()
        .unwrap()
        .iter()
        .find(|n| n.body.contains("tuxedo_io missing"))
        .unwrap()
        .id;

    // 2. Driver recovery sends CloseNotification(driver_notif_id).
    uccd_state.healthy.store(true, Ordering::SeqCst);
    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state
                .closed
                .lock()
                .unwrap()
                .contains(&driver_notif_id)
        })
        .await,
        "expected CloseNotification once driver recovers"
    );

    // 3. Power source transition ac -> bat triggers debounced Notify (800ms).
    let bat_policy =
        json!({"real_power_source": "bat", "current_mode": "bat", "effective_source": "bat"})
            .to_string();
    *uccd_state.policy_json.lock().unwrap() = bat_policy.clone();
    let iface_ref = uccd_conn
        .object_server()
        .interface::<_, FakeUccd>("/com/uniwill/uccd")
        .await
        .unwrap();
    FakeUccd::power_policy_changed(iface_ref.signal_emitter(), &bat_policy)
        .await
        .unwrap();

    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state
                .notifies
                .lock()
                .unwrap()
                .iter()
                .any(|n| n.icon == "battery")
        })
        .await,
        "expected power source transition battery notification"
    );

    let _ = agent.kill().await;
}

#[tokio::test]
async fn full_27_06_refresh_advisor_and_lid_dpms_dispatch() {
    let (bus_dir, bus_addr, mut _dbus_child) = spawn_isolated_dbus().await;
    let runtime_dir = bus_dir.path().join("runtime");
    let drm_dir = bus_dir.path().join("drm");
    let bin_dir = bus_dir.path().join("bin");
    let cmd_log = bus_dir.path().join("hyprctl.log");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&drm_dir).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_fake_hyprctl(&bin_dir, &cmd_log);

    let notif_state = Arc::new(FakeNotificationsState::default());
    let notif_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            FakeNotifications(Arc::clone(&notif_state)),
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let login_conn = connect_bus(&bus_addr, Some("org.freedesktop.login1")).await;
    login_conn
        .object_server()
        .at(
            "/org/freedesktop/login1",
            FakeLogin1 {
                lid_closed: AtomicBool::new(false),
            },
        )
        .await
        .unwrap();

    let uccd_state = Arc::new(FakeUccdState::new("bat", 60, true));
    let svc_name = "org.test.uccd.session.refresh_lid";
    let _uccd_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name(svc_name)
        .unwrap()
        .serve_at("/com/uniwill/uccd", FakeUccd(Arc::clone(&uccd_state)))
        .unwrap()
        .build()
        .await
        .unwrap();

    let orig_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{orig_path}", bin_dir.display());
    let exe = env!("CARGO_BIN_EXE_ucc-session-agent");
    let mut agent = Command::new(exe)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus_addr)
        .env("UCCD_DBUS_SERVICE", svc_name)
        .env("UCC_LID_BUS_SESSION", "1")
        .env("UCC_DRM_ROOT", &drm_dir)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("HYPRLAND_INSTANCE_SIGNATURE", "test-instance")
        .env("PATH", test_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // 1. RefreshAdvisor suggests switching 240Hz -> 60Hz on battery/60Hz profile.
    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state
                .notifies
                .lock()
                .unwrap()
                .iter()
                .any(|n| n.actions.iter().any(|a| a == "apply"))
        })
        .await,
        "expected RefreshAdvisor actionable notification"
    );
    let refresh_id = notif_state
        .notifies
        .lock()
        .unwrap()
        .iter()
        .find(|n| n.actions.iter().any(|a| a == "apply"))
        .unwrap()
        .id;

    let notif_iface = notif_conn
        .object_server()
        .interface::<_, FakeNotifications>("/org/freedesktop/Notifications")
        .await
        .unwrap();
    FakeNotifications::action_invoked(notif_iface.signal_emitter(), refresh_id, "apply")
        .await
        .unwrap();

    let read_log = || std::fs::read_to_string(&cmd_log).unwrap_or_default();
    assert!(
        poll_until(Duration::from_secs(4), || {
            read_log().contains("keyword monitor eDP-1,2560x1600@60,auto,auto")
        })
        .await,
        "expected ActionInvoked('apply') to invoke hyprctl keyword monitor, log={}",
        read_log()
    );

    // 2. LidClosed true -> dpms off; false -> dpms on.
    let login_iface = login_conn
        .object_server()
        .interface::<_, FakeLogin1>("/org/freedesktop/login1")
        .await
        .unwrap();
    {
        let iface = login_iface.get().await;
        iface.lid_closed.store(true, Ordering::SeqCst);
        iface
            .lid_closed_changed(login_iface.signal_emitter())
            .await
            .unwrap();
    }
    assert!(
        poll_until(Duration::from_secs(4), || {
            read_log().contains("dispatch dpms off eDP-1")
        })
        .await,
        "expected LidClosed=true to invoke dpms off, log={}",
        read_log()
    );

    {
        let iface = login_iface.get().await;
        iface.lid_closed.store(false, Ordering::SeqCst);
        iface
            .lid_closed_changed(login_iface.signal_emitter())
            .await
            .unwrap();
    }
    assert!(
        poll_until(Duration::from_secs(4), || {
            read_log().contains("dispatch dpms on eDP-1")
        })
        .await,
        "expected LidClosed=false to invoke dpms on, log={}",
        read_log()
    );

    let _ = agent.kill().await;
}

#[tokio::test]
async fn recheck_04_stale_refresh_offer_invalidated_after_profile_power_sleep_and_apply() {
    let (bus_dir, bus_addr, mut _dbus_child) = spawn_isolated_dbus().await;
    let runtime_dir = bus_dir.path().join("runtime");
    let drm_dir = bus_dir.path().join("drm");
    let bin_dir = bus_dir.path().join("bin");
    let cmd_log = bus_dir.path().join("hyprctl.log");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&drm_dir).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    write_fake_hyprctl(&bin_dir, &cmd_log);

    let notif_state = Arc::new(FakeNotificationsState::default());
    let notif_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            FakeNotifications(Arc::clone(&notif_state)),
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let login_conn = connect_bus(&bus_addr, Some("org.freedesktop.login1")).await;
    login_conn
        .object_server()
        .at(
            "/org/freedesktop/login1",
            FakeLogin1 {
                lid_closed: AtomicBool::new(false),
            },
        )
        .await
        .unwrap();

    let uccd_state = Arc::new(FakeUccdState::new("bat", 60, true));
    let svc_name = "org.test.uccd.session.recheck04";
    let uccd_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name(svc_name)
        .unwrap()
        .serve_at("/com/uniwill/uccd", FakeUccd(Arc::clone(&uccd_state)))
        .unwrap()
        .build()
        .await
        .unwrap();

    let orig_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{orig_path}", bin_dir.display());
    let exe = env!("CARGO_BIN_EXE_ucc-session-agent");
    let mut agent = Command::new(exe)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus_addr)
        .env("UCCD_DBUS_SERVICE", svc_name)
        .env("UCC_LID_BUS_SESSION", "1")
        .env("UCC_DRM_ROOT", &drm_dir)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("HYPRLAND_INSTANCE_SIGNATURE", "test-instance")
        .env("PATH", test_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Wait for initial 60Hz suggestion notification on bat/60Hz profile.
    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state
                .notifies
                .lock()
                .unwrap()
                .iter()
                .any(|n| n.actions.iter().any(|a| a == "apply"))
        })
        .await
    );
    let old_60hz_id = notif_state
        .notifies
        .lock()
        .unwrap()
        .iter()
        .find(|n| n.actions.iter().any(|a| a == "apply"))
        .unwrap()
        .id;

    let read_log = || std::fs::read_to_string(&cmd_log).unwrap_or_default();
    let reads_before = uccd_state.profile_reads.load(Ordering::SeqCst);

    // Case 1: Live state changed to 240Hz without signal -> clicking old 60Hz notification re-queries and aborts!
    *uccd_state.profile_json.lock().unwrap() = json!({
        "id": "raced_240",
        "display": { "useRefRate": true, "refreshRate": 240 }
    })
    .to_string();
    let notif_iface = notif_conn
        .object_server()
        .interface::<_, FakeNotifications>("/org/freedesktop/Notifications")
        .await
        .unwrap();
    FakeNotifications::action_invoked(notif_iface.signal_emitter(), old_60hz_id, "apply")
        .await
        .unwrap();
    assert!(
        poll_until(Duration::from_secs(3), || {
            uccd_state.profile_reads.load(Ordering::SeqCst) > reads_before
                && notif_state.closed.lock().unwrap().contains(&old_60hz_id)
        })
        .await,
        "expected ActionInvoked to re-query active profile and close old 60Hz notification"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !read_log().contains("keyword monitor eDP-1,2560x1600@60,auto,auto"),
        "raced 60Hz notification applied against live 240Hz profile: {}",
        read_log()
    );

    // Trigger a fresh 60Hz offer, then emit ProfileChanged -> 240Hz: offer must close and ActionInvoked must do nothing.
    let n_before_60 = notif_state.notifies.lock().unwrap().len();
    *uccd_state.profile_json.lock().unwrap() = json!({
        "id": "back_60",
        "display": { "useRefRate": true, "refreshRate": 60 }
    })
    .to_string();
    let uccd_iface = uccd_conn
        .object_server()
        .interface::<_, FakeUccd>("/com/uniwill/uccd")
        .await
        .unwrap();
    FakeUccd::profile_changed(uccd_iface.signal_emitter(), "back_60", "", "")
        .await
        .unwrap();
    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state
                .notifies
                .lock()
                .unwrap()
                .iter()
                .skip(n_before_60)
                .any(|n| n.actions.iter().any(|a| a == "apply"))
        })
        .await
    );
    let mid_60hz_id = notif_state
        .notifies
        .lock()
        .unwrap()
        .iter()
        .skip(n_before_60)
        .find(|n| n.actions.iter().any(|a| a == "apply"))
        .unwrap()
        .id;

    let reads_mid = uccd_state.profile_reads.load(Ordering::SeqCst);
    *uccd_state.profile_json.lock().unwrap() = json!({
        "id": "now_240",
        "display": { "useRefRate": true, "refreshRate": 240 }
    })
    .to_string();
    FakeUccd::profile_changed(uccd_iface.signal_emitter(), "now_240", "", "")
        .await
        .unwrap();
    assert!(
        poll_until(Duration::from_secs(3), || {
            uccd_state.profile_reads.load(Ordering::SeqCst) > reads_mid
                && notif_state.closed.lock().unwrap().contains(&mid_60hz_id)
        })
        .await,
        "expected ProfileChanged to close old 60Hz notification and re-read profile"
    );
    FakeNotifications::action_invoked(notif_iface.signal_emitter(), mid_60hz_id, "apply")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !read_log().contains("keyword monitor eDP-1,2560x1600@60,auto,auto"),
        "stale 60Hz notification applied after ProfileChanged to 240Hz: {}",
        read_log()
    );

    // Case 2: Fresh 60Hz offer invalidated by PrepareForSleep(true) before ActionInvoked.
    let n_before_sleep = notif_state.notifies.lock().unwrap().len();
    *uccd_state.profile_json.lock().unwrap() = json!({
        "id": "back_60_sleep",
        "display": { "useRefRate": true, "refreshRate": 60 }
    })
    .to_string();
    FakeUccd::profile_changed(uccd_iface.signal_emitter(), "back_60_sleep", "", "")
        .await
        .unwrap();
    assert!(
        poll_until(Duration::from_secs(4), || {
            notif_state
                .notifies
                .lock()
                .unwrap()
                .iter()
                .skip(n_before_sleep)
                .any(|n| n.actions.iter().any(|a| a == "apply"))
        })
        .await
    );
    let second_id = notif_state
        .notifies
        .lock()
        .unwrap()
        .iter()
        .skip(n_before_sleep)
        .find(|n| n.actions.iter().any(|a| a == "apply"))
        .unwrap()
        .id;

    let login_iface = login_conn
        .object_server()
        .interface::<_, FakeLogin1>("/org/freedesktop/login1")
        .await
        .unwrap();
    FakeLogin1::prepare_for_sleep(login_iface.signal_emitter(), true)
        .await
        .unwrap();
    assert!(
        poll_until(Duration::from_secs(3), || {
            notif_state.closed.lock().unwrap().contains(&second_id)
        })
        .await,
        "PrepareForSleep(true) must close pending refresh notification"
    );
    FakeNotifications::action_invoked(notif_iface.signal_emitter(), second_id, "apply")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !read_log().contains("keyword monitor eDP-1,2560x1600@60,auto,auto"),
        "invalidated offer after PrepareForSleep(true) must not apply: {}",
        read_log()
    );

    let _ = agent.kill().await;
}

#[tokio::test]
async fn deep_04_hung_hyprctl_does_not_delay_driver_health_notification_and_is_reaped() {
    let (bus_dir, bus_addr, mut _dbus_child) = spawn_isolated_dbus().await;
    let runtime_dir = bus_dir.path().join("runtime");
    let drm_dir = bus_dir.path().join("drm");
    let bin_dir = bus_dir.path().join("bin");
    let pid_file = bus_dir.path().join("hyprctl.pid");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&drm_dir).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();

    let hypr_script = bin_dir.join("hyprctl");
    std::fs::write(
        &hypr_script,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$$\" >> \"{}\"\nexec sleep 30\n",
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hypr_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let notif_state = Arc::new(FakeNotificationsState::default());
    let notif_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            FakeNotifications(Arc::clone(&notif_state)),
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let _ = notif_conn;

    let uccd_state = Arc::new(FakeUccdState::new("bat", 60, false));
    uccd_state.healthy.store(false, Ordering::SeqCst);
    let svc_name = "org.test.uccd.session.deep04";
    let _uccd_conn = zbus::connection::Builder::address(bus_addr.as_str())
        .unwrap()
        .name(svc_name)
        .unwrap()
        .serve_at("/com/uniwill/uccd", FakeUccd(Arc::clone(&uccd_state)))
        .unwrap()
        .build()
        .await
        .unwrap();

    let orig_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{orig_path}", bin_dir.display());
    let exe = env!("CARGO_BIN_EXE_ucc-session-agent");
    let mut agent = Command::new(exe)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus_addr)
        .env("UCCD_DBUS_SERVICE", svc_name)
        .env("UCC_LID_BUS_SESSION", "1")
        .env("UCC_DRM_ROOT", &drm_dir)
        .env("UCC_HEALTH_POLL_MS", "50")
        .env("UCC_COMPOSITOR_CMD_TIMEOUT_MS", "900")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("HYPRLAND_INSTANCE_SIGNATURE", "test-instance")
        .env("PATH", test_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Wait until hung hyprctl has definitely started, then check DriverHealth alert arrives
    // promptly while hyprctl is still blocked (`DEEP-04`).
    assert!(
        poll_until(Duration::from_secs(2), || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.lines().next().and_then(|l| l.trim().parse::<i32>().ok()))
                .is_some()
        })
        .await,
        "hung hyprctl script never recorded its PID"
    );
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let after_spawn = std::time::Instant::now();

    assert!(
        poll_until(Duration::from_millis(450), || {
            notif_state
                .notifies
                .lock()
                .unwrap()
                .iter()
                .any(|n| n.summary.contains("UCC Driver Alert") || n.body.contains("tuxedo_io"))
        })
        .await,
        "DriverHealth alert delayed past 450ms by hung compositor helper"
    );
    assert!(after_spawn.elapsed() < Duration::from_millis(450));
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "DriverHealth arrived only after hyprctl exited rather than concurrently"
    );

    assert!(
        poll_until(Duration::from_secs(3), || {
            !std::path::Path::new(&format!("/proc/{pid}")).exists()
        })
        .await,
        "timed-out hyprctl pid={pid} was not killed/reaped"
    );

    let _ = agent.kill().await;
}
