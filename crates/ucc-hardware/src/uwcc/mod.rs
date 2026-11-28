// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Rust `/dev/uwcc` platform backend (`M14` / `ADR-05`).
//!
//! Provides clean-room userspace EC/ACPI control for Uniwill chassis (specifically
//! `0x1A` / MECHREVO YAOSHI X6AR55xY): `SMRW` limits parsing, real tachometer RPM,
//! `PL1`/`PL2`/`PL4` & `TCC` offset, `cTGP`/`Dynamic Boost`, `PWUP`, `RamFan 1.5`
//! 16-point curves, charging thresholds, EC switches (`Fn Lock`, `Win Lock`,
//! lightbar power `0x767`/`0x768`, USB sleep charging), USB RGB+IR UVC webcam
//! toggle over [`Sysfs`], and WMI-to-uinput hotkey event translation.

pub mod ramfan;
pub mod uapi;
pub mod webcam;
pub mod wmi_events;

pub use ramfan::{
    ec_duty_to_percent, percent_to_ec_duty, RamFanController, RamFanCurve, RamFanPoint,
    RAMFAN_BASE, RAMFAN_END, RAMFAN_MAX_TEMP_C, RAMFAN_POINTS,
};
pub use uapi::{
    MockUwccTransport, UwccDevice, UwccEcReg, UwccInfo, UwccPwup, UwccSmrw, UwccTransport,
    UwccTransportWrite, UwccWmiEvent, UWCC_API_VERSION, UWCC_DEVICE_PATH, UWCC_FLAG_HAS_EC,
    UWCC_FLAG_HAS_HWMON, UWCC_FLAG_HAS_PWUP, UWCC_FLAG_HAS_SMRW, UWCC_FLAG_HAS_WMI_EVENTS,
    UWCC_IOC_EC_READ, UWCC_IOC_EC_WRITE, UWCC_IOC_GET_INFO, UWCC_IOC_PWUP, UWCC_IOC_SMRW,
    UWCC_MAGIC, UWCC_NR_EC_READ, UWCC_NR_EC_WRITE, UWCC_NR_GET_INFO, UWCC_NR_PWUP, UWCC_NR_SMRW,
    UWCC_SMRW_MAX_LEN, UWCC_WMI_RING_CAPACITY,
};
pub use webcam::{UsbWebcamController, UsbWebcamDevice, USB_CLASS_VIDEO, USB_DEVICES_ROOT};
#[cfg(unix)]
pub use wmi_events::LinuxUinputEmitter;
pub use wmi_events::{
    hotkey_to_linux_keycode, linux_key, translate_wmi_event, wmi_code, MockUinputEmitter,
    UinputEmitter, UwccHotkeyEvent, UINPUT_DEVICE_PATH,
};

use crate::detection::{
    identify_device, is_water_cooler_supported, DeviceCapabilities, DeviceVendor, DmiInfo,
};
use crate::platform::{PlatformBackend, PlatformBackendKind, PlatformCapabilities, PlatformError};
use crate::traits::Sysfs;
use std::fmt;
use std::sync::Arc;

/// Chassis project ID for MECHREVO YAOSHI X6AR55xY (`0x1A`).
pub const CHASSIS_PROJECT_0X1A: u32 = 0x1A;

/// Byte offset into the ACPI `SMRW` table where factory power limits reside.
pub const SMRW_POWER_LIMITS_OFFSET: u16 = 0;

/// Byte length of the factory power limits block read from `SMRW` (5 pairs of `u16` = 20 bytes).
pub const SMRW_POWER_LIMITS_LEN: u16 = 20;

/// Maximum physically plausible fan tachometer reading (RPM).
pub const MAX_SANE_FAN_RPM: u32 = 15_000;

/// Maximum allowed CPU TCC temperature offset in degrees Celsius (`0..=63`).
pub const MAX_TCC_OFFSET_C: u8 = 63;

/// Minimum allowed battery charge threshold percentage (`20..=100`).
pub const MIN_CHARGE_THRESHOLD_PCT: u8 = 20;

/// Maximum allowed battery charge threshold percentage (`20..=100`).
pub const MAX_CHARGE_THRESHOLD_PCT: u8 = 100;

/// Uniwill `0x1A` EC register addresses accessed via `/dev/uwcc` (`ECRR`/`ECRW`).
pub mod uwcc_ec_reg {
    pub const CPU_TEMP_C: u16 = 0x043E;
    pub const GPU_TEMP_C: u16 = 0x044F;

