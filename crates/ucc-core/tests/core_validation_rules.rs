// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{json, Value};
use ucc_core::{ProfileCpu, PumpVoltage, UccProfile, ValidationError};

#[test]
fn t_cfg_01_profile_roundtrip_preserves_flat_cpp_charging_ctgp_keyboard_and_extra() {
    let raw = json!({
        "id": "cpp-legacy-prof",
        "name": "C++ Legacy Profile",
        "description": "Roundtrip test",
        "display": {
            "brightness": 80,
            "useBrightness": true,
            "refreshRate": 144,
            "useRefRate": false
        },
        "cpu": {
            "onlineCores": 8,
            "governor": "powersave",
            "energyPerformancePreference": "balance_power",
            "noTurbo": false
        },
        "webcam": {
            "status": true,
            "useStatus": false
        },
        "fan": {
            "fanProfile": "fan-quiet",
            "useControl": true,
            "sameSpeed": false,
            "autoControlWC": true,
            "enableWaterCooler": false,
            "tableCPU": [],
            "tableGPU": []
        },
        "odmProfile": { "name": "balanced" },
        "odmPowerLimits": { "tdpValues": [45, 70, 95] },
        "chargingProfile": "stationary",
        "chargingPriority": "charge_battery",
        "chargeType": "Custom",
        "chargeStartThreshold": 60,
        "chargeEndThreshold": 80,
        "nvidiaCTGPOffset": -10,
        "keyboard": {
            "brightness": 50
        },
        "futureField": 123
    });

    let prof: UccProfile =
        serde_json::from_value(raw).expect("UccProfile must deserialize C++ profile");
    let out: Value = serde_json::to_value(&prof).expect("UccProfile must serialize");

    assert_eq!(out["chargingProfile"], "stationary");
    assert_eq!(out["chargingPriority"], "charge_battery");
    assert_eq!(out["chargeType"], "Custom");
    assert_eq!(out["chargeStartThreshold"], 60);
    assert_eq!(out["chargeEndThreshold"], 80);
    assert_eq!(out["nvidiaCTGPOffset"], -10);
    assert_eq!(out["keyboard"]["brightness"], 50);
    assert_eq!(out["futureField"], 123);
}

#[test]
fn t_cfg_02_profile_accepts_all_kernel_charge_types() {
    let base = serde_json::to_value(UccProfile::new("charge-types", "Charge Types")).unwrap();

    for ct in [
        "Standard", "Custom", "Trickle", "Fast", "Adaptive", "LongLife", "Bypass",
    ] {
        let mut payload = base.clone();
        payload["chargeType"] = json!(ct);
        let prof: UccProfile = serde_json::from_value(payload)
            .unwrap_or_else(|e| panic!("UccProfile rejected valid kernel chargeType '{ct}': {e}"));
        let out = serde_json::to_value(&prof).unwrap();
        assert_eq!(
            out["chargeType"], ct,
            "roundtrip lost kernel chargeType '{ct}'"
        );
    }
}

#[test]
fn t_cfg_08_profile_cpu_preserves_minus_1_and_minus_2_sentinels() {
    let raw = json!({
        "scalingMinFrequency": -2,
        "scalingMaxFrequency": -1,
        "governor": "powersave",
        "energyPerformancePreference": "balance_power"
    });

    let cpu: ProfileCpu = serde_json::from_value(raw).expect("parse ProfileCpu");
    let out = serde_json::to_value(&cpu).expect("serialize ProfileCpu");

    assert_eq!(
        out["scalingMaxFrequency"], -1,
        "-1 (reduced midpoint max frequency) sentinel must not be erased"
    );
    assert_eq!(
        out["scalingMinFrequency"], -2,
        "-2 (lock min to max) sentinel must not be erased"
    );
}

#[test]
fn t_cfg_03_profile_cpu_zero_sentinel_does_not_fail_validation() {
    let raw = json!({
        "onlineCores": 0,
        "scalingMinFrequency": 0,
        "scalingMaxFrequency": 0,
        "governor": "powersave",
        "energyPerformancePreference": "power"
    });

    let cpu: ProfileCpu = serde_json::from_value(raw).expect("parse zero-sentinel ProfileCpu");
    assert_eq!(cpu.online_cores, None);
    assert_eq!(cpu.min_freq_khz, None);
    assert_eq!(cpu.max_freq_khz, None);
}

