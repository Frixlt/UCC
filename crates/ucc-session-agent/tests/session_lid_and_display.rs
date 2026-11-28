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

#[test]
fn notification_parser_reset_clears_pending_header_and_dropping_state() {
    let mut parser = NotificationMonitorParser::new(Some(":1.7".to_string()));
    let t0 = Instant::now();

    // 1. reset() after a complete Notify header clears awaiting_app_name.
    let header_only = b"method call time=1.0 sender=:1.88 -> destination=:1.5 serial=11 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n";
    assert_eq!(parser.feed_bytes(header_only, t0), 0);
    parser.reset();
    assert_eq!(
        parser.feed_bytes(b"   string \"Firefox\"\n", t0 + Duration::from_millis(200)),
        0,
        "reset() must clear awaiting_app_name so following string line does not emit"
    );

    // 2. reset() mid-line discards partial buffered header bytes.
    let partial_first = b"method call time=1.1 sender=:1.88 -> destination=:1.5 serial=12 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications;";
    let partial_rest = b" member=Notify\n   string \"Firefox\"\n";
    assert_eq!(
        parser.feed_bytes(partial_first, t0 + Duration::from_millis(300)),
        0
    );
    parser.reset();
    assert_eq!(
        parser.feed_bytes(partial_rest, t0 + Duration::from_millis(450)),
        0,
        "reset() must clear partial line buffer"
    );

    // 3. reset() while dropping an oversized line restores normal parsing immediately.
    let oversized_no_newline = vec![b'x'; 5000];
    assert_eq!(
        parser.feed_bytes(&oversized_no_newline, t0 + Duration::from_millis(600)),
        0
    );
    parser.reset();
    let valid_external = b"method call time=1.5 sender=:1.88 -> destination=:1.5 serial=13 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Thunderbird\"\n";
    assert_eq!(
        parser.feed_bytes(valid_external, t0 + Duration::from_millis(800)),
        1,
        "reset() must clear dropping state so subsequent Notify events are parsed"
    );
}

#[test]
fn notification_parser_drops_lines_over_4096_bytes_and_recovers() {
    let mut parser = NotificationMonitorParser::new(None);
    let t0 = Instant::now();

    // 1. Oversized Notify header (> 4096 bytes) is dropped and does not arm awaiting_app_name.
    let padding = "a".repeat(4100);
    let huge_header = format!(
        "method call time=1.0 {padding} interface=org.freedesktop.Notifications; member=Notify\n   string \"Firefox\"\n"
    );
    assert_eq!(
        parser.feed_bytes(huge_header.as_bytes(), t0),
        0,
        "oversized header (>4096 bytes) must be dropped"
    );

    // 2. Valid header followed by oversized (> 4096 bytes) argument line drops only the huge line
    //    and recovers cleanly on the next normal line.
    let valid_header = b"method call time=2.0 sender=:1.42 -> destination=:1.5 serial=20 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n";
    assert_eq!(
        parser.feed_bytes(valid_header, t0 + Duration::from_millis(200)),
        0
    );
    let huge_arg = format!("   string \"{}\"\n", "b".repeat(4100));
    assert_eq!(
        parser.feed_bytes(huge_arg.as_bytes(), t0 + Duration::from_millis(210)),
        0,
        "oversized arg line (>4096 bytes) must be dropped without emitting"
    );
    assert_eq!(
        parser.feed_bytes(b"   string \"Signal\"\n", t0 + Duration::from_millis(220)),
        1,
        "parser must recover after oversized line newline"
    );

    // 3. Oversized unrelated noise line followed by a normal Notify event parses normally.
    let mut noise = vec![b'z'; 5000];
    noise.push(b'\n');
    assert_eq!(
        parser.feed_bytes(&noise, t0 + Duration::from_millis(400)),
        0
    );
    let valid_after_noise = b"method call time=3.0 sender=:1.99 -> destination=:1.5 serial=21 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Element\"\n";
    assert_eq!(
        parser.feed_bytes(valid_after_noise, t0 + Duration::from_millis(550)),
        1
    );
}

