use ucc_core::{
    FanControlLogic, FanCurve, FanLogicType, FanPoint, FanProfile, LedSettings, LegacyProfileDto,
    PowerPolicy, PumpPoint, UccProfile,
};

#[test]
fn fan_curve_exact_interpolation_avoids_ieee754_1ulp_drift() {
    // In IEEE-754 f64: 10.0 + (1.0 / 6.0) * 3.0 == 10.499999999999998 -> .round() == 10.
    // Exact rational interpolation is 10.5 -> round-half-away-from-zero == 11.
    let curve = FanCurve::new(vec![FanPoint::new(0, 10), FanPoint::new(6, 13)]).unwrap();
    assert_eq!(curve.get_speed_for_temp(1), 11);

    // Another classic 1-ULP case: (0, 20) -> (6, 23) at temp = 1 -> 20.5 -> 21.
    let curve2 = FanCurve::new(vec![FanPoint::new(0, 20), FanPoint::new(6, 23)]).unwrap();
    assert_eq!(curve2.get_speed_for_temp(1), 21);

    // Exact endpoint equality
    let flat = FanCurve::new(vec![FanPoint::new(30, 42), FanPoint::new(80, 42)]).unwrap();
    assert_eq!(flat.get_speed_for_temp(55), 42);
}

#[test]
fn fan_profile_is_valid_checks_curve_validity() {
    let mut profile = FanProfile::new("valid-check", "Valid Check");
    profile.table_cpu = vec![FanPoint::new(30, 20), FanPoint::new(70, 80)];
    profile.table_gpu = vec![FanPoint::new(30, 25), FanPoint::new(70, 85)];
    assert!(profile.is_valid());

    // Non-monotonic CPU curve must make is_valid() return false even though non-empty.
    let mut bad_cpu = profile.clone();
    bad_cpu.table_cpu = vec![FanPoint::new(70, 20), FanPoint::new(30, 80)];
    assert!(!bad_cpu.is_valid());

    // Out-of-range GPU speed must make is_valid() return false.
    let mut bad_gpu = profile.clone();
    bad_gpu.table_gpu = vec![FanPoint::new(30, 25), FanPoint::new(70, 105)];
    assert!(!bad_gpu.is_valid());

    // Invalid pump point must make is_valid() return false.
    let mut bad_pump = profile.clone();
    bad_pump.table_pump = vec![PumpPoint::new(50, 9)];
    assert!(!bad_pump.is_valid());
}

#[test]
fn cooling_hw_min_1_allows_fan_off_when_available() {
    let mut profile = FanProfile::new("hw-min-1", "HW Min 1");
    profile.table_cpu = vec![FanPoint::new(30, 0), FanPoint::new(80, 80)];
    profile.table_gpu = profile.table_cpu.clone();

    // When fans_off_available = true and hw_min = 1, requesting 0% must turn the fan off (0%).
    let mut ctrl = FanControlLogic::new(profile.clone(), FanLogicType::Cpu);
    ctrl.set_fans_min_speed_hw_limit(1);
    ctrl.set_fans_off_available(true);
    assert_eq!(ctrl.report_temperature_checked(30), Some(0));

    // When fans_off_available = false and hw_min = 1, requesting 0% must clamp to 1%.
    let mut ctrl_no_off = FanControlLogic::new(profile, FanLogicType::Cpu);
    ctrl_no_off.set_fans_min_speed_hw_limit(1);
    ctrl_no_off.set_fans_off_available(false);
    assert_eq!(ctrl_no_off.report_temperature_checked(30), Some(1));
}

