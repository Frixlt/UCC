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

pub mod cpu_sentinel_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<i128>::deserialize(deserializer)?;
        match opt {
            Some(v) if v <= 0 => Ok(None),
            Some(v) if v <= u32::MAX as i128 => Ok(Some(v as u32)),
            Some(_) => Err(serde::de::Error::custom(
                "CPU numeric value exceeds u32::MAX",
            )),
            None => Ok(None),
        }
    }

    pub fn serialize<S>(value: &Option<u32>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(v) => serializer.serialize_some(v),
            None => serializer.serialize_none(),
        }
    }
}

/// CPU settings as configured within a UCC system profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileCpu {
    pub online_cores: Option<u32>,
    pub enabled_cores: Option<Vec<u32>>,
    pub min_freq_khz: Option<u32>,
    pub max_freq_khz: Option<u32>,
    pub min_freq_sentinel: Option<i32>,
    pub max_freq_sentinel: Option<i32>,
    pub governor: String,
    pub energy_performance_preference: String,
    pub no_turbo: bool,
    pub hwp_dynamic_boost: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileCpuWire {
    #[serde(default, with = "cpu_sentinel_serde")]
    online_cores: Option<u32>,
    #[serde(default)]
    enabled_cores: Option<Vec<u32>>,
    #[serde(rename = "scalingMinFrequency", default)]
    scaling_min_frequency: Option<i64>,
    #[serde(rename = "scalingMaxFrequency", default)]
    scaling_max_frequency: Option<i64>,
    #[serde(default)]
    governor: String,
    #[serde(default, alias = "epp")]
    energy_performance_preference: String,
    #[serde(rename = "noTurbo", default)]
    no_turbo: bool,
    #[serde(rename = "hwpDynamicBoost", default)]
    hwp_dynamic_boost: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileCpuWireRef<'a> {
    #[serde(
        serialize_with = "cpu_sentinel_serde::serialize",
        skip_serializing_if = "Option::is_none"
    )]
    online_cores: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled_cores: Option<&'a [u32]>,
    #[serde(
        rename = "scalingMinFrequency",
        skip_serializing_if = "Option::is_none"
    )]
    scaling_min_frequency: Option<i64>,
    #[serde(
        rename = "scalingMaxFrequency",
        skip_serializing_if = "Option::is_none"
    )]
    scaling_max_frequency: Option<i64>,
    governor: &'a str,
    energy_performance_preference: &'a str,
    #[serde(rename = "noTurbo")]
    no_turbo: bool,
    #[serde(rename = "hwpDynamicBoost")]
    hwp_dynamic_boost: bool,
}

fn split_freq_value(raw: Option<i64>) -> Result<(Option<u32>, Option<i32>), &'static str> {
    match raw {
        None | Some(0) => Ok((None, None)),
        Some(v) if v < 0 => {
            let sentinel = i32::try_from(v).map_err(|_| "CPU frequency sentinel out of range")?;
            Ok((None, Some(sentinel)))
        }
        Some(v) => {
            let khz = u32::try_from(v).map_err(|_| "CPU frequency exceeds u32::MAX")?;
            Ok((Some(khz), None))
        }
    }
}

impl<'de> Deserialize<'de> for ProfileCpu {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProfileCpuWire::deserialize(deserializer)?;
        let (min_freq_khz, min_freq_sentinel) =
            split_freq_value(wire.scaling_min_frequency).map_err(serde::de::Error::custom)?;
        let (max_freq_khz, max_freq_sentinel) =
            split_freq_value(wire.scaling_max_frequency).map_err(serde::de::Error::custom)?;
        Ok(Self {
            online_cores: wire.online_cores,
            enabled_cores: wire.enabled_cores,
            min_freq_khz,
            max_freq_khz,
            min_freq_sentinel,
            max_freq_sentinel,
            governor: wire.governor,
            energy_performance_preference: wire.energy_performance_preference,
            no_turbo: wire.no_turbo,
            hwp_dynamic_boost: wire.hwp_dynamic_boost,
        })
    }
}

impl Serialize for ProfileCpu {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let wire = ProfileCpuWireRef {
            online_cores: self.online_cores,
            enabled_cores: self.enabled_cores.as_deref(),
            scaling_min_frequency: self
                .min_freq_khz
                .map(i64::from)
                .or_else(|| self.min_freq_sentinel.map(i64::from)),
            scaling_max_frequency: self
                .max_freq_khz
                .map(i64::from)
                .or_else(|| self.max_freq_sentinel.map(i64::from)),
            governor: &self.governor,
            energy_performance_preference: &self.energy_performance_preference,
            no_turbo: self.no_turbo,
            hwp_dynamic_boost: self.hwp_dynamic_boost,
        };
        wire.serialize(serializer)
    }
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
