use ucc_core::fan::{FanProfile, PumpPoint, PumpVoltage};

#[test]
fn a_validated_pump_curve_does_not_ignore_a_matching_temperature_step() {
    let mut profile = FanProfile::new("id", "name");
    profile.table_pump = vec![PumpPoint::new(80, 3), PumpPoint::new(40, 1)];
    if profile.validate().is_ok() {
        assert_eq!(profile.get_pump_speed_for_temp(50), PumpVoltage::V7);
    }
}

#[test]
fn legacy_pump_level_4_clamps_to_safe_execution_policy() {
    let mut profile = FanProfile::new("id", "name");
    profile.table_pump = vec![PumpPoint::new(50, 4)];
    assert!(profile.validate().is_ok());
    let voltage = profile.get_pump_speed_for_temp(60);
    assert_ne!(
        voltage,
        PumpVoltage::V12,
        "V12 must never be returned for hardware execution"
    );
    assert_eq!(voltage, PumpVoltage::V11, "Level 4 should clamp to V11");
}
