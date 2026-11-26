//! Domain-level platform backend seam decoupling control logic from `/dev/tuxedo_io`.
//!
//! Defines [`PlatformBackend`] covering core EC fan, TDP, and performance profile operations
//! (`M4.9`), currently backed by [`TuxedoIoBackend`] (`/dev/tuxedo_io`) and
//! [`MockPlatformBackend`]. Extended controls (battery charging thresholds, webcam/Fn-Lock
//! switches, extended EC register access, and WMI event streams) extend this seam alongside
//! `uccd` integration (`M8`) and the native Rust `UwccBackend` (`/dev/uwcc`, `M14` / `ADR-05`).

use crate::detection::{detect_device_capabilities, DeviceCapabilities, DeviceVendor, DmiInfo};
use crate::ioctl::{
    read_clevo_fan_speed_percent, read_uniwill_fan_speed_percent, read_uniwill_fan_temp,
    read_uniwill_tdp_limits, requests, set_clevo_fan_speed, set_clevo_fans_auto, set_perf_profile,
    set_uniwill_fan_speed, set_uniwill_fans_auto, set_uniwill_mode, set_uniwill_mode_enable,
    set_uniwill_tdp, IoctlError,
};
use crate::traits::{HardwareIo, Sysfs};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Identifies which platform kernel/EC backend is driving hardware controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlatformBackendKind {
    TuxedoIo,
    Uwcc,
    Mock,
    #[default]
    Unsupported,
}

/// Consolidated platform hardware capabilities reported by a [`PlatformBackend`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlatformCapabilities {
    pub backend: PlatformBackendKind,
    pub device: DeviceCapabilities,
    pub tdp_channels: usize,
    pub perf_profiles_available: usize,
}

/// Domain errors returned by [`PlatformBackend`] operations.
#[derive(Debug, Error)]
pub enum PlatformError {
    #[error(transparent)]
    Ioctl(#[from] IoctlError),
    #[error("Unsupported platform operation: {0}")]
    Unsupported(&'static str),
    #[error("Invalid platform argument: {0}")]
    InvalidArgument(&'static str),
    #[error("Temperature sensor unavailable for fan {0}")]
    SensorUnavailable(usize),
    #[error("Platform backend is unavailable")]
    Unavailable,
}

/// Recorded mutation operations on a mock [`PlatformBackend`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformWrite {
    SetFanSpeed { fan_idx: usize, percent: u8 },
    SetFansAuto,
    SetTdp { index: usize, watts: i32 },
    SetPerfProfile(i32),
    SetModeEnable(bool),
    SetMode(i32),
    SetWebcam(bool),
}

/// High-level domain abstraction over laptop EC/TDP/fan hardware.
pub trait PlatformBackend: Send + Sync {
    /// Returns the backend flavor (`TuxedoIo`, `Uwcc`, `Mock`, `Unsupported`).
    fn kind(&self) -> PlatformBackendKind;

    /// Checks whether the underlying hardware device is currently accessible.
    fn is_available(&self) -> bool;

    /// Returns cached hardware capabilities probed at initialization.
    fn capabilities(&self) -> PlatformCapabilities;

    /// Reads the temperature in degrees Celsius for the given fan index.
    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError>;

    /// Reads current fan duty percentage (`0..=100`) for the given fan index.
    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError>;

    /// Sets manual fan duty percentage (`0..=100`) for the given fan index.
    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError>;

    /// Restores all fans to automatic firmware/EC control.
    fn set_fans_auto(&self) -> Result<(), PlatformError>;

    /// Reads current TDP wattage for the given channel (`0..=2`: PL1/PL2/PL4).
    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError>;

    /// Reads hardware-enforced `(min, max)` TDP limits in watts for `index`.
    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError>;

    /// Sets TDP wattage for `index` after enforcing hardware bounds.
    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError>;

    /// Reads the active ODM/vendor performance profile code when supported by the backend.
    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        Err(PlatformError::Unsupported(
            "Reading perf profile unsupported",
        ))
    }

    /// Writes the active ODM/vendor performance profile code.
    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError>;

    /// Enables or disables Uniwill software performance mode override.
    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError>;

    /// Sets Uniwill mode register value.
    fn set_mode(&self, mode: i32) -> Result<(), PlatformError>;

    /// Reads the firmware webcam hardware switch state (`true` = enabled).
    fn read_webcam(&self) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported("Webcam switch unsupported"))
    }

    /// Sets the firmware webcam hardware switch state (`true` = enabled).
    fn set_webcam(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("Webcam switch unsupported"))
    }
}

