// SPDX-License-Identifier: GPL-3.0-or-later

//! D-Bus proxies and helper functions for desktop notifications, compositor
//! refresh-rate planning, and login1/UPower lid display management.

use serde_json::Value;
use std::collections::HashMap;
use std::future::poll_fn;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use tokio::process::Command;
use tracing::warn;
use ucc_session_agent::refresh_advisor::hyprland_configuration;
use ucc_session_agent::{RefreshBackend, RefreshPlan};
use zbus::export::futures_core::Stream;
use zbus::zvariant::Value as ZValue;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, ZValue<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
pub trait LoginManager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;

    #[zbus(property)]
    fn lid_closed(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
pub trait UPower {
    #[zbus(property)]
    fn lid_is_closed(&self) -> zbus::Result<bool>;
}

pub async fn next_optional<S: Stream + Unpin>(stream: &mut Option<S>) -> S::Item {
    loop {
        if let Some(ref mut inner) = stream {
            if let Some(item) = poll_fn(|cx| Pin::new(&mut *inner).poll_next(cx)).await {
                return item;
            }
            *stream = None;
        }
        std::future::pending::<()>().await;
    }
}

pub async fn send_notify(
    proxy: Option<&NotificationsProxy<'_>>,
    app_icon: &str,
    summary: &str,
    body: &str,
    actions: &[&str],
    expire_timeout: i32,
) -> Option<u32> {
    let proxy = proxy?;
    match proxy
        .notify(
            "ucc-rusted",
            0,
            app_icon,
            summary,
            body,
            actions,
            HashMap::new(),
            expire_timeout,
        )
        .await
    {
        Ok(id) => Some(id),
        Err(err) => {
            warn!("Failed to send org.freedesktop.Notifications.Notify: {err}");
            None
        }
    }
}

pub async fn close_notify(proxy: Option<&NotificationsProxy<'_>>, id: u32) {
    if id == 0 {
        return;
    }
    if let Some(proxy) = proxy {
        if let Err(err) = proxy.close_notification(id).await {
            warn!("Failed to close desktop notification {id}: {err}");
        }
    }
}

fn compositor_cmd_timeout() -> std::time::Duration {
    let ms = std::env::var("UCC_COMPOSITOR_CMD_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|ms| ms.max(50))
        .unwrap_or(1500);
    std::time::Duration::from_millis(ms)
}

async fn run_compositor_command(
    prog: &str,
    args: &[&str],
    capture_stdout: bool,
) -> std::io::Result<std::process::Output> {
    use std::os::unix::process::ExitStatusExt;
    use tokio::io::AsyncReadExt;

    let mut cmd = Command::new(prog);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = cmd.spawn()?;
    let stdout_task = child.stdout.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf).await;
            buf
        })
    });

    match tokio::time::timeout(compositor_cmd_timeout(), child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = if let Some(task) = stdout_task {
                tokio::time::timeout(std::time::Duration::from_millis(200), task)
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            Ok(std::process::Output {
                status,
                stdout,
                stderr: Vec::new(),
            })
        }
        Ok(Err(err)) => {
            if let Some(task) = stdout_task {
                task.abort();
            }
            Err(err)
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(task) = stdout_task {
                task.abort();
            }
            warn!(
                "Compositor helper `{prog} {:?}` timed out after {:?} and was killed",
                args,
                compositor_cmd_timeout()
            );
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(9),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }
}

pub async fn query_compositor_config(backend: RefreshBackend) -> Option<Value> {
    let (cmd, args): (&str, &[&str]) = match backend {
        RefreshBackend::Hyprland => ("hyprctl", &["-j", "monitors"]),
        RefreshBackend::Kde => ("kscreen-doctor", &["-j"]),
        RefreshBackend::Unsupported => return None,
    };
    let output = run_compositor_command(cmd, args, true).await.ok()?;
    if !output.status.success() {
        return None;
    }
    match backend {
        RefreshBackend::Hyprland => hyprland_configuration(&output.stdout),
        RefreshBackend::Kde => serde_json::from_slice::<Value>(&output.stdout).ok(),
        RefreshBackend::Unsupported => None,
    }
}

pub fn internal_monitors_from_config(cfg: Option<&Value>) -> Vec<String> {
    let mut monitors = Vec::new();
    if let Some(outputs) = cfg
        .and_then(|v| v.get("outputs"))
        .and_then(|v| v.as_array())
    {
        for out in outputs {
            if !out
                .get("connected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            if let Some(name) = out.get("name").and_then(|v| v.as_str()) {
                let upper = name.to_ascii_uppercase();
                if upper.starts_with("EDP") || upper.starts_with("LVDS") || upper.starts_with("DSI")
                {
                    monitors.push(name.to_string());
                }
            }
        }
    }
    if monitors.is_empty() {
        monitors.push("eDP-1".to_string());
    }
    monitors
}

pub fn parse_policy_source(policy_json: &str) -> String {
    let Ok(val) = serde_json::from_str::<Value>(policy_json) else {
        return "ac".to_string();
    };
    val.get("effective_source")
        .or_else(|| val.get("real_power_source"))
        .or_else(|| val.get("current_mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("ac")
        .to_string()
}

pub fn parse_requested_hz(profile_json: &str) -> i32 {
    let Ok(val) = serde_json::from_str::<Value>(profile_json) else {
        return 0;
    };
    let Some(disp) = val.get("display") else {
        return 0;
    };
    let use_ref = disp
        .get("useRefRate")
        .or_else(|| disp.get("useRefreshRate"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !use_ref {
        return 0;
    }
    disp.get("refreshRate")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32
}

pub fn format_refresh_prompt(plan: &RefreshPlan) -> String {
    format!(
        "Switch {} from {:.0} Hz to {:.0} Hz?",
        plan.output, plan.current_hz, plan.target_hz
    )
}

pub fn drm_root_from_env() -> std::path::PathBuf {
    std::env::var_os("UCC_DRM_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new("/sys/class/drm").to_path_buf())
}

pub async fn run_commands(cmds: Vec<(&'static str, Vec<String>)>) {
    for (prog, args) in cmds {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match run_compositor_command(prog, &arg_refs, false).await {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                warn!("Command {prog} {:?} exited with {}", args, out.status);
            }
            Err(err) => {
                warn!("Failed to execute {prog} {:?}: {err}", args);
            }
        }
    }
}
