//! Hardware vendor detection, DMI identification, descriptors, and exact DMI quirks.
//!
//! Handles identification of Clevo, Uniwill, and Unsupported devices based on
//! hardware capabilities and driver checks, rather than marketing strings.

use crate::ioctl::requests;
use crate::traits::{HardwareIo, Sysfs};
use std::path::Path;

/// Hardware platform / EC controller vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DeviceVendor {
    Clevo,
    Uniwill,
    #[default]
    Unsupported,
}

impl DeviceVendor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Clevo => "clevo",
            Self::Uniwill => "uniwill",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Laptop brand/manufacturer identified from DMI tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LaptopManufacturer {
    Tuxedo,
    Xmg,
    PcSpecialist,
    Uniwill,
    Mechrevo,
    #[default]
    Unknown,
}

impl LaptopManufacturer {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tuxedo => "TUXEDO",
            Self::Xmg => "XMG",
            Self::PcSpecialist => "PCSpecialist",
            Self::Uniwill => "Uniwill",
            Self::Mechrevo => "MECHREVO",
            Self::Unknown => "Unknown",
        }
    }
}

/// Supported hardware device models matching baseline `UniwillDeviceID`.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UniwillDeviceID {
    IBP17G6,
    PULSE1403,
    PULSE1404,
    IBP14G6_TUX,
    IBP14G6_TRX,
    IBP14G6_TQF,
    IBP14G7_AQF_ARX,
    IBPG8,
    IBPG10AMD,
    PULSE1502,
    AURA14G3,
    AURA15G3,
    POLARIS1XA02,
    POLARIS1XI02,
    POLARIS1XA03,
    POLARIS1XI03,
    POLARIS1XA05,
    STELLARIS1XA03,
    STELLARIS1XI03,
    STELLARIS1XI04,
    STEPOL1XA04,
    STELLARIS1XI05,
    STELLARIS1XA05,
    STELLARIS16I06,
    STELLARIS17I06,
    STELLSL15A06,
    STELLSL15I06,
    STELLARIS16A07,
    STELLARIS16I07,
    GEMINI17I04,
    IBM15A10,
    SIRIUS1601,
    SIRIUS1602,
    XNE16E25,
    XNE16A25,
    PCS_X6FR559Y,
}

