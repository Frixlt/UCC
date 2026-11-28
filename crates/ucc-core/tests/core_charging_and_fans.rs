// SPDX-License-Identifier: GPL-3.0-or-later
//! Reproduction integration tests for confirmed `ucc-core` findings on commit `1d07126`:
//! - R01: `FanControlLogic` emits fan speeds below `fans_min_speed_hw_limit` when fan-off is
//!   unavailable and during post-EWMA transitions `0% -> min%`.
//! - N18: Direct `UccProfile` deserialization rejects legacy charging sentinels (`-1`, `""`).

use serde_json::json;
use ucc_core::{
    FanControlLogic, FanLogicType, FanPoint, FanProfile, PumpPoint, TemporaryFanCurves, UccProfile,
};

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

#[test]
fn temporary_fan_curves_apply_to_profile_cpu_gpu_wc_and_pump() {
    let mut base = FanProfile::new("base-id", "Base Profile");
    base.table_cpu = vec![FanPoint::new(40, 20), FanPoint::new(80, 70)];
    base.table_gpu = vec![FanPoint::new(40, 25), FanPoint::new(80, 75)];
    base.table_water_cooler_fan = vec![FanPoint::new(40, 15), FanPoint::new(80, 60)];
    base.table_pump = vec![PumpPoint::new(40, 1), PumpPoint::new(75, 2)];

    let full_override = TemporaryFanCurves {
        cpu: vec![FanPoint::new(35, 30), FanPoint::new(85, 95)],
        gpu: vec![FanPoint::new(35, 40), FanPoint::new(85, 100)],
        water_cooler_fan: vec![FanPoint::new(35, 50), FanPoint::new(85, 90)],
        pump: vec![PumpPoint::new(45, 3)],
    };

    // fan_index == 0 overrides CPU, WC fan, and pump while keeping GPU intact.
    let applied_cpu = full_override.apply_to_profile(&base, 0);
    assert_eq!(applied_cpu.id, base.id);
    assert_eq!(applied_cpu.name, base.name);
    assert_eq!(applied_cpu.table_cpu, full_override.cpu);
    assert_eq!(applied_cpu.table_gpu, base.table_gpu);
    assert_eq!(
        applied_cpu.table_water_cooler_fan,
        full_override.water_cooler_fan
    );
    assert_eq!(applied_cpu.table_pump, full_override.pump);

    // fan_index > 0 overrides GPU, WC fan, and pump while keeping CPU intact.
    for fan_index in [1usize, 2usize] {
        let applied_gpu = full_override.apply_to_profile(&base, fan_index);
        assert_eq!(applied_gpu.table_cpu, base.table_cpu);
        assert_eq!(applied_gpu.table_gpu, full_override.gpu);
        assert_eq!(
            applied_gpu.table_water_cooler_fan,
            full_override.water_cooler_fan
        );
        assert_eq!(applied_gpu.table_pump, full_override.pump);
    }

    // Empty water_cooler_fan and pump preserve base WC/pump tables.
    let cpu_gpu_only = TemporaryFanCurves {
        cpu: vec![FanPoint::new(50, 55)],
        gpu: vec![FanPoint::new(50, 65)],
        water_cooler_fan: Vec::new(),
        pump: Vec::new(),
    };
    let merged_0 = cpu_gpu_only.apply_to_profile(&base, 0);
    assert_eq!(merged_0.table_cpu, cpu_gpu_only.cpu);
    assert_eq!(merged_0.table_gpu, base.table_gpu);
    assert_eq!(merged_0.table_water_cooler_fan, base.table_water_cooler_fan);
    assert_eq!(merged_0.table_pump, base.table_pump);

    let merged_1 = cpu_gpu_only.apply_to_profile(&base, 1);
    assert_eq!(merged_1.table_cpu, base.table_cpu);
    assert_eq!(merged_1.table_gpu, cpu_gpu_only.gpu);
    assert_eq!(merged_1.table_water_cooler_fan, base.table_water_cooler_fan);
    assert_eq!(merged_1.table_pump, base.table_pump);

    // Empty cpu/gpu with non-empty WC/pump leaves CPU and GPU untouched for any fan_index.
    let wc_pump_only = TemporaryFanCurves {
        cpu: Vec::new(),
        gpu: Vec::new(),
        water_cooler_fan: vec![FanPoint::new(40, 45), FanPoint::new(80, 85)],
        pump: vec![PumpPoint::new(50, 2)],
    };
    for fan_index in [0usize, 1usize] {
        let merged = wc_pump_only.apply_to_profile(&base, fan_index);
        assert_eq!(merged.table_cpu, base.table_cpu);
        assert_eq!(merged.table_gpu, base.table_gpu);
        assert_eq!(merged.table_water_cooler_fan, wc_pump_only.water_cooler_fan);
        assert_eq!(merged.table_pump, wc_pump_only.pump);
    }

    // Completely empty temporary curves return an exact clone of base.
    let empty = TemporaryFanCurves::default();
    assert_eq!(empty.apply_to_profile(&base, 0), base);
    assert_eq!(empty.apply_to_profile(&base, 1), base);
}

