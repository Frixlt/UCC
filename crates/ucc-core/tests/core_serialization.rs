use ucc_core::{
    ChargeType, FanPoint, LegacyFanDto, LegacyFanPointDto, LegacyProfileDto, PumpPoint,
    ValidationError,
};

#[test]
fn legacy_profile_rejects_negative_core_ids() {
    let doc = r#"{"id":"p","name":"P","cpu":{"enabledCores":[0,-1]}}"#;
    let result = LegacyProfileDto::parse_json(doc);
    assert!(
        result.is_err(),
        "negative core ID accepted and discarded: {result:?}"
    );
}

#[test]
fn legacy_profile_never_shifts_tdp_slot_by_filtering_entries() {
    let doc = r#"{"id":"p","name":"P","odmPowerLimits":{"tdpValues":[45,0,80]}}"#;
    if let Ok(profile) = LegacyProfileDto::parse_json(doc) {
        assert_eq!(
            profile.odm_power_limits,
            [45, 0, 80],
            "PL4 silently moved into PL2"
        );
    }
}

#[test]
fn legacy_profile_dto_clamping_and_roundtrip_boundaries() {
    // 1. Out-of-range fan/charging values and negative pump sentinel follow the clamping & None-mapping contract.
    let dto = LegacyProfileDto {
        id: "clamp-boundaries".to_string(),
        name: "Clamp Boundaries".to_string(),
        fan: Some(LegacyFanDto {
            use_control: true,
            fan_profile: "fan-custom".to_string(),
            same_speed: false,
            auto_control_wc: false,
            manual_fan_speed: 150,   // > 100 -> None
            manual_pump_voltage: -1, // negative legacy sentinel -> None
            enable_water_cooler: true,
            table_cpu: vec![
                LegacyFanPointDto {
                    temp: 30,
                    speed: -25,
                }, // clamped to 0
                LegacyFanPointDto {
                    temp: 80,
                    speed: 140,
                }, // clamped to 100
            ],
            table_gpu: vec![
                LegacyFanPointDto {
                    temp: 35,
                    speed: -1,
                }, // clamped to 0
                LegacyFanPointDto {
                    temp: 85,
                    speed: 255,
                }, // clamped to 100
            ],
            table_water_cooler_fan: vec![
                LegacyFanPointDto {
                    temp: 40,
                    speed: -10,
                }, // clamped to 0
                LegacyFanPointDto {
                    temp: 75,
                    speed: 101,
                }, // clamped to 100
            ],
            table_pump: vec![
                LegacyFanPointDto {
                    temp: 40,
                    speed: -5,
                }, // clamped to 0
                LegacyFanPointDto {
                    temp: 75,
                    speed: 99,
                }, // clamped to 4
            ],
        }),
        charge_type: Some("UnknownMode".to_string()), // unrecognized -> None
        charge_start_threshold: Some(-5),             // out of 0..=100 -> None
        charge_end_threshold: Some(105),              // out of 0..=100 -> None
        ..LegacyProfileDto::default()
    };

    let profile = dto
        .into_profile()
        .expect("out-of-range legacy fan/pump/charging values clamp or normalize cleanly");

    assert_eq!(profile.fan.manual_fan_speed, None);
    assert_eq!(profile.fan.manual_pump_voltage, None);
    assert_eq!(
        profile.fan.table_cpu,
        vec![FanPoint::new(30, 0), FanPoint::new(80, 100)]
    );
    assert_eq!(
        profile.fan.table_gpu,
        vec![FanPoint::new(35, 0), FanPoint::new(85, 100)]
    );
    assert_eq!(
        profile.fan.table_water_cooler_fan,
        vec![FanPoint::new(40, 0), FanPoint::new(75, 100)]
    );
    assert_eq!(
        profile.fan.table_pump,
        vec![PumpPoint::new(40, 0), PumpPoint::new(75, 4)]
    );
    assert_eq!(profile.charging.charge_type, None);
    assert_eq!(profile.charging.start_threshold, None);
    assert_eq!(profile.charging.end_threshold, None);

    // Roundtrip back via from_profile emits -1 sentinels for None manual fan/pump,
    // None for unset charging fields, and preserves clamped table values.
    let roundtrip = LegacyProfileDto::from_profile(&profile);
    let rt_fan = roundtrip.fan.as_ref().expect("fan dto present");
    assert_eq!(rt_fan.manual_fan_speed, -1);
    assert_eq!(rt_fan.manual_pump_voltage, -1);
    assert_eq!(
        rt_fan.table_cpu,
        vec![
            LegacyFanPointDto { temp: 30, speed: 0 },
            LegacyFanPointDto {
                temp: 80,
                speed: 100
            },
        ]
    );
    assert_eq!(
        rt_fan.table_pump,
        vec![
            LegacyFanPointDto { temp: 40, speed: 0 },
            LegacyFanPointDto { temp: 75, speed: 4 },
        ]
    );
    assert_eq!(roundtrip.charge_type, None);
    assert_eq!(roundtrip.charge_start_threshold, None);
    assert_eq!(roundtrip.charge_end_threshold, None);

    // 2. Exact inclusive boundary values (0, 100, discrete pump codes {0, 2, 3, 4}, Long Life alias)
    //    roundtrip faithfully through into_profile -> from_profile.
    for (manual_fan, pump_code) in [(0, 0), (50, 2), (100, 3), (0, 4)] {
        let valid_dto = LegacyProfileDto {
            id: "exact-bounds".to_string(),
            name: "Exact Bounds".to_string(),
            fan: Some(LegacyFanDto {
                use_control: true,
                fan_profile: "fan-balanced".to_string(),
                same_speed: true,
                auto_control_wc: true,
                manual_fan_speed: manual_fan,
                manual_pump_voltage: pump_code,
                enable_water_cooler: true,
                table_cpu: Vec::new(),
                table_gpu: Vec::new(),
                table_pump: Vec::new(),
                table_water_cooler_fan: Vec::new(),
            }),
            charge_type: Some("Long Life".to_string()),
            charge_start_threshold: Some(0),
            charge_end_threshold: Some(100),
            ..LegacyProfileDto::default()
        };

        let prof = valid_dto
            .into_profile()
            .expect("exact boundary values must pass");
        assert_eq!(prof.fan.manual_fan_speed, Some(manual_fan as u8));
        assert_eq!(prof.fan.manual_pump_voltage, Some(pump_code));
        assert_eq!(prof.charging.charge_type, Some(ChargeType::LongLife));
        assert_eq!(prof.charging.start_threshold, Some(0));
        assert_eq!(prof.charging.end_threshold, Some(100));

        let rt = LegacyProfileDto::from_profile(&prof);
        let fan = rt.fan.expect("fan dto present");
        assert_eq!(fan.manual_fan_speed, manual_fan);
        assert_eq!(fan.manual_pump_voltage, pump_code);
        assert_eq!(rt.charge_type.as_deref(), Some("LongLife"));
        assert_eq!(rt.charge_start_threshold, Some(0));
        assert_eq!(rt.charge_end_threshold, Some(100));
    }

    // 3. Invalid charging threshold ordering (start >= end within 0..=100) still fails validation.
    let bad_thresholds = LegacyProfileDto {
        id: "bad-charge".to_string(),
        name: "Bad Charge".to_string(),
        charge_start_threshold: Some(80),
        charge_end_threshold: Some(80),
        ..LegacyProfileDto::default()
    };
    assert_eq!(
        bad_thresholds.into_profile(),
        Err(ValidationError::InvalidChargingThresholds { start: 80, end: 80 })
    );
}

