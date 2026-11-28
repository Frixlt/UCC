// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Uniwill `RamFan 1.5` EC curve controller (`0x0F00..=0x0F5F`).
//!
//! Manages the 96-byte 16-level CPU and GPU fan tables in EC RAM
//! (`CPU_UpT`, `CPU_DownT`, `CPU_Duty`, `GPU_UpT`, `GPU_DownT`, `GPU_Duty`),
//! independent CPU/GPU split mode (`0x07C5 BIT(7)`), universal fan table latch
//! (`0x07C6 BIT(2)`), `Fan Boost` (`0x0751 BIT(6)`), monotonic validation, and
//! drop-safe restoration to firmware automatic control.

use crate::platform::PlatformError;
use crate::uwcc::uapi::UwccTransport;
use std::sync::{Arc, Mutex, MutexGuard};

/// Number of temperature/duty levels per fan in `RamFan 1.5`.
pub const RAMFAN_POINTS: usize = 16;

/// Lowest EC RAM address used by `RamFan 1.5` (`CPU_UpT[0]`).
pub const RAMFAN_BASE: u16 = 0x0F00;

/// Highest EC RAM address used by `RamFan 1.5` (`GPU_Duty[15]`).
pub const RAMFAN_END: u16 = 0x0F5F;

/// Maximum allowed temperature breakpoint in degrees Celsius.
pub const RAMFAN_MAX_TEMP_C: u8 = 115;

/// Deterministic step-down hysteresis (°C) applied to `DownT[1..=active]`.
pub const RAMFAN_HYSTERESIS_C: u8 = 4;

/// Sentinel byte used to pad unused `UpT` and `DownT` slots.
pub const RAMFAN_PADDING_TEMP: u8 = 0xFF;

/// `RamFan 1.5` EC RAM 96-byte table layout (`0x0F00..=0x0F5F`) and control registers.
pub mod reg {
    /// 16-byte CPU step-up temperature table (`0x0F00..=0x0F0F`, °C; unused = `0xFF`).
    pub const CPU_UP_BASE: u16 = 0x0F00;
    /// 16-byte CPU step-down hysteresis table (`0x0F10..=0x0F1F`, °C; slot `[0]` never written).
    pub const CPU_DOWN_BASE: u16 = 0x0F10;
    /// 16-byte CPU fan duty table (`0x0F20..=0x0F2F`, `0..=200` raw; `[0]` = `0x00` marker).
    pub const CPU_DUTY_BASE: u16 = 0x0F20;
    /// 16-byte GPU step-up temperature table (`0x0F30..=0x0F3F`, °C; unused = `0xFF`).
    pub const GPU_UP_BASE: u16 = 0x0F30;
    /// 16-byte GPU step-down hysteresis table (`0x0F40..=0x0F4F`, °C; slot `[0]` never written).
    pub const GPU_DOWN_BASE: u16 = 0x0F40;
    /// 16-byte GPU fan duty table (`0x0F50..=0x0F5F`, `0..=200` raw; `[0]` = `0x00` marker).
    pub const GPU_DUTY_BASE: u16 = 0x0F50;

    /// Manual fan control / mode register (`0x0751`, `BIT(6)` = `FAN_MODE_BOOST`).
    pub const FAN_MANUAL_MODE: u16 = 0x0751;
    /// Universal fan control register (`0x07C5`, `BIT(7)` = `SPLIT_TABLES` CPU/GPU independent).
    pub const UNIVERSAL_FAN_CTRL: u16 = 0x07C5;
    /// AP OEM 6 register (`0x07C6`, `BIT(2)` = `ENABLE_UNIVERSAL_FAN_CTRL` table latch).
    /// CRITICAL: Always modify via masked RMW (BIT(6) is power-button request on Intel EC 77.0).
    pub const AP_OEM_6: u16 = 0x07C6;
}

