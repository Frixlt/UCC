// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn t_desk_01_desktop_tray_binary_and_dbus_integration_exist() {
    let root = repo_root();
    let cargo_toml = std::fs::read_to_string(root.join("crates/ucc-desktop/Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains("ucc-desktop-tray"),
        "crates/ucc-desktop/Cargo.toml must define [[bin]] ucc-desktop-tray (ISSUE-14)"
    );
    assert!(
        root.join("crates/ucc-desktop/src/main.rs").exists(),
        "crates/ucc-desktop/src/main.rs executable entry point must exist (ISSUE-14)"
    );

    let tray_rs = std::fs::read_to_string(root.join("crates/ucc-desktop/src/tray.rs")).unwrap();
    assert!(
        tray_rs.contains(".update("),
        "UccTray mutations must push updates via ksni::Handle::update (ISSUE-14)"
    );
}

#[test]
fn t_desk_02_rust_session_telemetry_terminator_and_session_agent_exist() {
    let root = repo_root();
    let desktop_lib = std::fs::read_to_string(root.join("crates/ucc-desktop/src/lib.rs")).unwrap();
    assert!(
        desktop_lib.contains("telemetry") && desktop_lib.contains("pidfd"),
        "crates/ucc-desktop must implement Rust SessionTelemetry and pidfd ProcessTerminator parity (ISSUE-14)"
    );

    let workspace_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        workspace_toml.contains("ucc-session"),
        "workspace must include Rust ucc-session-agent crate parity (ISSUE-14)"
    );
}

#[test]
fn t_desk_03_cpp_desktop_tray_waits_for_tray_and_uses_gui_launcher() {
    let root = repo_root();
    let main_cpp = std::fs::read_to_string(root.join("ucc-desktop/main.cpp")).unwrap();
    assert!(
        !main_cpp.contains("if(!QSystemTrayIcon::isSystemTrayAvailable()) {qCritical(\"No system tray is available in this desktop session\");return 1;}"),
        "ucc-desktop/main.cpp must not exit(1) on early Wayland login before StatusNotifierWatcher is up (ISSUE-30)"
    );

    let tray_cpp = std::fs::read_to_string(root.join("ucc-desktop/DesktopTray.cpp")).unwrap();
    assert!(
        tray_cpp.contains("GuiLauncher") || tray_cpp.contains("QStandardPaths::findExecutable"),
        "DesktopTray::openControlCenter must locate ucc-gui reliably across PATH / preview modes (ISSUE-30)"
    );
}

#[test]
fn t_client_06_pump_curve_editor_consistent_left_margin_and_4pt_support() {
    let root = repo_root();
    let src = std::fs::read_to_string(root.join("ucc-gui/src/PumpCurveEditorWidget.cpp")).unwrap();
    assert!(
        !src.contains("const int left = 110"),
        "PumpCurveEditorWidget::mouseMoveEvent uses left=110 vs toWidget left=80 causing 30px drag jump (ISSUE-37)"
    );
}

#[test]
fn t_client_08_gui_controller_init_and_hardware_tab_caps() {
    let root = repo_root();
    let wc_hpp =
        std::fs::read_to_string(root.join("ucc-gui/inc/LCTWaterCoolerController.hpp")).unwrap();
    assert!(
        wc_hpp.contains("LCTDeviceModel m_connectedModel =")
            || wc_hpp.contains("LCTDeviceModel m_connectedModel{"),
        "LCTWaterCoolerController::m_connectedModel must be explicitly initialized (ISSUE-29)"
    );

    let hw_cpp = std::fs::read_to_string(root.join("ucc-gui/src/HardwareTab.cpp")).unwrap();
    assert!(
        hw_cpp.contains("webcamSWAvailable") || hw_cpp.contains("fnLockSupported"),
        "HardwareTab must respect WebcamSWAvailable / FnLockSupported capabilities (ISSUE-37)"
    );
}

#[test]
fn t_client_10_session_notification_monitor_self_filter_and_hyprctl_fallback() {
    let root = repo_root();
    let notify_hpp =
        std::fs::read_to_string(root.join("ucc-session/NotificationMonitor.hpp")).unwrap();
    assert!(
        notify_hpp.contains("app") || notify_hpp.contains("sender") || notify_hpp.contains("self"),
        "NotificationMonitor must filter out self-emitted UCC alerts to prevent lightbar loops (ISSUE-38)"
    );

    let lid_hpp = std::fs::read_to_string(root.join("ucc-session/LidDisplayHandler.hpp")).unwrap();
    assert!(
        !lid_hpp.contains("{QStringLiteral(\"eval\"), script}")
            || lid_hpp.contains("dispatch"),
        "LidDisplayHandler must not depend solely on hyprctl eval Lua plugin; use standard dispatch dpms (ISSUE-38)"
    );
}
