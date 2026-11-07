use crate::charging::{ChargeType, ChargingPolicy};
use crate::cpu::ProfileCpu;
use crate::error::{CoreError, ValidationError};
use crate::fan::{FanPoint, PumpPoint, PumpVoltage};
use crate::power::PolicyDecision;
use crate::profile::{ProfileDisplay, ProfileFanControl, ProfileKeyboard, ProfileWebcam, UccProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Legacy coordinate entry for fan speed or pump curve point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyFanPointDto {
    pub temp: i32,
    pub speed: i32,
}

/// Legacy wire DTO for display settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDisplayDto {
    #[serde(default = "default_hundred")]
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

const fn default_hundred() -> i32 {
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

/// Legacy wire DTO for CPU settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacyCpuDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_cores: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_cores: Option<Vec<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scaling_min_frequency: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scaling_max_frequency: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_performance_preference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_turbo: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwp_dynamic_boost: Option<bool>,
}

/// Legacy wire DTO for webcam settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyWebcamDto {
    #[serde(default = "default_true")]
    pub status: bool,
    #[serde(default = "default_true")]
    pub use_status: bool,
}

/// Legacy wire DTO for fan control settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFanDto {
    #[serde(default = "default_true")]
    pub use_control: bool,
    #[serde(default = "default_fan_profile")]
    pub fan_profile: String,
    #[serde(default = "default_true")]
    pub same_speed: bool,
    #[serde(rename = "autoControlWC", default = "default_true")]
    pub auto_control_wc: bool,
    #[serde(default = "default_neg_one")]
    pub manual_fan_speed: i32,
    #[serde(default = "default_neg_one")]
    pub manual_pump_voltage: i32,
    #[serde(default)]
    pub enable_water_cooler: bool,
    #[serde(rename = "tableCPU", default, skip_serializing_if = "Vec::is_empty")]
    pub table_cpu: Vec<LegacyFanPointDto>,
    #[serde(rename = "tableGPU", default, skip_serializing_if = "Vec::is_empty")]
    pub table_gpu: Vec<LegacyFanPointDto>,
    #[serde(rename = "tablePump", default, skip_serializing_if = "Vec::is_empty")]
    pub table_pump: Vec<LegacyFanPointDto>,
    #[serde(rename = "tableWaterCoolerFan", default, skip_serializing_if = "Vec::is_empty")]
    pub table_water_cooler_fan: Vec<LegacyFanPointDto>,
}

/// Legacy wire DTO for keyboard backlight references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacyKeyboardDto {
    #[serde(default)]
    pub keyboard_profile_data: Option<String>,
    #[serde(default)]
    pub keyboard_profile_name: Option<String>,
}

/// Legacy wire DTO for ODM profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LegacyOdmProfileDto {
    #[serde(default)]
    pub name: Option<String>,
}

/// Legacy wire DTO for ODM power limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacyOdmPowerLimitsDto {
    #[serde(default)]
    pub tdp_values: Vec<i32>,
}

/// Legacy wire DTO for complete profile representation across D-Bus and JSON files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacyProfileDto {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<LegacyDisplayDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<LegacyCpuDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webcam: Option<LegacyWebcamDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan: Option<LegacyFanDto>,
    #[serde(default, rename = "odmProfile", skip_serializing_if = "Option::is_none")]
    pub odm_profile: Option<LegacyOdmProfileDto>,
    #[serde(default, rename = "odmPowerLimits", skip_serializing_if = "Option::is_none")]
    pub odm_power_limits: Option<LegacyOdmPowerLimitsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<LegacyKeyboardDto>,
    #[serde(default, rename = "selectedKeyboardProfile", skip_serializing_if = "Option::is_none")]
    pub selected_keyboard_profile: Option<String>,
    #[serde(default, rename = "nvidiaCTGPOffset", skip_serializing_if = "Option::is_none")]
    pub nvidia_ctgp_offset: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_start_threshold: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_end_threshold: Option<i32>,
}