impl UniwillDeviceID {
    pub fn model_name(&self) -> &'static str {
        match self {
            Self::IBP17G6 => "InfinityBook Pro 17 Gen6",
            Self::IBP14G6_TUX | Self::IBP14G6_TRX | Self::IBP14G6_TQF => "InfinityBook Pro 14 Gen6",
            Self::IBP14G7_AQF_ARX => "InfinityBook Pro 14 Gen7",
            Self::IBPG8 => "InfinityBook Pro Gen8",
            Self::IBPG10AMD => "InfinityBook Pro Gen10 AMD",
            Self::IBM15A10 => "InfinityBook Metal 15 Gen10 AMD",
            Self::PULSE1403 => "Pulse 14 Gen3",
            Self::PULSE1404 => "Pulse 14 Gen4",
            Self::PULSE1502 => "Pulse 15 Gen2",
            Self::AURA14G3 => "Aura 14 Gen3",
            Self::AURA15G3 => "Aura 15 Gen3",
            Self::POLARIS1XA02 => "Polaris 15/17 AMD Gen2",
            Self::POLARIS1XI02 => "Polaris 15/17 Intel Gen2",
            Self::POLARIS1XA03 => "Polaris 15/17 AMD Gen3",
            Self::POLARIS1XI03 => "Polaris 15/17 Intel Gen3",
            Self::POLARIS1XA05 => "Polaris 15/17 AMD Gen5",
            Self::STELLARIS1XA03 => "Stellaris 15/17 AMD Gen3",
            Self::STELLARIS1XI03 => "Stellaris 15/17 Intel Gen3",
            Self::STELLARIS1XI04 => "Stellaris 15/17 Intel Gen4",
            Self::STEPOL1XA04 => "Stellaris/Polaris AMD Gen4",
            Self::STELLARIS1XI05 => "Stellaris 15/17 Intel Gen5",
            Self::STELLARIS1XA05 => "Stellaris 15/17 AMD Gen5",
            Self::STELLARIS16I06 => "Stellaris 16 Intel Gen6",
            Self::STELLARIS17I06 => "Stellaris 17 Intel Gen6",
            Self::STELLSL15A06 => "Stellaris Slim 15 AMD Gen6",
            Self::STELLSL15I06 => "Stellaris Slim 15 Intel Gen6",
            Self::STELLARIS16A07 => "Stellaris 16 AMD Gen7",
            Self::STELLARIS16I07 => "Stellaris 16 Intel Gen7",
            Self::GEMINI17I04 => "Gemini 17 Intel Gen4",
            Self::SIRIUS1601 => "Sirius 16 Gen1",
            Self::SIRIUS1602 => "Sirius 16 Gen2",
            Self::XNE16E25 => "NEO 16 Intel E25",
            Self::XNE16A25 => "NEO 16 AMD A25",
            Self::PCS_X6FR559Y => "X6FR559Y",
        }
    }

    pub fn year(&self) -> &'static str {
        match self {
            Self::IBP17G6 | Self::IBP14G6_TUX | Self::IBP14G6_TRX | Self::IBP14G6_TQF => "2021",
            Self::IBP14G7_AQF_ARX | Self::PULSE1502 | Self::POLARIS1XA02 | Self::POLARIS1XI02 => {
                "2022"
            }
            Self::IBPG8
            | Self::PULSE1403
            | Self::AURA14G3
            | Self::AURA15G3
            | Self::POLARIS1XA03
            | Self::POLARIS1XI03
            | Self::STELLARIS1XA03
            | Self::STELLARIS1XI03
            | Self::STELLARIS1XI04 => "2023",
            Self::PULSE1404
            | Self::STEPOL1XA04
            | Self::STELLARIS1XI05
            | Self::STELLARIS1XA05
            | Self::STELLARIS16I06
            | Self::STELLARIS17I06
            | Self::STELLSL15A06
            | Self::STELLSL15I06
            | Self::GEMINI17I04
            | Self::SIRIUS1601 => "2024",
            Self::IBPG10AMD
            | Self::IBM15A10
            | Self::POLARIS1XA05
            | Self::STELLARIS16A07
            | Self::STELLARIS16I07
            | Self::SIRIUS1602
            | Self::XNE16E25
            | Self::XNE16A25
            | Self::PCS_X6FR559Y => "2025",
        }
    }

    pub fn default_brand(&self) -> LaptopManufacturer {
        match self {
            Self::XNE16E25 | Self::XNE16A25 => LaptopManufacturer::Xmg,
            Self::PCS_X6FR559Y => LaptopManufacturer::PcSpecialist,
            _ => LaptopManufacturer::Tuxedo,
        }
    }
}

/// Supported device SKUs whitelist matching baseline `Utils.hpp`
pub const SUPPORTED_DEVICE_SKUS: &[&str] = &[
    "STELLARIS16A07",
    "STELLARIS16I07",
    "XNE16E25",
    "XNE16A25",
    "X6FR559Y",
    "IBP14A10MK1 / IBP15A10MK1",
];

/// Devices supporting Aquaris liquid water cooler matching baseline `UccDBusService.cpp`
pub const WATER_COOLER_DEVICES: &[UniwillDeviceID] = &[
    UniwillDeviceID::STELLARIS1XI04,
    UniwillDeviceID::STEPOL1XA04,
    UniwillDeviceID::STELLARIS1XI05,
    UniwillDeviceID::STELLARIS16I06,
    UniwillDeviceID::STELLARIS17I06,
    UniwillDeviceID::STELLARIS16A07,
    UniwillDeviceID::PCS_X6FR559Y,
    UniwillDeviceID::XNE16A25,
    UniwillDeviceID::XNE16E25,
    UniwillDeviceID::STELLARIS16I07,
];

