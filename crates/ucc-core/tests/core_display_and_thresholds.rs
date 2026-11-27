// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2 core fixes:
//! - Per-profile `ProfileDisplay.lid_close_action` (`lidCloseAction`)
//! - Public `ucc_core::charging::deserialize_opt_threshold`

use serde::Deserialize;
use serde_json::json;
use ucc_core::{ProfileDisplay, UccProfile};

#[test]
fn profile_display_lid_close_action_defaults_and_roundtrips() {
    let default_display = ProfileDisplay::default();
    assert_eq!(default_display.lid_close_action, None);

    let serialized_default = serde_json::to_string(&default_display).expect("serialize default");
    assert!(
        !serialized_default.contains("lidCloseAction"),
        "None lid_close_action must be omitted during serialization: {serialized_default}"
    );

    let parsed: ProfileDisplay = serde_json::from_value(json!({
        "brightness": 80,
        "useBrightness": true,
        "lidCloseAction": "ignore"
    }))
    .expect("deserialize ProfileDisplay with lidCloseAction");
    assert_eq!(parsed.lid_close_action.as_deref(), Some("ignore"));
    assert_eq!(parsed.brightness, 80);
    assert!(parsed.use_brightness);

    let roundtrip = serde_json::to_value(&parsed).expect("serialize ProfileDisplay");
    assert_eq!(roundtrip.get("lidCloseAction"), Some(&json!("ignore")));

    let profile: UccProfile = serde_json::from_value(json!({
        "id": "lid-test",
        "name": "Lid Test",
        "display": {
            "lidCloseAction": "screen_off"
        }
    }))
    .expect("deserialize UccProfile with display.lidCloseAction");
    assert_eq!(
        profile.display.lid_close_action.as_deref(),
        Some("screen_off")
    );
}

#[derive(Debug, Deserialize)]
struct ThresholdHolder {
    #[serde(
        default,
        deserialize_with = "ucc_core::charging::deserialize_opt_threshold"
    )]
    threshold: Option<u8>,
}

#[test]
fn deserialize_opt_threshold_is_public_and_handles_sentinels() {
    let sentinel: ThresholdHolder =
        serde_json::from_value(json!({ "threshold": -1 })).expect("deserialize -1");
    assert_eq!(sentinel.threshold, None);

    let valid: ThresholdHolder =
        serde_json::from_value(json!({ "threshold": 80 })).expect("deserialize 80");
    assert_eq!(valid.threshold, Some(80));

    let missing: ThresholdHolder =
        serde_json::from_value(json!({})).expect("deserialize missing");
    assert_eq!(missing.threshold, None);

    let out_of_range = serde_json::from_value::<ThresholdHolder>(json!({ "threshold": 150 }));
    assert!(out_of_range.is_err());
}