#[test]
fn notification_parser_handles_malformed_and_partial_dbus_output() {
    // Header validator rejects malformed/non-Notify lines.
    assert!(!NotificationMonitorParser::is_notify_header(""));
    assert!(!NotificationMonitorParser::is_notify_header(
        "signal time=1.0 interface=org.freedesktop.Notifications; member=Notify"
    ));
    assert!(!NotificationMonitorParser::is_notify_header(
        "method call time=1.0 interface=org.freedesktop.Notifications; member=CloseNotification"
    ));
    assert!(!NotificationMonitorParser::is_notify_header(
        "method call time=1.0 interface=org.example.Other; member=Notify"
    ));
    assert!(NotificationMonitorParser::is_notify_header(
        "  method call time=1.0 interface=org.freedesktop.Notifications; member=Notify  "
    ));

    let mut parser = NotificationMonitorParser::new(Some(":1.9".to_string()));
    let t0 = Instant::now();

    // 1. Invalid UTF-8 line is ignored without panicking.
    let invalid_utf8 = b"method call \xff\xfe interface=org.freedesktop.Notifications; member=Notify\n   string \"Firefox\"\n";
    assert_eq!(parser.feed_bytes(invalid_utf8, t0), 0);

    // 2. Partial/chunked delivery across multiple feed_bytes calls emits once complete.
    let chunk1 = b"method call time=1.0 sender=:1.42 -> destination=:1.5 ";
    let chunk2 = b"path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   str";
    let chunk3 = b"ing \"Slack\"\n";
    assert_eq!(
        parser.feed_bytes(chunk1, t0 + Duration::from_millis(150)),
        0
    );
    assert_eq!(
        parser.feed_bytes(chunk2, t0 + Duration::from_millis(160)),
        0
    );
    assert_eq!(
        parser.feed_bytes(chunk3, t0 + Duration::from_millis(170)),
        1
    );

    // 3. Malformed / non-string arg line after Notify header emits conservatively.
    let no_string_arg = b"method call time=2.0 sender=:1.42 -> destination=:1.5 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   uint32 42\n";
    assert_eq!(
        parser.feed_bytes(no_string_arg, t0 + Duration::from_millis(350)),
        1
    );

    // 4. Back-to-back Notify headers without an intervening string line emit for the first
    //    and keep awaiting_app_name armed for the second.
    let header = b"method call time=3.0 sender=:1.42 -> destination=:1.5 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n";
    assert_eq!(
        parser.feed_bytes(header, t0 + Duration::from_millis(550)),
        0
    );
    assert_eq!(
        parser.feed_bytes(header, t0 + Duration::from_millis(700)),
        1
    );
    assert_eq!(
        parser.feed_bytes(b"   string \"Discord\"\n", t0 + Duration::from_millis(850)),
        1
    );
}

#[test]
fn lid_handler_connector_matching_and_dpms_command_ordering() {
    let monitors = [
        "DP-1".to_string(),
        "eDP-1".to_string(),
        "HDMI-A-1".to_string(),
        "LVDS-1".to_string(),
        "DP-2".to_string(),
        "DSI-1".to_string(),
        "HDMI-2".to_string(),
    ];

    // Hyprland backend: only internal connectors matched, preserving input slice ordering.
    let mut hypr = LidDisplayHandler::new(RefreshBackend::Hyprland);
    assert!(!hypr.is_lid_closed());
    assert!(hypr.on_lid_state_changed(false, &monitors).is_empty());

    let close_cmds = hypr.on_lid_state_changed(true, &monitors);
    assert!(hypr.is_lid_closed());
    assert_eq!(
        close_cmds,
        vec![
            (
                "hyprctl",
                vec![
                    "dispatch".to_string(),
                    "dpms".to_string(),
                    "off".to_string(),
                    "eDP-1".to_string()
                ]
            ),
            (
                "hyprctl",
                vec![
                    "dispatch".to_string(),
                    "dpms".to_string(),
                    "off".to_string(),
                    "LVDS-1".to_string()
                ]
            ),
            (
                "hyprctl",
                vec![
                    "dispatch".to_string(),
                    "dpms".to_string(),
                    "off".to_string(),
                    "DSI-1".to_string()
                ]
            ),
        ]
    );

    // Duplicate close event is a no-op.
    assert!(hypr.on_lid_state_changed(true, &monitors).is_empty());

    let open_cmds = hypr.on_lid_state_changed(false, &monitors);
    assert!(!hypr.is_lid_closed());
    assert_eq!(
        open_cmds,
        vec![
            (
                "hyprctl",
                vec![
                    "dispatch".to_string(),
                    "dpms".to_string(),
                    "on".to_string(),
                    "eDP-1".to_string()
                ]
            ),
            (
                "hyprctl",
                vec![
                    "dispatch".to_string(),
                    "dpms".to_string(),
                    "on".to_string(),
                    "LVDS-1".to_string()
                ]
            ),
            (
                "hyprctl",
                vec![
                    "dispatch".to_string(),
                    "dpms".to_string(),
                    "on".to_string(),
                    "DSI-1".to_string()
                ]
            ),
        ]
    );

    // KDE backend: kscreen-doctor disable/enable in matching connector order.
    let mut kde = LidDisplayHandler::new(RefreshBackend::Kde);
    assert!(kde.on_lid_state_changed(false, &monitors).is_empty());
    assert_eq!(
        kde.on_lid_state_changed(true, &monitors),
        vec![
            ("kscreen-doctor", vec!["output.eDP-1.disable".to_string()]),
            ("kscreen-doctor", vec!["output.LVDS-1.disable".to_string()]),
            ("kscreen-doctor", vec!["output.DSI-1.disable".to_string()]),
        ]
    );
    assert_eq!(
        kde.on_lid_state_changed(false, &monitors),
        vec![
            ("kscreen-doctor", vec!["output.eDP-1.enable".to_string()]),
            ("kscreen-doctor", vec!["output.LVDS-1.enable".to_string()]),
            ("kscreen-doctor", vec!["output.DSI-1.enable".to_string()]),
        ]
    );

    // External-only monitor list emits no commands.
    let external_only = ["DP-1".to_string(), "HDMI-A-1".to_string()];
    assert!(hypr.on_lid_state_changed(true, &external_only).is_empty());
    assert!(hypr.on_lid_state_changed(false, &external_only).is_empty());

    // Unsupported backend emits no commands even for internal panels.
    let mut unsupported = LidDisplayHandler::new(RefreshBackend::Unsupported);
    assert!(unsupported
        .on_lid_state_changed(false, &monitors)
        .is_empty());
    assert!(unsupported.on_lid_state_changed(true, &monitors).is_empty());
}
