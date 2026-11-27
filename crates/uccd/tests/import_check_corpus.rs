// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus validation suite for `ucc-import-check` (`M8.6`).

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use ucc_config::{
    convert_offline_import, merge_tcc, write_offline_import_bundle, OfflineUserData, TccFanPoint,
    TccFanPreset,
};

fn run_import_check(settings_path: &Path, caps_path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ucc-import-check"))
        .arg(settings_path)
        .arg(caps_path)
        .output()
        .expect("failed to execute ucc-import-check")
}

fn baseline_ucc_v03_settings() -> Value {
    let states: Vec<Value> = [1_u8, 21, 56]
        .into_iter()
        .map(|r| json!({ "red": r, "green": 20, "blue": 30, "brightness": 17 }))
        .collect();
    let mut profiles = serde_json::Map::new();
    for mode in ["ac", "type_c", "bat"] {
        let id = format!("frixit-{mode}");
        profiles.insert(
            id.clone(),
            json!({
                "id": id,
                "name": format!("Тихий профиль ({mode})"),
                "selectedKeyboardProfile": "frixit-keyboard",
                "cpu": { "governor": "powersave", "noTurbo": mode == "bat", "onlineCores": 8 },
                "odmPowerLimits": { "tdpValues": [10, 20, 30] },
                "fan": {
                    "tableCPU": [{ "temp": 0, "speed": 20 }, { "temp": 100, "speed": 100 }],
                    "tableGPU": [{ "temp": 0, "speed": 20 }, { "temp": 100, "speed": 100 }]
                }
            }),
        );
    }
    json!({
        "keyboardLayout": "frixit-mechrevo",
        "keyboardBacklightStates": states,
        "customKeyboardProfiles": {
            "frixit-keyboard": {
                "id": "frixit-keyboard",
                "name": "Personal",
                "json": { "states": states }
            }
        },
        "customFanProfiles": {
            "custom-fan": {
                "id": "custom-fan",
                "name": "Custom Fan",
                "json": {
                    "tableCPU": [{ "temp": 0, "speed": 25 }, { "temp": 100, "speed": 90 }],
                    "tableGPU": [{ "temp": 0, "speed": 25 }, { "temp": 100, "speed": 90 }]
                }
            }
        },
        "powerDevices": {
            "enabled": true,
            "devices": [{ "type": "sys", "name": "No Turbo", "path": "/sys/devices/system/cpu/intel_pstate/no_turbo", "ac_val": "0", "type_c_val": "0", "bat_val": "1" }]
        },
        "lightbar": { "managed": true },
        "profiles": Value::Object(profiles),
        "stateMap": {
            "power_ac": "frixit-ac",
            "power_type_c": "frixit-type_c",
            "power_bat": "frixit-bat"
        }
    })
}

