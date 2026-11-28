//! Domain-level platform backend seam decoupling control logic from `/dev/tuxedo_io`.
//!
//! Defines [`PlatformBackend`] covering core EC fan, TDP, and performance profile operations
//! (`M4.9`), currently backed by [`TuxedoIoBackend`] (`/dev/tuxedo_io`),
//! [`crate::uwcc::UwccBackend`] (`/dev/uwcc`, `M14` / `ADR-05`),
//! [`crate::windows_wmi::WindowsWmiEcBackend`], and [`MockPlatformBackend`].

use crate::detection::{
    detect_device_capabilities, probe_uniwill_tdp_channels, DeviceCapabilities, DeviceVendor,
    DmiInfo,
};
use crate::ioctl::{
    read_clevo_fan_speed_percent, read_uniwill_fan_speed_percent, read_uniwill_fan_temp,
    read_uniwill_tdp_limits, requests, set_clevo_fan_speed, set_clevo_fans_auto, set_perf_profile,
    set_uniwill_fan_speed, set_uniwill_fans_auto, set_uniwill_mode, set_uniwill_mode_enable,
    set_uniwill_tdp, IoctlError,
};
use crate::traits::{HardwareIo, Sysfs};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Identifies which platform kernel/EC backend is driving hardware controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlatformBackendKind {
    TuxedoIo,
    Uwcc,
    WindowsWmi,
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
    SetTccOffset(u8),
    SetPwup { enable: bool, level: u32 },
    SetLightbarPower(bool),
    SetFnLock(bool),
    SetWinLock(bool),
    SetUsbSleepCharging(bool),
    SetChargeThresholds { start: u8, end: u8 },
    SetFanBoost(bool),
    ApplyRamFanCurve(Box<crate::uwcc::RamFanCurve>),
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

    /// Reads real tachometer fan speed in RPM when supported by the backend.
    fn read_fan_rpm(&self, _fan_idx: usize) -> Result<u32, PlatformError> {
        Err(PlatformError::Unsupported("Reading fan RPM unsupported"))
    }

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

    /// Reads CPU TCC temperature offset (`0..=63` °C) when supported.
    fn read_tcc_offset(&self) -> Result<u8, PlatformError> {
        Err(PlatformError::Unsupported("CPU TCC offset unsupported"))
    }

    /// Sets CPU TCC temperature offset (`0..=63` °C) when supported.
    fn set_tcc_offset(&self, _offset_c: u8) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("CPU TCC offset unsupported"))
    }

    /// Configures water-cooler power-up boost (`PWUP`) when supported.
    fn set_pwup(&self, _enable: bool, _level: u32) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("PWUP unsupported"))
    }

    /// Returns `(min_w, max_w)` GPU cTGP wattage limits when supported directly by the platform backend.
    fn read_ctgp_limits(&self) -> Option<(i32, i32)> {
        None
    }

    /// Reads the current GPU cTGP wattage from EC/platform registers when supported.
    fn read_ctgp_watts(&self) -> Result<i32, PlatformError> {
        Err(PlatformError::Unsupported("cTGP wattage read unsupported"))
    }

    /// Sets the GPU cTGP wattage via EC/platform registers when supported.
    fn set_ctgp_watts(&self, _watts: i32) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("cTGP wattage write unsupported"))
    }

    /// Returns firmware charging profile descriptors when supported directly by the platform backend.
    fn charging_profiles_available(&self) -> Vec<String> {
        Vec::new()
    }

    /// Reads the active firmware charging profile descriptor when supported directly by the platform.
    fn read_charging_profile(&self) -> Result<String, PlatformError> {
        Err(PlatformError::Unsupported(
            "Charging profile read unsupported",
        ))
    }

    /// Sets the active firmware charging profile descriptor when supported directly by the platform.
    fn set_charging_profile(&self, _profile: &str) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Charging profile write unsupported",
        ))
    }

    /// Returns `true` when the platform EC reports a backlit lid logo LED.
    fn has_lid_logo_ec(&self) -> bool {
        false
    }

    /// Persists lid logo LED state (`enabled`, RGB `0..=255`, brightness `0..=255`) to EC registers.
    fn set_lid_logo_ec(
        &self,
        _enabled: bool,
        _r: u8,
        _g: u8,
        _b: u8,
        _brightness: u8,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Lid logo EC control unsupported",
        ))
    }

    /// Controls EC lightbar / chassis LED power rail (`0x767`/`0x768`) when supported.
    fn set_lightbar_power(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Lightbar EC power switch unsupported",
        ))
    }

    /// Reads EC Fn-Lock state when supported.
    fn read_fn_lock(&self) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported("Fn Lock switch unsupported"))
    }

    /// Sets EC Fn-Lock state when supported.
    fn set_fn_lock(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("Fn Lock switch unsupported"))
    }

    /// Reads EC Win-Lock state when supported.
    fn read_win_lock(&self) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported("Win Lock switch unsupported"))
    }

    /// Sets EC Win-Lock state when supported.
    fn set_win_lock(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("Win Lock switch unsupported"))
    }

    /// Reads EC USB sleep-charging switch state when supported.
    fn read_usb_sleep_charging(&self) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported(
            "USB sleep charging switch unsupported",
        ))
    }

    /// Sets EC USB sleep-charging switch state when supported.
    fn set_usb_sleep_charging(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "USB sleep charging switch unsupported",
        ))
    }

    /// Reads battery `(start, end)` charging thresholds (`20..=100%`) when supported.
    fn read_charge_thresholds(&self) -> Result<(u8, u8), PlatformError> {
        Err(PlatformError::Unsupported(
            "Battery charge thresholds unsupported",
        ))
    }

    /// Sets battery `(start, end)` charging thresholds (`20 <= start < end <= 100`) when supported.
    fn set_charge_thresholds(&self, _start: u8, _end: u8) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Battery charge thresholds unsupported",
        ))
    }

    /// Enables or disables `Fan Boost` maximum fan override when supported.
    fn set_fan_boost(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("Fan Boost unsupported"))
    }

    /// Uploads a native 16-point `RamFan 1.5` CPU/GPU curve to EC RAM when supported.
    fn apply_ramfan_curve(&self, _curve: &crate::uwcc::RamFanCurve) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("RamFan 1.5 unsupported"))
    }

    /// Non-blocking poll for the next hardware WMI event when supported.
    fn poll_wmi_event(&self) -> Result<Option<crate::uwcc::UwccWmiEvent>, PlatformError> {
        Err(PlatformError::Unsupported("WMI event polling unsupported"))
    }

    /// Reads hardware display MUX active GPU and persistent BIOS boot mode when supported.
    fn read_mux_state(&self) -> Result<crate::uwcc::MuxState, PlatformError> {
        Err(PlatformError::Unsupported("Display MUX unsupported"))
    }

    /// Sets persistent BIOS display MUX boot mode (`MXDM(1..=4)`, applies after reboot).
    fn set_mux_boot_mode(&self, _mode: crate::uwcc::MuxBootMode) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("Display MUX unsupported"))
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
    sysfs: Option<std::sync::Arc<dyn Sysfs>>,
    cached_perf_profile: std::sync::Mutex<Option<i32>>,
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
        .find(|p| sys.exists(p) && sys.read(p).is_ok())
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

