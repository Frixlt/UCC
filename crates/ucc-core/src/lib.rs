//! Pure domain model for UCC (Universal Control Center / Frixit Control Center).
//!
//! Provides core domain entities, validation invariants, fan curve calculations,
//! power policy state machines, and CPU/charging/lighting configuration types.

pub mod charging;
pub mod cpu;
pub mod dto;
pub mod error;
pub mod fan;
pub mod led;
pub mod power;
pub mod profile;

// Re-exports for convenient access
pub use charging::{ChargeType, ChargingPolicy};
pub use cpu::{CpuLimits, ProfileCpu};
pub use dto::{
    LegacyAppPowerRequestDto, LegacyCpuDto, LegacyDisplayDto, LegacyFanDataDto, LegacyFanDto,
    LegacyFanPointDto, LegacyKeyboardDto, LegacyMetricDataDto, LegacyOdmPowerLimitsDto,
    LegacyOdmProfileDto, LegacyPowerPolicyStatusDto, LegacyProfileDto, LegacySettingsDto,
    LegacyWebcamDto,
};
pub use error::{CoreError, ValidationError};
pub use fan::{
    get_pump_speed_for_temp, pump_speed_to_voltage, voltage_to_pump_speed, FanCurve, FanPoint,
    FanProfile, PumpPoint, PumpVoltage,
};
pub use led::{KeyboardBacklightState, LedSettings};
pub use power::{
    AppPowerRequest, ManualPowerMode, PolicyDecision, PowerMode, PowerPolicy, ProfileState,
};
pub use profile::{
    ProfileDisplay, ProfileFanControl, ProfileKeyboard, ProfileWebcam, UccProfile,
};