/// Validates vendor-specific ODM performance profile wire codes (`W1-12`):
/// - Clevo: `0..=3` (`quiet=0`, `power_saving=1`, `performance=2`, `entertainment=3`)
/// - Uniwill: 1-based `1..=max` (`power_save=1`, `enthusiast=2`, `overboost=3`) when `avail >= 2`
fn validate_perf_profile_code(
    vendor: DeviceVendor,
    perf_profiles_available: usize,
    profile_code: i32,
) -> Result<(), PlatformError> {
    match vendor {
        DeviceVendor::Clevo => {
            if (0..=3).contains(&profile_code) {
                Ok(())
            } else {
                Err(PlatformError::InvalidArgument(
                    "Clevo performance profile code must be 0..=3",
                ))
            }
        }
        DeviceVendor::Uniwill => {
            if perf_profiles_available < 2 {
                return Err(PlatformError::Unsupported(
                    "Uniwill device reports fewer than 2 performance profiles",
                ));
            }
            let max_wire = perf_profiles_available.min(3) as i32;
            if (1..=max_wire).contains(&profile_code) {
                Ok(())
            } else {
                Err(PlatformError::InvalidArgument(
                    "Uniwill performance profile code outside available 1-based wire codes",
                ))
            }
        }
        DeviceVendor::Unsupported => Err(PlatformError::Unsupported(
            "Performance profiles unsupported on this hardware",
        )),
    }
}

/// [`PlatformBackend`] implementation backed by `/dev/tuxedo_io` (`HardwareIo`).
pub struct TuxedoIoBackend<H: HardwareIo> {
    hw: H,
    capabilities: PlatformCapabilities,
    platform_profile_path: Option<std::path::PathBuf>,
}

fn detect_readable_platform_profile(
    device_id: Option<crate::detection::UniwillDeviceID>,
    sysfs: Option<&dyn Sysfs>,
) -> Option<std::path::PathBuf> {
    let sys = sysfs?;
    let mut candidates = vec![std::path::PathBuf::from(
        "/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile",
    )];
    if !crate::detection::DmiQuirks::has_acpi_platform_profile_quirk(device_id) {
        candidates.push(std::path::PathBuf::from(
            "/sys/firmware/acpi/platform_profile",
        ));
    }
    candidates
        .into_iter()
        .find(|p| sys.exists(p) && std::fs::read_to_string(p).is_ok())
}

fn map_platform_profile_str_to_code(vendor: DeviceVendor, raw: &str) -> Option<i32> {
    let lower = raw.trim().to_ascii_lowercase();
    match vendor {
        DeviceVendor::Clevo => match lower.as_str() {
            "quiet" | "silent" | "low-power" => Some(0),
            "power_saving" | "powersave" | "power_save" => Some(1),
            "performance" | "balanced" | "enthusiast" => Some(2),
            "entertainment" | "overboost" | "high_performance" => Some(3),
            _ => None,
        },
        DeviceVendor::Uniwill => match lower.as_str() {
            "low-power" | "quiet" | "power_save" | "powersave" => Some(1),
            "balanced" | "enthusiast" | "office" => Some(2),
            "performance" | "overboost" | "high_performance" => Some(3),
            _ => None,
        },
        DeviceVendor::Unsupported => None,
    }
}