impl LegacyProfileDto {
    /// Converts legacy DTO into the internal strongly-typed `UccProfile`.
    pub fn into_profile(self) -> Result<UccProfile, ValidationError> {
        let display = if let Some(d) = self.display {
            ProfileDisplay {
                brightness: d.brightness,
                use_brightness: d.use_brightness,
                refresh_rate: d.refresh_rate,
                use_ref_rate: d.use_ref_rate,
                x_resolution: d.x_resolution,
                y_resolution: d.y_resolution,
                use_resolution: d.use_resolution,
            }
        } else {
            ProfileDisplay::default()
        };

        let cpu = if let Some(c) = self.cpu {
            ProfileCpu {
                online_cores: c.online_cores.and_then(|v| if v > 0 { Some(v as u32) } else { None }),
                enabled_cores: c.enabled_cores.map(|cores| {
                    cores.into_iter().filter(|&x| x >= 0).map(|x| x as u32).collect()
                }),
                min_freq_khz: c.scaling_min_frequency.and_then(|v| if v > 0 { Some(v as u32) } else { None }),
                max_freq_khz: c.scaling_max_frequency.and_then(|v| if v > 0 { Some(v as u32) } else { None }),
                governor: c.governor.unwrap_or_default(),
                energy_performance_preference: c.energy_performance_preference.unwrap_or_default(),
                no_turbo: c.no_turbo.unwrap_or(false),
                hwp_dynamic_boost: c.hwp_dynamic_boost.unwrap_or(false),
            }
        } else {
            ProfileCpu::default()
        };

        let webcam = if let Some(w) = self.webcam {
            ProfileWebcam {
                status: w.status,
                use_status: w.use_status,
            }
        } else {
            ProfileWebcam::default()
        };

        let fan = if let Some(f) = self.fan {
            let manual_fan_speed = if (0..=100).contains(&f.manual_fan_speed) {
                Some(f.manual_fan_speed as u8)
            } else {
                None
            };

            let manual_pump_voltage = if PumpVoltage::is_valid_manual_code(f.manual_pump_voltage) {
                Some(f.manual_pump_voltage)
            } else {
                None
            };

            let table_cpu = f.table_cpu.into_iter()
                .map(|p| FanPoint::new(p.temp, p.speed.clamp(0, 100) as u8))
                .collect();
            let table_gpu = f.table_gpu.into_iter()
                .map(|p| FanPoint::new(p.temp, p.speed.clamp(0, 100) as u8))
                .collect();
            let table_water_cooler_fan = f.table_water_cooler_fan.into_iter()
                .map(|p| FanPoint::new(p.temp, p.speed.clamp(0, 100) as u8))
                .collect();
            let table_pump = f.table_pump.into_iter()
                .map(|p| PumpPoint::new(p.temp, p.speed.clamp(0, 4) as u8))
                .collect();

            ProfileFanControl {
                use_control: f.use_control,
                fan_profile: f.fan_profile,
                same_speed: f.same_speed,
                auto_control_wc: f.auto_control_wc,
                manual_fan_speed,
                manual_pump_voltage,
                enable_water_cooler: f.enable_water_cooler,
                table_cpu,
                table_gpu,
                table_pump,
                table_water_cooler_fan,
            }
        } else {
            ProfileFanControl::default()
        };

        let keyboard = ProfileKeyboard {
            keyboard_profile_name: self.keyboard.as_ref().and_then(|k| k.keyboard_profile_name.clone()).unwrap_or_default(),
            keyboard_profile_id: self.selected_keyboard_profile.clone().unwrap_or_default(),
            keyboard_profile_data: self.keyboard.as_ref().and_then(|k| k.keyboard_profile_data.clone()).unwrap_or_default(),
        };

        let odm_power_limits = self.odm_power_limits
            .map(|l| l.tdp_values.into_iter().filter(|&x| x > 0).map(|x| x as u32).collect())
            .unwrap_or_default();

        let charge_type = match self.charge_type.as_deref() {
            Some("Custom") => Some(ChargeType::Custom),
            Some("Standard") => Some(ChargeType::Standard),
            _ => None,
        };

        let start_threshold = self.charge_start_threshold.and_then(|t| if (0..=100).contains(&t) { Some(t as u8) } else { None });
        let end_threshold = self.charge_end_threshold.and_then(|t| if (0..=100).contains(&t) { Some(t as u8) } else { None });

        let charging = ChargingPolicy {
            charging_profile: self.charging_profile,
            charging_priority: self.charging_priority,
            charge_type,
            start_threshold,
            end_threshold,
        };

        let profile = UccProfile {
            id: self.id,
            name: self.name,
            description: self.description.unwrap_or_default(),
            display,
            cpu,
            webcam,
            fan,
            keyboard,
            odm_profile_name: self.odm_profile.and_then(|o| o.name),
            odm_power_limits,
            nvidia_ctgp_offset: self.nvidia_ctgp_offset,
            selected_keyboard_profile: self.selected_keyboard_profile,
            charging,
        };

        profile.validate()?;
        Ok(profile)
    }

