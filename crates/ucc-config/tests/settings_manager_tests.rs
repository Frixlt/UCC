use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;
use ucc_config::*;
use ucc_core::ProfileState;

fn sample_settings_json() -> &'static str {
    r#"{
      "fahrenheit": true,
      "stateMap": {
        "power_ac":  "profile-perf",
        "power_bat": "profile-quiet",
        "power_wc":  "profile-cool"
      },
      "profiles": {
        "profile-perf":  {"id":"profile-perf","name":"Performance"},
        "profile-quiet": {"id":"profile-quiet","name":"Quiet"}
      },
      "shutdownTime": "22:30",
      "cpuSettingsEnabled": false,
      "fanControlEnabled": true,
      "keyboardBacklightControlSupported": false,
      "ycbcr420Workaround": [],
      "chargingProfile": "balanced",
      "chargingPriority": "performance",
      "chargeType": "Custom",
      "chargeStartThreshold": 40,
      "chargeEndThreshold": 80
    }"#
}

#[test]
fn test_parse_settings_happy_path() {
    let mgr = SettingsManager::default();
    let parsed = mgr.parse_settings_json(sample_settings_json()).expect("should parse settings");

    assert!(parsed.fahrenheit);
    assert!(!parsed.cpu_settings_enabled);
    assert!(parsed.fan_control_enabled);
    assert!(!parsed.keyboard_backlight_control_supported);

    assert_eq!(parsed.state_map.get("power_ac").map(|s| s.as_str()), Some("profile-perf"));
    assert_eq!(parsed.state_map.get("power_bat").map(|s| s.as_str()), Some("profile-quiet"));
    assert_eq!(parsed.state_map.get("power_wc").map(|s| s.as_str()), Some("profile-cool"));

    assert_eq!(parsed.profiles.len(), 2);
    assert!(parsed.profiles.contains_key("profile-perf"));
    assert!(parsed.profiles.contains_key("profile-quiet"));

    assert_eq!(parsed.shutdown_time, Some("22:30".to_string()));
    assert_eq!(parsed.charging_profile, Some("balanced".to_string()));
    assert_eq!(parsed.charging_priority, Some("performance".to_string()));
    assert_eq!(parsed.charge_type, Some("Custom".to_string()));
    assert_eq!(parsed.charge_start_threshold, Some(40));
    assert_eq!(parsed.charge_end_threshold, Some(80));
}

#[test]
fn test_parse_malformed_and_empty_json() {
    let mgr = SettingsManager::default();
    assert!(mgr.parse_settings_json("").is_none());
    assert!(mgr.parse_settings_json("   ").is_none());
    assert!(mgr.parse_settings_json("{{invalid json").is_none());
    assert!(mgr.parse_settings_json("12345").is_none());
    assert!(mgr.parse_settings_json("null").is_none());
}

#[test]
fn test_parse_minimal_json_uses_defaults() {
    let mgr = SettingsManager::default();
    let parsed = mgr.parse_settings_json("{}").expect("minimal json should parse");

    assert!(!parsed.fahrenheit);
    assert!(parsed.cpu_settings_enabled);
    assert!(parsed.fan_control_enabled);
    assert!(parsed.keyboard_backlight_control_supported);
    assert_eq!(parsed.keyboard_layout, "auto");
    assert!(parsed.shutdown_time.is_none());
    assert!(parsed.charging_profile.is_none());
    assert!(parsed.state_map.is_empty());
    assert!(parsed.profiles.is_empty());
}

#[test]
fn test_settings_to_json_roundtrip_and_no_duplicate_keys() {
    let mgr = SettingsManager::default();
    let initial = mgr.parse_settings_json(sample_settings_json()).unwrap();

    let serialized = mgr.settings_to_json(&initial).expect("serialization must succeed");

    // Intentional fix: eliminate duplicate keyboardBacklightStates key
    let count = serialized.matches("\"keyboardBacklightStates\"").count();
    assert_eq!(count, 1, "Must contain exactly ONE keyboardBacklightStates key (legacy duplicate bug fixed)");

    let re_parsed = mgr.parse_settings_json(&serialized).expect("re-parse must succeed");
    assert_eq!(initial.fahrenheit, re_parsed.fahrenheit);
    assert_eq!(initial.state_map, re_parsed.state_map);
    assert_eq!(initial.shutdown_time, re_parsed.shutdown_time);
    assert_eq!(initial.charging_profile, re_parsed.charging_profile);
}

#[test]
fn test_resolve_startup_profile() {
    let mgr = SettingsManager::default();
    let mut state_map = BTreeMap::new();
    state_map.insert("power_ac".to_string(), "profile-ac".to_string());
    state_map.insert("power_bat".to_string(), "profile-bat".to_string());

    let mut profiles = BTreeMap::new();
    profiles.insert("profile-ac".to_string(), r#"{"id":"profile-ac","name":"AC Performance"}"#.to_string());
    profiles.insert("profile-bat".to_string(), r#"{"id":"profile-bat","name":"Battery Saver"}"#.to_string());

    // Battery resolves to profile-bat
    let bat = mgr.resolve_startup_profile(&state_map, &profiles, ProfileState::Bat);
    assert_eq!(bat.id, "profile-bat");
    assert_eq!(bat.name, "Battery Saver");

    // AC resolves to profile-ac
    let ac = mgr.resolve_startup_profile(&state_map, &profiles, ProfileState::Ac);
    assert_eq!(ac.id, "profile-ac");
    assert_eq!(ac.name, "AC Performance");

    // TypeC not explicitly mapped -> falls back to AC profile
    let type_c = mgr.resolve_startup_profile(&state_map, &profiles, ProfileState::TypeC);
    assert_eq!(type_c.id, "profile-ac");

    // WC not mapped -> empty profile
    let wc = mgr.resolve_startup_profile(&state_map, &profiles, ProfileState::Wc);
    assert_eq!(wc.id, "");
}

#[test]
fn test_atomic_write_backup_and_recovery() {
    let dir = tempdir().expect("create tempdir");
    let settings_path = dir.path().join("settings.json");
    let mgr = SettingsManager::new(&settings_path);

    let mut settings = TccSettings {
        keyboard_layout: "frixit-layout".to_string(),
        ..Default::default()
    };
    settings.state_map.insert("power_ac".to_string(), "perf".to_string());

    // First write creates the file
    mgr.write_settings(&settings).expect("first write");
    assert!(settings_path.exists());

    // Read back
    let read1 = mgr.read_settings().expect("read first");
    assert_eq!(read1.keyboard_layout, "frixit-layout");

    // Second write creates .backup of the first version
    settings.keyboard_layout = "updated-layout".to_string();
    mgr.write_settings(&settings).expect("second write");

    let backup_path = dir.path().join("settings.json.backup");
    assert!(backup_path.exists(), "Backup file must be created on overwrite");

    // Corrupt main file
    fs::write(&settings_path, "CORRUPTED CONTENT NOT JSON").expect("corrupt main");

    // Read settings must recover from .backup
    let recovered = mgr.read_settings().expect("must recover from backup");
    assert_eq!(recovered.keyboard_layout, "frixit-layout", "Must recover data from previous version");
}
