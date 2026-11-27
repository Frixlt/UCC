// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop system tray (`StatusNotifierItem`) binary for Frixit Control Center.

use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};
use ucc_dbus::{UccdProxy, DBUS_INTERFACE, DBUS_PATH, DBUS_SERVICE};
use ucc_desktop::{
    activate_or_launch_gui, spawn_tray_bounded, update_active_profile, update_daemon_connected,
    update_fan_auto, update_profiles, update_water_cooler_connected, TrayAction,
};
use zbus::Connection;

const TRAY_DBUS_SERVICE: &str = "com.uniwill.UccDesktopTray";
const TRAY_PREVIEW_DBUS_SERVICE: &str = "com.uniwill.UccDesktopTrayPreview";
const ACTION_CHANNEL_CAPACITY: usize = 32;

fn is_running_as_root() -> bool {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                let mut fields = rest.split_whitespace();
                let real = fields.next().and_then(|s| s.parse::<u32>().ok());
                let effective = fields.next().and_then(|s| s.parse::<u32>().ok());
                if real == Some(0) || effective == Some(0) {
                    return true;
                }
            }
        }
    }
    false
}

#[derive(Debug, Default, Clone)]
struct DaemonSnapshot {
    profile_ids: Vec<String>,
    profile_names: Vec<String>,
    active_idx: usize,
    wc_connected: Option<bool>,
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
        .map_err(|e| format!("invalid D-Bus service name: {e}"))?
        .path(DBUS_PATH)
        .map_err(|e| format!("invalid D-Bus path: {e}"))?
        .interface(DBUS_INTERFACE)
        .map_err(|e| format!("invalid D-Bus interface: {e}"))?
        .build()
        .await
        .map_err(|e| format!("failed to build uccd D-Bus proxy: {e}"))?;

    Ok((conn, proxy))
}

fn parse_profiles_json(raw: &str, out: &mut Vec<(String, String)>) {
    let Ok(val) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    let Some(arr) = val.as_array() else {
        return;
    };
    for item in arr {
        let Some(id) = item
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(id);
        if !out.iter().any(|(existing_id, _)| existing_id == id) {
            out.push((id.to_string(), name.to_string()));
        }
    }
}

