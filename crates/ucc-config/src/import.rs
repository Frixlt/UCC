//! Read-only offline importer for legacy UCC, TCC, and user `Data/` settings.
//!
//! No JavaScript is ever executed and no live system paths are modified here.
//! Fan selection and modifier semantics follow TUXEDO Control Center
//! `FanControlBaseClass`, `FanControlLogic`, and `FanUtils` (GPL-3.0-or-later).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::ConfigError;

const ASAR_TARGET_PATH: [&str; 6] = [
    "dist",
    "tuxedo-control-center",
    "service-app",
    "common",
    "models",
    "TccFanTable.js",
];

const MAX_ASAR_JSON_SIZE: u64 = 8 * 1024 * 1024;
const MAX_ASAR_ENTRY_SIZE: u64 = 2 * 1024 * 1024;

/// A single temperature/speed point in a TCC fan curve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TccFanPoint {
    pub temp: i64,
    pub speed: i64,
}

/// CPU and GPU fan curves parsed from a TCC preset definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TccFanPreset {
    #[serde(rename = "tableCPU")]
    pub table_cpu: Vec<TccFanPoint>,
    #[serde(rename = "tableGPU")]
    pub table_gpu: Vec<TccFanPoint>,
}

/// Offline export bundle read from a user's `Data/` directory.
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineUserData {
    pub config: Value,
    pub keyboard_colors: Value,
    pub lightbar: Value,
    pub gui_settings: Value,
}

impl OfflineUserData {
    /// Reads `config.json`, `keyboard_colors.json`, `lightbar.json`, and
    /// `gui_settings.json` from `dir`.
    pub fn from_dir(dir: &Path) -> Result<Self, ConfigError> {
        Ok(Self {
            config: read_json_file(&dir.join("config.json"))?,
            keyboard_colors: read_json_file(&dir.join("keyboard_colors.json"))?,
            lightbar: read_json_file(&dir.join("lightbar.json"))?,
            gui_settings: read_json_file(&dir.join("gui_settings.json"))?,
        })
    }
}

/// Complete output of an offline `Data/` + TCC/UCC conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineImportOutput {
    pub settings: Value,
    pub report: Value,
    pub pending: Value,
}

fn read_json_file(path: &Path) -> Result<Value, ConfigError> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn require_strict_int(value: &Value, message: &str) -> Result<i64, ConfigError> {
    match value {
        Value::Number(num) => num
            .as_i64()
            .ok_or_else(|| ConfigError::Validation(message.to_string())),
        _ => Err(ConfigError::Validation(message.to_string())),
    }
}

fn require_object<'a>(
    value: &'a Value,
    message: &str,
) -> Result<&'a Map<String, Value>, ConfigError> {
    value
        .as_object()
        .ok_or_else(|| ConfigError::Validation(message.to_string()))
}

fn require_array<'a>(value: &'a Value, message: &str) -> Result<&'a Vec<Value>, ConfigError> {
    value
        .as_array()
        .ok_or_else(|| ConfigError::Validation(message.to_string()))
}

/// Validates JSON fan curve points (strict integers, monotonic temp `0..=120`, speed `0..=100`).
pub fn validate_tcc_fan_points(points: &[Value]) -> Result<Vec<TccFanPoint>, ConfigError> {
    if !(2..=512).contains(&points.len()) {
        return Err(ConfigError::Validation(
            "Invalid TCC fan table size".to_string(),
        ));
    }
    let mut previous = -1_i64;
    let mut validated = Vec::with_capacity(points.len());
    for point in points {
        let obj = require_object(point, "Invalid TCC fan point")?;
        let temp_val = obj
            .get("temp")
            .ok_or_else(|| ConfigError::Validation("Missing TCC fan temp".to_string()))?;
        let speed_val = obj
            .get("speed")
            .ok_or_else(|| ConfigError::Validation("Missing TCC fan speed".to_string()))?;
        let temp =
            require_strict_int(temp_val, "Invalid TCC fan temperature/speed or point order")?;
        let speed = require_strict_int(
            speed_val,
            "Invalid TCC fan temperature/speed or point order",
        )?;
        if !(previous < temp && temp <= 120) || !(0..=100).contains(&speed) {
            return Err(ConfigError::Validation(
                "Invalid TCC fan temperature/speed or point order".to_string(),
            ));
        }
        previous = temp;
        validated.push(TccFanPoint { temp, speed });
    }
    Ok(validated)
}

/// Validates a typed slice of [`TccFanPoint`] items.
pub fn validate_fan_points_slice(points: &[TccFanPoint]) -> Result<(), ConfigError> {
    if !(2..=512).contains(&points.len()) {
        return Err(ConfigError::Validation(
            "Invalid TCC fan table size".to_string(),
        ));
    }
    let mut previous = -1_i64;
    for point in points {
        if !(previous < point.temp && point.temp <= 120) || !(0..=100).contains(&point.speed) {
            return Err(ConfigError::Validation(
                "Invalid TCC fan temperature/speed or point order".to_string(),
            ));
        }
        previous = point.temp;
    }
    Ok(())
}

/// Bakes TCC's half-up interpolated fan table with offset and clamp at every degree `0..=100`.
pub fn effective_tcc_fan_table(
    points: &[TccFanPoint],
    minimum: i64,
    maximum: i64,
    offset: i64,
) -> Result<Vec<TccFanPoint>, ConfigError> {
    validate_fan_points_slice(points)?;
    if !(0..=100).contains(&minimum)
        || !(0..=100).contains(&maximum)
        || minimum > maximum
        || !(-100..=100).contains(&offset)
    {
        return Err(ConfigError::Validation(
            "Invalid TCC fan modifier".to_string(),
        ));
    }
    let mut result = Vec::with_capacity(101);
    for temp in 0..=100_i64 {
        let speed = if temp <= points[0].temp {
            points[0].speed
        } else if temp >= points[points.len() - 1].temp {
            points[points.len() - 1].speed
        } else {
            let high = points
                .iter()
                .position(|p| p.temp >= temp)
                .expect("point exists below points.last()");
            let a = &points[high - 1];
            let b = &points[high];
            let interpolated = a.speed as f64
                + (b.speed - a.speed) as f64 * (temp - a.temp) as f64 / (b.temp - a.temp) as f64;
            (interpolated + 0.5).floor() as i64
        };
        let clamped = (speed + offset).min(maximum).max(minimum).clamp(0, 100);
        result.push(TccFanPoint {
            temp,
            speed: clamped,
        });
    }
    Ok(result)
}