    /// Converts internal strongly-typed `UccProfile` into legacy DTO matching C++ wire schema.
    pub fn from_profile(profile: &UccProfile) -> Self {
        let display = Some(LegacyDisplayDto {
            brightness: profile.display.brightness,
            use_brightness: profile.display.use_brightness,
            refresh_rate: profile.display.refresh_rate,
            use_ref_rate: profile.display.use_ref_rate,
            x_resolution: profile.display.x_resolution,
            y_resolution: profile.display.y_resolution,
            use_resolution: profile.display.use_resolution,
        });

        let cpu = Some(LegacyCpuDto {
            online_cores: profile.cpu.online_cores.map(|v| v as i32),
            enabled_cores: profile.cpu.enabled_cores.as_ref().map(|cores| cores.iter().map(|&x| x as i32).collect()),
            scaling_min_frequency: profile.cpu.min_freq_khz.map(|v| v as i32),
            scaling_max_frequency: profile.cpu.max_freq_khz.map(|v| v as i32),
            governor: if profile.cpu.governor.is_empty() { None } else { Some(profile.cpu.governor.clone()) },
            energy_performance_preference: if profile.cpu.energy_performance_preference.is_empty() { None } else { Some(profile.cpu.energy_performance_preference.clone()) },
            no_turbo: Some(profile.cpu.no_turbo),
            hwp_dynamic_boost: Some(profile.cpu.hwp_dynamic_boost),
        });

        let webcam = Some(LegacyWebcamDto {
            status: profile.webcam.status,
            use_status: profile.webcam.use_status,
        });

        let fan = Some(LegacyFanDto {
            use_control: profile.fan.use_control,
            fan_profile: profile.fan.fan_profile.clone(),
            same_speed: profile.fan.same_speed,
            auto_control_wc: profile.fan.auto_control_wc,
            manual_fan_speed: profile.fan.manual_fan_speed.map(|s| s as i32).unwrap_or(-1),
            manual_pump_voltage: profile.fan.manual_pump_voltage.unwrap_or(-1),
            enable_water_cooler: profile.fan.enable_water_cooler,
            table_cpu: profile.fan.table_cpu.iter().map(|p| LegacyFanPointDto { temp: p.temp, speed: p.speed as i32 }).collect(),
            table_gpu: profile.fan.table_gpu.iter().map(|p| LegacyFanPointDto { temp: p.temp, speed: p.speed as i32 }).collect(),
            table_pump: profile.fan.table_pump.iter().map(|p| LegacyFanPointDto { temp: p.temp, speed: p.speed as i32 }).collect(),
            table_water_cooler_fan: profile.fan.table_water_cooler_fan.iter().map(|p| LegacyFanPointDto { temp: p.temp, speed: p.speed as i32 }).collect(),
        });

        let keyboard = Some(LegacyKeyboardDto {
            keyboard_profile_data: if profile.keyboard.keyboard_profile_data.is_empty() { None } else { Some(profile.keyboard.keyboard_profile_data.clone()) },
            keyboard_profile_name: if profile.keyboard.keyboard_profile_name.is_empty() { None } else { Some(profile.keyboard.keyboard_profile_name.clone()) },
        });

        let odm_profile = profile.odm_profile_name.as_ref().map(|name| LegacyOdmProfileDto {
            name: Some(name.clone()),
        });

        let odm_power_limits = if profile.odm_power_limits.is_empty() {
            None
        } else {
            Some(LegacyOdmPowerLimitsDto {
                tdp_values: profile.odm_power_limits.iter().map(|&x| x as i32).collect(),
            })
        };

        let charge_type = profile.charging.charge_type.map(|t| match t {
            ChargeType::Standard => "Standard".to_string(),
            ChargeType::Custom => "Custom".to_string(),
        });

        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            description: if profile.description.is_empty() { None } else { Some(profile.description.clone()) },
            display,
            cpu,
            webcam,
            fan,
            odm_profile,
            odm_power_limits,
            keyboard,
            selected_keyboard_profile: if profile.keyboard.keyboard_profile_id.is_empty() {
                profile.selected_keyboard_profile.clone()
            } else {
                Some(profile.keyboard.keyboard_profile_id.clone())
            },
            nvidia_ctgp_offset: profile.nvidia_ctgp_offset,
            charging_profile: profile.charging.charging_profile.clone(),
            charging_priority: profile.charging.charging_priority.clone(),
            charge_type,
            charge_start_threshold: profile.charging.start_threshold.map(|v| v as i32),
            charge_end_threshold: profile.charging.end_threshold.map(|v| v as i32),
        }
    }

    /// Parses JSON string from D-Bus into typed `UccProfile`.
    pub fn parse_json(json: &str) -> Result<UccProfile, CoreError> {
        let dto: Self = serde_json::from_str(json)?;
        let profile = dto.into_profile()?;
        Ok(profile)
    }

    /// Serializes typed `UccProfile` into legacy wire JSON.
    pub fn to_json(profile: &UccProfile) -> Result<String, CoreError> {
        let dto = Self::from_profile(profile);
        let s = serde_json::to_string(&dto)?;
        Ok(s)
    }
}

