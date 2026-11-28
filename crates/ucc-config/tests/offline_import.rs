//! Full parity tests for the Rust offline importer (`M11.1`, `M11.2`),
//! matching `tests/test_tcc_import.py`, `tests/test_controlcenter_migration.py`,
//! and `contrib/mechrevo/tests/test_migration.py`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use tempfile::TempDir;
use ucc_config::{
    convert_mechrevo_tcc, convert_offline_import, desktop_preferences, effective_tcc_fan_table,
    merge_tcc, read_tcc_fan_presets, write_offline_import_bundle, OfflineUserData, TccFanPoint,
    TccFanPreset,
};

fn build_asar_archive(path: &Path, entry_bytes: &[u8]) {
    let mut tree = json!({
        "size": entry_bytes.len(),
        "offset": "0",
    });
    for part in [
        "dist",
        "tuxedo-control-center",
        "service-app",
        "common",
        "models",
        "TccFanTable.js",
    ]
    .into_iter()
    .rev()
    {
        tree = json!({ "files": { (part): tree } });
    }
    let header = serde_json::to_vec(&tree).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&4_u32.to_le_bytes());
    out.extend_from_slice(&((header.len() + 8) as u32).to_le_bytes());
    out.extend_from_slice(&((header.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(entry_bytes);
    fs::write(path, out).unwrap();
}

#[test]
fn tcc_active_named_curve_wins_over_unused_custom_curve_and_bakes_modifiers() {
    let points = vec![
        TccFanPoint { temp: 0, speed: 0 },
        TccFanPoint {
            temp: 100,
            speed: 100,
        },
    ];
    let mut presets = BTreeMap::new();
    presets.insert(
        "Silent".to_string(),
        TccFanPreset {
            table_cpu: points.clone(),
            table_gpu: points,
        },
    );

    let profile = json!({
        "id": "user",
        "name": "Actual profile",
        "cpu": { "onlineCores": 8, "scalingMaxFrequency": 3400000 },
        "display": { "useRefRate": false, "refreshRate": 59.99 },
        "fan": {
            "useControl": true,
            "fanProfile": "Silent",
            "minimumFanspeed": 15,
            "maximumFanspeed": 68,
            "offsetFanspeed": 10,
            "customFanCurve": {
                "tableCPU": [{ "temp": 0, "speed": 99 }, { "temp": 100, "speed": 99 }],
                "tableGPU": [{ "temp": 0, "speed": 99 }, { "temp": 100, "speed": 99 }],
            }
        },
        "odmPowerLimits": { "tdpValues": [11, 20, 32] },
        "nvidiaPowerCTRLProfile": { "cTGPOffset": 95 }
    });
    let profiles = json!([profile.clone()]);
    let tcc_settings = json!({ "chargingProfile": "stationary" });

    let (merged, report) = merge_tcc(&json!({}), &tcc_settings, &profiles, &presets).unwrap();
    let merged_profile = &merged["profiles"]["user"];
    let table_cpu = merged_profile["fan"]["tableCPU"].as_array().unwrap();
    assert_eq!(
        [
            table_cpu[0]["speed"].as_i64().unwrap(),
            table_cpu[20]["speed"].as_i64().unwrap(),
            table_cpu[58]["speed"].as_i64().unwrap(),
            table_cpu[90]["speed"].as_i64().unwrap(),
        ],
        [15, 30, 68, 68]
    );
    assert_eq!(merged_profile["odmPowerLimits"], profile["odmPowerLimits"]);
    assert_eq!(merged_profile["cpu"], profile["cpu"]);
    assert_eq!(merged_profile["display"]["refreshRate"], json!(60));
    assert_eq!(merged_profile["nvidiaCTGPOffset"], json!(95));
    assert_eq!(merged["chargingProfile"], json!("stationary"));
    assert_eq!(report["profiles"]["user"]["fanPreset"], json!("Silent"));
    let fan_profile_id = merged_profile["fan"]["fanProfile"].as_str().unwrap();
    assert_eq!(
        merged["customFanProfiles"][fan_profile_id]["json"]["tableGPU"],
        merged_profile["fan"]["tableGPU"]
    );
}

#[test]
fn tcc_custom_curve_ignores_named_modifiers_and_uses_half_up_rounding() {
    let profile = json!({
        "id": "user",
        "name": "Actual profile",
        "fan": {
            "useControl": true,
            "fanProfile": "Custom",
            "minimumFanspeed": 15,
            "maximumFanspeed": 68,
            "offsetFanspeed": 10,
            "customFanCurve": {
                "tableCPU": [{ "temp": 0, "speed": 0 }, { "temp": 100, "speed": 50 }],
                "tableGPU": [{ "temp": 0, "speed": 0 }, { "temp": 100, "speed": 50 }],
            }
        }
    });
    let (merged, _) = merge_tcc(&json!({}), &json!({}), &json!([profile]), &BTreeMap::new())
        .expect("custom curve merge");
    let values = merged["profiles"]["user"]["fan"]["tableCPU"]
        .as_array()
        .unwrap();
    assert_eq!(
        [
            values[0]["speed"].as_i64().unwrap(),
            values[1]["speed"].as_i64().unwrap(),
            values[3]["speed"].as_i64().unwrap(),
            values[100]["speed"].as_i64().unwrap(),
        ],
        [0, 1, 2, 50]
    );
}

#[test]
fn tcc_exported_ucc_profile_and_cooler_settings_are_preserved() {
    let points = vec![
        TccFanPoint { temp: 0, speed: 0 },
        TccFanPoint {
            temp: 100,
            speed: 100,
        },
    ];
    let mut presets = BTreeMap::new();
    presets.insert(
        "Silent".to_string(),
        TccFanPreset {
            table_cpu: points.clone(),
            table_gpu: points,
        },
    );
    let profile = json!({
        "id": "user",
        "name": "Actual profile",
        "fan": { "useControl": true, "fanProfile": "Silent" }
    });
    let exported = json!({
        "profiles": { "user": { "id": "user", "fan": { "autoControlWC": true, "manualPumpVoltage": 4 } } },
        "stateMap": { "power_wc": "user" }
    });

    let (merged, _) = merge_tcc(
        &exported,
        &json!({ "keyboardBacklightControlEnabled": false }),
        &json!([profile]),
        &presets,
    )
    .unwrap();
    assert_eq!(merged["profiles"]["user"], exported["profiles"]["user"]);
    assert_eq!(merged["stateMap"], exported["stateMap"]);
    assert_eq!(merged["keyboardBacklightControlSupported"], json!(false));
}

#[test]
fn tcc_missing_named_curve_and_invalid_tables_fail() {
    let profile = json!({
        "id": "user",
        "fan": { "useControl": true, "fanProfile": "Silent" }
    });
    let err = merge_tcc(&json!({}), &json!({}), &json!([profile]), &BTreeMap::new()).unwrap_err();
    assert!(err.to_string().contains("unavailable"), "got {err}");

    assert!(effective_tcc_fan_table(&[], 0, 100, 0).is_err());
    assert!(effective_tcc_fan_table(
        &[
            TccFanPoint { temp: 1, speed: 1 },
            TccFanPoint { temp: 1, speed: 1 }
        ],
        0,
        100,
        0
    )
    .is_err());
    assert!(effective_tcc_fan_table(
        &[
            TccFanPoint {
                temp: 0,
                speed: 101
            },
            TccFanPoint {
                temp: 100,
                speed: 100
            }
        ],
        0,
        100,
        0
    )
    .is_err());
}

#[test]
fn tcc_installed_preset_literal_is_read_without_running_javascript() {
    let literal = b"export const defaultFanProfiles = [\n{name: 'Silent', tableCPU: [{temp: 0, speed: 0}, {temp: 100, speed: 100},], tableGPU: [{temp: 0, speed: 10}, {temp: 100, speed: 90}]},\n{name: 'Custom'},\n];\nthrow new Error('This code must never execute');\n";
    let tmp = TempDir::new().unwrap();
    let archive = tmp.path().join("app.asar");
    build_asar_archive(&archive, literal);

    let presets = read_tcc_fan_presets(&archive).unwrap();
    assert_eq!(presets["Silent"].table_gpu[0].speed, 10);
    assert!(!presets.contains_key("Custom"));

    fs::write(&archive, b"bad").unwrap();
    assert!(read_tcc_fan_presets(&archive).is_err());
}

fn sample_data() -> OfflineUserData {
    OfflineUserData {
        config: json!({
            "settings": {
                "ac": { "tuxedo_profile_id": "shared", "cpu_profile": "performance", "cpu_temp_limit": 85 },
                "type_c": { "tuxedo_profile_id": "shared", "cpu_profile": "balanced", "cpu_temp_limit": 85 },
                "bat": { "tuxedo_profile_id": "shared", "cpu_profile": "power-saver", "cpu_temp_limit": 85 }
            },
            "devices": [{
                "path": "/sys/devices/system/cpu/intel_pstate/no_turbo",
                "ac_val": "0",
                "type_c_val": "0",
                "bat_val": "1"
            }]
        }),
        keyboard_colors: json!({ "0": [1, 2, 3], "21": [4, 5, 6], "56": [7, 8, 9] }),
        lightbar: json!({ "silent_mode": true }),
        gui_settings: json!({ "theme": "system" }),
    }
}

fn sample_base() -> Value {
    json!({
        "profiles": {
            "shared": {
                "id": "shared",
                "name": "Original",
                "fan": { "fanProfile": "user-curve" },
                "odmPowerLimits": { "tdpValues": [55, 65, 75] }
            }
        },
        "stateMap": { "power_wc": "cooler" },
        "keyboardBacklightStates": [
            { "brightness": 17, "red": 10, "green": 20, "blue": 30 },
            { "brightness": 17, "red": 10, "green": 20, "blue": 30 },
            { "brightness": 17, "red": 10, "green": 20, "blue": 30 },
            { "brightness": 17, "red": 10, "green": 20, "blue": 30 }
        ]
    })
}

fn sample_caps() -> Value {
    json!({ "zones": 4, "maxBrightness": 50, "ledIds": [0, 21, 56, 66] })
}

#[test]
fn offline_user_colors_win_without_shifting_missing_leds() {
    let data = sample_data();
    let base = sample_base();
    let caps = sample_caps();

    let out = convert_offline_import(&data, &base, &caps, None).unwrap();
    let states = out.settings["customKeyboardProfiles"]["frixit-keyboard"]["json"]["states"]
        .as_array()
        .unwrap();
    let reds: Vec<i64> = states.iter().map(|s| s["red"].as_i64().unwrap()).collect();
    let brights: Vec<i64> = states
        .iter()
        .map(|s| s["brightness"].as_i64().unwrap())
        .collect();
    assert_eq!(reds, vec![1, 4, 7, 10]);
    assert_eq!(brights, vec![17, 17, 17, 17]);
    assert_eq!(out.settings["keyboardLayout"], json!("frixit-mechrevo"));
    assert_eq!(out.report["ignored"], json!(["cpu_temp_limit"]));
    assert_eq!(out.report["excludedFeatures"], json!(["gpu-run"]));
    assert!(out.pending.get("gpu_run_mode").is_none());
    assert_eq!(
        out.settings["lightbar"],
        json!({ "silent_mode": true, "managed": true })
    );
    assert!(out.pending.get("lightbar").is_none());
    assert_eq!(
        out.settings["powerDevices"]["devices"],
        data.config["devices"]
    );
    assert_eq!(
        out.settings["powerDevices"]["cpu"]["bat"]["aspm"],
        json!("powersupersave")
    );
    assert!(!serde_json::to_string(&out.settings["powerDevices"])
        .unwrap()
        .contains("cpu_temp_limit"));
}

#[test]
fn offline_three_modes_keep_fan_and_tdp_with_personal_cpu_policy() {
    let data = sample_data();
    let base = sample_base();
    let caps = sample_caps();

    let out = convert_offline_import(&data, &base, &caps, None).unwrap();
    assert_eq!(
        out.settings["stateMap"]["power_type_c"],
        json!("frixit-type_c")
    );
    assert_eq!(out.settings["stateMap"]["power_wc"], json!("cooler"));
    assert!(out.report["unresolvedProfiles"]
        .as_array()
        .unwrap()
        .is_empty());

    for (mode, gov, epp, turbo) in [
        ("ac", "performance", "performance", false),
        ("type_c", "powersave", "balance_performance", false),
        ("bat", "powersave", "power", true),
    ] {
        let p = &out.settings["profiles"][format!("frixit-{mode}")];
        assert_eq!(
            p["cpu"],
            json!({
                "governor": gov,
                "energyPerformancePreference": epp,
                "noTurbo": turbo
            })
        );
        assert_eq!(p["fan"], base["profiles"]["shared"]["fan"]);
        assert_eq!(
            p["odmPowerLimits"],
            base["profiles"]["shared"]["odmPowerLimits"]
        );
        assert!(!serde_json::to_string(p).unwrap().contains("cpu_temp_limit"));
    }
}

#[test]
fn offline_absent_tcc_profiles_are_reported_without_guessing_limits() {
    let data = sample_data();
    let caps = sample_caps();

    let out = convert_offline_import(&data, &json!({}), &caps, None).unwrap();
    assert_eq!(
        out.report["unresolvedProfiles"].as_array().unwrap().len(),
        3
    );
    assert_eq!(out.settings["profiles"], json!({}));
    assert_eq!(out.settings["stateMap"], json!({}));
}

#[test]
fn offline_desktop_preferences_preserve_minimized_and_map_system_theme() {
    let data = sample_data();
    assert_eq!(
        desktop_preferences(&data).unwrap(),
        json!({ "theme": "auto", "startMinimized": false })
    );

    let dark = OfflineUserData {
        gui_settings: json!({ "theme": "dark", "start_minimized": true }),
        ..data.clone()
    };
    assert_eq!(
        desktop_preferences(&dark).unwrap(),
        json!({ "theme": "dark", "startMinimized": true })
    );

    for invalid in [
        json!({ "theme": "unknown" }),
        json!({ "start_minimized": "yes" }),
    ] {
        let bad = OfflineUserData {
            gui_settings: invalid,
            ..data.clone()
        };
        assert!(desktop_preferences(&bad).is_err());
    }
}

#[test]
fn offline_invalid_or_missing_led_or_duplicate_led_is_rejected() {
    let data = sample_data();
    let base = sample_base();
    let caps = sample_caps();

    for bad_colors in [
        json!({ "55": [1, 2, 3] }),
        json!({ "0": [256, 0, 0] }),
        json!({ "0": [true, 0, 0] }),
    ] {
        let bad_data = OfflineUserData {
            keyboard_colors: bad_colors,
            ..data.clone()
        };
        assert!(convert_offline_import(&bad_data, &base, &caps, None).is_err());
    }

    let dup_caps = json!({ "zones": 4, "maxBrightness": 50, "ledIds": [0, 21, 21, 56] });
    assert!(convert_offline_import(&data, &base, &dup_caps, None).is_err());
}

#[test]
fn offline_bundle_writer_enforces_private_permissions_and_refuses_overwrite() {
    let tmp = TempDir::new().unwrap();
    let bundle_dir = tmp.path().join("bundle");
    let data = sample_data();
    let out = convert_offline_import(&data, &sample_base(), &sample_caps(), None).unwrap();
    let prefs = desktop_preferences(&data).unwrap();

    write_offline_import_bundle(&bundle_dir, &out, &prefs).unwrap();
    assert!(bundle_dir.join("settings").is_file());
    assert!(bundle_dir.join("report.json").is_file());
    assert!(bundle_dir.join("pending.json").is_file());
    assert!(bundle_dir.join("desktop-preferences.json").is_file());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&bundle_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(bundle_dir.join("settings"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    assert!(write_offline_import_bundle(&bundle_dir, &out, &prefs).is_err());
}

fn mechrevo_fixture() -> (Value, Value, Value, Value, Value) {
    let kbd_states: Vec<Value> = (0..126)
        .map(|_| json!({ "brightness": 26, "red": 1, "green": 2, "blue": 3 }))
        .collect();
    let settings = json!({
        "stateMap": { "power_ac": "example", "power_wc": "old" },
        "chargingProfile": "stationary",
        "keyboardBacklightStates": kbd_states
    });
    let profiles = json!([{
        "id": "example",
        "name": "Example",
        "display": { "refreshRate": null },
        "cpu": { "governor": "powersave", "useMaxPerfGov": true },
        "fan": {
            "fanProfile": "Balanced",
            "useControl": true,
            "minimumFanspeed": 0,
            "maximumFanspeed": 100,
            "offsetFanspeed": 20
        },
        "odmPowerLimits": { "tdpValues": [100, 120, 140] },
        "nvidiaPowerCTRLProfile": { "offset": 25 }
    }]);
    let saved = json!({
        "deviceUUID": "02:00:00:00:00:01",
        "red": 1,
        "green": 2,
        "blue": 3,
        "ledMode": 0,
        "ledOn": true,
        "pumpOn": true,
        "pumpVoltage": 0,
        "fanOn": true,
        "fanDutyCycle": 75
    });
    let user = json!({
        "aquarisLastDeviceName": "CoolingSystem LCT22002",
        "aquarisSaveState": saved.to_string()
    });
    let curves = json!({
        "tableCPU": [{ "temp": 60, "speed": 33 }, { "temp": 100, "speed": 90 }],
        "tableGPU": [{ "temp": 60, "speed": 33 }, { "temp": 100, "speed": 90 }]
    });
    let hardware = json!({
        "epp": "default",
        "hwpDynamicBoost": false,
        "ctgpWritable": false
    });
    (settings, profiles, user, curves, hardware)
}

#[test]
fn mechrevo_preserves_effective_controls_without_activating_gpu_writes() {
    let (settings, profiles, user, curves, hardware) = mechrevo_fixture();
    let (converted, cooler) =
        convert_mechrevo_tcc(&settings, &profiles, &user, &curves, &hardware).unwrap();
    let profile = &converted["profiles"]["example"];
    assert_eq!(
        profile["fan"]["tableCPU"],
        json!([{ "temp": 60, "speed": 53 }, { "temp": 100, "speed": 100 }])
    );
    assert_eq!(
        profile["fan"]["tablePump"],
        json!([{ "temp": 0, "speed": 3 }, { "temp": 100, "speed": 3 }])
    );
    assert_eq!(
        profile["fan"]["tableWaterCoolerFan"],
        json!([{ "temp": 0, "speed": 75 }, { "temp": 100, "speed": 75 }])
    );
    assert_eq!(profile["cpu"]["governor"], json!("performance"));
    assert_eq!(
        profile["cpu"]["energyPerformancePreference"],
        json!("default")
    );
    assert_eq!(
        profile["odmPowerLimits"]["tdpValues"],
        json!([100, 120, 140])
    );
    assert_eq!(profile["chargingProfile"], json!("stationary"));
    assert!(profile.get("nvidiaCTGPOffset").is_none());
    assert!(profile.get("nvidiaPowerCTRLProfile").is_none());
    assert_eq!(converted["stateMap"]["power_wc"], json!("example"));
    assert_eq!(
        converted["customKeyboardProfiles"]["tcc-keyboard"]["json"]["states"]
            .as_array()
            .unwrap()
            .len(),
        126
    );
    assert_eq!(cooler["address"], json!("02:00:00:00:00:01"));
    assert_eq!(cooler["green"], json!(2));
}

#[test]
fn mechrevo_preserves_disabled_cooler_outputs_and_rejects_v12_and_bad_address() {
    let (settings, profiles, mut user, curves, hardware) = mechrevo_fixture();
    let mut saved: Value =
        serde_json::from_str(user["aquarisSaveState"].as_str().unwrap()).unwrap();
    saved["pumpOn"] = json!(false);
    saved["fanOn"] = json!(false);
    saved["ledOn"] = json!(false);
    user["aquarisSaveState"] = json!(saved.to_string());

    let (converted, cooler) =
        convert_mechrevo_tcc(&settings, &profiles, &user, &curves, &hardware).unwrap();
    let fan = &converted["profiles"]["example"]["fan"];
    assert_eq!(
        fan["tablePump"],
        json!([{ "temp": 0, "speed": 0 }, { "temp": 100, "speed": 0 }])
    );
    assert_eq!(
        fan["tableWaterCoolerFan"],
        json!([{ "temp": 0, "speed": 0 }, { "temp": 100, "speed": 0 }])
    );
    assert_eq!(
        (
            cooler["red"].clone(),
            cooler["green"].clone(),
            cooler["blue"].clone()
        ),
        (json!(0), json!(0), json!(0))
    );

    // Reject V12 (`pumpVoltage == 1`).
    let mut v12_saved = saved.clone();
    v12_saved["pumpOn"] = json!(true);
    v12_saved["pumpVoltage"] = json!(1);
    let mut bad_user = user;
    bad_user["aquarisSaveState"] = json!(v12_saved.to_string());
    let err = convert_mechrevo_tcc(&settings, &profiles, &bad_user, &curves, &hardware)
        .unwrap_err()
        .to_string();
    assert!(err.contains("V12"), "got {err}");

    // Reject invalid address.
    let mut bad_addr_saved = saved;
    bad_addr_saved["deviceUUID"] = json!("not-an-address");
    bad_user["aquarisSaveState"] = json!(bad_addr_saved.to_string());
    let err = convert_mechrevo_tcc(&settings, &profiles, &bad_user, &curves, &hardware)
        .unwrap_err()
        .to_string();
    assert!(err.contains("address"), "got {err}");
}

#[test]
fn deep_16_rejects_brightness_over_u8_max_even_with_higher_max_brightness() {
    let data = sample_data();
    let settings = sample_base();
    let mut caps = sample_caps();
    caps["maxBrightness"] = json!(500);

    assert!(convert_offline_import(&data, &settings, &caps, Some(255)).is_ok());

    for invalid in [256, 300] {
        let err = convert_offline_import(&data, &settings, &caps, Some(invalid))
            .unwrap_err()
            .to_string();
        assert!(err.contains("255") || err.contains("u8"), "got {err}");
    }

    // Also verify valid bundle loads cleanly via ConfigManager::load_from_path.
    let valid_out = convert_offline_import(&data, &settings, &caps, Some(17)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let bundle_dir = dir.path().join("bundle");
    write_offline_import_bundle(&bundle_dir, &valid_out, &json!({})).unwrap();
    let loaded = ucc_config::ConfigManager::load_from_path(&bundle_dir.join("settings")).unwrap();
    assert_eq!(loaded.keyboard_backlight_states.unwrap()[0].brightness, 17);
}

#[test]
fn deep_17_preserves_physical_led_fallback_and_rejects_ambiguous_layouts() {
    let mut data = sample_data();
    let mut settings = sample_base();
    let mut caps = sample_caps();

    // 1. Full (126 zones) -> sparse ([0, 10, 20, 66]) with partial overlay on LED 10.
    let full_states: Vec<Value> = (0..126)
        .map(|led| {
            json!({
                "brightness": 40,
                "red": led,
                "green": 100 + led,
                "blue": 200,
                "mode": 0
            })
        })
        .collect();
    settings["keyboardBacklightStates"] = Value::Array(full_states);
    caps["zones"] = json!(4);
    caps["ledIds"] = json!([0, 10, 20, 66]);
    data.keyboard_colors = json!({ "10": [11, 22, 33] });

    let out = convert_offline_import(&data, &settings, &caps, Some(40)).unwrap();
    let states = out.settings["keyboardBacklightStates"].as_array().unwrap();
    assert_eq!(states.len(), 4);
    // Explicitly overlaid LED 10 (index 1).
    assert_eq!(
        (
            states[1]["red"].clone(),
            states[1]["green"].clone(),
            states[1]["blue"].clone()
        ),
        (json!(11), json!(22), json!(33))
    );
    // Unedited LED 66 (index 3) MUST take physical LED 66 from source, NOT index 3 (`DEEP-17`).
    assert_eq!(
        (
            states[3]["red"].clone(),
            states[3]["green"].clone(),
            states[3]["blue"].clone()
        ),
        (json!(66), json!(166), json!(200))
    );

    // 2. Compact (4 zones) -> compact (4 zones).
    settings["keyboardBacklightStates"] = json!([
        {"brightness": 40, "red": 1, "green": 2, "blue": 3},
        {"brightness": 40, "red": 4, "green": 5, "blue": 6},
        {"brightness": 40, "red": 7, "green": 8, "blue": 9},
        {"brightness": 40, "red": 77, "green": 88, "blue": 99}
    ]);
    let compact_out = convert_offline_import(&data, &settings, &caps, Some(40)).unwrap();
    let compact_states = compact_out.settings["keyboardBacklightStates"]
        .as_array()
        .unwrap();
    assert_eq!(compact_states[3]["red"], json!(77));

    // 3. Ambiguous 8-entry source mapped onto sparse LED 66 must error cleanly.
    settings["keyboardBacklightStates"] =
        Value::Array(vec![json!({"red": 1, "green": 2, "blue": 3}); 8]);
    let err = convert_offline_import(&data, &settings, &caps, Some(40))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Ambiguous"), "got {err}");
}

#[test]
fn deep_18_rejects_duplicate_mechrevo_profile_ids() {
    let (settings, profiles_val, user, curves, hardware) = mechrevo_fixture();
    let mut dup_profiles = profiles_val.as_array().unwrap().clone();
    let mut second = dup_profiles[0].clone();
    second["name"] = json!("Different Name Same ID");
    dup_profiles.push(second);

    let err = convert_mechrevo_tcc(
        &settings,
        &Value::Array(dup_profiles),
        &user,
        &curves,
        &hardware,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("example"), "got {err}");
}

#[test]
fn deep_19_atomic_staging_leaves_no_partial_bundle_on_validation_or_write_failure() {
    let data = sample_data();
    let settings = sample_base();
    let caps = sample_caps();
    let mut broken_out = convert_offline_import(&data, &settings, &caps, Some(50)).unwrap();
    // Corrupt brightness in settings so ConfigManager validation during staging fails after files are written.
    broken_out.settings["keyboardBacklightStates"][0]["brightness"] = json!(300);

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("atomic-bundle");
    assert!(write_offline_import_bundle(&dest, &broken_out, &json!({})).is_err());
    assert!(
        !dest.exists(),
        "failed bundle write must not leave destination directory"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        leftovers.is_empty(),
        "leftover staging files: {leftovers:?}"
    );

    // Subsequent valid retry into the exact same destination succeeds cleanly.
    let valid_out = convert_offline_import(&data, &settings, &caps, Some(50)).unwrap();
    write_offline_import_bundle(&dest, &valid_out, &json!({})).unwrap();
    assert!(dest.join("settings").is_file());
    assert!(dest.join("report.json").is_file());
}

#[test]
fn deep_20_tcc_converter_preserves_utf8_in_double_quoted_profile_names() {
    let literal = "export const defaultFanProfiles = [\n{name: \"Тихий\", tableCPU: [{temp: 0, speed: 0}, {temp: 100, speed: 100}], tableGPU: [{temp: 0, speed: 15}, {temp: 100, speed: 85}]},\n];\n";
    let tmp = TempDir::new().unwrap();
    let archive = tmp.path().join("app.asar");
    build_asar_archive(&archive, literal.as_bytes());

    let presets = read_tcc_fan_presets(&archive).unwrap();
    assert_eq!(presets["Тихий"].table_gpu[0].speed, 15);
}

#[test]
fn tcc_asar_and_mechrevo_import_edge_cases() {
    // 1. ASAR archive with Windows CRLF line endings and multiple presets
    let crlf_js = concat!(
        "export const defaultFanProfiles = [\r\n",
        "{name: 'Silent', tableCPU: [{temp: 0, speed: 0}, {temp: 100, speed: 80}], ",
        "tableGPU: [{temp: 0, speed: 5}, {temp: 100, speed: 85}]},\r\n",
        "{name: 'Turbo', tableCPU: [{temp: 0, speed: 20}, {temp: 100, speed: 100}], ",
        "tableGPU: [{temp: 0, speed: 25}, {temp: 100, speed: 100}]},\r\n",
        "];\r\n",
    );
    let tmp = TempDir::new().expect("create tempdir");
    let archive = tmp.path().join("crlf_app.asar");
    build_asar_archive(&archive, crlf_js.as_bytes());

    let presets = read_tcc_fan_presets(&archive).expect("parse CRLF ASAR presets");
    assert_eq!(presets.len(), 2);
    assert_eq!(presets["Silent"].table_cpu[1].speed, 80);
    assert_eq!(presets["Turbo"].table_gpu[0].speed, 25);

    // Malformed / truncated ASAR header must error cleanly
    let bad_archive = tmp.path().join("empty.asar");
    fs::write(&bad_archive, b"").expect("write empty asar");
    assert!(read_tcc_fan_presets(&bad_archive).is_err());

    // 2. convert_mechrevo_tcc with multiple profiles and Windows CRLF JSON payloads
    let (mut settings, profiles_val, mut user, curves, hardware) = mechrevo_fixture();
    settings["stateMap"]["power_bat"] = json!("quiet-work");
    let mut multi_profiles = profiles_val.as_array().unwrap().clone();
    multi_profiles.push(json!({
        "id": "quiet-work",
        "name": "Quiet Work",
        "display": { "refreshRate": 60 },
        "cpu": { "governor": "powersave", "useMaxPerfGov": false },
        "fan": {
            "fanProfile": "Balanced",
            "useControl": true,
            "minimumFanspeed": 0,
            "maximumFanspeed": 70,
            "offsetFanspeed": 0
        },
        "odmPowerLimits": { "tdpValues": [25, 35, 45] }
    }));
    let saved_pretty_crlf = serde_json::to_string_pretty(
        &serde_json::from_str::<Value>(user["aquarisSaveState"].as_str().unwrap()).unwrap(),
    )
    .unwrap()
    .replace('\n', "\r\n");
    user["aquarisSaveState"] = json!(saved_pretty_crlf);

    let settings_crlf: Value = serde_json::from_str(
        &serde_json::to_string_pretty(&settings)
            .unwrap()
            .replace('\n', "\r\n"),
    )
    .expect("parse CRLF settings");
    let profiles_crlf: Value = serde_json::from_str(
        &serde_json::to_string_pretty(&Value::Array(multi_profiles))
            .unwrap()
            .replace('\n', "\r\n"),
    )
    .expect("parse CRLF profiles");

    let (converted, cooler) =
        convert_mechrevo_tcc(&settings_crlf, &profiles_crlf, &user, &curves, &hardware)
            .expect("convert multi-profile CRLF mechrevo TCC");
    assert_eq!(converted["profiles"].as_object().unwrap().len(), 2);
    assert_eq!(
        converted["profiles"]["quiet-work"]["odmPowerLimits"]["tdpValues"],
        json!([25, 35, 45])
    );
    assert_eq!(converted["stateMap"]["power_bat"], json!("quiet-work"));
    assert_eq!(cooler["address"], json!("02:00:00:00:00:01"));

    // Empty / malformed inputs to convert_mechrevo_tcc
    assert!(convert_mechrevo_tcc(&json!([]), &profiles_crlf, &user, &curves, &hardware).is_err());
    assert!(convert_mechrevo_tcc(&settings_crlf, &json!({}), &user, &curves, &hardware).is_err());
    assert!(convert_mechrevo_tcc(
        &settings_crlf,
        &json!([{ "name": "Missing ID" }]),
        &user,
        &curves,
        &hardware
    )
    .is_err());
    let mut bad_user = user;
    bad_user["aquarisSaveState"] = json!("{malformed-json");
    assert!(convert_mechrevo_tcc(
        &settings_crlf,
        &profiles_crlf,
        &bad_user,
        &curves,
        &hardware
    )
    .is_err());

    // 3. convert_offline_import from CRLF Data/ JSON files with multiple profiles + malformed edge cases
    let data_dir = tmp.path().join("crlf_data");
    fs::create_dir_all(&data_dir).expect("create crlf_data dir");
    let write_crlf_json = |name: &str, val: &Value| {
        let crlf = serde_json::to_string_pretty(val)
            .unwrap()
            .replace('\n', "\r\n");
        fs::write(data_dir.join(name), crlf).expect("write crlf json");
    };
    write_crlf_json(
        "config.json",
        &json!({
            "settings": {
                "ac": { "tuxedo_profile_id": "ac-base", "cpu_profile": "performance" },
                "type_c": { "tuxedo_profile_id": "typec-base", "cpu_profile": "balanced" },
                "bat": { "tuxedo_profile_id": "bat-base", "cpu_profile": "power-saver" }
            },
            "devices": []
        }),
    );
    write_crlf_json("keyboard_colors.json", &json!({ "0": [10, 20, 30] }));
    write_crlf_json("lightbar.json", &json!({ "silent_mode": false }));
    write_crlf_json("gui_settings.json", &json!({ "theme": "dark" }));

    let crlf_data = OfflineUserData::from_dir(&data_dir).expect("load CRLF OfflineUserData");
    let multi_base_crlf: Value = serde_json::from_str(
        &serde_json::to_string_pretty(&json!({
            "profiles": {
                "ac-base": { "id": "ac-base", "name": "AC Base", "odmPowerLimits": { "tdpValues": [55, 65, 75] } },
                "typec-base": { "id": "typec-base", "name": "TypeC Base", "odmPowerLimits": { "tdpValues": [35, 45, 55] } },
                "bat-base": { "id": "bat-base", "name": "Bat Base", "odmPowerLimits": { "tdpValues": [15, 20, 25] } }
            },
            "stateMap": {},
            "keyboardBacklightStates": [
                { "brightness": 20, "red": 1, "green": 2, "blue": 3 },
                { "brightness": 20, "red": 1, "green": 2, "blue": 3 },
                { "brightness": 20, "red": 1, "green": 2, "blue": 3 },
                { "brightness": 20, "red": 1, "green": 2, "blue": 3 }
            ]
        }))
        .unwrap()
        .replace('\n', "\r\n"),
    )
    .expect("parse CRLF multi-profile base");
    let caps = sample_caps();

    let out = convert_offline_import(&crlf_data, &multi_base_crlf, &caps, Some(25))
        .expect("convert CRLF multi-profile offline import");
    assert!(out.report["unresolvedProfiles"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        out.settings["profiles"]["frixit-ac"]["odmPowerLimits"]["tdpValues"],
        json!([55, 65, 75])
    );
    assert_eq!(
        out.settings["profiles"]["frixit-type_c"]["odmPowerLimits"]["tdpValues"],
        json!([35, 45, 55])
    );
    assert_eq!(
        out.settings["profiles"]["frixit-bat"]["odmPowerLimits"]["tdpValues"],
        json!([15, 20, 25])
    );

    // Malformed / empty inputs to convert_offline_import must error cleanly
    assert!(convert_offline_import(&crlf_data, &json!([]), &caps, None).is_err());
    assert!(convert_offline_import(&crlf_data, &multi_base_crlf, &json!({}), None).is_err());
    let empty_data = OfflineUserData {
        config: json!(null),
        keyboard_colors: json!({}),
        lightbar: json!({}),
        gui_settings: json!({}),
    };
    assert!(convert_offline_import(&empty_data, &multi_base_crlf, &caps, Some(25)).is_err());
}