fn map_perf_profile_code_to_str(
    vendor: DeviceVendor,
    profile_code: i32,
    path: &std::path::Path,
    sysfs: Option<&dyn Sysfs>,
) -> Option<&'static str> {
    let candidates: &[&'static str] = match vendor {
        DeviceVendor::Uniwill => match profile_code {
            1 => &["low-power", "quiet", "power_save"],
            2 => &["balanced", "enthusiast", "office"],
            3 => &["performance", "overboost"],
            _ => return None,
        },
        DeviceVendor::Clevo => match profile_code {
            0 => &["quiet", "low-power"],
            1 => &["power_saving", "powersave"],
            2 => &["performance", "balanced"],
            3 => &["entertainment", "overboost"],
            _ => return None,
        },
        DeviceVendor::Unsupported => return None,
    };

    if let Some(choices_path) = path.parent().map(|p| p.join("platform_profile_choices")) {
        let raw_choices = match sysfs {
            Some(s) => s.read(&choices_path).ok(),
            None => std::fs::read_to_string(&choices_path).ok(),
        };
        if let Some(raw) = raw_choices {
            let tokens: Vec<&str> = raw.split_whitespace().collect();
            if let Some(matched) = candidates
                .iter()
                .copied()
                .find(|c| tokens.iter().any(|t| t.eq_ignore_ascii_case(c)))
            {
                return Some(matched);
            }
        }
    }
    candidates.first().copied()
}

