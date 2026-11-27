//! Pure fan cooling mathematics ported from `FanControlWorker.hpp`.
//!
//! Combines linear fan-curve interpolation, falling hysteresis, asymmetric EWMA
//! temperature/speed filtering, hardware fan-off/min-speed limits, and critical
//! temperature safety overrides.

use crate::fan::{FanPoint, FanProfile, PumpPoint};
use serde::{Deserialize, Serialize};

/// Asymmetric Exponentially Weighted Moving Average filter for temperatures.
///
/// Reacts quickly to rising temperatures (`alpha_rising = 0.5`) while smoothing
/// cool-down (`alpha_falling = 0.15`) to avoid oscillating fan speeds.
#[derive(Debug, Clone)]
pub struct TemperatureFilter {
    value: Option<f64>,
    alpha_rising: f64,
    alpha_falling: f64,
}

impl Default for TemperatureFilter {
    fn default() -> Self {
        Self::new(0.5, 0.15)
    }
}

impl TemperatureFilter {
    pub const fn new(alpha_rising: f64, alpha_falling: f64) -> Self {
        Self {
            value: None,
            alpha_rising,
            alpha_falling,
        }
    }

    pub fn add_value(&mut self, raw_temp_c: i32) {
        let raw = raw_temp_c as f64;
        match self.value {
            None => self.value = Some(raw),
            Some(prev) => {
                let alpha = if raw > prev {
                    self.alpha_rising
                } else {
                    self.alpha_falling
                };
                self.value = Some(prev + alpha * (raw - prev));
            }
        }
    }

