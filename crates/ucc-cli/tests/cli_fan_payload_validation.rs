// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2-17 in `ucc-cli`:
//! - `FanAction::Set` uses `is_empty_fan_profile_payload()` consistently with `FanAction::Get`.

use std::fs;
use std::path::Path;
use ucc_cli::commands::fan::is_empty_fan_profile_payload;

#[test]
fn v2_p2_17_is_empty_fan_profile_payload_detects_empty_and_whitespace_objects() {
    assert!(is_empty_fan_profile_payload(None));
    assert!(is_empty_fan_profile_payload(Some("")));
    assert!(is_empty_fan_profile_payload(Some("   \n\t ")));
    assert!(is_empty_fan_profile_payload(Some("{}")));
    assert!(is_empty_fan_profile_payload(Some(" { } \n")));
    assert!(!is_empty_fan_profile_payload(Some(
        r#"{"tableCPU":[{"temp":50,"speed":40}]}"#
    )));
    assert!(!is_empty_fan_profile_payload(Some(
        r#"{"cpu":[{"temp":50,"speed":40}]}"#
    )));
}

#[test]
fn v2_p2_17_fan_set_uses_is_empty_fan_profile_payload() {
    let fan_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/fan.rs"))
            .expect("read src/commands/fan.rs");
    let set_start = fan_src
        .find("FanAction::Set { id } =>")
        .expect("FanAction::Set must exist");
    let apply_start = fan_src
        .find("FanAction::Apply { json } =>")
        .expect("FanAction::Apply must exist");
    let set_body = &fan_src[set_start..apply_start];

    assert!(
        !set_body.contains("found_json.as_deref() == Some(\"{}\")"),
        "V2-P2-17: FanAction::Set must not compare raw Some(\"{{}}\") directly"
    );
    assert!(
        set_body.matches("is_empty_fan_profile_payload(").count() >= 3,
        "V2-P2-17: FanAction::Set must use is_empty_fan_profile_payload for builtin fallback, final check, and apply payload"
    );
}