/// Control bitflags for `RamFan 1.5` EC registers (`0x0751`, `0x07C5`, `0x07C6`).
pub mod flags {
    /// Maximum-speed Fan Boost override (`BIT(6)` of `0x0751`).
    pub const FAN_MODE_BOOST: u8 = 1 << 6;
    /// Independent CPU & GPU curves (`BIT(7)` of `0x07C5`).
    pub const SPLIT_TABLES: u8 = 1 << 7;
    /// Latch custom `0x0F00..=0x0F5F` RAM fan tables (`BIT(2)` of `0x07C6`).
    pub const ENABLE_UNIVERSAL_FAN_CTRL: u8 = 1 << 2;
}

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

    /// Constructs a 16-slot [`RamFanCurve`] from `1..=16` active CPU and GPU breakpoints,
    /// repeating each fan's final breakpoint to fill any trailing slots.
    pub fn from_points(
        cpu_points: &[RamFanPoint],
        gpu_points: &[RamFanPoint],
    ) -> Result<Self, PlatformError> {
        if cpu_points.is_empty()
            || cpu_points.len() > RAMFAN_POINTS
            || gpu_points.is_empty()
            || gpu_points.len() > RAMFAN_POINTS
        {
            return Err(PlatformError::InvalidArgument(
                "RamFan 1.5 curve must contain between 1 and 16 points per fan",
            ));
        }
        let mut cpu = [cpu_points[cpu_points.len() - 1]; RAMFAN_POINTS];
        cpu[..cpu_points.len()].copy_from_slice(cpu_points);
        let mut gpu = [gpu_points[gpu_points.len() - 1]; RAMFAN_POINTS];
        gpu[..gpu_points.len()].copy_from_slice(gpu_points);
        let curve = Self { cpu, gpu };
        curve.validate()?;
        Ok(curve)
    }

    /// Validates that both 16-point curves are monotonic and within safe hardware bounds.
    pub fn validate(&self) -> Result<(), PlatformError> {
        validate_curve_points(&self.cpu)?;
        validate_curve_points(&self.gpu)?;
        Ok(())
    }
}

/// Encoded 48-byte hardware table (`UpT`, `DownT`, `Duty`) for a single fan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedRamFanTable {
    pub up: [u8; RAMFAN_POINTS],
    pub down: [u8; RAMFAN_POINTS],
    pub duty: [u8; RAMFAN_POINTS],
    pub active_points: usize,
}

/// Converts a fan duty percentage (`0..=100`) to Uniwill EC raw duty (`0..=200`).
pub fn percent_to_ec_duty(percent: u8) -> u8 {
    ((u16::from(percent.min(100)) * 200) / 100) as u8
}

/// Converts Uniwill EC raw duty (`0..=200`) back to a percentage (`0..=100`).
pub fn ec_duty_to_percent(raw: u8) -> u8 {
    ((u16::from(raw) * 100) / 200).min(100) as u8
}

fn encode_active_duty(percent: u8) -> u8 {
    let raw = percent_to_ec_duty(percent);
    raw.max(2) & !1
}