impl<H: HardwareIo> TuxedoIoBackend<H> {
    /// Probes capabilities through `hw` (optionally enriched with `dmi` and `sysfs`).
    pub fn new(hw: H, dmi: Option<&DmiInfo>, sysfs: Option<&dyn Sysfs>) -> Self {
        Self::build(hw, dmi, sysfs, None)
    }

    /// Probes capabilities using a shared owned [`Sysfs`] handle for runtime sysfs reads.
    pub fn with_sysfs(hw: H, dmi: Option<&DmiInfo>, sysfs: std::sync::Arc<dyn Sysfs>) -> Self {
        let sys_ref: &dyn Sysfs = sysfs.as_ref();
        Self::build(hw, dmi, Some(sys_ref), Some(sysfs.clone()))
    }

    fn build(
        hw: H,
        dmi: Option<&DmiInfo>,
        sysfs: Option<&dyn Sysfs>,
        owned_sysfs: Option<std::sync::Arc<dyn Sysfs>>,
    ) -> Self {
        let device = detect_device_capabilities(&hw, dmi, sysfs);
        let platform_profile_path = detect_readable_platform_profile(device.device_id, sysfs);
        let initial_perf_profile = platform_profile_path
            .as_ref()
            .zip(sysfs)
            .and_then(|(path, sys)| sys.read(path).ok())
            .and_then(|raw| map_platform_profile_str_to_code(device.vendor, &raw));
        let backend = if device.vendor == DeviceVendor::Unsupported {
            PlatformBackendKind::Unsupported
        } else {
            PlatformBackendKind::TuxedoIo
        };
        let tdp_channels = match device.vendor {
            DeviceVendor::Uniwill => probe_uniwill_tdp_channels(&hw),
            _ => 0,
        };
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
            sysfs: owned_sysfs,
            cached_perf_profile: std::sync::Mutex::new(initial_perf_profile),
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
            DeviceVendor::Uniwill => {
                let temp = read_uniwill_fan_temp(&self.hw, fan_idx)?;
                if temp <= 0 {
                    return Err(PlatformError::SensorUnavailable(fan_idx));
                }
                Ok(temp)
            }
            DeviceVendor::Clevo => {
                let req = match fan_idx {
                    0 => requests::R_CL_FANINFO1,
                    1 => requests::R_CL_FANINFO2,
                    2 => requests::R_CL_FANINFO3,
                    _ => {
                        return Err(PlatformError::InvalidArgument(
                            "Clevo fan index must be 0..=2",
                        ))
                    }
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
            return Err(PlatformError::Unsupported(
                "TDP read requires Uniwill backend",
            ));
        }
        let req = match index {
            0 => requests::R_UW_TDP0,
            1 => requests::R_UW_TDP1,
            2 => requests::R_UW_TDP2,
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Uniwill TDP index must be 0..=2",
                ))
            }
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
            return Err(PlatformError::Unsupported(
                "TDP write requires Uniwill backend",
            ));
        }
        Ok(set_uniwill_tdp(&self.hw, index, watts, None)?)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        let vendor = self.require_available()?;
        let path = self
            .platform_profile_path
            .as_ref()
            .ok_or(PlatformError::Unsupported(
                "Reading perf profile unsupported",
            ))?;
        if let Some(ref sys) = self.sysfs {
            let raw = sys.read(path).map_err(|_| {
                PlatformError::Unsupported("Reading perf profile from sysfs unavailable")
            })?;
            return map_platform_profile_str_to_code(vendor, &raw).ok_or(
                PlatformError::Unsupported("Unrecognized platform_profile sysfs value"),
            );
        }
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Some(code) = map_platform_profile_str_to_code(vendor, &raw) {
                return Ok(code);
            }
        }
        self.cached_perf_profile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ok_or(PlatformError::Unsupported(
                "Reading perf profile from sysfs unavailable",
            ))
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        let vendor = self.require_available()?;
        let effective_available = if vendor == DeviceVendor::Uniwill
            && self.capabilities.perf_profiles_available < 2
            && self.platform_profile_path.is_some()
        {
            3
        } else {
            self.capabilities.perf_profiles_available
        };
        validate_perf_profile_code(vendor, effective_available, profile_code)?;

        let try_sysfs_fallback = || -> Option<()> {
            let path = self.platform_profile_path.as_ref()?;
            let profile_str =
                map_perf_profile_code_to_str(vendor, profile_code, path, self.sysfs.as_deref())?;
            let write_res = if let Some(ref sys) = self.sysfs {
                sys.write(path, profile_str)
            } else {
                std::fs::write(path, profile_str)
            };
            if write_res.is_ok() {
                *self
                    .cached_perf_profile
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(profile_code);
                Some(())
            } else {
                None
            }
        };

        if vendor == DeviceVendor::Uniwill && self.capabilities.perf_profiles_available < 2 {
            if try_sysfs_fallback().is_some() {
                return Ok(());
            }
            return Err(PlatformError::Unsupported(
                "Uniwill device reports fewer than 2 performance profiles",
            ));
        }

        match set_perf_profile(&self.hw, vendor, profile_code) {
            Ok(()) => {
                *self
                    .cached_perf_profile
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(profile_code);
                Ok(())
            }
            Err(ioctl_err) => {
                if try_sysfs_fallback().is_some() {
                    return Ok(());
                }
                Err(ioctl_err.into())
            }
        }
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
    tcc_offset: AtomicU8,
    lightbar_power: AtomicBool,
    fn_lock: AtomicBool,
    win_lock: AtomicBool,
    usb_sleep_charging: AtomicBool,
    fan_boost: AtomicBool,
    capabilities: Mutex<PlatformCapabilities>,
    fan_temps: Mutex<HashMap<usize, Result<i32, ()>>>,
    fan_speeds: Mutex<HashMap<usize, u8>>,
    fan_rpms: Mutex<HashMap<usize, u32>>,
    tdp_values: Mutex<HashMap<usize, i32>>,
    tdp_limits: Mutex<HashMap<usize, (i32, i32)>>,
    perf_profile: Mutex<Option<i32>>,
    charge_thresholds: Mutex<(u8, u8)>,
    ramfan_curve: Mutex<Option<crate::uwcc::RamFanCurve>>,
    wmi_events: Mutex<VecDeque<crate::uwcc::UwccWmiEvent>>,
    journal: Arc<Mutex<Vec<PlatformWrite>>>,
}

