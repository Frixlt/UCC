// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproduction tests for confirmed `ucc-session-agent` findings on commit `1d07126`:
//! - R11: `NotificationMonitorParser` must not filter prefix-colliding unique bus names (`:1.7` vs `:1.70`, `:1.77`)
//! - R12: `LidDisplayHandler` and `internal_monitors_from_config` must handle `DSI-1` consistently and never fabricate `"eDP-1"`
//! - R10: Initial `spawn_monitor()` failure in `main.rs` must schedule `restart_at = Some(...)`

use std::path::PathBuf;
use std::time::{Duration, Instant};
use ucc_session_agent::{LidDisplayHandler, NotificationMonitorParser, RefreshBackend};

/// R11: `NotificationMonitorParser::new(Some(":1.7".into()))` must filter exact self-sender `:1.7`
/// without filtering distinct unique bus names sharing the prefix (`:1.70`, `:1.77`).
#[test]
fn r11_notification_parser_does_not_filter_prefix_colliding_bus_name() {
    let mut parser = NotificationMonitorParser::new(Some(":1.7".to_string()));
    let t0 = Instant::now();

    let from_self = b"method call time=1.0 sender=:1.7 -> destination=:1.5 serial=1 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"ExternalApp\"\n";
    assert_eq!(
        parser.feed_bytes(from_self, t0),
        0,
        "exact own bus name :1.7 must be filtered"
    );

    let from_1_70 = b"method call time=1.2 sender=:1.70 -> destination=:1.5 serial=2 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Firefox\"\n";
    assert_eq!(
        parser.feed_bytes(from_1_70, t0 + Duration::from_millis(200)),
        1,
        "R11: sender=:1.70 must NOT be filtered when own_bus_name is :1.7"
    );

    let from_1_77 = b"method call time=1.5 sender=:1.77 -> destination=:1.5 serial=3 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Thunderbird\"\n";
    assert_eq!(
        parser.feed_bytes(from_1_77, t0 + Duration::from_millis(450)),
        1,
        "R11: sender=:1.77 must NOT be filtered when own_bus_name is :1.7"
    );
}

/// R12: `LidDisplayHandler` must accept internal `DSI-*` panels (matching `notifications.rs`),
/// and `internal_monitors_from_config(None)` must not fabricate a fake `"eDP-1"` fallback.
#[test]
fn r12_lid_handler_and_detector_handle_dsi_consistently_without_fake_edp1() {
    let mut handler = LidDisplayHandler::new(RefreshBackend::Hyprland);
    let monitors = vec!["DSI-1".to_string(), "HDMI-A-1".to_string()];

    // Initial seed produces no commands.
    assert!(handler.on_lid_state_changed(false, &monitors).is_empty());

    let close_cmds = handler.on_lid_state_changed(true, &monitors);
    assert_eq!(
        close_cmds.len(),
        1,
        "R12: LidDisplayHandler must emit a DPMS command for internal panel DSI-1, got {close_cmds:?}"
    );
    assert_eq!(close_cmds[0].0, "hyprctl");
    assert_eq!(close_cmds[0].1, vec!["dispatch", "dpms", "off", "DSI-1"]);

    // Verify `internal_monitors_from_config` in `notifications.rs` does not synthesize fake "eDP-1".
    let notif_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/notifications.rs");
    let notif_src = std::fs::read_to_string(&notif_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", notif_path.display()));
    let fn_start = notif_src
        .find("pub fn internal_monitors_from_config")
        .expect("internal_monitors_from_config must exist in notifications.rs");
    let fn_body = &notif_src[fn_start..notif_src.len().min(fn_start + 900)];
    assert!(
        !fn_body.contains("monitors.push(\"eDP-1\".to_string())"),
        "R12: internal_monitors_from_config must not fabricate fake \"eDP-1\" when config is None or has no internal panels"
    );
}

/// R10: When the initial `spawn_monitor()` returns `None` at agent startup, `main.rs` must
/// schedule `restart_at = Some(...)` so `dbus-monitor` is retried once available.
#[test]
fn r10_initial_dbus_monitor_spawn_failure_schedules_restart() {
    let main_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let main_src = std::fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", main_path.display()));

    let spawn_init = main_src
        .find("let mut monitor_state = spawn_monitor();")
        .expect("main.rs must initialize monitor_state via spawn_monitor()");
    let init_window = &main_src[spawn_init..main_src.len().min(spawn_init + 260)];

    assert!(
        !init_window.contains("let mut restart_at: Option<tokio::time::Instant> = None;"),
        "R10: main.rs unconditionally sets restart_at = None after initial spawn_monitor(), never retrying when initial spawn returns None"
    );
}
