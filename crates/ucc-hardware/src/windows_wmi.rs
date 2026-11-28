//! Windows driverless ACPI WMI (`root\WMI` `AcpiTest_MULong` over `wmiacpi.sys`) EC backend (`M15.2`, `ADR-06`).
//!
//! Provides a portable trait boundary ([`WmiEcTransport`]), machine-readable EV/WHQL driver
//! boundary metadata ([`WINDOWS_WMI_CAPABILITY_MATRIX`]), portable `AcpiTest_MULong` wire
//! encoding ([`AcpiWmiCommand`]), and [`WindowsWmiEcBackend`] implementing [`PlatformBackend`]
//! so Windows builds can manage performance modes, fans, temperatures, TDP, cTGP/Dynamic Boost,
//! charge thresholds, and hardware switches without requiring a custom signed KMDF driver.

use crate::detection::{DeviceCapabilities, DeviceVendor};
use crate::platform::{PlatformBackend, PlatformBackendKind, PlatformCapabilities, PlatformError};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// WMI namespace hosting Windows built-in `wmiacpi.sys` ACPI WMI classes.
pub const WMI_NAMESPACE: &str = r"root\WMI";

/// Uniwill ACPI WMI class exposed over `wmiacpi.sys`.
pub const WMI_CLASS_ACPI_TEST: &str = "AcpiTest_MULong";

/// Uniwill ACPI WMI method name for 64-bit packed EC register reads/writes.
pub const WMI_METHOD_GET_SET_ULONG: &str = "GetSetULong";

/// WMI method ID for `AcpiTest_MULong::GetSetULong` EC register read/write transactions.
pub const WMI_METHOD_ID_EC_RW: u32 = 0x04;

/// Bit 24 in the 64-bit `GetSetULong` argument selects an EC read (`1`) vs. EC write (`0`).
pub const WMI_EC_READ_FLAG: u64 = 1u64 << 24;

/// Minimum allowed battery charge threshold percentage (`20..=100`).
pub const MIN_CHARGE_THRESHOLD_PERCENT: u8 = 20;

/// Maximum allowed battery charge threshold percentage (`20..=100`).
pub const MAX_CHARGE_THRESHOLD_PERCENT: u8 = 100;

/// Uniwill EC register addresses exposed through ACPI `INOU0000` / `AcpiTest_MULong`.
pub mod ec_reg {
    pub const FAN_MANUAL_MODE: u16 = 0x0751;
    pub const CPU_FAN_DUTY: u16 = 0x1804;
    pub const GPU_FAN_DUTY: u16 = 0x1809;
    pub const CPU_TEMP_C: u16 = 0x043E;
    pub const GPU_TEMP_C: u16 = 0x044F;
    pub const PERF_MODE: u16 = 0x0752;
    pub const FAN_BOOST: u16 = 0x0753;
    pub const PL1_WATTS: u16 = 0x0740;
    pub const PL2_WATTS: u16 = 0x0741;
    pub const PL4_WATTS: u16 = 0x0742;
    pub const CTGP_OFFSET_WATTS: u16 = 0x0748;
    pub const DYNAMIC_BOOST_WATTS: u16 = 0x0749;
    pub const FN_LOCK: u16 = 0x0760;
    pub const WEBCAM_SWITCH: u16 = 0x0761;
    pub const LIGHTBAR_POWER: u16 = 0x0780;
    pub const CHARGE_START_THRESHOLD: u16 = 0x07B8;
    pub const CHARGE_END_THRESHOLD: u16 = 0x07B9;
}

/// Windows driver signing / deployment requirement for each Uniwill WMI feature (`ADR-06`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriverRequirement {
    /// Works driverless via Windows built-in `wmiacpi.sys` (`root\WMI` `AcpiTest_MULong`).
    DriverlessWmiAcpi,
    /// Requires a custom EV/WHQL-signed KMDF kernel driver (e.g. `UWACPIDriver.sys`).
    SignedOemKmdfEvWhql,
}

/// Uniwill hardware features classified across the driverless `wmiacpi.sys` vs. EV/WHQL boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WmiFeature {
    FanControl,
    FanBoost,
    TempSensors,
    PerfProfile,
    TdpPl1Pl2Pl4,
    CtgpDynamicBoost,
    ChargeThresholds,
    FnLock,
    WebcamSwitch,
    LightbarPower,
    OemMethodPwup,
    OemMethodSmrw,
    RawEcArbitraryIo,
}

/// Machine-readable capability matrix entry pairing a [`WmiFeature`] with its driver boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WmiFeatureSupport {
    pub feature: WmiFeature,
    pub requirement: DriverRequirement,
    pub transport_path: &'static str,
    pub notes: &'static str,
}