impl MockPlatformBackend {
    /// Creates a mock backend configured with custom [`PlatformCapabilities`].
    pub fn new(capabilities: PlatformCapabilities) -> Self {
        Self {
            available: AtomicBool::new(true),
            fail_writes: AtomicBool::new(false),
            webcam_enabled: AtomicBool::new(true),
            tcc_offset: AtomicU8::new(0),
            lightbar_power: AtomicBool::new(true),
            fn_lock: AtomicBool::new(false),
            win_lock: AtomicBool::new(false),
            usb_sleep_charging: AtomicBool::new(false),
            fan_boost: AtomicBool::new(false),
            capabilities: Mutex::new(capabilities),
            fan_temps: Mutex::new(HashMap::new()),
            fan_speeds: Mutex::new(HashMap::new()),
            fan_rpms: Mutex::new(HashMap::new()),
            tdp_values: Mutex::new(HashMap::new()),
            tdp_limits: Mutex::new(HashMap::new()),
            perf_profile: Mutex::new(None),
            charge_thresholds: Mutex::new((60, 80)),
            ramfan_curve: Mutex::new(None),
            wmi_events: Mutex::new(VecDeque::new()),
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
        backend.set_fan_rpm(0, 2400);
        backend.set_fan_rpm(1, 2100);
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
        *self.capabilities.lock().unwrap_or_else(|e| e.into_inner()) = capabilities;
    }

    pub fn set_fan_temp(&self, fan_idx: usize, temp_c: i32) {
        self.fan_temps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fan_idx, Ok(temp_c));
    }