/// Devices where cTGP adjustment is hidden matching baseline `UccDBusService.cpp`
pub const CTGP_HIDDEN_DEVICES: &[UniwillDeviceID] = &[
    UniwillDeviceID::IBP14G6_TUX,
    UniwillDeviceID::IBP14G6_TRX,
    UniwillDeviceID::IBP14G6_TQF,
    UniwillDeviceID::IBP14G7_AQF_ARX,
    UniwillDeviceID::IBPG8,
    UniwillDeviceID::IBPG10AMD,
];

/// DMI system information read from `/sys/class/dmi/id`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DmiInfo {
    pub sys_vendor: String,
    pub board_vendor: String,
    pub product_name: String,
    pub product_sku: String,
    pub board_name: String,
    pub board_version: String,
    pub bios_version: String,
}

impl DmiInfo {
    /// Read DMI tables from sysfs using the provided `Sysfs` abstraction.
    pub fn read_from_sysfs(sysfs: &dyn Sysfs) -> Self {
        let base = Path::new("/sys/class/dmi/id");
        let read_field = |name: &str| -> String {
            sysfs
                .read(&base.join(name))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };

        Self {
            sys_vendor: read_field("sys_vendor"),
            board_vendor: read_field("board_vendor"),
            product_name: read_field("product_name"),
            product_sku: read_field("product_sku"),
            board_name: read_field("board_name"),
            board_version: read_field("board_version"),
            bios_version: read_field("bios_version"),
        }
    }
}

/// Exact tuple for DMI quirk gating.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DmiTuple {
    pub sys_vendor: Option<&'static str>,
    pub board_vendor: Option<&'static str>,
    pub product_name: Option<&'static str>,
    pub product_sku: Option<&'static str>,
    pub board_name: Option<&'static str>,
    pub board_version: Option<&'static str>,
}

/// Hardware capabilities descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceCapabilities {
    pub vendor: DeviceVendor,
    pub device_id: Option<UniwillDeviceID>,
    pub number_of_fans: usize,
    pub fans_min_speed: i32,
    pub fans_off_available: bool,
    pub fan_control: bool,
    pub tdp_control: bool,
    pub webcam_switch: bool,
    pub flightmode_switch: bool,
    pub touchpad_switch: bool,
    pub water_cooler_supported: bool,
    pub charging_thresholds: bool,
    pub ctgp_supported: bool,
    pub hwp_dynamic_boost_supported: bool,
}

/// Detects the hardware vendor from device capabilities and ioctl checks.
///
/// Complies with `FT-F01-02`: Clevo, Uniwill and Dummy are selected from
/// hardware capabilities, never from laptop marketing name.
pub fn detect_vendor(hardware: &dyn HardwareIo) -> DeviceVendor {
    if !hardware.is_available() {
        return DeviceVendor::Unsupported;
    }

    // 1. Check Clevo driver capabilities
    if let Ok(1) = hardware.read_i32(requests::R_HWCHECK_CL) {
        return DeviceVendor::Clevo;
    }

    // 2. Check Uniwill driver capabilities
    if let Ok(1) = hardware.read_i32(requests::R_HWCHECK_UW) {
        return DeviceVendor::Uniwill;
    }

    // 3. Neither hardware interface responded
    DeviceVendor::Unsupported
}

/// Classify the laptop brand from DMI vendor strings.
pub fn classify_manufacturer(sys_vendor: &str, board_vendor: &str) -> LaptopManufacturer {
    let contains_ci = |haystack: &str, needle: &str| -> bool {
        haystack.to_lowercase().contains(&needle.to_lowercase())
    };

    if contains_ci(sys_vendor, "MECHREVO") || contains_ci(board_vendor, "MECHREVO") {
        return LaptopManufacturer::Mechrevo;
    }

    if contains_ci(sys_vendor, "TUXEDO") || contains_ci(board_vendor, "TUXEDO") {
        return LaptopManufacturer::Tuxedo;
    }

    if contains_ci(sys_vendor, "Schenker")
        || contains_ci(board_vendor, "Schenker")
        || contains_ci(sys_vendor, "XMG")
        || contains_ci(board_vendor, "XMG")
    {
        return LaptopManufacturer::Xmg;
    }

    if contains_ci(sys_vendor, "PCSpecialist") || contains_ci(board_vendor, "PCSpecialist") {
        return LaptopManufacturer::PcSpecialist;
    }

    if contains_ci(sys_vendor, "Uniwill") || contains_ci(board_vendor, "Uniwill") {
        return LaptopManufacturer::Uniwill;
    }

    LaptopManufacturer::Unknown
}