/// Legacy wire DTO for `/etc/ucc/settings.json` and `GetSettingsJSON()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacySettingsDto {
    #[serde(default)]
    pub fahrenheit: bool,
    #[serde(default)]
    pub state_map: BTreeMap<String, String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub custom_keyboard_profiles: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub custom_fan_profiles: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_time: Option<String>,
    #[serde(default = "default_true")]
    pub cpu_settings_enabled: bool,
    #[serde(default = "default_true")]
    pub fan_control_enabled: bool,
    #[serde(default = "default_true")]
    pub keyboard_backlight_control_supported: bool,
    #[serde(default = "default_auto")]
    pub keyboard_layout: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_backlight_states: Option<serde_json::Value>,
    #[serde(default, rename = "powerDevices", skip_serializing_if = "Option::is_none")]
    pub power_devices: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightbar: Option<serde_json::Value>,
    #[serde(default, rename = "ycbcr420Workaround", skip_serializing_if = "Option::is_none")]
    pub ycbcr420_workaround: Option<Vec<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_start_threshold: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_end_threshold: Option<i32>,
}

fn default_auto() -> String {
    "auto".to_string()
}

/// Legacy DTO for `GetPowerPolicyJSON()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyPowerPolicyStatusDto {
    pub real_power_source: String,
    pub current_mode: String,
    pub manual_mode: String,
    pub profile_state: String,
    pub water_cooler_connected: bool,
    pub registered_apps: Vec<LegacyAppPowerRequestDto>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyAppPowerRequestDto {
    pub owner: String,
    pub name: String,
    pub reason: String,
    pub force_mode: String,
}

impl From<&PolicyDecision> for LegacyPowerPolicyStatusDto {
    fn from(d: &PolicyDecision) -> Self {
        let apps: Vec<LegacyAppPowerRequestDto> = d.status["registered_apps"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| LegacyAppPowerRequestDto {
                        owner: v["owner"].as_str().unwrap_or_default().to_string(),
                        name: v["name"].as_str().unwrap_or_default().to_string(),
                        reason: v["reason"].as_str().unwrap_or_default().to_string(),
                        force_mode: v["force_mode"].as_str().unwrap_or_default().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            real_power_source: d.source.mode_name().to_string(),
            current_mode: d.effective.mode_name().to_string(),
            manual_mode: d.status["manual_mode"].as_str().unwrap_or("auto").to_string(),
            profile_state: d.profile_state.as_str().to_string(),
            water_cooler_connected: d.status["water_cooler_connected"].as_bool().unwrap_or(false),
            registered_apps: apps,
            revision: d.status["revision"].as_u64().unwrap_or(0),
        }
    }
}

/// Legacy fan metric entry (speed or temperature timestamped reading).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyMetricDataDto {
    pub timestamp: i64,
    pub data: i32,
}

/// Legacy fan telemetry structure returned as `a{sv}` by `GetFanDataCPU`, `GetFanDataGPU1`, `GetFanDataGPU2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyFanDataDto {
    pub speed: LegacyMetricDataDto,
    pub temp: LegacyMetricDataDto,
}