fn extract_export_const_block<'a>(
    source: &'a str,
    ident: &str,
    open_delim: char,
    close_line: &str,
) -> Option<&'a str> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(rel) = source[i..].find("export") {
            let start = i + rel;
            let before_ok = start == 0 || !is_ident_char(bytes[start - 1]);
            let mut pos = start + "export".len();
            if before_ok && pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos = skip_ws(bytes, pos);
                if source[pos..].starts_with("const") {
                    let after_const = pos + "const".len();
                    if after_const < bytes.len() && bytes[after_const].is_ascii_whitespace() {
                        pos = skip_ws(bytes, after_const);
                        if source[pos..].starts_with(ident) {
                            let after_ident = pos + ident.len();
                            if after_ident < bytes.len()
                                && (bytes[after_ident].is_ascii_whitespace()
                                    || bytes[after_ident] == b'=')
                            {
                                pos = skip_ws(bytes, after_ident);
                                if pos < bytes.len() && bytes[pos] == b'=' {
                                    pos = skip_ws(bytes, pos + 1);
                                    if pos < bytes.len() && bytes[pos] == open_delim as u8 {
                                        if let Some(end_rel) = source[pos..].find(close_line) {
                                            // Keep `[... \n]` or `{... \n}` without trailing `;`.
                                            let end = pos + end_rel + close_line.len() - 1;
                                            return Some(&source[pos..end]);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            i = start + "export".len();
        } else {
            break;
        }
    }
    None
}

fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn normalize_js_literal(raw: &str, allowed_keys: &[&str], convert_single_quotes: bool) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len() + 64);
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i];
        // Preserve already double-quoted JSON strings verbatim (preserving full UTF-8 chars).
        if ch == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    let esc_len = raw[i..].chars().next().map_or(1, char::len_utf8);
                    i += esc_len;
                    continue;
                }
                if c == b'"' {
                    i += 1;
                    break;
                }
                i += raw[i..].chars().next().map_or(1, char::len_utf8);
            }
            out.push_str(&raw[start..i]);
            continue;
        }

        // Convert `'Alnum _-'` single-quoted literals when enabled.
        if convert_single_quotes && ch == b'\'' {
            let mut j = i + 1;
            let mut valid = true;
            while j < bytes.len() && bytes[j] != b'\'' {
                let c = bytes[j];
                if !(c.is_ascii_alphanumeric() || c == b' ' || c == b'_' || c == b'-') {
                    valid = false;
                    break;
                }
                j += 1;
            }
            if valid && j < bytes.len() && j > i + 1 && bytes[j] == b'\'' {
                out.push('"');
                out.push_str(&raw[i + 1..j]);
                out.push('"');
                i = j + 1;
                continue;
            }
        }

        // Quote known JS object keys (`key:`).
        let boundary_before = i == 0 || !is_ident_char(bytes[i - 1]);
        if boundary_before {
            let mut matched_key: Option<&str> = None;
            for &key in allowed_keys {
                if raw[i..].starts_with(key) {
                    let after = i + key.len();
                    let boundary_after = after < bytes.len() && !is_ident_char(bytes[after]);
                    if boundary_after {
                        let colon_pos = skip_ws(bytes, after);
                        if colon_pos < bytes.len() && bytes[colon_pos] == b':' {
                            matched_key = Some(key);
                            i = colon_pos + 1;
                            out.push('"');
                            out.push_str(key);
                            out.push_str("\":");
                            break;
                        }
                    }
                }
            }
            if matched_key.is_some() {
                continue;
            }
        }

        // Strip trailing commas before `}` or `]`.
        if ch == b',' {
            let after_ws = skip_ws(bytes, i + 1);
            if after_ws < bytes.len() && (bytes[after_ws] == b'}' || bytes[after_ws] == b']') {
                i += 1;
                continue;
            }
        }

        let rest = &raw[i..];
        let next_char = rest.chars().next().expect("valid utf-8 boundary");
        out.push(next_char);
        i += next_char.len_utf8();
    }

    out
}

