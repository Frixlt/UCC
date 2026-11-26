use ucc_config::{create_default_settings, system_config_path, SettingsManager, TccSettings, UccSettings};

#[test]
fn legacy_config_path_is_preserved() {
    assert_eq!(system_config_path(), std::path::PathBuf::from("/etc/ucc/settings"));
}

#[test]
fn legacy_negative_manual_sentinels_load_through_gui_config_model() {
    let input = r#"{"profiles":{"p":{"id":"p","name":"p","fan":{"manualFanSpeed":-1,"manualPumpVoltage":-1}}}}"#;
    assert!(UccSettings::from_json(input).is_ok(), "Baseline serializer emits -1 manual sentinels; GUI load must accept them");
}

#[test]
fn fresh_install_assignments_match_baseline_policy() {
    let state_map = create_default_settings().state_map;
    // Actual baseline loadSettings(), not the stale TccSettings constructor comment:
    // selects one detected highest-TDP default and uses it for AC/BAT/WC.
    assert_eq!(state_map.get("power_ac"), state_map.get("power_bat"),
        "Baseline fresh install assigns the same detected profile to AC and battery");
    assert_eq!(state_map.get("power_ac"), state_map.get("power_wc"));
}

#[test]
fn legacy_settings_writer_obeys_preview_guard() {
    if std::env::var_os("UCC_TEST_PREVIEW_CHILD").is_none() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "legacy_settings_writer_obeys_preview_guard",
                "--nocapture",
            ])
            .env("UCC_TEST_PREVIEW_CHILD", "1")
            .env("UCC_READ_ONLY_PREVIEW", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        return;
    }
    assert!(ucc_config::is_read_only_preview());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let result = SettingsManager::new(&path).write_settings(&TccSettings::default());
    assert!(
        result.is_err() && !path.exists(),
        "write_settings created a file in preview: result={result:?} exists={}",
        path.exists()
    );
}

#[test]
fn ycbcr_legacy_object_array_is_not_rejected() {
    let input = r#"{"ycbcr420Workaround":[{"eDP-1":true}]}"#;
    assert!(UccSettings::from_json(input).is_ok(), "Legacy workaround entries are objects, not strings");
}

#[cfg(unix)]
#[test]
fn restore_from_backup_preserves_private_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join("settings.json");
    let backup = dir.path().join("settings.json.bak");
    std::fs::write(&primary, b"corrupt").unwrap();
    std::fs::write(&backup, create_default_settings().to_json_pretty().unwrap()).unwrap();
    for p in [&primary, &backup] {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    ucc_config::ConfigManager::restore_from_backup(&primary).unwrap();
    let mode = std::fs::metadata(&primary).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "restored file mode drifted to {mode:04o}");
}