/// Construct human-readable laptop model string from DMI and device identification.
pub fn build_laptop_model(
    device_id: Option<UniwillDeviceID>,
    manufacturer: LaptopManufacturer,
    dmi: &DmiInfo,
) -> String {
    // Exact Mechrevo quirk: A capabilities alias describes the controller, not the laptop branding
    if manufacturer == LaptopManufacturer::Mechrevo && !dmi.board_name.is_empty() {
        return format!("MECHREVO {}", dmi.board_name);
    }

    if let Some(id) = device_id {
        let brand = if manufacturer != LaptopManufacturer::Unknown {
            manufacturer.as_str()
        } else {
            id.default_brand().as_str()
        };
        return format!("{} {} ({})", brand, id.model_name(), id.year());
    }

    if !dmi.product_name.is_empty() {
        if manufacturer != LaptopManufacturer::Unknown {
            return format!("{} {}", manufacturer.as_str(), dmi.product_name);
        }
        if !dmi.sys_vendor.is_empty() {
            return format!("{} {}", dmi.sys_vendor, dmi.product_name);
        }
        return dmi.product_name.clone();
    }

    "Unknown Laptop".to_string()
}

/// DMI Quirks and hardware-specific policy gates.
pub struct DmiQuirks;

impl DmiQuirks {
    /// Exact DMI tuple matching gate (FT-F01-01).
    ///
    /// Every near miss (typo, prefix mismatch, different revision) strictly fails the gate.
    pub fn exact_dmi_match(dmi: &DmiInfo, required: &DmiTuple) -> bool {
        if let Some(v) = required.sys_vendor {
            if dmi.sys_vendor != v {
                return false;
            }
        }
        if let Some(v) = required.board_vendor {
            if dmi.board_vendor != v {
                return false;
            }
        }
        if let Some(v) = required.product_name {
            if dmi.product_name != v {
                return false;
            }
        }
        if let Some(v) = required.product_sku {
            if dmi.product_sku != v {
                return false;
            }
        }
        if let Some(v) = required.board_name {
            if dmi.board_name != v {
                return false;
            }
        }
        if let Some(v) = required.board_version {
            if dmi.board_version != v {
                return false;
            }
        }
        true
    }

    /// Quirk: Devices with non-functional ACPI platform_profile (e.g. IBPG10AMD).
    pub fn has_acpi_platform_profile_quirk(device_id: Option<UniwillDeviceID>) -> bool {
        matches!(device_id, Some(UniwillDeviceID::IBPG10AMD))
    }

    /// Quirk: Devices with display MUX restricting prime-select (e.g. IBM15A10).
    pub fn has_display_mux_quirk(device_id: Option<UniwillDeviceID>) -> bool {
        matches!(device_id, Some(UniwillDeviceID::IBM15A10))
    }

    /// Quirk: Detection of CPU models or DMI combinations where EPP write fails.
    pub fn has_no_epp_write_quirk(dmi: &DmiInfo, cpu_model: &str) -> bool {
        // Known quirk on specific AMD mobile architectures without CPPC v2 EPP
        let is_amd = cpu_model.to_lowercase().contains("amd");
        let is_known_epp_bug = dmi.board_name.contains("NO_EPP_SUPPORT");
        is_amd && is_known_epp_bug
    }
}

/// Check whether the laptop is a whitelisted supported device matching baseline `Utils.hpp`.
pub fn is_device_supported(dmi: &DmiInfo) -> bool {
    if dmi.product_sku == "0001"
        && dmi.sys_vendor == "MECHREVO"
        && dmi.board_name == "YAOSHI Series-X6AR55xY"
        && dmi.board_version == "Standard"
    {
        return true;
    }

    SUPPORTED_DEVICE_SKUS.contains(&dmi.product_sku.as_str())
}

