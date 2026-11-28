// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Rust `/dev/uwcc` platform backend (`M14` / `ADR-05`).
//!
//! Provides clean-room userspace EC/ACPI control for Uniwill chassis (specifically
//! `0x1A` / MECHREVO YAOSHI X6AR55xY): `SMRW` limits parsing, tear-free tachometer RPM,
//! `PL1`/`PL2`/`PL4` (including `0x0727 BIT(7)` `pl4_doubled` scaling) & `TCC` offset
//! (`0x0786`/`0x0463`), `cTGP`/`Dynamic Boost` (`0x0743..=0x0746`, live `0x0788`/`0x07D5`),
//! `PWUP`, `RamFan 1.5` 96-byte curves (`0x0F00..=0x0F5F`), charging thresholds (`0x07D0`/`0x07B9`)
//! & battery profiles (`0x07A6`), EC switches (`Fn Lock` `0x074E BIT(4)`, `Win Lock`,
//! lightbar AC control `0x0748`, USB sleep charging `0x0767 BIT(4)`), USB RGB+IR UVC webcam
//! toggle over [`Sysfs`], and WMI-to-uinput hotkey event translation.

pub mod ramfan;
pub mod uapi;
pub mod webcam;
pub mod wmi_events;

pub use ramfan::{
    ec_duty_to_percent, encode_fan_table, percent_to_ec_duty, EncodedRamFanTable, RamFanController,
    RamFanCurve, RamFanPoint, RAMFAN_BASE, RAMFAN_END, RAMFAN_HYSTERESIS_C, RAMFAN_MAX_TEMP_C,
    RAMFAN_PADDING_TEMP, RAMFAN_POINTS,
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

/// Maximum allowed CPU TCC temperature offset in degrees Celsius (`0..=40`).
pub const MAX_TCC_OFFSET_C: u8 = 40;

/// Minimum allowed battery charge threshold percentage (`20..=100`).
pub const MIN_CHARGE_THRESHOLD_PCT: u8 = 20;

/// Maximum allowed battery charge threshold percentage (`20..=100`).
pub const MAX_CHARGE_THRESHOLD_PCT: u8 = 100;

/// Uniwill `0x1A` EC register addresses accessed via `/dev/uwcc` (`ECRR`/`ECRW`).
pub mod uwcc_ec_reg {
    pub const CPU_TEMP_C: u16 = 0x043E;
    pub const GPU_TEMP_C: u16 = 0x044F;

    /// Read-only CPU TCC offset readback (`0x0463`, bits `6..=0` = offset °C from TjMax).
    pub const TCC_OFFSET_READ: u16 = 0x0463;
    pub const TCC_OFFSET_MASK: u8 = 0x7F;

    /// CPU tachometer RPM high byte (`0x464`) and low byte (`0x465`).
    pub const CPU_FAN_RPM_HI: u16 = 0x0464;
    pub const CPU_FAN_RPM_LO: u16 = 0x0465;

    /// GPU tachometer RPM low byte (`0x46D`) and high byte (`0x46C`, upstream `EC_ADDR_SECOND_FAN_RPM_1/2`).
    pub const GPU_FAN_RPM_LO: u16 = 0x046D;
    pub const GPU_FAN_RPM_HI: u16 = 0x046C;

    /// Live fan duty readback registers (`0..=200` raw).
    pub const CPU_FAN_DUTY_READ: u16 = 0x075B;
    pub const GPU_FAN_DUTY_READ: u16 = 0x075C;

    /// Uniwill `0x1A` EC Custom mode activation register (`0x0706`, `0x41` = active, `0x40` = inactive).
    pub const CUSTOM_MODE_0706: u16 = 0x0706;
    pub const CUSTOM_MODE_0706_ACTIVE: u8 = 0x41;

    /// Uniwill `0x1A` Custom power mode control register (`0x0726`, `BIT(7)` = Custom power loop).
    /// Always modify via RMW to preserve protection bits (`BIT(3..6)`).
    pub const CUSTOM_MODE_CTRL: u16 = 0x0726;
    pub const CUSTOM_MODE_CTRL_BIT: u8 = 1 << 7;

    /// Uniwill `0x1A` Custom mode enable register (`0x0727`, `BIT(6)` = `CUME`, white LED).
    /// WARNING: `BIT(7)` is read-only `PL4D` / `pl4_doubled` (2 W/LSB PL4 scaling) — NEVER write `BIT(7)`!
    pub const CUSTOM_MODE_ENABLE: u16 = 0x0727;
    pub const CUSTOM_MODE_CUME_BIT: u8 = 1 << 6;
    pub const CUSTOM_MODE_PL4D_BIT: u8 = 1 << 7;

    /// Manual fan control register (`0x0751` `EC_ADDR_MANUAL_FAN_CTRL`).
    /// On `0x1A`: `BIT(4)` = `TBME` (Turbo), `BIT(5)` = `HIGH`, `BIT(6)` = `BOOST`,
    /// `BIT(7)` = `UFME` (Office 45 W unless `CUME` is also active).
    pub const FAN_MANUAL_MODE: u16 = 0x0751;
    pub const FAN_MODE_TURBO_BIT: u8 = 1 << 4;
    pub const FAN_MODE_HIGH_BIT: u8 = 1 << 5;
    pub const FAN_MODE_BOOST_BIT: u8 = 1 << 6;
    pub const FAN_MODE_USER_BIT: u8 = 1 << 7;
    pub const CPU_FAN_DUTY_WRITE: u16 = 0x1804;
    pub const GPU_FAN_DUTY_WRITE: u16 = 0x1809;

    /// Legacy performance mode & software override registers (non-`0x1A` Uniwill chassis).
    /// On `0x1A`, `0x0752` is NOT a `1..=3` profile index and `0x0753` is CPU VRM current limit.
    pub const PERF_MODE: u16 = 0x0752;
    pub const MODE_ENABLE: u16 = 0x0753;

    /// Read-only chassis project identifier (`0x1A` on MECHREVO X6AR55xY).
    pub const PROJECT_ID: u16 = 0x0740;
    /// `0x0741` (`EC_ADDR_AP_OEM`): `BIT(0)` (`ENABLE_MANUAL_CTRL` / `AP_EXIST`) latches manual
    /// TDP writes (`0x0783..=0x0785`) and routes physical mode button presses to WMI `0x00B0`.
    pub const AP_OEM: u16 = 0x0741;
    pub const AP_OEM_ENABLE_MANUAL_CTRL_BIT: u8 = 1 << 0;
    /// `BIT(2)` of `0x0741`: EC activation handshake request (EC clears BIT(2) upon ACK).
    pub const AP_OEM_ACTIVATION_ACK_BIT: u8 = 1 << 2;
    /// `0x07C6` (`EC_ADDR_AP_OEM_6`): `BIT(2)` (`ENABLE_UNIVERSAL_FAN_CTRL`) latches RamFan 1.5 tables.
    pub const AP_OEM_6: u16 = 0x07C6;
    pub const AP_OEM_6_UNIVERSAL_FAN_BIT: u8 = 1 << 2;

    /// EC capability register (`0x078E` `EC_ADDR_FAN_CTRL`): `BIT(6)` = `RamFan 1.5` capable
    /// (gates whether `0x07A7..=0x07A9` are Turbo ceilings or Battery Saver limits).
    pub const FAN_CTRL_CAPS: u16 = 0x078E;
    pub const FAN_CTRL_CAPS_RAMFAN15_BIT: u8 = 1 << 6;

    /// Power and thermal limit registers (aligned with upstream Linux `uniwill-acpi.c`).
    pub const PL1_WATTS: u16 = 0x0783;
    pub const PL2_WATTS: u16 = 0x0784;
    pub const PL4_WATTS: u16 = 0x0785;
    /// CPU TCC offset write register (`0x0786`, `BIT(7)` enable | `BIT(6..=0)` offset `0..=40`).
    pub const TCC_OFFSET_C: u16 = 0x0786;
    pub const TCC_OFFSET_ENABLE_BIT: u8 = 1 << 7;
    /// Read-only EC factory Turbo TDP ceiling registers (valid ONLY when `0x078E BIT(6) == 1`).
    pub const TDP_FACTORY_PL1_MAX: u16 = 0x07A7;
    pub const TDP_FACTORY_PL2_MAX: u16 = 0x07A8;
    pub const TDP_FACTORY_PL4_MAX: u16 = 0x07A9;

    /// GPU cTGP & Dynamic Boost control/target registers (`0x0743..=0x0746`) + live readback.
    pub const CTGP_DB_CTRL: u16 = 0x0743;
    pub const CTGP_DB_GENERAL_ENABLE_BIT: u8 = 1 << 0;
    pub const CTGP_DB_DB_ENABLE_BIT: u8 = 1 << 1;
    pub const CTGP_DB_CTGP_ENABLE_BIT: u8 = 1 << 2;
    /// cTGP offset above `base_tgp_w` (`0..=254` W).
    pub const CTGP_WATTS: u16 = 0x0744;
    /// Dynamic Boost TPP target (`0xFF` when DB enabled, `0x00` when disabled).
    pub const DB_TPP_TARGET: u16 = 0x0745;
    /// Dynamic Boost watts (`0..=25` W, `EC_ADDR_CTGP_DB_DB_OFFSET`).
    pub const DYNAMIC_BOOST_WATTS: u16 = 0x0746;
    /// Live cTGP offset readback (`0x0788`, RO; live cTGP = `base_tgp_w + EC[0x0788]`).
    pub const CTGP_LIVE_OFFSET: u16 = 0x0788;
    /// Live Dynamic Boost watts readback (`0x07D5`, RO).
    pub const DB_LIVE_WATTS: u16 = 0x07D5;

    /// EC hardware switches & battery charging control (upstream `uniwill-acpi.c`).
    /// `FN_LOCK` is `BIT(4)` of `0x074E` (`EC_ADDR_BIOS_OEM`), NEVER clobbering other bits.
    pub const FN_LOCK: u16 = 0x074E;
    pub const FN_LOCK_BIT: u8 = 1 << 4;
    /// `0x0767` (`EC_ADDR_TRIGGER`) `BIT(0)` toggles Super/Win key lock; `0x0768` (`EC_ADDR_SWITCH_STATUS`) `BIT(0)` reads it.
    pub const WIN_LOCK_TRIGGER: u16 = 0x0767;
    pub const WIN_LOCK: u16 = 0x0768;
    pub const WIN_LOCK_BIT: u8 = 1 << 0;
    /// `USB_SLEEP_CHARGING` is `BIT(4)` of `0x0767` (`EC_ADDR_TRIGGER`), strictly RMW.
    pub const USB_SLEEP_CHARGING: u16 = 0x0767;
    pub const USB_SLEEP_CHARGING_BIT: u8 = 1 << 4;
    /// `0x07B9` (`EC_ADDR_CHARGE_CTRL`): bits `6..=0` (`0x7F`) hold charge end threshold (`0` = 100%).
    /// WARNING: `BIT(7)` (`CHARGE_CTRL_REACHED`) is RO status — NEVER write `BIT(7)` back!
    pub const CHARGE_CTRL: u16 = 0x07B9;
    pub const CHARGE_CTRL_MASK: u8 = 0x7F;
    /// `0x07D0` (`CHARGE_LIMIT_DOWN`): charge start/resume threshold percentage (`20..=95`).
    pub const CHARGE_LIMIT_DOWN: u16 = 0x07D0;

    /// Lightbar AC control register (`0x0748` `EC_ADDR_LIGHTBAR_AC_CTRL`):
    /// `BIT(0)` = `LIGHTBAR_APP_EXISTS` (kept set), `BIT(2)` = `LIGHTBAR_S0_OFF` (`0` = on, `1` = off).
    pub const LIGHTBAR_AC_CTRL: u16 = 0x0748;
    pub const LIGHTBAR_APP_EXISTS_BIT: u8 = 1 << 0;
    pub const LIGHTBAR_S0_OFF_BIT: u8 = 1 << 2;

    /// Battery charging profile mode register (`0x07A6`, mask `0x30`: `0x00`/`0x10`/`0x20`).
    /// Strictly preserves `BIT(3)` (touchpad LED) and `BIT(6)` (touchpad toggle).
    pub const BATTERY_CHARGE_MODE: u16 = 0x07A6;
    pub const BATTERY_CHARGE_MODE_MASK: u8 = 0x30;
    pub const BATTERY_MODE_HIGH_CAPACITY: u8 = 0x00;
    pub const BATTERY_MODE_BALANCED: u8 = 0x10;
    pub const BATTERY_MODE_STATIONARY: u8 = 0x20;

    /// Lid logo EC persistence registers (`0x0769..=0x076B` RGB 0..=50, `0x078C` commit `0x10`,
    /// `0x07AB` brightness/exists `0x01/0x21/0x41/0x61/0x81`, `0x07C5` bit 5 logo flag).
    pub const LOGO_R: u16 = 0x0769;
    pub const LOGO_G: u16 = 0x076A;
    pub const LOGO_B: u16 = 0x076B;
    pub const LOGO_COMMIT: u16 = 0x078C;
    pub const LOGO_COMMIT_VAL: u8 = 0x10;
    pub const LOGO_BRIGHTNESS: u16 = 0x07AB;
    pub const LOGO_EXISTS_BIT: u8 = 0x01;
    pub const LOGO_BRIGHTNESS_MASK: u8 = 0xE0;
    pub const OEM_BYTE5: u16 = 0x07C5;
    pub const OEM_BYTE5_LOGO_BIT: u8 = 1 << 5;
    pub const OEM_BYTE5_WHISPER_REQ_BIT: u8 = 1 << 6;
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
    pub base_tgp_w: i32,
    pub ctgp_min_w: i32,
    pub ctgp_max_w: i32,
    pub dynamic_boost_min_w: i32,
    pub dynamic_boost_max_w: i32,
    pub tcc_max_offset_c: u8,
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
            base_tgp_w: 55,
            ctgp_min_w: 55,
            ctgp_max_w: 175,
            dynamic_boost_min_w: 0,
            dynamic_boost_max_w: 25,
            tcc_max_offset_c: 0,
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
    /// Layout verified against `DSDT.dsl` and `EC_01_POWER_AND_MODES.md`:
    /// - `data[0]`: PL1 max (W)
    /// - `data[1]`: PL2 max (W)
    /// - `data[2]`: PL4 max (raw W; caller scales by `2 W/LSB` when `0x0727 BIT(7)` `pl4_doubled` is set)
    /// - `data[3]`: `TccOffset` (`tcc_max_offset_c`)
    /// - `data[4]`: `ConfigurableTGP` — max cTGP offset above `base_tgp_w` (W)
    /// - `data[5]`: `DynamicBoost` — max Dynamic Boost (W, separate from `ctgp_max_w`)
    /// - `data[6]`: `TargetTemperature` (°C)
    /// - `data[7]`: `DefaultTGP` — `base_tgp_w` (W), baseline for `0x0744`/`0x0788` offsets
    /// - `data[8..12]`: `0xFF` sentinel tail
    pub fn decode_inou0000_12byte(data: &[u8]) -> Option<Self> {
        if data.len() < usize::from(SMRW_INOU0000_LEN) || data[8..12] != [0xFF, 0xFF, 0xFF, 0xFF] {
            return None;
        }
        let pl1_max_w = i32::from(data[0]);
        let pl2_max_w = i32::from(data[1]);
        let pl4_max_w = i32::from(data[2]);
        let tcc_max_offset_c = data[3];
        let ctgp_offset_max_w = i32::from(data[4]);
        let dynamic_boost_max_w = i32::from(data[5]);
        let base_tgp_w = i32::from(data[7]);
        let ctgp_min_w = base_tgp_w;
        let ctgp_max_w = base_tgp_w + ctgp_offset_max_w;

        if pl1_max_w < 15
            || pl2_max_w < 15
            || pl4_max_w < 15
            || dynamic_boost_max_w > 100
            || !(35..=250).contains(&base_tgp_w)
            || ctgp_max_w > i32::from(u8::MAX)
        {
            return None;
        }

        Some(Self {
            pl1_min_w: 15,
            pl1_max_w,
            pl2_min_w: 20.min(pl2_max_w),
            pl2_max_w,
            pl4_min_w: 25.min(pl4_max_w),
            pl4_max_w,
            base_tgp_w,
            ctgp_min_w,
            ctgp_max_w,
            dynamic_boost_min_w: 0,
            dynamic_boost_max_w,
            tcc_max_offset_c,
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
            base_tgp_w: ctgp_min_w,
            ctgp_min_w,
            ctgp_max_w,
            dynamic_boost_min_w,
            dynamic_boost_max_w,
            tcc_max_offset_c: 0,
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
    pl4_doubled: bool,
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
        let api_version_valid = info.api_version == UWCC_API_VERSION;
        if transport.is_available() && !api_version_valid {
            tracing::warn!(
                found = info.api_version,
                expected = UWCC_API_VERSION,
                "UWCC driver API version mismatch; disabling backend operations"
            );
        }
        let has_ec = api_version_valid && (info.flags & UWCC_FLAG_HAS_EC) != 0;
        // Read-only hardware PL4 scaling bit (0x0727 BIT(7)): NEVER write BIT(7).
        let pl4_doubled = has_ec
            && transport
                .ec_read(uwcc_ec_reg::CUSTOM_MODE_ENABLE)
                .is_ok_and(|v| (v & uwcc_ec_reg::CUSTOM_MODE_PL4D_BIT) != 0);

        let smrw_limits = if api_version_valid {
            probe_smrw_limits(transport.as_ref(), info.flags, pl4_doubled)
        } else {
            SmrwPowerLimits::chassis_0x1a_default()
        };
        let webcam = sysfs.map(UsbWebcamController::new);
        let has_webcam_sysfs = webcam.is_some();

        let device_id = dmi.and_then(|d| identify_device(d, None));
        let has_pwup = api_version_valid && (info.flags & UWCC_FLAG_HAS_PWUP) != 0;
        let water_cooler_supported = has_pwup
            || (api_version_valid
                && (info.project_id == CHASSIS_PROJECT_0X1A
                    || is_water_cooler_supported(device_id)));

        let capabilities = PlatformCapabilities {
            backend: PlatformBackendKind::Uwcc,
            device: DeviceCapabilities {
                vendor: DeviceVendor::Uniwill,
                device_id,
                number_of_fans: if has_ec { 2 } else { 0 },
                fans_min_speed: if has_ec { 20 } else { 0 },
                fans_off_available: has_ec,
                fan_control: has_ec,
                tdp_control: has_ec,
                charging_thresholds: has_ec,
                ctgp_supported: has_ec,
                water_cooler_supported,
                webcam_switch: api_version_valid && has_webcam_sysfs,
                ..DeviceCapabilities::default()
            },
            tdp_channels: if has_ec { 3 } else { 0 },
            perf_profiles_available: if has_ec { 3 } else { 0 },
        };

        let ramfan = RamFanController::new(Arc::clone(&transport));

        Self {
            transport,
            ramfan,
            webcam,
            info,
            pl4_doubled,
            smrw_limits,
            capabilities,
        }
    }

    /// Returns cached driver and chassis info from `UWCC_IOC_GET_INFO`.
    pub fn info(&self) -> UwccInfo {
        self.info
    }

    /// Returns `true` when EC `0x0727 BIT(7)` indicates `2 W/LSB` PL4 encoding.
    pub fn is_pl4_doubled(&self) -> bool {
        self.pl4_doubled
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

    /// Sets GPU cTGP wattage after enforcing `SMRW` bounds:
    /// writes offset `(watts - base_tgp_w)` to `0x0744`, latches `0x0743` (`BIT(0) | BIT(2)`),
    /// and notifies `PWBT` when advertised.
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
        let offset = watts - self.smrw_limits.base_tgp_w;
        if !(0..=254).contains(&offset) {
            return Err(PlatformError::InvalidArgument(
                "cTGP offset above base TGP out of byte range (0..=254)",
            ));
        }
        let offset_raw = offset as u8;
        let prev_offset = self.transport.ec_read(uwcc_ec_reg::CTGP_WATTS).ok();
        let prev_ctrl = self.transport.ec_read(uwcc_ec_reg::CTGP_DB_CTRL).ok();

        self.transport
            .ec_write(uwcc_ec_reg::CTGP_WATTS, offset_raw)?;
        if let Err(err) = self.rmw_ec_bits(
            uwcc_ec_reg::CTGP_DB_CTRL,
            uwcc_ec_reg::CTGP_DB_GENERAL_ENABLE_BIT | uwcc_ec_reg::CTGP_DB_CTGP_ENABLE_BIT,
            0,
        ) {
            if let Some(prev) = prev_offset {
                let _ = self.transport.ec_write(uwcc_ec_reg::CTGP_WATTS, prev);
            }
            return Err(err);
        }

        if (self.info.flags & UWCC_FLAG_HAS_PWBT) != 0 {
            if let Err(err) = self
                .transport
                .set_gpu_power(UWCC_GPU_POWER_OP_BASE_TGP, watts as u32)
            {
                if let Some(prev) = prev_offset {
                    let _ = self.transport.ec_write(uwcc_ec_reg::CTGP_WATTS, prev);
                }
                if let Some(prev) = prev_ctrl {
                    let _ = self.transport.ec_write(uwcc_ec_reg::CTGP_DB_CTRL, prev);
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

    /// Sets GPU Dynamic Boost wattage after enforcing `SMRW` bounds:
    /// writes `0xFF`/`0x00` to `0x0745` (`DB_TPP_TARGET`), `watts` to `0x0746` (`DB_MAX_TGP`),
    /// RMW `0x0743` (`BIT(1)` and `BIT(0)` when enabling), and notifies `PWDB` when advertised.
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
        let tpp_target: u8 = if raw > 0 { 0xFF } else { 0x00 };

        let prev_tpp = self.transport.ec_read(uwcc_ec_reg::DB_TPP_TARGET).ok();
        let prev_db = self
            .transport
            .ec_read(uwcc_ec_reg::DYNAMIC_BOOST_WATTS)
            .ok();
        let prev_ctrl = self.transport.ec_read(uwcc_ec_reg::CTGP_DB_CTRL).ok();

        let rollback = || {
            if let Some(prev) = prev_tpp {
                let _ = self.transport.ec_write(uwcc_ec_reg::DB_TPP_TARGET, prev);
            }
            if let Some(prev) = prev_db {
                let _ = self
                    .transport
                    .ec_write(uwcc_ec_reg::DYNAMIC_BOOST_WATTS, prev);
            }
            if let Some(prev) = prev_ctrl {
                let _ = self.transport.ec_write(uwcc_ec_reg::CTGP_DB_CTRL, prev);
            }
        };

        self.transport
            .ec_write(uwcc_ec_reg::DB_TPP_TARGET, tpp_target)?;
        if let Err(err) = self
            .transport
            .ec_write(uwcc_ec_reg::DYNAMIC_BOOST_WATTS, raw)
        {
            rollback();
            return Err(err);
        }
        let ctrl_res = if raw > 0 {
            self.rmw_ec_bits(
                uwcc_ec_reg::CTGP_DB_CTRL,
                uwcc_ec_reg::CTGP_DB_GENERAL_ENABLE_BIT | uwcc_ec_reg::CTGP_DB_DB_ENABLE_BIT,
                0,
            )
        } else {
            self.rmw_ec_bits(
                uwcc_ec_reg::CTGP_DB_CTRL,
                0,
                uwcc_ec_reg::CTGP_DB_DB_ENABLE_BIT,
            )
        };
        if let Err(err) = ctrl_res {
            rollback();
            return Err(err);
        }

        if (self.info.flags & UWCC_FLAG_HAS_PWDB) != 0 {
            if let Err(err) = self
                .transport
                .set_gpu_power(UWCC_GPU_POWER_OP_DYNAMIC_BOOST, u32::from(raw))
            {
                rollback();
                return Err(err);
            }
        }
        Ok(())
    }

    /// Reads lid logo EC persistence state `(enabled, r, g, b, brightness)` from
    /// `0x07AB` and `0x0769..=0x076B`.
    pub fn read_lid_logo_ec(&self) -> Result<(bool, u8, u8, u8, u8), PlatformError> {
        self.require_ec()?;
        if !self.has_lid_logo_ec() {
            return Err(PlatformError::Unsupported(
                "Lid logo EC hardware not present",
            ));
        }
        let ab = self.transport.ec_read(uwcc_ec_reg::LOGO_BRIGHTNESS)?;
        let brightness = match ab & uwcc_ec_reg::LOGO_BRIGHTNESS_MASK {
            0x80 => 255,
            0x60 => 191,
            0x40 => 127,
            0x20 => 63,
            _ => 0,
        };
        let unscale = |v: u8| -> u8 { ((u16::from(v.min(50)) * 255 + 25) / 50) as u8 };
        let r = unscale(self.transport.ec_read(uwcc_ec_reg::LOGO_R)?);
        let g = unscale(self.transport.ec_read(uwcc_ec_reg::LOGO_G)?);
        let b = unscale(self.transport.ec_read(uwcc_ec_reg::LOGO_B)?);
        Ok((brightness > 0, r, g, b, brightness))
    }

    fn require_available(&self) -> Result<(), PlatformError> {
        if !self.transport.is_available() {
            return Err(PlatformError::Unavailable);
        }
        if self.info.api_version != UWCC_API_VERSION {
            return Err(PlatformError::Unsupported(
                "UWCC driver API version mismatch",
            ));
        }
        Ok(())
    }

    fn require_ec(&self) -> Result<(), PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_EC) == 0 {
            return Err(PlatformError::Unsupported(
                "EC interface unavailable in UWCC flags",
            ));
        }
        Ok(())
    }

    fn require_pwup(&self) -> Result<(), PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_PWUP) == 0 {
            return Err(PlatformError::Unsupported(
                "PWUP interface unavailable in UWCC flags",
            ));
        }
        Ok(())
    }

    fn require_wmi_events(&self) -> Result<(), PlatformError> {
        self.require_available()?;
        if (self.info.flags & UWCC_FLAG_HAS_WMI_EVENTS) == 0 {
            return Err(PlatformError::Unsupported(
                "WMI events unavailable in UWCC flags",
            ));
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

    fn is_chassis_0x1a(&self) -> bool {
        self.info.project_id == CHASSIS_PROJECT_0X1A
    }

    /// Sets or clears `mask` in EC register `reg` via read-modify-write, skipping the write
    /// if the register already has the desired bit state.
    fn rmw_ec_bits(&self, reg: u16, set_mask: u8, clear_mask: u8) -> Result<(), PlatformError> {
        let prev = self.transport.ec_read(reg)?;
        let next = (prev & !clear_mask) | set_mask;
        if next != prev {
            self.transport.ec_write(reg, next)?;
        }
        Ok(())
    }

    /// Activates real Uniwill `0x1A` Custom mode (white LED on mode button) so the EC applies
    /// `0x0783..=0x0785` TDP limits and custom fan control instead of throttling to Office 45 W.
    ///
    /// Strictly uses RMW on every register and NEVER modifies `0x0727 BIT(7)` (`PL4D`).
    fn enable_custom_mode_0x1a(&self) -> Result<(), PlatformError> {
        // 1. Latch AP_EXIST / ENABLE_MANUAL_CTRL (0x0741 BIT(0)) so EC keeps 0x0783..=0x0785.
        self.rmw_ec_bits(
            uwcc_ec_reg::AP_OEM,
            uwcc_ec_reg::AP_OEM_ENABLE_MANUAL_CTRL_BIT,
            0,
        )?;
        // 2. Enable Custom power loop (0x0726 BIT(7)) and CUME (0x0727 BIT(6), preserving BIT(7) PL4D).
        self.rmw_ec_bits(
            uwcc_ec_reg::CUSTOM_MODE_CTRL,
            uwcc_ec_reg::CUSTOM_MODE_CTRL_BIT,
            0,
        )?;
        self.rmw_ec_bits(
            uwcc_ec_reg::CUSTOM_MODE_ENABLE,
            uwcc_ec_reg::CUSTOM_MODE_CUME_BIT,
            0,
        )?;
        // 3. Best-effort latch CUSTOM_MODE (0x0706 = 0x41) when kernel allowlist permits it.
        if let Err(err) = self.transport.ec_write(
            uwcc_ec_reg::CUSTOM_MODE_0706,
            uwcc_ec_reg::CUSTOM_MODE_0706_ACTIVE,
        ) {
            tracing::debug!(%err, "Optional EC 0x0706 custom-mode latch skipped");
        }
        // 4. Set UFME (0x0751 BIT(7)) and clear TBME/HIGH so PMSC stays cleanly in Custom.
        self.rmw_ec_bits(
            uwcc_ec_reg::FAN_MANUAL_MODE,
            uwcc_ec_reg::FAN_MODE_USER_BIT,
            uwcc_ec_reg::FAN_MODE_TURBO_BIT | uwcc_ec_reg::FAN_MODE_HIGH_BIT,
        )?;
        Ok(())
    }

    /// Disables Uniwill `0x1A` Custom mode flags (`0x0727 BIT(6)` `CUME` and `0x0726 BIT(7)`),
    /// strictly preserving `0x0727 BIT(7)` (`PL4D`) and all unrelated bits.
    fn disable_custom_mode_0x1a(&self) -> Result<(), PlatformError> {
        self.rmw_ec_bits(
            uwcc_ec_reg::CUSTOM_MODE_ENABLE,
            0,
            uwcc_ec_reg::CUSTOM_MODE_CUME_BIT,
        )?;
        self.rmw_ec_bits(
            uwcc_ec_reg::CUSTOM_MODE_CTRL,
            0,
            uwcc_ec_reg::CUSTOM_MODE_CTRL_BIT,
        )?;
        Ok(())
    }
}

impl Drop for UwccBackend {
    fn drop(&mut self) {
        if self.transport.is_available() && (self.info.flags & UWCC_FLAG_HAS_EC) != 0 {
            // 1. Disarm RamFan 1.5 table latch (0x07C6 BIT(2)) and Fan Boost (0x0751 BIT(6)).
            let _ = self.rmw_ec_bits(
                uwcc_ec_reg::AP_OEM_6,
                0,
                uwcc_ec_reg::AP_OEM_6_UNIVERSAL_FAN_BIT,
            );
            let _ = self.rmw_ec_bits(
                uwcc_ec_reg::FAN_MANUAL_MODE,
                0,
                uwcc_ec_reg::FAN_MODE_BOOST_BIT,
            );
            // 2. On 0x1A, clear CUME (0x0727 BIT(6), preserving PL4D BIT(7)) and 0x0726 BIT(7).
            if self.is_chassis_0x1a() {
                let _ = self.disable_custom_mode_0x1a();
            }
            // 3. Release AP_EXIST (0x0741 BIT(0)) on all EC chassis so the physical mode button
            //    returns to autonomous EC firmware cycling when uccd stops.
            let _ = self.rmw_ec_bits(
                uwcc_ec_reg::AP_OEM,
                0,
                uwcc_ec_reg::AP_OEM_ENABLE_MANUAL_CTRL_BIT,
            );
        }
    }
}

fn probe_smrw_limits(
    transport: &dyn UwccTransport,
    flags: u32,
    pl4_doubled: bool,
) -> SmrwPowerLimits {
    if !transport.is_available() || (flags & UWCC_FLAG_HAS_SMRW) == 0 {
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
            if pl4_doubled {
                limits.pl4_max_w = limits.pl4_max_w.saturating_mul(2);
            }
            // Merge optional EC factory Turbo ceiling registers (0x07A7..=0x07A9) ONLY when
            // 0x078E BIT(6) (RamFan 1.5 capable) is set; otherwise 0x07A7..=0x07A9 is Battery Saver.
            if (flags & UWCC_FLAG_HAS_EC) != 0 {
                let has_ramfan15 = transport
                    .ec_read(uwcc_ec_reg::FAN_CTRL_CAPS)
                    .is_ok_and(|v| (v & uwcc_ec_reg::FAN_CTRL_CAPS_RAMFAN15_BIT) != 0);
                if has_ramfan15 {
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
                            let ec_pl4_w = if pl4_doubled {
                                i32::from(ec_pl4) * 2
                            } else {
                                i32::from(ec_pl4)
                            };
                            limits.pl4_max_w = limits.pl4_max_w.max(ec_pl4_w);
                        }
                    }
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
        self.transport.is_available() && self.info.api_version == UWCC_API_VERSION
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities.clone()
    }

    fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
        self.require_ec()?;
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
        self.require_ec()?;
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
        self.require_ec()?;
        let (hi_reg, lo_reg) = match fan_idx {
            0 => (uwcc_ec_reg::CPU_FAN_RPM_HI, uwcc_ec_reg::CPU_FAN_RPM_LO),
            1 => (uwcc_ec_reg::GPU_FAN_RPM_HI, uwcc_ec_reg::GPU_FAN_RPM_LO),
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
                ))
            }
        };
        // Tear-free 16-bit read: hi1 -> lo -> hi2 (up to 3 attempts).
        for _ in 0..3 {
            let hi1 = self.transport.ec_read(hi_reg)?;
            let lo = self.transport.ec_read(lo_reg)?;
            let hi2 = self.transport.ec_read(hi_reg)?;
            if hi1 == hi2 {
                return decode_tachometer_rpm(hi1, lo);
            }
        }
        Err(PlatformError::Unavailable)
    }

    fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
        self.require_ec()?;
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
        // On Uniwill 0x1A, setting UFME (0x0751 BIT(7)) without CUME (0x0727 BIT(6)) forces
        // Office mode (45 W CPU cap). Latch Custom mode bits and disarm RamFan table latch.
        if self.is_chassis_0x1a() {
            self.rmw_ec_bits(
                uwcc_ec_reg::CUSTOM_MODE_CTRL,
                uwcc_ec_reg::CUSTOM_MODE_CTRL_BIT,
                0,
            )?;
            self.rmw_ec_bits(
                uwcc_ec_reg::CUSTOM_MODE_ENABLE,
                uwcc_ec_reg::CUSTOM_MODE_CUME_BIT,
                0,
            )?;
            self.rmw_ec_bits(
                uwcc_ec_reg::AP_OEM_6,
                0,
                uwcc_ec_reg::AP_OEM_6_UNIVERSAL_FAN_BIT,
            )?;
        }
        let prev = self.transport.ec_read(uwcc_ec_reg::FAN_MANUAL_MODE)?;
        let mut next = prev | uwcc_ec_reg::FAN_MODE_USER_BIT;
        // On Uniwill 0x1A, legacy manual duty caps around ~60% (~3500 RPM) unless FAN_MODE_BOOST
        // (0x0751 BIT(6)) is set (hysteresis: >= 98% on, < 95% off).
        if self.is_chassis_0x1a() {
            if percent >= 98 {
                next |= uwcc_ec_reg::FAN_MODE_BOOST_BIT;
            } else if percent < 95 {
                next &= !uwcc_ec_reg::FAN_MODE_BOOST_BIT;
            }
        }
        let changed_mode = next != prev;
        if changed_mode {
            self.transport
                .ec_write(uwcc_ec_reg::FAN_MANUAL_MODE, next)?;
        }
        if let Err(err) = self.transport.ec_write(reg, duty) {
            if changed_mode {
                let _ = self.transport.ec_write(uwcc_ec_reg::FAN_MANUAL_MODE, prev);
            }
            return Err(err);
        }
        Ok(())
    }

    fn set_fans_auto(&self) -> Result<(), PlatformError> {
        self.require_ec()?;
        let ramfan_res = self.ramfan.restore_firmware_auto();
        let ec_res = self
            .transport
            .ec_read(uwcc_ec_reg::FAN_MANUAL_MODE)
            .and_then(|prev| {
                self.transport.ec_write(
                    uwcc_ec_reg::FAN_MANUAL_MODE,
                    prev & !(uwcc_ec_reg::FAN_MODE_USER_BIT | uwcc_ec_reg::FAN_MODE_BOOST_BIT),
                )
            });
        ramfan_res.and(ec_res)
    }

    fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
        self.require_ec()?;
        let reg = Self::tdp_reg(index)?;
        let raw = i32::from(self.transport.ec_read(reg)?);
        if index == 2 && self.pl4_doubled {
            Ok(raw * 2)
        } else {
            Ok(raw)
        }
    }

    fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
        self.require_ec()?;
        self.smrw_limits
            .tdp_bounds(index)
            .ok_or(PlatformError::InvalidArgument(
                "Uniwill TDP channel index must be 0..=2",
            ))
    }

    fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
        self.require_ec()?;
        let (min_w, max_w) = self.read_tdp_limits(index)?;
        if !(min_w..=max_w).contains(&watts) {
            return Err(PlatformError::InvalidArgument(
                "TDP wattage outside SMRW hardware limits",
            ));
        }
        let reg = Self::tdp_reg(index)?;
        let encoded = if index == 2 && self.pl4_doubled {
            watts / 2
        } else {
            watts
        };
        let raw = u8::try_from(encoded)
            .map_err(|_| PlatformError::InvalidArgument("Power limit out of byte range"))?;
        if self.is_chassis_0x1a() {
            // On Uniwill 0x1A, custom PL1/PL2/PL4 only take effect in Custom mode (CUME + 0x0726
            // BIT(7) + AP_OEM BIT(0) + UFME); otherwise EC caps CPU at 45 W or zeroes 0x0783..=0x0785.
            self.enable_custom_mode_0x1a()?;
        } else if let Ok(prev_oem) = self.transport.ec_read(uwcc_ec_reg::AP_OEM) {
            if (prev_oem & uwcc_ec_reg::AP_OEM_ENABLE_MANUAL_CTRL_BIT) == 0 {
                self.transport.ec_write(
                    uwcc_ec_reg::AP_OEM,
                    prev_oem | uwcc_ec_reg::AP_OEM_ENABLE_MANUAL_CTRL_BIT,
                )?;
            }
        }
        self.transport.ec_write(reg, raw)
    }

    fn read_perf_profile(&self) -> Result<i32, PlatformError> {
        self.require_ec()?;
        if self.is_chassis_0x1a() {
            return match self.read_pmsc_mode()? {
                "custom" | "turbo" => Ok(3),
                "office" => Ok(1),
                _ => Ok(2),
            };
        }
        let code = i32::from(self.transport.ec_read(uwcc_ec_reg::PERF_MODE)?);
        if !(1..=3).contains(&code) {
            return Err(PlatformError::Unavailable);
        }
        Ok(code)
    }

    fn read_pmsc_mode(&self) -> Result<&'static str, PlatformError> {
        self.require_ec()?;
        // Follow DSDT PMSC priority:
        // 1. CUME (0x0727 BIT(6)) -> "custom"
        // 2. TBME (0x0751 BIT(4)) -> "turbo"
        // 3. UFME (0x0751 BIT(7)) -> "office"
        // 4. else                 -> "gaming"
        let cume = (self.transport.ec_read(uwcc_ec_reg::CUSTOM_MODE_ENABLE)?
            & uwcc_ec_reg::CUSTOM_MODE_CUME_BIT)
            != 0;
        if cume {
            return Ok("custom");
        }
        let fan_ctrl = self.transport.ec_read(uwcc_ec_reg::FAN_MANUAL_MODE)?;
        if (fan_ctrl & uwcc_ec_reg::FAN_MODE_TURBO_BIT) != 0 {
            return Ok("turbo");
        }
        if (fan_ctrl & uwcc_ec_reg::FAN_MODE_USER_BIT) != 0 {
            return Ok("office");
        }
        Ok("gaming")
    }

    fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
        self.require_ec()?;
        if !(1..=3).contains(&profile_code) {
            return Err(PlatformError::InvalidArgument(
                "Uniwill performance profile code must be 1..=3",
            ));
        }
        if self.is_chassis_0x1a() {
            return match profile_code {
                // 1 = Silent / Office (green LED, 0x0751 = UFME | HIGH, CUME cleared)
                1 => {
                    self.disable_custom_mode_0x1a()?;
                    self.rmw_ec_bits(
                        uwcc_ec_reg::FAN_MANUAL_MODE,
                        uwcc_ec_reg::FAN_MODE_USER_BIT | uwcc_ec_reg::FAN_MODE_HIGH_BIT,
                        uwcc_ec_reg::FAN_MODE_TURBO_BIT,
                    )
                }
                // 2 = Balanced / Gaming (CUME cleared, UFME/TBME/HIGH cleared)
                2 => {
                    self.disable_custom_mode_0x1a()?;
                    self.rmw_ec_bits(
                        uwcc_ec_reg::FAN_MANUAL_MODE,
                        0,
                        uwcc_ec_reg::FAN_MODE_USER_BIT
                            | uwcc_ec_reg::FAN_MODE_TURBO_BIT
                            | uwcc_ec_reg::FAN_MODE_HIGH_BIT,
                    )
                }
                // 3 = Enthusiast / Custom (white LED, full user TDP & fan control)
                3 => self.enable_custom_mode_0x1a(),
                _ => unreachable!(),
            };
        }
        self.transport
            .ec_write(uwcc_ec_reg::PERF_MODE, profile_code as u8)
    }

    fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
        self.require_ec()?;
        if self.is_chassis_0x1a() {
            // On 0x1A, 0x0753 is CPU VRM current limit (NOT MODE_ENABLE).
            // Software manual control enable is EC_ADDR_AP_OEM BIT(0) (0x0741).
            return if enable {
                self.rmw_ec_bits(
                    uwcc_ec_reg::AP_OEM,
                    uwcc_ec_reg::AP_OEM_ENABLE_MANUAL_CTRL_BIT,
                    0,
                )
            } else {
                self.rmw_ec_bits(
                    uwcc_ec_reg::AP_OEM,
                    0,
                    uwcc_ec_reg::AP_OEM_ENABLE_MANUAL_CTRL_BIT,
                )
            };
        }
        self.transport
            .ec_write(uwcc_ec_reg::MODE_ENABLE, u8::from(enable))
    }

    fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
        self.require_ec()?;
        if self.is_chassis_0x1a() {
            return Err(PlatformError::Unsupported(
                "Raw EC 0x0752 mode writes are unsupported on chassis 0x1A; use set_perf_profile",
            ));
        }
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
        self.require_ec()?;
        Ok(self.transport.ec_read(uwcc_ec_reg::TCC_OFFSET_READ)? & uwcc_ec_reg::TCC_OFFSET_MASK)
    }

    fn set_tcc_offset(&self, offset_c: u8) -> Result<(), PlatformError> {
        self.require_ec()?;
        if offset_c > MAX_TCC_OFFSET_C {
            return Err(PlatformError::InvalidArgument(
                "CPU TCC offset must be 0..=40 C",
            ));
        }
        self.transport.ec_write(
            uwcc_ec_reg::TCC_OFFSET_C,
            uwcc_ec_reg::TCC_OFFSET_ENABLE_BIT | offset_c,
        )
    }

    fn set_pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
        self.require_pwup()?;
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
        self.require_ec()?;
        if let Ok(live) = self.transport.ec_read(uwcc_ec_reg::CTGP_LIVE_OFFSET) {
            if live != u8::MAX {
                return Ok(self.smrw_limits.base_tgp_w + i32::from(live));
            }
        }
        let offset = self.transport.ec_read(uwcc_ec_reg::CTGP_WATTS)?;
        if offset == u8::MAX {
            return Err(PlatformError::Unavailable);
        }
        Ok(self.smrw_limits.base_tgp_w + i32::from(offset))
    }

    fn set_ctgp_watts(&self, watts: i32) -> Result<(), PlatformError> {
        self.set_ctgp(watts)
    }

    fn read_dynamic_boost_watts(&self) -> Result<i32, PlatformError> {
        self.require_ec()?;
        if let Ok(live) = self.transport.ec_read(uwcc_ec_reg::DB_LIVE_WATTS) {
            if live != u8::MAX {
                return Ok(i32::from(live));
            }
        }
        let raw = self.transport.ec_read(uwcc_ec_reg::DYNAMIC_BOOST_WATTS)?;
        if raw == u8::MAX {
            return Err(PlatformError::Unavailable);
        }
        Ok(i32::from(raw))
    }

    fn set_dynamic_boost_watts(&self, watts: i32) -> Result<(), PlatformError> {
        self.set_dynamic_boost(watts)
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
            // 5 hardware brightness levels: 0x01 (off), 0x21 (25%), 0x41 (50%), 0x61 (75%), 0x81 (100%).
            let br_bits = match brightness {
                1..=63 => 0x20,
                64..=127 => 0x40,
                128..=191 => 0x60,
                _ => 0x80,
            };
            // Guard 0x07C5 BIT(5) so we never clobber Whisper Mode BIT(6) or other flags.
            let oem5 = self.transport.ec_read(uwcc_ec_reg::OEM_BYTE5)?;
            if (oem5 & uwcc_ec_reg::OEM_BYTE5_WHISPER_REQ_BIT) == 0 {
                let next_oem5 = oem5 | uwcc_ec_reg::OEM_BYTE5_LOGO_BIT;
                if next_oem5 != oem5 {
                    self.transport.ec_write(uwcc_ec_reg::OEM_BYTE5, next_oem5)?;
                }
            }
            self.transport.ec_write(uwcc_ec_reg::LOGO_R, scale(r))?;
            self.transport.ec_write(uwcc_ec_reg::LOGO_G, scale(g))?;
            self.transport.ec_write(uwcc_ec_reg::LOGO_B, scale(b))?;
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_BRIGHTNESS, preserved_low | br_bits)?;
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_COMMIT, uwcc_ec_reg::LOGO_COMMIT_VAL)
        } else {
            // Symmetrically clear 0x07C5 BIT(5) on disable without clobbering Whisper Mode BIT(6).
            let oem5 = self.transport.ec_read(uwcc_ec_reg::OEM_BYTE5)?;
            if (oem5 & uwcc_ec_reg::OEM_BYTE5_WHISPER_REQ_BIT) == 0 {
                let next_oem5 = oem5 & !uwcc_ec_reg::OEM_BYTE5_LOGO_BIT;
                if next_oem5 != oem5 {
                    self.transport.ec_write(uwcc_ec_reg::OEM_BYTE5, next_oem5)?;
                }
            }
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_BRIGHTNESS, preserved_low)?;
            self.transport
                .ec_write(uwcc_ec_reg::LOGO_COMMIT, uwcc_ec_reg::LOGO_COMMIT_VAL)
        }
    }

    fn set_lightbar_power(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_ec()?;
        let prev = self.transport.ec_read(uwcc_ec_reg::LIGHTBAR_AC_CTRL)?;
        // Always keep BIT(0) (LIGHTBAR_APP_EXISTS) set; toggle BIT(2) (S0_OFF: 0 = on, 1 = off).
        let next = if enabled {
            (prev | uwcc_ec_reg::LIGHTBAR_APP_EXISTS_BIT) & !uwcc_ec_reg::LIGHTBAR_S0_OFF_BIT
        } else {
            prev | uwcc_ec_reg::LIGHTBAR_APP_EXISTS_BIT | uwcc_ec_reg::LIGHTBAR_S0_OFF_BIT
        };
        self.transport.ec_write(uwcc_ec_reg::LIGHTBAR_AC_CTRL, next)
    }

    fn read_fn_lock(&self) -> Result<bool, PlatformError> {
        self.require_ec()?;
        Ok((self.transport.ec_read(uwcc_ec_reg::FN_LOCK)? & uwcc_ec_reg::FN_LOCK_BIT) != 0)
    }

    fn set_fn_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_ec()?;
        if enabled {
            self.rmw_ec_bits(uwcc_ec_reg::FN_LOCK, uwcc_ec_reg::FN_LOCK_BIT, 0)
        } else {
            self.rmw_ec_bits(uwcc_ec_reg::FN_LOCK, 0, uwcc_ec_reg::FN_LOCK_BIT)
        }
    }

    fn read_win_lock(&self) -> Result<bool, PlatformError> {
        self.require_ec()?;
        Ok((self.transport.ec_read(uwcc_ec_reg::WIN_LOCK)? & uwcc_ec_reg::WIN_LOCK_BIT) != 0)
    }

    fn set_win_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_ec()?;
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
        self.require_ec()?;
        Ok((self.transport.ec_read(uwcc_ec_reg::USB_SLEEP_CHARGING)?
            & uwcc_ec_reg::USB_SLEEP_CHARGING_BIT)
            != 0)
    }

    fn set_usb_sleep_charging(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_ec()?;
        if enabled {
            self.rmw_ec_bits(
                uwcc_ec_reg::USB_SLEEP_CHARGING,
                uwcc_ec_reg::USB_SLEEP_CHARGING_BIT,
                0,
            )
        } else {
            self.rmw_ec_bits(
                uwcc_ec_reg::USB_SLEEP_CHARGING,
                0,
                uwcc_ec_reg::USB_SLEEP_CHARGING_BIT,
            )
        }
    }

    fn read_charge_thresholds(&self) -> Result<(u8, u8), PlatformError> {
        self.require_ec()?;
        let raw_end =
            self.transport.ec_read(uwcc_ec_reg::CHARGE_CTRL)? & uwcc_ec_reg::CHARGE_CTRL_MASK;
        let end = if raw_end == 0 {
            MAX_CHARGE_THRESHOLD_PCT
        } else {
            raw_end
        };
        if !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&end) {
            return Err(PlatformError::InvalidArgument(
                "EC reported invalid charge threshold",
            ));
        }
        let raw_start = self.transport.ec_read(uwcc_ec_reg::CHARGE_LIMIT_DOWN)?;
        let start = if (MIN_CHARGE_THRESHOLD_PCT..end).contains(&raw_start) {
            raw_start
        } else {
            let fallback = end.saturating_sub(5).max(MIN_CHARGE_THRESHOLD_PCT);
            if fallback >= end {
                end.saturating_sub(1)
            } else {
                fallback
            }
        };
        Ok((start, end))
    }

    fn set_charge_thresholds(&self, start: u8, end: u8) -> Result<(), PlatformError> {
        self.require_ec()?;
        if !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&start)
            || !(MIN_CHARGE_THRESHOLD_PCT..=MAX_CHARGE_THRESHOLD_PCT).contains(&end)
            || start >= end
        {
            return Err(PlatformError::InvalidArgument(
                "Charge thresholds must satisfy 20 <= start < end <= 100",
            ));
        }
        self.transport
            .ec_write(uwcc_ec_reg::CHARGE_LIMIT_DOWN, start)?;
        // NEVER write back BIT(7) (CHARGE_CTRL_REACHED); encode 100% as 0.
        let end_raw = (if end == MAX_CHARGE_THRESHOLD_PCT {
            0
        } else {
            end
        }) & uwcc_ec_reg::CHARGE_CTRL_MASK;
        self.transport.ec_write(uwcc_ec_reg::CHARGE_CTRL, end_raw)
    }

    fn set_fan_boost(&self, enabled: bool) -> Result<(), PlatformError> {
        self.require_ec()?;
        self.ramfan.set_fan_boost(enabled)
    }

    fn apply_ramfan_curve(&self, curve: &RamFanCurve) -> Result<(), PlatformError> {
        self.require_ec()?;
        self.ramfan.apply_curve(curve)
    }

    fn poll_wmi_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
        self.require_wmi_events()?;
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