#[test]
fn legacy_profile_dto_rejects_forbidden_v12_and_invalid_pump_codes() {
    for invalid_code in [1, 5, 99] {
        let dto = LegacyProfileDto {
            id: "forbidden-pump".to_string(),
            name: "Forbidden Pump".to_string(),
            fan: Some(LegacyFanDto {
                use_control: true,
                fan_profile: "fan-balanced".to_string(),
                same_speed: true,
                auto_control_wc: true,
                manual_fan_speed: -1,
                manual_pump_voltage: invalid_code,
                enable_water_cooler: true,
                table_cpu: Vec::new(),
                table_gpu: Vec::new(),
                table_pump: Vec::new(),
                table_water_cooler_fan: Vec::new(),
            }),
            ..LegacyProfileDto::default()
        };
        assert_eq!(
            dto.into_profile(),
            Err(ValidationError::InvalidPumpVoltage(invalid_code)),
            "LegacyProfileDto must not silently drop invalid pump voltage code {invalid_code}"
        );
    }

    let raw_v12 = r#"{"id":"v12","name":"V12","fan":{"manualPumpVoltage":1}}"#;
    assert!(matches!(
        LegacyProfileDto::parse_json(raw_v12),
        Err(ucc_core::CoreError::Validation(
            ValidationError::InvalidPumpVoltage(1)
        ))
    ));
}
