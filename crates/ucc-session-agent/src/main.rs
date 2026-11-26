// SPDX-License-Identifier: GPL-3.0-or-later

//! Session background agent relaying notifications to lightbar, monitoring driver health,
//! alerting on power transitions, and handling lid/refresh events.

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
    DriverAlertAction, DriverAlertState, NotificationMonitorParser, PowerSourceAlertState,
    RefreshBackend,
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
    let managed = settings.get("managed").and_then(|v| v.as_bool()).unwrap_or(false);
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

    let _backend = RefreshBackend::detect_from_env();
    let mut driver_alert = DriverAlertState::default();
    let mut power_alert = PowerSourceAlertState::default();
    let mut parser = NotificationMonitorParser::new(None);
    let mut health_tick = tokio::time::interval(Duration::from_secs(10));
    let mut state_tick = tokio::time::interval(Duration::from_secs(5));

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
                        let (_gen, action) = driver_alert.evaluate(&val);
                        match action {
                            DriverAlertAction::Notify(msg) | DriverAlertAction::Replace { problem: msg, .. } => {
                                warn!("UCC driver health alert: {msg}");
                            }
                            DriverAlertAction::Close(_) | DriverAlertAction::None => {}
                        }
                    }
                }
            }
            _ = state_tick.tick() => {
                if let Ok(policy_raw) = proxy.get_power_policy_json().await {
                    if power_alert.on_policy_json(&policy_raw) {
                        if let Some(notice) = power_alert.take_pending_notice(ucc_config::is_read_only_preview()) {
                            info!("Power source transitioned: {}", notice.title);
                        }
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
                                .ok()
                                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                                .unwrap_or(Value::Null);
                            let policy = proxy
                                .get_power_policy_json()
                                .await
                                .ok()
                                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                                .unwrap_or(Value::Null);
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
