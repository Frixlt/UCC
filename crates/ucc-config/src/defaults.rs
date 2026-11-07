//! Default profiles and device-specific defaults.
//!
//! Exposes the standard Built-in profiles from the C++ baseline:
//! - "Max Energy Save [Built-in]"
//! - "Quiet [Built-in]"
//! - "Office [Built-in]"
//! - "High Performance [Built-in]"
//!
//! IMPORTANT: Public defaults intentionally exclude personal machine power limits
//! (odmPowerLimits / custom TDP overrides) to maintain portability across varied user hardware.

use ucc_core::{
    ChargingPolicy, FanPoint, ProfileCpu, ProfileDisplay, ProfileFanControl, ProfileKeyboard,
    ProfileWebcam, UccProfile,
};

/// Canonical profile identifier for Max Energy Save mode.
pub const PROFILE_ID_MAX_ENERGY_SAVE: &str = "Max Energy Save [Built-in]";
/// Canonical profile identifier for Quiet mode.
pub const PROFILE_ID_QUIET: &str = "Quiet [Built-in]";
/// Canonical profile identifier for Office / Balanced mode.
pub const PROFILE_ID_OFFICE: &str = "Office [Built-in]";
/// Canonical profile identifier for High Performance mode.
pub const PROFILE_ID_HIGH_PERFORMANCE: &str = "High Performance [Built-in]";

// Convenience alias constants
pub const ALIAS_MAX_ENERGY_SAVE: &str = "max_energy_save";
pub const ALIAS_QUIET: &str = "quiet";
pub const ALIAS_BALANCED: &str = "balanced";
pub const ALIAS_PERFORMANCE: &str = "performance";

/// Constructs the canonical default Max Energy Save profile.
pub fn default_max_energy_save_profile() -> UccProfile {
    let mut profile = UccProfile::new(PROFILE_ID_MAX_ENERGY_SAVE, PROFILE_ID_MAX_ENERGY_SAVE);
    profile.description =
        "Maximum energy savings and minimal acoustic footprint on battery power.".to_string();

    profile.display = ProfileDisplay {
        brightness: 40,
        use_brightness: true,
        refresh_rate: 60,
        use_ref_rate: false,
        x_resolution: 1920,
        y_resolution: 1080,
        use_resolution: false,
    };

    profile.cpu = ProfileCpu {
        governor: "powersave".to_string(),
        energy_performance_preference: "power".to_string(),
        no_turbo: true,
        ..Default::default()
    };

    profile.fan = ProfileFanControl {
        use_control: true,
        fan_profile: "silent".to_string(),
        same_speed: true,
        auto_control_wc: true,
        enable_water_cooler: false,
        table_cpu: vec![
            FanPoint::new(35, 10),
            FanPoint::new(50, 20),
            FanPoint::new(65, 35),
            FanPoint::new(80, 50),
            FanPoint::new(90, 70),
        ],
        table_gpu: vec![
            FanPoint::new(35, 10),
            FanPoint::new(50, 20),
            FanPoint::new(65, 35),
            FanPoint::new(80, 50),
            FanPoint::new(90, 70),
        ],
        ..Default::default()
    };

    profile.odm_profile_name = Some("power_save".to_string());
    // CRITICAL: Public defaults must exclude personal power limits
    profile.odm_power_limits = Vec::new();
    profile.webcam = ProfileWebcam::default();
    profile.keyboard = ProfileKeyboard::default();
    profile.charging = ChargingPolicy::default();

    profile
}