/// Exhaustive Windows capability matrix documenting the driverless `wmiacpi.sys` vs. EV/WHQL
/// `UWACPIDriver.sys` boundary (`ADR-06`, `M15.2`).
pub const WINDOWS_WMI_CAPABILITY_MATRIX: &[WmiFeatureSupport] = &[
    WmiFeatureSupport {
        feature: WmiFeature::FanControl,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC registers 0x0751, 0x1804, 0x1809",
    },
    WmiFeatureSupport {
        feature: WmiFeature::FanBoost,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC register 0x0753",
    },
    WmiFeatureSupport {
        feature: WmiFeature::TempSensors,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC registers 0x043E (CPU), 0x044F (GPU)",
    },
    WmiFeatureSupport {
        feature: WmiFeature::PerfProfile,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC register 0x0752",
    },
    WmiFeatureSupport {
        feature: WmiFeature::TdpPl1Pl2Pl4,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC registers 0x0740..=0x0742",
    },
    WmiFeatureSupport {
        feature: WmiFeature::CtgpDynamicBoost,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC registers 0x0748 (cTGP), 0x0749 (Dynamic Boost)",
    },
    WmiFeatureSupport {
        feature: WmiFeature::ChargeThresholds,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC registers 0x07B8 (start), 0x07B9 (end)",
    },
    WmiFeatureSupport {
        feature: WmiFeature::FnLock,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC register 0x0760",
    },
    WmiFeatureSupport {
        feature: WmiFeature::WebcamSwitch,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC register 0x0761",
    },
    WmiFeatureSupport {
        feature: WmiFeature::LightbarPower,
        requirement: DriverRequirement::DriverlessWmiAcpi,
        transport_path: r"root\WMI:AcpiTest_MULong::GetSetULong (wmiacpi.sys)",
        notes: "EC register 0x0780",
    },
    WmiFeatureSupport {
        feature: WmiFeature::OemMethodPwup,
        requirement: DriverRequirement::SignedOemKmdfEvWhql,
        transport_path: "UWACPIDriver.sys (EV/WHQL KMDF)",
        notes: "OEM ACPI PWUP liquid-cooler power boost requires signed KMDF driver",
    },
    WmiFeatureSupport {
        feature: WmiFeature::OemMethodSmrw,
        requirement: DriverRequirement::SignedOemKmdfEvWhql,
        transport_path: "UWACPIDriver.sys (EV/WHQL KMDF)",
        notes: "OEM ACPI SMRW factory limits table requires signed KMDF driver",
    },
    WmiFeatureSupport {
        feature: WmiFeature::RawEcArbitraryIo,
        requirement: DriverRequirement::SignedOemKmdfEvWhql,
        transport_path: "UWACPIDriver.sys (EV/WHQL KMDF)",
        notes: "Direct arbitrary EC port I/O requires signed KMDF kernel driver",
    },
];

/// Returns the Windows driver signing requirement for a [`WmiFeature`].
pub const fn feature_requirement(feature: WmiFeature) -> DriverRequirement {
    match feature {
        WmiFeature::FanControl
        | WmiFeature::FanBoost
        | WmiFeature::TempSensors
        | WmiFeature::PerfProfile
        | WmiFeature::TdpPl1Pl2Pl4
        | WmiFeature::CtgpDynamicBoost
        | WmiFeature::ChargeThresholds
        | WmiFeature::FnLock
        | WmiFeature::WebcamSwitch
        | WmiFeature::LightbarPower => DriverRequirement::DriverlessWmiAcpi,
        WmiFeature::OemMethodPwup | WmiFeature::OemMethodSmrw | WmiFeature::RawEcArbitraryIo => {
            DriverRequirement::SignedOemKmdfEvWhql
        }
    }
}

/// Looks up the full [`WmiFeatureSupport`] metadata record for `feature`.
pub fn feature_support(feature: WmiFeature) -> Option<&'static WmiFeatureSupport> {
    WINDOWS_WMI_CAPABILITY_MATRIX
        .iter()
        .find(|entry| entry.feature == feature)
}

/// Encodes a 16-bit Uniwill EC register address into the 64-bit `AcpiTest_MULong::GetSetULong`
/// read argument (`bits 0..15 = addr`, `bit 24 = 1`).
pub const fn encode_ec_read_arg(addr: u16) -> u64 {
    (addr as u64) | WMI_EC_READ_FLAG
}

/// Encodes a 16-bit Uniwill EC register address and byte value into the 64-bit
/// `AcpiTest_MULong::GetSetULong` write argument (`bits 0..15 = addr`, `bits 16..23 = val`,
/// `bit 24 = 0`).
pub const fn encode_ec_write_arg(addr: u16, val: u8) -> u64 {
    (addr as u64) | ((val as u64) << 16)
}

/// Extracts the EC register byte from the raw 64-bit `AcpiTest_MULong::GetSetULong` return value.
pub const fn decode_ec_read_byte(raw: u64) -> u8 {
    (raw & 0xFF) as u8
}

/// Portable command representation for `root\WMI` `AcpiTest_MULong::GetSetULong` transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiWmiCommand {
    ReadEcByte { addr: u16 },
    WriteEcByte { addr: u16, val: u8 },
}

impl AcpiWmiCommand {
    /// Returns the WMI method ID associated with this command (`GetSetULong`).
    pub const fn method_id(self) -> u32 {
        WMI_METHOD_ID_EC_RW
    }

    /// Encodes the command into the 64-bit packed `GetSetULong` `Data` argument.
    pub const fn encode_arg(self) -> u64 {
        match self {
            Self::ReadEcByte { addr } => encode_ec_read_arg(addr),
            Self::WriteEcByte { addr, val } => encode_ec_write_arg(addr, val),
        }
    }

    /// Decodes a 64-bit packed `GetSetULong` `Data` argument back into an [`AcpiWmiCommand`].
    pub const fn decode_arg(arg: u64) -> Self {
        let addr = (arg & 0xFFFF) as u16;
        if (arg & WMI_EC_READ_FLAG) != 0 {
            Self::ReadEcByte { addr }
        } else {
            let val = ((arg >> 16) & 0xFF) as u8;
            Self::WriteEcByte { addr, val }
        }
    }
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
    fail_write_reg: Mutex<Option<u16>>,
}

impl MockWmiEcTransport {
    pub fn new(available: bool) -> Self {
        Self {
            available: Mutex::new(available),
            regs: Mutex::new(BTreeMap::new()),
            ops: Mutex::new(Vec::new()),
            fail_write_reg: Mutex::new(None),
        }
    }

