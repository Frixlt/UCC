use crate::error::ValidationError;
use serde::{Deserialize, Serialize};

/// Battery charging control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChargeType {
    Standard,
    Custom,
}

/// Global and profile-level battery charging policy and threshold settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChargingPolicy {
    /// Firmware-level charging profile descriptor (e.g. "balanced", "stationary", "high_capacity")
    pub charging_profile: Option<String>,
    /// USB-C Power Delivery charging priority (e.g. "performance", "charge_battery")
    pub charging_priority: Option<String>,
    /// Charge mode type: Standard or Custom
    pub charge_type: Option<ChargeType>,
    /// Start charging below this battery percentage (0..=100)
    pub start_threshold: Option<u8>,
    /// Stop charging at this battery percentage (0..=100)
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
