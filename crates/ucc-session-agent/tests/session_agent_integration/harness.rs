// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(dead_code, unused_imports)]

use serde_json::json;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::Connection;

#[derive(Debug, Clone)]
pub(crate) struct RecordedNotify {
    pub(crate) id: u32,
    pub(crate) app_name: String,
    pub(crate) icon: String,
    pub(crate) summary: String,
    pub(crate) body: String,
    pub(crate) actions: Vec<String>,
}

#[derive(Default)]
pub(crate) struct FakeNotificationsState {
    pub(crate) next_id: AtomicU32,
    pub(crate) notifies: Mutex<Vec<RecordedNotify>>,
    pub(crate) closed: Mutex<Vec<u32>>,
}

pub(crate) struct FakeNotifications(pub(crate) Arc<FakeNotificationsState>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeNotifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        _replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.notifies.lock().unwrap().push(RecordedNotify {
            id,
            app_name: app_name.to_string(),
            icon: app_icon.to_string(),
            summary: summary.to_string(),
            body: body.to_string(),
            actions,
        });
        id
    }

    fn close_notification(&self, id: u32) {
        self.0.closed.lock().unwrap().push(id);
    }

    #[zbus(signal)]
    pub(crate) async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

pub(crate) struct FakeUccdState {
    pub(crate) healthy: AtomicBool,
    pub(crate) policy_json: Mutex<String>,
    pub(crate) profile_json: Mutex<String>,
    pub(crate) dev_settings_json: Mutex<String>,
    pub(crate) profile_reads: std::sync::atomic::AtomicUsize,
}

impl FakeUccdState {
    pub(crate) fn new(policy_source: &str, refresh_rate: i32, use_ref_rate: bool) -> Self {
        Self {
            healthy: AtomicBool::new(true),
            policy_json: Mutex::new(
                json!({
                    "real_power_source": policy_source,
                    "current_mode": policy_source,
                    "effective_source": policy_source
                })
                .to_string(),
            ),
            profile_json: Mutex::new(
                json!({
                    "id": "__test_profile__",
                    "display": {
                        "useRefRate": use_ref_rate,
                        "refreshRate": refresh_rate
                    }
                })
                .to_string(),
            ),
            dev_settings_json: Mutex::new(
                json!({
                    "lid": {
                        "ac": "screen_off",
                        "bat": "screen_off"
                    }
                })
                .to_string(),
            ),
            profile_reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

pub(crate) struct FakeUccd(pub(crate) Arc<FakeUccdState>);

#[zbus::interface(name = "com.uniwill.uccd")]
impl FakeUccd {
    #[zbus(name = "GetDriverHealthJSON")]
    fn get_driver_health_json(&self) -> String {
        if self.0.healthy.load(Ordering::SeqCst) {
            json!({"healthy": true}).to_string()
        } else {
            json!({"healthy": false, "message": "tuxedo_io missing"}).to_string()
        }
    }

    #[zbus(name = "GetPowerPolicyJSON")]
    fn get_power_policy_json(&self) -> String {
        self.0.policy_json.lock().unwrap().clone()
    }

    #[zbus(name = "GetPowerDeviceSettingsJSON")]
    fn get_power_device_settings_json(&self) -> String {
        self.0.dev_settings_json.lock().unwrap().clone()
    }

    #[zbus(name = "GetActiveProfileJSON")]
    fn get_active_profile_json(&self) -> String {
        self.0.profile_reads.fetch_add(1, Ordering::SeqCst);
        self.0.profile_json.lock().unwrap().clone()
    }

    #[zbus(name = "GetLightbarSettingsJSON")]
    fn get_lightbar_settings_json(&self) -> String {
        json!({"managed": false}).to_string()
    }

    #[zbus(signal)]
    pub(crate) async fn power_policy_changed(
        emitter: &SignalEmitter<'_>,
        json: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn profile_changed(
        emitter: &SignalEmitter<'_>,
        profile_id: &str,
        keyboard_profile_id: &str,
        fan_profile_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn power_device_status_changed(
        emitter: &SignalEmitter<'_>,
        json: &str,
    ) -> zbus::Result<()>;
}

pub(crate) struct FakeLogin1 {
    pub(crate) lid_closed: AtomicBool,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl FakeLogin1 {
    #[zbus(property)]
    fn lid_closed(&self) -> bool {
        self.lid_closed.load(Ordering::SeqCst)
    }

    #[zbus(signal)]
    pub(crate) async fn prepare_for_sleep(
        emitter: &SignalEmitter<'_>,
        start: bool,
    ) -> zbus::Result<()>;
}

pub(crate) async fn spawn_isolated_dbus() -> (tempfile::TempDir, String, tokio::process::Child) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("bus");
    let cfg = dir.path().join("session.conf");
    std::fs::write(
        &cfg,
        format!(
            "<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <policy context=\"default\">
    <allow send_destination=\"*\" eavesdrop=\"true\"/>
    <allow eavesdrop=\"true\"/>
    <allow own=\"*\"/>
    <allow user=\"*\"/>
  </policy>
</busconfig>
",
            sock.display()
        ),
    )
    .unwrap();
    let child = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", cfg.display()))
        .arg("--nofork")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("dbus-daemon must be installed for integration tests");

    let addr = format!("unix:path={}", sock.display());
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (dir, addr, child)
}

pub(crate) async fn connect_bus(addr: &str, name: Option<&str>) -> Connection {
    let mut builder = zbus::connection::Builder::address(addr).unwrap();
    if let Some(n) = name {
        builder = builder.name(n).unwrap();
    }
    builder.build().await.unwrap()
}

pub(crate) async fn poll_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if pred() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(crate) fn write_fake_hyprctl(bin_dir: &std::path::Path, log_path: &std::path::Path) {
    let script = bin_dir.join("hyprctl");
    let body = format!(
        r#"#!/usr/bin/env bash
if [ "$1" = "-j" ] && [ "$2" = "monitors" ]; then
  echo '[{{"name":"eDP-1","disabled":false,"mirrorOf":"none","width":2560,"height":1600,"refreshRate":240.0,"dpmsStatus":true,"availableModes":["2560x1600@240.00Hz","2560x1600@60.00Hz"]}}]'
  exit 0
fi
printf '%s\n' "$*" >> "{log}"
"#,
        log = log_path.display()
    );
    std::fs::write(&script, body).unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
}