/// Identifies the specific Uniwill device ID from DMI and hardware descriptors.
pub fn identify_device(dmi: &DmiInfo, hardware: Option<&dyn HardwareIo>) -> Option<UniwillDeviceID> {
    // 1. Exact Mechrevo four-field alias
    if dmi.sys_vendor == "MECHREVO"
        && dmi.board_name == "YAOSHI Series-X6AR55xY"
        && dmi.board_version == "Standard"
        && dmi.product_sku == "0001"
    {
        return Some(UniwillDeviceID::STELLARIS16I07);
    }

    // 2. SKU matching table
    match dmi.product_sku.as_str() {
        "IBS1706" => return Some(UniwillDeviceID::IBP17G6),
        "IBP1XI08MK1" | "IBP1XI08MK2" | "IBP14I08MK2" | "IBP16I08MK2" | "OMNIA08IMK2" => {
            return Some(UniwillDeviceID::IBPG8);
        }
        "IBP14A10MK1 / IBP15A10MK1" | "IIBP14A10MK1 / IBP15A10MK1" => {
            return Some(UniwillDeviceID::IBPG10AMD);
        }
        "POLARIS1XA02" => return Some(UniwillDeviceID::POLARIS1XA02),
        "POLARIS1XI02" => return Some(UniwillDeviceID::POLARIS1XI02),
        "POLARIS1XA03" => return Some(UniwillDeviceID::POLARIS1XA03),
        "POLARIS1XI03" => return Some(UniwillDeviceID::POLARIS1XI03),
        "STELLARIS1XA03" => return Some(UniwillDeviceID::STELLARIS1XA03),
        "STEPOL1XA04" => return Some(UniwillDeviceID::STEPOL1XA04),
        "STELLARIS1XI03" => return Some(UniwillDeviceID::STELLARIS1XI03),
        "STELLARIS1XI04" => return Some(UniwillDeviceID::STELLARIS1XI04),
        "PULSE1502" => return Some(UniwillDeviceID::PULSE1502),
        "PULSE1403" => return Some(UniwillDeviceID::PULSE1403),
        "PULSE1404" => return Some(UniwillDeviceID::PULSE1404),
        "STELLARIS1XI05" => return Some(UniwillDeviceID::STELLARIS1XI05),
        "POLARIS1XA05" => return Some(UniwillDeviceID::POLARIS1XA05),
        "STELLARIS1XA05" => return Some(UniwillDeviceID::STELLARIS1XA05),
        "STELLARIS16I06" => return Some(UniwillDeviceID::STELLARIS16I06),
        "STELLARIS17I06" => return Some(UniwillDeviceID::STELLARIS17I06),
        "STELLSL15A06" => return Some(UniwillDeviceID::STELLSL15A06),
        "STELLSL15I06" => return Some(UniwillDeviceID::STELLSL15I06),
        "AURA14GEN3" => return Some(UniwillDeviceID::AURA14G3),
        "AURA15GEN3" => return Some(UniwillDeviceID::AURA15G3),
        "STELLARIS16A07" => return Some(UniwillDeviceID::STELLARIS16A07),
        "STELLARIS16I07" => return Some(UniwillDeviceID::STELLARIS16I07),
        "XNE16A25" => return Some(UniwillDeviceID::XNE16A25),
        "XNE16E25" => return Some(UniwillDeviceID::XNE16E25),
        "GEMINI17I04" | "GEMINIGEN4I" => return Some(UniwillDeviceID::GEMINI17I04),
        "IBM15A10" => return Some(UniwillDeviceID::IBM15A10),
        "SIRIUS1601" => return Some(UniwillDeviceID::SIRIUS1601),
        "SIRIUS1602" => return Some(UniwillDeviceID::SIRIUS1602),
        _ => {}
    }

    // 3. Product name matching table
    if dmi.product_name == "X6FR559Y" {
        return Some(UniwillDeviceID::PCS_X6FR559Y);
    }

    // 4. UWID (Uniwill WMI interface) model ID check via ioctl
    if let Some(hw) = hardware {
        if let Ok(model_id) = hw.read_i32(requests::R_UW_MODEL_ID) {
            match model_id {
                0x13 => return Some(UniwillDeviceID::IBP14G6_TUX),
                0x12 => return Some(UniwillDeviceID::IBP14G6_TRX),
                0x14 => return Some(UniwillDeviceID::IBP14G6_TQF),
                0x17 => return Some(UniwillDeviceID::IBP14G7_AQF_ARX),
                _ => {}
            }
        }
    }

    None
}