impl<H: HardwareIo> TuxedoIoBackend<H> {
    /// Probes capabilities through `hw` (optionally enriched with `dmi` and `sysfs`).
    pub fn new(hw: H, dmi: Option<&DmiInfo>, sysfs: Option<&dyn Sysfs>) -> Self {
        let device = detect_device_capabilities(&hw, dmi, sysfs);
        let platform_profile_path = detect_readable_platform_profile(device.device_id, sysfs);
        let backend = if device.vendor == DeviceVendor::Unsupported {
            PlatformBackendKind::Unsupported
        } else {
            PlatformBackendKind::TuxedoIo
        };
        let tdp_channels = if device.tdp_control { 3 } else { 0 };
        let perf_profiles_available = match device.vendor {
            DeviceVendor::Clevo => 4,
            DeviceVendor::Uniwill => hw
                .read_i32(requests::R_UW_PROFS_AVAILABLE)
                .map(|v| v.max(0) as usize)
                .unwrap_or(0),
            DeviceVendor::Unsupported => 0,
        };

        Self {
            hw,
            capabilities: PlatformCapabilities {
                backend,
                device,
                tdp_channels,
                perf_profiles_available,
            },
            platform_profile_path,
        }
    }

    /// Borrows the underlying low-level [`HardwareIo`] implementation.
    pub fn hardware_io(&self) -> &H {
        &self.hw
    }

    fn require_available(&self) -> Result<DeviceVendor, PlatformError> {
        if !self.hw.is_available() {
            return Err(PlatformError::Unavailable);
        }
        let vendor = self.capabilities.device.vendor;
        if vendor == DeviceVendor::Unsupported {
            return Err(PlatformError::Unsupported(
                "Hardware interface is unsupported",
            ));
        }
        Ok(vendor)
    }
}

impl<H: HardwareIo> PlatformBackend for TuxedoIoBackend<H> {
    fn kind(&self) -> PlatformBackendKind {
        self.capabilities.backend
    }

