//! Windows driverless ACPI WMI (`root\WMI` `AcpiTest_MULong` over `wmiacpi.sys`) EC backend (`M15.2`, `ADR-06`).
//!
//! Provides a portable trait boundary ([`WmiEcTransport`]) and [`WindowsWmiEcBackend`]
//! implementing [`PlatformBackend`] so Windows builds can manage performance modes, fans,
//! temperatures, and TDP limits without requiring a custom signed KMDF driver.

use crate::detection::{DeviceCapabilities, DeviceVendor};
use crate::platform::{PlatformBackend, PlatformBackendKind, PlatformCapabilities, PlatformError};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Uniwill EC register addresses exposed through ACPI `INOU0000` / `AcpiTest_MULong`.
pub mod ec_reg {
    pub const FAN_MANUAL_MODE: u16 = 0x0751;
    pub const CPU_FAN_DUTY: u16 = 0x1804;
    pub const GPU_FAN_DUTY: u16 = 0x1809;
    pub const CPU_TEMP_C: u16 = 0x043E;
    pub const GPU_TEMP_C: u16 = 0x044F;
    pub const PERF_MODE: u16 = 0x0752;
    pub const PL1_WATTS: u16 = 0x0740;
    pub const PL2_WATTS: u16 = 0x0741;
    pub const PL4_WATTS: u16 = 0x0742;
}

/// Recorded ACPI WMI EC operation for deterministic testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WmiEcOp {
    ReadByte(u16),
    WriteByte { addr: u16, val: u8 },
    CallMethod { method_id: u32, arg: u64 },
}

/// Abstract ACPI WMI transport targeting Windows `root\WMI` `AcpiTest_MULong` (`M15.2`).
pub trait WmiEcTransport: Send + Sync {
    fn is_available(&self) -> bool;
    fn read_ec_byte(&self, addr: u16) -> Result<u8, PlatformError>;
    fn write_ec_byte(&self, addr: u16, val: u8) -> Result<(), PlatformError>;
    fn call_wmi_method(&self, method_id: u32, arg: u64) -> Result<u64, PlatformError>;
}

/// Thread-safe in-memory mock ACPI WMI EC register bank for cross-platform testing.
#[derive(Debug, Default)]
pub struct MockWmiEcTransport {
    available: Mutex<bool>,
    regs: Mutex<BTreeMap<u16, u8>>,
    ops: Mutex<Vec<WmiEcOp>>,
}

impl MockWmiEcTransport {
    pub fn new(available: bool) -> Self {
        Self {
            available: Mutex::new(available),
            regs: Mutex::new(BTreeMap::new()),
            ops: Mutex::new(Vec::new()),
        }
    }

    pub fn set_reg(&self, addr: u16, val: u8) {
        self.regs.lock().unwrap().insert(addr, val);
    }

    pub fn get_reg(&self, addr: u16) -> Option<u8> {
        self.regs.lock().unwrap().get(&addr).copied()
    }

    pub fn set_available(&self, avail: bool) {
        *self.available.lock().unwrap() = avail;
    }

    pub fn ops(&self) -> Vec<WmiEcOp> {
        self.ops.lock().unwrap().clone()
    }
}

impl WmiEcTransport for MockWmiEcTransport {
    fn is_available(&self) -> bool {
        *self.available.lock().unwrap()
    }

    fn read_ec_byte(&self, addr: u16) -> Result<u8, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.ops.lock().unwrap().push(WmiEcOp::ReadByte(addr));
        Ok(self.regs.lock().unwrap().get(&addr).copied().unwrap_or(0))
    }

    fn write_ec_byte(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.ops
            .lock()
            .unwrap()
            .push(WmiEcOp::WriteByte { addr, val });
        self.regs.lock().unwrap().insert(addr, val);
        Ok(())
    }

    fn call_wmi_method(&self, method_id: u32, arg: u64) -> Result<u64, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.ops
            .lock()
            .unwrap()
            .push(WmiEcOp::CallMethod { method_id, arg });
        Ok(0)
    }
}

/// Static platform TDP and fan capability descriptor loaded from JSON platform profiles (`ADR-06`).
#[derive(Debug, Clone)]
pub struct WmiPlatformLimits {
    pub tdp_limits: [(i32, i32); 3],
    pub perf_profiles_available: usize,
    pub fans_min_speed: i32,
    pub fans_off_available: bool,
}

impl Default for WmiPlatformLimits {
    fn default() -> Self {
        Self {
            tdp_limits: [(15, 90), (20, 120), (25, 140)],
            perf_profiles_available: 3,
            fans_min_speed: 20,
            fans_off_available: true,
        }
    }
}

/// [`PlatformBackend`] driven over [`WmiEcTransport`] (Windows driverless `wmiacpi.sys` path).
pub struct WindowsWmiEcBackend {
    transport: Arc<dyn WmiEcTransport>,
    limits: WmiPlatformLimits,
    caps: PlatformCapabilities,
}

