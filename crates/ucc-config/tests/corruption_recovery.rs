//! Stress tests and fault-injection tests of configuration persistence,
//! corrupted JSON handling, .bak recovery, and atomic write resilience.
//!
//! Addresses M2.6, M2.7, and M2.8.

use std::fs;
use std::path::{Path, PathBuf};
use ucc_config::{
    cleanup_temp_files, create_default_settings, ConfigError, ConfigManager, ProfilesCatalog,
    UccSettings,
};

fn create_unique_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ucc_stress_{}_{}_{}",
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

fn get_fixture_path(name: &str) -> PathBuf {
    for prefix in &["../../artifacts/rust-migration/fixtures", "../../../../artifacts/rust-migration/fixtures"] {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(prefix).join(name);
        if p.exists() {
            return p;
        }
    }
    Path::new("/home/frixit/Рабочий стол/fork control center/UCC-Revorked/artifacts/rust-migration/fixtures").join(name)
}

// ========================================================================
// 1. Corrupted JSON Tests (Syntax Errors, Truncated Writes, Invalid Types)
// ========================================================================

#[test]
fn test_corrupted_json_syntax_errors() {
    let temp_dir = create_unique_temp_dir("syntax_err");
    let test_file = temp_dir.join("corrupted.json");

    let malformed_payloads = [
        ("unclosed_brace", r#"{"stateMap": {"power_ac": "performance""#),
        ("trailing_comma", r#"{"stateMap": {"power_ac": "performance",,}}"#),
        ("unquoted_token", r#"{"stateMap": {"power_ac": performance}}"#),
        ("colon_without_value", r#"{"stateMap": {"power_ac": }}"#),
        ("single_quote", r#"{'stateMap': {'power_ac': 'performance'}}"#),
        ("plain_text", "This is completely invalid non-JSON content!"),
        ("empty_string", ""),
    ];

    for (name, payload) in malformed_payloads {
        fs::write(&test_file, payload).expect("write malformed payload");

        let err = ConfigManager::load_from_path(&test_file)
            .expect_err(&format!("Payload '{}' must fail parsing", name));

        match err {
            ConfigError::Json(json_err) => {
                assert!(
                    !json_err.to_string().is_empty(),
                    "Error description must be informative for {}",
                    name
                );
            }
            other => panic!("Expected ConfigError::Json, got: {:?}", other),
        }

        // Also test direct UccSettings::from_json
        let direct_res = UccSettings::from_json(payload);
        assert!(direct_res.is_err(), "Direct parsing must fail for {}", name);
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_corrupted_json_truncated_write() {
    let temp_dir = create_unique_temp_dir("truncated");
    let test_file = temp_dir.join("truncated.json");

    let default_settings = create_default_settings();
    let valid_json = default_settings
        .to_json_pretty()
        .expect("serialize default settings");
    let total_len = valid_json.len();

    // Test truncation at various fractions: 1%, 10%, 25%, 50%, 75%, 90%, 99%
    let cutoffs = [
        total_len / 100,
        total_len / 10,
        total_len / 4,
        total_len / 2,
        (total_len * 3) / 4,
        (total_len * 9) / 10,
        total_len - 5,
        total_len - 1,
    ];

    for &cut in &cutoffs {
        if cut == 0 || cut >= total_len {
            continue;
        }
        let truncated = &valid_json[..cut];
        fs::write(&test_file, truncated).expect("write truncated json");

        let err = ConfigManager::load_from_path(&test_file)
            .expect_err(&format!("Truncation at byte {} must fail", cut));

        match err {
            ConfigError::Json(e) => {
                assert!(e.is_eof() || e.is_syntax());
            }
            other => panic!("Expected ConfigError::Json, got: {:?}", other),
        }
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_corrupted_json_invalid_types() {
    let temp_dir = create_unique_temp_dir("invalid_types");
    let test_file = temp_dir.join("invalid_types.json");

    let invalid_type_cases = [
        ("string_where_array_expected", r#"{"keyboardBacklightStates": "not_an_array"}"#),
        ("object_where_int_expected", r#"{"chargeStartThreshold": {"val": 40}}"#),
        ("array_where_map_expected", r#"{"stateMap": ["power_ac", "performance"]}"#),
        ("int_where_map_expected", r#"{"profiles": 42}"#),
        ("null_where_string_expected", r#"{"keyboardLayout": 12345}"#),
    ];

    for (case_name, payload) in invalid_type_cases {
        fs::write(&test_file, payload).expect("write invalid type json");

        let err = ConfigManager::load_from_path(&test_file)
            .expect_err(&format!("Case '{}' must fail with type error", case_name));

        match err {
            ConfigError::Json(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("invalid type") || msg.contains("expected"),
                    "Error for '{}' should mention type expectation: {}",
                    case_name,
                    msg
                );
            }
            other => panic!("Expected ConfigError::Json, got: {:?}", other),
        }
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_corrupted_json_binary_garbage() {
    let temp_dir = create_unique_temp_dir("binary_garbage");
    let test_file = temp_dir.join("garbage.json");

    // Random non-UTF8 bytes and control characters
    let garbage: [u8; 32] = [
        0xFF, 0xFE, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03, 0x80, 0x81,
        0x82, 0x83, 0x00, 0x00, 0xFF, 0xAA, 0x55, 0xCC, 0x33, 0x00, 0x1B, 0x5B, 0x32, 0x4A,
        0x00, 0x00, 0x00, 0x00,
    ];

    fs::write(&test_file, garbage).expect("write binary garbage");

    let err = ConfigManager::load_from_path(&test_file).expect_err("Binary garbage must not load");
    // Could fail at UTF-8 read or JSON decode
    assert!(matches!(err, ConfigError::Io(_) | ConfigError::Json(_)));

    let _ = fs::remove_dir_all(&temp_dir);
}

// ========================================================================
// 2. Backup Recovery Tests (.bak recovery upon corruption)
// ========================================================================

#[test]
fn test_recovery_from_backup_when_main_file_is_corrupted() {
    let temp_dir = create_unique_temp_dir("recovery_main_corrupt");
    let config_file = temp_dir.join("settings.json");
    let bak_file = temp_dir.join("settings.json.bak");

    let mgr = ConfigManager::with_custom_path(&config_file);

    // 1. Save revision 1
    let mut v1 = create_default_settings();
    v1.charging_profile = Some("stationary".to_string());
    v1.charge_end_threshold = Some(80);
    mgr.save(&v1).expect("save revision 1");

    // 2. Save revision 2 -> this creates settings.json.bak containing revision 1
    let mut v2 = v1.clone();
    v2.charging_profile = Some("balanced".to_string());
    v2.charge_end_threshold = Some(90);
    mgr.save(&v2).expect("save revision 2");

    assert!(config_file.exists());
    assert!(bak_file.exists());

    // 3. Corrupt the main settings.json with invalid truncated JSON
    fs::write(&config_file, r#"{"stateMap": {"power_ac": "invalid"#).expect("corrupt main");

    // Direct load_from_path must fail
    assert!(ConfigManager::load_from_path(&config_file).is_err());

    // 4. load_with_recovery must detect the corruption and restore from .bak
    let (recovered_settings, was_recovered) =
        ConfigManager::load_with_recovery(&config_file).expect("recovery from backup");
    assert!(was_recovered, "was_recovered must be true");
    // Revision in backup was revision 1 ("stationary", 80%)
    assert_eq!(recovered_settings.charging_profile.as_deref(), Some("stationary"));
    assert_eq!(recovered_settings.charge_end_threshold, Some(80));

    // 5. ConfigManager::load() using custom path must also recover automatically
    let auto_recovered = mgr.load().expect("auto load with recovery");
    assert_eq!(auto_recovered.charging_profile.as_deref(), Some("stationary"));

    // 6. Explicitly restore from backup to replace the corrupted main file
    let restored = ConfigManager::restore_from_backup(&config_file).expect("restore from backup");
    assert_eq!(restored.charging_profile.as_deref(), Some("stationary"));

    // Now direct load_from_path succeeds again
    let fresh_load = ConfigManager::load_from_path(&config_file).expect("fresh load");
    assert_eq!(fresh_load.charging_profile.as_deref(), Some("stationary"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_recovery_from_backup_when_main_is_zero_bytes() {
    let temp_dir = create_unique_temp_dir("recovery_zero_bytes");
    let config_file = temp_dir.join("settings.json");
    let bak_file = temp_dir.join("settings.json.bak");

    let mgr = ConfigManager::with_custom_path(&config_file);

    // Save initial valid settings
    let mut v1 = create_default_settings();
    v1.charging_profile = Some("high_capacity".to_string());
    mgr.save(&v1).expect("save initial");

    // Save again to create backup
    mgr.save(&v1).expect("save again to create bak");
    assert!(bak_file.exists());

    // Truncate main file to 0 bytes (simulating power failure right during truncation)
    fs::write(&config_file, b"").expect("truncate main to zero");
    assert_eq!(fs::metadata(&config_file).unwrap().len(), 0);

    // Recovery must succeed from backup
    let (recovered, was_recovered) =
        ConfigManager::load_with_recovery(&config_file).expect("recover from bak");
    assert!(was_recovered);
    assert_eq!(recovered.charging_profile.as_deref(), Some("high_capacity"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_failure_when_both_main_and_backup_are_corrupted() {
    let temp_dir = create_unique_temp_dir("both_corrupted");
    let config_file = temp_dir.join("settings.json");
    let bak_file = temp_dir.join("settings.json.bak");

    // Both files contain garbage
    fs::write(&config_file, "{corrupted main").expect("write bad main");
    fs::write(&bak_file, "{corrupted backup").expect("write bad bak");

    let res = ConfigManager::load_with_recovery(&config_file);
    assert!(res.is_err(), "Must return error when backup is also corrupted");

    let restore_res = ConfigManager::restore_from_backup(&config_file);
    assert!(restore_res.is_err());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_failure_when_main_is_corrupted_and_no_backup_exists() {
    let temp_dir = create_unique_temp_dir("no_backup");
    let config_file = temp_dir.join("settings.json");
    let bak_file = temp_dir.join("settings.json.bak");

    fs::write(&config_file, "not valid json").expect("write bad main");
    assert!(!bak_file.exists());

    let res = ConfigManager::load_with_recovery(&config_file);
    assert!(res.is_err(), "Must fail when no backup exists");

    let restore_res = ConfigManager::restore_from_backup(&config_file);
    assert!(restore_res.is_err());
    match restore_res {
        Err(ConfigError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("Expected NotFound IO error, got: {:?}", other),
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

// ========================================================================
// 3. Atomic Write Resilience & Temp File Cleanup Tests
// ========================================================================

#[test]
fn test_atomic_write_preserves_target_on_failure() {
    let temp_dir = create_unique_temp_dir("atomic_resilience");
    let config_file = temp_dir.join("settings.json");

    let mgr = ConfigManager::with_custom_path(&config_file);

    // 1. Initial valid write
    let initial_settings = create_default_settings();
    mgr.save(&initial_settings).expect("initial write");
    let initial_content = fs::read_to_string(&config_file).expect("read initial");

    // 2. Make target file read-only to simulate write failure
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config_file, fs::Permissions::from_mode(0o444))
            .expect("set readonly");

        let mut modified = initial_settings.clone();
        modified.charging_profile = Some("should_not_be_saved".to_string());

        let res = mgr.save(&modified);
        assert!(
            res.is_err(),
            "Write to read-only file must fail with permission error"
        );

        // 3. Restore permissions to verify contents
        fs::set_permissions(&config_file, fs::Permissions::from_mode(0o644))
            .expect("restore permissions");

        let content_after_failure = fs::read_to_string(&config_file).expect("read after fail");
        assert_eq!(
            initial_content, content_after_failure,
            "Target file must remain completely undamaged and unmodified after write failure"
        );

        // 4. Assert NO temporary files remain
        let temp_files_count = fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(
            temp_files_count, 0,
            "Temporary file must be cleaned up on failure"
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_cleanup_stale_temp_files() {
    let temp_dir = create_unique_temp_dir("temp_cleanup");

    // Create several orphan temporary files left from crashes
    let stale_tmp1 = temp_dir.join(".settings.json.tmp.1234.169999999");
    let stale_tmp2 = temp_dir.join(".settings.json.tmp.5678.170000000");
    let stale_tmp3 = temp_dir.join(".custom.json.tmp.9999.170000001");
    let legit_file = temp_dir.join("settings.json");
    let legit_bak = temp_dir.join("settings.json.bak");

    fs::write(&stale_tmp1, "orphaned tmp 1").expect("write tmp1");
    fs::write(&stale_tmp2, "orphaned tmp 2").expect("write tmp2");
    fs::write(&stale_tmp3, "orphaned tmp 3").expect("write tmp3");
    fs::write(&legit_file, "legitimate file").expect("write legit");
    fs::write(&legit_bak, "legitimate bak").expect("write legit bak");

    assert!(stale_tmp1.exists());
    assert!(stale_tmp2.exists());
    assert!(stale_tmp3.exists());

    let cleaned = cleanup_temp_files(&temp_dir).expect("cleanup stale temp files");
    assert_eq!(cleaned, 3, "Must clean up exactly 3 orphan temp files");

    assert!(!stale_tmp1.exists());
    assert!(!stale_tmp2.exists());
    assert!(!stale_tmp3.exists());
    assert!(legit_file.exists(), "Legitimate settings file must NOT be removed");
    assert!(legit_bak.exists(), "Legitimate bak file must NOT be removed");

    let _ = fs::remove_dir_all(&temp_dir);
}

// ========================================================================
// 4. Differential & Round-Trip Tests (Fixtures)
// ========================================================================

#[test]
fn test_personal_import_settings_differential_and_roundtrip() {
    let fixture_path = get_fixture_path("personal-import-settings.json");
    assert!(
        fixture_path.exists(),
        "Fixture personal-import-settings.json must exist at {}",
        fixture_path.display()
    );

    let original_json = fs::read_to_string(&fixture_path).expect("read fixture");
    let parsed: UccSettings =
        UccSettings::from_json(&original_json).expect("parse personal-import-settings.json");

    // 1. Structural assertions matching baseline fixture
    assert_eq!(parsed.keyboard_layout.as_deref(), Some("frixit-mechrevo"));

    let backlight = parsed
        .keyboard_backlight_states
        .as_ref()
        .expect("keyboardBacklightStates present");
    assert_eq!(backlight.len(), 3, "Must have exactly 3 keyboard backlight zones");
    for zone in backlight {
        assert_eq!(zone.red, 0);
        assert_eq!(zone.green, 120);
        assert_eq!(zone.blue, 215);
        assert_eq!(zone.brightness, 35);
    }

    // Custom keyboard profiles
    let custom_kb = parsed
        .custom_keyboard_profiles
        .as_ref()
        .expect("customKeyboardProfiles present");
    assert!(custom_kb.contains_key("frixit-keyboard"));

    // State map
    assert_eq!(parsed.state_map.get("power_ac").map(String::as_str), Some("frixit-ac"));
    assert_eq!(parsed.state_map.get("power_type_c").map(String::as_str), Some("frixit-type_c"));
    assert_eq!(parsed.state_map.get("power_bat").map(String::as_str), Some("frixit-bat"));

    // Profiles catalog
    assert_eq!(parsed.profiles.len(), 3);
    let ac_profile = parsed.profiles.get("frixit-ac").expect("frixit-ac profile");
    assert_eq!(ac_profile.id, "frixit-ac");
    assert_eq!(ac_profile.name, "Frixit AC");
    assert_eq!(ac_profile.odm_power_limits, vec![25, 45, 65]);
    assert_eq!(ac_profile.cpu.governor, "performance");
    assert_eq!(ac_profile.cpu.energy_performance_preference, "performance");

    // 2. Semantic round-trip: serialize and re-parse
    let serialized_pretty = parsed.to_json_pretty().expect("serialize to pretty JSON");
    let roundtripped: UccSettings =
        UccSettings::from_json(&serialized_pretty).expect("re-parse serialized settings");

    assert_eq!(
        parsed, roundtripped,
        "Deserialized struct must be strictly identical across round-trip serialization"
    );

    // 3. Serialization diff assertions: no duplicate keys
    let mut seen_keys = std::collections::HashSet::new();
    for line in serialized_pretty.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('"') && trimmed.contains("\":") {
            let key = trimmed.split("\":").next().unwrap().trim_matches('"');
            // Check top-level keys
            if !line.starts_with("    ") && !line.starts_with("      ") {
                assert!(
                    seen_keys.insert(key.to_string()),
                    "Duplicate top-level key found in serializer output: {}",
                    key
                );
            }
        }
    }

    // Verify key fields are preserved in raw JSON string
    assert!(serialized_pretty.contains("\"keyboardLayout\": \"frixit-mechrevo\""));
    assert!(serialized_pretty.contains("\"keyboardBacklightStates\": ["));
    assert!(serialized_pretty.contains("\"frixit-ac\""));
    assert!(serialized_pretty.contains("\"frixit-type_c\""));
    assert!(serialized_pretty.contains("\"frixit-bat\""));
    assert!(serialized_pretty.contains("\"odmPowerLimits\": {"));
    assert!(serialized_pretty.contains("\"tdpValues\": ["));
}

#[test]
fn test_reproducer_rejected_old_settings_fixture_validation() {
    let fixture_path = get_fixture_path("reproducer-rejected-old-settings.json");
    assert!(fixture_path.exists());

    let content = fs::read_to_string(&fixture_path).expect("read rejected fixture");
    let parsed: UccSettings = UccSettings::from_json(&content).expect("raw deserialization");

    // This fixture lacks stateMap and profiles
    assert!(
        parsed.state_map.is_empty(),
        "reproducer-rejected-old-settings lacks stateMap"
    );
    assert!(
        parsed.profiles.is_empty(),
        "reproducer-rejected-old-settings lacks profiles catalog"
    );

    // Profile resolution must fail
    assert!(parsed.get_profile("balanced").is_none());
    assert!(parsed.get_profile("enthusiast").is_none());
    assert!(parsed.get_active_profile_for_mode("power_ac").is_none());

    // Legacy/unknown keys must be captured in extra without crashing
    assert!(parsed.extra.contains_key("activeProfile"));
    assert!(parsed.extra.contains_key("manualPowerMode"));
    assert!(parsed.extra.contains_key("powerPolicy"));
}

#[test]
fn test_legacy_profiles_fixture_roundtrip() {
    let fixture_path = get_fixture_path("profiles.json");
    assert!(fixture_path.exists());

    let content = fs::read_to_string(&fixture_path).expect("read profiles fixture");
    let catalog = ProfilesCatalog::from_json(&content).expect("parse profiles.json");

    assert_eq!(catalog.profiles.len(), 3);
    let ids: Vec<&str> = catalog.profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["quiet", "balanced", "enthusiast"]);

    // Test alias lookup
    let mut settings = create_default_settings();
    // Insert enthusiast profile into settings catalog
    let enthusiast = catalog.profiles[2].clone();
    settings.profiles.insert("enthusiast".to_string(), enthusiast);

    // Profile lookup with alias
    let looked_up = settings.get_profile("enthusiast");
    assert!(looked_up.is_some());
    assert_eq!(looked_up.unwrap().name, "Enthusiast");
}

#[test]
fn test_settings_fixture_roundtrip() {
    let fixture_path = get_fixture_path("settings.json");
    assert!(fixture_path.exists());

    let content = fs::read_to_string(&fixture_path).expect("read settings fixture");
    let parsed: UccSettings = UccSettings::from_json(&content).expect("parse settings.json");

    assert_eq!(parsed.keyboard_layout.as_deref(), Some("frixit-mechrevo"));
    assert_eq!(parsed.profiles.len(), 3);

    let reserialized = parsed.to_json_pretty().expect("reserialize");
    let reloaded = UccSettings::from_json(&reserialized).expect("reload");

    assert_eq!(parsed, reloaded);
}
