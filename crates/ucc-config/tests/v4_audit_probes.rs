use std::fs;
use ucc_config::{ConfigManager, UccSettings};

#[test]
fn reads_cpp_profile_with_default_cpu_fields() {
    let doc = r#"{"keyboardBacklightStates":[],"profiles":{"p":{"id":"p","name":"Profile","cpu":{"onlineCores":-1,"scalingMinFrequency":-1,"scalingMaxFrequency":-1},"fan":{"manualFanSpeed":-1,"manualPumpVoltage":-1}}},"keyboardBacklightStates":[]}"#;
    let loaded = UccSettings::from_json(doc);
    assert!(loaded.is_ok(), "{loaded:?}");
}

#[test]
fn restore_does_not_write_in_preview() {
    if std::env::var_os("UCC_TEST_V4_PREVIEW_CHILD").is_none() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "restore_does_not_write_in_preview",
                "--nocapture",
            ])
            .env("UCC_TEST_V4_PREVIEW_CHILD", "1")
            .env("UCC_READ_ONLY_PREVIEW", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        return;
    }
    assert!(ucc_config::is_read_only_preview());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    fs::write(&path, "{broken").unwrap();
    fs::write(dir.path().join("settings.backup"), r#"{"fahrenheit":true}"#).unwrap();
    let result = ConfigManager::restore_from_backup(&path);
    assert!(result.is_err(), "restore succeeded in preview");
    assert_eq!(fs::read_to_string(path).unwrap(), "{broken");
}

#[test]
fn save_after_recovery_keeps_a_valid_previous_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let backup = dir.path().join("settings.backup");
    fs::write(&path, "{broken").unwrap();
    fs::write(&backup, r#"{"fahrenheit":true}"#).unwrap();
    let mgr = ConfigManager::with_custom_path(&path);
    let loaded = mgr.load().unwrap();
    assert_eq!(loaded.fahrenheit, Some(true));
    mgr.save(&loaded).unwrap();
    let has_valid_backup = [backup, dir.path().join("settings.bak")]
        .iter()
        .any(|candidate| ConfigManager::load_from_path(candidate).is_ok());
    assert!(
        has_valid_backup,
        "save rotated corrupt primary onto both backup paths"
    );
}
