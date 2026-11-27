use crate::error::ValidationError;
use serde::de::IntoDeserializer;
use serde::{Deserialize, Deserializer, Serialize};

pub(crate) fn deserialize_opt_threshold<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<i64> = Option::deserialize(deserializer)?;
    match opt {
        None | Some(-1) => Ok(None),
        Some(v) if (0..=100).contains(&v) => Ok(Some(v as u8)),
        Some(v) => Err(serde::de::Error::custom(format!(
            "charge threshold must be -1 or 0..=100, got {v}"
        ))),
    }
}

pub(crate) fn deserialize_opt_charge_type<'de, D>(
    deserializer: D,
) -> Result<Option<ChargeType>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => ChargeType::deserialize(s.as_str().into_deserializer()).map(Some),
    }
}

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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_charge_type"
    )]
    pub charge_type: Option<ChargeType>,
    /// Start charging below this battery percentage (0..=100)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_threshold"
    )]
    pub start_threshold: Option<u8>,
    /// Stop charging at this battery percentage (0..=100)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_threshold"
    )]
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
