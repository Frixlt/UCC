// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic fuzz and mutation corpus harness for `ucc-config`.
//!
//! Covers four parsing and persistence domains with zero external fuzzing crates:
//! 1. JSON settings & profile configs (`UccSettings`, `ProfilesCatalog`, `LegacyProfileDto`).
//! 2. Offline import (`import.rs`: TCC fan tables, ASAR extraction, `OfflineUserData`, Mechrevo).
//! 3. User-config INI / QSettings rejection & `legacy.rs` (`SettingsManager`, `TccSettings`).
//! 4. Persistence, backup recovery, symlink safety, and `platform_paths` (`sleep_state_file`).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use tempfile::TempDir;
use ucc_config::{
    cleanup_temp_files, convert_mechrevo_tcc, convert_offline_import, create_default_settings,
    desktop_preferences, effective_tcc_fan_table, is_valid_settings_content, merge_tcc,
    read_tcc_fan_presets, resolve_platform_paths, validate_fan_points_slice,
    validate_tcc_fan_points, write_atomic, ConfigError, ConfigManager, LegacyProfileDto,
    OfflineUserData, PathPlatform, ProfileState, ProfilesCatalog, SettingsManager, TccFanPoint,
    TccFanPreset, TccSettings, UccSettings,
};

// ============================================================================
// Self-contained SplitMix64 PRNG & deterministic mutators
// ============================================================================

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() as usize) % bound
    }

    fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    fn next_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = self.next_usize(items.len());
        &items[idx]
    }
}

const ADVERSARIAL_TOKENS: &[&[u8]] = &[
    b"null",
    b"true",
    b"false",
    b"-1",
    b"0",
    b"255",
    b"256",
    b"65535",
    b"65536",
    b"-2147483648",
    b"2147483647",
    b"-9223372036854775808",
    b"9223372036854775807",
    b"18446744073709551615",
    b"1e308",
    b"-1e308",
    b"1e309",
    b"-0.0",
    b"NaN",
    b"Infinity",
    b"\"\"",
    b"\"\\u0000\\u001b\\u007f\"",
    b"[]",
    b"{}",
    b"\"keyboardBacklightStates\":[]",
    b"\"stateMap\":{}",
    b"\"profiles\":{}",
    b"\"chargeStartThreshold\":-1",
    b"\"chargeEndThreshold\":101",
    b"\xFF\xFE\x80\xC0\xAF\xED\xA0\x80",
    b"\x00\x01\x07\x0B\x1B\x7F",
    b"[General]\nkey=value\n",
];

const BOUNDARY_NUMBERS: &[i64] = &[
    i64::MIN,
    i64::MIN + 1,
    i32::MIN as i64,
    -1000,
    -101,
    -100,
    -1,
    0,
    1,
    50,
    100,
    101,
    120,
    121,
    255,
    256,
    65535,
    65536,
    i32::MAX as i64,
    i64::MAX - 1,
    i64::MAX,
];

fn mutate_bit_flip(rng: &mut SplitMix64, buf: &mut [u8]) {
    if buf.is_empty() {
        return;
    }
    let flips = 1 + rng.next_usize(4);
    for _ in 0..flips {
        let idx = rng.next_usize(buf.len());
        let bit = 1_u8 << (rng.next_usize(8) as u8);
        buf[idx] ^= bit;
    }
}

fn mutate_byte_splice(rng: &mut SplitMix64, buf: &mut Vec<u8>, donor: &[u8]) {
    if donor.is_empty() {
        return;
    }
    let start = rng.next_usize(donor.len());
    let max_len = (donor.len() - start).min(64);
    let len = 1 + rng.next_usize(max_len);
    let insert_at = rng.next_usize(buf.len() + 1);
    buf.splice(insert_at..insert_at, donor[start..start + len].iter().copied());
}

fn mutate_truncate(rng: &mut SplitMix64, buf: &mut Vec<u8>) {
    let cut = rng.next_usize(buf.len() + 1);
    buf.truncate(cut);
}

fn mutate_duplicate_key_inject(rng: &mut SplitMix64, buf: &mut Vec<u8>) {
    const DUP_PAIRS: &[&[u8]] = &[
        b"\"keyboardBacklightStates\":[{\"red\":1,\"green\":2,\"blue\":3,\"brightness\":10}],",
        b"\"chargeStartThreshold\":20,\"chargeStartThreshold\":40,",
        b"\"chargeEndThreshold\":90,\"chargeEndThreshold\":80,",
        b"\"fahrenheit\":true,\"fahrenheit\":false,",
        b"\"stateMap\":{\"power_ac\":\"a\"},\"stateMap\":{\"power_ac\":\"b\"},",
        b"\"profiles\":{},\"profiles\":null,",
    ];
    let pair = *rng.choose(DUP_PAIRS);
    if let Some(pos) = buf.iter().position(|&b| b == b'{') {
        buf.splice(pos + 1..pos + 1, pair.iter().copied());
    } else {
        buf.extend_from_slice(pair);
    }
}

fn mutate_deep_nest(rng: &mut SplitMix64, buf: &mut Vec<u8>) {
    let depth = 64 + rng.next_usize(196);
    let use_obj = rng.next_bool();
    let mut nested = Vec::with_capacity(depth * 6 + 16);
    if use_obj {
        for _ in 0..depth {
            nested.extend_from_slice(b"{\"a\":");
        }
        nested.extend_from_slice(b"0");
        for _ in 0..depth {
            nested.push(b'}');
        }
    } else {
        nested.resize(depth, b'[');
        nested.push(b'0');
        nested.resize(depth * 2 + 1, b']');
    }
    *buf = nested;
}

fn mutate_invalid_utf8_and_controls(rng: &mut SplitMix64, buf: &mut Vec<u8>) {
    const BAD_SEQS: &[&[u8]] = &[
        b"\xFF\xFE\xFD",
        b"\x80\x81\x82\x83",
        b"\xC0\xAF",
        b"\xED\xA0\x80",
        b"\x00\x01\x02\x07\x0B\x1B\x7F",
    ];
    let seq = *rng.choose(BAD_SEQS);
    let pos = rng.next_usize(buf.len() + 1);
    buf.splice(pos..pos, seq.iter().copied());
}

