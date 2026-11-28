// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Rust `/dev/uwcc` platform backend (`M14` / `ADR-05`).
//!
//! Provides clean-room userspace EC/ACPI control for Uniwill chassis (specifically
//! `0x1A` / MECHREVO YAOSHI X6AR55xY): `SMRW` limits parsing, real tachometer RPM,
//! `PL1`/`PL2`/`PL4` & `TCC` offset, `cTGP`/`Dynamic Boost`, `PWUP`, `RamFan 1.5`
//! 16-point curves, charging thresholds, EC switches (`Fn Lock`, `Win Lock`,
//! lightbar AC control `0x0748`, USB sleep charging), USB RGB+IR UVC webcam
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
    MockUwccTransport, UwccDevice, UwccEcReg, UwccGpuPower, UwccInfo, UwccMuxState, UwccPwup,
    UwccSmrw, UwccTransport, UwccTransportWrite, UwccWmiEvent, UWCC_API_VERSION, UWCC_DEVICE_PATH,
    UWCC_FLAG_HAS_EC, UWCC_FLAG_HAS_HWMON, UWCC_FLAG_HAS_MUX, UWCC_FLAG_HAS_PWBT,
    UWCC_FLAG_HAS_PWDB, UWCC_FLAG_HAS_PWUP, UWCC_FLAG_HAS_SMRW, UWCC_FLAG_HAS_WMI_EVENTS,
    UWCC_GPU_POWER_OP_BASE_TGP, UWCC_GPU_POWER_OP_DYNAMIC_BOOST, UWCC_IOC_EC_READ,
    UWCC_IOC_EC_WRITE, UWCC_IOC_GET_INFO, UWCC_IOC_GPU_POWER, UWCC_IOC_MUX_GET, UWCC_IOC_MUX_SET,
    UWCC_IOC_PWUP, UWCC_IOC_SMRW, UWCC_MAGIC, UWCC_MUX_ACTIVE_DGPU, UWCC_MUX_ACTIVE_IGPU,
    UWCC_MUX_ACTIVE_UNKNOWN, UWCC_MUX_MODE_DGPU_ONLY, UWCC_MUX_MODE_DYNAMIC,
    UWCC_MUX_MODE_IGPU_ONLY, UWCC_MUX_MODE_MSHYBRID, UWCC_MUX_MODE_UNKNOWN, UWCC_NR_EC_READ,
    UWCC_NR_EC_WRITE, UWCC_NR_GET_INFO, UWCC_NR_GPU_POWER, UWCC_NR_MUX_GET, UWCC_NR_MUX_SET,
    UWCC_NR_PWUP, UWCC_NR_SMRW, UWCC_SMRW_MAX_LEN, UWCC_WMI_RING_CAPACITY,
};

/// Active display panel owner reported by ACPI `MXDS(0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MuxActiveGpu {
    Unknown,
    Igpu,
    Dgpu,
}

impl MuxActiveGpu {
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            UWCC_MUX_ACTIVE_IGPU => Self::Igpu,
            UWCC_MUX_ACTIVE_DGPU => Self::Dgpu,
            _ => Self::Unknown,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Igpu => "igpu",
            Self::Dgpu => "dgpu",
            Self::Unknown => "unknown",
        }
    }
}

/// Persistent BIOS display MUX boot mode reported/written by ACPI `MXDM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MuxBootMode {
    Unknown,
    Dynamic,
    DgpuOnly,
    IgpuOnly,
    MsHybrid,
}

impl MuxBootMode {
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            UWCC_MUX_MODE_DYNAMIC => Self::Dynamic,
            UWCC_MUX_MODE_DGPU_ONLY => Self::DgpuOnly,
            UWCC_MUX_MODE_IGPU_ONLY => Self::IgpuOnly,
            UWCC_MUX_MODE_MSHYBRID => Self::MsHybrid,
            _ => Self::Unknown,
        }
    }

    pub const fn to_raw(self) -> Option<u32> {
        match self {
            Self::Dynamic => Some(UWCC_MUX_MODE_DYNAMIC),
            Self::DgpuOnly => Some(UWCC_MUX_MODE_DGPU_ONLY),
            Self::IgpuOnly => Some(UWCC_MUX_MODE_IGPU_ONLY),
            Self::MsHybrid => Some(UWCC_MUX_MODE_MSHYBRID),
            Self::Unknown => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::DgpuOnly => "dgpu_only",
            Self::IgpuOnly => "igpu_only",
            Self::MsHybrid => "mshybrid",
            Self::Unknown => "unknown",
        }
    }
}