    pub fn set_reg(&self, addr: u16, val: u8) {
        self.regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(addr, val);
    }

    pub fn get_reg(&self, addr: u16) -> Option<u8> {
        self.regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&addr)
            .copied()
    }

    pub fn set_available(&self, avail: bool) {
        *self.available.lock().unwrap_or_else(|e| e.into_inner()) = avail;
    }

    /// Injects a write failure whenever `write_ec_byte` targets `addr`.
    pub fn set_fail_write_reg(&self, addr: Option<u16>) {
        *self
            .fail_write_reg
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = addr;
    }

    /// Executes a mutating callback with the register map locked (useful for poison recovery tests).
    pub fn update_regs<F: FnOnce(&mut BTreeMap<u16, u8>)>(&self, f: F) {
        let mut guard = self.regs.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
    }

    pub fn ops(&self) -> Vec<WmiEcOp> {
        self.ops.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn clear_ops(&self) {
        self.ops.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl WmiEcTransport for MockWmiEcTransport {
    fn is_available(&self) -> bool {
        *self.available.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read_ec_byte(&self, addr: u16) -> Result<u8, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.ops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WmiEcOp::ReadByte(addr));
        Ok(self
            .regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&addr)
            .copied()
            .unwrap_or(0))
    }

    fn write_ec_byte(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        if *self
            .fail_write_reg
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            == Some(addr)
        {
            return Err(PlatformError::Unavailable);
        }
        self.ops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WmiEcOp::WriteByte { addr, val });
        self.regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(addr, val);
        Ok(())
    }

    fn call_wmi_method(&self, method_id: u32, arg: u64) -> Result<u64, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        self.ops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
                webcam_switch: true,
                charging_thresholds: true,
                ctgp_supported: true,
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

    /// Checks whether `feature` is supported on the driverless `wmiacpi.sys` path.
    ///
    /// Returns [`PlatformError::Unsupported`] for features requiring a signed EV/WHQL KMDF driver.
    pub fn check_feature_support(&self, feature: WmiFeature) -> Result<(), PlatformError> {
        match feature_requirement(feature) {
            DriverRequirement::DriverlessWmiAcpi => Ok(()),
            DriverRequirement::SignedOemKmdfEvWhql => Err(PlatformError::Unsupported(
                "Feature requires a signed EV/WHQL KMDF OEM driver (UWACPIDriver.sys)",
            )),
        }
    }

    /// Reads battery charge `(start_percent, end_percent)` thresholds (`0x07B8`/`0x07B9`).
    pub fn read_charge_thresholds(&self) -> Result<(u8, u8), PlatformError> {
        let start = self
            .transport
            .read_ec_byte(ec_reg::CHARGE_START_THRESHOLD)?;
        let end = self.transport.read_ec_byte(ec_reg::CHARGE_END_THRESHOLD)?;
        Ok((start, end))
    }

    /// Writes battery charge `(start_percent, end_percent)` thresholds (`0x07B8`/`0x07B9`).
    ///
    /// Enforces `20..=100` bounds for both values and `start_percent < end_percent`, rolling back
    /// the start register if the second write fails.
    pub fn set_charge_thresholds(
        &self,
        start_percent: u8,
        end_percent: u8,
    ) -> Result<(), PlatformError> {
        if !(MIN_CHARGE_THRESHOLD_PERCENT..=MAX_CHARGE_THRESHOLD_PERCENT).contains(&start_percent) {
            return Err(PlatformError::InvalidArgument(
                "Charge start threshold must be 20..=100",
            ));
        }
        if !(MIN_CHARGE_THRESHOLD_PERCENT..=MAX_CHARGE_THRESHOLD_PERCENT).contains(&end_percent) {
            return Err(PlatformError::InvalidArgument(
                "Charge end threshold must be 20..=100",
            ));
        }
        if start_percent >= end_percent {
            return Err(PlatformError::InvalidArgument(
                "Charge start threshold must be less than end threshold",
            ));
        }
        let prev_start = self
            .transport
            .read_ec_byte(ec_reg::CHARGE_START_THRESHOLD)
            .ok();
        self.transport
            .write_ec_byte(ec_reg::CHARGE_START_THRESHOLD, start_percent)?;
        if let Err(err) = self
            .transport
            .write_ec_byte(ec_reg::CHARGE_END_THRESHOLD, end_percent)
        {
            if let Some(prev) = prev_start {
                let _ = self
                    .transport
                    .write_ec_byte(ec_reg::CHARGE_START_THRESHOLD, prev);
            }
            return Err(err);
        }
        Ok(())
    }

    /// Reads the Fn-Lock hardware switch state (`0x0760`).
    pub fn read_fn_lock(&self) -> Result<bool, PlatformError> {
        Ok(self.transport.read_ec_byte(ec_reg::FN_LOCK)? != 0)
    }

    /// Sets the Fn-Lock hardware switch state (`0x0760`).
    pub fn set_fn_lock(&self, enabled: bool) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::FN_LOCK, u8::from(enabled))
    }

    /// Reads the firmware webcam hardware switch state (`0x0761`).
    pub fn read_webcam_switch(&self) -> Result<bool, PlatformError> {
        Ok(self.transport.read_ec_byte(ec_reg::WEBCAM_SWITCH)? != 0)
    }

    /// Sets the firmware webcam hardware switch state (`0x0761`).
    pub fn set_webcam_switch(&self, enabled: bool) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::WEBCAM_SWITCH, u8::from(enabled))
    }

    /// Reads the chassis lightbar power rail switch (`0x0780`).
    pub fn read_lightbar_power(&self) -> Result<bool, PlatformError> {
        Ok(self.transport.read_ec_byte(ec_reg::LIGHTBAR_POWER)? != 0)
    }

    /// Enables or disables the chassis lightbar power rail (`0x0780`).
    pub fn set_lightbar_power(&self, enabled: bool) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::LIGHTBAR_POWER, u8::from(enabled))
    }

    /// Reads the EC Fan Boost mode state (`0x0753`).
    pub fn read_fan_boost(&self) -> Result<bool, PlatformError> {
        Ok(self.transport.read_ec_byte(ec_reg::FAN_BOOST)? != 0)
    }

    /// Enables or disables EC Fan Boost mode (`0x0753`).
    pub fn set_fan_boost(&self, enabled: bool) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::FAN_BOOST, u8::from(enabled))
    }

    /// Reads the dGPU cTGP offset in watts (`0x0748`).
    pub fn read_ctgp_offset_watts(&self) -> Result<u8, PlatformError> {
        self.transport.read_ec_byte(ec_reg::CTGP_OFFSET_WATTS)
    }

    /// Writes the dGPU cTGP offset in watts (`0x0748`).
    pub fn set_ctgp_offset_watts(&self, watts: u8) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::CTGP_OFFSET_WATTS, watts)
    }

    /// Reads the dGPU Dynamic Boost budget in watts (`0x0749`).
    pub fn read_dynamic_boost_watts(&self) -> Result<u8, PlatformError> {
        self.transport.read_ec_byte(ec_reg::DYNAMIC_BOOST_WATTS)
    }

    /// Writes the dGPU Dynamic Boost budget in watts (`0x0749`).
    pub fn set_dynamic_boost_watts(&self, watts: u8) -> Result<(), PlatformError> {
        self.transport
            .write_ec_byte(ec_reg::DYNAMIC_BOOST_WATTS, watts)
    }

    /// Explicit EV/WHQL boundary guard for the OEM ACPI `PWUP` method.
    ///
    /// Always returns [`PlatformError::Unsupported`] on the driverless `wmiacpi.sys` backend.
    pub fn invoke_oem_pwup(&self, _level: u8) -> Result<(), PlatformError> {
        self.check_feature_support(WmiFeature::OemMethodPwup)?;
        Err(PlatformError::Unsupported(
            "OEM ACPI method PWUP requires a signed EV/WHQL KMDF driver (UWACPIDriver.sys)",
        ))
    }

    /// Explicit EV/WHQL boundary guard for the OEM ACPI `SMRW` factory limits table method.
    ///
    /// Always returns [`PlatformError::Unsupported`] on the driverless `wmiacpi.sys` backend.
    pub fn read_oem_smrw_table(&self) -> Result<Vec<u8>, PlatformError> {
        self.check_feature_support(WmiFeature::OemMethodSmrw)?;
        Err(PlatformError::Unsupported(
            "OEM ACPI method SMRW requires a signed EV/WHQL KMDF driver (UWACPIDriver.sys)",
        ))
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
        let prev_mode = self
            .transport
            .read_ec_byte(ec_reg::FAN_MANUAL_MODE)
            .unwrap_or(0);
        self.transport.write_ec_byte(ec_reg::FAN_MANUAL_MODE, 1)?;
        if let Err(err) = self.transport.write_ec_byte(reg, ec_duty) {
            if prev_mode == 0 {
                let _ = self.transport.write_ec_byte(ec_reg::FAN_MANUAL_MODE, 0);
            }
            return Err(err);
        }
        Ok(())
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
        let watts_u8 = u8::try_from(watts).map_err(|_| {
            PlatformError::InvalidArgument("TDP watts exceeds 8-bit EC register range")
        })?;
        self.transport.write_ec_byte(reg, watts_u8)
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

    fn read_webcam(&self) -> Result<bool, PlatformError> {
        self.read_webcam_switch()
    }

    fn set_webcam(&self, enabled: bool) -> Result<(), PlatformError> {
        self.set_webcam_switch(enabled)
    }
}

