// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2 config fixes:
//! - V2-P2-11 (N18): Top-level `UccSettings` deserializes `-1` charge thresholds as `None`.
//! - V2-P2-12: `ConfigManager::restore_from_backup` rejects symlink targets, uses `libc::O_NOFOLLOW`,
//!   and sets permissions via the opened `File` descriptor.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use ucc_config::{create_default_settings, ConfigManager, UccSettings};

#[test]
fn v2_p2_11_top_level_charge_thresholds_accept_minus_one_sentinel() {
    let settings = UccSettings::from_json(
        r#"{
            "chargeStartThreshold": -1,
            "chargeEndThreshold": -1
        }"#,
    )
    .expect("V2-P2-11: top-level -1 charge thresholds must deserialize cleanly");
    assert_eq!(settings.charge_start_threshold, None);
    assert_eq!(settings.charge_end_threshold, None);

    let valid = UccSettings::from_json(
        r#"{
            "chargeStartThreshold": 20,
            "chargeEndThreshold": 80
        }"#,
    )
    .expect("valid top-level charge thresholds must deserialize");
    assert_eq!(valid.charge_start_threshold, Some(20));
    assert_eq!(valid.charge_end_threshold, Some(80));

    let invalid = UccSettings::from_json(r#"{ "chargeStartThreshold": 120 }"#);
    assert!(
        invalid.is_err(),
        "out-of-range top-level charge threshold must be rejected"
    );
}

#[test]
fn v2_p2_12_restore_from_backup_rejects_symlink_target_and_uses_fd_permissions() {
    let tmp = TempDir::new().expect("temp dir");
    let cfg_dir = tmp.path().join("cfg");
    let outside_dir = tmp.path().join("outside");
    fs::create_dir_all(&cfg_dir).expect("create cfg dir");
    fs::create_dir_all(&outside_dir).expect("create outside dir");

    let defaults = create_default_settings();
    let valid_json = defaults.to_json_pretty().expect("serialize defaults");

    // 1. Symlink target must be rejected without touching the symlink destination.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        use ucc_config::ConfigError;
        let victim = outside_dir.join("victim.json");
        fs::write(&victim, "original-victim").expect("write victim");

        let target_link = cfg_dir.join("settings.json");
        let bak_path = cfg_dir.join("settings.json.bak");
        fs::write(&bak_path, &valid_json).expect("write backup");
        symlink(&victim, &target_link).expect("create symlink");

        let res = ConfigManager::restore_from_backup(&target_link);
        assert!(
            matches!(res, Err(ConfigError::PermissionDenied(_))),
            "V2-P2-12: restore_from_backup must reject symlink target, got {res:?}"
        );
        assert_eq!(
            fs::read_to_string(&victim).expect("read victim"),
            "original-victim"
        );
    }

    // 2. Normal regular file target restores cleanly.
    let regular_target = cfg_dir.join("regular_settings.json");
    let regular_bak = cfg_dir.join("regular_settings.json.bak");
    fs::write(&regular_target, "{corrupt json").expect("write corrupt target");
    fs::write(&regular_bak, &valid_json).expect("write regular backup");
    let restored =
        ConfigManager::restore_from_backup(&regular_target).expect("restore regular target");
    assert!(!restored.profiles.is_empty());

    // 3. Source audit: O_NOFOLLOW and fd-based set_permissions in restore_from_backup.
    let manager_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/manager.rs"))
            .expect("read src/manager.rs");
    let fn_start = manager_src
        .find("pub fn restore_from_backup")
        .expect("restore_from_backup must exist");
    let fn_end = manager_src[fn_start..]
        .find("pub fn save(&self")
        .map(|rel| fn_start + rel)
        .unwrap_or(manager_src.len());
    let fn_body = &manager_src[fn_start..fn_end];
    assert!(
        fn_body.contains("libc::O_NOFOLLOW"),
        "V2-P2-12: restore_from_backup must set libc::O_NOFOLLOW"
    );
    assert!(
        fn_body.contains("temp_file.set_permissions("),
        "V2-P2-12: restore_from_backup must call set_permissions on opened File handle"
    );
    assert!(
        !fn_body.contains("fs::set_permissions(&temp_path"),
        "V2-P2-12: restore_from_backup must not use path-based fs::set_permissions(&temp_path, ...)"
    );
}