/// Reads named TCC fan presets from an ASAR package without executing any JavaScript.
pub fn read_tcc_fan_presets(archive: &Path) -> Result<BTreeMap<String, TccFanPreset>, ConfigError> {
    let mut file = File::open(archive)?;
    let mut prefix = [0_u8; 16];
    let n = file.read(&mut prefix)?;
    if n != 16 {
        return Err(ConfigError::Validation("Truncated TCC archive".to_string()));
    }

    let size = u32::from_le_bytes(prefix[0..4].try_into().unwrap()) as u64;
    let header_size = u32::from_le_bytes(prefix[4..8].try_into().unwrap()) as u64;
    let _unk = u32::from_le_bytes(prefix[8..12].try_into().unwrap()) as u64;
    let json_size = u32::from_le_bytes(prefix[12..16].try_into().unwrap()) as u64;

    if size != 4
        || json_size == 0
        || json_size > MAX_ASAR_JSON_SIZE
        || header_size < json_size + 8
        || header_size > json_size + 32
    {
        return Err(ConfigError::Validation(
            "Invalid TCC archive header".to_string(),
        ));
    }

    let mut header_buf = vec![0_u8; json_size as usize];
    file.read_exact(&mut header_buf)
        .map_err(|_| ConfigError::Validation("Truncated TCC archive".to_string()))?;
    let mut node: Value = serde_json::from_slice(&header_buf)?;

    for part in ASAR_TARGET_PATH {
        node = node
            .get("files")
            .and_then(|files| files.get(part))
            .cloned()
            .ok_or_else(|| {
                ConfigError::Validation("Missing TccFanTable.js in TCC archive".to_string())
            })?;
    }

    let unpacked = node.get("unpacked").is_some_and(|v| match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_i64() != Some(0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    });
    let length_val = node
        .get("size")
        .ok_or_else(|| ConfigError::Validation("Invalid TCC fan preset entry".to_string()))?;
    let length = require_strict_int(length_val, "Invalid TCC fan preset entry")?;
    let offset = match node.get("offset") {
        Some(Value::Number(num)) => num
            .as_i64()
            .ok_or_else(|| ConfigError::Validation("Invalid TCC fan preset entry".to_string()))?,
        Some(Value::String(s)) => s
            .parse::<i64>()
            .map_err(|_| ConfigError::Validation("Invalid TCC fan preset entry".to_string()))?,
        _ => {
            return Err(ConfigError::Validation(
                "Invalid TCC fan preset entry".to_string(),
            ));
        }
    };

    if unpacked || length <= 0 || (length as u64) > MAX_ASAR_ENTRY_SIZE || offset < 0 {
        return Err(ConfigError::Validation(
            "Invalid TCC fan preset entry".to_string(),
        ));
    }

    let seek_pos = 8_u64
        .checked_add(header_size)
        .and_then(|v| v.checked_add(offset as u64))
        .ok_or_else(|| ConfigError::Validation("Invalid TCC fan preset offset".to_string()))?;
    file.seek(SeekFrom::Start(seek_pos))?;

    let mut payload = vec![0_u8; length as usize];
    file.read_exact(&mut payload)
        .map_err(|_| ConfigError::Validation("Truncated TCC fan presets".to_string()))?;
    let source = String::from_utf8(payload)
        .map_err(|_| ConfigError::Validation("Invalid UTF-8 in TCC fan presets".to_string()))?;

    let default_block = extract_export_const_block(&source, "defaultFanProfiles", '[', "\n];")
        .ok_or_else(|| ConfigError::Validation("Unsupported TCC fan preset format".to_string()))?;
    let normalized = normalize_js_literal(
        default_block,
        &["name", "tableCPU", "tableGPU", "temp", "speed"],
        true,
    );
    let parsed: Vec<Value> = serde_json::from_str(&normalized)?;

    let mut presets = BTreeMap::new();
    for entry in parsed {
        let obj = require_object(&entry, "Invalid TCC fan preset object")?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ConfigError::Validation("Invalid or duplicate TCC fan preset name".to_string())
            })?
            .to_string();
        if presets.contains_key(&name) {
            return Err(ConfigError::Validation(
                "Invalid or duplicate TCC fan preset name".to_string(),
            ));
        }
        if name == "Custom" && obj.len() == 1 {
            continue;
        }
        let table_cpu = validate_tcc_fan_points(require_array(
            obj.get("tableCPU")
                .ok_or_else(|| ConfigError::Validation("Missing tableCPU".to_string()))?,
            "Invalid TCC fan table size",
        )?)?;
        let table_gpu = validate_tcc_fan_points(require_array(
            obj.get("tableGPU")
                .ok_or_else(|| ConfigError::Validation("Missing tableGPU".to_string()))?,
            "Invalid TCC fan table size",
        )?)?;
        presets.insert(
            name,
            TccFanPreset {
                table_cpu,
                table_gpu,
            },
        );
    }

    if let Some(custom_block) = extract_export_const_block(&source, "customFanPreset", '{', "\n};")
    {
        let normalized_custom = normalize_js_literal(
            custom_block,
            &["tableCPU", "tableGPU", "temp", "speed"],
            false,
        );
        let entry: Value = serde_json::from_str(&normalized_custom)?;
        let obj = require_object(&entry, "Invalid custom TCC fan preset")?;
        let table_cpu = validate_tcc_fan_points(require_array(
            obj.get("tableCPU")
                .ok_or_else(|| ConfigError::Validation("Missing tableCPU".to_string()))?,
            "Invalid TCC fan table size",
        )?)?;
        let table_gpu = validate_tcc_fan_points(require_array(
            obj.get("tableGPU")
                .ok_or_else(|| ConfigError::Validation("Missing tableGPU".to_string()))?,
            "Invalid TCC fan table size",
        )?)?;
        presets.insert(
            "Custom".to_string(),
            TccFanPreset {
                table_cpu,
                table_gpu,
            },
        );
    }

    if presets.is_empty() {
        return Err(ConfigError::Validation(
            "No TCC fan presets found".to_string(),
        ));
    }
    Ok(presets)
}

fn preset_from_value(value: &Value) -> Result<TccFanPreset, ConfigError> {
    let obj = require_object(value, "Invalid customFanCurve container")?;
    let table_cpu = validate_tcc_fan_points(require_array(
        obj.get("tableCPU")
            .ok_or_else(|| ConfigError::Validation("Missing tableCPU".to_string()))?,
        "Invalid TCC fan table size",
    )?)?;
    let table_gpu = validate_tcc_fan_points(require_array(
        obj.get("tableGPU")
            .ok_or_else(|| ConfigError::Validation("Missing tableGPU".to_string()))?,
        "Invalid TCC fan table size",
    )?)?;
    Ok(TccFanPreset {
        table_cpu,
        table_gpu,
    })
}

