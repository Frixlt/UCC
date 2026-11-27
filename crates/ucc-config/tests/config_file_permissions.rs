use std::fs;
use ucc_config::ConfigManager;

#[test]
fn save_does_not_rotate_schema_invalid_primary_onto_good_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    fs::write(
        &path,
        r#"{"profiles":{"p":{"id":"p","name":"P","fan":{"tableCPU":[{"temp":60,"speed":200}]}}}}"#,
    )
    .unwrap();
    let backup = dir.path().join("settings.backup");
    fs::write(&backup, r#"{"fahrenheit":true}"#).unwrap();
    let manager = ConfigManager::with_custom_path(&path);
    let recovered = manager.load().unwrap();
    assert_eq!(recovered.fahrenheit, Some(true));
    manager.save(&recovered).unwrap();
    assert!(
        ConfigManager::backup_paths(&path)
            .iter()
            .any(|p| ConfigManager::load_from_path(p).is_ok()),
        "all previous valid backups were replaced"
    );
}

#[test]
fn cpu_numbers_larger_than_u32_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    fs::write(
        &path,
        r#"{"profiles":{"p":{"id":"p","name":"P","cpu":{"onlineCores":4294967297}}}}"#,
    )
    .unwrap();
    let parsed = ConfigManager::load_from_path(&path);
    assert!(
        parsed.is_err(),
        "oversized onlineCores silently became {:?}",
        parsed.unwrap().profiles["p"].cpu.online_cores
    );
}

#[test]
fn save_rejects_settings_that_its_loader_would_reject() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let settings = ucc_config::UccSettings::from_json(
        r#"{"profiles":{"p":{"id":"p","name":"P","fan":{"tableCPU":[{"temp":60,"speed":200}]}}}}"#,
    )
    .unwrap();
    let result = ConfigManager::with_custom_path(&path).save(&settings);
    assert!(
        result.is_err() && !path.exists(),
        "invalid configuration was saved: {result:?}"
    );
}
