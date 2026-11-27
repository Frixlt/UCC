// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2-14 and V2-P2-15 in `ucc-session-agent`:
//! - V2-P2-14 (R12 + NEW-05): `is_internal_panel()` matches `EDP-`/`LVDS-`/`DSI-` and bare
//!   `EDP`/`LVDS`/`DSI` case-insensitively, and invalid/non-matching earlier outputs do not
//!   abort scanning later outputs in `RefreshAdvisor::plan`.
//! - V2-P2-15: `send_notify`, `close_notify`, and `spawn_refresh_action` D-Bus queries are
//!   wrapped in `tokio::time::timeout(Duration::from_secs(3), ...)`.

use serde_json::json;
use std::fs;
use std::path::Path;
use ucc_session_agent::RefreshAdvisor;

fn make_panel_output(name: &str, current_hz: f64, high_hz: f64) -> serde_json::Value {
    json!({
        "name": name,
        "connected": true,
        "enabled": true,
        "currentModeId": "mode-60",
        "modes": [
            {
                "id": "mode-60",
                "refreshRate": current_hz,
                "size": { "width": 2560, "height": 1600 }
            },
            {
                "id": "mode-high",
                "refreshRate": high_hz,
                "size": { "width": 2560, "height": 1600 }
            }
        ]
    })
}

#[test]
fn v2_p2_14_internal_panel_matching_supports_dsi_lvds_edp_case_insensitively() {
    let drm = Path::new("/nonexistent_drm_root");
    for panel_name in ["DSI-1", "dsi-1", "EDP-1", "edp-1", "LVDS-1", "EDP", "LVDS", "DSI"] {
        let cfg = json!({
            "outputs": [make_panel_output(panel_name, 60.0, 120.0)]
        });
        let plan = RefreshAdvisor::plan(&cfg, "ac", drm, 0).unwrap_or_else(|| {
            panic!("V2-P2-14: expected RefreshPlan for internal panel `{panel_name}`")
        });
        assert_eq!(plan.output, panel_name);
        assert_eq!(plan.target_mode, "mode-high");
    }

    for ext_name in ["HDMI-A-1", "DP-1", "eDPFOO"] {
        let cfg = json!({
            "outputs": [make_panel_output(ext_name, 60.0, 120.0)]
        });
        assert!(
            RefreshAdvisor::plan(&cfg, "ac", drm, 0).is_none(),
            "External/non-internal output `{ext_name}` must not match"
        );
    }
}

#[test]
fn v2_p2_14_invalid_earlier_outputs_do_not_abort_scanning_subsequent_outputs() {
    let drm = Path::new("/nonexistent_drm_root");
    let cfg = json!({
        "outputs": [
            // Missing name -> previously triggered `?` early return!
            {
                "connected": true,
                "enabled": true
            },
            // Internal panel with missing currentModeId / modes -> previously triggered `?`!
            {
                "name": "eDP-1",
                "connected": true,
                "enabled": true
            },
            // Internal panel with current mode missing size -> previously triggered `?`!
            {
                "name": "LVDS-1",
                "connected": true,
                "enabled": true,
                "currentModeId": "bad",
                "modes": [
                    { "id": "bad", "refreshRate": 60.0 }
                ]
            },
            // Valid internal panel later in list must still be planned.
            make_panel_output("DSI-1", 60.0, 144.0)
        ]
    });

    let plan = RefreshAdvisor::plan(&cfg, "ac", drm, 0)
        .expect("V2-P2-14: invalid earlier outputs must not abort scanning later valid DSI-1 panel");
    assert_eq!(plan.output, "DSI-1");
    assert_eq!(plan.target_mode, "mode-high");
    assert!((plan.target_hz - 144.0).abs() < 0.01);
}

#[test]
fn v2_p2_15_notifications_and_refresh_action_dbus_calls_have_3s_timeout() {
    let notif_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/notifications.rs"))
            .expect("read src/notifications.rs");
    let send_start = notif_src
        .find("pub async fn send_notify")
        .expect("send_notify must exist");
    let close_start = notif_src
        .find("pub async fn close_notify")
        .expect("close_notify must exist");
    let send_body = &notif_src[send_start..close_start];
    let close_body = &notif_src[close_start..notif_src.len().min(close_start + 600)];

    assert!(
        send_body.contains("tokio::time::timeout(") && send_body.contains("Duration::from_secs(3)"),
        "V2-P2-15: send_notify must wrap proxy.notify in tokio::time::timeout(Duration::from_secs(3), ...)"
    );
    assert!(
        close_body.contains("tokio::time::timeout(") && close_body.contains("Duration::from_secs(3)"),
        "V2-P2-15: close_notify must wrap proxy.close_notification in tokio::time::timeout(Duration::from_secs(3), ...)"
    );

    let main_src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("read src/main.rs");
    let spawn_start = main_src
        .find("fn spawn_refresh_action")
        .expect("spawn_refresh_action must exist");
    let spawn_body = &main_src[spawn_start..main_src.len().min(spawn_start + 1800)];
    assert!(
        spawn_body.contains("tokio::time::timeout(Duration::from_secs(3), proxy.get_active_profile_json())"),
        "V2-P2-15: spawn_refresh_action must time-bound get_active_profile_json"
    );
    assert!(
        spawn_body.contains("tokio::time::timeout(Duration::from_secs(3), proxy.get_power_policy_json())"),
        "V2-P2-15: spawn_refresh_action must time-bound get_power_policy_json"
    );
}
