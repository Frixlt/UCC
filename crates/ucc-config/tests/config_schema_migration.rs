use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use ucc_config::ConfigManager;
#[cfg(unix)]
use ucc_config::UccSettings;

#[cfg(unix)]
#[test]
fn save_never_follows_a_backup_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let outside = dir.path().join("external");
    fs::write(&path, r#"{"fahrenheit":true}"#).unwrap();
    fs::write(&outside, "external").unwrap();
    symlink(&outside, dir.path().join("settings.bak")).unwrap();
    let settings = UccSettings::from_json(r#"{"fahrenheit":false}"#).unwrap();
    let result = ConfigManager::with_custom_path(&path).save(&settings);
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "external",
        "{result:?}"
    );
}

#[cfg(unix)]
#[test]
fn save_does_not_broaden_an_existing_private_files_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let settings = UccSettings::from_json("{}").unwrap();
    ConfigManager::with_custom_path(&path)
        .save(&settings)
        .unwrap();
    let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode & 0o077, 0, "private file changed to {mode:o}");
}

#[test]
fn loader_rejects_inverted_global_charge_thresholds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    fs::write(
        &path,
        r#"{"chargeType":"Custom","chargeStartThreshold":95,"chargeEndThreshold":60}"#,
    )
    .unwrap();
    assert!(ConfigManager::load_from_path(&path).is_err());
}
