// SPDX-License-Identifier: GPL-3.0-or-later

//! Library components of `uccd`: real hardware backend, orchestrator runtime, and lid/wakeup policy.

pub mod backend;
pub mod lid;
pub mod orchestrator;

pub use backend::{build_default_custom_fan_catalog, DbusSignalEvent, LiveUccdBackend, UccdDependencies};
pub use lid::{
    apply_wakeup_policy, is_allowed_wakeup_device, LidAction, LidController, LidPolicyConfig,
    WakeupApplyResult, WakeupPolicy, DEFAULT_AC_UNPLUG_DEBOUNCE,
};
pub use orchestrator::DaemonRuntime;
