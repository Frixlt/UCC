pub mod launcher;
pub mod pidfd;
pub mod telemetry;
pub mod tray;

pub use launcher::{
    activate_gui_dbus, activate_gui_dbus_on, activate_or_launch_gui, find_gui_binary, launch_gui,
    GUI_DBUS_INTERFACE, GUI_DBUS_PATH, GUI_DBUS_PREVIEW_SERVICE, GUI_DBUS_SERVICE,
};
pub use pidfd::{ProcessTerminator, TerminationReport, TerminationTarget};
pub use telemetry::{
    battery_summary, BatterySample, GpuCardSample, ProcessIdentity, SessionPaths, SessionReader,
    TelemetrySnapshot,
};
pub use tray::{
    spawn_tray, spawn_tray_bounded, update_active_profile, update_daemon_connected,
    update_fan_auto, update_profiles, update_water_cooler_connected, TrayAction, UccTray,
};