/// Constructs the canonical default Quiet profile.
pub fn default_quiet_profile() -> UccProfile {
    let mut profile = UccProfile::new(PROFILE_ID_QUIET, PROFILE_ID_QUIET);
    profile.description =
        "Quiet daily operation with whisper-quiet fan speeds and battery efficiency.".to_string();

    profile.display = ProfileDisplay {
        brightness: 50,
        use_brightness: true,
        refresh_rate: -1,
        use_ref_rate: false,
        ..Default::default()
    };

    profile.cpu = ProfileCpu {
        governor: "powersave".to_string(),
        energy_performance_preference: "power".to_string(),
        no_turbo: false,
        ..Default::default()
    };

    profile.fan = ProfileFanControl {
        use_control: true,
        fan_profile: "quiet".to_string(),
        same_speed: true,
        auto_control_wc: true,
        enable_water_cooler: false,
        table_cpu: vec![
            FanPoint::new(35, 15),
            FanPoint::new(50, 25),
            FanPoint::new(65, 40),
            FanPoint::new(80, 60),
            FanPoint::new(90, 80),
        ],
        table_gpu: vec![
            FanPoint::new(35, 15),
            FanPoint::new(50, 25),
            FanPoint::new(65, 40),
            FanPoint::new(80, 60),
            FanPoint::new(90, 80),
        ],
        ..Default::default()
    };

    profile.odm_profile_name = Some("power_save".to_string());
    // CRITICAL: Public defaults must exclude personal power limits
    profile.odm_power_limits = Vec::new();
    profile.webcam = ProfileWebcam::default();
    profile.keyboard = ProfileKeyboard::default();
    profile.charging = ChargingPolicy::default();

    profile
}

/// Constructs the canonical default Office / Balanced profile.
pub fn default_office_profile() -> UccProfile {
    let mut profile = UccProfile::new(PROFILE_ID_OFFICE, PROFILE_ID_OFFICE);
    profile.description =
        "Standard everyday performance for office, coding, and multitasking.".to_string();

    profile.display = ProfileDisplay {
        brightness: 60,
        use_brightness: true,
        refresh_rate: -1,
        use_ref_rate: false,
        ..Default::default()
    };

    profile.cpu = ProfileCpu {
        governor: "powersave".to_string(),
        energy_performance_preference: "balance_performance".to_string(),
        no_turbo: false,
        ..Default::default()
    };

    profile.fan = ProfileFanControl {
        use_control: true,
        fan_profile: "balanced".to_string(),
        same_speed: true,
        auto_control_wc: true,
        enable_water_cooler: false,
        table_cpu: vec![
            FanPoint::new(35, 20),
            FanPoint::new(50, 35),
            FanPoint::new(65, 55),
            FanPoint::new(80, 80),
            FanPoint::new(90, 100),
        ],
        table_gpu: vec![
            FanPoint::new(35, 20),
            FanPoint::new(50, 35),
            FanPoint::new(65, 55),
            FanPoint::new(80, 80),
            FanPoint::new(90, 100),
        ],
        ..Default::default()
    };

    profile.odm_profile_name = Some("enthusiast".to_string());
    // CRITICAL: Public defaults must exclude personal power limits
    profile.odm_power_limits = Vec::new();
    profile.webcam = ProfileWebcam::default();
    profile.keyboard = ProfileKeyboard::default();
    profile.charging = ChargingPolicy::default();

    profile
}

/// Constructs the canonical default High Performance profile.
pub fn default_high_performance_profile() -> UccProfile {
    let mut profile = UccProfile::new(PROFILE_ID_HIGH_PERFORMANCE, PROFILE_ID_HIGH_PERFORMANCE);
    profile.description =
        "Maximum sustained hardware performance with aggressive thermal envelope.".to_string();

    profile.display = ProfileDisplay {
        brightness: 60,
        use_brightness: true,
        refresh_rate: -1,
        use_ref_rate: false,
        ..Default::default()
    };

    profile.cpu = ProfileCpu {
        governor: "performance".to_string(),
        energy_performance_preference: "performance".to_string(),
        no_turbo: false,
        ..Default::default()
    };

    profile.fan = ProfileFanControl {
        use_control: true,
        fan_profile: "balanced".to_string(),
        same_speed: true,
        auto_control_wc: false,
        manual_fan_speed: Some(85),
        enable_water_cooler: true,
        table_cpu: vec![
            FanPoint::new(30, 30),
            FanPoint::new(45, 50),
            FanPoint::new(60, 70),
            FanPoint::new(75, 90),
            FanPoint::new(85, 100),
        ],
        table_gpu: vec![
            FanPoint::new(30, 30),
            FanPoint::new(45, 50),
            FanPoint::new(60, 70),
            FanPoint::new(75, 90),
            FanPoint::new(85, 100),
        ],
        ..Default::default()
    };

    profile.odm_profile_name = Some("overboost".to_string());
    // CRITICAL: Public defaults must exclude personal power limits
    profile.odm_power_limits = Vec::new();
    profile.webcam = ProfileWebcam::default();
    profile.keyboard = ProfileKeyboard::default();
    profile.charging = ChargingPolicy::default();

    profile
}