    /// CPU tachometer RPM high byte (`0x464`) and low byte (`0x465`).
    pub const CPU_FAN_RPM_HI: u16 = 0x0464;
    pub const CPU_FAN_RPM_LO: u16 = 0x0465;

    /// GPU tachometer RPM low byte (`0x46B`) and high byte (`0x46C`).
    pub const GPU_FAN_RPM_LO: u16 = 0x046B;
    pub const GPU_FAN_RPM_HI: u16 = 0x046C;

    /// Live fan duty readback registers (`0..=200` raw).
    pub const CPU_FAN_DUTY_READ: u16 = 0x075B;
    pub const GPU_FAN_DUTY_READ: u16 = 0x075C;

    /// Manual fan control registers.
    pub const FAN_MANUAL_MODE: u16 = 0x0751;
    pub const CPU_FAN_DUTY_WRITE: u16 = 0x1804;
    pub const GPU_FAN_DUTY_WRITE: u16 = 0x1809;

    /// Performance mode & software override registers.
    pub const PERF_MODE: u16 = 0x0752;
    pub const MODE_ENABLE: u16 = 0x0753;

    /// Power and thermal limit registers.
    pub const PL1_WATTS: u16 = 0x0740;
    pub const PL2_WATTS: u16 = 0x0741;
    pub const PL4_WATTS: u16 = 0x0742;
    pub const TCC_OFFSET_C: u16 = 0x0743;
    pub const CTGP_WATTS: u16 = 0x0744;
    pub const DYNAMIC_BOOST_WATTS: u16 = 0x0745;

    /// EC hardware switches & battery charging thresholds.
    pub const FN_LOCK: u16 = 0x0760;
    pub const WIN_LOCK: u16 = 0x0761;
    pub const USB_SLEEP_CHARGING: u16 = 0x0762;
    pub const CHARGE_START_PCT: u16 = 0x0765;
    pub const CHARGE_END_PCT: u16 = 0x0766;

    /// Lightbar / chassis LED EC power rails (`0x767`/`0x768`).
    pub const LIGHTBAR_POWER_PRIMARY: u16 = 0x0767;
    pub const LIGHTBAR_POWER_SECONDARY: u16 = 0x0768;
}

/// Configured backend selection policy (`Auto`, `Uwcc`, or `TuxedoIo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlatformBackendSelection {
    /// Prefer `/dev/uwcc` when available, otherwise fall back to `/dev/tuxedo_io`.
    #[default]
    Auto,
    /// Force native `/dev/uwcc` backend.
    Uwcc,
    /// Force legacy `/dev/tuxedo_io` backend.
    TuxedoIo,
}

impl PlatformBackendSelection {
    /// Returns the canonical lowercase config representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Uwcc => "uwcc",
            Self::TuxedoIo => "tuxedo_io",
        }
    }
}

impl fmt::Display for PlatformBackendSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parses a user/config string (`"auto"`, `"uwcc"`, `"tuxedo_io"`, `"tuxedo-io"`) into
/// [`PlatformBackendSelection`].
pub fn parse_backend_selection(raw: &str) -> Result<PlatformBackendSelection, PlatformError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(PlatformBackendSelection::Auto),
        "uwcc" => Ok(PlatformBackendSelection::Uwcc),
        "tuxedo_io" | "tuxedo-io" | "tuxedoio" => Ok(PlatformBackendSelection::TuxedoIo),
        _ => Err(PlatformError::InvalidArgument(
            "Backend selection must be 'auto', 'uwcc', or 'tuxedo_io'",
        )),
    }
}

/// Factory power limits parsed from the chassis `SMRW` byte table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmrwPowerLimits {
    pub pl1_min_w: i32,
    pub pl1_max_w: i32,
    pub pl2_min_w: i32,
    pub pl2_max_w: i32,
    pub pl4_min_w: i32,
    pub pl4_max_w: i32,
    pub ctgp_min_w: i32,
    pub ctgp_max_w: i32,
    pub dynamic_boost_min_w: i32,
    pub dynamic_boost_max_w: i32,
}

impl SmrwPowerLimits {
    /// Default validated bounds for chassis `0x1A` (MECHREVO YAOSHI X6AR55xY).
    pub const fn chassis_0x1a_default() -> Self {
        Self {
            pl1_min_w: 15,
            pl1_max_w: 90,
            pl2_min_w: 20,
            pl2_max_w: 125,
            pl4_min_w: 25,
            pl4_max_w: 160,
            ctgp_min_w: 55,
            ctgp_max_w: 140,
            dynamic_boost_min_w: 0,
            dynamic_boost_max_w: 25,
        }
    }

