use ucc_core::*;

#[test]
fn test_fan_curve_monotonicity_valid() {
    let curve = FanCurve::new(vec![
        FanPoint::new(30, 20),
        FanPoint::new(50, 40),
        FanPoint::new(70, 60),
        FanPoint::new(90, 100),
    ]);
    assert!(curve.is_ok());
    assert!(curve.unwrap().validate().is_ok());
}

#[test]
fn test_fan_curve_temperature_non_monotonic_fails() {
    // Decreasing temperature
    let curve = FanCurve::new(vec![
        FanPoint::new(50, 20),
        FanPoint::new(40, 40),
    ]);
    assert_eq!(
        curve.unwrap_err(),
        ValidationError::NonMonotonicTemperature {
            index: 1,
            prev: 50,
            curr: 40,
        }
    );

    // Duplicate temperature
    let curve = FanCurve::new(vec![
        FanPoint::new(50, 20),
        FanPoint::new(50, 40),
    ]);
    assert_eq!(
        curve.unwrap_err(),
        ValidationError::NonMonotonicTemperature {
            index: 1,
            prev: 50,
            curr: 50,
        }
    );
}

#[test]
fn test_fan_curve_speed_non_monotonic_fails() {
    // Fan speed decreasing when temperature increases
    let curve = FanCurve::new(vec![
        FanPoint::new(40, 50),
        FanPoint::new(60, 30),
    ]);
    assert_eq!(
        curve.unwrap_err(),
        ValidationError::NonMonotonicSpeed {
            index: 1,
            prev: 50,
            curr: 30,
        }
    );
}

#[test]
fn test_fan_curve_invalid_speed_or_empty() {
    let empty = FanCurve::new(vec![]);
    assert_eq!(empty.unwrap_err(), ValidationError::EmptyFanCurve);

    let invalid_speed = FanCurve::new(vec![FanPoint::new(30, 101)]);
    assert_eq!(
        invalid_speed.unwrap_err(),
        ValidationError::InvalidFanSpeed { speed: 101 }
    );
}

#[test]
fn test_fan_curve_interpolation() {
    let curve = FanCurve::new(vec![
        FanPoint::new(30, 20),
        FanPoint::new(50, 40),
        FanPoint::new(70, 60),
        FanPoint::new(90, 100),
    ])
    .unwrap();

    // Below first -> clamp to first (20%)
    assert_eq!(curve.get_speed_for_temp(10), 20);
    // Exact first point
    assert_eq!(curve.get_speed_for_temp(30), 20);
    // Midpoint between 30 (20%) and 50 (40%) -> 40 deg is 30%
    assert_eq!(curve.get_speed_for_temp(40), 30);
    // Quarter interpolation: 35 deg = 20 + 0.25 * 20 = 25%
    assert_eq!(curve.get_speed_for_temp(35), 25);
    // Exact mid point
    assert_eq!(curve.get_speed_for_temp(50), 40);
    // Exact last point
    assert_eq!(curve.get_speed_for_temp(90), 100);
    // Beyond last point -> clamp to last (100%)
    assert_eq!(curve.get_speed_for_temp(105), 100);
}

#[test]
fn test_discrete_pump_codes_accepted_and_rejected() {
    // Accepted discrete pump codes: 0, 2, 3, 4
    assert_eq!(PumpVoltage::from_discrete_code(0), Ok(PumpVoltage::V11));
    assert_eq!(PumpVoltage::from_discrete_code(2), Ok(PumpVoltage::V7));
    assert_eq!(PumpVoltage::from_discrete_code(3), Ok(PumpVoltage::V8));
    assert_eq!(PumpVoltage::from_discrete_code(4), Ok(PumpVoltage::Off));

    // Code 1 (12V) is strictly forbidden and rejected
    assert_eq!(
        PumpVoltage::from_discrete_code(1),
        Err(ValidationError::InvalidPumpVoltage(1))
    );

    // Negative and higher codes are rejected
    assert_eq!(
        PumpVoltage::from_discrete_code(-1),
        Err(ValidationError::InvalidPumpVoltage(-1))
    );
    assert_eq!(
        PumpVoltage::from_discrete_code(5),
        Err(ValidationError::InvalidPumpVoltage(5))
    );
    assert_eq!(
        PumpVoltage::from_discrete_code(42),
        Err(ValidationError::InvalidPumpVoltage(42))
    );

    // Helper predicate
    assert!(PumpVoltage::is_valid_manual_code(0));
    assert!(!PumpVoltage::is_valid_manual_code(1));
    assert!(PumpVoltage::is_valid_manual_code(2));
    assert!(PumpVoltage::is_valid_manual_code(3));
    assert!(PumpVoltage::is_valid_manual_code(4));
    assert!(!PumpVoltage::is_valid_manual_code(5));
}

