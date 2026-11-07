//! System settings document representation and serialization.
//!
//! Models `settings.json`, mapping system power modes (AC, Type-C, Battery, Water Cooler)
//! to active profiles, user backlight preferences, and custom profile catalogs.

use crate::defaults::{
    default_high_performance_profile, default_max_energy_save_profile, default_office_profile,
    default_quiet_profile, ALIAS_BALANCED, ALIAS_MAX_ENERGY_SAVE, ALIAS_PERFORMANCE, ALIAS_QUIET,
    PROFILE_ID_HIGH_PERFORMANCE, PROFILE_ID_MAX_ENERGY_SAVE, PROFILE_ID_OFFICE, PROFILE_ID_QUIET,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ucc_core::{KeyboardBacklightState, UccProfile};

/// Custom saved keyboard profile entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomKeyboardProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub json: serde_json::Value,
}

/// Complete Frixit Control Center settings structure (stored in `settings.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UccSettings {
    /// Active keyboard layout identifier (e.g. "frixit-mechrevo", "iso", "ansi").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_layout: Option<String>,

    /// Backlight RGB states for physical keyboard zones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_backlight_states: Option<Vec<KeyboardBacklightState>>,

    /// User custom saved keyboard backlight presets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_keyboard_profiles: Option<HashMap<String, CustomKeyboardProfile>>,

    /// Power devices management section (ASPM, Wi-Fi powersave, CPU mode overlay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_devices: Option<serde_json::Value>,

    /// Front lightbar management settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightbar: Option<serde_json::Value>,

    /// Mapping of system power supply state to active profile ID.
    /// Standard keys: "power_ac", "power_type_c", "power_bat", "power_wc".
    #[serde(default)]
    pub state_map: HashMap<String, String>,

    /// Configured system profiles mapped by profile ID.
    #[serde(default)]
    pub profiles: HashMap<String, UccProfile>,

    /// Global battery charging policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_profile: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging_priority: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_type: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_start_threshold: Option<u8>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_end_threshold: Option<u8>,

    /// Scheduled daily shutdown time (e.g. "23:00").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_time: Option<String>,

    /// Temperature unit display toggle: true for Fahrenheit, false for Celsius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fahrenheit: Option<bool>,

    /// Flag enabling or disabling direct CPU policy management.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_settings_enabled: Option<bool>,

    /// Flag enabling or disabling fan curve daemon management.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_control_enabled: Option<bool>,

    /// Hardware capability indicator for keyboard backlight control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_backlight_control_supported: Option<bool>,

    /// List of display identifiers requiring YCbCr / YUV420 color format workarounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ycbcr420_workaround: Option<Vec<String>>,

    /// Preserves unknown/vendor-specific legacy attributes without loss.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for UccSettings {
    fn default() -> Self {
        create_default_settings()
    }
}

impl UccSettings {
    /// Deserializes settings from a JSON string.
    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    /// Serializes settings into a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Serializes settings into a compact JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Resolves a profile by ID or human-readable name, supporting canonical
    /// IDs and common aliases.
    pub fn get_profile(&self, id_or_name: &str) -> Option<&UccProfile> {
        // Direct key lookup
        if let Some(p) = self.profiles.get(id_or_name) {
            return Some(p);
        }

        let lower = id_or_name.trim().to_lowercase();

        // Exact ID or Name match (case-insensitive)
        if let Some(p) = self
            .profiles
            .values()
            .find(|p| p.id.to_lowercase() == lower || p.name.to_lowercase() == lower)
        {
            return Some(p);
        }

        // Canonical alias matching
        if lower == ALIAS_QUIET || lower.contains("quiet") {
            return self.profiles.get(PROFILE_ID_QUIET);
        }
        if lower == ALIAS_BALANCED || lower.contains("balanced") || lower == "office" {
            return self.profiles.get(PROFILE_ID_OFFICE);
        }
        if lower == ALIAS_PERFORMANCE
            || lower.contains("performance")
            || lower.contains("enthusiast")
        {
            return self.profiles.get(PROFILE_ID_HIGH_PERFORMANCE);
        }
        if lower == ALIAS_MAX_ENERGY_SAVE || lower.contains("energy") {
            return self.profiles.get(PROFILE_ID_MAX_ENERGY_SAVE);
        }

        None
    }

    /// Returns the active profile assigned to a given system power state ("power_ac", "power_bat", "power_type_c", "power_wc").
    /// In accordance with F06, if Type-C has no explicit profile assigned, it falls back to AC.
    pub fn get_active_profile_for_mode(&self, mode: &str) -> Option<&UccProfile> {
        if let Some(profile_id) = self.state_map.get(mode) {
            if let Some(profile) = self.get_profile(profile_id) {
                return Some(profile);
            }
        }

        // F06 fallback: Type-C falls back to AC
        if mode == "power_type_c" {
            if let Some(ac_id) = self.state_map.get("power_ac") {
                if let Some(profile) = self.get_profile(ac_id) {
                    return Some(profile);
                }
            }
        }

        // Fallback to Office / Balanced profile
        self.get_profile(PROFILE_ID_OFFICE)
    }

    /// Sets the assigned profile ID for a power supply state.
    pub fn set_active_profile_for_mode(&mut self, mode: &str, profile_id: impl Into<String>) {
        self.state_map.insert(mode.to_string(), profile_id.into());
    }
}