    /// Encodes the limits table into a 64-byte `UwccSmrw::data` buffer (little-endian `u16` pairs).
    pub fn encode(&self) -> [u8; UWCC_SMRW_MAX_LEN] {
        let mut buf = [0u8; UWCC_SMRW_MAX_LEN];
        let pairs = [
            (self.pl1_min_w, self.pl1_max_w),
            (self.pl2_min_w, self.pl2_max_w),
            (self.pl4_min_w, self.pl4_max_w),
            (self.ctgp_min_w, self.ctgp_max_w),
            (self.dynamic_boost_min_w, self.dynamic_boost_max_w),
        ];
        for (idx, &(min_w, max_w)) in pairs.iter().enumerate() {
            let base = idx * 4;
            let lo = min_w.clamp(0, i32::from(u16::MAX)) as u16;
            let hi = max_w.clamp(0, i32::from(u16::MAX)) as u16;
            buf[base..base + 2].copy_from_slice(&lo.to_le_bytes());
            buf[base + 2..base + 4].copy_from_slice(&hi.to_le_bytes());
        }
        buf
    }

    /// Parses and validates a 64-byte `UwccSmrw::data` buffer into [`SmrwPowerLimits`].
    pub fn decode(data: &[u8; UWCC_SMRW_MAX_LEN]) -> Option<Self> {
        let read_pair = |idx: usize| -> (i32, i32) {
            let base = idx * 4;
            let lo = u16::from_le_bytes([data[base], data[base + 1]]);
            let hi = u16::from_le_bytes([data[base + 2], data[base + 3]]);
            (i32::from(lo), i32::from(hi))
        };

        let (pl1_min_w, pl1_max_w) = read_pair(0);
        let (pl2_min_w, pl2_max_w) = read_pair(1);
        let (pl4_min_w, pl4_max_w) = read_pair(2);
        let (ctgp_min_w, ctgp_max_w) = read_pair(3);
        let (dynamic_boost_min_w, dynamic_boost_max_w) = read_pair(4);

        if pl1_min_w <= 0
            || pl1_min_w > pl1_max_w
            || pl2_min_w <= 0
            || pl2_min_w > pl2_max_w
            || pl4_min_w <= 0
            || pl4_min_w > pl4_max_w
            || ctgp_min_w <= 0
            || ctgp_min_w > ctgp_max_w
            || dynamic_boost_min_w < 0
            || dynamic_boost_min_w > dynamic_boost_max_w
        {
            return None;
        }

        Some(Self {
            pl1_min_w,
            pl1_max_w,
            pl2_min_w,
            pl2_max_w,
            pl4_min_w,
            pl4_max_w,
            ctgp_min_w,
            ctgp_max_w,
            dynamic_boost_min_w,
            dynamic_boost_max_w,
        })
    }

    /// Returns `(min, max)` TDP wattage bounds for channel `0` (PL1), `1` (PL2), or `2` (PL4).
    pub fn tdp_bounds(&self, index: usize) -> Option<(i32, i32)> {
        match index {
            0 => Some((self.pl1_min_w, self.pl1_max_w)),
            1 => Some((self.pl2_min_w, self.pl2_max_w)),
            2 => Some((self.pl4_min_w, self.pl4_max_w)),
            _ => None,
        }
    }
}

/// Native `/dev/uwcc` [`PlatformBackend`] implementation for Uniwill chassis (`0x1A`).
pub struct UwccBackend {
    transport: Arc<dyn UwccTransport>,
    ramfan: RamFanController,
    webcam: Option<UsbWebcamController>,
    info: UwccInfo,
    smrw_limits: SmrwPowerLimits,
    capabilities: PlatformCapabilities,
}