async fn fetch_snapshot(proxy: &UccdProxy<'_>) -> Result<DaemonSnapshot, String> {
    let mut any_ok = false;
    let mut list = Vec::new();
    if let Ok(raw) = proxy.get_default_profiles_json().await {
        any_ok = true;
        parse_profiles_json(&raw, &mut list);
    }
    if let Ok(raw) = proxy.get_custom_profiles_json().await {
        any_ok = true;
        parse_profiles_json(&raw, &mut list);
    }
    if list.is_empty() {
        if let Ok(raw) = proxy.get_profiles_json().await {
            any_ok = true;
            parse_profiles_json(&raw, &mut list);
        }
    }

    let active_id = match proxy.get_active_profile_json().await {
        Ok(s) => {
            any_ok = true;
            serde_json::from_str::<Value>(&s)
                .ok()
                .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        }
        Err(_) => None,
    };

    let active_idx = active_id
        .as_deref()
        .and_then(|id| list.iter().position(|(pid, _)| pid == id))
        .unwrap_or(0);

    let wc_connected = match proxy.get_water_cooler_connected().await {
        Ok(connected) => {
            any_ok = true;
            Some(connected)
        }
        Err(_) => None,
    };

    if !any_ok {
        return Err("uccd D-Bus service unavailable".to_string());
    }

    let (profile_ids, profile_names) = list.into_iter().unzip();
    Ok(DaemonSnapshot {
        profile_ids,
        profile_names,
        active_idx,
        wc_connected,
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("ucc-desktop-tray {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "ucc-desktop-tray {} — StatusNotifierItem system tray for UCC",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }

    if is_running_as_root() {
        eprintln!("Error: ucc-desktop-tray refuses to run as root");
        std::process::exit(1);
    }

    tracing_subscriber::fmt::init();

    let preview = args.iter().any(|a| a == "--preview")
        || std::env::var("UCC_PREVIEW").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let singleton_name = if preview {
        TRAY_PREVIEW_DBUS_SERVICE
    } else {
        TRAY_DBUS_SERVICE
    };

    let singleton_conn = match Connection::session().await {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Error: failed to connect to session D-Bus: {err}");
            std::process::exit(2);
        }
    };
    if let Ok(dbus_proxy) = zbus::fdo::DBusProxy::new(&singleton_conn).await {
        let bus_name = zbus::names::WellKnownName::from_static_str_unchecked(singleton_name);
        match dbus_proxy
            .request_name(bus_name, zbus::fdo::RequestNameFlags::DoNotQueue.into())
            .await
        {
            Ok(
                zbus::fdo::RequestNameReply::PrimaryOwner
                | zbus::fdo::RequestNameReply::AlreadyOwner,
            ) => {}
            Ok(zbus::fdo::RequestNameReply::Exists | zbus::fdo::RequestNameReply::InQueue) => {
                info!("{singleton_name} is already owned on session D-Bus; exiting");
                return;
            }
            Err(err) => {
                warn!("Could not claim singleton name {singleton_name}: {err}");
            }
        }
    }

    let mut conn_pair = connect_uccd().await.ok();
    let (mut connected, mut state) = match conn_pair.as_ref() {
        Some((_, proxy)) => match fetch_snapshot(proxy).await {
            Ok(snap) => (true, snap),
            Err(err) => {
                warn!("Initial uccd snapshot unavailable: {err}");
                (false, DaemonSnapshot::default())
            }
        },
        None => (false, DaemonSnapshot::default()),
    };

    let initial_names = if state.profile_names.is_empty() {
        vec!["Default".to_string()]
    } else {
        state.profile_names.clone()
    };

    let (tx, mut rx) = mpsc::channel(ACTION_CHANNEL_CAPACITY);
    let handle = match spawn_tray_bounded(initial_names, state.active_idx, tx).await {
        Ok(h) => h,
        Err(err) => {
            eprintln!("Error: failed to register StatusNotifierItem tray: {err}");
            std::process::exit(1);
        }
    };
    update_daemon_connected(&handle, connected).await;
    if let Some(wc) = state.wc_connected {
        update_water_cooler_connected(&handle, wc).await;
    }
    info!(
        "ucc-desktop-tray registered StatusNotifierItem (preview={preview}, connected={connected})"
    );

    let mut refresh = tokio::time::interval(Duration::from_secs(5));
    let mut fan_auto = true;
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received SIGINT, closing tray");
                break;
            }
            _ = async {
                match sigterm.as_mut() {
                    Some(s) => s.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                info!("Received SIGTERM, closing tray");
                break;
            }
            _ = refresh.tick() => {
                if conn_pair.is_none() {
                    conn_pair = connect_uccd().await.ok();
                }
                if let Some((_, ref proxy)) = conn_pair {
                    match fetch_snapshot(proxy).await {
                        Ok(next) => {
                            if !connected {
                                connected = true;
                                update_daemon_connected(&handle, true).await;
                            }
                            if next.profile_names != state.profile_names && !next.profile_names.is_empty() {
                                update_profiles(&handle, next.profile_names.clone(), next.active_idx).await;
                            } else if next.active_idx != state.active_idx {
                                update_active_profile(&handle, next.active_idx).await;
                            }
                            if let Some(wc) = next.wc_connected {
                                if state.wc_connected != Some(wc) {
                                    update_water_cooler_connected(&handle, wc).await;
                                }
                            }
                            state = next;
                        }
                        Err(_) => {
                            if connected {
                                connected = false;
                                update_daemon_connected(&handle, false).await;
                                warn!("Lost connection to uccd; waiting to reconnect");
                            }
                            conn_pair = None;
                        }
                    }
                } else if connected {
                    connected = false;
                    update_daemon_connected(&handle, false).await;
                }
            }
            Some(action) = rx.recv() => {
                match action {
                    TrayAction::OpenGui(tray_token) => {
                        let env_token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
                        let token = tray_token.or(env_token);
                        if let Err(err) = activate_or_launch_gui(token.as_deref(), preview).await {
                            warn!("Failed to activate or launch ucc-gui: {err}");
                        }
                    }
                    TrayAction::SelectProfile(idx) => {
                        if preview {
                            warn!("Ignoring SelectProfile in preview mode");
                            continue;
                        }
                        let Some((_, ref proxy)) = conn_pair else {
                            warn!("Ignoring SelectProfile while uccd is disconnected");
                            continue;
                        };
                        if let Some(id) = state.profile_ids.get(idx) {
                            if proxy.set_active_profile(id).await.unwrap_or(false) {
                                state.active_idx = idx;
                                update_active_profile(&handle, idx).await;
                            } else {
                                warn!("Daemon rejected profile switch to {id}");
                            }
                        }
                    }
                    TrayAction::ToggleFanAuto => {
                        if preview {
                            warn!("Ignoring ToggleFanAuto in preview mode");
                            continue;
                        }
                        let Some((_, ref proxy)) = conn_pair else {
                            warn!("Ignoring ToggleFanAuto while uccd is disconnected");
                            continue;
                        };
                        let ok = if fan_auto {
                            proxy.apply_fan_profiles("{}").await.unwrap_or(false)
                        } else {
                            proxy.revert_fan_profiles().await.unwrap_or(false)
                        };
                        if ok {
                            fan_auto = !fan_auto;
                            update_fan_auto(&handle, fan_auto).await;
                        }
                    }
                    TrayAction::ToggleWaterCooler => {
                        if preview {
                            warn!("Ignoring ToggleWaterCooler in preview mode");
                            continue;
                        }
                        let Some((_, ref proxy)) = conn_pair else {
                            warn!("Ignoring ToggleWaterCooler while uccd is disconnected");
                            continue;
                        };
                        let target = !state.wc_connected.unwrap_or(false);
                        if proxy.enable_water_cooler(target).await.unwrap_or(false) {
                            match proxy.get_water_cooler_connected().await {
                                Ok(confirmed) => {
                                    state.wc_connected = Some(confirmed);
                                    update_water_cooler_connected(&handle, confirmed).await;
                                }
                                Err(err) => {
                                    warn!("Water cooler state readback failed after toggle: {err}");
                                }
                            }
                        }
                    }
                    TrayAction::Quit => {
                        info!("Quit requested from tray menu");
                        break;
                    }
                }
            }
        }
    }

    handle.shutdown().await;
}
