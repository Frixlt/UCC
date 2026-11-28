use crate::error::ValidationError;
use serde::{Deserialize, Serialize};

/// Single temperature/fan speed coordinate point in a fan curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FanPoint {
    /// Temperature in degrees Celsius
    pub temp: i32,
    /// Fan speed percentage (0..=100)
    pub speed: u8,
}

impl FanPoint {
    pub const fn new(temp: i32, speed: u8) -> Self {
        Self { temp, speed }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.speed > 100 {
            return Err(ValidationError::InvalidFanSpeed { speed: self.speed });
        }
        if self.temp < -20 || self.temp > 150 {
            return Err(ValidationError::InvalidTemperature { temp: self.temp });
        }
        Ok(())
    }
}

/// A continuous piecewise-linear fan curve defined by a sequence of points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FanCurve {
    pub points: Vec<FanPoint>,
}

impl FanCurve {
    pub fn new(points: Vec<FanPoint>) -> Result<Self, ValidationError> {
        Self::validate_slice(&points)?;
        Ok(Self { points })
    }

    /// Validates a slice of fan curve points against monotonicity invariants without allocating:
    /// 1. Curve must not be empty.
    /// 2. Temperatures must be strictly monotonically increasing: `t_{i-1} < t_i`.
    /// 3. Fan speeds must be monotonically non-decreasing: `s_{i-1} <= s_i`.
    /// 4. Speeds must be in range 0..=100%.
    pub fn validate_slice(points: &[FanPoint]) -> Result<(), ValidationError> {
        if points.is_empty() {
            return Err(ValidationError::EmptyFanCurve);
        }

        for (i, point) in points.iter().enumerate() {
            point.validate()?;
            if i > 0 {
                let prev = &points[i - 1];
                if point.temp <= prev.temp {
                    return Err(ValidationError::NonMonotonicTemperature {
                        index: i,
                        prev: prev.temp,
                        curr: point.temp,
                    });
                }
                if point.speed < prev.speed {
                    return Err(ValidationError::NonMonotonicSpeed {
                        index: i,
                        prev: prev.speed,
                        curr: point.speed,
                    });
                }
            }
        }
        Ok(())
    }

    /// Validates fan curve monotonicity invariants:
    /// 1. Curve must not be empty.
    /// 2. Temperatures must be strictly monotonically increasing: `t_{i-1} < t_i`.
    /// 3. Fan speeds must be monotonically non-decreasing: `s_{i-1} <= s_i`.
    /// 4. Speeds must be in range 0..=100%.
    pub fn validate(&self) -> Result<(), ValidationError> {
        Self::validate_slice(&self.points)
    }

    /// Computes fan speed percentage (0..=100) for a given temperature directly from a slice
    /// via exact integer-rounded linear interpolation (round-half-away-from-zero, avoiding
    /// IEEE-754 1-ULP drift).
    /// Returns -1 if the slice is empty.
    /// Clamps below the first entry to the first speed, and above the last entry to the last speed.
    pub fn get_speed_for_temp_slice(points: &[FanPoint], temp: i32) -> i32 {
        if points.is_empty() {
            return -1;
        }

        let first = &points[0];
        if temp <= first.temp {
            return first.speed as i32;
        }

        let last = &points[points.len() - 1];
        if temp >= last.temp {
            return last.speed as i32;
        }

        for window in points.windows(2) {
            let prev = &window[0];
            let entry = &window[1];

            if entry.temp == temp {
                return entry.speed as i32;
            }

            if temp > prev.temp && temp < entry.temp {
                let temp_diff = i128::from(entry.temp) - i128::from(prev.temp);
                if temp_diff == 0 || prev.speed == entry.speed {
                    return prev.speed as i32;
                }
                let dt = i128::from(temp) - i128::from(prev.temp);
                let ds = i128::from(entry.speed) - i128::from(prev.speed);
                // Exact non-negative rational value: (prev.speed * temp_diff + dt * ds) / temp_diff.
                // Round half away from zero: (2 * total_num + temp_diff) / (2 * temp_diff).
                let total_num = i128::from(prev.speed) * temp_diff + dt * ds;
                return ((total_num * 2 + temp_diff) / (2 * temp_diff)) as i32;
            }
        }

        last.speed as i32
    }

    /// Computes fan speed percentage (0..=100) for a given temperature via linear interpolation.
    /// Returns -1 if the curve is empty.
    /// Clamps below the first entry to the first speed, and above the last entry to the last speed.
    /// Uses exact round-half-away-from-zero interpolation matching C++ `std::lround`.
    pub fn get_speed_for_temp(&self, temp: i32) -> i32 {
        Self::get_speed_for_temp_slice(&self.points, temp)
    }
}

/// Discrete pump voltage levels for external liquid coolers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PumpVoltage {
    V11 = 0x00,
    V12 = 0x01, // Reserved/forbidden on LCT coolers in production
    V7 = 0x02,
    V8 = 0x03,
    Off = 0x04,
}

impl PumpVoltage {
    /// Validates discrete pump voltage codes according to the UCC specification:
    /// - 0: 11V (`V11`)
    /// - 2: 7V (`V7`)
    /// - 3: 8V (`V8`)
    /// - 4: Off
    ///
    /// Code 1 (`V12`) is strictly forbidden / rejected on production hardware.
    /// All other values are invalid.
    pub fn from_discrete_code(code: i32) -> Result<Self, ValidationError> {
        match code {
            0 => Ok(Self::V11),
            2 => Ok(Self::V7),
            3 => Ok(Self::V8),
            4 => Ok(Self::Off),
            1 => Err(ValidationError::InvalidPumpVoltage(1)),
            other => Err(ValidationError::InvalidPumpVoltage(other)),
        }
    }