    pub fn filtered_value(&self) -> i32 {
        match self.value {
            Some(v) => v.round() as i32,
            None => 0,
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanLogicType {
    Cpu,
    Gpu,
}

/// Optional temporary curve overrides (live preview in UI) layered over a profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryFanCurves {
    pub cpu: Vec<FanPoint>,
    pub gpu: Vec<FanPoint>,
    pub water_cooler_fan: Vec<FanPoint>,
    pub pump: Vec<PumpPoint>,
}

impl TemporaryFanCurves {
    pub fn apply_to_profile(&self, base: &FanProfile, fan_index: usize) -> FanProfile {
        let mut merged = base.clone();
        if fan_index == 0 && !self.cpu.is_empty() {
            merged.table_cpu = self.cpu.clone();
        } else if fan_index > 0 && !self.gpu.is_empty() {
            merged.table_gpu = self.gpu.clone();
        }
        if !self.water_cooler_fan.is_empty() {
            merged.table_water_cooler_fan = self.water_cooler_fan.clone();
        }
        if !self.pump.is_empty() {
            merged.table_pump = self.pump.clone();
        }
        merged
    }
}

/// Per-fan controller implementing interpolation, hysteresis, EWMA speed smoothing,
/// hardware limits, and critical thermal override.
#[derive(Debug, Clone)]
pub struct FanControlLogic {
    fan_profile: FanProfile,
    logic_type: FanLogicType,
    temp_filter: TemperatureFilter,
    latest_speed_percent: u8,
    smoothed_speed: Option<f64>,
    last_effective_temp: Option<i32>,
    fans_min_speed_hw_limit: i32,
    fans_off_available: bool,
}

impl FanControlLogic {
    pub const HYSTERESIS_DEG: i32 = 3;
    pub const SPEED_ALPHA_UP: f64 = 0.4;
    pub const SPEED_ALPHA_DOWN: f64 = 0.08;
    pub const CRITICAL_TEMPERATURE_C: i32 = 85;
    pub const OVERHEAT_TEMPERATURE_C: i32 = 90;

    pub fn new(fan_profile: FanProfile, logic_type: FanLogicType) -> Self {
        Self {
            fan_profile,
            logic_type,
            temp_filter: TemperatureFilter::default(),
            latest_speed_percent: 0,
            smoothed_speed: None,
            last_effective_temp: None,
            fans_min_speed_hw_limit: 0,
            fans_off_available: true,
        }
    }

    pub fn set_fans_min_speed_hw_limit(&mut self, speed: i32) {
        self.fans_min_speed_hw_limit = speed.clamp(0, 100);
    }

    pub fn set_fans_off_available(&mut self, available: bool) {
        self.fans_off_available = available;
    }

    pub fn update_fan_profile(&mut self, fan_profile: FanProfile) {
        self.fan_profile = fan_profile;
    }

    pub fn fan_profile(&self) -> &FanProfile {
        &self.fan_profile
    }

    pub fn speed_percent(&self) -> u8 {
        self.latest_speed_percent
    }

    pub fn filtered_temperature(&self) -> i32 {
        self.temp_filter.filtered_value()
    }

    /// Feeds a raw temperature sample in degrees Celsius and returns the updated speed (`0..=100`).
    /// For empty/absent fan curves, prefer [`Self::report_temperature_checked`].
    pub fn report_temperature(&mut self, temperature_c: i32) -> u8 {
        self.report_temperature_checked(temperature_c).unwrap_or(0)
    }

    /// Feeds a raw temperature sample and returns `Some(speed)` when the active curve defines at
    /// least one point, or `None` when the curve table is empty/absent (distinct from an explicit `0%` point).
    pub fn report_temperature_checked(&mut self, temperature_c: i32) -> Option<u8> {
        self.temp_filter.add_value(temperature_c);
        self.calculate_speed_percent_checked().inspect(|&spd| {
            self.latest_speed_percent = spd;
        })
    }

    fn apply_hysteresis(&mut self, filtered_temp: i32) -> i32 {
        let next = match self.last_effective_temp {
            None => filtered_temp,
            Some(prev) if filtered_temp >= prev => filtered_temp,
            Some(prev) => {
                let floor = filtered_temp + Self::HYSTERESIS_DEG;
                prev.min(floor).max(filtered_temp)
            }
        };
        self.last_effective_temp = Some(next);
        next
    }

    fn apply_hw_fan_limitations(&self, speed: i32) -> i32 {
        let min_speed = self.fans_min_speed_hw_limit;
        let half_min = min_speed / 2;
        if speed < min_speed {
            if self.fans_off_available && speed < half_min {
                return 0;
            }
            return min_speed;
        }
        speed
    }

    fn smooth_speed(&mut self, target_speed: i32) -> i32 {
        let target = target_speed as f64;
        match self.smoothed_speed {
            None => {
                self.smoothed_speed = Some(target);
                target_speed
            }
            Some(prev) => {
                let alpha = if target > prev {
                    Self::SPEED_ALPHA_UP
                } else {
                    Self::SPEED_ALPHA_DOWN
                };
                let next = prev + alpha * (target - prev);
                self.smoothed_speed = Some(next);
                next.round() as i32
            }
        }
    }

    fn manage_critical_temperature(temp: i32, speed: i32) -> i32 {
        if temp >= Self::OVERHEAT_TEMPERATURE_C {
            100
        } else if temp >= Self::CRITICAL_TEMPERATURE_C {
            speed.max(80)
        } else {
            speed
        }
    }

    fn calculate_speed_percent_checked(&mut self) -> Option<u8> {
        let filtered_temp = self.temp_filter.filtered_value();
        let effective_temp = self.apply_hysteresis(filtered_temp);

        let is_cpu = self.logic_type == FanLogicType::Cpu;
        let raw_curve_speed = self.fan_profile.get_speed_for_temp(effective_temp, is_cpu);
        if raw_curve_speed < 0 {
            return None;
        }

        let curve_speed = self.apply_hw_fan_limitations(raw_curve_speed.clamp(0, 100));
        let mut speed = self.smooth_speed(curve_speed);
        speed = self.apply_hw_fan_limitations(speed.clamp(0, 100));
        speed = Self::manage_critical_temperature(filtered_temp, speed);
        Some(speed.clamp(0, 100) as u8)
    }
}
