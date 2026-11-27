use crate::charging::{ChargeType, ChargingPolicy};
use crate::cpu::ProfileCpu;
use crate::error::ValidationError;
use crate::fan::{FanCurve, FanPoint, PumpPoint, PumpVoltage};
use serde::{Deserialize, Serialize};

/// Display resolution and refresh rate settings for a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDisplay {
    #[serde(default = "default_brightness")]
    pub brightness: i32,
    #[serde(default)]
    pub use_brightness: bool,
    #[serde(default = "default_neg_one")]
    pub refresh_rate: i32,
    #[serde(default)]
    pub use_ref_rate: bool,
    #[serde(default = "default_neg_one")]
    pub x_resolution: i32,
    #[serde(default = "default_neg_one")]
    pub y_resolution: i32,
    #[serde(default)]
    pub use_resolution: bool,
}

impl Default for ProfileDisplay {
    fn default() -> Self {
        Self {
            brightness: 100,
            use_brightness: false,
            refresh_rate: -1,
            use_ref_rate: false,
            x_resolution: -1,
            y_resolution: -1,
            use_resolution: false,
        }
    }
}

impl ProfileDisplay {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.use_brightness && !(0..=100).contains(&self.brightness) {
            return Err(ValidationError::InvalidDisplayBrightness(self.brightness));
        }
        Ok(())
    }
}

/// Webcam toggle status for a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileWebcam {
    #[serde(default = "default_true")]
    pub status: bool,
    #[serde(default = "default_true")]
    pub use_status: bool,
}

impl Default for ProfileWebcam {
    fn default() -> Self {
        Self {
            status: true,
            use_status: true,
        }
    }
}

mod manual_fan_speed_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(val: &Option<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match val {
            Some(v) => serializer.serialize_some(v),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<i64> = Option::deserialize(deserializer)?;
        Ok(match opt {
            Some(v) if (0..=100).contains(&v) => Some(v as u8),
            _ => None, // -1 sentinel or None
        })
    }
}

mod manual_pump_voltage_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(val: &Option<i32>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match val {
            Some(v) => serializer.serialize_some(v),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<i32> = Option::deserialize(deserializer)?;
        Ok(match opt {
            Some(v) if v >= 0 => Some(v),
            _ => None, // -1 sentinel or None
        })
    }
}

/// Fan and water cooler control settings within a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileFanControl {
    #[serde(default = "default_true")]
    pub use_control: bool,
    #[serde(default = "default_fan_profile")]
    pub fan_profile: String,
    #[serde(default = "default_true")]
    pub same_speed: bool,
    #[serde(rename = "autoControlWC", default = "default_true")]
    pub auto_control_wc: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "manual_fan_speed_serde::deserialize",
        serialize_with = "manual_fan_speed_serde::serialize"
    )]
    pub manual_fan_speed: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "manual_pump_voltage_serde::deserialize",
        serialize_with = "manual_pump_voltage_serde::serialize"
    )]
    pub manual_pump_voltage: Option<i32>,
    #[serde(default)]
    pub enable_water_cooler: bool,
    #[serde(rename = "tableCPU", default)]
    pub table_cpu: Vec<FanPoint>,
    #[serde(rename = "tableGPU", default)]
    pub table_gpu: Vec<FanPoint>,
    #[serde(rename = "tablePump", default)]
    pub table_pump: Vec<PumpPoint>,
    #[serde(rename = "tableWaterCoolerFan", default)]
    pub table_water_cooler_fan: Vec<FanPoint>,
}

impl Default for ProfileFanControl {
    fn default() -> Self {
        Self {
            use_control: true,
            fan_profile: "fan-balanced".to_string(),
            same_speed: true,
            auto_control_wc: true,
            manual_fan_speed: None,
            manual_pump_voltage: None,
            enable_water_cooler: false,
            table_cpu: Vec::new(),
            table_gpu: Vec::new(),
            table_pump: Vec::new(),
            table_water_cooler_fan: Vec::new(),
        }
    }
}

