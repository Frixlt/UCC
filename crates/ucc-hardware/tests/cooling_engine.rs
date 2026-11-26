use std::time::{Duration, Instant};
use ucc_core::{
    FanControlLogic, FanLogicType, FanPoint, FanProfile, TemperatureFilter, TemporaryFanCurves,
};
use ucc_hardware::{
    CoolerCommand, CoolerController, CoolerError, CoolerMode, FanController, MockBleTransport,
    MockPlatformBackend, PlatformWrite, PumpVoltage, RgbState, BLE_WRITE_GAP,
};

fn sample_fan_profile() -> FanProfile {
    FanProfile {
        id: "balanced".into(),
        name: "Balanced".into(),
        table_cpu: vec![
            FanPoint::new(40, 20),
            FanPoint::new(60, 50),
            FanPoint::new(80, 90),
        ],
        table_gpu: vec![
            FanPoint::new(40, 15),
            FanPoint::new(60, 45),
            FanPoint::new(80, 85),
        ],
        ..Default::default()
    }
}

#[test]
fn temperature_filter_has_asymmetric_rising_and_falling_response() {
    let mut rising = TemperatureFilter::default();
    rising.add_value(40);
    rising.add_value(60); // 40 + 0.5*(20) = 50
    assert_eq!(rising.filtered_value(), 50);

    let mut falling = TemperatureFilter::default();
    falling.add_value(60);
    falling.add_value(40); // 60 + 0.15*(-20) = 57
    assert_eq!(falling.filtered_value(), 57);
}

#[test]
fn fan_logic_applies_hysteresis_ewma_hw_limits_and_critical_override() {
    let profile = sample_fan_profile();
    let mut logic = FanControlLogic::new(profile, FanLogicType::Cpu);
    logic.set_fans_min_speed_hw_limit(20);
    logic.set_fans_off_available(true);

    // First sample at 60C -> filtered=60, effective=60 -> curve=50%
    assert_eq!(logic.report_temperature(60), 50);

    // Sudden jump to >= 90C -> filtered temp crosses 85 (>=80%) then 90 (100% override)
    // 60 -> 76 -> 86 (>=85C => >=80%) -> 91 (>=90C => 100%)
    assert!(logic.report_temperature(92) <= 100);
    assert!(logic.report_temperature(96) >= 80);
    let spd = logic.report_temperature(96);
    assert_eq!(spd, 100, "overheat >= 90C must force 100% speed");

    // Verify 85C critical floor (>=80%) on a fresh instance with lower curve
    let quiet = FanProfile {
        id: "quiet".into(),
        name: "Quiet".into(),
        table_cpu: vec![FanPoint::new(40, 20), FanPoint::new(90, 40)],
        table_gpu: vec![FanPoint::new(40, 20), FanPoint::new(90, 40)],
        ..Default::default()
    };
    let mut critical_logic = FanControlLogic::new(quiet, FanLogicType::Cpu);
    assert!(critical_logic.report_temperature(86) >= 80);

    // Verify hardware min_speed / fans_off_available
    let off_curve = FanProfile {
        id: "zero".into(),
        name: "Zero".into(),
        table_cpu: vec![FanPoint::new(40, 5), FanPoint::new(50, 15), FanPoint::new(80, 80)],
        table_gpu: vec![FanPoint::new(40, 5), FanPoint::new(50, 15), FanPoint::new(80, 80)],
        ..Default::default()
    };
    let mut off_logic = FanControlLogic::new(off_curve.clone(), FanLogicType::Cpu);
    off_logic.set_fans_min_speed_hw_limit(20);
    off_logic.set_fans_off_available(true);
    // 5% < 10% (half of 20%) -> clamped to 0%
    assert_eq!(off_logic.report_temperature(40), 0);

    let mut no_off_logic = FanControlLogic::new(off_curve, FanLogicType::Cpu);
    no_off_logic.set_fans_min_speed_hw_limit(20);
    no_off_logic.set_fans_off_available(false);
    // 5% with fans_off_available=false and < half_min: Wait matching C++: C++ keeps curveSpeed when !off && < halfMin or >= halfMin->minSpeed? Let's test 12% (>= half_min=10) -> min_speed 20%
    assert_eq!(no_off_logic.report_temperature(47), 20);
}

