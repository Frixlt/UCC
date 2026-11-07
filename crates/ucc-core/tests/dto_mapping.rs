use ucc_core::dto::*;
use ucc_core::*;

#[test]
fn test_legacy_profile_dto_roundtrip_and_camel_case_wire_parity() {
    let raw_json = r#"{
        "id": "test-profile-42",
        "name": "My Test Profile",
        "description": "A unit-test profile",
        "display": {
            "brightness": 80,
            "useBrightness": true,
            "refreshRate": 144,
            "useRefRate": true,
            "xResolution": 1920,
            "yResolution": 1080,
            "useResolution": false
        },
        "cpu": {
            "onlineCores": 8,
            "enabledCores": [0, 1, 8, 9],
            "scalingMinFrequency": 800000,
            "scalingMaxFrequency": 4500000,
            "governor": "powersave",
            "energyPerformancePreference": "balance_performance",
            "noTurbo": false,
            "hwpDynamicBoost": false
        },
        "webcam": {
            "status": true,
            "useStatus": false
        },
        "fan": {
            "useControl": true,
            "fanProfile": "fan-balanced",
            "sameSpeed": false,
            "autoControlWC": true,
            "manualFanSpeed": 70,
            "manualPumpVoltage": 3,
            "enableWaterCooler": false,
            "tableCPU": [{"temp":30,"speed":20},{"temp":70,"speed":60}],
            "tableGPU": [{"temp":30,"speed":25},{"temp":70,"speed":65}],
            "tablePump": [{"temp":40,"speed":1},{"temp":60,"speed":2}],
            "tableWaterCoolerFan": [{"temp":30,"speed":15}]
        },
        "odmProfile": { "name": "enthusiast" },
        "odmPowerLimits": { "tdpValues": [45, 80] },
        "nvidiaCTGPOffset": 15,
        "keyboard": { "keyboardProfileName": "Rainbow" },
        "selectedKeyboardProfile": "kb-uuid-001",
        "chargingProfile": "balanced",
        "chargingPriority": "performance",
        "chargeType": "Standard",
        "chargeStartThreshold": 40,
        "chargeEndThreshold": 80
    }"#;

    let profile = LegacyProfileDto::parse_json(raw_json).expect("should parse legacy profile JSON");

    assert_eq!(profile.id, "test-profile-42");
    assert_eq!(profile.name, "My Test Profile");
    assert_eq!(profile.description, "A unit-test profile");
    assert_eq!(profile.display.brightness, 80);
    assert!(profile.display.use_brightness);
    assert_eq!(profile.display.refresh_rate, 144);
    assert!(profile.display.use_ref_rate);
    assert_eq!(profile.display.x_resolution, 1920);
    assert_eq!(profile.display.y_resolution, 1080);
    assert!(!profile.display.use_resolution);

    assert_eq!(profile.cpu.online_cores, Some(8));
    assert_eq!(profile.cpu.enabled_cores, Some(vec![0, 1, 8, 9]));
    assert_eq!(profile.cpu.min_freq_khz, Some(800_000));
    assert_eq!(profile.cpu.max_freq_khz, Some(4_500_000));
    assert_eq!(profile.cpu.governor, "powersave");
    assert_eq!(profile.cpu.energy_performance_preference, "balance_performance");
    assert!(!profile.cpu.no_turbo);

    assert!(profile.webcam.status);
    assert!(!profile.webcam.use_status);

    assert!(profile.fan.use_control);
    assert_eq!(profile.fan.fan_profile, "fan-balanced");
    assert!(!profile.fan.same_speed);
    assert!(profile.fan.auto_control_wc);
    assert_eq!(profile.fan.manual_fan_speed, Some(70));
    assert_eq!(profile.fan.manual_pump_voltage, Some(3));
    assert!(!profile.fan.enable_water_cooler);
    assert_eq!(profile.fan.table_cpu.len(), 2);
    assert_eq!(profile.fan.table_gpu.len(), 2);
    assert_eq!(profile.fan.table_pump.len(), 2);
    assert_eq!(profile.fan.table_water_cooler_fan.len(), 1);

    assert_eq!(profile.odm_profile_name, Some("enthusiast".to_string()));
    assert_eq!(profile.odm_power_limits, vec![45, 80]);
    assert_eq!(profile.nvidia_ctgp_offset, Some(15));
    assert_eq!(profile.keyboard.keyboard_profile_name, "Rainbow");
    assert_eq!(profile.selected_keyboard_profile, Some("kb-uuid-001".to_string()));

    assert_eq!(profile.charging.charging_profile, Some("balanced".to_string()));
    assert_eq!(profile.charging.charging_priority, Some("performance".to_string()));
    assert_eq!(profile.charging.charge_type, Some(ChargeType::Standard));
    assert_eq!(profile.charging.start_threshold, Some(40));
    assert_eq!(profile.charging.end_threshold, Some(80));

    // Serialize back to JSON and verify exact camelCase keys are preserved
    let serialized = LegacyProfileDto::to_json(&profile).expect("should serialize to JSON");
    assert!(serialized.contains("\"tableCPU\""), "Must contain exact tableCPU key for C++ wire parity");
    assert!(serialized.contains("\"tableGPU\""), "Must contain exact tableGPU key for C++ wire parity");
    assert!(serialized.contains("\"autoControlWC\""), "Must contain exact autoControlWC key for C++ wire parity");
    assert!(serialized.contains("\"selectedKeyboardProfile\""), "Must contain exact selectedKeyboardProfile key");
    assert!(serialized.contains("\"nvidiaCTGPOffset\""), "Must contain exact nvidiaCTGPOffset key");

    // Re-parse the serialized JSON to ensure round-trip equality
    let roundtripped = LegacyProfileDto::parse_json(&serialized).expect("re-parse must succeed");
    assert_eq!(profile, roundtripped);
}