impl ProfileFanControl {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(speed) = self.manual_fan_speed {
            if speed > 100 {
                return Err(ValidationError::InvalidFanSpeed { speed });
            }
        }

        if let Some(voltage) = self.manual_pump_voltage {
            PumpVoltage::from_discrete_code(voltage)?;
        }

        if !self.table_cpu.is_empty() {
            FanCurve::new(self.table_cpu.clone())?.validate()?;
        }
        if !self.table_gpu.is_empty() {
            FanCurve::new(self.table_gpu.clone())?.validate()?;
        }
        if !self.table_water_cooler_fan.is_empty() {
            FanCurve::new(self.table_water_cooler_fan.clone())?.validate()?;
        }
        for point in &self.table_pump {
            point.validate()?;
        }

        Ok(())
    }
}

/// Keyboard backlight references within a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileKeyboard {
    #[serde(default)]
    pub keyboard_profile_name: String,
    #[serde(default)]
    pub keyboard_profile_id: String,
    #[serde(default)]
    pub keyboard_profile_data: String,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Per-profile lid-close lighting behavior overrides.
///
/// Missing fields fall back to global `powerDevices.lid` settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLidLighting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_off_keyboard_backlight_on_lid_close: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_off_water_cooler_led_on_lid_close: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum OdmPowerLimitsSerde {
    Object {
        #[serde(rename = "tdpValues", default)]
        tdp_values: Vec<u32>,
    },
    Array(Vec<u32>),
}

fn deserialize_odm_power_limits<'de, D>(deserializer: D) -> Result<Vec<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<OdmPowerLimitsSerde>::deserialize(deserializer)?;
    match opt {
        Some(OdmPowerLimitsSerde::Object { tdp_values }) => Ok(tdp_values),
        Some(OdmPowerLimitsSerde::Array(vals)) => Ok(vals),
        None => Ok(Vec::new()),
    }
}

fn serialize_odm_power_limits<S>(limits: &Vec<u32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry("tdpValues", limits)?;
    map.end()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum OdmProfileSerde {
    Object { name: String },
    String(String),
}

fn deserialize_odm_profile<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<OdmProfileSerde>::deserialize(deserializer)?;
    match opt {
        Some(OdmProfileSerde::Object { name }) => Ok(Some(name)),
        Some(OdmProfileSerde::String(s)) => Ok(Some(s)),
        None => Ok(None),
    }
}

fn serialize_odm_profile<S>(name: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    match name {
        Some(n) => {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("name", n)?;
            map.end()
        }
        None => serializer.serialize_none(),
    }
}