/// Helper structure for standalone profile catalogs (e.g. `profiles.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesCatalog {
    pub profiles: Vec<UccProfile>,
}

impl ProfilesCatalog {
    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }
}

/// Generates pristine default settings populated with the 4 canonical C++ profiles.
pub fn create_default_settings() -> UccSettings {
    let mut profiles = HashMap::new();
    profiles.insert(
        PROFILE_ID_MAX_ENERGY_SAVE.to_string(),
        default_max_energy_save_profile(),
    );
    profiles.insert(PROFILE_ID_QUIET.to_string(), default_quiet_profile());
    profiles.insert(PROFILE_ID_OFFICE.to_string(), default_office_profile());
    profiles.insert(
        PROFILE_ID_HIGH_PERFORMANCE.to_string(),
        default_high_performance_profile(),
    );

    let mut state_map = HashMap::new();
    state_map.insert(
        "power_ac".to_string(),
        PROFILE_ID_HIGH_PERFORMANCE.to_string(),
    );
    state_map.insert("power_type_c".to_string(), PROFILE_ID_OFFICE.to_string());
    state_map.insert("power_bat".to_string(), PROFILE_ID_QUIET.to_string());
    state_map.insert(
        "power_wc".to_string(),
        PROFILE_ID_HIGH_PERFORMANCE.to_string(),
    );

    UccSettings {
        keyboard_layout: Some("ansi".to_string()),
        keyboard_backlight_states: Some(vec![
            KeyboardBacklightState {
                red: 0,
                green: 120,
                blue: 215,
                brightness: 50,
            },
            KeyboardBacklightState {
                red: 0,
                green: 120,
                blue: 215,
                brightness: 50,
            },
            KeyboardBacklightState {
                red: 0,
                green: 120,
                blue: 215,
                brightness: 50,
            },
        ]),
        custom_keyboard_profiles: Some(HashMap::new()),
        power_devices: None,
        lightbar: None,
        state_map,
        profiles,
        charging_profile: Some("balanced".to_string()),
        charging_priority: Some("performance".to_string()),
        charge_type: Some("Standard".to_string()),
        charge_start_threshold: Some(40),
        charge_end_threshold: Some(80),
        shutdown_time: None,
        fahrenheit: Some(false),
        cpu_settings_enabled: Some(true),
        fan_control_enabled: Some(true),
        keyboard_backlight_control_supported: Some(true),
        ycbcr420_workaround: Some(Vec::new()),
        extra: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings_round_trip() {
        let original = create_default_settings();
        let json = original.to_json_pretty().expect("serialization failed");
        let restored = UccSettings::from_json(&json).expect("deserialization failed");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_wire_casing_acronyms_exact_match() {
        let settings = create_default_settings();
        let json = settings.to_json().expect("serialize");

        // FINDING-V2-03: tableCPU, tableGPU, autoControlWC must serialize exactly
        assert!(
            json.contains("\"tableCPU\""),
            "Serialization must preserve tableCPU casing"
        );
        assert!(
            json.contains("\"tableGPU\""),
            "Serialization must preserve tableGPU casing"
        );
        assert!(
            json.contains("\"autoControlWC\""),
            "Serialization must preserve autoControlWC casing"
        );
        assert!(
            !json.contains("\"table_cpu\""),
            "Must not serialize snake_case table_cpu"
        );
        assert!(
            !json.contains("\"auto_control_wc\""),
            "Must not serialize snake_case auto_control_wc"
        );
    }

    #[test]
    fn test_profile_lookup_canonical_and_aliases() {
        let settings = create_default_settings();

        assert_eq!(
            settings.get_profile("quiet").map(|p| p.id.as_str()),
            Some(PROFILE_ID_QUIET)
        );
        assert_eq!(
            settings
                .get_profile("Quiet [Built-in]")
                .map(|p| p.id.as_str()),
            Some(PROFILE_ID_QUIET)
        );

        assert_eq!(
            settings.get_profile("balanced").map(|p| p.id.as_str()),
            Some(PROFILE_ID_OFFICE)
        );
        assert_eq!(
            settings
                .get_profile("Office [Built-in]")
                .map(|p| p.id.as_str()),
            Some(PROFILE_ID_OFFICE)
        );

        assert_eq!(
            settings.get_profile("performance").map(|p| p.id.as_str()),
            Some(PROFILE_ID_HIGH_PERFORMANCE)
        );
        assert_eq!(
            settings
                .get_profile("High Performance [Built-in]")
                .map(|p| p.id.as_str()),
            Some(PROFILE_ID_HIGH_PERFORMANCE)
        );
    }

    #[test]
    fn test_type_c_falls_back_to_ac_if_unassigned() {
        let mut settings = create_default_settings();
        settings.state_map.remove("power_type_c");
        settings.state_map.insert(
            "power_ac".to_string(),
            PROFILE_ID_HIGH_PERFORMANCE.to_string(),
        );

        let active = settings.get_active_profile_for_mode("power_type_c");
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, PROFILE_ID_HIGH_PERFORMANCE);
    }

    #[test]
    fn test_serialization_diff_01_no_duplicate_keys() {
        let settings = create_default_settings();
        let json = settings.to_json().expect("serialize");

        let count = json.matches("\"keyboardBacklightStates\"").count();
        assert_eq!(count, 1, "Must never emit duplicate keys (DIFF-01)");
    }
}