/// Merges legacy TCC settings and profiles into baseline UCC settings, baking curves.
pub fn merge_tcc(
    settings: &Value,
    tcc_settings: &Value,
    profiles: &Value,
    presets: &BTreeMap<String, TccFanPreset>,
) -> Result<(Value, Value), ConfigError> {
    let settings_obj = require_object(settings, "Invalid TCC settings/profile container")?;
    let tcc_obj = require_object(tcc_settings, "Invalid TCC settings/profile container")?;
    let profiles_arr = require_array(profiles, "Invalid TCC settings/profile container")?;

    let mut result_map = tcc_obj.clone();
    for (k, v) in settings_obj {
        result_map.insert(k.clone(), v.clone());
    }
    let kbd_supported = settings_obj
        .get("keyboardBacklightControlSupported")
        .cloned()
        .or_else(|| tcc_obj.get("keyboardBacklightControlEnabled").cloned())
        .unwrap_or(Value::Bool(true));
    result_map.insert(
        "keyboardBacklightControlSupported".to_string(),
        kbd_supported,
    );

    let mut merged_profiles = Map::new();
    let mut custom_fans = Map::new();
    let mut report_profiles = Map::new();

    for original in profiles_arr {
        let mut profile = require_object(original, "Invalid TCC profile object")?.clone();
        let identifier = profile
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ConfigError::Validation("Invalid or duplicate TCC profile ID".to_string())
            })?
            .to_string();
        if merged_profiles.contains_key(&identifier) {
            return Err(ConfigError::Validation(
                "Invalid or duplicate TCC profile ID".to_string(),
            ));
        }

        let fan = require_object(
            profile.get("fan").ok_or_else(|| {
                ConfigError::Validation("Missing fan section in TCC profile".to_string())
            })?,
            "Invalid fan section in TCC profile",
        )?;
        let name = fan
            .get("fanProfile")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ConfigError::Validation("Missing fanProfile in TCC profile".to_string())
            })?
            .to_string();
        let custom = name == "Custom";
        let source_preset = if custom {
            if let Some(curve_val) = fan.get("customFanCurve") {
                Some(preset_from_value(curve_val)?)
            } else {
                presets.get("Custom").cloned()
            }
        } else {
            presets.get(&name).cloned()
        };
        let source = source_preset.ok_or_else(|| {
            ConfigError::Validation(format!(
                "TCC fan preset {name:?} is unavailable; provide --tcc-archive"
            ))
        })?;

        let (min_speed, max_speed, offset_speed) = if custom {
            (0_i64, 100_i64, 0_i64)
        } else {
            let min_v = fan
                .get("minimumFanspeed")
                .map(|v| require_strict_int(v, "Invalid TCC fan modifier"))
                .transpose()?
                .unwrap_or(0);
            let max_v = fan
                .get("maximumFanspeed")
                .map(|v| require_strict_int(v, "Invalid TCC fan modifier"))
                .transpose()?
                .unwrap_or(100);
            let off_v = fan
                .get("offsetFanspeed")
                .map(|v| require_strict_int(v, "Invalid TCC fan modifier"))
                .transpose()?
                .unwrap_or(0);
            (min_v, max_v, off_v)
        };

        let fan_id = format!("frixit-tcc-{identifier}");
        let table_cpu =
            effective_tcc_fan_table(&source.table_cpu, min_speed, max_speed, offset_speed)?;
        let table_gpu =
            effective_tcc_fan_table(&source.table_gpu, min_speed, max_speed, offset_speed)?;
        let table_cpu_val = serde_json::to_value(&table_cpu)?;
        let table_gpu_val = serde_json::to_value(&table_gpu)?;

        let mut new_fan = Map::new();
        for key in [
            "useControl",
            "sameSpeed",
            "autoControlWC",
            "enableWaterCooler",
            "manualFanSpeed",
            "manualPumpVoltage",
            "tablePump",
            "tableWaterCoolerFan",
        ] {
            if let Some(v) = fan.get(key) {
                new_fan.insert(key.to_string(), v.clone());
            }
        }
        new_fan.insert("fanProfile".to_string(), Value::String(fan_id.clone()));
        new_fan.insert("tableCPU".to_string(), table_cpu_val.clone());
        new_fan.insert("tableGPU".to_string(), table_gpu_val.clone());
        new_fan
            .entry("sameSpeed".to_string())
            .or_insert(Value::Bool(true));
        new_fan
            .entry("autoControlWC".to_string())
            .or_insert(Value::Bool(false));
        new_fan
            .entry("enableWaterCooler".to_string())
            .or_insert(Value::Bool(false));
        profile.insert("fan".to_string(), Value::Object(new_fan));

        let profile_display_name = profile
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&identifier)
            .to_string();
        custom_fans.insert(
            fan_id.clone(),
            json!({
                "id": fan_id,
                "name": format!("{profile_display_name} (TCC {name})"),
                "json": {
                    "tableCPU": table_cpu_val,
                    "tableGPU": table_gpu_val,
                }
            }),
        );

        if let Some(Value::Object(display)) = profile.get_mut("display") {
            match display.get("refreshRate") {
                None | Some(Value::Null) => {
                    display.insert("refreshRate".to_string(), json!(-1));
                }
                Some(Value::Number(num)) if num.as_i64().is_none() => {
                    if let Some(rate) = num.as_f64() {
                        display.insert(
                            "refreshRate".to_string(),
                            json!((rate + 0.5).floor() as i64),
                        );
                    }
                }
                _ => {}
            }
        }

        if let Some(Value::Object(nvidia_obj)) = profile.remove("nvidiaPowerCTRLProfile") {
            if let Some(offset_val) = nvidia_obj.get("cTGPOffset").filter(|v| !v.is_null()) {
                let ctgp = require_strict_int(offset_val, "Invalid TCC cTGP offset")?;
                if !(0..=1000).contains(&ctgp) {
                    return Err(ConfigError::Validation(
                        "Invalid TCC cTGP offset".to_string(),
                    ));
                }
                profile.insert("nvidiaCTGPOffset".to_string(), json!(ctgp));
            }
        }

        let tdp_values = profile
            .get("odmPowerLimits")
            .and_then(Value::as_object)
            .and_then(|o| o.get("tdpValues"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let report_entry = json!({
            "name": profile.get("name").cloned().unwrap_or(Value::Null),
            "fanPreset": name,
            "fanModifiers": {
                "minimum": min_speed,
                "maximum": max_speed,
                "offset": offset_speed,
            },
            "tdpValues": tdp_values,
            "nvidiaCTGPOffset": profile.get("nvidiaCTGPOffset").cloned().unwrap_or(Value::Null),
        });
        report_profiles.insert(identifier.clone(), report_entry);
        merged_profiles.insert(identifier, Value::Object(profile));
    }

    // Explicit exported UCC profiles and their cooler settings take precedence.
    if let Some(Value::Object(explicit_profiles)) = settings_obj.get("profiles") {
        for (k, v) in explicit_profiles {
            merged_profiles.insert(k.clone(), v.clone());
        }
    }
    if let Some(Value::Object(explicit_fans)) = settings_obj.get("customFanProfiles") {
        for (k, v) in explicit_fans {
            custom_fans.insert(k.clone(), v.clone());
        }
    }

    result_map.insert("profiles".to_string(), Value::Object(merged_profiles));
    result_map.insert("customFanProfiles".to_string(), Value::Object(custom_fans));

    let report = json!({
        "profiles": Value::Object(report_profiles),
        "runtimeCooling": "UCC filtering, hysteresis and critical-temperature overrides",
    });

    Ok((Value::Object(result_map), report))
}

fn cpu_policy(name: &str) -> Result<(&'static str, &'static str), ConfigError> {
    match name {
        "performance" => Ok(("performance", "performance")),
        "balanced" => Ok(("powersave", "balance_performance")),
        "power-saver" => Ok(("powersave", "power")),
        other => Err(ConfigError::Validation(format!(
            "Unsupported cpu_profile: {other}"
        ))),
    }
}

