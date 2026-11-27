// SPDX-License-Identifier: GPL-3.0-or-later

use ksni::{MenuItem, Tray};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use ucc_desktop::{
    activate_gui_dbus_on, TrayAction, UccTray, GUI_DBUS_PATH, GUI_DBUS_PREVIEW_PATH,
    GUI_DBUS_PREVIEW_SERVICE, GUI_DBUS_SERVICE,
};
use zbus::connection::Builder;

struct PrivateDbusDaemon {
    child: Child,
    address: String,
}

impl PrivateDbusDaemon {
    fn spawn() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon must be installed to run GUI activation D-Bus tests");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("failed reading dbus-daemon address");
        let address = line.trim().to_string();
        assert!(!address.is_empty(), "empty dbus-daemon address");
        Self { child, address }
    }
}

impl Drop for PrivateDbusDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct MockGuiTokenService {
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "com.uniwill.UccGui")]
impl MockGuiTokenService {
    async fn activate(&self) {
        self.calls.lock().unwrap().push("Activate".to_string());
    }

    async fn activate_with_token(&self, token: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("ActivateWithToken:{token}"));
    }
}

struct MockGuiLegacyService {
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "com.uniwill.UccGui")]
impl MockGuiLegacyService {
    async fn activate(&self) {
        self.calls.lock().unwrap().push("Activate".to_string());
    }
}

#[test]
fn t_desk01_bounded_tray_action_channel_drops_excess_without_blocking() {
    let (tx, mut rx) = mpsc::channel(32);
    let mut tray = UccTray::new_bounded(vec!["Balanced".into(), "Performance".into()], 0, tx);

    for _ in 0..64 {
        tray.activate(0, 0);
    }

    let mut received = Vec::new();
    while let Ok(action) = rx.try_recv() {
        received.push(action);
    }
    assert_eq!(received.len(), 32);
    assert!(received.iter().all(|a| matches!(a, TrayAction::OpenGui(_))));
}

#[test]
fn t_desk01_disconnected_daemon_disables_controls_and_shows_service_unavailable() {
    let (tx, mut rx) = mpsc::channel(32);
    let mut tray = UccTray::new_bounded(vec!["Balanced".into(), "Performance".into()], 0, tx);
    assert!(tray.is_daemon_connected());

    tray.set_daemon_connected(false);
    assert!(!tray.is_daemon_connected());

    let menu = tray.menu();
    let mut saw_unavailable = false;
    let mut radio_disabled = false;
    let mut fan_disabled = false;
    let mut wc_disabled = false;

    for item in menu {
        match item {
            MenuItem::Standard(std_item) => {
                if std_item.label.contains("Service unavailable") {
                    saw_unavailable = true;
                    assert!(!std_item.enabled);
                } else if std_item.label.starts_with("Fan:") {
                    fan_disabled = !std_item.enabled;
                    (std_item.activate)(&mut tray);
                } else if std_item.label.starts_with("Water Cooler:") {
                    wc_disabled = !std_item.enabled;
                    (std_item.activate)(&mut tray);
                }
            }
            MenuItem::RadioGroup(rg) => {
                radio_disabled = rg.options.iter().all(|opt| !opt.enabled);
                (rg.select)(&mut tray, 1);
            }
            _ => {}
        }
    }

    assert!(
        saw_unavailable,
        "disconnected menu must expose 'Service unavailable'"
    );
    assert!(
        radio_disabled,
        "profile radio options must be disabled while disconnected"
    );
    assert!(
        fan_disabled,
        "fan toggle must be disabled while disconnected"
    );
    assert!(
        wc_disabled,
        "water cooler toggle must be disabled while disconnected"
    );
    assert!(
        rx.try_recv().is_err(),
        "disabled daemon controls must not enqueue TrayAction while disconnected"
    );

    tray.set_daemon_connected(true);
    let reenabled_menu = tray.menu();
    let has_unavailable = reenabled_menu.iter().any(|item| match item {
        MenuItem::Standard(s) => s.label.contains("Service unavailable"),
        _ => false,
    });
    assert!(
        !has_unavailable,
        "reconnected menu should clear 'Service unavailable'"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_m97_activate_gui_dbus_prefers_activate_with_token_and_uses_preview_bus_name() {
    let daemon = PrivateDbusDaemon::spawn();
    let preview_calls = Arc::new(Mutex::new(Vec::new()));
    let legacy_calls = Arc::new(Mutex::new(Vec::new()));

    let _preview_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(GUI_DBUS_PREVIEW_SERVICE)
        .unwrap()
        .serve_at(
            GUI_DBUS_PREVIEW_PATH,
            MockGuiTokenService {
                calls: Arc::clone(&preview_calls),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let _legacy_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(GUI_DBUS_SERVICE)
        .unwrap()
        .serve_at(
            GUI_DBUS_PATH,
            MockGuiLegacyService {
                calls: Arc::clone(&legacy_calls),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let client_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    activate_gui_dbus_on(&client_conn, GUI_DBUS_PREVIEW_SERVICE, Some("xdg-token-42"))
        .await
        .expect("preview GUI activation with token must succeed");

    activate_gui_dbus_on(&client_conn, GUI_DBUS_SERVICE, Some("xdg-token-99"))
        .await
        .expect("legacy GUI without ActivateWithToken must fall back to Activate");

    assert_eq!(
        *preview_calls.lock().unwrap(),
        vec!["ActivateWithToken:xdg-token-42".to_string()]
    );
    assert_eq!(*legacy_calls.lock().unwrap(), vec!["Activate".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_m1_5_activate_with_token_reaches_main_gui_service() {
    let daemon = PrivateDbusDaemon::spawn();
    let gui_calls = Arc::new(Mutex::new(Vec::new()));

    let _gui_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .name(GUI_DBUS_SERVICE)
        .unwrap()
        .serve_at(
            GUI_DBUS_PATH,
            MockGuiTokenService {
                calls: Arc::clone(&gui_calls),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();

    let client_conn = Builder::address(daemon.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    activate_gui_dbus_on(&client_conn, GUI_DBUS_SERVICE, Some("ksni-wayland-tok-7"))
        .await
        .expect("main GUI ActivateWithToken must succeed");

    assert_eq!(
        *gui_calls.lock().unwrap(),
        vec!["ActivateWithToken:ksni-wayland-tok-7".to_string()]
    );
}
