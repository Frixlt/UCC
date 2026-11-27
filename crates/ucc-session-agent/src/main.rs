// SPDX-License-Identifier: GPL-3.0-or-later

//! Session background agent relaying notifications to lightbar, monitoring driver health,
//! alerting on power transitions, and handling lid/refresh events.

mod notifications;

use notifications::{
    close_notify, drm_root_from_env, format_refresh_prompt, internal_monitors_from_config,
    next_optional, parse_policy_source, parse_requested_hz, query_compositor_config, run_commands,
    send_notify, LoginManagerProxy, NotificationsProxy, UPowerProxy,
};
use serde_json::Value;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{info, warn};
use ucc_dbus::{UccdProxy, DBUS_INTERFACE, DBUS_PATH, DBUS_SERVICE};
use ucc_session_agent::{
    DriverAlertAction, DriverAlertState, LidDisplayHandler, NotificationMonitorParser,
    PowerSourceAlertState, RefreshAdvisor, RefreshBackend, RefreshPlan,
};
use zbus::Connection;

fn acquire_single_instance_lock() -> Option<std::fs::File> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let lock_path = PathBuf::from(runtime).join("ucc-session-agent.lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .ok()?;
    let rc = unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    if rc == 0 {
        Some(file)
    } else {
        None
    }
}

async fn connect_uccd() -> Result<(Connection, UccdProxy<'static>), String> {
    let service_name =
        std::env::var("UCCD_DBUS_SERVICE").unwrap_or_else(|_| DBUS_SERVICE.to_string());
    let use_session =
        std::env::var("UCCD_DBUS_SERVICE").is_ok() || std::env::var("UCCD_DBUS_SESSION").is_ok();
    let conn = if use_session {
        Connection::session().await
    } else {
        Connection::system().await
    }
    .map_err(|e| format!("failed to connect to D-Bus: {e}"))?;

    let proxy = UccdProxy::builder(&conn)
        .destination(service_name)
        .map_err(|e| format!("invalid service name: {e}"))?
        .path(DBUS_PATH)
        .map_err(|e| format!("invalid path: {e}"))?
        .interface(DBUS_INTERFACE)
        .map_err(|e| format!("invalid interface: {e}"))?
        .build()
        .await
        .map_err(|e| format!("failed to build uccd proxy: {e}"))?;

    Ok((conn, proxy))
}

