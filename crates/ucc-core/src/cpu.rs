use crate::error::ValidationError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// CPU frequency and power limit boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CpuLimits {
    /// Scaling minimum frequency in kHz
    pub min_freq_khz: Option<u32>,
    /// Scaling maximum frequency in kHz
    pub max_freq_khz: Option<u32>,
    /// Power limits (e.g. PL1, PL2, PL3) in Watts
    pub tdp_values_w: Vec<u32>,
    /// Number of active online logical cores
    pub online_cores: Option<u32>,
    /// Explicit list of enabled logical core IDs (0-indexed)
    pub enabled_cores: Option<Vec<u32>>,
}

impl CpuLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates all CPU limits:
    /// - Frequency bounds: min <= max, within [100_000, 10_000_000] kHz.
    /// - TDP bounds: 1..=500 W, monotonic non-decreasing order (PL1 <= PL2 <= PL3).
    /// - Online cores: online_cores > 0.
    /// - Enabled cores: non-empty, <= 4096 cores, must contain mandatory core 0, no duplicate IDs.
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Frequency limits validation
        if let (Some(min), Some(max)) = (self.min_freq_khz, self.max_freq_khz) {
            if min > max {
                return Err(ValidationError::InvalidFrequencyRange { min, max });
            }
        }
        if let Some(min) = self.min_freq_khz {
            if !(100_000..=10_000_000).contains(&min) {
                return Err(ValidationError::InvalidFrequency(min));
            }
        }
        if let Some(max) = self.max_freq_khz {
            if !(100_000..=10_000_000).contains(&max) {
                return Err(ValidationError::InvalidFrequency(max));
            }
        }

        // TDP limits validation
        for &tdp in &self.tdp_values_w {
            if tdp == 0 || tdp > 500 {
                return Err(ValidationError::InvalidTdp(tdp));
            }
        }
        for i in 1..self.tdp_values_w.len() {
            if self.tdp_values_w[i] < self.tdp_values_w[i - 1] {
                return Err(ValidationError::NonMonotonicTdp {
                    index: i,
                    prev: self.tdp_values_w[i - 1],
                    curr: self.tdp_values_w[i],
                });
            }
        }

        // Online cores validation
        if let Some(online) = self.online_cores {
            if online == 0 {
                return Err(ValidationError::InvalidCoreSelection(
                    "online_cores must be > 0".to_string(),
                ));
            }
        }

        // Enabled cores validation (matches test_cpu_selection.cpp baseline requirements)
        if let Some(ref cores) = self.enabled_cores {
            if cores.is_empty() {
                return Err(ValidationError::InvalidCoreSelection(
                    "enabled_cores must not be empty".to_string(),
                ));
            }
            if cores.len() > 4096 {
                return Err(ValidationError::InvalidCoreSelection(
                    "enabled_cores exceeds maximum supported 4096 cores".to_string(),
                ));
            }
            let mut seen = BTreeSet::new();
            for &core in cores {
                if core > 1_048_575 {
                    return Err(ValidationError::InvalidCoreSelection(format!(
                        "core ID {} out of range",
                        core
                    )));
                }
                if !seen.insert(core) {
                    return Err(ValidationError::InvalidCoreSelection(format!(
                        "duplicate core ID {}",
                        core
                    )));
                }
            }
            if !seen.contains(&0) {
                return Err(ValidationError::InvalidCoreSelection(
                    "core 0 is mandatory and cannot be disabled".to_string(),
                ));
            }
        }

        Ok(())
    }
}

/// CPU settings as configured within a UCC system profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCpu {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_cores: Option<Vec<u32>>,
    #[serde(rename = "scalingMinFrequency", default, skip_serializing_if = "Option::is_none")]
    pub min_freq_khz: Option<u32>,
    #[serde(rename = "scalingMaxFrequency", default, skip_serializing_if = "Option::is_none")]
    pub max_freq_khz: Option<u32>,
    #[serde(default)]
    pub governor: String,
    #[serde(default, alias = "epp")]
    pub energy_performance_preference: String,
    #[serde(rename = "noTurbo", default)]
    pub no_turbo: bool,
    #[serde(rename = "hwpDynamicBoost", default)]
    pub hwp_dynamic_boost: bool,
}

impl ProfileCpu {
    pub fn to_limits(&self, tdp_values: &[u32]) -> CpuLimits {
        CpuLimits {
            min_freq_khz: self.min_freq_khz,
            max_freq_khz: self.max_freq_khz,
            tdp_values_w: tdp_values.to_vec(),
            online_cores: self.online_cores,
            enabled_cores: self.enabled_cores.clone(),
        }
    }

    pub fn validate(&self, tdp_values: &[u32]) -> Result<(), ValidationError> {
        self.to_limits(tdp_values).validate()
    }
}