/// Converts offline `Data/` export and baseline settings without modifying live paths.
pub fn convert_offline_import(
    data: &OfflineUserData,
    settings: &Value,
    capabilities: &Value,
    brightness: Option<i64>,
) -> Result<OfflineImportOutput, ConfigError> {
    let settings_obj = require_object(settings, "Invalid settings container")?;
    let caps_obj = require_object(capabilities, "Invalid capabilities container")?;
    let zones = require_strict_int(
        caps_obj
            .get("zones")
            .ok_or_else(|| ConfigError::Validation("Invalid keyboard zone count".to_string()))?,
        "Invalid keyboard zone count",
    )?;
    if !(1..=1024).contains(&zones) {
        return Err(ConfigError::Validation(
            "Invalid keyboard zone count".to_string(),
        ));
    }
    let maximum = require_strict_int(
        caps_obj.get("maxBrightness").ok_or_else(|| {
            ConfigError::Validation("Invalid hardware brightness range".to_string())
        })?,
        "Invalid hardware brightness range",
    )?;
    if !(1..=65535).contains(&maximum) {
        return Err(ConfigError::Validation(
            "Invalid hardware brightness range".to_string(),
        ));
    }

    let leds: Vec<i64> = if let Some(led_val) = caps_obj.get("ledIds") {
        let arr = require_array(led_val, "Invalid physical LED identities")?;
        let mut parsed = Vec::with_capacity(arr.len());
        for item in arr {
            parsed.push(require_strict_int(item, "Invalid physical LED identities")?);
        }
        parsed
    } else {
        (0..zones).collect()
    };

    if leds.len() != zones as usize
        || leds.iter().any(|&led| led < 0)
        || leds.iter().copied().collect::<BTreeSet<_>>().len() != zones as usize
    {
        return Err(ConfigError::Validation(
            "Invalid physical LED identities".to_string(),
        ));
    }

    let old_states = settings_obj
        .get("keyboardBacklightStates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let effective_brightness = match brightness {
        Some(b) => b,
        None => {
            if let Some(first_obj) = old_states.first().and_then(Value::as_object) {
                if let Some(b_val) = first_obj.get("brightness") {
                    require_strict_int(b_val, "Brightness must use hardware units")?
                } else {
                    maximum
                }
            } else {
                maximum
            }
        }
    };
    if !(0..=maximum).contains(&effective_brightness) {
        return Err(ConfigError::Validation(
            "Brightness must use hardware units".to_string(),
        ));
    }
    if effective_brightness > i64::from(u8::MAX) {
        return Err(ConfigError::Validation(format!(
            "Brightness {effective_brightness} exceeds u8 hardware limit (0..={})",
            u8::MAX
        )));
    }

    let blank_zone = || {
        let mut m = Map::new();
        m.insert("red".to_string(), json!(0));
        m.insert("green".to_string(), json!(0));
        m.insert("blue".to_string(), json!(0));
        m
    };
    let states_by_led: Option<BTreeMap<i64, Map<String, Value>>> =
        if old_states.iter().any(|v| v.get("ledId").is_some()) {
            let mut map = BTreeMap::new();
            for item in &old_states {
                let obj = item.as_object().ok_or_else(|| {
                    ConfigError::Validation("Invalid keyboardBacklightStates entry".to_string())
                })?;
                let led_id = obj
                    .get("ledId")
                    .and_then(|v| require_strict_int(v, "Invalid keyboard state ledId").ok())
                    .ok_or_else(|| {
                        ConfigError::Validation(
                            "Ambiguous keyboardBacklightStates mix with missing ledId".to_string(),
                        )
                    })?;
                map.insert(led_id, obj.clone());
            }
            Some(map)
        } else {
            None
        };
    let full_by_physical_index =
        !old_states.is_empty() && leds.iter().all(|&led| (led as usize) < old_states.len());
    if !old_states.is_empty()
        && states_by_led.is_none()
        && old_states.len() != zones as usize
        && !full_by_physical_index
    {
        return Err(ConfigError::Validation(format!(
            "Ambiguous keyboard layout: {} saved states cannot map to target LEDs {:?}",
            old_states.len(),
            leds
        )));
    }

    let mut states: Vec<Map<String, Value>> = leds
        .iter()
        .enumerate()
        .map(|(i, &led)| {
            if let Some(ref map) = states_by_led {
                map.get(&led).cloned().unwrap_or_else(blank_zone)
            } else if old_states.is_empty() {
                blank_zone()
            } else if old_states.len() == zones as usize {
                old_states
                    .get(i)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_else(blank_zone)
            } else {
                old_states
                    .get(led as usize)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_else(blank_zone)
            }
        })
        .collect();

    let by_led: BTreeMap<i64, usize> = leds
        .iter()
        .copied()
        .enumerate()
        .map(|(index, led)| (led, index))
        .collect();

    let colors = require_object(&data.keyboard_colors, "Missing per-key colors")?;
    if colors.is_empty() {
        return Err(ConfigError::Validation(
            "Missing per-key colors".to_string(),
        ));
    }
    for (led_key, rgb_val) in colors {
        let led_id = led_key
            .parse::<i64>()
            .map_err(|_| ConfigError::Validation(format!("Invalid LED key: {led_key}")))?;
        let &index = by_led.get(&led_id).ok_or_else(|| {
            ConfigError::Validation(format!(
                "Saved LED {led_id} is absent from target capabilities"
            ))
        })?;
        let rgb_arr = require_array(rgb_val, &format!("Invalid RGB value for LED {led_id}"))?;
        if rgb_arr.len() != 3 {
            return Err(ConfigError::Validation(format!(
                "Invalid RGB value for LED {led_id}"
            )));
        }
        let r = require_strict_int(&rgb_arr[0], &format!("Invalid RGB value for LED {led_id}"))?;
        let g = require_strict_int(&rgb_arr[1], &format!("Invalid RGB value for LED {led_id}"))?;
        let b = require_strict_int(&rgb_arr[2], &format!("Invalid RGB value for LED {led_id}"))?;
        if !(0..=255).contains(&r) || !(0..=255).contains(&g) || !(0..=255).contains(&b) {
            return Err(ConfigError::Validation(format!(
                "Invalid RGB value for LED {led_id}"
            )));
        }
        states[index].insert("red".to_string(), json!(r));
        states[index].insert("green".to_string(), json!(g));
        states[index].insert("blue".to_string(), json!(b));
    }
    for state in &mut states {
        state.insert("brightness".to_string(), json!(effective_brightness));
        state.insert("mode".to_string(), json!(0));
    }

    let states_value = Value::Array(states.into_iter().map(Value::Object).collect());
    let mut result_map = settings_obj.clone();
    let keyboard_id = "frixit-keyboard";
    result_map.insert(
        "keyboardLayout".to_string(),
        Value::String("frixit-mechrevo".to_string()),
    );
    result_map.insert("keyboardBacklightStates".to_string(), states_value.clone());

    let mut custom_kbd = result_map
        .remove("customKeyboardProfiles")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    custom_kbd.insert(
        keyboard_id.to_string(),
        json!({
            "id": keyboard_id,
            "name": "Frixit keyboard",
            "json": {
                "brightness": effective_brightness,
                "states": states_value,
            }
        }),
    );
    result_map.insert(
        "customKeyboardProfiles".to_string(),
        Value::Object(custom_kbd),
    );

    let mut profiles = result_map
        .remove("profiles")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut mappings = result_map
        .remove("stateMap")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let config_obj = require_object(&data.config, "Missing config in Data/")?;
    let config_settings = require_object(
        config_obj
            .get("settings")
            .ok_or_else(|| ConfigError::Validation("Missing config.settings".to_string()))?,
        "Invalid config.settings",
    )?;
    let devices = config_obj
        .get("devices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let turbo = devices
        .iter()
        .find(|dev| {
            dev.get("path").and_then(Value::as_str)
                == Some("/sys/devices/system/cpu/intel_pstate/no_turbo")
        })
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let meta_names = config_obj
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|m| m.get("tuxedo_profiles"))
        .and_then(Value::as_object);

    let mut report_profiles = Map::new();
    let mut unresolved_profiles = Vec::new();
    let mut per_mode = Map::new();

    for mode in ["ac", "type_c", "bat"] {
        let source = require_object(
            config_settings
                .get(mode)
                .ok_or_else(|| ConfigError::Validation(format!("Missing mode {mode}")))?,
            "Invalid mode settings",
        )?;
        let cpu_name = source
            .get("cpu_profile")
            .and_then(Value::as_str)
            .unwrap_or("balanced");
        let (governor, epp) = cpu_policy(cpu_name)?;

        per_mode.insert(
            mode.to_string(),
            json!({
                "governor": governor,
                "epp": epp,
                "aspm": if cpu_name == "power-saver" { "powersupersave" } else { "default" },
            }),
        );

        let source_id_val = source
            .get("tuxedo_profile_id")
            .cloned()
            .unwrap_or(Value::Null);
        let base_opt = source_id_val
            .as_str()
            .and_then(|id| profiles.get(id).cloned());
        let Some(base_val) = base_opt else {
            unresolved_profiles.push(json!({
                "mode": mode,
                "tccProfileId": source_id_val,
            }));
            continue;
        };

        let base_obj = match base_val {
            Value::String(s) => serde_json::from_str::<Value>(&s)?,
            other => other,
        };
        let mut profile = require_object(&base_obj, "Invalid base profile")?.clone();
        let profile_id = format!("frixit-{mode}");
        let source_id_str = source_id_val.as_str().unwrap_or_default();
        let resolved_name = meta_names
            .and_then(|m| m.get(source_id_str))
            .and_then(Value::as_str)
            .or_else(|| profile.get("name").and_then(Value::as_str))
            .unwrap_or(mode)
            .to_string();

        profile.insert("id".to_string(), Value::String(profile_id.clone()));
        profile.insert("name".to_string(), Value::String(resolved_name));

        let mut cpu_obj = profile
            .remove("cpu")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        cpu_obj.insert("governor".to_string(), json!(governor));
        cpu_obj.insert("energyPerformancePreference".to_string(), json!(epp));
        let turbo_key = format!("{mode}_val");
        if let Some(turbo_val) = turbo.get(&turbo_key) {
            cpu_obj.insert(
                "noTurbo".to_string(),
                Value::Bool(turbo_val.as_str() == Some("1")),
            );
        }
        profile.insert("cpu".to_string(), Value::Object(cpu_obj));
        profile.insert(
            "selectedKeyboardProfile".to_string(),
            Value::String(keyboard_id.to_string()),
        );

        profiles.insert(profile_id.clone(), Value::Object(profile));
        mappings.insert(format!("power_{mode}"), Value::String(profile_id.clone()));
        report_profiles.insert(mode.to_string(), Value::String(profile_id));
    }

    result_map.insert("profiles".to_string(), Value::Object(profiles));
    result_map.insert("stateMap".to_string(), Value::Object(mappings));

    let keep_nvidia_awake = devices.iter().any(|dev| {
        let name = dev.get("name").and_then(Value::as_str).unwrap_or("");
        name.contains("GB203") || name.contains("RTX 5080")
    });
    result_map.insert(
        "powerDevices".to_string(),
        json!({
            "enabled": true,
            "cpu": Value::Object(per_mode),
            "keepNvidiaAwake": keep_nvidia_awake,
            "devices": Value::Array(devices.clone()),
        }),
    );

    let mut lightbar_obj = data.lightbar.as_object().cloned().unwrap_or_default();
    lightbar_obj.insert("managed".to_string(), Value::Bool(true));
    result_map.insert("lightbar".to_string(), Value::Object(lightbar_obj));

    let report = json!({
        "importedKeys": colors.len(),
        "importedDevices": devices.len(),
        "brightness": effective_brightness,
        "ignored": ["cpu_temp_limit"],
        "excludedFeatures": ["gpu-run"],
        "profiles": Value::Object(report_profiles),
        "unresolvedProfiles": Value::Array(unresolved_profiles),
    });

    let pending = json!({
        "description": "Original GUI values retained for reference. Apply desktop-preferences.json as the desktop user; this file is not daemon settings.",
        "gui": data.gui_settings.clone(),
    });

    let settings_val = Value::Object(result_map);
    serde_json::from_value::<crate::UccSettings>(settings_val.clone()).map_err(|err| {
        ConfigError::Validation(format!(
            "Converted settings failed schema validation: {err}"
        ))
    })?;

    Ok(OfflineImportOutput {
        settings: settings_val,
        report,
        pending,
    })
}

