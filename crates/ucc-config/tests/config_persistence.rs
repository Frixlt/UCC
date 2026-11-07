//! Integration tests for UCC configuration persistence, atomic updates, and preview protection.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use ucc_config::{
    create_default_settings, default_profiles, is_read_only_preview, ConfigError, ConfigManager,
    ProfilesCatalog, UccSettings, ENV_READ_ONLY_PREVIEW, PROFILE_ID_HIGH_PERFORMANCE,
    PROFILE_ID_MAX_ENERGY_SAVE, PROFILE_ID_OFFICE, PROFILE_ID_QUIET,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn get_fixture_path(name: &str) -> PathBuf {
    for prefix in &["../../artifacts/rust-migration/fixtures", "../../../../artifacts/rust-migration/fixtures"] {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(prefix).join(name);
        if p.exists() {
            return p;
        }
    }
    Path::new("/home/frixit/Рабочий стол/fork control center/UCC-Revorked/artifacts/rust-migration/fixtures").join(name)
}

fn create_unique_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ucc_integ_{}_{}_{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

#[test]
fn test_config_manager_load_and_atomic_save_roundtrip() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp_dir = create_unique_temp_dir("roundtrip");
    let config_file = temp_dir.join("settings.json");

    let mgr = ConfigManager::with_custom_path(&config_file);

    // Initial load when file does not exist yields default settings
    let mut settings = mgr.load().expect("load default settings");
    assert_eq!(settings.profiles.len(), 4);
    assert!(settings.profiles.contains_key(PROFILE_ID_MAX_ENERGY_SAVE));
    assert!(settings.profiles.contains_key(PROFILE_ID_QUIET));
    assert!(settings.profiles.contains_key(PROFILE_ID_OFFICE));
    assert!(settings.profiles.contains_key(PROFILE_ID_HIGH_PERFORMANCE));

    // Modify a setting and persist atomically
    settings.charging_profile = Some("stationary".to_string());
    settings.charge_end_threshold = Some(70);
    let saved_path = mgr.save(&settings).expect("atomic save");
    assert_eq!(saved_path, config_file);
    assert!(config_file.exists());

    // Read back through fresh manager instance
    let reloaded = ConfigManager::load_from_path(&config_file).expect("reload settings");
    assert_eq!(reloaded.charging_profile.as_deref(), Some("stationary"));
    assert_eq!(reloaded.charge_end_threshold, Some(70));
    assert_eq!(reloaded.profiles.len(), 4);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_atomic_write_creates_bak_file_with_previous_state() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp_dir = create_unique_temp_dir("backup");
    let config_file = temp_dir.join("settings.json");
    let bak_file = temp_dir.join("settings.json.bak");

    let mgr = ConfigManager::with_custom_path(&config_file);

    // Write revision 1
    let mut v1 = create_default_settings();
    v1.charging_profile = Some("balanced".to_string());
    mgr.save(&v1).expect("save v1");

    assert!(config_file.exists());
    assert!(!bak_file.exists(), "No backup should exist after first write");

    // Write revision 2
    let mut v2 = v1.clone();
    v2.charging_profile = Some("stationary".to_string());
    mgr.save(&v2).expect("save v2");

    assert!(config_file.exists());
    assert!(bak_file.exists(), "Backup must exist after second write");

    // Verify target file has revision 2 and backup file has revision 1
    let cur_settings = ConfigManager::load_from_path(&config_file).expect("load cur");
    let bak_settings = ConfigManager::load_from_path(&bak_file).expect("load bak");

    assert_eq!(cur_settings.charging_profile.as_deref(), Some("stationary"));
    assert_eq!(bak_settings.charging_profile.as_deref(), Some("balanced"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_preview_mode_blocks_saving_via_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp_dir = create_unique_temp_dir("preview_block");
    let config_file = temp_dir.join("settings.json");

    // Activate read-only preview mode via environment variable
    std::env::set_var(ENV_READ_ONLY_PREVIEW, "1");
    assert!(is_read_only_preview());

    let mgr = ConfigManager::with_custom_path(&config_file);
    let settings = create_default_settings();

    let result = mgr.save(&settings);
    assert!(
        matches!(result, Err(ConfigError::ReadOnlyPreview)),
        "Saving in preview mode must return ConfigError::ReadOnlyPreview"
    );
    assert!(
        !config_file.exists(),
        "No configuration file may be created in preview mode"
    );

    // Deactivate preview mode
    std::env::remove_var(ENV_READ_ONLY_PREVIEW);
    assert!(!is_read_only_preview());

    // Should succeed once preview is disabled
    let success = mgr.save(&settings);
    assert!(success.is_ok());
    assert!(config_file.exists());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_public_defaults_canonical_ids_and_no_personal_power_limits() {
    let profiles = default_profiles();

    let mut ids = Vec::new();
    for p in &profiles {
        ids.push(p.id.as_str());
        // M2.4 Requirement: "исключить личные power limits из публичных дефолтов."
        assert!(
            p.odm_power_limits.is_empty(),
            "Profile {} must have no personal power limits in public defaults",
            p.name
        );
    }

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
fn test_load_and_roundtrip_legacy_fixture_settings() {
    let fixture_path = get_fixture_path("settings.json");
    if !fixture_path.exists() {
        eprintln!("Skipping fixture test: {:?} not found", fixture_path);
        return;
    }

    let settings =
        ConfigManager::load_from_path(&fixture_path).expect("failed to load settings.json fixture");

    // Verify key fields
    assert_eq!(settings.keyboard_layout.as_deref(), Some("frixit-mechrevo"));
    assert!(settings.state_map.contains_key("power_ac"));
    assert!(settings.state_map.contains_key("power_type_c"));
    assert!(settings.state_map.contains_key("power_bat"));

    // Verify round-trip serialization without error
    let serialized = settings.to_json_pretty().expect("reserialize settings");
    let reloaded = UccSettings::from_json(&serialized).expect("reload serialized settings");

    assert_eq!(settings.keyboard_layout, reloaded.keyboard_layout);
    assert_eq!(settings.state_map, reloaded.state_map);
    assert_eq!(settings.profiles.len(), reloaded.profiles.len());
}

#[test]
fn test_load_and_roundtrip_legacy_fixture_profiles() {
    let fixture_path = get_fixture_path("profiles.json");
    if !fixture_path.exists() {
        eprintln!("Skipping fixture test: {:?} not found", fixture_path);
        return;
    }

    let content = fs::read_to_string(&fixture_path).expect("read profiles.json");
    let catalog = ProfilesCatalog::from_json(&content).expect("parse profiles.json catalog");

    assert_eq!(catalog.profiles.len(), 3);
    let ids: Vec<&str> = catalog.profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["quiet", "balanced", "enthusiast"]);

    // Verify fan and display parsing
    let quiet = &catalog.profiles[0];
    assert_eq!(quiet.display.brightness, 50);
    assert_eq!(quiet.cpu.governor.as_str(), "powersave");
    assert_eq!(quiet.cpu.energy_performance_preference.as_str(), "power");
}

#[test]
fn test_load_personal_import_settings_fixture() {
    let fixture_path = get_fixture_path("personal-import-settings.json");
    if !fixture_path.exists() {
        eprintln!("Skipping fixture test: {:?} not found", fixture_path);
        return;
    }

    let settings = ConfigManager::load_from_path(&fixture_path)
        .expect("failed to load personal-import-settings.json");

    assert_eq!(settings.keyboard_layout.as_deref(), Some("frixit-mechrevo"));
    assert!(settings.profiles.contains_key("frixit-ac"));
    assert!(settings.profiles.contains_key("frixit-type_c"));
    assert!(settings.profiles.contains_key("frixit-bat"));
}

#[test]
fn test_permission_denied_on_readonly_directory() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp_dir = create_unique_temp_dir("perms");
    let readonly_dir = temp_dir.join("readonly_sub");
    fs::create_dir_all(&readonly_dir).expect("create readonly sub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Strip write permission from directory
        fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o555)).expect("chmod 555");

        let target_file = readonly_dir.join("forbidden.json");
        let mgr = ConfigManager::with_custom_path(&target_file);
        let settings = create_default_settings();

        let res = mgr.save(&settings);
        assert!(
            matches!(res, Err(ConfigError::PermissionDenied(_))),
            "Write to read-only directory must fail with ConfigError::PermissionDenied"
        );

        // Restore permissions for cleanup
        let _ = fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o755));
    }

    let _ = fs::remove_dir_all(&temp_dir);
}