#[cfg(windows)]
mod win32_wmi {
    use super::{
        decode_ec_read_byte, encode_ec_read_arg, encode_ec_write_arg, PlatformError,
        WmiEcTransport, WMI_CLASS_ACPI_TEST, WMI_METHOD_GET_SET_ULONG, WMI_METHOD_ID_EC_RW,
        WMI_NAMESPACE,
    };
    use std::ffi::c_void;
    use std::ptr::{self, NonNull};
    use std::sync::Mutex;

    type HResult = i32;
    type Bstr = *mut u16;

    const S_OK: HResult = 0;
    const S_FALSE: HResult = 1;
    const RPC_E_CHANGED_MODE: HResult = 0x8001_0106u32 as i32;
    const COINIT_MULTITHREADED: u32 = 0x0;
    const CLSCTX_INPROC_SERVER: u32 = 0x1;
    const RPC_C_AUTHN_WINNT: u32 = 10;
    const RPC_C_AUTHZ_NONE: u32 = 0;
    const RPC_C_AUTHN_LEVEL_CALL: u32 = 3;
    const RPC_C_IMP_LEVEL_IMPERSONATE: u32 = 3;
    const EOAC_NONE: u32 = 0;
    const WBEM_FLAG_FORWARD_ONLY: i32 = 0x20;
    const WBEM_FLAG_RETURN_IMMEDIATELY: i32 = 0x10;
    const WBEM_INFINITE: i32 = -1;

