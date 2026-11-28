// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Uniwill `RamFan 1.5` EC curve controller (`0x0F00..=0x0F5C`).
//!
//! Manages 16-point CPU and GPU fan curves in EC RAM with `CUME`/`UFME` control flags,
//! `Fan Boost` override, monotonic validation, and drop-safe restoration to firmware
//! automatic control on clean or abnormal shutdown.

use crate::platform::PlatformError;
use crate::uwcc::uapi::UwccTransport;
use std::sync::{Arc, Mutex, MutexGuard};

/// Number of temperature/duty breakpoints per fan in `RamFan 1.5`.
pub const RAMFAN_POINTS: usize = 16;

/// Lowest EC RAM address used by `RamFan 1.5`.
pub const RAMFAN_BASE: u16 = 0x0F00;

/// Highest EC RAM address used by `RamFan 1.5`.
pub const RAMFAN_END: u16 = 0x0F5C;

/// `RamFan 1.5` EC RAM register map (`0x0F00..=0x0F5C`).
pub mod reg {
    /// Control flags register (`CUME`, `UFME`, `FAN_BOOST`).
    pub const CTRL: u16 = 0x0F00;
    /// 16-byte CPU temperature table (`0x0F10..=0x0F1F`, °C).
    pub const CPU_TEMP_BASE: u16 = 0x0F10;
    /// 16-byte CPU fan duty table (`0x0F20..=0x0F2F`, `0..=200` raw).
    pub const CPU_DUTY_BASE: u16 = 0x0F20;
    /// 16-byte GPU temperature table (`0x0F30..=0x0F3F`, °C).
    pub const GPU_TEMP_BASE: u16 = 0x0F30;
    /// 16-byte GPU fan duty table (`0x0F40..=0x0F4F`, `0..=200` raw).
    pub const GPU_DUTY_BASE: u16 = 0x0F40;
    /// Atomic commit/latch register (`0x01` apply custom table, `0x00` firmware auto).
    pub const COMMIT: u16 = 0x0F5C;
}

/// Control bitflags written to [`reg::CTRL`] (`0x0F00`).
pub mod flags {
    /// Custom User Mode Enable (`CUME`).
    pub const CUME: u8 = 0x01;
    /// User Fan Mode Enable (`UFME`).
    pub const UFME: u8 = 0x02;
    /// Maximum-speed Fan Boost override.
    pub const FAN_BOOST: u8 = 0x04;
}

/// Maximum allowed temperature breakpoint in degrees Celsius.
pub const RAMFAN_MAX_TEMP_C: u8 = 115;

/// Single `(temperature °C, duty %)` breakpoint in a 16-point `RamFan 1.5` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RamFanPoint {
    pub temp_c: u8,
    pub duty_percent: u8,
}

impl RamFanPoint {
    /// Creates a new `RamFan 1.5` curve point.
    pub const fn new(temp_c: u8, duty_percent: u8) -> Self {
        Self {
            temp_c,
            duty_percent,
        }
    }
}

/// Dual 16-point CPU and GPU curve payload for `RamFan 1.5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamFanCurve {
    pub cpu: [RamFanPoint; RAMFAN_POINTS],
    pub gpu: [RamFanPoint; RAMFAN_POINTS],
}

impl RamFanCurve {
    /// Builds a conservative balanced 16-point default curve for both CPU and GPU.
    pub fn balanced_default() -> Self {
        let mut cpu = [RamFanPoint::default(); RAMFAN_POINTS];
        let mut gpu = [RamFanPoint::default(); RAMFAN_POINTS];
        for i in 0..RAMFAN_POINTS {
            let temp = 35u8.saturating_add((i as u8).saturating_mul(4));
            let duty = (20u8.saturating_add((i as u8).saturating_mul(5))).min(100);
            cpu[i] = RamFanPoint::new(temp, duty);
            gpu[i] = RamFanPoint::new(temp, duty);
        }
        Self { cpu, gpu }
    }

    /// Validates that both 16-point curves are monotonic and within safe hardware bounds.
    pub fn validate(&self) -> Result<(), PlatformError> {
        validate_curve_points(&self.cpu)?;
        validate_curve_points(&self.gpu)?;
        Ok(())
    }
}

/// Converts a fan duty percentage (`0..=100`) to Uniwill EC raw duty (`0..=200`).
pub fn percent_to_ec_duty(percent: u8) -> u8 {
    ((u16::from(percent.min(100)) * 200) / 100) as u8
}

/// Converts Uniwill EC raw duty (`0..=200`) back to a percentage (`0..=100`).
pub fn ec_duty_to_percent(raw: u8) -> u8 {
    ((u16::from(raw) * 100) / 200).min(100) as u8
}

