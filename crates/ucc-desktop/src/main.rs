// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop system tray (`StatusNotifierItem`) binary for Frixit Control Center.

use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};
use ucc_dbus::{UccdProxy, DBUS_INTERFACE, DBUS_PATH, DBUS_SERVICE};
use ucc_desktop::{
    activate_or_launch_gui, spawn_tray_bounded, update_active_profile, update_daemon_connected,
    update_fan_auto, update_profiles, update_water_cooler_state, TrayAction,
};
use zbus::Connection;

const TRAY_DBUS_SERVICE: &str = "com.uniwill.UccDesktopTray";
const TRAY_PREVIEW_DBUS_SERVICE: &str = "com.uniwill.UccDesktopTrayPreview";
const ACTION_CHANNEL_CAPACITY: usize = 32;
const MAX_FAN_CURVES_JSON: &str = r#"{"cpu":[{"temp":20,"speed":100},{"temp":100,"speed":100}],"gpu":[{"temp":20,"speed":100},{"temp":100,"speed":100}],"waterCoolerFan":[{"temp":20,"speed":100},{"temp":100,"speed":100}]}"#;

fn rpc_timeout_duration() -> Duration {
    if let Ok(raw) = std::env::var("UCC_DBUS_TIMEOUT_MS") {
        if let Ok(ms) = raw.trim().parse::<u64>() {
            return Duration::from_millis(ms.max(50));
        }
    }
    Duration::from_secs(3)
}

fn install_shutdown_watch() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received SIGINT, closing tray");
            }
            _ = async {
                match sigterm.as_mut() {
                    Some(s) => s.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                info!("Received SIGTERM, closing tray");
            }
        }
        let _ = tx.send(true);
    });
    rx
}

async fn run_rpc_or_shutdown<F, T>(
    shutdown_rx: &tokio::sync::watch::Receiver<bool>,
    fut: F,
) -> Option<Result<T, String>>
where
    F: std::future::Future<Output = Result<T, zbus::Error>>,
{
    if *shutdown_rx.borrow() {
        return None;
    }
    let mut stop_rx = shutdown_rx.clone();
    tokio::select! {
        biased;
        _ = stop_rx.wait_for(|&stop| stop) => None,
        res = tokio::time::timeout(rpc_timeout_duration(), fut) => match res {
            Ok(Ok(v)) => Some(Ok(v)),
            Ok(Err(e)) => Some(Err(format!("D-Bus error: {e}"))),
            Err(_) => Some(Err("D-Bus RPC timed out".to_string())),
        }
    }
}

fn next_wc_toggle_target(state: &DaemonSnapshot) -> bool {
    !state.wc_enabled.unwrap_or(false)
}

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
    wc_enabled: Option<bool>,
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