    const VT_I4: u16 = 3;
    const VT_BSTR: u16 = 8;
    const VT_UI4: u16 = 19;
    const VT_I8: u16 = 20;
    const VT_UI8: u16 = 21;

    #[allow(dead_code)]
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // CLSID_WbemLocator = {4590f811-1d3a-11d0-891f-00aa004b2e24}
    const CLSID_WBEM_LOCATOR: Guid = Guid {
        data1: 0x4590_f811,
        data2: 0x1d3a,
        data3: 0x11d0,
        data4: [0x89, 0x1f, 0x00, 0xaa, 0x00, 0x4b, 0x2e, 0x24],
    };

    // IID_IWbemLocator = {dc12a687-737f-11cf-884d-00aa004b2e24}
    const IID_IWBEM_LOCATOR: Guid = Guid {
        data1: 0xdc12_a687,
        data2: 0x737f,
        data3: 0x11cf,
        data4: [0x88, 0x4d, 0x00, 0xaa, 0x00, 0x4b, 0x2e, 0x24],
    };

    #[allow(dead_code)]
    #[repr(C)]
    struct Variant {
        vt: u16,
        w_reserved1: u16,
        w_reserved2: u16,
        w_reserved3: u16,
        #[cfg(target_pointer_width = "64")]
        data: [u64; 2],
        #[cfg(target_pointer_width = "32")]
        data: [u64; 1],
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> HResult;
        fn CoUninitialize();
        fn CoCreateInstance(
            rclsid: *const Guid,
            p_unk_outer: *mut c_void,
            dw_cls_context: u32,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> HResult;
        fn CoSetProxyBlanket(
            p_proxy: *mut c_void,
            dw_authn_svc: u32,
            dw_authz_svc: u32,
            p_server_princ_name: *const u16,
            dw_authn_level: u32,
            dw_imp_level: u32,
            p_auth_info: *mut c_void,
            dw_capabilities: u32,
        ) -> HResult;
    }

    #[link(name = "oleaut32")]
    extern "system" {
        fn SysAllocString(psz: *const u16) -> Bstr;
        fn SysFreeString(bstr: Bstr);
        fn VariantInit(pvarg: *mut Variant);
        fn VariantClear(pvarg: *mut Variant) -> HResult;
    }

    #[repr(C)]
    struct IUnknownVtbl {
        _query_interface: *const c_void,
        _add_ref: *const c_void,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct IWbemLocatorVtbl {
        base: IUnknownVtbl,
        connect_server: unsafe extern "system" fn(
            *mut IWbemLocator,
            Bstr,
            Bstr,
            Bstr,
            Bstr,
            i32,
            Bstr,
            *mut c_void,
            *mut *mut IWbemServices,
        ) -> HResult,
    }

    #[repr(C)]
    struct IWbemLocator {
        vtbl: *const IWbemLocatorVtbl,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct IWbemServicesVtbl {
        base: IUnknownVtbl,
        _open_namespace: *const c_void,
        _cancel_async_call: *const c_void,
        _query_object_sink: *const c_void,
        get_object: unsafe extern "system" fn(
            *mut IWbemServices,
            Bstr,
            i32,
            *mut c_void,
            *mut *mut IWbemClassObject,
            *mut *mut c_void,
        ) -> HResult,
        _get_object_async: *const c_void,
        _put_class: *const c_void,
        _put_class_async: *const c_void,
        _delete_class: *const c_void,
        _delete_class_async: *const c_void,
        _create_class_enum: *const c_void,
        _create_class_enum_async: *const c_void,
        _put_instance: *const c_void,
        _put_instance_async: *const c_void,
        _delete_instance: *const c_void,
        _delete_instance_async: *const c_void,
        create_instance_enum: unsafe extern "system" fn(
            *mut IWbemServices,
            Bstr,
            i32,
            *mut c_void,
            *mut *mut IEnumWbemClassObject,
        ) -> HResult,
        _create_instance_enum_async: *const c_void,
        _exec_query: *const c_void,
        _exec_query_async: *const c_void,
        _exec_notification_query: *const c_void,
        _exec_notification_query_async: *const c_void,
        exec_method: unsafe extern "system" fn(
            *mut IWbemServices,
            Bstr,
            Bstr,
            i32,
            *mut c_void,
            *mut IWbemClassObject,
            *mut *mut IWbemClassObject,
            *mut *mut c_void,
        ) -> HResult,
    }

    #[repr(C)]
    struct IWbemServices {
        vtbl: *const IWbemServicesVtbl,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct IEnumWbemClassObjectVtbl {
        base: IUnknownVtbl,
        _reset: *const c_void,
        next: unsafe extern "system" fn(
            *mut IEnumWbemClassObject,
            i32,
            u32,
            *mut *mut IWbemClassObject,
            *mut u32,
        ) -> HResult,
    }