#[test]
fn test_pump_step_wise_lookup() {
    let table = vec![
        PumpPoint::new(40, 1), // V7
        PumpPoint::new(60, 2), // V8
        PumpPoint::new(80, 3), // V11
    ];

    // Below first -> Off
    assert_eq!(get_pump_speed_for_temp(&table, 30), PumpVoltage::Off);
    // Exact first -> V7
    assert_eq!(get_pump_speed_for_temp(&table, 40), PumpVoltage::V7);
    // Mid step -> V7
    assert_eq!(get_pump_speed_for_temp(&table, 50), PumpVoltage::V7);
    // Next step -> V8
    assert_eq!(get_pump_speed_for_temp(&table, 60), PumpVoltage::V8);
    // Exact last step -> V11
    assert_eq!(get_pump_speed_for_temp(&table, 80), PumpVoltage::V11);
    // Beyond last step -> V11
    assert_eq!(get_pump_speed_for_temp(&table, 100), PumpVoltage::V11);
}

#[test]
fn test_cpu_limits_frequencies_validation() {
    // Valid frequency range
    let valid = CpuLimits {
        min_freq_khz: Some(800_000),
        max_freq_khz: Some(4_500_000),
        ..Default::default()
    };
    assert!(valid.validate().is_ok());

    // min > max fails
    let inverted = CpuLimits {
        min_freq_khz: Some(4_500_000),
        max_freq_khz: Some(800_000),
        ..Default::default()
    };
    assert_eq!(
        inverted.validate().unwrap_err(),
        ValidationError::InvalidFrequencyRange {
            min: 4_500_000,
            max: 800_000,
        }
    );

    // Out of bounds frequency
    let too_low = CpuLimits {
        min_freq_khz: Some(50_000),
        ..Default::default()
    };
    assert_eq!(
        too_low.validate().unwrap_err(),
        ValidationError::InvalidFrequency(50_000)
    );
}

#[test]
fn test_cpu_limits_tdp_validation() {
    // Valid monotonic TDP values
    let valid = CpuLimits {
        tdp_values_w: vec![25, 35, 45],
        ..Default::default()
    };
    assert!(valid.validate().is_ok());

    // Zero or > 500W TDP fails
    let zero_tdp = CpuLimits {
        tdp_values_w: vec![0],
        ..Default::default()
    };
    assert_eq!(
        zero_tdp.validate().unwrap_err(),
        ValidationError::InvalidTdp(0)
    );

    let excessive_tdp = CpuLimits {
        tdp_values_w: vec![600],
        ..Default::default()
    };
    assert_eq!(
        excessive_tdp.validate().unwrap_err(),
        ValidationError::InvalidTdp(600)
    );

    // Non-monotonic TDP fails
    let non_mono_tdp = CpuLimits {
        tdp_values_w: vec![60, 45],
        ..Default::default()
    };
    assert_eq!(
        non_mono_tdp.validate().unwrap_err(),
        ValidationError::NonMonotonicTdp {
            index: 1,
            prev: 60,
            curr: 45,
        }
    );
}

#[test]
fn test_cpu_limits_core_selection_validation() {
    // Valid enabled cores including mandatory core 0
    let valid = CpuLimits {
        enabled_cores: Some(vec![0, 1, 8, 9]),
        online_cores: Some(4),
        ..Default::default()
    };
    assert!(valid.validate().is_ok());

    // Empty list fails
    let empty = CpuLimits {
        enabled_cores: Some(vec![]),
        ..Default::default()
    };
    assert!(empty.validate().is_err());

    // Missing core 0 fails (core 0 cannot be disabled)
    let missing_c0 = CpuLimits {
        enabled_cores: Some(vec![1, 2, 3]),
        ..Default::default()
    };
    assert!(missing_c0.validate().is_err());

    // Duplicate core IDs fail
    let duplicate = CpuLimits {
        enabled_cores: Some(vec![0, 8, 8]),
        ..Default::default()
    };
    assert!(duplicate.validate().is_err());

    // online_cores == 0 fails
    let zero_online = CpuLimits {
        online_cores: Some(0),
        ..Default::default()
    };
    assert!(zero_online.validate().is_err());
}