fn parse_bound_profile_ids(raw: &str) -> Vec<String> {
    let Ok(val) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let obj = val
        .get("stateMap")
        .and_then(|v| v.as_object())
        .or_else(|| val.as_object());
    let Some(obj) = obj else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for key in ["power_ac", "power_type_c", "power_bat", "power_wc"] {
        if let Some(id) = obj
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

fn filter_profiles_to_bound(
    all: Vec<(String, String)>,
    bound_ids: &[String],
    active_id: Option<&str>,
) -> Vec<(String, String)> {
    if bound_ids.is_empty() {
        return all;
    }
    let filtered: Vec<(String, String)> = all
        .iter()
        .filter(|(id, _)| bound_ids.iter().any(|b| b == id) || active_id.is_some_and(|a| a == id))
        .cloned()
        .collect();
    if filtered.is_empty() {
        all
    } else {
        filtered
    }
}

async fn fetch_snapshot(
    proxy: &UccdProxy<'_>,
    shutdown_rx: &tokio::sync::watch::Receiver<bool>,
) -> Result<DaemonSnapshot, String> {
    let mut any_ok = false;
    let mut list = Vec::new();
    match run_rpc_or_shutdown(shutdown_rx, proxy.get_default_profiles_json()).await {
        None => return Err("shutdown requested".to_string()),
        Some(Ok(raw)) => {
            any_ok = true;
            parse_profiles_json(&raw, &mut list);
        }
        Some(Err(_)) => {}
    }
    match run_rpc_or_shutdown(shutdown_rx, proxy.get_custom_profiles_json()).await {
        None => return Err("shutdown requested".to_string()),
        Some(Ok(raw)) => {
            any_ok = true;
            parse_profiles_json(&raw, &mut list);
        }
        Some(Err(_)) => {}
    }
    if list.is_empty() {
        match run_rpc_or_shutdown(shutdown_rx, proxy.get_profiles_json()).await {
            None => return Err("shutdown requested".to_string()),
            Some(Ok(raw)) => {
                any_ok = true;
                parse_profiles_json(&raw, &mut list);
            }
            Some(Err(_)) => {}
        }
    }

    let bound_ids = match run_rpc_or_shutdown(shutdown_rx, proxy.get_settings_json()).await {
        None => return Err("shutdown requested".to_string()),
        Some(Ok(raw)) => {
            any_ok = true;
            parse_bound_profile_ids(&raw)
        }
        Some(Err(_)) => Vec::new(),
    };

    let active_id = match run_rpc_or_shutdown(shutdown_rx, proxy.get_active_profile_json()).await {
        None => return Err("shutdown requested".to_string()),
        Some(Ok(s)) => {
            any_ok = true;
            serde_json::from_str::<Value>(&s)
                .ok()
                .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        }
        Some(Err(_)) => None,
    };

    let list = filter_profiles_to_bound(list, &bound_ids, active_id.as_deref());

    let active_idx = active_id
        .as_deref()
        .and_then(|id| list.iter().position(|(pid, _)| pid == id))
        .unwrap_or(0);

    let wc_enabled = match run_rpc_or_shutdown(shutdown_rx, proxy.is_water_cooler_enabled()).await {
        None => return Err("shutdown requested".to_string()),
        Some(Ok(enabled)) => {
            any_ok = true;
            Some(enabled)
        }
        Some(Err(_)) => None,
    };

    let wc_connected =
        match run_rpc_or_shutdown(shutdown_rx, proxy.get_water_cooler_connected()).await {
            None => return Err("shutdown requested".to_string()),
            Some(Ok(connected)) => {
                any_ok = true;
                Some(connected)
            }
            Some(Err(_)) => None,
        };

    if !any_ok {
        return Err("uccd D-Bus service unavailable".to_string());
    }

    let (profile_ids, profile_names) = list.into_iter().unzip();
    Ok(DaemonSnapshot {
        profile_ids,
        profile_names,
        active_idx,
        wc_enabled,
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

    let shutdown_rx = install_shutdown_watch();

    let mut conn_rx = shutdown_rx.clone();
    let mut conn_pair = tokio::select! {
        biased;
        _ = conn_rx.wait_for(|&stop| stop) => return,
        res = tokio::time::timeout(rpc_timeout_duration(), connect_uccd()) => {
            res.ok().and_then(Result::ok)
        }
    };
    let (mut connected, mut state) = match conn_pair.as_ref() {
        Some((_, proxy)) => match fetch_snapshot(proxy, &shutdown_rx).await {
            Ok(snap) => (true, snap),
            Err(err) => {
                if *shutdown_rx.borrow() {
                    return;
                }
                warn!("Initial uccd snapshot unavailable: {err}");
                (false, DaemonSnapshot::default())
            }
        },
        None => (false, DaemonSnapshot::default()),
    };
    if *shutdown_rx.borrow() {
        return;
    }

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
    update_water_cooler_state(
        &handle,
        state.wc_enabled.unwrap_or(false),
        state.wc_connected.unwrap_or(false),
    )
    .await;
    info!(
        "ucc-desktop-tray registered StatusNotifierItem (preview={preview}, connected={connected})"
    );

    let mut refresh = tokio::time::interval(Duration::from_secs(5));
    let mut fan_auto = true;

    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        let mut loop_stop_rx = shutdown_rx.clone();
        let mut inner_stop_rx = shutdown_rx.clone();
        tokio::select! {
            biased;
            _ = loop_stop_rx.wait_for(|&stop| stop) => {
                break;
            }
            _ = refresh.tick() => {
                if conn_pair.is_none() {
                    conn_pair = tokio::select! {
                        biased;
                        _ = inner_stop_rx.wait_for(|&stop| stop) => break,
                        res = tokio::time::timeout(rpc_timeout_duration(), connect_uccd()) => {
                            res.ok().and_then(Result::ok)
                        }
                    };
                }
                if let Some((_, ref proxy)) = conn_pair {
                    match fetch_snapshot(proxy, &shutdown_rx).await {
                        Ok(next) => {
                            if !connected {
                                connected = true;
                                update_daemon_connected(&handle, true).await;
                            }
                            if next.profile_names != state.profile_names && !next.profile_names.is_empty() {
                                update_profiles(&handle, next.profile_names.clone(), next.active_idx).await;
                                fan_auto = true;
                                update_fan_auto(&handle, true).await;
                            } else if next.active_idx != state.active_idx {
                                update_active_profile(&handle, next.active_idx).await;
                                fan_auto = true;
                                update_fan_auto(&handle, true).await;
                            }
                            if next.wc_enabled != state.wc_enabled || next.wc_connected != state.wc_connected {
                                update_water_cooler_state(
                                    &handle,
                                    next.wc_enabled.unwrap_or(false),
                                    next.wc_connected.unwrap_or(false),
                                )
                                .await;
                            }
                            state = next;
                        }
                        Err(_) => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
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
            action = rx.recv() => {
                let Some(action) = action else {
                    info!("Tray action channel closed; exiting tray event loop");
                    break;
                };
                match action {
                    TrayAction::OpenGui(tray_token) => {
                        let env_token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
                        let token = tray_token.or(env_token);
                        match tokio::time::timeout(
                            rpc_timeout_duration(),
                            activate_or_launch_gui(token.as_deref(), preview),
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => {
                                warn!("Failed to activate or launch ucc-gui: {err}");
                            }
                            Err(_) => {
                                warn!("Timed out activating or launching ucc-gui");
                            }
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
                            match run_rpc_or_shutdown(&shutdown_rx, proxy.set_active_profile(id)).await {
                                None => break,
                                Some(Ok(true)) => {
                                    state.active_idx = idx;
                                    update_active_profile(&handle, idx).await;
                                    fan_auto = true;
                                    update_fan_auto(&handle, true).await;
                                }
                                Some(Ok(false)) => {
                                    warn!("Daemon rejected profile switch to {id}");
                                }
                                Some(Err(err)) => {
                                    warn!("Failed to switch profile to {id}: {err}");
                                }
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
                        let res = if fan_auto {
                            run_rpc_or_shutdown(
                                &shutdown_rx,
                                proxy.apply_fan_profiles(MAX_FAN_CURVES_JSON),
                            )
                            .await
                        } else {
                            run_rpc_or_shutdown(&shutdown_rx, proxy.revert_fan_profiles()).await
                        };
                        match res {
                            None => break,
                            Some(Ok(true)) => {
                                fan_auto = !fan_auto;
                                update_fan_auto(&handle, fan_auto).await;
                            }
                            Some(Ok(false)) => {
                                warn!("Daemon rejected fan Auto/Max toggle");
                            }
                            Some(Err(err)) => {
                                warn!("Failed to toggle fan Auto/Max: {err}");
                            }
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
                        let target = next_wc_toggle_target(&state);
                        match run_rpc_or_shutdown(&shutdown_rx, proxy.enable_water_cooler(target))
                            .await
                        {
                            None => break,
                            Some(Ok(true)) => {
                                let enabled = match run_rpc_or_shutdown(
                                    &shutdown_rx,
                                    proxy.is_water_cooler_enabled(),
                                )
                                .await
                                {
                                    None => break,
                                    Some(Ok(v)) => v,
                                    Some(Err(err)) => {
                                        warn!("Water cooler enabled readback failed: {err}");
                                        target
                                    }
                                };
                                let wc_conn = match run_rpc_or_shutdown(
                                    &shutdown_rx,
                                    proxy.get_water_cooler_connected(),
                                )
                                .await
                                {
                                    None => break,
                                    Some(Ok(v)) => v,
                                    Some(Err(err)) => {
                                        warn!("Water cooler state readback failed after toggle: {err}");
                                        state.wc_connected.unwrap_or(false)
                                    }
                                };
                                state.wc_enabled = Some(enabled);
                                state.wc_connected = Some(wc_conn);
                                update_water_cooler_state(&handle, enabled, wc_conn).await;
                            }
                            Some(Ok(false)) => {
                                warn!("Daemon rejected EnableWaterCooler({target})");
                            }
                            Some(Err(err)) => {
                                warn!("Failed to toggle EnableWaterCooler({target}): {err}");
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

#[cfg(test)]
mod tests {
    use super::{
        filter_profiles_to_bound, next_wc_toggle_target, parse_bound_profile_ids, DaemonSnapshot,
        MAX_FAN_CURVES_JSON,
    };

    #[test]
    fn filters_tray_profiles_to_bound_state_map_entries_and_active_profile() {
        let bound = parse_bound_profile_ids(
            r#"{"power_ac":"frixit-ac","power_bat":"frixit-bat","power_type_c":"frixit-type_c","power_wc":""}"#,
        );
        assert_eq!(bound, vec!["frixit-ac", "frixit-type_c", "frixit-bat"]);

        let all = vec![
            ("__quiet__".into(), "Quiet [Built-in]".into()),
            ("__office__".into(), "Office [Built-in]".into()),
            (
                "__default_custom_profile__".into(),
                "TUXEDO Defaults".into(),
            ),
            ("frixit-ac".into(), "max perfomance".into()),
            ("frixit-bat".into(), "powersave".into()),
            ("frixit-type_c".into(), "type c".into()),
        ];

        let filtered = filter_profiles_to_bound(all.clone(), &bound, Some("frixit-ac"));
        assert_eq!(
            filtered,
            vec![
                ("frixit-ac".into(), "max perfomance".into()),
                ("frixit-bat".into(), "powersave".into()),
                ("frixit-type_c".into(), "type c".into()),
            ]
        );

        let fallback = filter_profiles_to_bound(all.clone(), &[], None);
        assert_eq!(fallback, all);
    }

    #[test]
    fn ta_10_max_fan_curves_json_contains_valid_100_percent_curves() {
        let val: serde_json::Value = serde_json::from_str(MAX_FAN_CURVES_JSON).unwrap();
        for key in ["cpu", "gpu", "waterCoolerFan"] {
            let pts = val[key].as_array().expect("fan curve array present");
            assert!(pts.len() >= 2);
            assert!(pts.iter().all(|pt| pt["speed"].as_u64() == Some(100)));
        }
    }

    #[test]
    fn ta_11_wc_toggle_targets_enabled_flag_not_connected_state() {
        let scanning = DaemonSnapshot {
            wc_enabled: Some(true),
            wc_connected: Some(false),
            ..Default::default()
        };
        assert!(
            !next_wc_toggle_target(&scanning),
            "clicking Water Cooler while enabled & searching must toggle enabled -> false"
        );
        let disabled = DaemonSnapshot {
            wc_enabled: Some(false),
            wc_connected: Some(false),
            ..Default::default()
        };
        assert!(next_wc_toggle_target(&disabled));
    }
}