impl UwccBackend {
    /// Creates a new [`UwccBackend`] over `transport`, optional [`DmiInfo`], and optional
    /// [`Sysfs`] handle used for combined USB RGB+IR UVC webcam toggling.
    pub fn new(
        transport: Arc<dyn UwccTransport>,
        dmi: Option<&DmiInfo>,
        sysfs: Option<Arc<dyn Sysfs>>,
    ) -> Self {
        let info = transport.get_info().unwrap_or_default();
        let smrw_limits = probe_smrw_limits(transport.as_ref());
        let webcam = sysfs.map(UsbWebcamController::new);
        let has_webcam_sysfs = webcam.is_some();

        let device_id = dmi.and_then(|d| identify_device(d, None));
        let water_cooler_supported = (info.flags & UWCC_FLAG_HAS_PWUP) != 0
            || info.project_id == CHASSIS_PROJECT_0X1A
            || is_water_cooler_supported(device_id);

        let capabilities = PlatformCapabilities {
            backend: PlatformBackendKind::Uwcc,
            device: DeviceCapabilities {
                vendor: DeviceVendor::Uniwill,
                device_id,
                number_of_fans: 2,
                fans_min_speed: 20,
                fans_off_available: true,
                fan_control: true,
                tdp_control: true,
                charging_thresholds: true,
                ctgp_supported: true,
                water_cooler_supported,
                webcam_switch: has_webcam_sysfs,
                ..DeviceCapabilities::default()
            },
            tdp_channels: 3,
            perf_profiles_available: 3,
        };

        let ramfan = RamFanController::new(Arc::clone(&transport));

        Self {
            transport,
            ramfan,
            webcam,
            info,
            smrw_limits,
            capabilities,
        }
    }

    /// Returns cached driver and chassis info from `UWCC_IOC_GET_INFO`.
    pub fn info(&self) -> UwccInfo {
        self.info
    }

    /// Returns parsed `SMRW` factory power limits for this chassis.
    pub fn smrw_limits(&self) -> SmrwPowerLimits {
        self.smrw_limits
    }

    /// Borrows the underlying [`RamFanController`].
    pub fn ramfan(&self) -> &RamFanController {
        &self.ramfan
    }

    /// Returns `(min, max)` GPU cTGP wattage limits from `SMRW`.
    pub fn read_ctgp_limits(&self) -> (i32, i32) {
        (self.smrw_limits.ctgp_min_w, self.smrw_limits.ctgp_max_w)
    }

    /// Sets GPU cTGP wattage after enforcing `SMRW` bounds.
    pub fn set_ctgp(&self, watts: i32) -> Result<(), PlatformError> {
        self.require_available()?;
        if !(self.smrw_limits.ctgp_min_w..=self.smrw_limits.ctgp_max_w).contains(&watts) {
            return Err(PlatformError::InvalidArgument(
                "cTGP wattage outside SMRW hardware limits",
            ));
        }
        let raw = u8::try_from(watts)
            .map_err(|_| PlatformError::InvalidArgument("Power limit out of byte range"))?;
        self.transport.ec_write(uwcc_ec_reg::CTGP_WATTS, raw)
    }

    /// Returns `(min, max)` GPU Dynamic Boost wattage limits from `SMRW`.
    pub fn read_dynamic_boost_limits(&self) -> (i32, i32) {
        (
            self.smrw_limits.dynamic_boost_min_w,
            self.smrw_limits.dynamic_boost_max_w,
        )
    }

    /// Sets GPU Dynamic Boost wattage after enforcing `SMRW` bounds.
    pub fn set_dynamic_boost(&self, watts: i32) -> Result<(), PlatformError> {
        self.require_available()?;
        if !(self.smrw_limits.dynamic_boost_min_w..=self.smrw_limits.dynamic_boost_max_w)
            .contains(&watts)
        {
            return Err(PlatformError::InvalidArgument(
                "Dynamic Boost wattage outside SMRW hardware limits",
            ));
        }
        let raw = u8::try_from(watts)
            .map_err(|_| PlatformError::InvalidArgument("Power limit out of byte range"))?;
        self.transport
            .ec_write(uwcc_ec_reg::DYNAMIC_BOOST_WATTS, raw)
    }

    fn require_available(&self) -> Result<(), PlatformError> {
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(())
    }

    fn tdp_reg(index: usize) -> Result<u16, PlatformError> {
        match index {
            0 => Ok(uwcc_ec_reg::PL1_WATTS),
            1 => Ok(uwcc_ec_reg::PL2_WATTS),
            2 => Ok(uwcc_ec_reg::PL4_WATTS),
            _ => Err(PlatformError::InvalidArgument(
                "Uniwill TDP channel index must be 0..=2",
            )),
        }
    }
}

fn probe_smrw_limits(transport: &dyn UwccTransport) -> SmrwPowerLimits {
    if !transport.is_available() {
        return SmrwPowerLimits::chassis_0x1a_default();
    }
    let req = UwccSmrw {
        offset: SMRW_POWER_LIMITS_OFFSET,
        len: SMRW_POWER_LIMITS_LEN,
        data: [0u8; UWCC_SMRW_MAX_LEN],
    };
    transport
        .smrw(req)
        .ok()
        .and_then(|resp| SmrwPowerLimits::decode(&resp.data))
        .unwrap_or_else(SmrwPowerLimits::chassis_0x1a_default)
}

