// SPDX-License-Identifier: GPL-3.0-or-later
//! Reproduction integration tests for confirmed `ucc-core` findings on commit `1d07126`:
//! - R01: `FanControlLogic` emits fan speeds below `fans_min_speed_hw_limit` when fan-off is
//!   unavailable and during post-EWMA transitions `0% -> min%`.
//! - N18: Direct `UccProfile` deserialization rejects legacy charging sentinels (`-1`, `""`).

use serde_json::json;
use ucc_core::{FanControlLogic, FanLogicType, FanPoint, FanProfile, UccProfile};

#[test]
fn r01_fan_control_logic_never_drops_below_min_speed_when_off_unavailable() {
    const MIN_SPEED: i32 = 20;

    // 1. With fans_off_available = false, every curve target in 0..=100 must yield >= min_speed.
    for target_speed in 0u8..=100u8 {
        let mut profile = FanProfile::new("r01-const", "R01 Constant");
        profile.table_cpu = vec![
            FanPoint::new(0, target_speed),
            FanPoint::new(100, target_speed),
        ];
        profile.table_gpu = profile.table_cpu.clone();

        let mut logic = FanControlLogic::new(profile, FanLogicType::Cpu);
        logic.set_fans_min_speed_hw_limit(MIN_SPEED);
        logic.set_fans_off_available(false);

        let emitted = logic.report_temperature(50);
        assert!(
            emitted >= MIN_SPEED as u8,
            "R01: with min={MIN_SPEED}% and fans_off_available=false, curve speed {target_speed}% \
             produced {emitted}% (below hardware minimum)"
        );
    }

    // 2. With fans_off_available = true, dynamic transitions 0% -> 20% and 20% -> 0% after EWMA
    //    smoothing must never emit forbidden intermediate speeds 1..=19%.
    let mut profile = FanProfile::new("r01-ramp", "R01 Ramp");
    profile.table_cpu = vec![FanPoint::new(30, 0), FanPoint::new(60, 20)];
    profile.table_gpu = profile.table_cpu.clone();

    let mut logic = FanControlLogic::new(profile, FanLogicType::Cpu);
    logic.set_fans_min_speed_hw_limit(MIN_SPEED);
    logic.set_fans_off_available(true);

    let initial = logic.report_temperature(30);
    assert_eq!(initial, 0, "Initial fan speed at 30°C should be 0%");

    for tick in 1..=20 {
        let emitted = logic.report_temperature(60);
        assert!(
            emitted == 0 || emitted >= MIN_SPEED as u8,
            "R01: post-EWMA rising transition 0% -> 20% emitted forbidden speed {emitted}% on tick {tick}"
        );
    }

    for tick in 1..=30 {
        let emitted = logic.report_temperature(30);
        assert!(
            emitted == 0 || emitted >= MIN_SPEED as u8,
            "R01: post-EWMA falling transition 20% -> 0% emitted forbidden speed {emitted}% on tick {tick}"
        );
    }
}

#[test]
fn n18_ucc_profile_accepts_legacy_charging_sentinels() {
    let legacy_payload = json!({
        "id": "legacy-charging-sentinels",
        "name": "Legacy Charging Sentinels",
        "chargeStartThreshold": -1,
        "chargeEndThreshold": -1,
        "chargeType": ""
    });

    let profile: UccProfile = serde_json::from_value(legacy_payload).expect(
        "N18: UccProfile must accept legacy charging sentinels (-1 thresholds and empty chargeType)",
    );

    assert_eq!(
        profile.charging.start_threshold, None,
        "chargeStartThreshold = -1 sentinel must deserialize as None"
    );
    assert_eq!(
        profile.charging.end_threshold, None,
        "chargeEndThreshold = -1 sentinel must deserialize as None"
    );
    assert_eq!(
        profile.charging.charge_type, None,
        "chargeType = \"\" sentinel must deserialize as None"
    );
    assert!(
        profile.validate().is_ok(),
        "Deserialized profile with legacy charging sentinels must pass validation"
    );
}