    #[repr(C)]
    struct IEnumWbemClassObject {
        vtbl: *const IEnumWbemClassObjectVtbl,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct IWbemClassObjectVtbl {
        base: IUnknownVtbl,
        _get_qualifier_set: *const c_void,
        get: unsafe extern "system" fn(
            *mut IWbemClassObject,
            *const u16,
            i32,
            *mut Variant,
            *mut i32,
            *mut i32,
        ) -> HResult,
        put: unsafe extern "system" fn(
            *mut IWbemClassObject,
            *const u16,
            i32,
            *const Variant,
            i32,
        ) -> HResult,
        _delete: *const c_void,
        _get_names: *const c_void,
        _begin_enumeration: *const c_void,
        _next: *const c_void,
        _end_enumeration: *const c_void,
        _get_property_qualifier_set: *const c_void,
        _clone: *const c_void,
        _get_object_text: *const c_void,
        _spawn_derived_class: *const c_void,
        spawn_instance: unsafe extern "system" fn(
            *mut IWbemClassObject,
            i32,
            *mut *mut IWbemClassObject,
        ) -> HResult,
        _compare_to: *const c_void,
        _get_property_origin: *const c_void,
        _inherits_from: *const c_void,
        get_method: unsafe extern "system" fn(
            *mut IWbemClassObject,
            *const u16,
            i32,
            *mut *mut IWbemClassObject,
            *mut *mut IWbemClassObject,
        ) -> HResult,
    }

    #[repr(C)]
    struct IWbemClassObject {
        vtbl: *const IWbemClassObjectVtbl,
    }

    fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// RAII guard for per-thread COM initialization (`CoInitializeEx` / `CoUninitialize`).
    struct ComInitGuard {
        should_uninit: bool,
    }

    impl ComInitGuard {
        fn init() -> Result<Self, PlatformError> {
            // SAFETY: `CoInitializeEx` with `null_mut()` and `COINIT_MULTITHREADED` is valid
            // on any Windows thread; we balance every `S_OK`/`S_FALSE` call with `CoUninitialize`.
            let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED) };
            if hr == S_OK || hr == S_FALSE {
                Ok(Self {
                    should_uninit: true,
                })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    should_uninit: false,
                })
            } else {
                Err(PlatformError::Unavailable)
            }
        }
    }

    impl Drop for ComInitGuard {
        fn drop(&mut self) {
            if self.should_uninit {
                // SAFETY: `self.should_uninit` is true only when `CoInitializeEx` returned
                // `S_OK` or `S_FALSE` on this thread.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// RAII guard for Win32 `BSTR` allocations (`SysAllocString` / `SysFreeString`).
    struct BstrGuard(Bstr);

    impl BstrGuard {
        fn new(s: &str) -> Result<Self, PlatformError> {
            let wide = to_wide_null(s);
            // SAFETY: `wide` is a valid NUL-terminated UTF-16 slice.
            let bstr = unsafe { SysAllocString(wide.as_ptr()) };
            if bstr.is_null() {
                Err(PlatformError::Unavailable)
            } else {
                Ok(Self(bstr))
            }
        }

        fn as_raw(&self) -> Bstr {
            self.0
        }
    }

    impl Drop for BstrGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by `SysAllocString` and is freed exactly once.
                unsafe { SysFreeString(self.0) };
                self.0 = ptr::null_mut();
            }
        }
    }

    /// RAII guard for Win32 `VARIANT` values (`VariantInit` / `VariantClear`).
    struct VariantGuard(Variant);

    impl VariantGuard {
        fn empty() -> Self {
            let mut v = Variant {
                vt: 0,
                w_reserved1: 0,
                w_reserved2: 0,
                w_reserved3: 0,
                #[cfg(target_pointer_width = "64")]
                data: [0; 2],
                #[cfg(target_pointer_width = "32")]
                data: [0; 1],
            };
            // SAFETY: `v` is a properly aligned mutable `Variant` struct.
            unsafe { VariantInit(&mut v) };
            Self(v)
        }

        /// Encodes a `u64` into a WMI `CIM_UINT64`-compatible `VT_BSTR` variant.
        fn from_wmi_u64(val: u64) -> Result<Self, PlatformError> {
            let text = val.to_string();
            let wide = to_wide_null(&text);
            // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer; ownership of the
            // resulting `BSTR` is transferred to `Variant` and released by `VariantClear`.
            let bstr = unsafe { SysAllocString(wide.as_ptr()) };
            if bstr.is_null() {
                return Err(PlatformError::Unavailable);
            }
            let mut guard = Self::empty();
            guard.0.vt = VT_BSTR;
            guard.0.data[0] = bstr as usize as u64;
            Ok(guard)
        }

        fn as_ptr(&self) -> *const Variant {
            &self.0
        }

        fn as_mut_ptr(&mut self) -> *mut Variant {
            &mut self.0
        }

        fn to_string_opt(&self) -> Option<String> {
            if self.0.vt != VT_BSTR {
                return None;
            }
            let ptr = self.0.data[0] as usize as *const u16;
            if ptr.is_null() {
                return None;
            }
            // SAFETY: For a `VT_BSTR` populated by WMI, `ptr` points to a NUL-terminated
            // UTF-16 string whose byte length is stored in the `u32` immediately preceding it.
            let len_bytes = unsafe { *(ptr.cast::<u32>().offset(-1)) };
            let len_u16 = (len_bytes / 2) as usize;
            // SAFETY: `ptr` is valid for `len_u16` UTF-16 code units per the BSTR ABI.
            let slice = unsafe { std::slice::from_raw_parts(ptr, len_u16) };
            Some(String::from_utf16_lossy(slice))
        }

        fn to_u64(&self) -> Result<u64, PlatformError> {
            match self.0.vt {
                VT_UI8 | VT_I8 => Ok(self.0.data[0]),
                VT_UI4 | VT_I4 => Ok(u64::from(self.0.data[0] as u32)),
                VT_BSTR => self
                    .to_string_opt()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .ok_or(PlatformError::Unavailable),
                _ => Err(PlatformError::Unavailable),
            }
        }
    }

    impl Drop for VariantGuard {
        fn drop(&mut self) {
            // SAFETY: `self.0` was initialized via `VariantInit` and is cleared exactly once.
            let _ = unsafe { VariantClear(&mut self.0) };
        }
    }