/// Extracts and validates user GUI theme/autostart preferences from `Data/gui_settings.json`.
pub fn desktop_preferences(data: &OfflineUserData) -> Result<Value, ConfigError> {
    let source = if data.gui_settings.is_null() {
        Map::new()
    } else {
        require_object(&data.gui_settings, "Invalid source GUI preferences")?.clone()
    };
    let theme = match source.get("theme") {
        None => "system",
        Some(v) => v
            .as_str()
            .ok_or_else(|| ConfigError::Validation("Invalid source GUI preferences".to_string()))?,
    };
    let minimized = match source.get("start_minimized") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            return Err(ConfigError::Validation(
                "Invalid source GUI preferences".to_string(),
            ));
        }
    };
    if !matches!(theme, "system" | "auto" | "light" | "dark") {
        return Err(ConfigError::Validation(
            "Invalid source GUI preferences".to_string(),
        ));
    }
    Ok(json!({
        "theme": if theme == "system" { "auto" } else { theme },
        "startMinimized": minimized,
    }))
}

fn is_valid_mac_address(addr: &str) -> bool {
    let parts: Vec<&str> = addr.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Converts a Mechrevo TCC capture + saved Aquaris state into UCC settings and `aquaris.json`.
pub fn convert_mechrevo_tcc(
    settings: &Value,
    profiles: &Value,
    user: &Value,
    balanced: &Value,
    hardware: &Value,
) -> Result<(Value, Value), ConfigError> {
    let settings_obj = require_object(settings, "Invalid Mechrevo TCC settings")?;
    let profiles_arr = require_array(profiles, "Invalid Mechrevo TCC profiles")?;
    let user_obj = require_object(user, "Invalid Mechrevo user settings")?;
    let hw_obj = require_object(hardware, "Invalid hardware capabilities")?;

    let mut result_map = Map::new();
    for key in [
        "fahrenheit",
        "stateMap",
        "cpuSettingsEnabled",
        "fanControlEnabled",
        "chargingProfile",
        "chargingPriority",
        "ycbcr420Workaround",
    ] {
        if let Some(val) = settings_obj.get(key) {
            result_map.insert(key.to_string(), val.clone());
        }
    }
    result_map.insert(
        "keyboardBacklightControlSupported".to_string(),
        settings_obj
            .get("keyboardBacklightControlEnabled")
            .cloned()
            .unwrap_or(Value::Bool(true)),
    );

    let states = settings_obj
        .get("keyboardBacklightStates")
        .and_then(Value::as_array)
        .ok_or_else(|| ConfigError::Validation("Missing keyboardBacklightStates".to_string()))?;
    let brightness = states
        .first()
        .and_then(|s| s.get("brightness"))
        .cloned()
        .ok_or_else(|| ConfigError::Validation("Missing keyboard brightness".to_string()))?;
    let brightness_int = require_strict_int(&brightness, "Invalid keyboard brightness")?;
    if !(0..=i64::from(u8::MAX)).contains(&brightness_int) {
        return Err(ConfigError::Validation(format!(
            "Brightness {brightness_int} exceeds u8 hardware limit (0..={})",
            u8::MAX
        )));
    }

    result_map.insert(
        "customKeyboardProfiles".to_string(),
        json!({
            "tcc-keyboard": {
                "id": "tcc-keyboard",
                "name": "Imported TCC keyboard",
                "json": {
                    "brightness": brightness,
                    "states": Value::Array(states.clone()),
                }
            }
        }),
    );

    let last_device = user_obj
        .get("aquarisLastDeviceName")
        .and_then(Value::as_str);
    if last_device != Some("CoolingSystem LCT22002") {
        return Err(ConfigError::Validation(
            "This migration supports the audited LCT22002 only".to_string(),
        ));
    }

    let saved_str = user_obj
        .get("aquarisSaveState")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::Validation("Missing aquarisSaveState".to_string()))?;
    let saved_val: Value = serde_json::from_str(saved_str)?;
    let saved = require_object(&saved_val, "Invalid aquarisSaveState JSON")?;

    let device_uuid = saved
        .get("deviceUUID")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::Validation("Invalid saved cooler address".to_string()))?;
    if !is_valid_mac_address(device_uuid) {
        return Err(ConfigError::Validation(
            "Invalid saved cooler address".to_string(),
        ));
    }

    let led_on = saved
        .get("ledOn")
        .and_then(Value::as_bool)
        .ok_or_else(|| ConfigError::Validation("Invalid ledOn flag".to_string()))?;
    let (red, green, blue) = if led_on {
        (
            saved.get("red").cloned().unwrap_or(json!(0)),
            saved.get("green").cloned().unwrap_or(json!(0)),
            saved.get("blue").cloned().unwrap_or(json!(0)),
        )
    } else {
        (json!(0), json!(0), json!(0))
    };
    let aquaris = json!({
        "address": device_uuid.to_ascii_uppercase(),
        "name": "CoolingSystem LCT22002",
        "red": red,
        "green": green,
        "blue": blue,
        "ledMode": saved.get("ledMode").cloned().unwrap_or(json!(0)),
    });

    let pump_on = saved
        .get("pumpOn")
        .and_then(Value::as_bool)
        .ok_or_else(|| ConfigError::Validation("Invalid pumpOn flag".to_string()))?;
    let pump_level = if pump_on {
        let voltage = require_strict_int(
            saved
                .get("pumpVoltage")
                .ok_or_else(|| ConfigError::Validation("Missing pumpVoltage".to_string()))?,
            "Invalid pumpVoltage",
        )?;
        match voltage {
            0 => 3,
            1 => 4,
            2 => 1,
            3 => 2,
            4 => 0,
            other => {
                return Err(ConfigError::Validation(format!(
                    "Unsupported pumpVoltage: {other}"
                )));
            }
        }
    } else {
        0
    };
    if pump_level == 4 {
        return Err(ConfigError::Validation(
            "Upstream UCC explicitly rejects V12; manual review required".to_string(),
        ));
    }

    let fan_on = saved
        .get("fanOn")
        .and_then(Value::as_bool)
        .ok_or_else(|| ConfigError::Validation("Invalid fanOn flag".to_string()))?;
    let wc_fan = if fan_on {
        require_strict_int(
            saved
                .get("fanDutyCycle")
                .ok_or_else(|| ConfigError::Validation("Missing fanDutyCycle".to_string()))?,
            "Invalid water cooler fan duty",
        )?
    } else {
        0
    };
    if !(0..=100).contains(&wc_fan) {
        return Err(ConfigError::Validation(
            "Invalid water cooler fan duty".to_string(),
        ));
    }

    let mut out_profiles = Map::new();
    let mut custom_fans = Map::new();

    for old in profiles_arr {
        let mut p = require_object(old, "Invalid profile entry")?.clone();
        let old_name = p
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ConfigError::Validation("Missing profile name".to_string()))?;
        p.insert("name".to_string(), json!(format!("{old_name} (imported)")));

        if let Some(Value::Object(display)) = p.get_mut("display") {
            if display.get("refreshRate").is_none_or(Value::is_null) {
                display.insert("refreshRate".to_string(), json!(-1));
            }
        }

        if let Some(Value::Object(cpu)) = p.get_mut("cpu") {
            if cpu.remove("useMaxPerfGov").and_then(|v| v.as_bool()) == Some(true) {
                cpu.insert("governor".to_string(), json!("performance"));
            }
            if let Some(epp) = hw_obj.get("epp") {
                cpu.insert("energyPerformancePreference".to_string(), epp.clone());
            }
            if let Some(boost) = hw_obj.get("hwpDynamicBoost") {
                cpu.insert("hwpDynamicBoost".to_string(), boost.clone());
            }
        }

        let fan = require_object(
            p.get("fan")
                .ok_or_else(|| ConfigError::Validation("Missing fan section".to_string()))?,
            "Invalid fan section",
        )?
        .clone();
        let fan_profile_name = fan
            .get("fanProfile")
            .and_then(Value::as_str)
            .ok_or_else(|| ConfigError::Validation("Missing fanProfile".to_string()))?;
        let curves_val = match fan_profile_name {
            "Balanced" => balanced,
            "Custom" => fan
                .get("customFanCurve")
                .ok_or_else(|| ConfigError::Validation("Missing customFanCurve".to_string()))?,
            other => {
                return Err(ConfigError::Validation(format!(
                    "Unreviewed TCC fan preset: {other}"
                )));
            }
        };
        let curves = require_object(curves_val, "Invalid fan curves object")?;

        let min_fan = require_strict_int(
            fan.get("minimumFanspeed")
                .ok_or_else(|| ConfigError::Validation("Missing minimumFanspeed".to_string()))?,
            "Invalid minimumFanspeed",
        )?;
        let max_fan = require_strict_int(
            fan.get("maximumFanspeed")
                .ok_or_else(|| ConfigError::Validation("Missing maximumFanspeed".to_string()))?,
            "Invalid maximumFanspeed",
        )?;
        let off_fan = require_strict_int(
            fan.get("offsetFanspeed")
                .ok_or_else(|| ConfigError::Validation("Missing offsetFanspeed".to_string()))?,
            "Invalid offsetFanspeed",
        )?;

        let mut new_fan = Map::new();
        if let Some(use_ctrl) = fan.get("useControl") {
            new_fan.insert("useControl".to_string(), use_ctrl.clone());
        }
        new_fan.insert("fanProfile".to_string(), json!("tcc-imported"));
        new_fan.insert("sameSpeed".to_string(), json!(false));
        new_fan.insert("autoControlWC".to_string(), json!(true));
        new_fan.insert("enableWaterCooler".to_string(), json!(true));

        let mut fan_json = Map::new();
        for key in ["tableCPU", "tableGPU"] {
            let entries = require_array(
                curves
                    .get(key)
                    .ok_or_else(|| ConfigError::Validation(format!("Missing {key}")))?,
                "Invalid fan curve array",
            )?;
            let mut baked = Vec::with_capacity(entries.len());
            for entry in entries {
                let entry_obj = require_object(entry, "Invalid fan curve entry")?;
                let temp = entry_obj
                    .get("temp")
                    .cloned()
                    .ok_or_else(|| ConfigError::Validation("Missing temp".to_string()))?;
                let speed = require_strict_int(
                    entry_obj
                        .get("speed")
                        .ok_or_else(|| ConfigError::Validation("Missing speed".to_string()))?,
                    "Invalid speed",
                )?;
                let clamped = speed
                    .saturating_add(off_fan)
                    .min(max_fan)
                    .max(min_fan)
                    .clamp(0, 100);
                baked.push(json!({ "temp": temp, "speed": clamped }));
            }
            let baked_val = Value::Array(baked);
            new_fan.insert(key.to_string(), baked_val.clone());
            fan_json.insert(key.to_string(), baked_val);
        }

        let pump_table = json!([
            { "temp": 0, "speed": pump_level },
            { "temp": 100, "speed": pump_level }
        ]);
        let wc_fan_table = json!([
            { "temp": 0, "speed": wc_fan },
            { "temp": 100, "speed": wc_fan }
        ]);
        new_fan.insert("tablePump".to_string(), pump_table.clone());
        new_fan.insert("tableWaterCoolerFan".to_string(), wc_fan_table.clone());
        fan_json.insert("tablePump".to_string(), pump_table);
        fan_json.insert("tableWaterCoolerFan".to_string(), wc_fan_table);

        p.insert("fan".to_string(), Value::Object(new_fan));
        custom_fans.insert(
            "tcc-imported".to_string(),
            json!({
                "id": "tcc-imported",
                "name": "Imported TCC Balanced +20%",
                "json": Value::Object(fan_json),
            }),
        );

        p.remove("nvidiaPowerCTRLProfile");
        if hw_obj.get("ctgpWritable").and_then(Value::as_bool) == Some(true) {
            if let Some(ctgp_offset) = hw_obj.get("ctgpOffset") {
                p.insert("nvidiaCTGPOffset".to_string(), ctgp_offset.clone());
            }
        }

        let charging = settings_obj
            .get("chargingProfile")
            .and_then(Value::as_str)
            .unwrap_or("");
        p.insert("chargingProfile".to_string(), json!(charging));
        p.insert(
            "keyboard".to_string(),
            json!({
                "brightness": brightness,
                "states": Value::Array(states.clone()),
            }),
        );
        p.insert("selectedKeyboardProfile".to_string(), json!("tcc-keyboard"));

        let id = p
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ConfigError::Validation("Missing profile id".to_string()))?
            .to_string();
        if out_profiles.contains_key(&id) {
            return Err(ConfigError::Validation(format!(
                "Duplicate Mechrevo profile id: {id}"
            )));
        }
        out_profiles.insert(id, Value::Object(p));
    }

    result_map.insert("profiles".to_string(), Value::Object(out_profiles));
    result_map.insert("customFanProfiles".to_string(), Value::Object(custom_fans));

    if let Some(Value::Object(state_map)) = result_map.get_mut("stateMap") {
        if let Some(power_ac) = state_map.get("power_ac").cloned() {
            state_map.insert("power_wc".to_string(), power_ac);
        }
    }

    Ok((Value::Object(result_map), aquaris))
}