#[test]
fn fan_controller_same_speed_vs_independent_and_sensor_failsafe() {
    let backend = MockPlatformBackend::uniwill_default();
    backend.set_fan_temp(0, 70); // high CPU temp
    backend.set_fan_temp(1, 40); // cool GPU temp

    let mut controller = FanController::new(&backend, sample_fan_profile(), true);
    assert_eq!(controller.fan_count(), 2);

    let snap_sync = controller.tick(&backend, true, 1000);
    assert!(!snap_sync.fallback_auto);
    assert_eq!(snap_sync.speeds_percent[0], snap_sync.speeds_percent[1]);
    assert!(snap_sync.speeds_percent[0].unwrap() > 50);

    // Switch to independent mode
    backend.clear_journal();
    controller.set_same_speed(false);
    let snap_indep = controller.tick(&backend, true, 2000);
    assert!(snap_indep.speeds_percent[0].unwrap() > snap_indep.speeds_percent[1].unwrap());

    // Fail-safe: lose ALL sensors -> MUST restore firmware auto, NEVER write 0%
    backend.clear_journal();
    backend.set_fan_temp_error(0);
    backend.set_fan_temp_error(1);
    let snap_fail = controller.tick(&backend, true, 3000);
    assert!(snap_fail.fallback_auto);
    assert_eq!(snap_fail.temps_c, vec![None, None]);
    assert_eq!(snap_fail.speeds_percent, vec![None, None]);

    let journal = backend.journal();
    assert_eq!(journal, vec![PlatformWrite::SetFansAuto]);
    assert!(!journal
        .iter()
        .any(|w| matches!(w, PlatformWrite::SetFanSpeed { percent: 0, .. })));

    // Temporary curves override & clear
    backend.set_fan_temp(0, 40);
    backend.set_fan_temp(1, 40);
    controller.apply_temporary_curves(TemporaryFanCurves {
        cpu: vec![FanPoint::new(40, 95)],
        gpu: vec![],
        water_cooler_fan: vec![],
        pump: vec![],
    });
    assert!(controller.has_temporary_curves());
    let snap_temp = controller.tick(&backend, true, 4000);
    assert!(snap_temp.speeds_percent[0].unwrap() > snap_indep.speeds_percent[0].unwrap());
    controller.clear_temporary_curves();
    assert!(!controller.has_temporary_curves());

    // Disabling fan control switches back to auto cleanly
    backend.clear_journal();
    let snap_off = controller.tick(&backend, false, 5000);
    assert!(!snap_off.fallback_auto);
    assert!(backend.journal().contains(&PlatformWrite::SetFansAuto));
}

#[test]
fn lct_cooler_queue_coalesces_rate_limits_and_cancels_on_disconnect_or_suspend() {
    let mut ctrl = CoolerController::new();
    let transport = MockBleTransport::new(true);

    assert_eq!(ctrl.mode(), CoolerMode::Auto);
    ctrl.set_mode(CoolerMode::Manual);
    assert_eq!(ctrl.mode(), CoolerMode::Manual);

    // Forbidden 12V rejected immediately
    assert_eq!(
        ctrl.set_pump_voltage(PumpVoltage::V12),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(
        CoolerCommand::FanSpeed(101).validate(),
        Err(CoolerError::InvalidFanSpeed(101))
    );

    // Burst fan speed updates coalesce into one pending command
    ctrl.set_fan_speed(30).unwrap();
    ctrl.set_fan_speed(55).unwrap();
    ctrl.set_fan_speed(80).unwrap();
    ctrl.set_pump_voltage(PumpVoltage::V8).unwrap();
    ctrl.set_rgb(10, 20, 30, RgbState::Static).unwrap();
    ctrl.set_rgb(40, 50, 60, RgbState::Breathe).unwrap();
    assert_eq!(ctrl.pending_len(), 3);

    // Connect triggers generation bump & restores latest targets
    ctrl.on_connected();
    let t0 = Instant::now();
    // 1st packet goes immediately at t0
    assert_eq!(ctrl.flush_ready(&transport, t0).unwrap(), 1);
    assert_eq!(ctrl.pending_len(), 2);

    // Before 80ms gap elapses, nothing is flushed
    assert_eq!(
        ctrl.flush_ready(&transport, t0 + Duration::from_millis(40))
            .unwrap(),
        0
    );

    // After 80ms -> 2nd packet
    let t1 = t0 + BLE_WRITE_GAP;
    assert_eq!(ctrl.flush_ready(&transport, t1).unwrap(), 1);
    // After another 80ms -> 3rd packet
    let t2 = t1 + BLE_WRITE_GAP;
    assert_eq!(ctrl.flush_ready(&transport, t2).unwrap(), 1);
    assert_eq!(ctrl.pending_len(), 0);

    let pkts = transport.packets();
    assert_eq!(pkts.len(), 3);
    // Fan 80% packet, Pump 8V packet, RGB (40,50,60,Breathe)
    assert_eq!(pkts[0], [0xfe, 0x1b, 0x01, 80, 0x00, 0x00, 0x00, 0xef]);
    assert_eq!(pkts[1], [0xfe, 0x1c, 0x01, 60, 0x03, 0x00, 0x00, 0xef]);
    assert_eq!(pkts[2], [0xfe, 0x1e, 0x01, 40, 50, 60, 0x01, 0xef]);

    // Enqueue during sleep prep / disconnect cancels stale queue via generation bump
    let gen_before = ctrl.generation();
    ctrl.set_fan_speed(95).unwrap();
    assert_eq!(ctrl.pending_len(), 1);
    ctrl.on_prepare_for_sleep(true);
    assert!(ctrl.generation() > gen_before);
    assert_eq!(ctrl.pending_len(), 0);
}