    /// RAII smart pointer for COM interfaces whose first field is a vtable starting with `IUnknown`.
    struct ComPtr<T>(NonNull<T>);

    impl<T> ComPtr<T> {
        /// Wraps a non-null COM interface pointer, taking ownership of one reference count.
        unsafe fn from_raw(ptr: *mut T) -> Result<Self, PlatformError> {
            NonNull::new(ptr)
                .map(Self)
                .ok_or(PlatformError::Unavailable)
        }

        fn as_ptr(&self) -> *mut T {
            self.0.as_ptr()
        }
    }

    impl<T> Drop for ComPtr<T> {
        fn drop(&mut self) {
            // SAFETY: Every COM interface pointer `T` starts with a pointer to a vtable whose
            // prefix is `IUnknownVtbl`; `ComPtr` owns one reference count to release.
            unsafe {
                let vtbl_ptr = *(self.0.as_ptr().cast::<*const IUnknownVtbl>());
                ((*vtbl_ptr).release)(self.0.as_ptr().cast::<c_void>());
            }
        }
    }

    /// Active WMI services session keeping `IWbemServices` ahead of `ComInitGuard` so Rust
    /// drops `services` (`IWbemServices::Release`) before `_com` (`CoUninitialize`).
    struct WmiServicesSession {
        services: ComPtr<IWbemServices>,
        _com: ComInitGuard,
    }