/// Returns the 4 canonical public default profiles:
/// Max Energy Save, Quiet, Office, High Performance.
pub fn default_profiles() -> Vec<UccProfile> {
    vec![
        default_max_energy_save_profile(),
        default_quiet_profile(),
        default_office_profile(),
        default_high_performance_profile(),
    ]
}

/// Returns device-specific default profiles matching a detected hardware model,
/// or falls back to canonical default profiles if device is generic or unlisted.
pub fn device_specific_profiles(device_id: &str) -> Vec<UccProfile> {
    let normalized = device_id.trim().to_uppercase();

    match normalized.as_str() {
        // Thin & light models (no dedicated overboost cooling)
        "IBP14G6_TUX" | "IBP14G6_TRX" | "IBP14G6_TQF" | "IBP14G7_AQF_ARX" | "IBPG8"
        | "PULSE1502" => vec![
            default_max_energy_save_profile(),
            default_quiet_profile(),
            default_office_profile(),
        ],

        // Enthusiast / Desktop replacement models with full profile suite
        "POLARIS1XI02" | "POLARIS1XI03" | "POLARIS1XA02" | "POLARIS1XA03" | "POLARIS1XA05"
        | "STELLARIS1XI03" | "STELLARIS1XI04" | "STELLARIS1XI05" | "STELLARIS16I06"
        | "STELLARIS17I06" | "STELLARIS16I07" | "STELLARIS16A07" | "XNE16E25" | "XNE16A25"
        | "PCS_X6FR559Y" => vec![
            default_max_energy_save_profile(),
            default_quiet_profile(),
            default_office_profile(),
            default_high_performance_profile(),
        ],

        // Default fallback: all 4 canonical profiles
        _ => default_profiles(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_profiles_inventory_and_ids() {
        let profiles = default_profiles();
        assert_eq!(profiles.len(), 4);

        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                PROFILE_ID_MAX_ENERGY_SAVE,
                PROFILE_ID_QUIET,
                PROFILE_ID_OFFICE,
                PROFILE_ID_HIGH_PERFORMANCE
            ]
        );
    }

    #[test]
    fn test_public_defaults_exclude_personal_power_limits() {
        for profile in default_profiles() {
            // Must strictly have empty odm_power_limits in public defaults
            assert!(
                profile.odm_power_limits.is_empty(),
                "Profile {} must not contain personal power limits in public defaults",
                profile.name
            );
            // Must also pass domain validation
            assert!(
                profile.validate().is_ok(),
                "Default profile {} must satisfy all domain validation invariants",
                profile.name
            );
        }
    }

    #[test]
    fn test_device_specific_profiles_mapping() {
        // Thin & light gets 3 profiles (MaxEnergySave, Quiet, Office)
        let ibp = device_specific_profiles("IBP14G6_TUX");
        assert_eq!(ibp.len(), 3);
        assert_eq!(ibp[0].id, PROFILE_ID_MAX_ENERGY_SAVE);
        assert_eq!(ibp[1].id, PROFILE_ID_QUIET);
        assert_eq!(ibp[2].id, PROFILE_ID_OFFICE);

        // Enthusiast model gets 4 profiles
        let stellaris = device_specific_profiles("STELLARIS16I06");
        assert_eq!(stellaris.len(), 4);

        // Unknown device falls back to standard 4 profiles
        let unknown = device_specific_profiles("UnknownVendor_Laptop123");
        assert_eq!(unknown.len(), 4);
    }
}