/// Combines tachometer high/low bytes into RPM with sane upper bound validation.
pub fn decode_tachometer_rpm(hi: u8, lo: u8) -> Result<u32, PlatformError> {
    let raw = u32::from(u16::from_be_bytes([hi, lo]));
    if raw > MAX_SANE_FAN_RPM {
        return Err(PlatformError::InvalidArgument(
            "EC fan tachometer reading outside sane RPM bounds",
        ));
    }
    Ok(raw)
}

impl PlatformBackend for UwccBackend {
    fn kind(&self) -> PlatformBackendKind {
        PlatformBackendKind::Uwcc
    }

    fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities.clone()
    }

    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        self.require_available()?;
        let reg = match fan_idx {
            0 => uwcc_ec_reg::CPU_TEMP_C,
            1 => uwcc_ec_reg::GPU_TEMP_C,
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
                ))
            }
        };
        let temp = i32::from(self.transport.ec_read(reg)?);
        if temp == 0 {
            return Err(PlatformError::SensorUnavailable(fan_idx));
        }
        Ok(temp)
    }

    fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
        self.require_available()?;
        let reg = match fan_idx {
            0 => uwcc_ec_reg::CPU_FAN_DUTY_READ,
            1 => uwcc_ec_reg::GPU_FAN_DUTY_READ,
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
                ))
            }
        };
        let raw = self.transport.ec_read(reg)?;
        Ok(ec_duty_to_percent(raw))
    }

    fn read_fan_rpm(&self, fan_idx: usize) -> Result<u32, PlatformError> {
        self.require_available()?;
        let (hi_reg, lo_reg) = match fan_idx {
            0 => (uwcc_ec_reg::CPU_FAN_RPM_HI, uwcc_ec_reg::CPU_FAN_RPM_LO),
            1 => (uwcc_ec_reg::GPU_FAN_RPM_HI, uwcc_ec_reg::GPU_FAN_RPM_LO),
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
                ))
            }
        };
        let hi = self.transport.ec_read(hi_reg)?;
        let lo = self.transport.ec_read(lo_reg)?;
        decode_tachometer_rpm(hi, lo)
    }

    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        self.require_available()?;
        if percent > 100 {
            return Err(PlatformError::InvalidArgument(
                "Fan speed percentage must be 0..=100",
            ));
        }
        let reg = match fan_idx {
            0 => uwcc_ec_reg::CPU_FAN_DUTY_WRITE,
            1 => uwcc_ec_reg::GPU_FAN_DUTY_WRITE,
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
                ))
            }
        };
        let duty = percent_to_ec_duty(percent);
        self.transport.ec_write(uwcc_ec_reg::FAN_MANUAL_MODE, 1)?;
        self.transport.ec_write(reg, duty)
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        self.require_available()?;
        self.ramfan.restore_firmware_auto()?;
        self.transport.ec_write(uwcc_ec_reg::FAN_MANUAL_MODE, 0)
    }

    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
        self.require_available()?;
        let reg = Self::tdp_reg(index)?;
        Ok(i32::from(self.transport.ec_read(reg)?))
    }

    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
        self.require_available()?;
        self.smrw_limits
            .tdp_bounds(index)
            .ok_or(PlatformError::InvalidArgument(
                "Uniwill TDP channel index must be 0..=2",
            ))
    }

    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
        self.require_available()?;
        let (min_w, max_w) = self.read_tdp_limits(index)?;
        if !(min_w..=max_w).contains(&watts) {
            return Err(PlatformError::InvalidArgument(
                "TDP wattage outside SMRW hardware limits",
            ));
        }
        let reg = Self::tdp_reg(index)?;
        let raw = u8::try_from(watts)
            .map_err(|_| PlatformError::InvalidArgument("Power limit out of byte range"))?;
        self.transport.ec_write(reg, raw)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.require_available()?;
        let code = i32::from(self.transport.ec_read(uwcc_ec_reg::PERF_MODE)?);
        if !(1..=3).contains(&code) {
            return Err(PlatformError::Unavailable);
        }
        Ok(code)
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        self.require_available()?;
        if !(1..=3).contains(&profile_code) {
            return Err(PlatformError::InvalidArgument(
                "Uniwill performance profile code must be 1..=3",
            ));
        }
        self.transport
            .ec_write(uwcc_ec_reg::PERF_MODE, profile_code as u8)
    }

    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        self.transport
            .ec_write(uwcc_ec_reg::MODE_ENABLE, u8::from(enable))
    }

    fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
        self.require_available()?;
        if !(0..=255).contains(&mode) {
            return Err(PlatformError::InvalidArgument(
                "Uniwill mode must be in 0..=255",
            ));
        }
        self.transport.ec_write(uwcc_ec_reg::PERF_MODE, mode as u8)
    }

    fn read_webcam(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        let controller = self.webcam.as_ref().ok_or(PlatformError::Unsupported(
            "Webcam sysfs controller not configured",
        ))?;
        controller.is_enabled()
    }

    fn set_webcam(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        let controller = self.webcam.as_ref().ok_or(PlatformError::Unsupported(
            "Webcam sysfs controller not configured",
        ))?;
        controller.set_enabled(enabled)
    }

    fn read_tcc_offset(&self) -> Result<u8, PlatformError> {
        self.require_available()?;
        let offset = self.transport.ec_read(uwcc_ec_reg::TCC_OFFSET_C)?;
        Ok(offset.min(MAX_TCC_OFFSET_C))
    }

    fn set_tcc_offset(&self, offset_c: u8) -> Result<(), PlatformError> {
        self.require_available()?;
        if offset_c > MAX_TCC_OFFSET_C {
            return Err(PlatformError::InvalidArgument(
                "CPU TCC offset must be 0..=63 C",
            ));
        }
        self.transport.ec_write(uwcc_ec_reg::TCC_OFFSET_C, offset_c)
    }

    fn set_pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
        self.require_available()?;
        self.transport.pwup(enable, level)
    }

    fn set_lightbar_power(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        let val = u8::from(enabled);
        self.transport
            .ec_write(uwcc_ec_reg::LIGHTBAR_POWER_PRIMARY, val)?;
        self.transport
            .ec_write(uwcc_ec_reg::LIGHTBAR_POWER_SECONDARY, val)
    }

    fn read_fn_lock(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        Ok(self.transport.ec_read(uwcc_ec_reg::FN_LOCK)? != 0)
    }

    fn set_fn_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        self.transport
            .ec_write(uwcc_ec_reg::FN_LOCK, u8::from(enabled))
    }

    fn read_win_lock(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        Ok(self.transport.ec_read(uwcc_ec_reg::WIN_LOCK)? != 0)
    }

    fn set_win_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        self.transport
            .ec_write(uwcc_ec_reg::WIN_LOCK, u8::from(enabled))
    }

    fn read_usb_sleep_charging(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        Ok(self.transport.ec_read(uwcc_ec_reg::USB_SLEEP_CHARGING)? != 0)
    }

    fn set_usb_sleep_charging(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        self.transport
            .ec_write(uwcc_ec_reg::USB_SLEEP_CHARGING, u8::from(enabled))
    }

    fn read_charge_thresholds(&self) -> Result<(u8, u8), PlatformError> {
        self.require_available()?;
        let start = self.transport.ec_read(uwcc_ec_reg::CHARGE_START_PCT)?;
        let end = self.transport.ec_read(uwcc_ec_reg::CHARGE_END_PCT)?;
        if !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&start)
            || !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&end)
            || start >= end
        {
            return Err(PlatformError::InvalidArgument(
                "EC reported invalid charge thresholds",
            ));
        }
        Ok((start, end))
    }

    fn set_charge_thresholds(&self, start: u8, end: u8) -> Result<(), PlatformError> {
        self.require_available()?;
        if !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&start)
            || !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&end)
            || start >= end
        {
            return Err(PlatformError::InvalidArgument(
                "Charge thresholds must satisfy 20 <= start < end <= 100",
            ));
        }
        self.transport
            .ec_write(uwcc_ec_reg::CHARGE_START_PCT, start)?;
        self.transport.ec_write(uwcc_ec_reg::CHARGE_END_PCT, end)
    }

    fn set_fan_boost(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        self.ramfan.set_fan_boost(enabled)
    }

    fn apply_ramfan_curve(&self, curve: &RamFanCurve) -> Result<(), PlatformError> {
        self.require_available()?;
        self.ramfan.apply_curve(curve)
    }

    fn poll_wmi_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
        self.require_available()?;
        self.transport.poll_event()
    }
}