impl WindowsWmiEcBackend {
    pub fn new(transport: Arc<dyn WmiEcTransport>, limits: WmiPlatformLimits) -> Self {
        let caps = PlatformCapabilities {
            backend: PlatformBackendKind::WindowsWmi,
            device: DeviceCapabilities {
                vendor: DeviceVendor::Uniwill,
                number_of_fans: 2,
                fans_min_speed: limits.fans_min_speed,
                fans_off_available: limits.fans_off_available,
                fan_control: true,
                tdp_control: true,
                ..Default::default()
            },
            tdp_channels: 3,
            perf_profiles_available: limits.perf_profiles_available,
        };
        Self {
            transport,
            limits,
            caps,
        }
    }

    fn fan_duty_reg(fan_idx: usize) -> Result<u16, PlatformError> {
        match fan_idx {
            0 => Ok(ec_reg::CPU_FAN_DUTY),
            1 => Ok(ec_reg::GPU_FAN_DUTY),
            _ => Err(PlatformError::InvalidArgument("Invalid fan index")),
        }
    }

    fn tdp_reg(idx: usize) -> Result<u16, PlatformError> {
        match idx {
            0 => Ok(ec_reg::PL1_WATTS),
            1 => Ok(ec_reg::PL2_WATTS),
            2 => Ok(ec_reg::PL4_WATTS),
            _ => Err(PlatformError::InvalidArgument("Invalid TDP channel index")),
        }
    }
}

impl PlatformBackend for WindowsWmiEcBackend {
    fn kind(&self) -> PlatformBackendKind {
        PlatformBackendKind::WindowsWmi
    }

    fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.caps.clone()
    }

    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        let reg = match fan_idx {
            0 => ec_reg::CPU_TEMP_C,
            1 => ec_reg::GPU_TEMP_C,
            _ => return Err(PlatformError::InvalidArgument("Invalid fan index")),
        };
        let c = i32::from(self.transport.read_ec_byte(reg)?);
        if c == 0 {
            return Err(PlatformError::SensorUnavailable(fan_idx));
        }
        Ok(c)
    }

    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        let reg = Self::fan_duty_reg(fan_idx)?;
        let raw = self.transport.read_ec_byte(reg)?;
        Ok(((u16::from(raw) * 100) / 200).min(100) as u8)
    }

    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        if percent > 100 {
            return Err(PlatformError::InvalidArgument(
                "Fan percentage must be 0..=100",
            ));
        }
        let reg = Self::fan_duty_reg(fan_idx)?;
        let ec_duty = ((u16::from(percent) * 200) / 100) as u8;
        self.transport.write_ec_byte(ec_reg::FAN_MANUAL_MODE, 1)?;
        self.transport.write_ec_byte(reg, ec_duty)
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        self.transport.write_ec_byte(ec_reg::FAN_MANUAL_MODE, 0)
    }

    fn read_tdp(&self, idx: usize) -> Result<i32, PlatformError> {
        let reg = Self::tdp_reg(idx)?;
        Ok(i32::from(self.transport.read_ec_byte(reg)?))
    }

    fn read_tdp_limits(&self, idx: usize) -> Result<(i32, i32), PlatformError> {
        self.limits
            .tdp_limits
            .get(idx)
            .copied()
            .ok_or(PlatformError::InvalidArgument("Invalid TDP channel index"))
    }

    fn set_tdp(&self, idx: usize, watts: i32) -> Result<(), PlatformError> {
        let reg = Self::tdp_reg(idx)?;
        let (min_w, max_w) = self.read_tdp_limits(idx)?;
        if !(min_w..=max_w).contains(&watts) {
            return Err(PlatformError::InvalidArgument(
                "TDP watts out of platform bounds",
            ));
        }
        self.transport.write_ec_byte(reg, watts as u8)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        Ok(i32::from(self.transport.read_ec_byte(ec_reg::PERF_MODE)?))
    }

    fn set_perf_profile(&self, value: i32) -> Result<(), PlatformError> {
        if !(1..=self.limits.perf_profiles_available as i32).contains(&value) {
            return Err(PlatformError::InvalidArgument(
                "Performance profile code out of range",
            ));
        }
        self.transport.write_ec_byte(ec_reg::PERF_MODE, value as u8)
    }

    fn set_mode_enable(&self, value: bool) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::FAN_MANUAL_MODE, u8::from(value))
    }

    fn set_mode(&self, value: i32) -> Result<(), PlatformError> {
        if !(0..=255).contains(&value) {
            return Err(PlatformError::InvalidArgument(
                "Mode value out of byte range",
            ));
        }
        self.transport.write_ec_byte(ec_reg::PERF_MODE, value as u8)
    }
}
