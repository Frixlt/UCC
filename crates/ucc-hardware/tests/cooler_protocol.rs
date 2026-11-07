use std::time::Duration;
use ucc_hardware::cooler::*;

#[test]
fn test_packet_encoder_reset() {
    let packet = CoolerPacketEncoder::reset();
    assert_eq!(packet, [0xfe, 0x19, 0x00, 0x01, 0x00, 0x00, 0x00, 0xef]);
    assert!(CoolerPacketEncoder::validate_packet(&packet).is_ok());
}

#[test]
fn test_packet_encoder_fan_speed() {
    // Valid 0%
    let p0 = CoolerPacketEncoder::fan_speed(0).unwrap();
    assert_eq!(p0, [0xfe, 0x1b, 0x01, 0x00, 0x00, 0x00, 0x00, 0xef]);

    // Valid 50%
    let p50 = CoolerPacketEncoder::fan_speed(50).unwrap();
    assert_eq!(p50, [0xfe, 0x1b, 0x01, 50, 0x00, 0x00, 0x00, 0xef]);

    // Valid 100%
    let p100 = CoolerPacketEncoder::fan_speed(100).unwrap();
    assert_eq!(p100, [0xfe, 0x1b, 0x01, 100, 0x00, 0x00, 0x00, 0xef]);

    // Invalid > 100%
    assert_eq!(
        CoolerPacketEncoder::fan_speed(101).unwrap_err(),
        CoolerError::InvalidFanSpeed(101)
    );
}

#[test]
fn test_packet_encoder_pump_voltage() {
    // V11 (0x00)
    let p_v11 = CoolerPacketEncoder::pump_voltage(PumpVoltage::V11).unwrap();
    assert_eq!(p_v11, [0xfe, 0x1c, 0x01, 60, 0x00, 0x00, 0x00, 0xef]);

    // V7 (0x02)
    let p_v7 = CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap();
    assert_eq!(p_v7, [0xfe, 0x1c, 0x01, 60, 0x02, 0x00, 0x00, 0xef]);

    // V8 (0x03)
    let p_v8 = CoolerPacketEncoder::pump_voltage(PumpVoltage::V8).unwrap();
    assert_eq!(p_v8, [0xfe, 0x1c, 0x01, 60, 0x03, 0x00, 0x00, 0xef]);

    // Off (0x04) -> maps to turn_off(CMD_PUMP)
    let p_off = CoolerPacketEncoder::pump_voltage(PumpVoltage::Off).unwrap();
    assert_eq!(p_off, [0xfe, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0xef]);

    // V12 is forbidden
    assert_eq!(
        CoolerPacketEncoder::pump_voltage(PumpVoltage::V12).unwrap_err(),
        CoolerError::V12Forbidden
    );
}

#[test]
fn test_packet_encoder_rgb() {
    let p = CoolerPacketEncoder::rgb(255, 128, 64, RgbState::Breathe);
    assert_eq!(p, [0xfe, 0x1e, 0x01, 255, 128, 64, 0x01, 0xef]);
    assert!(CoolerPacketEncoder::validate_packet(&p).is_ok());
}

#[test]
fn test_packet_encoder_turn_off() {
    let p = CoolerPacketEncoder::turn_off(CMD_FAN);
    assert_eq!(p, [0xfe, 0x1b, 0x00, 0x00, 0x00, 0x00, 0x00, 0xef]);
}

#[test]
fn test_packet_validation_errors() {
    // Bad length
    assert_eq!(
        CoolerPacketEncoder::validate_packet(&[0xfe, 0x19, 0xef]),
        Err(CoolerError::InvalidPacketLength(3))
    );

    // Bad header/footer
    assert_eq!(
        CoolerPacketEncoder::validate_packet(&[0x00, 0, 0, 0, 0, 0, 0, 0xef]),
        Err(CoolerError::InvalidFraming(0x00, 0xef))
    );
}

#[test]
fn test_ble_write_limiter() {
    let mut limiter = BleWriteLimiter::new(Duration::from_millis(50));
    assert_eq!(limiter.required_delay(), Duration::ZERO);

    limiter.record_write();
    let delay = limiter.required_delay();
    assert!(delay > Duration::ZERO && delay <= Duration::from_millis(50));
}

#[test]
fn test_cooler_state_machine_reconnect_and_keepalive() {
    let mut sm = CoolerStateMachine::new();
    assert_eq!(sm.state(), CoolerState::Disconnected);
    assert!(!sm.is_connected());

    // Connect
    sm.on_connected();
    assert_eq!(sm.state(), CoolerState::Connected);
    assert!(sm.is_connected());

    // Keepalive checks
    assert!(!sm.should_send_keepalive()); // Just connected, no keepalive yet
    sm.on_keepalive_sent();
    sm.on_keepalive_ack();

    // Missed keepalives
    assert!(!sm.on_keepalive_missed()); // 1
    assert!(!sm.on_keepalive_missed()); // 2
    assert!(sm.on_keepalive_missed()); // 3 -> declared dead!
    assert_eq!(sm.state(), CoolerState::Disconnected);

    // Connection failures & backoff
    assert!(!sm.on_connection_failure()); // 1 -> reconnecting
    assert_eq!(sm.state(), CoolerState::Reconnecting);
    assert!(!sm.on_connection_failure()); // 2 -> reconnecting
    assert!(sm.on_connection_failure()); // 3 -> Failed (threshold reached)
    assert_eq!(sm.state(), CoolerState::Failed);
}
