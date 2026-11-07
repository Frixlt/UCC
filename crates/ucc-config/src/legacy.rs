//! Configuration management, persistence, and legacy DTO mapping for UCC.
//!
//! Handles `/etc/ucc/settings.json`, atomic writing with backup/recovery,
//! startup profile resolution, and round-trip parsing compatible with legacy C++ UCC.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use ucc_core::{ProfileState, UccProfile};

pub const DEFAULT_SETTINGS_FILE: &str = "/etc/ucc/settings.json";

use crate::error::ConfigError;

/// YCbCr 4:2:0 workaround port configuration
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct YCbCr420Port {
    pub port: String,
    pub enabled: bool,
}

/// YCbCr 4:2:0 workaround card configuration
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct YCbCr420Card {
    pub card: i32,
    pub ports: Vec<YCbCr420Port>,
}

/// In-memory representation of global UCC settings matching `TccSettings`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TccSettings {
    pub fahrenheit: bool,
    pub state_map: BTreeMap<String, String>,
    pub profiles: BTreeMap<String, String>,
    pub custom_keyboard_profiles: BTreeMap<String, String>,
    pub custom_fan_profiles: BTreeMap<String, String>,
    pub shutdown_time: Option<String>,
    pub cpu_settings_enabled: bool,
    pub fan_control_enabled: bool,
    pub keyboard_backlight_control_supported: bool,
    pub keyboard_layout: String,
    pub keyboard_backlight_states: String,
    pub power_device_settings: String,
    pub lightbar_settings: String,
    pub ycbcr420_workaround: Vec<YCbCr420Card>,
    pub charging_profile: Option<String>,
    pub charging_priority: Option<String>,
    pub charge_type: Option<String>,
    pub charge_start_threshold: Option<i32>,
    pub charge_end_threshold: Option<i32>,
}

impl Default for TccSettings {
    fn default() -> Self {
        Self {
            fahrenheit: false,
            state_map: BTreeMap::new(),
            profiles: BTreeMap::new(),
            custom_keyboard_profiles: BTreeMap::new(),
            custom_fan_profiles: BTreeMap::new(),
            shutdown_time: None,
            cpu_settings_enabled: true,
            fan_control_enabled: true,
            keyboard_backlight_control_supported: true,
            keyboard_layout: "auto".to_string(),
            keyboard_backlight_states: "[]".to_string(),
            power_device_settings: "{}".to_string(),
            lightbar_settings: "{}".to_string(),
            ycbcr420_workaround: Vec::new(),
            charging_profile: None,
            charging_priority: None,
            charge_type: None,
            charge_start_threshold: None,
            charge_end_threshold: None,
        }
    }
}

/// Manager responsible for reading, writing, and parsing UCC settings files.
#[derive(Debug, Clone)]
pub struct SettingsManager {
    settings_file: PathBuf,
}

impl Default for SettingsManager {
    fn default() -> Self {
        Self::new(DEFAULT_SETTINGS_FILE)
    }
}

