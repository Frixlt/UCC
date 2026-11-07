use std::time::Duration;
use ucc_hardware::cooler::*;

#[test]
fn test_stress_fuzzed_corrupted_packets() {
    // Fuzz packet validator with varying lengths and header/footer corruptions
    let bad_lengths: Vec<Vec<u8>> = vec![
        vec![],
        vec![0xfe],
        vec![0xfe, 0xef],
        vec![0xfe, 0x19, 0x00, 0x01, 0x00, 0x00, 0xef],       // 7 bytes (short)
        vec![0xfe, 0x19, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xef], // 9 bytes (long)
    ];

    for raw in bad_lengths {
        assert!(matches!(
            CoolerPacketEncoder::validate_packet(&raw),
            Err(CoolerError::InvalidPacketLength(_))
        ));
    }

    // Correct length (8 bytes) but wrong header or footer
    let bad_framing = [
        [0x00, 0x19, 0x00, 0x01, 0x00, 0x00, 0x00, 0xef], // bad header
        [0xfe, 0x19, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00], // bad footer
        [0xff, 0x19, 0x00, 0x01, 0x00, 0x00, 0x00, 0xee], // bad both
    ];

    for raw in bad_framing {
        assert!(matches!(
            CoolerPacketEncoder::validate_packet(&raw),
            Err(CoolerError::InvalidFraming(_, _))
        ));
    }
}

#[test]
fn test_stress_fan_speed_boundaries() {
    // 0 to 100 must all succeed and produce 8-byte valid packets
    for speed in 0..=100 {
        let packet = CoolerPacketEncoder::fan_speed(speed).unwrap();
        assert_eq!(packet[0], PACKET_HEADER);
        assert_eq!(packet[1], CMD_FAN);
        assert_eq!(packet[2], 0x01); // enable
        assert_eq!(packet[3], speed);
        assert_eq!(packet[7], PACKET_FOOTER);
        assert!(CoolerPacketEncoder::validate_packet(&packet).is_ok());
    }

    // 101 to 255 must all return InvalidFanSpeed
    for speed in 101..=255 {
        assert_eq!(
            CoolerPacketEncoder::fan_speed(speed).unwrap_err(),
            CoolerError::InvalidFanSpeed(speed)
        );
    }
}

#[test]
fn test_stress_pump_voltage_coverage_and_safety() {
    let safe_voltages = [
        (PumpVoltage::V11, 0x00),
        (PumpVoltage::V7, 0x02),
        (PumpVoltage::V8, 0x03),
    ];

    for (v, expected_code) in safe_voltages {
        let p = CoolerPacketEncoder::pump_voltage(v).unwrap();
        assert_eq!(p[0], PACKET_HEADER);
        assert_eq!(p[1], CMD_PUMP);
        assert_eq!(p[2], 0x01); // enable
        assert_eq!(p[3], 60);   // duty cycle 60%
        assert_eq!(p[4], expected_code);
        assert_eq!(p[7], PACKET_FOOTER);
    }

    // V12 is strictly forbidden for device safety
    assert_eq!(
        CoolerPacketEncoder::pump_voltage(PumpVoltage::V12).unwrap_err(),
        CoolerError::V12Forbidden
    );

    // Off must send turn_off packet
    let p_off = CoolerPacketEncoder::pump_voltage(PumpVoltage::Off).unwrap();
    assert_eq!(p_off[1], CMD_PUMP);
    assert_eq!(p_off[2], 0x00); // disable
}

#[test]
fn test_stress_rapid_burst_writes_rate_limiting() {
    let mut limiter = BleWriteLimiter::new(Duration::from_millis(80));

    // First write: no delay
    assert_eq!(limiter.required_delay(), Duration::ZERO);
    limiter.record_write();

    // Immediate second write: must enforce delay <= 80ms
    let delay1 = limiter.required_delay();
    assert!(delay1 > Duration::from_millis(70) && delay1 <= Duration::from_millis(80));

    // Simulate elapsed time (by testing with tiny limiter)
    let mut fast_limiter = BleWriteLimiter::new(Duration::from_millis(5));
    fast_limiter.record_write();
    std::thread::sleep(Duration::from_millis(6));
    assert_eq!(fast_limiter.required_delay(), Duration::ZERO);
}

#[test]
fn test_state_machine_failure_threshold_and_recovery() {
    let mut sm = CoolerStateMachine::new();

    // Consecutive failures trigger backoff
    assert!(!sm.on_connection_failure()); // failure 1
    assert_eq!(sm.state(), CoolerState::Reconnecting);

    assert!(!sm.on_connection_failure()); // failure 2
    assert_eq!(sm.state(), CoolerState::Reconnecting);

    // 3rd failure reaches FAST_RECONNECT_MAX_FAILURES -> triggers purge / failure
    let purge_needed = sm.on_connection_failure();
    assert!(purge_needed);
    assert_eq!(sm.state(), CoolerState::Failed);

    // Recovery upon successful reconnect
    sm.on_connected();
    assert_eq!(sm.state(), CoolerState::Connected);
    assert!(sm.is_connected());
}
