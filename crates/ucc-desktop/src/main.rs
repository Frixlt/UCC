// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop system tray (`StatusNotifierItem`) binary for Frixit Control Center.

use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};
use ucc_dbus::{UccdProxy, DBUS_INTERFACE, DBUS_PATH, DBUS_SERVICE};
use ucc_desktop::{
    launch_gui, spawn_tray, update_active_profile, update_profiles, update_water_cooler_connected,
    TrayAction,
};
use zbus::Connection;

#[derive(Debug, Default, Clone)]
struct DaemonSnapshot {
    profile_ids: Vec<String>,
    profile_names: Vec<String>,
    active_idx: usize,
    wc_connected: bool,
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
        let Some(id) = item.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
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

async fn fetch_snapshot(proxy: &UccdProxy<'_>) -> DaemonSnapshot {
    let mut list = Vec::new();
    if let Ok(raw) = proxy.get_default_profiles_json().await {
        parse_profiles_json(&raw, &mut list);
    }
    if let Ok(raw) = proxy.get_custom_profiles_json().await {
        parse_profiles_json(&raw, &mut list);
    }
    if list.is_empty() {
        if let Ok(raw) = proxy.get_profiles_json().await {
            parse_profiles_json(&raw, &mut list);
        }
    }

    let active_id = proxy
        .get_active_profile_json()
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string));

    let active_idx = active_id
        .as_deref()
        .and_then(|id| list.iter().position(|(pid, _)| pid == id))
        .unwrap_or(0);

    let wc_connected = proxy.get_water_cooler_connected().await.unwrap_or(false);

    let (profile_ids, profile_names) = list.into_iter().unzip();
    DaemonSnapshot {
        profile_ids,
        profile_names,
        active_idx,
        wc_connected,
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("ucc-desktop-tray {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("ucc-desktop-tray {} — StatusNotifierItem system tray for UCC", env!("CARGO_PKG_VERSION"));
        return;
    }

    tracing_subscriber::fmt::init();

    let (_conn, proxy) = match connect_uccd().await {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(2);
        }
    };

    let mut state = fetch_snapshot(&proxy).await;
    let initial_names = if state.profile_names.is_empty() {
        vec!["Default".to_string()]
    } else {
        state.profile_names.clone()
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = match spawn_tray(initial_names, state.active_idx, tx).await {
        Ok(h) => h,
        Err(err) => {
            eprintln!("Error: failed to register StatusNotifierItem tray: {err}");
            std::process::exit(1);
        }
    };
    update_water_cooler_connected(&handle, state.wc_connected).await;
    info!("ucc-desktop-tray registered StatusNotifierItem");

    let mut refresh = tokio::time::interval(Duration::from_secs(5));
    let mut fan_auto = true;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received shutdown signal, closing tray");
                break;
            }
            _ = refresh.tick() => {
                let next = fetch_snapshot(&proxy).await;
                if next.profile_names != state.profile_names && !next.profile_names.is_empty() {
                    update_profiles(&handle, next.profile_names.clone(), next.active_idx).await;
                } else if next.active_idx != state.active_idx {
                    update_active_profile(&handle, next.active_idx).await;
                }
                if next.wc_connected != state.wc_connected {
                    update_water_cooler_connected(&handle, next.wc_connected).await;
                }
                state = next;
            }
            Some(action) = rx.recv() => {
                match action {
                    TrayAction::OpenGui => {
                        let token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
                        if let Err(err) = launch_gui(token.as_deref()) {
                            warn!("Failed to launch ucc-gui: {err}");
                        }
                    }
                    TrayAction::SelectProfile(idx) => {
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
                        let ok = if fan_auto {
                            proxy.apply_fan_profiles("{}").await.unwrap_or(false)
                        } else {
                            proxy.revert_fan_profiles().await.unwrap_or(false)
                        };
                        if ok {
                            fan_auto = !fan_auto;
                            ucc_desktop::update_fan_auto(&handle, fan_auto).await;
                        }
                    }
                    TrayAction::ToggleWaterCooler => {
                        let target = !state.wc_connected;
                        if proxy.enable_water_cooler(target).await.unwrap_or(false) {
                            state.wc_connected = proxy.get_water_cooler_connected().await.unwrap_or(target);
                            update_water_cooler_connected(&handle, state.wc_connected).await;
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