impl SettingsManager {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            settings_file: path.into(),
        }
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_file
    }

    /// Parses settings from a JSON string with resilience to missing fields and legacy formats.
    pub fn parse_settings_json(&self, json_str: &str) -> Option<TccSettings> {
        if json_str.trim().is_empty() {
            return None;
        }

        let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
        if !parsed.is_object() {
            return None;
        }

        let mut settings = TccSettings::default();

        // stateMap
        if let Some(state_map) = parsed.get("stateMap").and_then(|v| v.as_object()) {
            for (k, v) in state_map {
                if let Some(val_str) = v.as_str() {
                    settings.state_map.insert(k.clone(), val_str.to_string());
                }
            }
        }

        // profiles
        if let Some(profiles) = parsed.get("profiles").and_then(|v| v.as_object()) {
            for (k, v) in profiles {
                settings.profiles.insert(k.clone(), v.to_string());
            }
        }

        // customKeyboardProfiles
        if let Some(kbps) = parsed.get("customKeyboardProfiles").and_then(|v| v.as_object()) {
            for (k, v) in kbps {
                settings.custom_keyboard_profiles.insert(k.clone(), v.to_string());
            }
        }

        // customFanProfiles
        if let Some(fps) = parsed.get("customFanProfiles").and_then(|v| v.as_object()) {
            for (k, v) in fps {
                settings.custom_fan_profiles.insert(k.clone(), v.to_string());
            }
        }

        // booleans
        if let Some(b) = parsed.get("fahrenheit").and_then(|v| v.as_bool()) {
            settings.fahrenheit = b;
        }
        if let Some(b) = parsed.get("cpuSettingsEnabled").and_then(|v| v.as_bool()) {
            settings.cpu_settings_enabled = b;
        }
        if let Some(b) = parsed.get("fanControlEnabled").and_then(|v| v.as_bool()) {
            settings.fan_control_enabled = b;
        }
        if let Some(b) = parsed.get("keyboardBacklightControlSupported").and_then(|v| v.as_bool()) {
            settings.keyboard_backlight_control_supported = b;
        }

        // strings and objects
        if let Some(s) = parsed.get("keyboardLayout").and_then(|v| v.as_str()) {
            settings.keyboard_layout = s.to_string();
        }
        if let Some(v) = parsed.get("keyboardBacklightStates") {
            settings.keyboard_backlight_states = v.to_string();
        }
        if let Some(v) = parsed.get("powerDevices") {
            settings.power_device_settings = v.to_string();
        }
        if let Some(v) = parsed.get("lightbar") {
            settings.lightbar_settings = v.to_string();
        }

        // optional string fields
        if let Some(s) = parsed.get("shutdownTime").and_then(|v| v.as_str()) {
            settings.shutdown_time = Some(s.to_string());
        }
        if let Some(s) = parsed.get("chargingProfile").and_then(|v| v.as_str()) {
            settings.charging_profile = Some(s.to_string());
        }
        if let Some(s) = parsed.get("chargingPriority").and_then(|v| v.as_str()) {
            settings.charging_priority = Some(s.to_string());
        }
        if let Some(s) = parsed.get("chargeType").and_then(|v| v.as_str()) {
            settings.charge_type = Some(s.to_string());
        }
        if let Some(i) = parsed.get("chargeStartThreshold").and_then(|v| v.as_i64()) {
            settings.charge_start_threshold = Some(i as i32);
        }
        if let Some(i) = parsed.get("chargeEndThreshold").and_then(|v| v.as_i64()) {
            settings.charge_end_threshold = Some(i as i32);
        }

        Some(settings)
    }

    /// Serializes settings to canonical JSON string matching C++ format.
    /// Eliminates the legacy duplicate `keyboardBacklightStates` key.
    pub fn settings_to_json(&self, settings: &TccSettings) -> Result<String, ConfigError> {
        let mut map = serde_json::Map::new();

        map.insert("keyboardLayout".to_string(), serde_json::json!(settings.keyboard_layout));

        let lightbar: serde_json::Value = serde_json::from_str(&settings.lightbar_settings)
            .unwrap_or_else(|_| serde_json::json!({}));
        map.insert("lightbar".to_string(), lightbar);

        let power_devices: serde_json::Value = serde_json::from_str(&settings.power_device_settings)
            .unwrap_or_else(|_| serde_json::json!({}));
        map.insert("powerDevices".to_string(), power_devices);

        let kb_states: serde_json::Value = serde_json::from_str(&settings.keyboard_backlight_states)
            .unwrap_or_else(|_| serde_json::json!([]));
        map.insert("keyboardBacklightStates".to_string(), kb_states);

        map.insert("fahrenheit".to_string(), serde_json::json!(settings.fahrenheit));

        let mut state_map_json = serde_json::Map::new();
        for (k, v) in &settings.state_map {
            state_map_json.insert(k.clone(), serde_json::json!(v));
        }
        map.insert("stateMap".to_string(), serde_json::Value::Object(state_map_json));

        let mut profiles_json = serde_json::Map::new();
        for (k, v) in &settings.profiles {
            let val: serde_json::Value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::json!({}));
            profiles_json.insert(k.clone(), val);
        }
        map.insert("profiles".to_string(), serde_json::Value::Object(profiles_json));

        let mut kbp_json = serde_json::Map::new();
        for (k, v) in &settings.custom_keyboard_profiles {
            let val: serde_json::Value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::json!({}));
            kbp_json.insert(k.clone(), val);
        }
        map.insert("customKeyboardProfiles".to_string(), serde_json::Value::Object(kbp_json));

        let mut fanp_json = serde_json::Map::new();
        for (k, v) in &settings.custom_fan_profiles {
            let val: serde_json::Value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::json!({}));
            fanp_json.insert(k.clone(), val);
        }
        map.insert("customFanProfiles".to_string(), serde_json::Value::Object(fanp_json));

        map.insert("shutdownTime".to_string(), serde_json::json!(settings.shutdown_time));
        map.insert("cpuSettingsEnabled".to_string(), serde_json::json!(settings.cpu_settings_enabled));
        map.insert("fanControlEnabled".to_string(), serde_json::json!(settings.fan_control_enabled));
        map.insert("keyboardBacklightControlSupported".to_string(), serde_json::json!(settings.keyboard_backlight_control_supported));

        map.insert("ycbcr420Workaround".to_string(), serde_json::json!(settings.ycbcr420_workaround));
        map.insert("chargingProfile".to_string(), serde_json::json!(settings.charging_profile));
        map.insert("chargingPriority".to_string(), serde_json::json!(settings.charging_priority));

        if let Some(ref ct) = settings.charge_type {
            map.insert("chargeType".to_string(), serde_json::json!(ct));
        }
        if let Some(st) = settings.charge_start_threshold {
            map.insert("chargeStartThreshold".to_string(), serde_json::json!(st));
        }
        if let Some(et) = settings.charge_end_threshold {
            map.insert("chargeEndThreshold".to_string(), serde_json::json!(et));
        }

        let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
        Ok(json)
    }

    /// Atomically writes settings to disk with automatic `.backup` retention.
    pub fn write_settings(&self, settings: &TccSettings) -> Result<(), ConfigError> {
        let json = self.settings_to_json(settings)?;

        if let Some(parent) = self.settings_file.parent() {
            fs::create_dir_all(parent)?;
        }

        // Backup existing file if present
        if self.settings_file.exists() {
            let backup_file = self.settings_file.with_extension("json.backup");
            let _ = fs::copy(&self.settings_file, &backup_file);
        }

        // Write to temporary file in same directory, then atomic rename
        let temp_file = self.settings_file.with_extension("json.tmp");
        fs::write(&temp_file, &json)?;
        fs::rename(&temp_file, &self.settings_file)?;

        Ok(())
    }

    /// Reads settings from disk, falling back to `.backup` if the primary file is corrupted.
    pub fn read_settings(&self) -> Result<TccSettings, ConfigError> {
        if self.settings_file.exists() {
            if let Ok(content) = fs::read_to_string(&self.settings_file) {
                if let Some(settings) = self.parse_settings_json(&content) {
                    return Ok(settings);
                }
            }
        }

        // Fallback to backup
        let backup_file = self.settings_file.with_extension("json.backup");
        if backup_file.exists() {
            if let Ok(content) = fs::read_to_string(&backup_file) {
                if let Some(settings) = self.parse_settings_json(&content) {
                    return Ok(settings);
                }
            }
        }

        Err(ConfigError::ParseError)
    }

    /// Resolves the startup profile given current power state, stateMap, and profiles map.
    ///
    /// Parity with `ProfileManager::resolveStartupProfile`:
    /// - Checks `stateMap` for matching state key (e.g. "power_bat").
    /// - For `TypeC`, if "power_type_c" is missing, falls back to "power_ac".
    /// - Returns parsed `UccProfile` if profile ID is found in profiles; empty profile otherwise.
    pub fn resolve_startup_profile(
        &self,
        state_map: &BTreeMap<String, String>,
        profiles: &BTreeMap<String, String>,
        state: ProfileState,
    ) -> UccProfile {
        let key = state.as_str();
        let mut profile_id = state_map.get(key);

        // Fallback: TypeC falls back to AC if not explicitly mapped
        if profile_id.is_none() && state == ProfileState::TypeC {
            profile_id = state_map.get(ProfileState::Ac.as_str());
        }

        if let Some(id) = profile_id {
            if let Some(json_content) = profiles.get(id) {
                if let Ok(profile) = ucc_core::dto::LegacyProfileDto::parse_json(json_content) {
                    return profile;
                }
            }
        }

        UccProfile::new("", "")
    }
}