#[test]
fn t_daemon_01_pump_voltage_discrete_codes_contract() {
    assert_eq!(PumpVoltage::from_discrete_code(0), Ok(PumpVoltage::V11));
    assert!(PumpVoltage::from_discrete_code(1).is_err());
    assert_eq!(PumpVoltage::from_discrete_code(2), Ok(PumpVoltage::V7));
    assert_eq!(PumpVoltage::from_discrete_code(3), Ok(PumpVoltage::V8));
    assert_eq!(PumpVoltage::from_discrete_code(4), Ok(PumpVoltage::Off));
}

#[test]
fn profile_manual_fan_speed_and_pump_voltage_deserialization_bounds() {
    // 1. Valid manualFanSpeed (0..=100) and valid manualPumpVoltage ({0, 2, 3, 4}).
    for (fan_speed, pump_code) in [(0u8, 0i32), (50, 2), (100, 3), (25, 4)] {
        let payload = json!({
            "id": "manual-valid",
            "name": "Manual Valid",
            "fan": {
                "manualFanSpeed": fan_speed,
                "manualPumpVoltage": pump_code
            }
        });
        let prof: UccProfile =
            serde_json::from_value(payload).expect("valid manual fan/pump must deserialize");
        assert_eq!(prof.fan.manual_fan_speed, Some(fan_speed));
        assert_eq!(prof.fan.manual_pump_voltage, Some(pump_code));
        assert!(prof.validate().is_ok());

        let out = serde_json::to_value(&prof).expect("serialize valid profile");
        assert_eq!(out["fan"]["manualFanSpeed"], fan_speed);
        assert_eq!(out["fan"]["manualPumpVoltage"], pump_code);
    }

    // 2. Legacy sentinels (-1) and null deserialize to None and are omitted on serialization.
    for sentinel in [json!(-1), Value::Null] {
        let payload = json!({
            "id": "manual-sentinel",
            "name": "Manual Sentinel",
            "fan": {
                "manualFanSpeed": sentinel,
                "manualPumpVoltage": sentinel
            }
        });
        let prof: UccProfile =
            serde_json::from_value(payload).expect("sentinel manual fan/pump must deserialize");
        assert_eq!(prof.fan.manual_fan_speed, None);
        assert_eq!(prof.fan.manual_pump_voltage, None);
        assert!(prof.validate().is_ok());

        let out = serde_json::to_value(&prof).expect("serialize sentinel profile");
        assert!(out["fan"].get("manualFanSpeed").is_none());
        assert!(out["fan"].get("manualPumpVoltage").is_none());
    }

    // 3. Out-of-range manualFanSpeed in JSON (-2, 101, 1000) normalizes to None on deserialize,
    //    while direct struct assignment > 100 is rejected by validate().
    for oob_fan in [-2i64, 101, 1000] {
        let payload = json!({
            "id": "manual-fan-oob",
            "name": "Manual Fan OOB",
            "fan": {
                "manualFanSpeed": oob_fan
            }
        });
        let prof: UccProfile =
            serde_json::from_value(payload).expect("out-of-range manualFanSpeed normalizes");
        assert_eq!(prof.fan.manual_fan_speed, None);
        assert!(prof.validate().is_ok());
    }
    let mut direct_oob = UccProfile::new("direct-fan-oob", "Direct Fan OOB");
    direct_oob.fan.manual_fan_speed = Some(101);
    assert_eq!(
        direct_oob.validate(),
        Err(ValidationError::InvalidFanSpeed { speed: 101 })
    );

    // 4. Negative manualPumpVoltage (e.g. -5) normalizes to None, whereas forbidden/out-of-range
    //    non-negative codes (1 = V12, 5, 99) deserialize as Some(code) and fail validate().
    let neg_pump_payload = json!({
        "id": "manual-pump-neg",
        "name": "Manual Pump Neg",
        "fan": {
            "manualPumpVoltage": -5
        }
    });
    let neg_pump_prof: UccProfile =
        serde_json::from_value(neg_pump_payload).expect("negative pump code normalizes to None");
    assert_eq!(neg_pump_prof.fan.manual_pump_voltage, None);
    assert!(neg_pump_prof.validate().is_ok());

    for invalid_code in [1i32, 5, 99] {
        let payload = json!({
            "id": "manual-pump-invalid",
            "name": "Manual Pump Invalid",
            "fan": {
                "manualPumpVoltage": invalid_code
            }
        });
        let prof: UccProfile = serde_json::from_value(payload)
            .expect("non-negative pump code deserializes before validation");
        assert_eq!(prof.fan.manual_pump_voltage, Some(invalid_code));
        assert_eq!(
            prof.validate(),
            Err(ValidationError::InvalidPumpVoltage(invalid_code))
        );
    }
}