/// Complete UCC system performance and hardware configuration profile.
#[derive(Debug, Clone, PartialEq)]
pub struct UccProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub display: ProfileDisplay,
    pub cpu: ProfileCpu,
    pub webcam: ProfileWebcam,
    pub fan: ProfileFanControl,
    pub keyboard: ProfileKeyboard,
    pub lid: Option<ProfileLidLighting>,
    pub odm_profile_name: Option<String>,
    pub odm_power_limits: Vec<u32>,
    pub nvidia_ctgp_offset: Option<i32>,
    pub selected_keyboard_profile: Option<String>,
    pub charging: ChargingPolicy,
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UccProfileWire {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    display: ProfileDisplay,
    #[serde(default)]
    cpu: ProfileCpu,
    #[serde(default)]
    webcam: ProfileWebcam,
    #[serde(default)]
    fan: ProfileFanControl,
    #[serde(default)]
    keyboard: ProfileKeyboard,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lid: Option<ProfileLidLighting>,
    #[serde(
        default,
        rename = "odmProfile",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_odm_profile",
        serialize_with = "serialize_odm_profile"
    )]
    odm_profile_name: Option<String>,
    #[serde(
        default,
        rename = "odmPowerLimits",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_odm_power_limits",
        serialize_with = "serialize_odm_power_limits"
    )]
    odm_power_limits: Vec<u32>,
    #[serde(
        default,
        rename = "nvidiaCTGPOffset",
        skip_serializing_if = "Option::is_none"
    )]
    nvidia_ctgp_offset: Option<i32>,
    #[serde(
        default,
        rename = "selectedKeyboardProfile",
        skip_serializing_if = "Option::is_none"
    )]
    selected_keyboard_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    charging: Option<ChargingPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    charging_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    charging_priority: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::charging::deserialize_opt_charge_type"
    )]
    charge_type: Option<ChargeType>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::charging::deserialize_opt_threshold"
    )]
    charge_start_threshold: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::charging::deserialize_opt_threshold"
    )]
    charge_end_threshold: Option<u8>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for UccProfile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = UccProfileWire::deserialize(deserializer)?;
        let mut charging = wire.charging.unwrap_or_default();
        if wire.charging_profile.is_some() {
            charging.charging_profile = wire.charging_profile;
        }
        if wire.charging_priority.is_some() {
            charging.charging_priority = wire.charging_priority;
        }
        if wire.charge_type.is_some() {
            charging.charge_type = wire.charge_type;
        }
        if wire.charge_start_threshold.is_some() {
            charging.start_threshold = wire.charge_start_threshold;
        }
        if wire.charge_end_threshold.is_some() {
            charging.end_threshold = wire.charge_end_threshold;
        }
        Ok(Self {
            id: wire.id,
            name: wire.name,
            description: wire.description,
            display: wire.display,
            cpu: wire.cpu,
            webcam: wire.webcam,
            fan: wire.fan,
            keyboard: wire.keyboard,
            lid: wire.lid,
            odm_profile_name: wire.odm_profile_name,
            odm_power_limits: wire.odm_power_limits,
            nvidia_ctgp_offset: wire.nvidia_ctgp_offset,
            selected_keyboard_profile: wire.selected_keyboard_profile,
            charging,
            extra: wire.extra,
        })
    }
}

impl Serialize for UccProfile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let wire = UccProfileWire {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            display: self.display.clone(),
            cpu: self.cpu.clone(),
            webcam: self.webcam.clone(),
            fan: self.fan.clone(),
            keyboard: self.keyboard.clone(),
            lid: self.lid.clone(),
            odm_profile_name: self.odm_profile_name.clone(),
            odm_power_limits: self.odm_power_limits.clone(),
            nvidia_ctgp_offset: self.nvidia_ctgp_offset,
            selected_keyboard_profile: self.selected_keyboard_profile.clone(),
            charging: None,
            charging_profile: self.charging.charging_profile.clone(),
            charging_priority: self.charging.charging_priority.clone(),
            charge_type: self.charging.charge_type,
            charge_start_threshold: self.charging.start_threshold,
            charge_end_threshold: self.charging.end_threshold,
            extra: self.extra.clone(),
        };
        wire.serialize(serializer)
    }
}

impl UccProfile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            display: ProfileDisplay::default(),
            cpu: ProfileCpu::default(),
            webcam: ProfileWebcam::default(),
            fan: ProfileFanControl::default(),
            keyboard: ProfileKeyboard::default(),
            lid: None,
            odm_profile_name: None,
            odm_power_limits: Vec::new(),
            nvidia_ctgp_offset: None,
            selected_keyboard_profile: None,
            charging: ChargingPolicy::default(),
            extra: std::collections::BTreeMap::new(),
        }
    }

    /// Validates the profile against all domain invariants:
    /// - Display brightness
    /// - CPU limits, frequency bounds, core selection
    /// - ODM power limits (TDP range and monotonicity)
    /// - Fan speed, pump voltage, and curve monotonicity
    /// - Charging policy thresholds
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.display.validate()?;
        self.cpu.validate(&self.odm_power_limits)?;
        self.fan.validate()?;
        self.charging.validate()?;
        Ok(())
    }
}

const fn default_brightness() -> i32 {
    100
}
const fn default_neg_one() -> i32 {
    -1
}
const fn default_true() -> bool {
    true
}
fn default_fan_profile() -> String {
    "fan-balanced".to_string()
}