fn lightbar_notify_enabled(settings: &Value, policy: &Value) -> bool {
    let managed = settings
        .get("managed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let silent = settings
        .get("silent_mode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mode = policy
        .get("current_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let enabled = settings
        .get("power_settings")
        .and_then(|ps| ps.get(mode))
        .and_then(|m| m.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    managed && !silent && enabled
}

struct SessionState {
    backend: RefreshBackend,
    drm_root: PathBuf,
    policy_source: String,
    requested_hz: i32,
    pending_refresh: Option<(u32, RefreshPlan)>,
    last_notified_refresh: Option<(String, String)>,
}

impl SessionState {
    async fn maybe_suggest_refresh(&mut self, notif: Option<&NotificationsProxy<'_>>) {
        let Some(cfg) = query_compositor_config(self.backend).await else {
            return;
        };
        let Some(plan) =
            RefreshAdvisor::plan(&cfg, &self.policy_source, &self.drm_root, self.requested_hz)
        else {
            self.last_notified_refresh = None;
            return;
        };
        let key = (plan.output.clone(), plan.target_mode.clone());
        if self.last_notified_refresh.as_ref() == Some(&key) {
            return;
        }
        let body = format_refresh_prompt(&plan);
        if let Some(id) = send_notify(
            notif,
            "video-display",
            "Display Refresh Rate",
            &body,
            &["apply", "Switch", "dismiss", "Dismiss"],
            10000,
        )
        .await
        {
            self.last_notified_refresh = Some(key);
            self.pending_refresh = Some((id, plan));
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("ucc-session-agent {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("UCC desktop notification relay and KDE/Hyprland display-refresh suggestions. No notification content is retained.");
        return;
    }
    if ucc_config::is_read_only_preview() {
        println!("Session relay disabled in read-only preview.");
        return;
    }

    tracing_subscriber::fmt::init();

    let Some(_lock_guard) = acquire_single_instance_lock() else {
        info!("ucc-session-agent is already running");
        return;
    };

    let (_conn, proxy) = match connect_uccd().await {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(2);
        }
    };

    let session_conn = Connection::session().await.ok();
    let own_bus_name = session_conn
        .as_ref()
        .and_then(|c| c.unique_name().map(|n| n.as_str().to_string()));
    let notif_proxy = if let Some(ref sc) = session_conn {
        NotificationsProxy::new(sc).await.ok()
    } else {
        None
    };

    let mut action_stream = match notif_proxy.as_ref() {
        Some(np) => np.receive_action_invoked().await.ok(),
        None => None,
    };
    let mut closed_stream = match notif_proxy.as_ref() {
        Some(np) => np.receive_notification_closed().await.ok(),
        None => None,
    };

    let lid_conn = if std::env::var_os("UCC_LID_BUS_SESSION").is_some() {
        session_conn.clone()
    } else {
        Connection::system().await.ok()
    };
    let login_proxy = match lid_conn.as_ref() {
        Some(lc) => LoginManagerProxy::new(lc).await.ok(),
        None => None,
    };
    let upower_proxy = match lid_conn.as_ref() {
        Some(lc) => UPowerProxy::new(lc).await.ok(),
        None => None,
    };

    let mut lid_closed_stream = match login_proxy.as_ref() {
        Some(lp) => lp.receive_lid_closed_changed().await.into(),
        None => None,
    };
    let mut sleep_stream = match login_proxy.as_ref() {
        Some(lp) => lp.receive_prepare_for_sleep().await.ok(),
        None => None,
    };
    let mut upower_lid_stream = match upower_proxy.as_ref() {
        Some(up) => up.receive_lid_is_closed_changed().await.into(),
        None => None,
    };

    let mut profile_stream = proxy.receive_profile_changed().await.ok();
    let mut policy_stream = proxy.receive_power_policy_changed().await.ok();
    let mut dev_status_stream = proxy.receive_power_device_status_changed().await.ok();

    let backend = RefreshBackend::detect_from_env();
    let mut driver_alert = DriverAlertState::default();
    let mut power_alert = PowerSourceAlertState::default();
    let mut lid_handler = LidDisplayHandler::new(backend);
    let mut parser = NotificationMonitorParser::new(own_bus_name);

    let mut state = SessionState {
        backend,
        drm_root: drm_root_from_env(),
        policy_source: "ac".to_string(),
        requested_hz: 0,
        pending_refresh: None,
        last_notified_refresh: None,
    };

    // Best-effort initial state hydration (minimal mocks may not implement every method).
    if let Ok(policy_raw) = proxy.get_power_policy_json().await {
        power_alert.on_policy_json(&policy_raw);
        lid_handler.policy_changed(&policy_raw);
        state.policy_source = parse_policy_source(&policy_raw);
    }
    if let Ok(dev_settings_raw) = proxy.get_power_device_settings_json().await {
        lid_handler.device_settings_changed(&dev_settings_raw);
    }
    if let Ok(prof_raw) = proxy.get_active_profile_json().await {
        state.requested_hz = parse_requested_hz(&prof_raw);
    }
    let mut login1_active = false;
    if let Some(ref lp) = login_proxy {
        if let Ok(closed) = lp.lid_closed().await {
            login1_active = true;
            let _ = lid_handler.on_lid_state_changed(closed, &[]);
        }
    }
    if !login1_active {
        if let Some(ref up) = upower_proxy {
            if let Ok(closed) = up.lid_is_closed().await {
                let _ = lid_handler.on_lid_state_changed(closed, &[]);
            }
        }
    }
    state.maybe_suggest_refresh(notif_proxy.as_ref()).await;

    let health_poll_ms = std::env::var("UCC_HEALTH_POLL_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&ms| ms >= 50)
        .unwrap_or(10_000);
    let mut health_tick = tokio::time::interval(Duration::from_millis(health_poll_ms));
    let mut state_tick = tokio::time::interval(Duration::from_secs(5));
    let mut power_notice_deadline: Option<tokio::time::Instant> = None;

    let spawn_monitor = || {
        let mut child = Command::new("dbus-monitor")
            .args([
                "--session",
                "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        Some((child, stdout))
    };

    let mut monitor_state = spawn_monitor();
    let mut restart_at: Option<tokio::time::Instant> = None;
    let mut buf = [0u8; 4096];

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down ucc-session-agent");
                break;
            }
            _ = health_tick.tick() => {
                if let Ok(raw) = proxy.get_driver_health_json().await {
                    if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                        let (gen, action) = driver_alert.evaluate(&val);
                        match action {
                            DriverAlertAction::Notify(msg) => {
                                warn!("UCC driver health alert: {msg}");
                                let id = send_notify(
                                    notif_proxy.as_ref(),
                                    "dialog-warning",
                                    "UCC Driver Alert",
                                    &msg,
                                    &[],
                                    0,
                                )
                                .await;
                                if let Some(stale) = driver_alert.on_notify_finished(gen, id) {
                                    close_notify(notif_proxy.as_ref(), stale).await;
                                }
                            }
                            DriverAlertAction::Replace { close_id, problem } => {
                                warn!("UCC driver health alert: {problem}");
                                close_notify(notif_proxy.as_ref(), close_id).await;
                                let id = send_notify(
                                    notif_proxy.as_ref(),
                                    "dialog-warning",
                                    "UCC Driver Alert",
                                    &problem,
                                    &[],
                                    0,
                                )
                                .await;
                                if let Some(stale) = driver_alert.on_notify_finished(gen, id) {
                                    close_notify(notif_proxy.as_ref(), stale).await;
                                }
                            }
                            DriverAlertAction::Close(id) => {
                                close_notify(notif_proxy.as_ref(), id).await;
                            }
                            DriverAlertAction::None => {}
                        }
                    }
                }
            }
            sig = next_optional(&mut policy_stream) => {
                if let Ok(args) = sig.args() {
                    let policy_raw = args.json;
                    if power_alert.on_policy_json(&policy_raw) {
                        power_notice_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(800));
                    }
                    lid_handler.policy_changed(&policy_raw);
                    state.policy_source = parse_policy_source(&policy_raw);
                    state.maybe_suggest_refresh(notif_proxy.as_ref()).await;
                }
            }
            _ = state_tick.tick() => {
                if let Ok(policy_raw) = proxy.get_power_policy_json().await {
                    if power_alert.on_policy_json(&policy_raw) {
                        power_notice_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(800));
                    }
                    lid_handler.policy_changed(&policy_raw);
                    state.policy_source = parse_policy_source(&policy_raw);
                }
            }
            _ = async {
                match power_notice_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                power_notice_deadline = None;
                if let Some(notice) = power_alert.take_pending_notice(ucc_config::is_read_only_preview()) {
                    info!("Power source transitioned: {}", notice.title);
                    let _ = send_notify(
                        notif_proxy.as_ref(),
                        notice.icon,
                        notice.title,
                        notice.body,
                        &[],
                        5000,
                    )
                    .await;
                }
            }
            _ = next_optional(&mut profile_stream) => {
                if let Ok(prof_raw) = proxy.get_active_profile_json().await {
                    state.requested_hz = parse_requested_hz(&prof_raw);
                }
                state.maybe_suggest_refresh(notif_proxy.as_ref()).await;
            }
            _ = next_optional(&mut dev_status_stream) => {
                if let Ok(settings_raw) = proxy.get_power_device_settings_json().await {
                    lid_handler.device_settings_changed(&settings_raw);
                }
            }
            sig = next_optional(&mut action_stream) => {
                if let Ok(args) = sig.args() {
                    if let Some((pending_id, ref plan)) = state.pending_refresh {
                        if args.id == pending_id && args.action_key == "apply" {
                            if let Some(cmd) = state.backend.command_name() {
                                let cmd_args = RefreshAdvisor::apply_command_args(state.backend, plan);
                                run_commands(vec![(cmd, cmd_args)]).await;
                            }
                            state.pending_refresh = None;
                        }
                    }
                }
            }
            sig = next_optional(&mut closed_stream) => {
                if let Ok(args) = sig.args() {
                    if state.pending_refresh.as_ref().is_some_and(|(id, _)| *id == args.id) {
                        state.pending_refresh = None;
                    }
                }
            }
            sig = next_optional(&mut sleep_stream) => {
                if let Ok(args) = sig.args() {
                    if !args.start {
                        state.maybe_suggest_refresh(notif_proxy.as_ref()).await;
                    }
                }
            }
            prop = next_optional(&mut lid_closed_stream) => {
                if let Ok(closed) = prop.get().await {
                    login1_active = true;
                    let cfg = query_compositor_config(state.backend).await;
                    let monitors = internal_monitors_from_config(cfg.as_ref());
                    let cmds = lid_handler.on_lid_state_changed(closed, &monitors);
                    run_commands(cmds).await;
                }
            }
            prop = next_optional(&mut upower_lid_stream) => {
                if !login1_active {
                    if let Ok(closed) = prop.get().await {
                        let cfg = query_compositor_config(state.backend).await;
                        let monitors = internal_monitors_from_config(cfg.as_ref());
                        let cmds = lid_handler.on_lid_state_changed(closed, &monitors);
                        run_commands(cmds).await;
                    }
                }
            }
            _ = async {
                match restart_at {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                restart_at = None;
                parser.reset();
                monitor_state = spawn_monitor();
                if monitor_state.is_none() {
                    restart_at = Some(tokio::time::Instant::now() + Duration::from_secs(5));
                }
            }
            read_res = async {
                match monitor_state.as_mut() {
                    Some((_, out)) => out.read(&mut buf).await,
                    None => std::future::pending().await,
                }
            } => {
                match read_res {
                    Ok(0) | Err(_) => {
                        monitor_state = None;
                        restart_at = Some(tokio::time::Instant::now() + Duration::from_secs(5));
                    }
                    Ok(n) => {
                        if parser.feed_bytes(&buf[..n], Instant::now()) > 0 {
                            let settings = proxy
                                .get_lightbar_settings_json()
                                .await
                                .await_value_or_null();
                            let policy = proxy
                                .get_power_policy_json()
                                .await
                                .await_value_or_null();
                            if lightbar_notify_enabled(&settings, &policy) {
                                let _ = proxy.trigger_lightbar_animation(false).await;
                            }
                        }
                    }
                }
            }
        }
    }
}

trait JsonResultExt {
    fn await_value_or_null(self) -> Value;
}

impl JsonResultExt for zbus::Result<String> {
    fn await_value_or_null(self) -> Value {
        self.ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or(Value::Null)
    }
}