#[test]
fn test_charging_policy_thresholds() {
    // Valid: start < end
    let policy = ChargingPolicy {
        start_threshold: Some(40),
        end_threshold: Some(80),
        charge_type: Some(ChargeType::Custom),
        charging_profile: Some("balanced".to_string()),
        charging_priority: Some("performance".to_string()),
    };
    assert!(policy.validate().is_ok());

    // Invalid: start >= end
    let inverted = ChargingPolicy {
        start_threshold: Some(80),
        end_threshold: Some(40),
        ..Default::default()
    };
    assert_eq!(
        inverted.validate().unwrap_err(),
        ValidationError::InvalidChargingThresholds {
            start: 80,
            end: 40,
        }
    );

    let equal = ChargingPolicy {
        start_threshold: Some(60),
        end_threshold: Some(60),
        ..Default::default()
    };
    assert_eq!(
        equal.validate().unwrap_err(),
        ValidationError::InvalidChargingThresholds {
            start: 60,
            end: 60,
        }
    );

    // Out of range (>100)
    let out_of_range = ChargingPolicy {
        start_threshold: Some(101),
        ..Default::default()
    };
    assert_eq!(
        out_of_range.validate().unwrap_err(),
        ValidationError::ThresholdOutOfRange(101)
    );
}

#[test]
fn test_led_settings_validation() {
    let valid_anim = LedSettings {
        brightness: 200,
        red: 255,
        green: 128,
        blue: 0,
        mode: Some("Pulse (stays on)".to_string()),
    };
    assert!(valid_anim.validate().is_ok());

    let invalid_mode = LedSettings {
        brightness: 100,
        red: 0,
        green: 0,
        blue: 0,
        mode: Some("NonExistentMode".to_string()),
    };
    assert_eq!(
        invalid_mode.validate().unwrap_err(),
        ValidationError::InvalidLedMode("NonExistentMode".to_string())
    );
}

#[test]
fn test_profile_state_wire_representations() {
    assert_eq!(ProfileState::Ac.as_str(), "power_ac");
    assert_eq!(ProfileState::Bat.as_str(), "power_bat");
    assert_eq!(ProfileState::Wc.as_str(), "power_wc");
    assert_eq!(ProfileState::TypeC.as_str(), "power_type_c");

    assert_eq!("power_ac".parse::<ProfileState>().unwrap(), ProfileState::Ac);
    assert_eq!("power_bat".parse::<ProfileState>().unwrap(), ProfileState::Bat);
    assert_eq!("power_wc".parse::<ProfileState>().unwrap(), ProfileState::Wc);
    assert_eq!("power_type_c".parse::<ProfileState>().unwrap(), ProfileState::TypeC);

    // Serialization round-trip
    let serialized = serde_json::to_string(&ProfileState::TypeC).unwrap();
    assert_eq!(serialized, "\"power_type_c\"");
    let deserialized: ProfileState = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized, ProfileState::TypeC);
}

#[test]
fn test_power_policy_matrix_exhaustive() {
    // Parity with tests/test_power_policy.cpp: exhaustiveSourceOverrideRequestAndCoolerMatrix
    let sources = [ProfileState::Ac, ProfileState::TypeC, ProfileState::Bat];
    let manual_modes = ["auto", "ac", "type_c", "bat"];
    let requests = ["", "ac", "type_c", "bat"];
    let coolers = [false, true];

    for &source in &sources {
        for &manual in &manual_modes {
            for &request in &requests {
                for &cooler in &coolers {
                    let mut policy = PowerPolicy::new();
                    assert!(policy.set_manual_mode(manual));
                    assert!(policy.register_request(":1.23", "client", "Matrix", request));

                    let to_state = |s: &str| match s {
                        "ac" => ProfileState::Ac,
                        "type_c" => ProfileState::TypeC,
                        _ => ProfileState::Bat,
                    };
                    let rank = |s: ProfileState| match s {
                        ProfileState::Ac => 3,
                        ProfileState::TypeC => 2,
                        _ => 1,
                    };

                    let forced = manual != "auto";
                    let elevated = !forced && !request.is_empty() && rank(to_state(request)) > rank(source);
                    let expected_effective = if forced {
                        to_state(manual)
                    } else if elevated {
                        to_state(request)
                    } else {
                        source
                    };
                    let expected_profile_state = if !forced && !elevated && cooler {
                        ProfileState::Wc
                    } else {
                        expected_effective
                    };

                    let decision = policy.evaluate(source, cooler);
                    assert_eq!(decision.source, source);
                    assert_eq!(decision.effective, expected_effective);
                    assert_eq!(decision.profile_state, expected_profile_state);

                    policy.remove_owner(":1.23");
                    let restored = policy.evaluate(source, cooler);
                    assert_eq!(
                        restored.effective,
                        if forced { to_state(manual) } else { source }
                    );
                }
            }
        }
    }
}