fn validate_curve_points(points: &[RamFanPoint; RAMFAN_POINTS]) -> Result<(), PlatformError> {
    for (idx, point) in points.iter().enumerate() {
        if point.duty_percent > 100 {
            return Err(PlatformError::InvalidArgument(
                "RamFan 1.5 duty percentage must be 0..=100",
            ));
        }
        if point.temp_c > RAMFAN_MAX_TEMP_C {
            return Err(PlatformError::InvalidArgument(
                "RamFan 1.5 temperature breakpoint exceeds 115 C",
            ));
        }
        if idx > 0 {
            let prev = points[idx - 1];
            if point.temp_c < prev.temp_c {
                return Err(PlatformError::InvalidArgument(
                    "RamFan 1.5 temperature points must be monotonically non-decreasing",
                ));
            }
            if point.duty_percent < prev.duty_percent {
                return Err(PlatformError::InvalidArgument(
                    "RamFan 1.5 duty points must be monotonically non-decreasing",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct RamFanState {
    custom_active: bool,
    fan_boost: bool,
}

/// Stateful `RamFan 1.5` controller with drop-safe restoration to firmware auto mode.
pub struct RamFanController {
    transport: Arc<dyn UwccTransport>,
    state: Mutex<RamFanState>,
}

impl RamFanController {
    /// Creates a new `RamFan 1.5` controller backed by `transport`.
    pub fn new(transport: Arc<dyn UwccTransport>) -> Self {
        Self {
            transport,
            state: Mutex::new(RamFanState::default()),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, RamFanState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns `true` if a custom `RamFan 1.5` curve is currently active.
    pub fn is_custom_active(&self) -> bool {
        self.lock_state().custom_active
    }

    /// Returns `true` if `Fan Boost` is currently enabled.
    pub fn is_fan_boost_enabled(&self) -> bool {
        self.lock_state().fan_boost
    }

    fn restore_firmware_auto_locked(&self, state: &mut RamFanState) -> Result<(), PlatformError> {
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.transport.ec_write(reg::CTRL, 0x00)?;
        self.transport.ec_write(reg::COMMIT, 0x00)?;
        state.custom_active = false;
        state.fan_boost = false;
        Ok(())
    }

    /// Validates and uploads a 16-point CPU & GPU curve into EC RAM (`0x0F00..=0x0F5C`),
    /// enabling `CUME | UFME`. On partial write failure, attempts fail-safe firmware auto restore.
    pub fn apply_curve(&self, curve: &RamFanCurve) -> Result<(), PlatformError> {
        curve.validate()?;
        let mut state = self.lock_state();
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }

        let write_result = (|| -> Result<(), PlatformError> {
            for i in 0..RAMFAN_POINTS {
                let offset = i as u16;
                self.transport
                    .ec_write(reg::CPU_TEMP_BASE + offset, curve.cpu[i].temp_c)?;
                self.transport.ec_write(
                    reg::CPU_DUTY_BASE + offset,
                    percent_to_ec_duty(curve.cpu[i].duty_percent),
                )?;
                self.transport
                    .ec_write(reg::GPU_TEMP_BASE + offset, curve.gpu[i].temp_c)?;
                self.transport.ec_write(
                    reg::GPU_DUTY_BASE + offset,
                    percent_to_ec_duty(curve.gpu[i].duty_percent),
                )?;
            }

            let mut ctrl = flags::CUME | flags::UFME;
            if state.fan_boost {
                ctrl |= flags::FAN_BOOST;
            }
            self.transport.ec_write(reg::CTRL, ctrl)?;
            self.transport.ec_write(reg::COMMIT, 0x01)?;
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = self.restore_firmware_auto_locked(&mut state);
            return Err(err);
        }

        state.custom_active = true;
        Ok(())
    }

    /// Enables or disables `Fan Boost` (`FLAG_FAN_BOOST` in `0x0F00`).
    pub fn set_fan_boost(&self, enabled: bool) -> Result<(), PlatformError> {
        let mut state = self.lock_state();
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }
        let mut ctrl = if state.custom_active {
            flags::CUME | flags::UFME
        } else {
            0
        };
        if enabled {
            ctrl |= flags::FAN_BOOST;
        }
        self.transport.ec_write(reg::CTRL, ctrl)?;
        self.transport
            .ec_write(reg::COMMIT, u8::from(ctrl != 0))?;
        state.fan_boost = enabled;
        Ok(())
    }

    /// Clears `CUME`, `UFME`, and `FAN_BOOST` flags, restoring EC firmware automatic fan control.
    pub fn restore_firmware_auto(&self) -> Result<(), PlatformError> {
        let mut state = self.lock_state();
        self.restore_firmware_auto_locked(&mut state)
    }
}

impl Drop for RamFanController {
    fn drop(&mut self) {
        let mut state = self.lock_state();
        if state.custom_active || state.fan_boost {
            let _ = self.restore_firmware_auto_locked(&mut state);
        }
    }
}
