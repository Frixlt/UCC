use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("Fan curve is empty")]
    EmptyFanCurve,

    #[error("Non-monotonic temperature at point {index}: previous {prev}°C, current {curr}°C")]
    NonMonotonicTemperature { index: usize, prev: i32, curr: i32 },

    #[error("Non-monotonic speed at point {index}: previous {prev}%, current {curr}%")]
    NonMonotonicSpeed { index: usize, prev: u8, curr: u8 },

    #[error("Invalid fan speed {speed}% (must be 0..=100)")]
    InvalidFanSpeed { speed: u8 },

    #[error("Invalid temperature {temp}°C (must be -20..=150)")]
    InvalidTemperature { temp: i32 },

    #[error("Invalid pump voltage code {0}: code 1 is forbidden, valid codes are {{0, 2, 3, 4}}")]
    InvalidPumpVoltage(i32),

    #[error("Invalid pump speed level {0} (must be 0..=4)")]
    InvalidPumpSpeedLevel(u8),

    #[error("Invalid frequency range: min {min} kHz > max {max} kHz")]
    InvalidFrequencyRange { min: u32, max: u32 },

    #[error("Invalid frequency {0} kHz (must be 100_000..=10_000_000)")]
    InvalidFrequency(u32),

    #[error("Invalid TDP value {0} W (must be 1..=500)")]
    InvalidTdp(u32),

    #[error("Non-monotonic TDP at index {index}: previous {prev} W, current {curr} W")]
    NonMonotonicTdp { index: usize, prev: u32, curr: u32 },

    #[error("Invalid charging thresholds: start {start}% >= end {end}%")]
    InvalidChargingThresholds { start: u8, end: u8 },

    #[error("Charging threshold {0}% out of range (must be 0..=100)")]
    ThresholdOutOfRange(u8),

    #[error("Invalid CPU core selection: {0}")]
    InvalidCoreSelection(String),

    #[error("Invalid display brightness {0} (must be 0..=100)")]
    InvalidDisplayBrightness(i32),

    #[error("Invalid LED mode: {0}")]
    InvalidLedMode(String),

    #[error("Invalid power mode: {0}")]
    InvalidPowerMode(String),

    #[error("Invalid profile state: {0}")]
    InvalidProfileState(String),

    #[error("Validation error: {0}")]
    Custom(String),
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Validation(#[from] ValidationError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