fn mutate_bytes(rng: &mut SplitMix64, seed: &[u8], corpus: &[&[u8]]) -> Vec<u8> {
    let mut out = seed.to_vec();
    let rounds = 1 + rng.next_usize(3);
    for _ in 0..rounds {
        match rng.next_usize(8) {
            0 => mutate_bit_flip(rng, &mut out),
            1 => {
                let donor = *rng.choose(corpus);
                mutate_byte_splice(rng, &mut out, donor);
            }
            2 => mutate_truncate(rng, &mut out),
            3 => mutate_duplicate_key_inject(rng, &mut out),
            4 => mutate_deep_nest(rng, &mut out),
            5 => {
                let tok = *rng.choose(ADVERSARIAL_TOKENS);
                mutate_byte_splice(rng, &mut out, tok);
            }
            6 => mutate_invalid_utf8_and_controls(rng, &mut out),
            _ => {
                if !out.is_empty() {
                    let pos = rng.next_usize(out.len());
                    out[pos] = rng.next_u8();
                } else {
                    out.push(rng.next_u8());
                }
            }
        }
        if out.len() > 16 * 1024 {
            out.truncate(16 * 1024);
        }
    }
    out
}

fn build_asar_archive_bytes(header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + header.len() + body.len());
    out.extend_from_slice(&4_u32.to_le_bytes());
    out.extend_from_slice(&((header.len() + 8) as u32).to_le_bytes());
    out.extend_from_slice(&((header.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(body);
    out
}

fn valid_asar_archive_bytes(js_body: &[u8]) -> Vec<u8> {
    let mut tree = json!({
        "size": js_body.len(),
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
    let header = serde_json::to_vec(&tree).expect("serialize asar header");
    build_asar_archive_bytes(&header, js_body)
}

fn sample_offline_data() -> OfflineUserData {
    OfflineUserData {
        config: json!({
            "settings": {
                "ac": {
                    "tuxedo_profile_id": "shared",
                    "cpu_profile": "performance",
                    "cpu_temp_limit": 85
                },
                "type_c": {
                    "tuxedo_profile_id": "shared",
                    "cpu_profile": "balanced",
                    "cpu_temp_limit": 85
                },
                "bat": {
                    "tuxedo_profile_id": "shared",
                    "cpu_profile": "power-saver",
                    "cpu_temp_limit": 85
                }
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
        gui_settings: json!({ "theme": "system", "start_minimized": false }),
    }
}

fn sample_offline_base() -> Value {
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

fn sample_offline_caps() -> Value {
    json!({ "zones": 4, "maxBrightness": 50, "ledIds": [0, 21, 56, 66] })
}

fn sample_mechrevo_fixture() -> (Value, Value, Value, Value, Value) {
    let kbd_states: Vec<Value> = (0..4)
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

// ============================================================================
// Domain 1: JSON settings & profile configs
// ============================================================================

#[test]
fn fuzz_domain1_settings_and_profiles_json_corpus() {
    let default_settings_json = create_default_settings()
        .to_json_pretty()
        .expect("serialize default settings");
    let autosave_seed = concat!(
        r#"{"stateMap":{"power_ac":"p1"},"profiles":{"p1":{"id":"p1","name":"Auto"}},"#,
        r#""fahrenheit":true}"#,
    );
    let aquaris_seed = concat!(
        r#"{"address":"02:00:00:00:00:01","name":"CoolingSystem LCT22002","#,
        r#""red":12,"green":34,"blue":56,"ledMode":0}"#,
    );
    let catalog_seed = r#"{"profiles":[{"id":"q","name":"Quiet","cpu":{"governor":"powersave"}}]}"#;
    let profile_seed = concat!(
        r#"{"id":"prof1","name":"Profile 1","display":{"brightness":80},"#,
        r#""cpu":{"governor":"powersave","onlineCores":8},"#,
        r#""fan":{"useControl":true,"tableCPU":[{"temp":40,"speed":20},{"temp":80,"speed":80}]},"#,
        r#""odmPowerLimits":{"tdpValues":[25,45,65]}}"#,
    );
    let custom_fan_seed = concat!(
        r#"{"customFanProfiles":[{"id":"cf1","name":"Silent","json":{"tableCPU":"#,
        r#"[{"temp":40,"speed":10}]}}],"ycbcr420Workaround":[{"eDP-1":true},"#,
        r#"{"port":"HDMI-1","enabled":false},{"card":0,"ports":[]},"DP-1"]}"#,
    );

    let seeds: [&[u8]; 6] = [
        default_settings_json.as_bytes(),
        autosave_seed.as_bytes(),
        aquaris_seed.as_bytes(),
        catalog_seed.as_bytes(),
        profile_seed.as_bytes(),
        custom_fan_seed.as_bytes(),
    ];

    // Verify valid seeds parse cleanly and UccSettings round-trips identically.
    let parsed_default = UccSettings::from_json(&default_settings_json).expect("parse default");
    let rt_json = parsed_default.to_json().expect("serialize compact");
    assert_eq!(
        parsed_default,
        UccSettings::from_json(&rt_json).expect("reparse compact")
    );
    assert!(is_valid_settings_content(&default_settings_json));
    assert!(ProfilesCatalog::from_json(catalog_seed).is_ok());
    assert!(LegacyProfileDto::parse_json(profile_seed).is_ok());
    assert!(UccSettings::from_json(custom_fan_seed).is_ok());

    let mut rng = SplitMix64::new(0xD001_0001_CAFE_BEEF);
    let mut parsed_ok_count = 0_usize;

    for _ in 0..1000 {
        let base = *rng.choose(&seeds);
        let mutated = mutate_bytes(&mut rng, base, &seeds);
        let text = String::from_utf8_lossy(&mutated);

        if let Ok(settings) = UccSettings::from_json(&text) {
            parsed_ok_count += 1;
            let _ = ConfigManager::validate_settings(&settings);
            let _ = settings.get_profile("quiet");
            let _ = settings.get_active_profile_for_mode("power_ac");
            let _ = settings.get_active_profile_for_mode("power_type_c");
            let _ = settings
                .get_active_profile_for_mode_with_fallback("power_wc", Some("power_bat"));

            let reserialized = settings
                .to_json()
                .expect("valid UccSettings must always serialize");
            let reparsed = UccSettings::from_json(&reserialized)
                .expect("serialized UccSettings must always reparse");
            assert_eq!(settings, reparsed, "UccSettings round-trip invariant failed");
        }

        let _ = ProfilesCatalog::from_json(&text);
        let _ = is_valid_settings_content(&text);
        let _ = LegacyProfileDto::parse_json(&text);
    }

    assert!(
        parsed_ok_count > 0,
        "mutation corpus should still exercise valid parse paths"
    );
}

#[test]
fn fuzz_domain1_unknown_fields_duplicate_keys_and_deep_nesting() {
    // 1. Unknown/vendor fields are preserved across round-trip in `extra`.
    let with_unknown = r#"{
        "keyboardLayout": "iso",
        "vendorCustomFlag": true,
        "legacyNested": {"nestedKey": [1, 2, 3], "tag": "keep-me"},
        "customThresholdHint": 42.5
    }"#;
    let parsed = UccSettings::from_json(with_unknown).expect("parse with unknown fields");
    assert_eq!(parsed.extra.get("vendorCustomFlag"), Some(&json!(true)));
    assert_eq!(
        parsed.extra.get("legacyNested"),
        Some(&json!({"nestedKey": [1, 2, 3], "tag": "keep-me"}))
    );
    let reserialized = parsed.to_json_pretty().expect("reserialize");
    let reparsed = UccSettings::from_json(&reserialized).expect("reparse");
    assert_eq!(parsed, reparsed);

    // 2. Duplicate keys preserve last-value semantics and serialize without duplicates.
    let dup_json = r#"{
        "keyboardBacklightStates": [{"red": 1, "green": 2, "blue": 3, "brightness": 10}],
        "keyboardBacklightStates": [{"red": 9, "green": 8, "blue": 7, "brightness": 50}],
        "chargeStartThreshold": 20,
        "chargeStartThreshold": 40,
        "chargeEndThreshold": 95,
        "chargeEndThreshold": 80
    }"#;
    let dup_parsed = UccSettings::from_json(dup_json).expect("duplicate keys must parse");
    let states = dup_parsed
        .keyboard_backlight_states
        .as_ref()
        .expect("backlight states");
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].red, 9);
    assert_eq!(states[0].brightness, 50);
    assert_eq!(dup_parsed.charge_start_threshold, Some(40));
    assert_eq!(dup_parsed.charge_end_threshold, Some(80));
    let out_json = dup_parsed.to_json().expect("serialize");
    assert_eq!(out_json.matches("\"keyboardBacklightStates\"").count(), 1);

    // 3. Stringified and invalid customFanProfiles shapes.
    let stringified_fan = r#"{
        "customFanProfiles": {
            "cf1": "{\"id\":\"\",\"name\":\"FromString\",\"json\":{\"tableCPU\":[]}}"
        }
    }"#;
    let cf_parsed = UccSettings::from_json(stringified_fan).expect("stringified fan profile");
    let cf_map = cf_parsed.custom_fan_profiles.expect("fan map");
    assert_eq!(cf_map["cf1"].id, "cf1");
    assert_eq!(cf_map["cf1"].name, "FromString");
    assert!(UccSettings::from_json(r#"{"customFanProfiles": 12345}"#).is_err());

    // 4. Deeply nested JSON hits serde_json recursion limit cleanly without stack overflow.
    for depth in [130_usize, 260, 512] {
        let mut deep_obj = String::with_capacity(depth * 6 + 4);
        for _ in 0..depth {
            deep_obj.push_str("{\"a\":");
        }
        deep_obj.push('1');
        for _ in 0..depth {
            deep_obj.push('}');
        }
        assert!(UccSettings::from_json(&deep_obj).is_err());
        assert!(ProfilesCatalog::from_json(&deep_obj).is_err());
        assert!(!is_valid_settings_content(&deep_obj));
    }

    // 5. Boundary threshold values: -1 maps to None, 0..=100 valid, out-of-range errors.
    let sentinel = UccSettings::from_json(
        r#"{"chargeStartThreshold": -1, "chargeEndThreshold": -1}"#,
    )
    .expect("-1 sentinel thresholds");
    assert_eq!(sentinel.charge_start_threshold, None);
    assert_eq!(sentinel.charge_end_threshold, None);
    for bad in [-2_i64, 101, 255, i64::MIN, i64::MAX] {
        let payload = format!(r#"{{"chargeStartThreshold": {bad}}}"#);
        assert!(UccSettings::from_json(&payload).is_err());
    }
}

#[test]
fn fuzz_domain1_oversized_and_corrupt_file_rejection() {
    let tmp = TempDir::new().expect("create tempdir");

    // 1. Oversized file (> 8 MiB MAX_CONFIG_FILE_BYTES) must be rejected before JSON parsing.
    let huge_path = tmp.path().join("huge.json");
    let huge_payload = vec![b' '; 8 * 1024 * 1024 + 64];
    fs::write(&huge_path, &huge_payload).expect("write huge file");
    let err = ConfigManager::load_from_path(&huge_path).expect_err("must reject >8MiB file");
    assert!(matches!(err, ConfigError::Io(_)), "expected Io error, got {err:?}");

    // 2. Invalid UTF-8 bytes in file must return ConfigError::Io without panic.
    let bad_utf8 = tmp.path().join("bad_utf8.json");
    fs::write(&bad_utf8, [0x7B, 0xFF, 0xFE, 0x80, 0x7D]).expect("write bad utf8");
    let err = ConfigManager::load_from_path(&bad_utf8).expect_err("must reject bad utf8");
    assert!(matches!(err, ConfigError::Io(_)), "expected Io error, got {err:?}");
}

// ============================================================================
// Domain 2: Offline import (import.rs)
// ============================================================================

#[test]
fn fuzz_domain2_tcc_fan_tables_and_asar_archive_parser() {
    let mut rng = SplitMix64::new(0xD002_1111_ABCD_EF01);

    // 1. Fan point validation and effective table baking with boundary integers.
    for _ in 0..500 {
        let len = rng.next_usize(12);
        let mut points = Vec::with_capacity(len);
        let mut json_points = Vec::with_capacity(len);
        for _ in 0..len {
            let temp = *rng.choose(BOUNDARY_NUMBERS);
            let speed = *rng.choose(BOUNDARY_NUMBERS);
            points.push(TccFanPoint { temp, speed });
            json_points.push(json!({ "temp": temp, "speed": speed }));
        }
        let min_s = *rng.choose(BOUNDARY_NUMBERS);
        let max_s = *rng.choose(BOUNDARY_NUMBERS);
        let off_s = *rng.choose(BOUNDARY_NUMBERS);

        let _ = validate_tcc_fan_points(&json_points);
        let _ = validate_fan_points_slice(&points);
        if let Ok(baked) = effective_tcc_fan_table(&points, min_s, max_s, off_s) {
            assert_eq!(baked.len(), 101);
            for (idx, pt) in baked.iter().enumerate() {
                assert_eq!(pt.temp, idx as i64);
                assert!((0..=100).contains(&pt.speed));
                assert!(pt.speed >= min_s && pt.speed <= max_s);
            }
        }
    }

    // Oversized fan table (513 points > 512 cap) must fail validation.
    let oversized: Vec<TccFanPoint> = (0..513)
        .map(|i| TccFanPoint {
            temp: i as i64,
            speed: 50,
        })
        .collect();
    assert!(validate_fan_points_slice(&oversized).is_err());

    // 2. ASAR archive binary + JS literal parser fuzzing.
    let tmp = TempDir::new().expect("create tempdir");
    let archive_path = tmp.path().join("fuzz.asar");

    let valid_js = concat!(
        "export const defaultFanProfiles = [\n",
        "{name: 'Silent', tableCPU: [{temp: 0, speed: 0}, {temp: 100, speed: 100},], ",
        "tableGPU: [{temp: 0, speed: 10}, {temp: 100, speed: 90}]},\n",
        "{name: \"Custom\"},\n",
        "];\n",
        "export const customFanPreset = {\n",
        "tableCPU: [{temp: 0, speed: 20}, {temp: 100, speed: 80}],\n",
        "tableGPU: [{temp: 0, speed: 20}, {temp: 100, speed: 80}],\n",
        "};\n"
    );
    let valid_asar = valid_asar_archive_bytes(valid_js.as_bytes());
    fs::write(&archive_path, &valid_asar).expect("write valid asar");
    let presets = read_tcc_fan_presets(&archive_path).expect("parse valid asar");
    assert!(presets.contains_key("Silent"));
    assert!(presets.contains_key("Custom"));

    let asar_corpus: [&[u8]; 3] = [
        &valid_asar,
        valid_js.as_bytes(),
        b"\x04\x00\x00\x00\xFF\xFF\xFF\xFF\x00\x00\x00\x00\x01\x00\x00\x00",
    ];

    for iter in 0..400 {
        let payload = if iter % 2 == 0 {
            // Mutate raw ASAR bytes (header sizes, truncation, invalid UTF-8).
            mutate_bytes(&mut rng, &valid_asar, &asar_corpus)
        } else {
            // Keep valid ASAR framing and mutate the embedded JS payload.
            let mutated_js = mutate_bytes(&mut rng, valid_js.as_bytes(), &asar_corpus);
            valid_asar_archive_bytes(&mutated_js)
        };
        fs::write(&archive_path, &payload).expect("write mutated asar");
        if let Ok(parsed_presets) = read_tcc_fan_presets(&archive_path) {
            assert!(!parsed_presets.is_empty());
            for preset in parsed_presets.values() {
                assert!(validate_fan_points_slice(&preset.table_cpu).is_ok());
                assert!(validate_fan_points_slice(&preset.table_gpu).is_ok());
            }
        }
    }
}

#[test]
fn fuzz_domain2_offline_user_data_and_merge_tcc() {
    let mut rng = SplitMix64::new(0xD002_2222_1234_5678);
    let base_data = sample_offline_data();
    let base_settings = sample_offline_base();
    let base_caps = sample_offline_caps();

    // Verify valid baseline succeeds and yields valid UccSettings.
    let baseline_out =
        convert_offline_import(&base_data, &base_settings, &base_caps, None).expect("baseline");
    let baseline_json = serde_json::to_string(&baseline_out.settings).expect("serialize");
    assert!(UccSettings::from_json(&baseline_json).is_ok());

    let bad_values = [
        Value::Null,
        json!(true),
        json!(false),
        json!(-1),
        json!(0),
        json!(256),
        json!(65536),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(1.5),
        json!(""),
        json!("invalid-mode"),
        json!([]),
        json!({}),
        json!({ "0": [256, -1, 0] }),
        json!({ "not_int": [1, 2, 3] }),
        json!({ "9999": [1, 2, 3] }),
        json!({ "0": [1, 2] }),
        json!({ "0": [true, "x", null] }),
    ];

    for iter in 0..500 {
        let mut data = base_data.clone();
        let mut settings = base_settings.clone();
        let mut caps = base_caps.clone();

        match iter % 6 {
            0 => {
                data.keyboard_colors = rng.choose(&bad_values).clone();
            }
            1 => {
                data.config = rng.choose(&bad_values).clone();
            }
            2 => {
                data.lightbar = rng.choose(&bad_values).clone();
                data.gui_settings = rng.choose(&bad_values).clone();
            }
            3 => {
                caps["zones"] = rng.choose(&bad_values).clone();
                caps["maxBrightness"] = rng.choose(&bad_values).clone();
                caps["ledIds"] = json!([
                    *rng.choose(BOUNDARY_NUMBERS),
                    *rng.choose(BOUNDARY_NUMBERS),
                    *rng.choose(BOUNDARY_NUMBERS),
                    *rng.choose(BOUNDARY_NUMBERS)
                ]);
            }
            4 => {
                settings["keyboardBacklightStates"] = rng.choose(&bad_values).clone();
            }
            _ => {
                if let Some(obj) = data.config.as_object_mut() {
                    if let Some(s) = obj.get_mut("settings").and_then(Value::as_object_mut) {
                        if let Some(ac) = s.get_mut("ac").and_then(Value::as_object_mut) {
                            ac.insert("cpu_profile".to_string(), rng.choose(&bad_values).clone());
                        }
                    }
                }
            }
        }

        let brightness = if rng.next_bool() {
            Some(*rng.choose(BOUNDARY_NUMBERS))
        } else {
            None
        };

        let _ = desktop_preferences(&data);
        if let Ok(out) = convert_offline_import(&data, &settings, &caps, brightness) {
            let s_json = serde_json::to_string(&out.settings).expect("serialize");
            assert!(
                UccSettings::from_json(&s_json).is_ok(),
                "convert_offline_import returned schema-invalid settings"
            );
        }
    }

    // Fuzz OfflineUserData::from_dir with truncated/malformed/invalid-UTF-8 files on disk.
    let tmp = TempDir::new().expect("create tempdir");
    let config_bytes = serde_json::to_vec(&base_data.config).expect("ser config");
    let kbd_bytes = serde_json::to_vec(&base_data.keyboard_colors).expect("ser kbd");
    let lb_bytes = serde_json::to_vec(&base_data.lightbar).expect("ser lb");
    let gui_bytes = serde_json::to_vec(&base_data.gui_settings).expect("ser gui");
    let file_corpus: [&[u8]; 4] = [&config_bytes, &kbd_bytes, &lb_bytes, &gui_bytes];

    for iter in 0..80 {
        let dir = tmp.path().join(format!("data_{iter}"));
        fs::create_dir_all(&dir).expect("mkdir");
        for (name, seed) in [
            ("config.json", config_bytes.as_slice()),
            ("keyboard_colors.json", kbd_bytes.as_slice()),
            ("lightbar.json", lb_bytes.as_slice()),
            ("gui_settings.json", gui_bytes.as_slice()),
        ] {
            let bytes = mutate_bytes(&mut rng, seed, &file_corpus);
            fs::write(dir.join(name), bytes).expect("write data file");
        }
        if let Ok(loaded) = OfflineUserData::from_dir(&dir) {
            let _ = desktop_preferences(&loaded);
            let _ = convert_offline_import(&loaded, &base_settings, &base_caps, None);
        }
    }

    // Fuzz merge_tcc with mutated profiles, modifiers, cTGPOffset, and customFanCurve.
    let valid_pts = vec![
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
            table_cpu: valid_pts.clone(),
            table_gpu: valid_pts,
        },
    );

    for _ in 0..300 {
        let min_s = *rng.choose(BOUNDARY_NUMBERS);
        let max_s = *rng.choose(BOUNDARY_NUMBERS);
        let off_s = *rng.choose(BOUNDARY_NUMBERS);
        let ctgp = *rng.choose(BOUNDARY_NUMBERS);
        let fan_name = *rng.choose(&["Silent", "Custom", "MissingPreset", ""]);
        let profile = json!({
            "id": if rng.next_bool() { "p1" } else { "" },
            "name": "FuzzProfile",
            "display": { "refreshRate": rng.choose(&bad_values) },
            "fan": {
                "useControl": true,
                "fanProfile": fan_name,
                "minimumFanspeed": min_s,
                "maximumFanspeed": max_s,
                "offsetFanspeed": off_s,
                "customFanCurve": rng.choose(&bad_values),
            },
            "nvidiaPowerCTRLProfile": { "cTGPOffset": ctgp }
        });
        let _ = merge_tcc(&json!({}), &json!({}), &json!([profile]), &presets);
    }
}

#[test]
fn fuzz_domain2_mechrevo_tcc_and_aquaris_import() {
    let mut rng = SplitMix64::new(0xD002_3333_9876_5432);
    let (base_settings, base_profiles, base_user, base_curves, base_hw) =
        sample_mechrevo_fixture();

    // Baseline check
    assert!(
        convert_mechrevo_tcc(&base_settings, &base_profiles, &base_user, &base_curves, &base_hw)
            .is_ok()
    );

    // Note: `import.rs:1468` computes `speed + off_fan` without saturating/checked add;
    // keep Mechrevo fan speed/offset fuzz bounds within i32 range so debug builds do not
    // panic on i64 overflow while still exercising extreme out-of-range values.
    let safe_extreme_ints: &[i64] = &[
        i32::MIN as i64,
        -1000,
        -100,
        -1,
        0,
        1,
        4,
        5,
        100,
        101,
        255,
        256,
        1000,
        i32::MAX as i64,
    ];
    let bad_macs = [
        "",
        "02:00:00:00:00:01",
        "AA:BB:CC:DD:EE:FF",
        "GG:00:00:00:00:01",
        "02:00:00:00:00",
        "02:00:00:00:00:01:02",
        "not-a-mac",
    ];

    for iter in 0..300 {
        let mut settings = base_settings.clone();
        let mut profiles = base_profiles.clone();
        let mut user = base_user.clone();
        let mut curves = base_curves.clone();
        let hw = base_hw.clone();

        match iter % 5 {
            0 => {
                // Mutate aquarisSaveState JSON fields
                let saved = json!({
                    "deviceUUID": *rng.choose(&bad_macs),
                    "red": *rng.choose(safe_extreme_ints),
                    "green": *rng.choose(safe_extreme_ints),
                    "blue": *rng.choose(safe_extreme_ints),
                    "ledMode": *rng.choose(safe_extreme_ints),
                    "ledOn": rng.next_bool(),
                    "pumpOn": rng.next_bool(),
                    "pumpVoltage": *rng.choose(safe_extreme_ints),
                    "fanOn": rng.next_bool(),
                    "fanDutyCycle": *rng.choose(safe_extreme_ints),
                });
                user["aquarisSaveState"] = json!(saved.to_string());
                if iter % 15 == 0 {
                    user["aquarisLastDeviceName"] = json!("UnsupportedCooler");
                }
            }
            1 => {
                // Corrupt aquarisSaveState JSON string itself
                let raw = user["aquarisSaveState"]
                    .as_str()
                    .unwrap_or("{}")
                    .as_bytes()
                    .to_vec();
                let mutated = mutate_bytes(&mut rng, &raw, &[&raw]);
                user["aquarisSaveState"] = json!(String::from_utf8_lossy(&mutated).into_owned());
            }
            2 => {
                // Extreme fan modifiers and curve speeds (within i32 bounds)
                if let Some(p) = profiles.get_mut(0).and_then(Value::as_object_mut) {
                    if let Some(fan) = p.get_mut("fan").and_then(Value::as_object_mut) {
                        fan.insert(
                            "minimumFanspeed".to_string(),
                            json!(*rng.choose(safe_extreme_ints)),
                        );
                        fan.insert(
                            "maximumFanspeed".to_string(),
                            json!(*rng.choose(safe_extreme_ints)),
                        );
                        fan.insert(
                            "offsetFanspeed".to_string(),
                            json!(*rng.choose(safe_extreme_ints)),
                        );
                    }
                }
                curves["tableCPU"] = json!([
                    { "temp": 0, "speed": *rng.choose(safe_extreme_ints) },
                    { "temp": 100, "speed": *rng.choose(safe_extreme_ints) }
                ]);
            }
            3 => {
                // Extreme keyboard brightness or missing states
                if rng.next_bool() {
                    let bright = *rng.choose(safe_extreme_ints);
                    settings["keyboardBacklightStates"] = json!([
                        { "brightness": bright, "red": 1, "green": 2, "blue": 3 }
                    ]);
                } else {
                    settings["keyboardBacklightStates"] = json!([]);
                }
            }
            _ => {
                // Unknown fan preset or duplicate profile IDs
                if rng.next_bool() {
                    if let Some(p) = profiles.get_mut(0).and_then(Value::as_object_mut) {
                        if let Some(fan) = p.get_mut("fan").and_then(Value::as_object_mut) {
                            fan.insert("fanProfile".to_string(), json!("UnreviewedPreset"));
                        }
                    }
                } else if let Some(arr) = profiles.as_array_mut() {
                    let dup = arr[0].clone();
                    arr.push(dup);
                }
            }
        }

        let _ = convert_mechrevo_tcc(&settings, &profiles, &user, &curves, &hw);
    }
}

// ============================================================================
// Domain 3: User-config INI / legacy QSettings & legacy.rs
// ============================================================================

#[test]
fn fuzz_domain3_ini_qsettings_rejection_and_legacy_json_corpus() {
    let mut rng = SplitMix64::new(0xD003_4444_5566_7788);
    let legacy_mgr = SettingsManager::default();

    // 1. INI / QSettings corpus (`uccrc`, `UniwillControlCenter`) — malformed sections,
    // missing `=`, duplicate sections/keys, boundary numbers, huge keys/values, invalid UTF-8.
    let huge_val = "X".repeat(32 * 1024);
    let huge_ini = format!("[General]\nhugeKey={huge_val}\n[{huge_val}]\nmissing_equals_line\n");

    let ini_seeds: Vec<Vec<u8>> = vec![
        b"[General]\ntheme=dark\nstartMinimized=true\n".to_vec(),
        b"[General]\n[General]\ntheme=light\ntheme=dark\n".to_vec(),
        b"[UnclosedSection\nno_equals_sign\n=empty_key\nkey=\n".to_vec(),
        concat!(
            "[UniwillControlCenter]\n",
            "chargeStartThreshold=99999999999999999999\n",
            "chargeEndThreshold=-1\n",
        )
        .as_bytes()
        .to_vec(),
        b"[Power]\x00\xFF\xFE\nkey\x01=val\x1B\x7F\n".to_vec(),
        huge_ini.into_bytes(),
    ];
    let ini_refs: Vec<&[u8]> = ini_seeds.iter().map(Vec::as_slice).collect();

    for iter in 0..400 {
        let base = ini_refs[iter % ini_refs.len()];
        let mutated = if iter < ini_refs.len() {
            base.to_vec()
        } else {
            mutate_bytes(&mut rng, base, &ini_refs)
        };
        let lossy = String::from_utf8_lossy(&mutated);

        // INI/QSettings text is not JSON and must never panic or silently parse as UccSettings.
        assert!(UccSettings::from_json(&lossy).is_err());
        assert!(ProfilesCatalog::from_json(&lossy).is_err());
        assert!(!is_valid_settings_content(&lossy));
        assert!(legacy_mgr.parse_settings_json(&lossy).is_none());
        assert!(LegacyProfileDto::parse_json(&lossy).is_err());
    }

    // 2. Legacy `SettingsManager` / `TccSettings` JSON fuzzing and round-trip invariants.
    let legacy_seed = r#"{
        "fahrenheit": true,
        "stateMap": {"power_ac": "p_ac", "power_bat": "p_bat"},
        "profiles": {
            "p_ac": {"id": "p_ac", "name": "AC Perf"},
            "p_bat": {"id": "p_bat", "name": "Bat Quiet"}
        },
        "customKeyboardProfiles": {"k1": {"id": "k1", "name": "K1"}},
        "customFanProfiles": {"f1": {"id": "f1", "name": "F1"}},
        "keyboardLayout": "frixit-mechrevo",
        "keyboardBacklightStates": [{"red": 1, "green": 2, "blue": 3, "brightness": 40}],
        "keyboardBacklightStates": [{"red": 5, "green": 6, "blue": 7, "brightness": 50}],
        "powerDevices": {"enabled": true},
        "lightbar": {"managed": true},
        "ycbcr420Workaround": [{"eDP-1": true, "HDMI-1": false}],
        "shutdownTime": "23:00",
        "chargingProfile": "stationary",
        "chargingPriority": "performance",
        "chargeType": "Custom",
        "chargeStartThreshold": 40,
        "chargeEndThreshold": 80
    }"#;

    let minimal_seed = r#"{}"#;
    let boundary_seed = r#"{
        "chargeStartThreshold": 9223372036854775807,
        "chargeEndThreshold": -9223372036854775808,
        "ycbcr420Workaround": [{"DP-1": "not-bool"}, 42, null],
        "stateMap": {"power_ac": 123, "power_bat": "ok"},
        "profiles": {"ok": "not-json-object"}
    }"#;

    let legacy_corpus: [&[u8]; 3] = [
        legacy_seed.as_bytes(),
        minimal_seed.as_bytes(),
        boundary_seed.as_bytes(),
    ];

    let mut legacy_ok_count = 0_usize;
    for _ in 0..600 {
        let base = *rng.choose(&legacy_corpus);
        let mutated = mutate_bytes(&mut rng, base, &legacy_corpus);
        let text = String::from_utf8_lossy(&mutated);

        if let Some(parsed) = legacy_mgr.parse_settings_json(&text) {
            legacy_ok_count += 1;
            let serialized = legacy_mgr
                .settings_to_json(&parsed)
                .expect("TccSettings must serialize");
            assert_eq!(
                serialized.matches("\"keyboardBacklightStates\"").count(),
                1,
                "legacy serializer must never emit duplicate keyboardBacklightStates"
            );
            let reparsed = legacy_mgr
                .parse_settings_json(&serialized)
                .expect("serialized TccSettings must reparse");
            assert_eq!(parsed.fahrenheit, reparsed.fahrenheit);
            assert_eq!(parsed.state_map, reparsed.state_map);
            assert_eq!(parsed.keyboard_layout, reparsed.keyboard_layout);

            for state in [
                ProfileState::Ac,
                ProfileState::Bat,
                ProfileState::TypeC,
                ProfileState::Wc,
            ] {
                let _ = legacy_mgr.resolve_startup_profile(
                    &parsed.state_map,
                    &parsed.profiles,
                    state,
                );
            }
        }
    }

    assert!(
        legacy_ok_count > 0,
        "legacy mutation corpus should exercise valid TccSettings parses"
    );

    // Verify `TccSettings` with malformed inner JSON strings still serializes safely.
    let mut broken_inner = TccSettings {
        keyboard_backlight_states: "{broken".to_string(),
        power_device_settings: "[broken".to_string(),
        lightbar_settings: "not json".to_string(),
        ..Default::default()
    };
    broken_inner
        .profiles
        .insert("p1".to_string(), "{{bad".to_string());
    broken_inner
        .custom_keyboard_profiles
        .insert("k1".to_string(), "bad".to_string());
    broken_inner
        .custom_fan_profiles
        .insert("f1".to_string(), "bad".to_string());
    let fallback_json = legacy_mgr
        .settings_to_json(&broken_inner)
        .expect("malformed inner JSON must fall back cleanly");
    assert!(legacy_mgr.parse_settings_json(&fallback_json).is_some());
}

// ============================================================================
// Domain 4: Persistence, backup recovery, symlink safety & platform_paths
// ============================================================================

#[test]
fn fuzz_domain4_backup_recovery_and_persistence_invariants() {
    let mut rng = SplitMix64::new(0xD004_6666_DEAD_BEEF);
    let tmp = TempDir::new().expect("create tempdir");

    let valid_v1 = create_default_settings();
    let mut valid_v2 = valid_v1.clone();
    valid_v2.charging_profile = Some("stationary".to_string());
    valid_v2.charge_start_threshold = Some(40);
    valid_v2.charge_end_threshold = Some(80);
    let valid_v1_bytes = valid_v1
        .to_json_pretty()
        .expect("serialize v1")
        .into_bytes();

    let corrupt_seeds: [&[u8]; 5] = [
        b"",
        b"{truncated",
        b"\xFF\xFE\x00\x01\x80",
        b"[General]\ntheme=dark\n",
        b"{\"profiles\": 42, \"stateMap\": []}",
    ];

    // 1. Corrupt primary + valid backup recovery across 150 deterministic mutations.
    for iter in 0..150 {
        let case_dir = tmp.path().join(format!("rec_{iter}"));
        fs::create_dir_all(&case_dir).expect("mkdir");
        let cfg_path = case_dir.join("settings.json");
        let mgr = ConfigManager::with_custom_path(&cfg_path);

        mgr.save(&valid_v1).expect("save v1");
        mgr.save(&valid_v2).expect("save v2 -> creates .bak and .backup");

        // Corrupt primary with deterministic mutated garbage (guaranteed invalid).
        let base = *rng.choose(&corrupt_seeds);
        let mut corrupt = mutate_bytes(&mut rng, base, &corrupt_seeds);
        if is_valid_settings_content(&String::from_utf8_lossy(&corrupt)) {
            corrupt = b"{corrupt_primary".to_vec();
        }
        fs::write(&cfg_path, &corrupt).expect("write corrupt primary");

        assert!(ConfigManager::load_from_path(&cfg_path).is_err());
        let (recovered, was_recovered) =
            ConfigManager::load_with_recovery(&cfg_path).expect("must recover from backup");
        assert!(was_recovered);
        assert_eq!(recovered, valid_v1);

        // Writing new valid content over a corrupt primary must NOT clobber the valid backup.
        write_atomic(&cfg_path, &valid_v1_bytes).expect("overwrite corrupt primary");
        let bak_path = case_dir.join("settings.json.bak");
        assert!(
            ConfigManager::load_from_path(&bak_path).is_ok(),
            "valid backup must not be overwritten with corrupt primary"
        );

        // Re-corrupt and verify explicit restore_from_backup repairs primary atomically.
        fs::write(&cfg_path, &corrupt).expect("re-corrupt primary");
        let restored =
            ConfigManager::restore_from_backup(&cfg_path).expect("restore from backup");
        assert_eq!(restored, valid_v1);
        assert_eq!(
            ConfigManager::load_from_path(&cfg_path).expect("load repaired primary"),
            valid_v1
        );
    }

    // 2. Legacy SettingsManager backup recovery across 100 deterministic mutations.
    for iter in 0..100 {
        let case_dir = tmp.path().join(format!("legacy_rec_{iter}"));
        fs::create_dir_all(&case_dir).expect("mkdir");
        let cfg_path = case_dir.join("settings");
        let legacy_mgr = SettingsManager::new(&cfg_path);

        let mut s1 = TccSettings {
            keyboard_layout: "iso".to_string(),
            ..Default::default()
        };
        legacy_mgr.write_settings(&s1).expect("write s1");
        s1.keyboard_layout = "ansi".to_string();
        legacy_mgr.write_settings(&s1).expect("write s2");

        let base = *rng.choose(&corrupt_seeds);
        let mut corrupt = mutate_bytes(&mut rng, base, &corrupt_seeds);
        if legacy_mgr
            .parse_settings_json(&String::from_utf8_lossy(&corrupt))
            .is_some()
        {
            corrupt = b"[General]\nnot_json=1\n".to_vec();
        }
        fs::write(&cfg_path, &corrupt).expect("corrupt legacy primary");

        let recovered = legacy_mgr
            .read_settings()
            .expect("legacy read_settings must recover from backup");
        assert_eq!(recovered.keyboard_layout, "iso");
    }

    // 3. Orphan temp file cleanup leaves legitimate files intact.
    let clean_dir = tmp.path().join("cleanup");
    fs::create_dir_all(&clean_dir).expect("mkdir cleanup");
    fs::write(clean_dir.join(".settings.json.tmp.1.1"), b"x").expect("write tmp");
    fs::write(clean_dir.join(".settings.json.tmp_restore.2.2"), b"y").expect("write tmp_restore");
    fs::write(clean_dir.join("settings.json"), &valid_v1_bytes).expect("write settings");
    fs::write(clean_dir.join("settings.json.bak"), &valid_v1_bytes).expect("write bak");
    let removed = cleanup_temp_files(&clean_dir).expect("cleanup");
    assert_eq!(removed, 2);
    assert!(clean_dir.join("settings.json").is_file());
    assert!(clean_dir.join("settings.json.bak").is_file());
    assert_eq!(
        cleanup_temp_files(tmp.path().join("nonexistent")).expect("nonexistent dir"),
        0
    );
}

#[test]
fn fuzz_domain4_symlink_and_platform_paths_safety() {
    let tmp = TempDir::new().expect("create tempdir");

    // 1. Symlink safety checks on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let inner_dir = tmp.path().join("inner");
        let outside_dir = tmp.path().join("outside");
        fs::create_dir_all(&inner_dir).expect("mkdir inner");
        fs::create_dir_all(&outside_dir).expect("mkdir outside");

        let valid_bytes = create_default_settings()
            .to_json_pretty()
            .expect("serialize")
            .into_bytes();

        // Broken symlink must be rejected.
        let broken_link = inner_dir.join("broken.json");
        symlink(inner_dir.join("does_not_exist.json"), &broken_link).expect("broken symlink");
        let err = write_atomic(&broken_link, &valid_bytes).expect_err("reject broken symlink");
        assert!(matches!(err, ConfigError::PermissionDenied(_)));

        // Escaping symlink pointing outside parent directory must be rejected.
        let outside_target = outside_dir.join("secret.json");
        fs::write(&outside_target, b"{}").expect("write outside target");
        let escape_link = inner_dir.join("escape.json");
        symlink(&outside_target, &escape_link).expect("escape symlink");
        let err = write_atomic(&escape_link, &valid_bytes).expect_err("reject escaping symlink");
        assert!(matches!(err, ConfigError::PermissionDenied(_)));
        assert_eq!(fs::read(&outside_target).expect("read outside"), b"{}");

        // restore_from_backup must refuse symlink target even when valid .bak exists.
        let in_dir_target = inner_dir.join("real.json");
        fs::write(&in_dir_target, b"{}").expect("write real");
        let restore_link = inner_dir.join("restore_target.json");
        symlink(&in_dir_target, &restore_link).expect("restore symlink");
        fs::write(inner_dir.join("restore_target.json.bak"), &valid_bytes).expect("write bak");
        let err = ConfigManager::restore_from_backup(&restore_link)
            .expect_err("restore_from_backup must refuse symlink");
        assert!(matches!(err, ConfigError::PermissionDenied(_)));
    }

    // 2. Deterministic fuzzing of `resolve_platform_paths` (including `sleep_state_file`).
    let mut rng = SplitMix64::new(0xD004_7777_1357_2468);
    let long_dir = format!("/tmp/{}", "a".repeat(512));
    let env_candidates: [&str; 10] = [
        "",
        "   ",
        "\t\n",
        "/home/user/.config",
        "/tmp/with spaces/dir",
        "../relative/../../escape",
        r"C:\ProgramData",
        r"C:\Users\Tester\AppData\Roaming",
        "/dev/null",
        long_dir.as_str(),
    ];

    for _ in 0..500 {
        let platform = if rng.next_bool() {
            PathPlatform::Unix
        } else {
            PathPlatform::Windows
        };
        let xdg = if rng.next_bool() {
            Some((*rng.choose(&env_candidates)).to_string())
        } else {
            None
        };
        let home = if rng.next_bool() {
            Some((*rng.choose(&env_candidates)).to_string())
        } else {
            None
        };
        let prog_data = if rng.next_bool() {
            Some((*rng.choose(&env_candidates)).to_string())
        } else {
            None
        };
        let app_data = if rng.next_bool() {
            Some((*rng.choose(&env_candidates)).to_string())
        } else {
            None
        };
        let user_prof = if rng.next_bool() {
            Some((*rng.choose(&env_candidates)).to_string())
        } else {
            None
        };

        let paths = resolve_platform_paths(platform, |key| match key {
            "XDG_CONFIG_HOME" => xdg.clone(),
            "HOME" => home.clone(),
            "PROGRAMDATA" => prog_data.clone(),
            "APPDATA" => app_data.clone(),
            "USERPROFILE" => user_prof.clone(),
            _ => None,
        });

        assert_eq!(
            paths.system_settings_file.file_name().and_then(|s| s.to_str()),
            Some("settings.json")
        );
        assert_eq!(
            paths.sleep_state_file.file_name().and_then(|s| s.to_str()),
            Some("sleep-state.json")
        );
        assert!(!paths.user_config_dir.as_os_str().is_empty());
        assert!(!paths.runtime_dir.as_os_str().is_empty());
        assert!(Path::new(&paths.sleep_state_file).starts_with(&paths.runtime_dir));
    }
}
