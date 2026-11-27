// SPDX-License-Identifier: GPL-3.0-or-later

//! Offline configuration import preflight check (`ISSUE-04`).
//! Never opens hardware, D-Bus, or installed settings.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use ucc_config::UccSettings;
use ucc_hardware::PowerDeviceController;
use ucc_lighting::LightbarSettings;

const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;

fn read_json(path: &str) -> Result<Value, String> {
    let meta = fs::metadata(Path::new(path)).map_err(|_| format!("Cannot open input: {path}"))?;
    if meta.len() > MAX_IMPORT_BYTES {
        return Err(format!("Input exceeds size limit: {path}"));
    }
    let bytes = fs::read(path).map_err(|_| format!("Cannot open input: {path}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON in {path}: {e}"))
}

fn run_check(settings_path: &str, caps_path: &str) -> Result<Value, String> {
    let root = read_json(settings_path)?;
    let caps = read_json(caps_path)?;

    let root_str =
        serde_json::to_string(&root).map_err(|e| format!("Failed encoding settings: {e}"))?;
    let parsed_settings = UccSettings::from_json(&root_str)
        .map_err(|e| format!("UCC settings parser rejected import: {e}"))?;

    let power_devices = root
        .get("powerDevices")
        .ok_or_else(|| "Missing powerDevices section".to_string())?;
    PowerDeviceController::validate(power_devices)?;

    let lightbar = root
        .get("lightbar")
        .ok_or_else(|| "Missing lightbar section".to_string())?;
    LightbarSettings::normalize_json(lightbar).map_err(|e| e.to_string())?;

    let max_brightness = caps
        .get("maxBrightness")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "Missing maxBrightness capability".to_string())?;
    let zones = caps
        .get("zones")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing zones capability".to_string())?;
    if max_brightness <= 0 || zones == 0 || zones > 1024 {
        return Err("Invalid keyboard capabilities".to_string());
    }
    let effective_max_brightness = max_brightness.min(i64::from(u8::MAX));

    if let Some(led_ids_val) = caps.get("ledIds") {
        let led_ids = led_ids_val
            .as_array()
            .ok_or_else(|| "Invalid ledIds capability array".to_string())?;
        if led_ids.len() != zones as usize {
            return Err(format!(
                "ledIds length {} does not match keyboard zones {}",
                led_ids.len(),
                zones
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in led_ids {
            let id = item
                .as_i64()
                .ok_or_else(|| "Non-integer entry in ledIds".to_string())?;
            if id < 0 || !seen.insert(id) {
                return Err(format!("Invalid or duplicate LED ID: {id}"));
            }
        }
    }

    let validate_state_entry = |obj: &serde_json::Map<String, Value>| -> Result<(), String> {
        for ch in ["red", "green", "blue", "brightness"] {
            let v = obj
                .get(ch)
                .and_then(|n| n.as_i64())
                .ok_or_else(|| format!("Invalid keyboard channel: {ch}"))?;
            let max = if ch == "brightness" {
                effective_max_brightness
            } else {
                255
            };
            if !(0..=max).contains(&v) {
                return Err(format!("Keyboard value out of range for {ch}: {v}"));
            }
        }
        Ok(())
    };

    let states = root
        .get("keyboardBacklightStates")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Missing keyboardBacklightStates array".to_string())?;
    if states.len() != zones as usize {
        return Err("Keyboard zone count differs from hardware".to_string());
    }
    for st in states {
        let obj = st
            .as_object()
            .ok_or_else(|| "Invalid keyboard state entry".to_string())?;
        validate_state_entry(obj)?;
    }

    if let Some(custom_kbd) = parsed_settings.custom_keyboard_profiles.as_ref() {
        for (id, entry) in custom_kbd {
            if let Some(arr) = entry.json.get("states").and_then(|v| v.as_array()) {
                for st in arr {
                    let obj = st.as_object().ok_or_else(|| {
                        format!("Invalid state entry in customKeyboardProfiles.{id}")
                    })?;
                    validate_state_entry(obj)?;
                }
            }
        }
    }

    fn parse_rounded_curve_coord(v: &Value) -> Option<i64> {
        v.as_i64().or_else(|| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.round() as i64)
        })
    }

    if let Some(custom_fans) = parsed_settings.custom_fan_profiles.as_ref() {
        for (id, fan_entry) in custom_fans {
            if let Some(obj) = fan_entry.json.as_object() {
                for curve_key in ["tableCPU", "tableGPU", "tablePump", "tableWaterCoolerFan"] {
                    if let Some(points) = obj.get(curve_key).and_then(|v| v.as_array()) {
                        for pt in points {
                            let temp = pt
                                .get("temp")
                                .and_then(parse_rounded_curve_coord)
                                .ok_or_else(|| {
                                    format!("Invalid temp in customFanProfiles.{id}.{curve_key}")
                                })?;
                            let speed = pt
                                .get("speed")
                                .and_then(parse_rounded_curve_coord)
                                .ok_or_else(|| {
                                    format!("Invalid speed in customFanProfiles.{id}.{curve_key}")
                                })?;
                            if !(0..=150).contains(&temp) || !(0..=100).contains(&speed) {
                                return Err(format!(
                                    "Out-of-range fan point ({temp}, {speed}) in customFanProfiles.{id}.{curve_key}"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    for (id, prof) in &parsed_settings.profiles {
        if prof.id != *id {
            return Err("Profile identity mismatch in profiles map".to_string());
        }
        prof.validate().map_err(|e| e.to_string())?;
        if let Some(ref kb_id) = prof.selected_keyboard_profile {
            let kb_trimmed = kb_id.trim();
            if !kb_trimmed.is_empty() && !kb_trimmed.starts_with("__") {
                if let Some(custom_kbd) = parsed_settings.custom_keyboard_profiles.as_ref() {
                    if !custom_kbd.contains_key(kb_trimmed) {
                        return Err(format!(
                            "Profile {id} references unknown selectedKeyboardProfile: {kb_trimmed}"
                        ));
                    }
                }
            }
        }
    }

    let mut validated_modes = 0usize;
    for mode in ["ac", "type_c", "bat"] {
        let key = format!("power_{mode}");
        let id = parsed_settings
            .state_map
            .get(&key)
            .ok_or_else(|| format!("Missing stateMap entry: {key}"))?;
        let prof = parsed_settings
            .profiles
            .get(id)
            .ok_or_else(|| format!("stateMap points to missing profile: {id}"))?;
        if prof.id != *id {
            return Err("Profile identity changed during UCC parsing".to_string());
        }
        prof.validate().map_err(|e| e.to_string())?;
        validated_modes += 1;
    }

    let devices_len = power_devices
        .get("devices")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    Ok(json!({
        "ok": true,
        "profiles": validated_modes,
        "keyboardZones": zones,
        "devices": devices_len,
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: ucc-import-check SETTINGS CAPABILITIES");
        std::process::exit(2);
    }
    match run_check(&args[1], &args[2]) {
        Ok(summary) => {
            println!("{summary}");
        }
        Err(err) => {
            eprintln!("Import rejected: {err}");
            std::process::exit(1);
        }
    }
}