#[test]
fn test_power_policy_validation_and_registration_limits() {
    let mut policy = PowerPolicy::new();

    // Invalid manual mode rejected
    assert!(!policy.set_manual_mode("power_wc"));
    assert!(!policy.set_manual_mode("invalid"));
    assert!(policy.set_manual_mode("ac"));
    assert_eq!(policy.revision(), 1);
    // Setting identical manual mode does not increment revision
    assert!(policy.set_manual_mode("ac"));
    assert_eq!(policy.revision(), 1);

    // Empty owner rejected
    assert!(!policy.register_request("", "client", "reason", "ac"));
    // Empty name rejected
    assert!(!policy.register_request("owner", "", "reason", "ac"));
    // Overlong name rejected (>128)
    assert!(!policy.register_request("owner", &"x".repeat(129), "reason", "ac"));
    // Overlong reason rejected (>512)
    assert!(!policy.register_request("owner", "client", &"r".repeat(513), "ac"));
    // Invalid mode rejected
    assert!(!policy.register_request("owner", "client", "reason", "super_fast"));

    // Valid registration
    assert!(policy.register_request(":1.1", "renderer", "GPU compute", "ac"));
    assert!(policy.unregister_request(":1.1", "renderer"));
    // Unregister non-existent returns false
    assert!(!policy.unregister_request(":1.1", "renderer"));

    // Per-owner max 32 requests limit
    let mut owner_policy = PowerPolicy::new();
    for i in 0..32 {
        let name = format!("req_{i}");
        assert!(owner_policy.register_request(":1.100", &name, "test", "type_c"));
    }
    // 33rd request by same owner rejected
    assert!(!owner_policy.register_request(":1.100", "req_32", "test", "type_c"));
    // Updating existing request succeeds
    assert!(owner_policy.register_request(":1.100", "req_0", "updated", "ac"));
    // Another owner can register
    assert!(owner_policy.register_request(":1.200", "other_req", "test", "type_c"));
}

#[test]
fn test_ucc_profile_validation_comprehensive() {
    let mut profile = UccProfile::new("perf", "Performance");
    profile.display.use_brightness = true;
    profile.display.brightness = 80;
    profile.cpu.min_freq_khz = Some(800_000);
    profile.cpu.max_freq_khz = Some(4_500_000);
    profile.odm_power_limits = vec![25, 45, 65];
    profile.charging.start_threshold = Some(40);
    profile.charging.end_threshold = Some(80);
    profile.fan.manual_pump_voltage = Some(3);
    profile.fan.manual_fan_speed = Some(70);
    profile.fan.table_cpu = vec![
        FanPoint::new(30, 20),
        FanPoint::new(60, 50),
        FanPoint::new(90, 100),
    ];
    profile.fan.table_gpu = vec![
        FanPoint::new(30, 25),
        FanPoint::new(60, 55),
        FanPoint::new(90, 100),
    ];

    assert!(profile.validate().is_ok());

    // Setting forbidden pump voltage (1) makes profile validation fail!
    profile.fan.manual_pump_voltage = Some(1);
    assert!(profile.validate().is_err());
    profile.fan.manual_pump_voltage = Some(3);

    // Inverting charging thresholds makes profile validation fail!
    profile.charging.start_threshold = Some(90);
    profile.charging.end_threshold = Some(50);
    assert!(profile.validate().is_err());
}
