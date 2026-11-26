pub mod launcher;
pub mod pidfd;
pub mod telemetry;
pub mod tray;

pub use launcher::{find_gui_binary, launch_gui};
pub use pidfd::{ProcessTerminator, TerminationReport, TerminationTarget};
pub use telemetry::{
    battery_summary, BatterySample, GpuCardSample, ProcessIdentity, SessionPaths, SessionReader,
    TelemetrySnapshot,
};
pub use tray::{
    spawn_tray, update_active_profile, update_fan_auto, update_profiles,
    update_water_cooler_connected, TrayAction, UccTray,
};