#[test]
fn fan_profile_water_cooler_fan_speed_fallback_and_empty_tables() {
    // 1. All tables empty -> returns -1.
    let empty = FanProfile::new("empty-wc", "Empty WC");
    for temp in [0, 45, 95] {
        assert_eq!(empty.get_water_cooler_fan_speed_for_temp(temp), -1);
    }

    // 2. Only CPU table populated -> falls back to CPU speed (including 0% over -1).
    let mut cpu_only = FanProfile::new("cpu-only", "CPU Only");
    cpu_only.table_cpu = vec![FanPoint::new(30, 0), FanPoint::new(70, 80)];
    assert_eq!(cpu_only.get_water_cooler_fan_speed_for_temp(25), 0);
    assert_eq!(cpu_only.get_water_cooler_fan_speed_for_temp(50), 40);
    assert_eq!(cpu_only.get_water_cooler_fan_speed_for_temp(80), 80);

    // 3. Only GPU table populated -> falls back to GPU speed.
    let mut gpu_only = FanProfile::new("gpu-only", "GPU Only");
    gpu_only.table_gpu = vec![FanPoint::new(40, 30), FanPoint::new(80, 90)];
    assert_eq!(gpu_only.get_water_cooler_fan_speed_for_temp(40), 30);
    assert_eq!(gpu_only.get_water_cooler_fan_speed_for_temp(60), 60);

    // 4. Both CPU and GPU populated, WC empty -> max(cpu, gpu) across crossing curves.
    let mut fallback = FanProfile::new("fallback-wc", "Fallback WC");
    fallback.table_cpu = vec![FanPoint::new(40, 20), FanPoint::new(80, 90)];
    fallback.table_gpu = vec![FanPoint::new(40, 50), FanPoint::new(80, 60)];
    // At 40C: cpu=20, gpu=50 -> 50
    assert_eq!(fallback.get_water_cooler_fan_speed_for_temp(40), 50);
    // At 60C: cpu=55, gpu=55 -> 55
    assert_eq!(fallback.get_water_cooler_fan_speed_for_temp(60), 55);
    // At 80C: cpu=90, gpu=60 -> 90
    assert_eq!(fallback.get_water_cooler_fan_speed_for_temp(80), 90);

    // 5. Explicit WC fan table overrides CPU/GPU fallback even when lower than both.
    fallback.table_water_cooler_fan = vec![FanPoint::new(40, 10), FanPoint::new(80, 30)];
    assert_eq!(fallback.get_water_cooler_fan_speed_for_temp(20), 10);
    assert_eq!(fallback.get_water_cooler_fan_speed_for_temp(60), 20);
    assert_eq!(fallback.get_water_cooler_fan_speed_for_temp(95), 30);
}