#[test]
fn cooling_critical_temp_hysteresis_prevents_85_84_whipsaw() {
    let mut profile = FanProfile::new("low-curve", "Low Curve");
    profile.table_cpu = vec![FanPoint::new(30, 20), FanPoint::new(90, 40)];
    profile.table_gpu = profile.table_cpu.clone();

    let mut ctrl = FanControlLogic::new(profile, FanLogicType::Cpu);

    // Hit 85°C critical threshold -> forces >= 80%.
    let at_85 = ctrl.report_temperature_checked(85).unwrap();
    assert!(at_85 >= 80, "expected >=80% at 85°C, got {at_85}%");

    // Oscillating 84°C / 85°C must NOT whipsaw back down to 40% while inside the 3°C hysteresis band.
    for _ in 0..5 {
        let s84 = ctrl.report_temperature_checked(84).unwrap();
        assert!(
            s84 >= 80,
            "84°C within hysteresis band after 85°C must hold >=80%, got {s84}%"
        );
        let s85 = ctrl.report_temperature_checked(85).unwrap();
        assert!(s85 >= 80, "85°C must hold >=80%, got {s85}%");
    }

    // Once cooled well below the hysteresis band (e.g. 60°C), speed ramps down smoothly via EWMA
    // instead of cliff-dropping straight to 30% on the first tick below hysteresis.
    let mut prev = ctrl.speed_percent();
    let mut dropped_below_80 = false;
    for _ in 0..30 {
        let cur = ctrl.report_temperature_checked(60).unwrap();
        if cur < 80 {
            dropped_below_80 = true;
            assert!(
                prev - cur <= 10,
                "speed should ramp down smoothly after exiting critical hysteresis, jumped {prev}% -> {cur}%"
            );
        }
        prev = cur;
    }
    assert!(
        dropped_below_80,
        "fan should eventually ramp down after cooling to 60°C"
    );
}

#[test]
fn power_policy_bounds_owner_length_to_256_bytes() {
    let mut policy = PowerPolicy::new();
    let owner_ok = "a".repeat(256);
    let owner_too_long = "a".repeat(257);

    assert!(policy.register_request(&owner_ok, "app", "reason", "ac"));
    assert!(!policy.register_request(&owner_too_long, "app", "reason", "ac"));
    assert_eq!(policy.requests().len(), 1);
}

#[test]
fn led_settings_valid_modes_single_source_of_truth() {
    for anim in LedSettings::VALID_ANIMATIONS {
        assert!(
            LedSettings::VALID_MODES.contains(&anim),
            "VALID_MODES must include animation {anim}"
        );
    }
    for mode in LedSettings::VALID_MODES {
        let s = LedSettings::new(255, 10, 20, 30, Some(mode.to_string()));
        assert!(s.validate().is_ok(), "mode {mode} should validate");
    }
    let bad = LedSettings::new(255, 10, 20, 30, Some("NotARealMode".to_string()));
    assert!(bad.validate().is_err());
}

#[test]
fn borrowed_legacy_profile_dto_to_json_matches_from_profile_exactly() {
    let mut profile = UccProfile::new("p-ref", "Borrowed Wire Test");
    profile.description = "Exact wire parity".to_string();
    profile.cpu.online_cores = Some(8);
    profile.cpu.enabled_cores = Some(vec![0, 1, 2, 3]);
    profile.cpu.min_freq_khz = Some(800_000);
    profile.cpu.max_freq_khz = Some(4_200_000);
    profile.cpu.governor = "powersave".to_string();
    profile.cpu.energy_performance_preference = "balance_power".to_string();
    profile.fan.manual_fan_speed = Some(55);
    profile.fan.manual_pump_voltage = Some(2);
    profile.fan.table_cpu = vec![FanPoint::new(40, 20), FanPoint::new(80, 80)];
    profile.fan.table_pump = vec![PumpPoint::new(40, 1), PumpPoint::new(70, 3)];
    profile.odm_profile_name = Some("balanced".to_string());
    profile.odm_power_limits = vec![35, 55, 80];
    profile.nvidia_ctgp_offset = Some(10);
    profile.charging.charging_profile = Some("balanced".to_string());
    profile.charging.start_threshold = Some(40);
    profile.charging.end_threshold = Some(80);

    let via_ref = LegacyProfileDto::to_json(&profile).expect("to_json");
    let via_owned = serde_json::to_string(&LegacyProfileDto::from_profile(&profile))
        .expect("from_profile serialize");
    assert_eq!(via_ref, via_owned);
}