    fn is_available(&self) -> bool {
        self.hw.is_available()
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities.clone()
    }

    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        match self.require_available()? {
            DeviceVendor::Uniwill => Ok(read_uniwill_fan_temp(&self.hw, fan_idx)?),
            DeviceVendor::Clevo => {
                let req = match fan_idx {
                    0 => requests::R_CL_FANINFO1,
                    1 => requests::R_CL_FANINFO2,
                    2 => requests::R_CL_FANINFO3,
                    _ => return Err(PlatformError::InvalidArgument("Clevo fan index must be 0..=2")),
                };
                let info = self.hw.read_i32(req)?;
                let temp = ((info >> 16) & 0xff) as i8 as i32;
                if temp <= 1 {
                    return Err(PlatformError::SensorUnavailable(fan_idx));
                }
                Ok(temp)
            }
            DeviceVendor::Unsupported => Err(PlatformError::Unsupported("No supported vendor")),
        }
    }

    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        match self.require_available()? {
            DeviceVendor::Uniwill => Ok(read_uniwill_fan_speed_percent(&self.hw, fan_idx)?),
            DeviceVendor::Clevo => Ok(read_clevo_fan_speed_percent(&self.hw, fan_idx)?),
            DeviceVendor::Unsupported => Err(PlatformError::Unsupported("No supported vendor")),
        }
    }

    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        match self.require_available()? {
            DeviceVendor::Uniwill => Ok(set_uniwill_fan_speed(&self.hw, fan_idx, percent)?),
            DeviceVendor::Clevo => Ok(set_clevo_fan_speed(&self.hw, fan_idx, percent)?),
            DeviceVendor::Unsupported => Err(PlatformError::Unsupported("No supported vendor")),
        }
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        match self.require_available()? {
            DeviceVendor::Uniwill => Ok(set_uniwill_fans_auto(&self.hw)?),
            DeviceVendor::Clevo => Ok(set_clevo_fans_auto(&self.hw)?),
            DeviceVendor::Unsupported => Err(PlatformError::Unsupported("No supported vendor")),
        }
    }

    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
        if self.require_available()? != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported("TDP read requires Uniwill backend"));
        }
        let req = match index {
            0 => requests::R_UW_TDP0,
            1 => requests::R_UW_TDP1,
            2 => requests::R_UW_TDP2,
            _ => return Err(PlatformError::InvalidArgument("Uniwill TDP index must be 0..=2")),
        };
        Ok(self.hw.read_i32(req)?)
    }

    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
        if self.require_available()? != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported(
                "TDP limits require Uniwill backend",
            ));
        }
        Ok(read_uniwill_tdp_limits(&self.hw, index)?)
    }

    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
        if self.require_available()? != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported("TDP write requires Uniwill backend"));
        }
        Ok(set_uniwill_tdp(&self.hw, index, watts, None)?)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        let vendor = self.require_available()?;
        let path = self.platform_profile_path.as_ref().ok_or({
            PlatformError::Unsupported("Reading perf profile unsupported")
        })?;
        let raw = std::fs::read_to_string(path).map_err(|_| {
            PlatformError::Unsupported("Reading perf profile from sysfs unavailable")
        })?;
        map_platform_profile_str_to_code(vendor, &raw).ok_or(PlatformError::Unsupported(
            "Unrecognized platform_profile sysfs value",
        ))
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        let vendor = self.require_available()?;
        validate_perf_profile_code(
            vendor,
            self.capabilities.perf_profiles_available,
            profile_code,
        )?;
        Ok(set_perf_profile(&self.hw, vendor, profile_code)?)
    }

    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
        if self.require_available()? != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported(
                "Mode enable requires Uniwill backend",
            ));
        }
        Ok(set_uniwill_mode_enable(&self.hw, enable)?)
    }

    fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
        if self.require_available()? != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported(
                "Mode index requires Uniwill backend",
            ));
        }
        Ok(set_uniwill_mode(&self.hw, mode)?)
    }

    fn read_webcam(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        if !self.capabilities.device.webcam_switch {
            return Err(PlatformError::Unsupported("Webcam switch unsupported"));
        }
        Ok(self.hw.read_i32(requests::R_CL_WEBCAM_SW)? != 0)
    }

    fn set_webcam(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        if !self.capabilities.device.webcam_switch {
            return Err(PlatformError::Unsupported("Webcam switch unsupported"));
        }
        Ok(self
            .hw
            .write_i32(requests::W_CL_WEBCAM_SW, if enabled { 1 } else { 0 })?)
    }
}

/// Deterministic in-memory mock [`PlatformBackend`] with write journal.
pub struct MockPlatformBackend {
    available: AtomicBool,
    fail_writes: AtomicBool,
    webcam_enabled: AtomicBool,
    capabilities: Mutex<PlatformCapabilities>,
    fan_temps: Mutex<HashMap<usize, Result<i32, ()>>>,
    fan_speeds: Mutex<HashMap<usize, u8>>,
    tdp_values: Mutex<HashMap<usize, i32>>,
    tdp_limits: Mutex<HashMap<usize, (i32, i32)>>,
    perf_profile: Mutex<Option<i32>>,
    journal: Arc<Mutex<Vec<PlatformWrite>>>,
}