    pub const fn to_discrete_code(self) -> i32 {
        self as u8 as i32
    }

    pub const fn is_valid_manual_code(code: i32) -> bool {
        matches!(code, 0 | 2 | 3 | 4)
    }
}

/// Single temperature/pump speed level coordinate point in a pump curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PumpPoint {
    pub temp: i32,
    /// Pump speed discrete level: 0=Off, 1=V7, 2=V8, 3=V11, 4=V12
    pub speed: u8,
}

impl PumpPoint {
    pub const fn new(temp: i32, speed: u8) -> Self {
        Self { temp, speed }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.speed > 4 {
            return Err(ValidationError::InvalidPumpSpeedLevel(self.speed));
        }
        if self.temp < -20 || self.temp > 150 {
            return Err(ValidationError::InvalidTemperature { temp: self.temp });
        }
        Ok(())
    }
}

/// Translates a tablePump discrete speed step index (0..4) into the corresponding `PumpVoltage`.
/// Matching C++ `FanProfile.hpp`:
/// - 0 -> `PumpVoltage::Off` (hardware code 4)
/// - 1 -> `PumpVoltage::V7` (hardware code 2)
/// - 2 -> `PumpVoltage::V8` (hardware code 3)
/// - 3 -> `PumpVoltage::V11` (hardware code 0)
/// - 4 -> `PumpVoltage::V12` (hardware code 1)
pub fn pump_speed_to_voltage(speed_idx: u8) -> Result<PumpVoltage, ValidationError> {
    match speed_idx {
        0 => Ok(PumpVoltage::Off),
        1 => Ok(PumpVoltage::V7),
        2 => Ok(PumpVoltage::V8),
        3 => Ok(PumpVoltage::V11),
        4 => Ok(PumpVoltage::V12),
        other => Err(ValidationError::InvalidPumpSpeedLevel(other)),
    }
}

/// Translates a `PumpVoltage` to the tablePump discrete speed step index (0..4).
pub const fn voltage_to_pump_speed(voltage: PumpVoltage) -> u8 {
    match voltage {
        PumpVoltage::Off => 0,
        PumpVoltage::V7 => 1,
        PumpVoltage::V8 => 2,
        PumpVoltage::V11 => 3,
        PumpVoltage::V12 => 4,
    }
}

/// Step-wise (floor) lookup for pump speed based on temperature, matching C++ `FanProfile::getPumpSpeedForTemp`.
/// Scans in O(n) without allocating or sorting (breaking duplicate-temperature ties by last entry,
/// matching stable sort semantics) and clamps legacy pump level 4 (V12) to V11 for safe hardware execution.
pub fn get_pump_speed_for_temp(table: &[PumpPoint], temp: i32) -> PumpVoltage {
    let mut best: Option<&PumpPoint> = None;
    for entry in table {
        if temp >= entry.temp && best.is_none_or(|b| entry.temp >= b.temp) {
            best = Some(entry);
        }
    }
    if let Some(entry) = best {
        let speed_clamped = entry.speed.min(3);
        if let Ok(voltage) = pump_speed_to_voltage(speed_clamped) {
            return voltage;
        }
    }
    PumpVoltage::Off
}

/// Full fan profile containing CPU, GPU, pump, and water cooler fan curves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FanProfile {
    pub id: String,
    pub name: String,
    #[serde(rename = "tableCPU", default, skip_serializing_if = "Vec::is_empty")]
    pub table_cpu: Vec<FanPoint>,
    #[serde(rename = "tableGPU", default, skip_serializing_if = "Vec::is_empty")]
    pub table_gpu: Vec<FanPoint>,
    #[serde(rename = "tablePump", default, skip_serializing_if = "Vec::is_empty")]
    pub table_pump: Vec<PumpPoint>,
    #[serde(
        rename = "tableWaterCoolerFan",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub table_water_cooler_fan: Vec<FanPoint>,
}

impl FanProfile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.table_cpu.is_empty() && !self.table_gpu.is_empty() && self.validate().is_ok()
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.table_cpu.is_empty() {
            FanCurve::validate_slice(&self.table_cpu)?;
        }
        if !self.table_gpu.is_empty() {
            FanCurve::validate_slice(&self.table_gpu)?;
        }
        if !self.table_water_cooler_fan.is_empty() {
            FanCurve::validate_slice(&self.table_water_cooler_fan)?;
        }
        for point in &self.table_pump {
            point.validate()?;
        }
        Ok(())
    }

    pub fn get_speed_for_temp(&self, temp: i32, use_cpu: bool) -> i32 {
        let points = if use_cpu {
            &self.table_cpu
        } else {
            &self.table_gpu
        };
        FanCurve::get_speed_for_temp_slice(points, temp)
    }

    /// Returns water cooler fan speed. If the water cooler fan table is empty,
    /// falls back to `max(cpu_speed, gpu_speed)` matching C++ reference behavior.
    pub fn get_water_cooler_fan_speed_for_temp(&self, temp: i32) -> i32 {
        if self.table_water_cooler_fan.is_empty() {
            let cpu_speed = self.get_speed_for_temp(temp, true);
            let gpu_speed = self.get_speed_for_temp(temp, false);
            cpu_speed.max(gpu_speed)
        } else {
            FanCurve::get_speed_for_temp_slice(&self.table_water_cooler_fan, temp)
        }
    }

    pub fn get_pump_speed_for_temp(&self, temp: i32) -> PumpVoltage {
        get_pump_speed_for_temp(&self.table_pump, temp)
    }
}