#[test]
fn fan_control_logic_empty_curve_critical_temp_and_profile_hot_swap() {
    // 1. Empty curve at 95C (above OVERHEAT_TEMPERATURE_C): checked API returns None,
    //    unchecked returns 0, while the temperature filter still tracks 95C.
    let mut logic = FanControlLogic::new(FanProfile::new("empty", "Empty"), FanLogicType::Cpu);
    assert_eq!(logic.report_temperature_checked(95), None);
    assert_eq!(logic.filtered_temperature(), 95);
    assert_eq!(logic.speed_percent(), 0);
    assert_eq!(logic.report_temperature(95), 0);

    // Hot-swapping from empty profile to a populated profile immediately yields overheat 100%
    // because the existing filtered temperature is already 95C.
    let mut low_curve = FanProfile::new("low", "Low Curve");
    low_curve.table_cpu = vec![FanPoint::new(30, 10), FanPoint::new(80, 40)];
    logic.update_fan_profile(low_curve.clone());
    assert_eq!(logic.fan_profile().id, "low");
    assert_eq!(logic.report_temperature_checked(95), Some(100));
    assert_eq!(logic.speed_percent(), 100);

    // 2. Step response 40C -> 95C: crosses CRITICAL_TEMPERATURE_C (85C -> >=80%)
    //    and OVERHEAT_TEMPERATURE_C (90C -> 100%) despite curve cap at 40%.
    let mut step_logic = FanControlLogic::new(low_curve, FanLogicType::Cpu);
    let at_40 = step_logic
        .report_temperature_checked(40)
        .expect("non-empty curve");
    assert_eq!(step_logic.filtered_temperature(), 40);
    assert_eq!(at_40, 16);

    let mut reached_critical = false;
    let mut reached_overheat = false;
    for _ in 0..8 {
        let spd = step_logic
            .report_temperature_checked(95)
            .expect("non-empty curve");
        let filtered = step_logic.filtered_temperature();
        if filtered >= FanControlLogic::OVERHEAT_TEMPERATURE_C {
            assert_eq!(spd, 100, "overheat (>=90C) must force 100%");
            reached_overheat = true;
            break;
        } else if filtered >= FanControlLogic::CRITICAL_TEMPERATURE_C {
            assert!(
                spd >= 80,
                "critical temp (>=85C) must force >=80%, got {spd}"
            );
            reached_critical = true;
        }
    }
    assert!(
        reached_critical,
        "40C -> 95C step should pass through critical band [85C, 90C)"
    );
    assert!(
        reached_overheat,
        "40C -> 95C step should reach overheat band >=90C"
    );

    // 3. Hot-swap between two non-empty profiles at normal temperature (50C) updates target smoothly.
    let mut quiet = FanProfile::new("quiet", "Quiet");
    quiet.table_cpu = vec![FanPoint::new(30, 20), FanPoint::new(80, 20)];
    let mut loud = FanProfile::new("loud", "Loud");
    loud.table_cpu = vec![FanPoint::new(30, 80), FanPoint::new(80, 80)];

    let mut swap_logic = FanControlLogic::new(quiet, FanLogicType::Cpu);
    assert_eq!(swap_logic.report_temperature_checked(50), Some(20));

    swap_logic.update_fan_profile(loud);
    assert_eq!(swap_logic.fan_profile().id, "loud");
    let first_after_swap = swap_logic
        .report_temperature_checked(50)
        .expect("populated swapped profile");
    assert!(
        first_after_swap > 20 && first_after_swap <= 80,
        "hot-swap to 80% curve should immediately ramp speed upward, got {first_after_swap}"
    );

    // Hot-swapping back to an empty profile returns None and preserves previous speed_percent.
    swap_logic.update_fan_profile(FanProfile::new("cleared", "Cleared"));
    assert_eq!(swap_logic.report_temperature_checked(50), None);
    assert_eq!(swap_logic.speed_percent(), first_after_swap);
}