/// Writes the offline import bundle into a brand-new `0o700` directory with `0o600` files.
///
/// Stages files in a private sibling directory, validates them, and atomically renames on
/// completion so an interrupted write never leaves a partial/unretryable target directory.
pub fn write_offline_import_bundle(
    output_dir: &Path,
    output: &OfflineImportOutput,
    preferences: &Value,
) -> Result<(), ConfigError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

    if fs::symlink_metadata(output_dir).is_ok() {
        return Err(ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("Output directory already exists: {}", output_dir.display()),
        )));
    }

    let parent = output_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let base_name = output_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("import-bundle");

    let staging_dir = loop {
        let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{base_name}.staging-{}-{seq}", std::process::id()));
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&candidate)
        };
        #[cfg(not(unix))]
        let created = fs::create_dir(&candidate);
        match created {
            Ok(()) => break candidate,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(ConfigError::Io(err)),
        }
    };

    let stage_result: Result<(), ConfigError> = (|| {
        for (name, value) in [
            ("settings", &output.settings),
            ("report.json", &output.report),
            ("pending.json", &output.pending),
            ("desktop-preferences.json", preferences),
        ] {
            let path = staging_dir.join(name);
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(&path)?;
            let mut serialized = serde_json::to_string_pretty(value)?;
            serialized.push('\n');
            file.write_all(serialized.as_bytes())?;
            file.sync_all()?;
        }

        let _ = crate::ConfigManager::load_from_path(&staging_dir.join("settings"))?;

        if fs::symlink_metadata(output_dir).is_ok() {
            return Err(ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Output directory already exists: {}", output_dir.display()),
            )));
        }
        fs::rename(&staging_dir, output_dir)?;
        Ok(())
    })();

    if let Err(err) = stage_result {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(err);
    }

    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}