#[test]
fn test_legacy_profile_dto_nullable_defaults() {
    let minimal = r#"{"id":"min","name":"Minimal"}"#;
    let profile = LegacyProfileDto::parse_json(minimal).expect("minimal must parse");

    assert_eq!(profile.id, "min");
    assert_eq!(profile.name, "Minimal");
    assert_eq!(profile.description, "");
    assert_eq!(profile.display.brightness, 100);
    assert!(!profile.display.use_brightness);
    assert!(profile.webcam.status);
    assert!(profile.webcam.use_status);
    assert!(profile.fan.use_control);
    assert_eq!(profile.fan.manual_fan_speed, None);
    assert_eq!(profile.fan.manual_pump_voltage, None);
    assert!(profile.fan.table_cpu.is_empty());
    assert!(profile.odm_power_limits.is_empty());
}

#[test]
fn test_pump_speed_to_voltage_translator_finding_v2_04() {
    // R1 Finding [FINDING-V2-04]:
    // In tablePump, speed is step index (0..4).
    // In hardware/driver, discrete code is PumpVoltage (0, 2, 3, 4).
    assert_eq!(pump_speed_to_voltage(0), Ok(PumpVoltage::Off)); // code 4
    assert_eq!(pump_speed_to_voltage(1), Ok(PumpVoltage::V7));  // code 2
    assert_eq!(pump_speed_to_voltage(2), Ok(PumpVoltage::V8));  // code 3
    assert_eq!(pump_speed_to_voltage(3), Ok(PumpVoltage::V11)); // code 0
    assert_eq!(pump_speed_to_voltage(4), Ok(PumpVoltage::V12)); // code 1

    assert_eq!(voltage_to_pump_speed(PumpVoltage::Off), 0);
    assert_eq!(voltage_to_pump_speed(PumpVoltage::V7), 1);
    assert_eq!(voltage_to_pump_speed(PumpVoltage::V8), 2);
    assert_eq!(voltage_to_pump_speed(PumpVoltage::V11), 3);
    assert_eq!(voltage_to_pump_speed(PumpVoltage::V12), 4);

    assert!(pump_speed_to_voltage(5).is_err());
}

#[test]
fn test_legacy_power_policy_status_dto_mapping() {
    let mut policy = PowerPolicy::new();
    policy.set_manual_mode("auto");
    policy.register_request(":1.42", "game", "Gaming boost", "ac");

    let decision = policy.evaluate(ProfileState::Bat, true);
    let dto = LegacyPowerPolicyStatusDto::from(&decision);

    assert_eq!(dto.real_power_source, "bat");
    assert_eq!(dto.current_mode, "ac");
    assert_eq!(dto.manual_mode, "auto");
    assert_eq!(dto.profile_state, "power_ac"); // elevated by request, WC overridden
    assert!(dto.water_cooler_connected);
    assert_eq!(dto.registered_apps.len(), 1);
    assert_eq!(dto.registered_apps[0].owner, ":1.42");
    assert_eq!(dto.registered_apps[0].name, "game");
    assert_eq!(dto.registered_apps[0].reason, "Gaming boost");
    assert_eq!(dto.registered_apps[0].force_mode, "ac");
}