    pub fn set_fan_temp_error(&self, fan_idx: usize) {
        self.fan_temps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fan_idx, Err(()));
    }

    pub fn set_fan_speed(&self, fan_idx: usize, percent: u8) {
        self.fan_speeds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fan_idx, percent);
    }

    pub fn set_fan_rpm(&self, fan_idx: usize, rpm: u32) {
        self.fan_rpms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fan_idx, rpm);
    }

    pub fn set_tdp_value(&self, index: usize, watts: i32) {
        self.tdp_values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(index, watts);
    }

    pub fn set_tdp_limits(&self, index: usize, limits: (i32, i32)) {
        self.tdp_limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(index, limits);
        self.tdp_values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(index)
            .or_insert(limits.1);
    }

    pub fn set_mock_tcc_offset(&self, offset_c: u8) {
        self.tcc_offset.store(offset_c, Ordering::SeqCst);
    }

    pub fn set_mock_lightbar_power(&self, enabled: bool) {
        self.lightbar_power.store(enabled, Ordering::SeqCst);
    }

    pub fn mock_lightbar_power(&self) -> bool {
        self.lightbar_power.load(Ordering::SeqCst)
    }

    pub fn set_mock_fn_lock(&self, enabled: bool) {
        self.fn_lock.store(enabled, Ordering::SeqCst);
    }

    pub fn set_mock_win_lock(&self, enabled: bool) {
        self.win_lock.store(enabled, Ordering::SeqCst);
    }

    pub fn set_mock_usb_sleep_charging(&self, enabled: bool) {
        self.usb_sleep_charging.store(enabled, Ordering::SeqCst);
    }

    pub fn set_mock_charge_thresholds(&self, start: u8, end: u8) {
        *self
            .charge_thresholds
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = (start, end);
    }

    pub fn mock_fan_boost(&self) -> bool {
        self.fan_boost.load(Ordering::SeqCst)
    }

    pub fn mock_ramfan_curve(&self) -> Option<crate::uwcc::RamFanCurve> {
        self.ramfan_curve
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn push_wmi_event(&self, event: crate::uwcc::UwccWmiEvent) {
        self.wmi_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
    }

    pub fn journal(&self) -> Vec<PlatformWrite> {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear_journal(&self) {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .backend
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        match self
            .fan_temps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&fan_idx)
            .copied()
        {
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
            .unwrap_or_else(|e| e.into_inner())
            .get(&fan_idx)
            .copied()
            .ok_or(PlatformError::SensorUnavailable(fan_idx))
    }

    fn read_fan_rpm(&self, fan_idx: usize) -> Result<u32, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.fan_rpms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
        self.fan_speeds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fan_idx, percent);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetFanSpeed { fan_idx, percent });
        Ok(())
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.fan_boost.store(false, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetFansAuto);
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
            .unwrap_or_else(|e| e.into_inner())
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
            .unwrap_or_else(|e| e.into_inner())
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
        self.tdp_values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(index, watts);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetTdp { index, watts });
        Ok(())
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.perf_profile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
        *self.perf_profile.lock().unwrap_or_else(|e| e.into_inner()) = Some(profile_code);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
            .unwrap_or_else(|e| e.into_inner())
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
            .unwrap_or_else(|e| e.into_inner())
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
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetWebcam(enabled));
        Ok(())
    }

    fn read_tcc_offset(&self) -> Result<u8, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self.tcc_offset.load(Ordering::SeqCst))
    }

    fn set_tcc_offset(&self, offset_c: u8) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        if offset_c > crate::uwcc::MAX_TCC_OFFSET_C {
            return Err(PlatformError::InvalidArgument(
                "CPU TCC offset must be 0..=63 C",
            ));
        }
        self.tcc_offset.store(offset_c, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetTccOffset(offset_c));
        Ok(())
    }

    fn set_pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        if level > 3 || (!enable && level != 0) {
            return Err(PlatformError::InvalidArgument(
                "PWUP level must be 0..=3 (and 0 when disabled)",
            ));
        }
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetPwup { enable, level });
        Ok(())
    }

    fn set_lightbar_power(&self, enabled: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.lightbar_power.store(enabled, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetLightbarPower(enabled));
        Ok(())
    }

    fn read_fn_lock(&self) -> Result<bool, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self.fn_lock.load(Ordering::SeqCst))
    }

    fn set_fn_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.fn_lock.store(enabled, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetFnLock(enabled));
        Ok(())
    }

    fn read_win_lock(&self) -> Result<bool, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self.win_lock.load(Ordering::SeqCst))
    }

    fn set_win_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.win_lock.store(enabled, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetWinLock(enabled));
        Ok(())
    }

    fn read_usb_sleep_charging(&self) -> Result<bool, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self.usb_sleep_charging.load(Ordering::SeqCst))
    }

    fn set_usb_sleep_charging(&self, enabled: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.usb_sleep_charging.store(enabled, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetUsbSleepCharging(enabled));
        Ok(())
    }

    fn read_charge_thresholds(&self) -> Result<(u8, u8), PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(*self
            .charge_thresholds
            .lock()
            .unwrap_or_else(|e| e.into_inner()))
    }

    fn set_charge_thresholds(&self, start: u8, end: u8) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        if !(crate::uwcc::MIN_CHARGE_THRESHOLD_PCT..=crate::uwcc::MAX_CHARGE_THRESHOLD_PCT)
            .contains(&start)
            || !(crate::uwcc::MIN_CHARGE_THRESHOLD_PCT..=crate::uwcc::MAX_CHARGE_THRESHOLD_PCT)
                .contains(&end)
            || start >= end
        {
            return Err(PlatformError::InvalidArgument(
                "Charge thresholds must satisfy 20 <= start < end <= 100",
            ));
        }
        *self
            .charge_thresholds
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = (start, end);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetChargeThresholds { start, end });
        Ok(())
    }

    fn set_fan_boost(&self, enabled: bool) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        self.fan_boost.store(enabled, Ordering::SeqCst);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::SetFanBoost(enabled));
        Ok(())
    }

    fn apply_ramfan_curve(&self, curve: &crate::uwcc::RamFanCurve) -> Result<(), PlatformError> {
        self.ensure_write_ready()?;
        curve.validate()?;
        *self.ramfan_curve.lock().unwrap_or_else(|e| e.into_inner()) = Some(curve.clone());
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(PlatformWrite::ApplyRamFanCurve(Box::new(curve.clone())));
        Ok(())
    }

    fn poll_wmi_event(&self) -> Result<Option<crate::uwcc::UwccWmiEvent>, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self
            .wmi_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front())
    }
}