#[test]
fn m8_6_corpus_accepts_ucc_tcc_merged_and_offline_imported_configs() {
    let dir = TempDir::new().unwrap();
    let caps_path = dir.path().join("caps.json");
    std::fs::write(
        &caps_path,
        serde_json::to_string(&json!({
            "zones": 3,
            "maxBrightness": 50,
            "ledIds": [0, 21, 56]
        }))
        .unwrap(),
    )
    .unwrap();

    // 1. Legacy UCC v0.3 settings with UTF-8 profile names, custom fan curves, and safe device path.
    let s1 = dir.path().join("ucc_v03.json");
    std::fs::write(
        &s1,
        serde_json::to_string(&baseline_ucc_v03_settings()).unwrap(),
    )
    .unwrap();
    let out1 = run_import_check(&s1, &caps_path);
    assert!(
        out1.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let parsed1: Value = serde_json::from_slice(&out1.stdout).unwrap();
    assert_eq!(
        parsed1,
        json!({ "ok": true, "profiles": 3, "keyboardZones": 3, "devices": 1 })
    );

    // 2. TCC-merged settings validated via ucc-import-check.
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
    let mut base_for_tcc = baseline_ucc_v03_settings();
    let (mut merged, _) = merge_tcc(
        &base_for_tcc,
        &json!({ "chargingProfile": "stationary" }),
        &json!([{
            "id": "frixit-ac",
            "name": "TCC Imported",
            "cpu": { "onlineCores": 4, "scalingMaxFrequency": 3200000 },
            "display": { "useRefRate": false, "refreshRate": 60 },
            "fan": { "useControl": true, "fanProfile": "Silent" },
            "odmPowerLimits": { "tdpValues": [15, 25, 35] }
        }]),
        &presets,
    )
    .unwrap();
    // Preserve stateMap & keyboard layout from base.
    base_for_tcc["profiles"] = merged["profiles"].take();
    let s2 = dir.path().join("tcc_merged.json");
    std::fs::write(&s2, serde_json::to_string(&base_for_tcc).unwrap()).unwrap();
    let out2 = run_import_check(&s2, &caps_path);
    assert!(
        out2.status.success(),
        "TCC merged failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );

    // 3. Offline importer bundle settings validated via ucc-import-check.
    let offline_caps = json!({ "zones": 4, "maxBrightness": 50, "ledIds": [0, 21, 56, 66] });
    let offline_caps_path = dir.path().join("offline_caps.json");
    std::fs::write(
        &offline_caps_path,
        serde_json::to_string(&offline_caps).unwrap(),
    )
    .unwrap();
    let data = OfflineUserData {
        config: json!({
            "settings": {
                "ac": { "tuxedo_profile_id": "shared", "cpu_profile": "performance", "cpu_temp_limit": 85 },
                "type_c": { "tuxedo_profile_id": "shared", "cpu_profile": "balanced", "cpu_temp_limit": 85 },
                "bat": { "tuxedo_profile_id": "shared", "cpu_profile": "power-saver", "cpu_temp_limit": 85 }
            },
            "devices": []
        }),
        keyboard_colors: json!({ "0": [1, 2, 3], "21": [4, 5, 6], "56": [7, 8, 9] }),
        lightbar: json!({ "silent_mode": false }),
        gui_settings: json!({ "theme": "system" }),
    };
    let offline_base = json!({
        "profiles": {
            "shared": {
                "id": "shared",
                "name": "Тихий",
                "fan": { "fanProfile": "user-curve" },
                "odmPowerLimits": { "tdpValues": [35, 45, 55] }
            }
        },
        "stateMap": { "power_wc": "shared" }
    });
    let converted = convert_offline_import(&data, &offline_base, &offline_caps, Some(25)).unwrap();
    let bundle_dir = dir.path().join("bundle");
    write_offline_import_bundle(&bundle_dir, &converted, &json!({})).unwrap();
    let out3 = run_import_check(&bundle_dir.join("settings"), &offline_caps_path);
    assert!(
        out3.status.success(),
        "Offline bundle failed: {}",
        String::from_utf8_lossy(&out3.stderr)
    );
}

#[test]
fn m8_6_corpus_rejects_corrupt_brightness_curves_leds_and_unsafe_devices() {
    let dir = TempDir::new().unwrap();
    let caps_path = dir.path().join("caps.json");
    std::fs::write(
        &caps_path,
        serde_json::to_string(&json!({
            "zones": 3,
            "maxBrightness": 500,
            "ledIds": [0, 21, 56]
        }))
        .unwrap(),
    )
    .unwrap();

    // 1. Corrupt JSON syntax.
    let corrupt_path = dir.path().join("corrupt.json");
    std::fs::write(&corrupt_path, "{ not valid json").unwrap();
    assert!(!run_import_check(&corrupt_path, &caps_path).status.success());

    // 2. Brightness > 255 rejected even when capabilities.maxBrightness == 500.
    let mut bad_bright = baseline_ucc_v03_settings();
    bad_bright["keyboardBacklightStates"][0]["brightness"] = json!(300);
    let p_bright = dir.path().join("bad_bright.json");
    std::fs::write(&p_bright, serde_json::to_string(&bad_bright).unwrap()).unwrap();
    assert!(!run_import_check(&p_bright, &caps_path).status.success());

    // 3. Invalid fan curve speed (> 100) inside customFanProfiles.
    let mut bad_curve = baseline_ucc_v03_settings();
    bad_curve["customFanProfiles"]["custom-fan"]["json"]["tableCPU"][0]["speed"] = json!(101);
    let p_curve = dir.path().join("bad_curve.json");
    std::fs::write(&p_curve, serde_json::to_string(&bad_curve).unwrap()).unwrap();
    assert!(!run_import_check(&p_curve, &caps_path).status.success());

    // 4. Disallowed device path (/etc/shadow).
    let mut bad_dev = baseline_ucc_v03_settings();
    bad_dev["powerDevices"]["devices"] = json!([{ "path": "/etc/shadow" }]);
    let p_dev = dir.path().join("bad_dev.json");
    std::fs::write(&p_dev, serde_json::to_string(&bad_dev).unwrap()).unwrap();
    assert!(!run_import_check(&p_dev, &caps_path).status.success());

    // 5. Duplicate LED IDs in capabilities rejected.
    let valid_path = dir.path().join("valid.json");
    std::fs::write(
        &valid_path,
        serde_json::to_string(&baseline_ucc_v03_settings()).unwrap(),
    )
    .unwrap();
    let bad_caps_path = dir.path().join("bad_caps.json");
    std::fs::write(
        &bad_caps_path,
        serde_json::to_string(&json!({
            "zones": 3,
            "maxBrightness": 50,
            "ledIds": [0, 21, 21]
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(!run_import_check(&valid_path, &bad_caps_path)
        .status
        .success());

    // 6. Missing stateMap mode (power_type_c).
    let mut missing_mode = baseline_ucc_v03_settings();
    missing_mode["stateMap"]
        .as_object_mut()
        .unwrap()
        .remove("power_type_c");
    let p_mode = dir.path().join("missing_mode.json");
    std::fs::write(&p_mode, serde_json::to_string(&missing_mode).unwrap()).unwrap();
    assert!(!run_import_check(&p_mode, &caps_path).status.success());
}
