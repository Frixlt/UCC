// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(unix)]
use ucc_config::write_atomic;
use ucc_config::{
    cleanup_temp_files, default_high_performance_profile, default_max_energy_save_profile,
    default_office_profile, default_quiet_profile, ConfigManager, UccSettings,
};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ucc_audit_cfg_{}_{}_{}",
        prefix,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn t_sec_03_write_atomic_over_symlink_preserves_target_mode_and_never_sets_0777() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let dir = unique_temp_dir("symlink");
    let real_target = dir.join("real-settings.json");
    let link_path = dir.join("settings.json");

    fs::write(&real_target, "{}\n").unwrap();
    fs::set_permissions(&real_target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&real_target, &link_path).unwrap();

    let payload = serde_json::to_string_pretty(&ucc_config::create_default_settings()).unwrap();
    write_atomic(&link_path, payload.as_bytes()).expect("write_atomic over symlink");

    let link_meta = fs::symlink_metadata(&link_path).unwrap();
    assert!(
        link_meta.file_type().is_symlink(),
        "atomic save through symlink must write to resolved target rather than clobbering symlink"
    );

    let target_mode = fs::metadata(&link_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        target_mode, 0o600,
        "write_atomic leaked symlink 0o777 permissions onto config file (mode={target_mode:#o})"
    );

    let bak_path = dir.join("real-settings.json.bak");
    if bak_path.exists() {
        let bak_mode = fs::symlink_metadata(&bak_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(bak_mode, 0o600, "backup mode was {bak_mode:#o}");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t_cfg_01_settings_roundtrip_preserves_cpp_profile_charging_and_ctgp() {
    let dir = unique_temp_dir("cpp_charging");
    let path = dir.join("settings.json");

    let mut base = serde_json::to_value(ucc_config::create_default_settings()).unwrap();
    base["profiles"]["custom-charge"] = json!({
        "id": "custom-charge",
        "name": "Custom Charge",
        "display": { "brightness": 70, "useBrightness": true, "refreshRate": 60, "useRefRate": false },
        "cpu": { "governor": "powersave", "energyPerformancePreference": "balance_power" },
        "webcam": { "status": true, "useStatus": false },
        "fan": { "fanProfile": "fan-quiet", "useControl": true },
        "odmProfile": { "name": "balanced" },
        "odmPowerLimits": { "tdpValues": [45, 70, 95] },
        "chargingProfile": "stationary",
        "chargingPriority": "charge_battery",
        "chargeType": "Custom",
        "chargeStartThreshold": 50,
        "chargeEndThreshold": 80,
        "nvidiaCTGPOffset": -5
    });

    fs::write(&path, serde_json::to_string_pretty(&base).unwrap()).unwrap();

    let loaded = ConfigManager::load_from_path(&path).expect("load settings with C++ charging");
    ConfigManager::with_custom_path(&path)
        .save(&loaded)
        .expect("resave settings");

    let raw_after: Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("parse resaved json");
    let saved_prof = &raw_after["profiles"]["custom-charge"];

    assert_eq!(saved_prof["chargingProfile"], "stationary");
    assert_eq!(saved_prof["chargingPriority"], "charge_battery");
    assert_eq!(saved_prof["chargeType"], "Custom");
    assert_eq!(saved_prof["chargeStartThreshold"], 50);
    assert_eq!(saved_prof["chargeEndThreshold"], 80);
    assert_eq!(saved_prof["nvidiaCTGPOffset"], -5);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t_cfg_03_load_quarantines_single_legacy_invalid_profile_without_rejecting_file() {
    let dir = unique_temp_dir("legacy_partial");
    let path = dir.join("settings.json");

    let mut base = serde_json::to_value(ucc_config::create_default_settings()).unwrap();
    base["profiles"]["good-user"] = json!({
        "id": "good-user",
        "name": "Good User",
        "cpu": { "governor": "powersave" },
        "fan": { "useControl": true },
        "odmPowerLimits": { "tdpValues": [45, 70, 95] }
    });
    base["stateMap"]["power_ac"] = json!("good-user");
    // Inactive legacy profile with non-monotonic TDP and equal adjacent fan curve temps
    base["profiles"]["old-legacy-bad"] = json!({
        "id": "old-legacy-bad",
        "name": "Legacy Quirky",
        "cpu": { "onlineCores": 0, "governor": "powersave" },
        "fan": {
            "useControl": true,
            "tableCPU": [{"temp": 50, "speed": 30}, {"temp": 50, "speed": 40}],
            "tableGPU": []
        },
        "odmPowerLimits": { "tdpValues": [80, 60] }
    });

    fs::write(&path, serde_json::to_string_pretty(&base).unwrap()).unwrap();

    let loaded = ConfigManager::load_from_path(&path)
        .expect("one quirky legacy profile must not reject the entire settings file");
    assert!(loaded.profiles.contains_key("good-user"));
    assert_eq!(
        loaded.state_map.get("power_ac").map(String::as_str),
        Some("good-user")
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t_cfg_04_defaults_match_cpp_fan_ids_and_water_cooler_defaults() {
    let energy = default_max_energy_save_profile();
    let quiet = default_quiet_profile();
    let office = default_office_profile();
    let perf = default_high_performance_profile();

    assert_eq!(energy.fan.fan_profile, "fan-silent");
    assert_eq!(quiet.fan.fan_profile, "fan-silent");
    assert_eq!(office.fan.fan_profile, "fan-quiet");
    assert_eq!(perf.fan.fan_profile, "fan-balanced");

    assert!(
        !perf.fan.enable_water_cooler,
        "High Performance built-in must not force BLE water cooler scanning by default"
    );
    assert!(
        perf.fan.auto_control_wc,
        "High Performance built-in must default auto_control_wc to true"
    );
    assert_eq!(perf.fan.manual_fan_speed, None);
    assert!(!energy.cpu.no_turbo);
}

#[test]
fn t_cfg_05_get_profile_does_not_shadow_deleted_custom_profile_via_substring() {
    let settings: UccSettings = ucc_config::create_default_settings();
    assert!(settings.get_profile("My Gaming Performance").is_none());
    assert!(settings.get_profile("Quiet Night Mode").is_none());
    assert!(settings.get_profile("Office Travel Profile").is_none());
}

#[test]
fn t_cfg_06_cleanup_temp_files_removes_restore_temp_orphans() {
    let dir = unique_temp_dir("cleanup_restore");
    let orphan = dir.join(".settings.json.tmp_restore.1234.5678");
    fs::write(&orphan, "stale").unwrap();

    let _ = cleanup_temp_files(&dir);

    assert!(
        !orphan.exists(),
        "cleanup_temp_files left .tmp_restore. orphan behind"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn t_sec_02_resolve_read_path_checks_euid_before_user_config() {
    let src = include_str!("../src/manager.rs");
    let fn_start = src.find("pub fn resolve_read_path").unwrap();
    let fn_end = src[fn_start..]
        .find("pub fn resolve_write_path")
        .map(|off| fn_start + off)
        .unwrap();
    let resolve_read_body = &src[fn_start..fn_end];
    let user_pos = resolve_read_body
        .find("user_config_path()")
        .expect("resolve_read_path references user_config_path");
    let euid_pos = resolve_read_body
        .find("geteuid")
        .expect("resolve_read_path must guard user_config_path with geteuid() == 0");
    assert!(
        euid_pos < user_pos,
        "resolve_read_path must check geteuid() before reading user_config_path"
    );
}
