use std::fs;
use ucc_config::{ConfigManager, SettingsManager, UccSettings};

#[test]
fn accepts_keyboard_states_duplicate_from_cpp_writer() {
    let parsed = UccSettings::from_json(r#"{"keyboardBacklightStates":[],"keyboardBacklightStates":[]}"#);
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
    let data = serde_json::json!({"ycbcr420Workaround":[{"eDP-1":true,"HDMI-A-1":false},{"DP-1":true}]});
    fs::write(&path, data.to_string()).unwrap();
    let manager = SettingsManager::new(&path);
    let settings = manager.read_settings().unwrap();
    manager.write_settings(&settings).unwrap();
    let stored: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(stored["ycbcr420Workaround"], data["ycbcr420Workaround"]);
}
