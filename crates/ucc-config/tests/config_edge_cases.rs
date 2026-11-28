use serde_json::json;
use std::fs;
use std::sync::Mutex;
use ucc_config::{
    create_default_settings, is_valid_settings_content, user_config_path, write_atomic,
    ConfigError, ConfigManager, SettingsManager, UccSettings,
};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn normal_load_validates_known_profile_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let doc =
        r#"{"profiles":{"p":{"id":"p","name":"P","fan":{"tableCPU":[{"temp":60,"speed":200}]}}}}"#;
    fs::write(&path, doc).unwrap();
    let result = ConfigManager::with_custom_path(&path).load();
    assert!(
        result.is_err(),
        "a profile containing 200% duty cycle was accepted"
    );
}

#[test]
fn single_pass_settings_deserialization_duplicate_keys_and_extra_fields() {
    let raw = r#"{
        "keyboardBacklightStates": [{"brightness": 10, "red": 1, "green": 2, "blue": 3}],
        "keyboardBacklightStates": [{"brightness": 99, "red": 255, "green": 128, "blue": 0}],
        "chargeStartThreshold": -1,
        "chargeEndThreshold": 80,
        "fahrenheit": true,
        "customVendorFlag": "preserved-extra"
    }"#;

    let parsed = UccSettings::from_json(raw).expect("single-pass parse must succeed");
    let states = parsed
        .keyboard_backlight_states
        .expect("keyboardBacklightStates present");
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].brightness, 99);
    assert_eq!(states[0].red, 255);
    assert_eq!(parsed.charge_start_threshold, None);
    assert_eq!(parsed.charge_end_threshold, Some(80));
    assert_eq!(parsed.fahrenheit, Some(true));
    assert_eq!(
        parsed.extra.get("customVendorFlag"),
        Some(&json!("preserved-extra"))
    );
}

#[test]
fn concurrent_write_atomic_has_no_temp_name_race_or_leaks() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("settings.json");

    // Seed initial valid file so backup rotation exercises concurrent temp names too.
    write_atomic(&target, br#"{"fahrenheit":false}"#).unwrap();

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let path = target.clone();
            std::thread::spawn(move || {
                for j in 0..10 {
                    let payload = format!(r#"{{"fahrenheit":{}}}"#, (i + j) % 2 == 0);
                    write_atomic(&path, payload.as_bytes()).expect("concurrent write_atomic");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("writer thread panicked");
    }

    // No orphaned temporary files may remain in the directory.
    let leftover_temps: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(
        leftover_temps.is_empty(),
        "unexpected leftover temp files: {leftover_temps:?}"
    );
}

#[test]
fn quarantine_validation_aligned_with_is_valid_settings_content() {
    let mut base = serde_json::to_value(create_default_settings()).unwrap();
    // Add one unreferenced invalid legacy profile alongside healthy referenced profiles.
    base["profiles"]["legacy-broken"] = json!({
        "id": "legacy-broken",
        "name": "Legacy Broken",
        "cpu": { "onlineCores": 0 },
        "odmPowerLimits": { "tdpValues": [80, 60] }
    });
    let with_quarantinable = serde_json::to_string_pretty(&base).unwrap();
    assert!(
        is_valid_settings_content(&with_quarantinable),
        "is_valid_settings_content must agree with load_from_path quarantine rules"
    );

    // A file where all profiles are invalid must still be rejected.
    let all_invalid =
        r#"{"profiles":{"p":{"id":"p","name":"P","fan":{"tableCPU":[{"temp":60,"speed":200}]}}}}"#;
    assert!(!is_valid_settings_content(all_invalid));
}

#[test]
fn user_settings_json_bak_recovery_when_primary_missing() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ucc_dir = dir.path().join("ucc");
    fs::create_dir_all(&ucc_dir).unwrap();

    // Primary `settings` and `settings.json` are missing; only `settings.json.bak` exists.
    let json_bak = ucc_dir.join("settings.json.bak");
    fs::write(&json_bak, r#"{"fahrenheit":true,"keyboardLayout":"iso"}"#).unwrap();

    let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
    std::env::set_var("XDG_CONFIG_HOME", dir.path());

    let expected_json = ucc_dir.join("settings.json");
    let resolved_user = user_config_path();
    let mgr = ConfigManager::new();
    let resolved_read = mgr.resolve_read_path();
    let loaded = mgr.load();

    if let Some(prev) = prev_xdg {
        std::env::set_var("XDG_CONFIG_HOME", prev);
    } else {
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    assert_eq!(resolved_user.as_deref(), Some(expected_json.as_path()));
    assert_eq!(resolved_read.as_deref(), Some(expected_json.as_path()));
    let settings = loaded.expect("load must recover from user settings.json.bak");
    assert_eq!(settings.fahrenheit, Some(true));
    assert_eq!(settings.keyboard_layout.as_deref(), Some("iso"));
}

#[test]
fn legacy_settings_manager_rejects_oversized_files_and_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join("settings.json");
    let bak = dir.path().join("settings.json.bak");

    // Create a sparse/oversized primary file (> 16 MiB).
    let f = fs::File::create(&primary).unwrap();
    f.set_len(17 * 1024 * 1024).unwrap();

    // Without backup, oversized primary fails with ParseError.
    let mgr = SettingsManager::new(&primary);
    assert!(matches!(mgr.read_settings(), Err(ConfigError::ParseError)));

    // With valid backup, falls back cleanly.
    fs::write(&bak, r#"{"keyboardLayout":"recovered-from-bak"}"#).unwrap();
    let recovered = mgr
        .read_settings()
        .expect("should fall back to .bak when primary exceeds 16 MiB");
    assert_eq!(recovered.keyboard_layout, "recovered-from-bak");
}

#[test]
fn persistence_uses_fd_set_permissions_not_path_toctou() {
    let src = include_str!("../src/persistence.rs");
    assert!(
        !src.contains("fs::set_permissions"),
        "persistence.rs must use FD-based File::set_permissions (fchmod), not path-based fs::set_permissions"
    );
}