impl MockPlatformBackend {
    /// Creates a mock backend configured with custom [`PlatformCapabilities`].
    pub fn new(capabilities: PlatformCapabilities) -> Self {
        Self {
            available: AtomicBool::new(true),
            fail_writes: AtomicBool::new(false),
            webcam_enabled: AtomicBool::new(true),
            capabilities: Mutex::new(capabilities),
            fan_temps: Mutex::new(HashMap::new()),
            fan_speeds: Mutex::new(HashMap::new()),
            tdp_values: Mutex::new(HashMap::new()),
            tdp_limits: Mutex::new(HashMap::new()),
            perf_profile: Mutex::new(None),
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Creates a standard 2-fan Uniwill mock with 3 TDP channels `(15..=115 W)`.
    pub fn uniwill_default() -> Self {
        let caps = PlatformCapabilities {
            backend: PlatformBackendKind::Mock,
            device: DeviceCapabilities {
                vendor: DeviceVendor::Uniwill,
                number_of_fans: 2,
                fans_min_speed: 20,
                fans_off_available: true,
                fan_control: true,
                tdp_control: true,
                charging_thresholds: true,
                ctgp_supported: true,
                ..DeviceCapabilities::default()
            },
            tdp_channels: 3,
            perf_profiles_available: 3,
        };
        let backend = Self::new(caps);
        backend.set_fan_temp(0, 45);
        backend.set_fan_temp(1, 40);
        backend.set_fan_speed(0, 30);
        backend.set_fan_speed(1, 30);
        for idx in 0..3 {
            backend.set_tdp_limits(idx, (15, 115));
            backend.set_tdp_value(idx, 55);
        }
        backend
    }

    /// Creates a standard 3-fan Clevo mock without TDP channels.
    pub fn clevo_default() -> Self {
        let caps = PlatformCapabilities {
            backend: PlatformBackendKind::Mock,
            device: DeviceCapabilities {
                vendor: DeviceVendor::Clevo,
                number_of_fans: 3,
                fans_min_speed: 20,
                fans_off_available: true,
                fan_control: true,
                tdp_control: false,
                webcam_switch: true,
                flightmode_switch: true,
                touchpad_switch: true,
                ..DeviceCapabilities::default()
            },
            tdp_channels: 0,
            perf_profiles_available: 4,
        };
        let backend = Self::new(caps);
        for fan_idx in 0..3 {
            backend.set_fan_temp(fan_idx, 45);
            backend.set_fan_speed(fan_idx, 25);
        }
        backend
    }

    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }

    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    pub fn set_capabilities(&self, capabilities: PlatformCapabilities) {
        *self.capabilities.lock().unwrap() = capabilities;
    }

    pub fn set_fan_temp(&self, fan_idx: usize, temp_c: i32) {
        self.fan_temps.lock().unwrap().insert(fan_idx, Ok(temp_c));
    }

    pub fn set_fan_temp_error(&self, fan_idx: usize) {
        self.fan_temps.lock().unwrap().insert(fan_idx, Err(()));
    }

    pub fn set_fan_speed(&self, fan_idx: usize, percent: u8) {
        self.fan_speeds.lock().unwrap().insert(fan_idx, percent);
    }

    pub fn set_tdp_value(&self, index: usize, watts: i32) {
        self.tdp_values.lock().unwrap().insert(index, watts);
    }

    pub fn set_tdp_limits(&self, index: usize, limits: (i32, i32)) {
        self.tdp_limits.lock().unwrap().insert(index, limits);
        self.tdp_values.lock().unwrap().entry(index).or_insert(limits.1);
    }

    pub fn journal(&self) -> Vec<PlatformWrite> {
        self.journal.lock().unwrap().clone()
    }

    pub fn clear_journal(&self) {
        self.journal.lock().unwrap().clear();
    }

    fn ensure_write_ready(&self) -> Result<(), PlatformError> {
        if !self.is_available() || self.fail_writes.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        Ok(())
    }
}

impl PlatformBackend for MockPlatformBackend {
    fn kind(&self) -> PlatformBackendKind {
        self.capabilities.lock().unwrap().backend
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities.lock().unwrap().clone()
    }

    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        match self.fan_temps.lock().unwrap().get(&fan_idx).copied() {
            Some(Ok(temp)) => Ok(temp),
            _ => Err(PlatformError::SensorUnavailable(fan_idx)),
        }
    }

    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.fan_speeds
            .lock()
            .unwrap()
            .get(&fan_idx)
            .copied()
            .ok_or(PlatformError::SensorUnavailable(fan_idx))
    }

    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        let max_fans = self.capabilities().device.number_of_fans;
        if fan_idx >= max_fans {
            return Err(PlatformError::InvalidArgument("Fan index out of range"));
        }
        if percent > 100 {
            return Err(PlatformError::InvalidArgument(
                "Fan speed percentage must be 0..=100",
            ));
        }
        self.fan_speeds.lock().unwrap().insert(fan_idx, percent);
        self.journal
            .lock()
            .unwrap()
            .push(PlatformWrite::SetFanSpeed { fan_idx, percent });
        Ok(())
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.journal.lock().unwrap().push(PlatformWrite::SetFansAuto);
        Ok(())
    }

    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        let caps = self.capabilities();
        if caps.tdp_channels == 0 {
            return Err(PlatformError::Unsupported("TDP unsupported"));
        }
        if index >= caps.tdp_channels {
            return Err(PlatformError::InvalidArgument("TDP index out of range"));
        }
        self.tdp_values
            .lock()
            .unwrap()
            .get(&index)
            .copied()
            .ok_or(PlatformError::Unavailable)
    }

    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        let caps = self.capabilities();
        if caps.tdp_channels == 0 {
            return Err(PlatformError::Unsupported("TDP unsupported"));
        }
        if index >= caps.tdp_channels {
            return Err(PlatformError::InvalidArgument("TDP index out of range"));
        }
        self.tdp_limits
            .lock()
            .unwrap()
            .get(&index)
            .copied()
            .ok_or(PlatformError::Unavailable)
    }

    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        let (min_w, max_w) = self.read_tdp_limits(index)?;
        if !(min_w..=max_w).contains(&watts) {
            return Err(PlatformError::InvalidArgument(
                "TDP wattage outside hardware limits",
            ));
        }
        self.tdp_values.lock().unwrap().insert(index, watts);
        self.journal
            .lock()
            .unwrap()
            .push(PlatformWrite::SetTdp { index, watts });
        Ok(())
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.perf_profile
            .lock()
            .unwrap()
            .ok_or(PlatformError::Unavailable)
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        let caps = self.capabilities();
        validate_perf_profile_code(
            caps.device.vendor,
            caps.perf_profiles_available,
            profile_code,
        )?;
        *self.perf_profile.lock().unwrap() = Some(profile_code);
        self.journal
            .lock()
            .unwrap()
            .push(PlatformWrite::SetPerfProfile(profile_code));
        Ok(())
    }

    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        if self.capabilities().device.vendor != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported(
                "Mode enable requires Uniwill vendor",
            ));
        }
        self.journal
            .lock()
            .unwrap()
            .push(PlatformWrite::SetModeEnable(enable));
        Ok(())
    }

    fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        if self.capabilities().device.vendor != DeviceVendor::Uniwill {
            return Err(PlatformError::Unsupported(
                "Mode index requires Uniwill vendor",
            ));
        }
        if mode < 0 {
            return Err(PlatformError::InvalidArgument(
                "Uniwill mode cannot be negative",
            ));
        }
        self.journal
            .lock()
            .unwrap()
            .push(PlatformWrite::SetMode(mode));
        Ok(())
    }

    fn read_webcam(&self) -> Result<bool, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        if !self.capabilities().device.webcam_switch {
            return Err(PlatformError::Unsupported("Webcam switch unsupported"));
        }
        Ok(self.webcam_enabled.load(Ordering::SeqCst))
    }

    fn set_webcam(&self, enabled: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        if !self.capabilities().device.webcam_switch {
            return Err(PlatformError::Unsupported("Webcam switch unsupported"));
        }
        self.webcam_enabled.store(enabled, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap()
            .push(PlatformWrite::SetWebcam(enabled));
        Ok(())
    }
}