/// Combined hardware display MUX telemetry (`MXDS(0)` + `MXDM(0)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxState {
    pub active_gpu: MuxActiveGpu,
    pub boot_mode: MuxBootMode,
}
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

/// Byte length of the native `INOU0000` `SMRW` power table (`AX20`/`AX90`/`BX90` = 12 bytes).
pub const SMRW_INOU0000_LEN: u16 = 12;

/// Byte length of the legacy extended `SMRW` power limits block (5 pairs of `u16` = 20 bytes).
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

    /// GPU tachometer RPM low byte (`0x46D`) and high byte (`0x46C`, upstream `EC_ADDR_SECOND_FAN_RPM_1/2`).
    pub const GPU_FAN_RPM_LO: u16 = 0x046D;
    pub const GPU_FAN_RPM_HI: u16 = 0x046C;

    /// Live fan duty readback registers (`0..=200` raw).
    pub const CPU_FAN_DUTY_READ: u16 = 0x075B;
    pub const GPU_FAN_DUTY_READ: u16 = 0x075C;

    /// Manual fan control register (`0x0751` `EC_ADDR_MANUAL_FAN_CTRL`, `BIT(7)` = `FAN_MODE_USER`).
    pub const FAN_MANUAL_MODE: u16 = 0x0751;
    pub const FAN_MODE_USER_BIT: u8 = 1 << 7;
    pub const CPU_FAN_DUTY_WRITE: u16 = 0x1804;
    pub const GPU_FAN_DUTY_WRITE: u16 = 0x1809;

    /// Performance mode & software override registers.
    pub const PERF_MODE: u16 = 0x0752;
    pub const MODE_ENABLE: u16 = 0x0753;

    /// Read-only chassis project identifier (`0x1A` on MECHREVO X6AR55xY).
    pub const PROJECT_ID: u16 = 0x0740;

    /// Power and thermal limit registers (aligned with upstream Linux `uniwill-acpi.c`).
    pub const PL1_WATTS: u16 = 0x0783;
    pub const PL2_WATTS: u16 = 0x0784;
    pub const PL4_WATTS: u16 = 0x0785;
    /// Read-only EC factory TDP ceiling registers (`0xD2 = 210W` on `YAOSHI Series-X6AR55xY`).
    pub const TDP_FACTORY_PL1_MAX: u16 = 0x07A7;
    pub const TDP_FACTORY_PL2_MAX: u16 = 0x07A8;
    pub const TDP_FACTORY_PL4_MAX: u16 = 0x07A9;
    pub const TCC_OFFSET_C: u16 = 0x0743;
    pub const CTGP_WATTS: u16 = 0x0744;
    pub const DYNAMIC_BOOST_WATTS: u16 = 0x0745;

    /// EC hardware switches & battery charging control (upstream `uniwill-acpi.c`).
    /// `FN_LOCK` is `BIT(3)` of `0x07A4` (`EC_ADDR_OEM_2`), NEVER clobbering other bits.
    pub const FN_LOCK: u16 = 0x07A4;
    pub const FN_LOCK_BIT: u8 = 1 << 3;
    /// `0x0767` (`EC_ADDR_TRIGGER`) `BIT(0)` toggles Super/Win key lock; `0x0768` (`EC_ADDR_SWITCH_STATUS`) `BIT(0)` reads it.
    pub const WIN_LOCK_TRIGGER: u16 = 0x0767;
    pub const WIN_LOCK: u16 = 0x0768;
    pub const WIN_LOCK_BIT: u8 = 1 << 0;
    pub const USB_SLEEP_CHARGING: u16 = 0x0762;
    /// `0x07B9` (`EC_ADDR_CHARGE_CTRL`): bits `6..=0` (`0x7F`) hold charge end threshold percentage.
    pub const CHARGE_CTRL: u16 = 0x07B9;
    pub const CHARGE_CTRL_MASK: u8 = 0x7F;

    /// Lightbar AC control register (`0x0748` `EC_ADDR_LIGHTBAR_AC_CTRL`, `BIT(0)` = `LIGHTBAR_APP_EXISTS`).
    /// Never write `0x0767`/`0x0768` (those control `SUPER_KEY_LOCK`).
    pub const LIGHTBAR_AC_CTRL: u16 = 0x0748;
    pub const LIGHTBAR_APP_EXISTS_BIT: u8 = 1 << 0;
    /// Battery charging profile mode register (`0x07A6`, mask `0x38`: `0x08`/`0x18`/`0x28`).
    pub const BATTERY_CHARGE_MODE: u16 = 0x07A6;
    pub const BATTERY_CHARGE_MODE_MASK: u8 = 0x38;
    pub const BATTERY_MODE_HIGH_CAPACITY: u8 = 0x08;
    pub const BATTERY_MODE_BALANCED: u8 = 0x18;
    pub const BATTERY_MODE_STATIONARY: u8 = 0x28;
    /// Lid logo EC persistence registers (`0x0769..=0x076B` RGB 0..=50, `0x078C` commit `0x10`,
    /// `0x07AB` brightness/exists, `0x07C5` bit 5 logo flag).
    pub const LOGO_R: u16 = 0x0769;
    pub const LOGO_G: u16 = 0x076A;
    pub const LOGO_B: u16 = 0x076B;
    pub const LOGO_COMMIT: u16 = 0x078C;
    pub const LOGO_COMMIT_VAL: u8 = 0x10;
    pub const LOGO_BRIGHTNESS: u16 = 0x07AB;
    pub const LOGO_EXISTS_BIT: u8 = 0x01;
    pub const OEM_BYTE5: u16 = 0x07C5;
    pub const OEM_BYTE5_LOGO_BIT: u8 = 1 << 5;
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
    /// Default validated bounds for chassis `0x1A` (MECHREVO YAOSHI X6AR55xY, Arrow Lake-HX + RTX 5080 Mobile).
    pub const fn chassis_0x1a_default() -> Self {
        Self {
            pl1_min_w: 15,
            pl1_max_w: 215,
            pl2_min_w: 20,
            pl2_max_w: 225,
            pl4_min_w: 25,
            pl4_max_w: 255,
            ctgp_min_w: 55,
            ctgp_max_w: 175,
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

    /// Parses a native 12-byte `INOU0000` `SMRW` buffer (`AX20`/`AX40`/`AX60`/`AX90`/`BX90`/`G2X9`).
    ///
    /// Layout verified against `DSDT.dsl`:
    /// - `data[0]`: PL1 max (W)
    /// - `data[1]`: PL2 max (W)
    /// - `data[2]`: PL4 max (W)
    /// - `data[4]`: cTGP base floor (W) (`0` on `AX*`/`BX*`, e.g. `0x46` = 70 W on `G2X9`)
    /// - `data[5]`: Dynamic Boost max (W) (e.g. `0x19` = 25 W)
    /// - `data[7]`: Base TGP offset / max (W); `base_tgp_max = data[4] + data[7]`,
    ///   total cTGP max = `base_tgp_max + dynamic_boost_max`
    /// - `data[8..12]`: `0xFF` sentinel tail
    pub fn decode_inou0000_12byte(data: &[u8]) -> Option<Self> {
        if data.len() < usize::from(SMRW_INOU0000_LEN) || data[8..12] != [0xFF, 0xFF, 0xFF, 0xFF] {
            return None;
        }
        let pl1_max_w = i32::from(data[0]);
        let pl2_max_w = i32::from(data[1]);
        let pl4_max_w = i32::from(data[2]);
        let base_tgp_min_w = i32::from(data[4]);
        let dynamic_boost_max_w = i32::from(data[5]);
        let base_tgp_offset_w = i32::from(data[7]);
        let base_tgp_max_w = base_tgp_min_w + base_tgp_offset_w;
        let ctgp_max_w = base_tgp_max_w + dynamic_boost_max_w;

        if pl1_max_w < 15
            || pl2_max_w < 15
            || pl4_max_w < 15
            || dynamic_boost_max_w > 100
            || !(35..=250).contains(&base_tgp_max_w)
            || ctgp_max_w > i32::from(u8::MAX)
        {
            return None;
        }

        let ctgp_min_w = if base_tgp_min_w > 0 {
            base_tgp_min_w.min(ctgp_max_w)
        } else {
            55.min(ctgp_max_w)
        };

        Some(Self {
            pl1_min_w: 15,
            pl1_max_w,
            pl2_min_w: 20.min(pl2_max_w),
            pl2_max_w,
            pl4_min_w: 25.min(pl4_max_w),
            pl4_max_w,
            ctgp_min_w,
            ctgp_max_w,
            dynamic_boost_min_w: 0,
            dynamic_boost_max_w,
        })
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
        if (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return Err(PlatformError::Unsupported("EC unavailable for cTGP write"));
        }
        if !(self.smrw_limits.ctgp_min_w..=self.smrw_limits.ctgp_max_w).contains(&watts) {
            return Err(PlatformError::InvalidArgument(
                "cTGP wattage outside SMRW hardware limits",
            ));
        }
        let raw = u8::try_from(watts)
            .map_err(|_| PlatformError::InvalidArgument("Power limit out of byte range"))?;
        let prev_raw = self.transport.ec_read(uwcc_ec_reg::CTGP_WATTS).ok();
        self.transport.ec_write(uwcc_ec_reg::CTGP_WATTS, raw)?;
        if (self.info.flags & UWCC_FLAG_HAS_PWBT) != 0 {
            if let Err(err) = self
                .transport
                .set_gpu_power(UWCC_GPU_POWER_OP_BASE_TGP, u32::from(raw))
            {
                if let Some(prev) = prev_raw {
                    let _ = self.transport.ec_write(uwcc_ec_reg::CTGP_WATTS, prev);
                }
                return Err(err);
            }
        }
        Ok(())
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
        if (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return Err(PlatformError::Unsupported("EC Dynamic Boost unsupported"));
        }
        if !(self.smrw_limits.dynamic_boost_min_w..=self.smrw_limits.dynamic_boost_max_w)
            .contains(&watts)
        {
            return Err(PlatformError::InvalidArgument(
                "Dynamic Boost wattage outside SMRW hardware limits",
            ));
        }
        let raw = u8::try_from(watts)
            .map_err(|_| PlatformError::InvalidArgument("Power limit out of byte range"))?;
        let prev_raw = self
            .transport
            .ec_read(uwcc_ec_reg::DYNAMIC_BOOST_WATTS)
            .ok();
        self.transport
            .ec_write(uwcc_ec_reg::DYNAMIC_BOOST_WATTS, raw)?;
        if (self.info.flags & UWCC_FLAG_HAS_PWDB) != 0 {
            if let Err(err) = self
                .transport
                .set_gpu_power(UWCC_GPU_POWER_OP_DYNAMIC_BOOST, u32::from(raw))
            {
                if let Some(prev) = prev_raw {
                    let _ = self
                        .transport
                        .ec_write(uwcc_ec_reg::DYNAMIC_BOOST_WATTS, prev);
                }
                return Err(err);
            }
        }
        Ok(())
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
    // 1. Prefer native 12-byte INOU0000 SMRW read (zero AE_AML_BUFFER_LIMIT in kernel dmesg).
    let req12 = UwccSmrw {
        offset: SMRW_POWER_LIMITS_OFFSET,
        len: SMRW_INOU0000_LEN,
        data: [0u8; UWCC_SMRW_MAX_LEN],
    };
    if let Ok(resp) = transport.smrw(req12) {
        if let Some(mut limits) = SmrwPowerLimits::decode_inou0000_12byte(&resp.data[..12]) {
            // Merge optional EC factory ceiling registers (0x07A7..=0x07A9, e.g. 0xD2 = 210W).
            if let Ok(ec_pl1) = transport.ec_read(uwcc_ec_reg::TDP_FACTORY_PL1_MAX) {
                if (15..u8::MAX).contains(&ec_pl1) {
                    limits.pl1_max_w = limits.pl1_max_w.max(i32::from(ec_pl1));
                }
            }
            if let Ok(ec_pl2) = transport.ec_read(uwcc_ec_reg::TDP_FACTORY_PL2_MAX) {
                if (15..u8::MAX).contains(&ec_pl2) {
                    limits.pl2_max_w = limits.pl2_max_w.max(i32::from(ec_pl2));
                }
            }
            if let Ok(ec_pl4) = transport.ec_read(uwcc_ec_reg::TDP_FACTORY_PL4_MAX) {
                if (15..u8::MAX).contains(&ec_pl4) {
                    limits.pl4_max_w = limits.pl4_max_w.max(i32::from(ec_pl4));
                }
            }
            return limits;
        }
    }

    // 2. Fall back to legacy 20-byte u16-pair SMRW layout, then chassis 0x1A defaults.
    let req20 = UwccSmrw {
        offset: SMRW_POWER_LIMITS_OFFSET,
        len: SMRW_POWER_LIMITS_LEN,
        data: [0u8; UWCC_SMRW_MAX_LEN],
    };
    transport
        .smrw(req20)
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
        if temp == 0 || temp > 150 {
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
        let prev = self.transport.ec_read(uwcc_ec_reg::FAN_MANUAL_MODE)?;
        let enabled_manual = (prev & uwcc_ec_reg::FAN_MODE_USER_BIT) == 0;
        if enabled_manual {
            self.transport.ec_write(
                uwcc_ec_reg::FAN_MANUAL_MODE,
                prev | uwcc_ec_reg::FAN_MODE_USER_BIT,
            )?;
        }
        if let Err(err) = self.transport.ec_write(reg, duty) {
            if enabled_manual {
                let _ = self.transport.ec_write(uwcc_ec_reg::FAN_MANUAL_MODE, prev);
            }
            return Err(err);
        }
        Ok(())
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        self.require_available()?;
        self.ramfan.restore_firmware_auto()?;
        let prev = self.transport.ec_read(uwcc_ec_reg::FAN_MANUAL_MODE)?;
        self.transport.ec_write(
            uwcc_ec_reg::FAN_MANUAL_MODE,
            prev & !uwcc_ec_reg::FAN_MODE_USER_BIT,
        )
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
        Err(PlatformError::Unsupported(
            "CPU TCC offset is not mapped to EC 0x0743 (EC_ADDR_CTGP_DB_CTRL)",
        ))
    }

    fn set_tcc_offset(&self, offset_c: u8) -> Result<(), PlatformError> {
        self.require_available()?;
        if offset_c > MAX_TCC_OFFSET_C {
            return Err(PlatformError::InvalidArgument(
                "CPU TCC offset must be 0..=63 C",
            ));
        }
        Err(PlatformError::Unsupported(
            "CPU TCC offset is not mapped to EC 0x0743 (EC_ADDR_CTGP_DB_CTRL)",
        ))
    }

    fn set_pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
        self.require_available()?;
        self.transport.pwup(enable, level)
    }

    fn read_ctgp_limits(&self) -> Option<(i32, i32)> {
        if self.is_available()
            && (self.info.flags & UWCC_FLAG_HAS_EC) != 0
            && self.capabilities.device.ctgp_supported
        {
            Some((self.smrw_limits.ctgp_min_w, self.smrw_limits.ctgp_max_w))
        } else {
            None
        }
    }

    fn read_ctgp_watts(&self) -> Result<i32, PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return Err(PlatformError::Unsupported("EC unavailable for cTGP read"));
        }
        let raw = self.transport.ec_read(uwcc_ec_reg::CTGP_WATTS)?;
        if raw == 0 {
            return Err(PlatformError::Unavailable);
        }
        Ok(i32::from(raw))
    }

    fn set_ctgp_watts(&self, watts: i32) -> Result<(), PlatformError> {
        self.set_ctgp(watts)
    }

    fn charging_profiles_available(&self) -> Vec<String> {
        if self.is_available() && (self.info.flags & UWCC_FLAG_HAS_EC) != 0 {
            vec![
                "high_capacity".to_string(),
                "balanced".to_string(),
                "stationary".to_string(),
            ]
        } else {
            Vec::new()
        }
    }

    fn read_charging_profile(&self) -> Result<String, PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return Err(PlatformError::Unsupported(
                "EC unavailable for charging profile read",
            ));
        }
        let raw = self.transport.ec_read(uwcc_ec_reg::BATTERY_CHARGE_MODE)?;
        let mode = match raw & uwcc_ec_reg::BATTERY_CHARGE_MODE_MASK {
            uwcc_ec_reg::BATTERY_MODE_HIGH_CAPACITY => "high_capacity",
            uwcc_ec_reg::BATTERY_MODE_BALANCED => "balanced",
            uwcc_ec_reg::BATTERY_MODE_STATIONARY => "stationary",
            _ => {
                return Err(PlatformError::Unavailable);
            }
        };
        Ok(mode.to_string())
    }

    fn set_charging_profile(&self, profile: &str) -> Result<(), PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return Err(PlatformError::Unsupported(
                "EC unavailable for charging profile write",
            ));
        }
        let bits = match profile.trim() {
            "high_capacity" => uwcc_ec_reg::BATTERY_MODE_HIGH_CAPACITY,
            "balanced" => uwcc_ec_reg::BATTERY_MODE_BALANCED,
            "stationary" => uwcc_ec_reg::BATTERY_MODE_STATIONARY,
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Unknown charging profile descriptor",
                ));
            }
        };
        let prev = self.transport.ec_read(uwcc_ec_reg::BATTERY_CHARGE_MODE)?;
        let next = (prev & !uwcc_ec_reg::BATTERY_CHARGE_MODE_MASK) | bits;
        self.transport
            .ec_write(uwcc_ec_reg::BATTERY_CHARGE_MODE, next)
    }

    fn has_lid_logo_ec(&self) -> bool {
        if !self.is_available() || (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return false;
        }
        self.transport
            .ec_read(uwcc_ec_reg::LOGO_BRIGHTNESS)
            .is_ok_and(|val| (val & uwcc_ec_reg::LOGO_EXISTS_BIT) != 0)
    }

    fn set_lid_logo_ec(
        &self,
        enabled: bool,
        r: u8,
        g: u8,
        b: u8,
        brightness: u8,
    ) -> Result<(), PlatformError> {
        self.require_available()?;
        if !self.has_lid_logo_ec() {
            return Err(PlatformError::Unsupported(
                "Lid logo EC hardware not present",
            ));
        }
        let prev_ab = self.transport.ec_read(uwcc_ec_reg::LOGO_BRIGHTNESS)?;
        let preserved_low = (prev_ab & 0x1F) | uwcc_ec_reg::LOGO_EXISTS_BIT;
        if enabled && brightness > 0 {
            let scale = |v: u8| -> u8 { ((u16::from(v) * 50 + 127) / 255) as u8 };
            let br_bits = match brightness {
                1..=85 => 0x20,
                86..=170 => 0x40,
                _ => 0x80,
            };
            let oem5 = self.transport.ec_read(uwcc_ec_reg::OEM_BYTE5)?;
            self.transport.ec_write(
                uwcc_ec_reg::OEM_BYTE5,
                oem5 | uwcc_ec_reg::OEM_BYTE5_LOGO_BIT,
            )?;
            self.transport.ec_write(uwcc_ec_reg::LOGO_R, scale(r))?;
            self.transport.ec_write(uwcc_ec_reg::LOGO_G, scale(g))?;
            self.transport.ec_write(uwcc_ec_reg::LOGO_B, scale(b))?;
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_BRIGHTNESS, preserved_low | br_bits)?;
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_COMMIT, uwcc_ec_reg::LOGO_COMMIT_VAL)
        } else {
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_BRIGHTNESS, preserved_low)?;
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_COMMIT, uwcc_ec_reg::LOGO_COMMIT_VAL)
        }
    }

    fn set_lightbar_power(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        let prev = self.transport.ec_read(uwcc_ec_reg::LIGHTBAR_AC_CTRL)?;
        let next = if enabled {
            prev | uwcc_ec_reg::LIGHTBAR_APP_EXISTS_BIT
        } else {
            prev & !uwcc_ec_reg::LIGHTBAR_APP_EXISTS_BIT
        };
        self.transport.ec_write(uwcc_ec_reg::LIGHTBAR_AC_CTRL, next)
    }

    fn read_fn_lock(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        Ok((self.transport.ec_read(uwcc_ec_reg::FN_LOCK)? & uwcc_ec_reg::FN_LOCK_BIT) != 0)
    }

    fn set_fn_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        let prev = self.transport.ec_read(uwcc_ec_reg::FN_LOCK)?;
        let next = if enabled {
            prev | uwcc_ec_reg::FN_LOCK_BIT
        } else {
            prev & !uwcc_ec_reg::FN_LOCK_BIT
        };
        self.transport.ec_write(uwcc_ec_reg::FN_LOCK, next)
    }

    fn read_win_lock(&self) -> Result<bool, PlatformError> {
        self.require_available()?;
        Ok((self.transport.ec_read(uwcc_ec_reg::WIN_LOCK)? & uwcc_ec_reg::WIN_LOCK_BIT) != 0)
    }

    fn set_win_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_available()?;
        let currently_locked = self.read_win_lock()?;
        if currently_locked == enabled {
            return Ok(());
        }
        let prev_trigger = self.transport.ec_read(uwcc_ec_reg::WIN_LOCK_TRIGGER)?;
        self.transport.ec_write(
            uwcc_ec_reg::WIN_LOCK_TRIGGER,
            prev_trigger | uwcc_ec_reg::WIN_LOCK_BIT,
        )
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
        let raw = self.transport.ec_read(uwcc_ec_reg::CHARGE_CTRL)?;
        let end = raw & uwcc_ec_reg::CHARGE_CTRL_MASK;
        if !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&end) {
            return Err(PlatformError::InvalidArgument(
                "EC reported invalid charge threshold",
            ));
        }
        let start = end.saturating_sub(5).max(MIN_CHARGE_THRESHOLD_PCT);
        let start = if start >= end {
            end.saturating_sub(1)
        } else {
            start
        };
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
        let prev = self.transport.ec_read(uwcc_ec_reg::CHARGE_CTRL)?;
        let next = (prev & !uwcc_ec_reg::CHARGE_CTRL_MASK) | (end & uwcc_ec_reg::CHARGE_CTRL_MASK);
        self.transport.ec_write(uwcc_ec_reg::CHARGE_CTRL, next)
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

    fn read_mux_state(&self) -> Result<MuxState, PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_MUX) == 0 {
            return Err(PlatformError::Unsupported("Display MUX unsupported"));
        }
        let raw = self.transport.mux_get()?;
        Ok(MuxState {
            active_gpu: MuxActiveGpu::from_raw(raw.active_gpu),
            boot_mode: MuxBootMode::from_raw(raw.boot_mode),
        })
    }

    fn set_mux_boot_mode(&self, mode: MuxBootMode) -> Result<(), PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_MUX) == 0 {
            return Err(PlatformError::Unsupported("Display MUX unsupported"));
        }
        let raw = mode.to_raw().ok_or(PlatformError::InvalidArgument(
            "MUX boot mode cannot be Unknown",
        ))?;
        self.transport.mux_set(raw)
    }
}