/// Checks if Aquaris liquid water cooler is supported for the device.
pub fn is_water_cooler_supported(device_id: Option<UniwillDeviceID>) -> bool {
    match device_id {
        Some(id) => WATER_COOLER_DEVICES.contains(&id),
        None => false,
    }
}

/// Checks if cTGP adjustment is supported.
pub fn is_ctgp_supported(device_id: Option<UniwillDeviceID>, sysfs: Option<&dyn Sysfs>) -> bool {
    if let Some(id) = device_id {
        !CTGP_HIDDEN_DEVICES.contains(&id)
    } else if let Some(s) = sysfs {
        s.exists(Path::new("/sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset"))
    } else {
        false
    }
}

/// Checks if Intel HWP dynamic boost is supported.
pub fn is_hwp_dynamic_boost_supported(sysfs: &dyn Sysfs) -> bool {
    sysfs.exists(Path::new("/sys/devices/system/cpu/intel_pstate/hwp_dynamic_boost"))
}

/// Detect device capabilities from hardware interface, DMI data, and sysfs.
///
/// When the hardware interface is unavailable or unsupported, produces an honest
/// fail-closed unsupported state with zero active features (FT-F01-03).
pub fn detect_device_capabilities(
    hardware: &dyn HardwareIo,
    dmi: Option<&DmiInfo>,
    sysfs: Option<&dyn Sysfs>,
) -> DeviceCapabilities {
    let vendor = detect_vendor(hardware);

    match vendor {
        DeviceVendor::Clevo => {
            let hwp = sysfs.map(is_hwp_dynamic_boost_supported).unwrap_or(false);
            DeviceCapabilities {
                vendor,
                device_id: None,
                number_of_fans: 3,
                fans_min_speed: 20,
                fans_off_available: true,
                fan_control: true,
                tdp_control: false,
                webcam_switch: true,
                flightmode_switch: true,
                touchpad_switch: true,
                water_cooler_supported: false,
                charging_thresholds: false,
                ctgp_supported: false,
                hwp_dynamic_boost_supported: hwp,
            }
        }
        DeviceVendor::Uniwill => {
            let device_id = dmi.and_then(|d| identify_device(d, Some(hardware)));
            let min_speed = hardware
                .read_i32(requests::R_UW_FANS_MIN_SPEED)
                .unwrap_or(0);
            let off_available = hardware
                .read_i32(requests::R_UW_FANS_OFF_AVAILABLE)
                .map(|v| v == 1)
                .unwrap_or(false);

            let water_cooler = is_water_cooler_supported(device_id);
            let ctgp = is_ctgp_supported(device_id, sysfs);
            let hwp = sysfs.map(is_hwp_dynamic_boost_supported).unwrap_or(false);

            DeviceCapabilities {
                vendor,
                device_id,
                number_of_fans: 2,
                fans_min_speed: min_speed,
                fans_off_available: off_available,
                fan_control: true,
                tdp_control: true,
                webcam_switch: false,
                flightmode_switch: false,
                touchpad_switch: false,
                water_cooler_supported: water_cooler,
                charging_thresholds: true,
                ctgp_supported: ctgp,
                hwp_dynamic_boost_supported: hwp,
            }
        }
        DeviceVendor::Unsupported => DeviceCapabilities {
            vendor: DeviceVendor::Unsupported,
            device_id: None,
            number_of_fans: 0,
            fans_min_speed: 0,
            fans_off_available: false,
            fan_control: false,
            tdp_control: false,
            webcam_switch: false,
            flightmode_switch: false,
            touchpad_switch: false,
            water_cooler_supported: false,
            charging_thresholds: false,
            ctgp_supported: false,
            hwp_dynamic_boost_supported: false,
        },
    }
}
