use std::fs;
use ucc_config::{ConfigManager, SettingsManager, UccSettings};

#[test]
fn accepts_keyboard_states_duplicate_from_cpp_writer() {
    let parsed =
        UccSettings::from_json(r#"{"keyboardBacklightStates":[],"keyboardBacklightStates":[]}"#);
    assert!(parsed.is_ok(), "{parsed:?}");
}

#[test]
fn gui_loader_recovers_from_cpp_backup() {
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join("settings");
    fs::write(&primary, "{truncated").unwrap();
    fs::write(dir.path().join("settings.backup"), r#"{"fahrenheit":true}"#).unwrap();
    let recovered = ConfigManager::with_custom_path(&primary).load().unwrap();
    assert_eq!(recovered.fahrenheit, Some(true));
}

#[test]
fn legacy_manager_preserves_ycbcr_port_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let data =
        serde_json::json!({"ycbcr420Workaround":[{"eDP-1":true,"HDMI-A-1":false},{"DP-1":true}]});
    fs::write(&path, data.to_string()).unwrap();
    let manager = SettingsManager::new(&path);
    let settings = manager.read_settings().unwrap();
    manager.write_settings(&settings).unwrap();
    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(stored["ycbcr420Workaround"], data["ycbcr420Workaround"]);
}

#[test]
fn config_manager_roundtrips_126_zone_and_host_settings() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("settings");
    let host_settings = std::path::Path::new("/etc/ucc/settings");
    if host_settings.exists() {
        let (mut loaded, _) = ConfigManager::load_with_recovery(host_settings).unwrap();
        loaded.keyboard_backlight_states = Some(vec![
            ucc_core::KeyboardBacklightState {
                brightness: 31,
                red: 180,
                green: 0,
                blue: 220,
            };
            126
        ]);
        ConfigManager::with_custom_path(&target)
            .save(&loaded)
            .unwrap();
        let (reloaded, recovered) = ConfigManager::load_with_recovery(&target).unwrap();
        assert!(!recovered);
        assert_eq!(
            reloaded.keyboard_backlight_states.as_ref().map(Vec::len),
            Some(126)
        );
    }
}

#[test]
fn config_manager_roundtrips_126_zone_fixture_in_tempdir() {
    use ucc_config::{create_default_settings, CustomKeyboardProfile};

    let dir = tempfile::tempdir().expect("create tempdir");
    let target = dir.path().join("settings.json");

    let zones: Vec<ucc_core::KeyboardBacklightState> = (0..126_u16)
        .map(|i| ucc_core::KeyboardBacklightState {
            brightness: (i % 50) as u8,
            red: ((i * 2) % 256) as u8,
            green: (255 - ((i * 2) % 256)) as u8,
            blue: ((i * 3) % 256) as u8,
        })
        .collect();

    let mut settings = create_default_settings();
    settings.keyboard_layout = Some("frixit-mechrevo".to_string());
    settings.keyboard_backlight_states = Some(zones.clone());
    settings
        .custom_keyboard_profiles
        .get_or_insert_default()
        .insert(
            "fixture-126".to_string(),
            CustomKeyboardProfile {
                id: "fixture-126".to_string(),
                name: "126-Zone Deterministic Fixture".to_string(),
                json: serde_json::json!({
                    "brightness": 31,
                    "states": zones,
                }),
            },
        );

    let mgr = ConfigManager::with_custom_path(&target);
    mgr.save(&settings).expect("save 126-zone fixture");

    let (reloaded, recovered) =
        ConfigManager::load_with_recovery(&target).expect("reload 126-zone fixture");
    assert!(!recovered);
    assert_eq!(reloaded, settings);

    let reloaded_zones = reloaded
        .keyboard_backlight_states
        .as_ref()
        .expect("keyboardBacklightStates present");
    assert_eq!(reloaded_zones.len(), 126);
    assert_eq!(reloaded_zones[0].brightness, 0);
    assert_eq!(reloaded_zones[0].red, 0);
    assert_eq!(reloaded_zones[0].green, 255);
    assert_eq!(reloaded_zones[63].red, 126);
    assert_eq!(reloaded_zones[125].blue, 119);

    let custom_kb = reloaded
        .custom_keyboard_profiles
        .as_ref()
        .expect("customKeyboardProfiles present");
    assert_eq!(
        custom_kb["fixture-126"].json["states"]
            .as_array()
            .map(Vec::len),
        Some(126)
    );

    // Verify serialized file has no duplicate keyboardBacklightStates keys
    let raw = fs::read_to_string(&target).expect("read saved settings");
    assert_eq!(raw.matches("\"keyboardBacklightStates\"").count(), 1);
}
