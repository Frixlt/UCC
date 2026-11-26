// SPDX-License-Identifier: GPL-3.0-or-later

//! Rust session relay, health/power notifications, lid display DPMS, and refresh rate advisor.

pub mod alerts;
pub mod lid_handler;
pub mod notification_monitor;
pub mod refresh_advisor;

pub use alerts::{DriverAlertAction, DriverAlertState, PowerSourceAlertState, PowerSourceNotice};
pub use lid_handler::LidDisplayHandler;
pub use notification_monitor::NotificationMonitorParser;
pub use refresh_advisor::{RefreshAdvisor, RefreshBackend, RefreshPlan};
