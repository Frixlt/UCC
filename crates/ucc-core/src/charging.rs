use crate::error::ValidationError;
use serde::{Deserialize, Serialize};

/// Battery charging control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChargeType {
    Standard,
    Custom,
    Trickle,
    Fast,
    Adaptive,
    #[serde(alias = "Long Life")]
    LongLife,
    Bypass,
}

impl ChargeType {
    /// Canonical wire/sysfs string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Custom => "Custom",
            Self::Trickle => "Trickle",
            Self::Fast => "Fast",
            Self::Adaptive => "Adaptive",
            Self::LongLife => "LongLife",
            Self::Bypass => "Bypass",
        }
    }

    /// Parses a charge type string returned by kernel sysfs or C++/Rust configs.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "Standard" => Some(Self::Standard),
            "Custom" => Some(Self::Custom),
            "Trickle" => Some(Self::Trickle),
            "Fast" => Some(Self::Fast),
            "Adaptive" => Some(Self::Adaptive),
            "LongLife" | "Long Life" => Some(Self::LongLife),
            "Bypass" => Some(Self::Bypass),
            _ => None,
        }
    }
}

/// Global and profile-level battery charging policy and threshold settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChargingPolicy {
    /// Firmware-level charging profile descriptor (e.g. "balanced", "stationary", "high_capacity")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_profile: Option<String>,
    /// USB-C Power Delivery charging priority (e.g. "performance", "charge_battery")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_priority: Option<String>,
    /// Charge mode type: Standard or Custom
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_type: Option<ChargeType>,
    /// Start charging below this battery percentage (0..=100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_threshold: Option<u8>,
    /// Stop charging at this battery percentage (0..=100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_threshold: Option<u8>,
}

impl ChargingPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates charging thresholds:
    /// - `start_threshold` must be in 0..=100.
    /// - `end_threshold` must be in 0..=100.
    /// - `start_threshold < end_threshold` when both are defined.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(start) = self.start_threshold {
            if start > 100 {
                return Err(ValidationError::ThresholdOutOfRange(start));
            }
        }
        if let Some(end) = self.end_threshold {
            if end > 100 {
                return Err(ValidationError::ThresholdOutOfRange(end));
            }
        }
        if let (Some(start), Some(end)) = (self.start_threshold, self.end_threshold) {
            if start >= end {
                return Err(ValidationError::InvalidChargingThresholds { start, end });
            }
        }
        Ok(())
    }
}
