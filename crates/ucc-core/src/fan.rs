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
        let curve = Self { points };
        curve.validate()?;
        Ok(curve)
    }

    /// Validates fan curve monotonicity invariants:
    /// 1. Curve must not be empty.
    /// 2. Temperatures must be strictly monotonically increasing: `t_{i-1} < t_i`.
    /// 3. Fan speeds must be monotonically non-decreasing: `s_{i-1} <= s_i`.
    /// 4. Speeds must be in range 0..=100%.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.points.is_empty() {
            return Err(ValidationError::EmptyFanCurve);
        }

        for (i, point) in self.points.iter().enumerate() {
            point.validate()?;
            if i > 0 {
                let prev = &self.points[i - 1];
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

    /// Computes fan speed percentage (0..=100) for a given temperature via linear interpolation.
    /// Returns -1 if the curve is empty.
    /// Clamps below the first entry to the first speed, and above the last entry to the last speed.
    /// Uses standard round-half-away-from-zero interpolation matching C++ `std::lround`.
    pub fn get_speed_for_temp(&self, temp: i32) -> i32 {
        if self.points.is_empty() {
            return -1;
        }

        let first = &self.points[0];
        if temp <= first.temp {
            return first.speed as i32;
        }

        let last = &self.points[self.points.len() - 1];
        if temp >= last.temp {
            return last.speed as i32;
        }

        for i in 1..self.points.len() {
            let prev = &self.points[i - 1];
            let entry = &self.points[i];

            if entry.temp == temp {
                return entry.speed as i32;
            }

            if temp > prev.temp && temp < entry.temp {
                let temp_diff = entry.temp - prev.temp;
                if temp_diff == 0 {
                    return prev.speed as i32;
                }
                let frac = (temp - prev.temp) as f64 / temp_diff as f64;
                let interpolated = prev.speed as f64 + frac * (entry.speed as f64 - prev.speed as f64);
                return interpolated.round() as i32;
            }
        }

        last.speed as i32
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

/// Step-wise (floor) lookup for pump speed based on temperature, matching C++ `FanProfile::getPumpSpeedForTemp`.
pub fn get_pump_speed_for_temp(table: &[PumpPoint], temp: i32) -> PumpVoltage {
    const PUMP_SPEED_TO_VOLTAGE: [PumpVoltage; 5] = [
        PumpVoltage::Off,
        PumpVoltage::V7,
        PumpVoltage::V8,
        PumpVoltage::V11,
        PumpVoltage::V12,
    ];

    let mut result = PumpVoltage::Off;
    for entry in table {
        if temp >= entry.temp {
            let idx = (entry.speed as usize).min(4);
            result = PUMP_SPEED_TO_VOLTAGE[idx];
        } else {
            break;
        }
    }
    result
}

/// Full fan profile containing CPU, GPU, pump, and water cooler fan curves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FanProfile {
    pub id: String,
    pub name: String,
    pub table_cpu: Vec<FanPoint>,
    pub table_gpu: Vec<FanPoint>,
    pub table_pump: Vec<PumpPoint>,
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
        !self.table_cpu.is_empty() && !self.table_gpu.is_empty()
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.table_cpu.is_empty() {
            FanCurve::new(self.table_cpu.clone())?.validate()?;
        }
        if !self.table_gpu.is_empty() {
            FanCurve::new(self.table_gpu.clone())?.validate()?;
        }
        if !self.table_water_cooler_fan.is_empty() {
            FanCurve::new(self.table_water_cooler_fan.clone())?.validate()?;
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
        FanCurve {
            points: points.clone(),
        }
        .get_speed_for_temp(temp)
    }

    /// Returns water cooler fan speed. If the water cooler fan table is empty,
    /// falls back to `max(cpu_speed, gpu_speed)` matching C++ reference behavior.
    pub fn get_water_cooler_fan_speed_for_temp(&self, temp: i32) -> i32 {
        if self.table_water_cooler_fan.is_empty() {
            let cpu_speed = self.get_speed_for_temp(temp, true);
            let gpu_speed = self.get_speed_for_temp(temp, false);
            cpu_speed.max(gpu_speed)
        } else {
            FanCurve {
                points: self.table_water_cooler_fan.clone(),
            }
            .get_speed_for_temp(temp)
        }
    }

    pub fn get_pump_speed_for_temp(&self, temp: i32) -> PumpVoltage {
        get_pump_speed_for_temp(&self.table_pump, temp)
    }
}
