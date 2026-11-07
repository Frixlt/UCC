//! Pure domain model for UCC (Universal Control Center / Frixit Control Center).
//!
//! Provides core domain entities, validation invariants, fan curve calculations,
//! power policy state machines, and CPU/charging/lighting configuration types.

pub mod charging;
pub mod cpu;
pub mod error;
pub mod fan;
pub mod led;
pub mod power;
pub mod profile;

// Re-exports for convenient access
pub use charging::{ChargeType, ChargingPolicy};
pub use cpu::{CpuLimits, ProfileCpu};
pub use error::{CoreError, ValidationError};
pub use fan::{
    get_pump_speed_for_temp, FanCurve, FanPoint, FanProfile, PumpPoint, PumpVoltage,
};
pub use led::{KeyboardBacklightState, LedSettings};
pub use power::{
    AppPowerRequest, ManualPowerMode, PolicyDecision, PowerMode, PowerPolicy, ProfileState,
};
pub use profile::{
    ProfileDisplay, ProfileFanControl, ProfileKeyboard, ProfileWebcam, UccProfile,
};