    fn connect_wmi_services() -> Result<WmiServicesSession, PlatformError> {
        let com = ComInitGuard::init()?;

        let mut locator_raw: *mut c_void = ptr::null_mut();
        // SAFETY: `CLSID_WBEM_LOCATOR` and `IID_IWBEM_LOCATOR` are valid COM GUIDs and
        // `locator_raw` is a valid out-pointer.
        let hr = unsafe {
            CoCreateInstance(
                &CLSID_WBEM_LOCATOR,
                ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IWBEM_LOCATOR,
                &mut locator_raw,
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `CoCreateInstance` succeeded with `IID_IWBEM_LOCATOR`, returning an
        // owned `IWbemLocator` COM interface pointer.
        let locator = unsafe { ComPtr::from_raw(locator_raw.cast::<IWbemLocator>()) }?;

        let namespace = BstrGuard::new(WMI_NAMESPACE)?;
        let mut services_raw: *mut IWbemServices = ptr::null_mut();
        // SAFETY: `locator` is a valid `IWbemLocator` COM object and `namespace` is a valid BSTR.
        let hr = unsafe {
            let vtbl = (*locator.as_ptr()).vtbl;
            ((*vtbl).connect_server)(
                locator.as_ptr(),
                namespace.as_raw(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut services_raw,
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `ConnectServer` succeeded and returned an owned `IWbemServices` pointer.
        let services = unsafe { ComPtr::from_raw(services_raw) }?;

        // SAFETY: `services` is a valid COM proxy returned by `ConnectServer`.
        let hr = unsafe {
            CoSetProxyBlanket(
                services.as_ptr().cast::<c_void>(),
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                ptr::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                ptr::null_mut(),
                EOAC_NONE,
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }

        Ok(WmiServicesSession {
            services,
            _com: com,
        })
    }

    fn find_first_instance_relpath(
        services: &ComPtr<IWbemServices>,
    ) -> Result<String, PlatformError> {
        let class_bstr = BstrGuard::new(WMI_CLASS_ACPI_TEST)?;
        let mut enum_raw: *mut IEnumWbemClassObject = ptr::null_mut();
        // SAFETY: `services` is a valid `IWbemServices` proxy and `class_bstr` is a valid BSTR.
        let hr = unsafe {
            let vtbl = (*services.as_ptr()).vtbl;
            ((*vtbl).create_instance_enum)(
                services.as_ptr(),
                class_bstr.as_raw(),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                ptr::null_mut(),
                &mut enum_raw,
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `CreateInstanceEnum` succeeded and returned an owned enumerator.
        let enumerator = unsafe { ComPtr::from_raw(enum_raw) }?;

        let mut obj_raw: *mut IWbemClassObject = ptr::null_mut();
        let mut returned: u32 = 0;
        // SAFETY: `enumerator` is valid and `obj_raw`/`returned` are valid out-pointers.
        let hr = unsafe {
            let vtbl = (*enumerator.as_ptr()).vtbl;
            ((*vtbl).next)(
                enumerator.as_ptr(),
                WBEM_INFINITE,
                1,
                &mut obj_raw,
                &mut returned,
            )
        };
        if hr < 0 || returned == 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `Next` returned 1 owned `IWbemClassObject` instance.
        let instance = unsafe { ComPtr::from_raw(obj_raw) }?;

        let relpath_name = to_wide_null("__RELPATH");
        let mut val = VariantGuard::empty();
        // SAFETY: `instance` is valid, `relpath_name` is NUL-terminated UTF-16, and `val` is initialized.
        let hr = unsafe {
            let vtbl = (*instance.as_ptr()).vtbl;
            ((*vtbl).get)(
                instance.as_ptr(),
                relpath_name.as_ptr(),
                0,
                val.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        val.to_string_opt().ok_or(PlatformError::Unavailable)
    }

    fn exec_get_set_ulong(arg: u64) -> Result<u64, PlatformError> {
        let session = connect_wmi_services()?;
        let services = &session.services;
        let relpath = find_first_instance_relpath(services)?;

        let class_bstr = BstrGuard::new(WMI_CLASS_ACPI_TEST)?;
        let mut class_obj_raw: *mut IWbemClassObject = ptr::null_mut();
        // SAFETY: `services` is a valid `IWbemServices` proxy and `class_bstr` is a valid BSTR.
        let hr = unsafe {
            let vtbl = (*services.as_ptr()).vtbl;
            ((*vtbl).get_object)(
                services.as_ptr(),
                class_bstr.as_raw(),
                0,
                ptr::null_mut(),
                &mut class_obj_raw,
                ptr::null_mut(),
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `GetObject` succeeded and returned an owned class definition object.
        let class_obj = unsafe { ComPtr::from_raw(class_obj_raw) }?;

        let method_wide = to_wide_null(WMI_METHOD_GET_SET_ULONG);
        let mut in_sig_raw: *mut IWbemClassObject = ptr::null_mut();
        // SAFETY: `class_obj` is valid and `method_wide` is NUL-terminated UTF-16.
        let hr = unsafe {
            let vtbl = (*class_obj.as_ptr()).vtbl;
            ((*vtbl).get_method)(
                class_obj.as_ptr(),
                method_wide.as_ptr(),
                0,
                &mut in_sig_raw,
                ptr::null_mut(),
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `GetMethod` succeeded and returned an owned input signature object.
        let in_sig = unsafe { ComPtr::from_raw(in_sig_raw) }?;

        let mut in_params_raw: *mut IWbemClassObject = ptr::null_mut();
        // SAFETY: `in_sig` is a valid `IWbemClassObject`.
        let hr = unsafe {
            let vtbl = (*in_sig.as_ptr()).vtbl;
            ((*vtbl).spawn_instance)(in_sig.as_ptr(), 0, &mut in_params_raw)
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `SpawnInstance` succeeded and returned an owned parameter instance.
        let in_params = unsafe { ComPtr::from_raw(in_params_raw) }?;

        let data_name = to_wide_null("Data");
        let data_val = VariantGuard::from_wmi_u64(arg)?;
        // SAFETY: `in_params` is valid, `data_name` is NUL-terminated UTF-16, and `data_val` is valid.
        let hr = unsafe {
            let vtbl = (*in_params.as_ptr()).vtbl;
            ((*vtbl).put)(
                in_params.as_ptr(),
                data_name.as_ptr(),
                0,
                data_val.as_ptr(),
                0,
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }

        let obj_path_bstr = BstrGuard::new(&relpath)?;
        let method_bstr = BstrGuard::new(WMI_METHOD_GET_SET_ULONG)?;
        let mut out_params_raw: *mut IWbemClassObject = ptr::null_mut();
        // SAFETY: `services`, `obj_path_bstr`, `method_bstr`, and `in_params` are all valid.
        let hr = unsafe {
            let vtbl = (*services.as_ptr()).vtbl;
            ((*vtbl).exec_method)(
                services.as_ptr(),
                obj_path_bstr.as_raw(),
                method_bstr.as_raw(),
                0,
                ptr::null_mut(),
                in_params.as_ptr(),
                &mut out_params_raw,
                ptr::null_mut(),
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        // SAFETY: `ExecMethod` succeeded and returned an owned output parameters object.
        let out_params = unsafe { ComPtr::from_raw(out_params_raw) }?;

        let ret_name = to_wide_null("Return");
        let mut ret_val = VariantGuard::empty();
        // SAFETY: `out_params` is valid, `ret_name` is NUL-terminated UTF-16, and `ret_val` is initialized.
        let hr = unsafe {
            let vtbl = (*out_params.as_ptr()).vtbl;
            ((*vtbl).get)(
                out_params.as_ptr(),
                ret_name.as_ptr(),
                0,
                ret_val.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if hr < 0 {
            return Err(PlatformError::Unavailable);
        }
        ret_val.to_u64()
    }

    /// Native Windows COM/WMI transport targeting `root\WMI` `AcpiTest_MULong::GetSetULong`
    /// over Windows built-in `wmiacpi.sys` (zero external Cargo dependencies).
    #[derive(Debug, Default)]
    pub struct Win32WmiEcTransport {
        io_lock: Mutex<()>,
    }

    impl Win32WmiEcTransport {
        pub fn new() -> Self {
            Self {
                io_lock: Mutex::new(()),
            }
        }
    }

    impl WmiEcTransport for Win32WmiEcTransport {
        fn is_available(&self) -> bool {
            let _guard = self.io_lock.lock().unwrap_or_else(|e| e.into_inner());
            connect_wmi_services()
                .and_then(|session| find_first_instance_relpath(&session.services))
                .is_ok()
        }

        fn read_ec_byte(&self, addr: u16) -> Result<u8, PlatformError> {
            let raw = self.call_wmi_method(WMI_METHOD_ID_EC_RW, encode_ec_read_arg(addr))?;
            Ok(decode_ec_read_byte(raw))
        }

        fn write_ec_byte(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
            let _ = self.call_wmi_method(WMI_METHOD_ID_EC_RW, encode_ec_write_arg(addr, val))?;
            Ok(())
        }

        fn call_wmi_method(&self, method_id: u32, arg: u64) -> Result<u64, PlatformError> {
            if method_id != WMI_METHOD_ID_EC_RW {
                return Err(PlatformError::Unsupported(
                    "Only AcpiTest_MULong::GetSetULong (method_id 0x04) is supported driverless",
                ));
            }
            let _guard = self.io_lock.lock().unwrap_or_else(|e| e.into_inner());
            exec_get_set_ulong(arg)
        }
    }
}

#[cfg(windows)]
pub use win32_wmi::Win32WmiEcTransport;