/// Encodes a validated 16-point fan curve into the hardware `RamFan 1.5` layout:
/// - `duty[0] = 0` mandatory EC zero-level marker;
/// - `up[0..active]` populated (`up[15]` always `0xFF`), unused `up` padded with `0xFF`;
/// - deterministic 4 °C hysteresis in `down[1..=active]` (`down[0]` is never written to EC),
///   unused `down` padded with `0xFF`;
/// - active `duty[1..active]` rounded even (`>= 2`), unused `duty[active..16]` padded with
///   the last active duty.
pub fn encode_fan_table(points: &[RamFanPoint; RAMFAN_POINTS]) -> EncodedRamFanTable {
    let mut active = RAMFAN_POINTS;
    while active > 1 && points[active - 1] == points[active - 2] {
        active -= 1;
    }

    let mut up = [RAMFAN_PADDING_TEMP; RAMFAN_POINTS];
    for (slot, point) in up
        .iter_mut()
        .zip(points.iter())
        .take(active.min(RAMFAN_POINTS - 1))
    {
        *slot = point.temp_c;
    }

    let mut down = [RAMFAN_PADDING_TEMP; RAMFAN_POINTS];
    for i in 1..active {
        down[i] = up[i - 1].saturating_sub(RAMFAN_HYSTERESIS_C);
    }
    if active < RAMFAN_POINTS {
        let prev_down = if active > 1 {
            up[active - 2].saturating_sub(RAMFAN_HYSTERESIS_C)
        } else {
            0
        };
        down[active] = prev_down.saturating_add(RAMFAN_HYSTERESIS_C);
    }

    let mut duty = [0u8; RAMFAN_POINTS];
    // Mandatory EC marker at level 0.
    duty[0] = 0x00;
    for i in 1..active {
        duty[i] = encode_active_duty(points[i].duty_percent);
    }
    let last_duty = encode_active_duty(points[active - 1].duty_percent);
    for slot in duty.iter_mut().skip(active) {
        *slot = last_duty;
    }

    EncodedRamFanTable {
        up,
        down,
        duty,
        active_points: active,
    }
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
    applied_curve: Option<RamFanCurve>,
    applied_tables: Option<(EncodedRamFanTable, EncodedRamFanTable)>,
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

    fn rmw_ec_bits(&self, addr: u16, set_mask: u8, clear_mask: u8) -> Result<(), PlatformError> {
        let prev = self.transport.ec_read(addr)?;
        let next = (prev & !clear_mask) | set_mask;
        if next != prev {
            self.transport.ec_write(addr, next)?;
        }
        Ok(())
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
        state.applied_curve = None;
        state.applied_tables = None;
        if !self.transport.is_available() {
            state.custom_active = false;
            state.fan_boost = false;
            return Err(PlatformError::Unavailable);
        }
        // Strictly RMW 0x07C6 BIT(2) = 0 and 0x0751 BIT(6) = 0; NEVER touch 0x0F00 or 0x0F5C.
        let latch_res = self.rmw_ec_bits(reg::AP_OEM_6, 0, flags::ENABLE_UNIVERSAL_FAN_CTRL);
        let boost_res = self.rmw_ec_bits(reg::FAN_MANUAL_MODE, 0, flags::FAN_MODE_BOOST);
        state.custom_active = false;
        state.fan_boost = false;
        latch_res.and(boost_res)
    }

    /// Invalidates the cached EC curve state (e.g. after suspend/resume or EC reset).
    pub fn invalidate_cache(&self) {
        let mut state = self.lock_state();
        state.applied_curve = None;
        state.applied_tables = None;
        state.custom_active = false;
    }

    fn write_fan_table(
        &self,
        up_base: u16,
        down_base: u16,
        duty_base: u16,
        table: &EncodedRamFanTable,
        prev: Option<&EncodedRamFanTable>,
    ) -> Result<(), PlatformError> {
        for i in 0..RAMFAN_POINTS {
            let offset = i as u16;
            if prev.is_none_or(|p| p.up[i] != table.up[i]) {
                self.transport.ec_write(up_base + offset, table.up[i])?;
            }
            // Slot [0] of DownT (0x0F10 / 0x0F40) is reserved and must never be overwritten.
            if i > 0 && prev.is_none_or(|p| p.down[i] != table.down[i]) {
                self.transport.ec_write(down_base + offset, table.down[i])?;
            }
            if prev.is_none_or(|p| p.duty[i] != table.duty[i]) {
                self.transport.ec_write(duty_base + offset, table.duty[i])?;
            }
        }
        Ok(())
    }

    /// Validates and uploads a 16-level CPU & GPU curve into the 96-byte `RamFan 1.5` EC RAM
    /// region (`0x0F00..=0x0F5F`):
    /// 1. RMW `0x07C6 BIT(2) = 0` (disarm table latch while updating);
    /// 2. RMW `0x07C5 BIT(7) = 1` (independent CPU/GPU tables);
    /// 3. Write CPU and GPU `UpT`, `DownT[1..16]`, and `Duty` bytes (diff-cached);
    /// 4. Verify `EC[0x0F00] != 0 || EC[0x0F01] != 0`;
    /// 5. RMW `0x07C6 BIT(2) = 1` (latch custom RAM table).
    ///
    /// On failure, invalidates the cache and attempts fail-safe firmware auto restore.
    pub fn apply_curve(&self, curve: &RamFanCurve) -> Result<(), PlatformError> {
        curve.validate()?;
        let mut state = self.lock_state();
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }

        if state.custom_active || state.applied_curve.is_some() {
            let live_custom_active = self
                .transport
                .ec_read(reg::AP_OEM_6)
                .map(|val| (val & flags::ENABLE_UNIVERSAL_FAN_CTRL) != 0)
                .unwrap_or(false);
            if !live_custom_active {
                state.applied_curve = None;
                state.applied_tables = None;
                state.custom_active = false;
            }
        }

        if state.custom_active && state.applied_curve.as_ref() == Some(curve) {
            return Ok(());
        }

        let cpu_table = encode_fan_table(&curve.cpu);
        let gpu_table = encode_fan_table(&curve.gpu);
        let prev_tables = state.applied_tables;

        let write_result = (|| -> Result<(), PlatformError> {
            // 1. Clear table latch before updating EC RAM.
            self.rmw_ec_bits(reg::AP_OEM_6, 0, flags::ENABLE_UNIVERSAL_FAN_CTRL)?;
            // 2. Enable independent CPU/GPU curves.
            self.rmw_ec_bits(reg::UNIVERSAL_FAN_CTRL, flags::SPLIT_TABLES, 0)?;
            // 3. Write CPU & GPU RamFan 1.5 tables (0x0F00..=0x0F5F, skipping DownT[0]).
            self.write_fan_table(
                reg::CPU_UP_BASE,
                reg::CPU_DOWN_BASE,
                reg::CPU_DUTY_BASE,
                &cpu_table,
                prev_tables.as_ref().map(|(c, _)| c),
            )?;
            self.write_fan_table(
                reg::GPU_UP_BASE,
                reg::GPU_DOWN_BASE,
                reg::GPU_DUTY_BASE,
                &gpu_table,
                prev_tables.as_ref().map(|(_, g)| g),
            )?;
            // 4. Verify table is populated before latching.
            let b0 = self.transport.ec_read(reg::CPU_UP_BASE)?;
            let b1 = self.transport.ec_read(reg::CPU_UP_BASE + 1)?;
            if b0 == 0 && b1 == 0 {
                return Err(PlatformError::InvalidArgument(
                    "RamFan 1.5 verification failed: EC[0x0F00..=0x0F01] are zero",
                ));
            }
            // 5. Latch custom table.
            self.rmw_ec_bits(reg::AP_OEM_6, flags::ENABLE_UNIVERSAL_FAN_CTRL, 0)?;
            Ok(())
        })();

        if let Err(err) = write_result {
            state.applied_curve = None;
            state.applied_tables = None;
            let _ = self.restore_firmware_auto_locked(&mut state);
            return Err(err);
        }

        state.custom_active = true;
        state.applied_curve = Some(curve.clone());
        state.applied_tables = Some((cpu_table, gpu_table));
        Ok(())
    }

    /// Enables or disables `Fan Boost` via RMW on `0x0751 BIT(6)` (`FAN_MODE_BOOST`).
    pub fn set_fan_boost(&self, enabled: bool) -> Result<(), PlatformError> {
        let mut state = self.lock_state();
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }
        if enabled {
            self.rmw_ec_bits(reg::FAN_MANUAL_MODE, flags::FAN_MODE_BOOST, 0)?;
        } else {
            self.rmw_ec_bits(reg::FAN_MANUAL_MODE, 0, flags::FAN_MODE_BOOST)?;
        }
        state.fan_boost = enabled;
        Ok(())
    }

    /// Clears `0x07C6 BIT(2)` and `0x0751 BIT(6)` via RMW, restoring EC firmware automatic
    /// fan control without corrupting `0x0F00..=0x0F5F`.
    pub fn restore_firmware_auto(&self) -> Result<(), PlatformError> {
        let mut state = self.lock_state();
        self.restore_firmware_auto_locked(&mut state)
    }
}

impl Drop for RamFanController {
    fn drop(&mut self) {
        let mut state = self.lock_state();
        if !self.transport.is_available() {
            return;
        }
        if state.custom_active {
            let _ = self.rmw_ec_bits(reg::AP_OEM_6, 0, flags::ENABLE_UNIVERSAL_FAN_CTRL);
            state.custom_active = false;
        }
        if state.fan_boost {
            let _ = self.rmw_ec_bits(reg::FAN_MANUAL_MODE, 0, flags::FAN_MODE_BOOST);
            state.fan_boost = false;
        }
    }
}
