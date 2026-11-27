// SPDX-License-Identifier: GPL-3.0-or-later

//! Library components of `uccd`: real hardware backend, orchestrator runtime, lid/wakeup policy,
//! and volatile sleep-state persistence.

pub mod backend;
pub mod lid;
pub mod orchestrator;
pub mod sleep_state;

pub use backend::{
    build_default_custom_fan_catalog, has_ite_sysfs_leds, parse_platform_backend_choice,
    resolve_platform_backend_choice, select_lighting_transport,
    select_lighting_transport_with_platform, select_platform_backend,
    select_platform_backend_with_openers, DbusSignalEvent, LiveUccdBackend, UccdDependencies,
    ENV_PLATFORM_BACKEND,
};
pub use lid::{
    apply_wakeup_policy, is_allowed_wakeup_device, parse_lid_policy_from_json, LidAction,
    LidController, LidPolicyConfig, LidSourceAction, LidSourcePolicy, WakeupApplyResult,
    WakeupPolicy, DEFAULT_AC_UNPLUG_DEBOUNCE,
};
pub use orchestrator::DaemonRuntime;
pub use sleep_state::{
    prepare_sleep, resume_sleep, SleepOperations, SleepState, SleepStateStore,
    DEFAULT_SLEEP_STATE_PATH,
};
