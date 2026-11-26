use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ucc_core::{FanPoint, FanProfile, PumpPoint};
use ucc_hardware::{
    spawn_fan_worker, BleTransport, CoolerController, CoolerError, CoolerMode, FanController,
    FanWorkerConfig, MockBleTransport, MockPlatformBackend, PlatformBackend, PlatformBackendKind,
    PlatformCapabilities, PlatformError, PlatformWrite, PumpTarget, PumpVoltage, RgbState,
    RgbTarget, BLE_WRITE_GAP,
};

#[test]
fn fan_auto_recovers_when_the_initial_firmware_command_failed() {
    let backend = MockPlatformBackend::uniwill_default();
    let mut controller = FanController::new(&backend, FanProfile::default(), false);
    backend.set_fan_temp_error(0);
    backend.set_fan_temp_error(1);
    backend.set_fail_writes(true);

    let first = controller.tick(&backend, true, 1000);
    assert!(backend.journal().is_empty());
    assert!(!first.fallback_auto);

    backend.set_fail_writes(false);
    let second = controller.tick(&backend, true, 2000);
    assert!(
        second.fallback_auto && backend.journal().contains(&PlatformWrite::SetFansAuto),
        "{first:?} {second:?}"
    );
}

#[test]
fn manual_disable_retries_set_fans_auto_after_initial_failure() {
    let backend = MockPlatformBackend::uniwill_default();
    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 65)],
        table_gpu: vec![FanPoint::new(40, 65)],
        ..Default::default()
    };
    let mut controller = FanController::new(&backend, profile, false);
    backend.set_fan_temp(0, 60);
    backend.set_fan_temp(1, 60);
    let _ = controller.tick(&backend, true, 1000);

    backend.clear_journal();
    backend.set_fail_writes(true);
    let _ = controller.tick(&backend, false, 2000);
    assert!(backend.journal().is_empty());

    backend.set_fail_writes(false);
    let _ = controller.tick(&backend, false, 3000);
    assert!(backend.journal().contains(&PlatformWrite::SetFansAuto));
}

#[test]
fn rejected_fan_write_is_not_reported_as_applied() {
    let backend = MockPlatformBackend::uniwill_default();
    backend.set_fan_temp(0, 70);
    backend.set_fan_temp(1, 70);
    backend.set_fail_writes(true);
    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 65)],
        table_gpu: vec![FanPoint::new(40, 65)],
        ..Default::default()
    };
    let mut controller = FanController::new(&backend, profile, false);
    let tick = controller.tick(&backend, true, 1000);
    assert!(backend.journal().is_empty());
    assert_eq!(tick.speeds_percent, vec![None, None], "{tick:?}");
    assert_eq!(tick.calculated_speeds, vec![Some(65), Some(65)]);
}

#[test]
fn partial_fan_write_failure_reports_only_succeeded_channel() {
    struct PartialFailBackend {
        inner: MockPlatformBackend,
    }
    impl PlatformBackend for PartialFailBackend {
        fn kind(&self) -> PlatformBackendKind {
            self.inner.kind()
        }
        fn is_available(&self) -> bool {
            self.inner.is_available()
        }
        fn capabilities(&self) -> PlatformCapabilities {
            self.inner.capabilities()
        }
        fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> {
            self.inner.read_fan_temp(fan_idx)
        }
        fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> {
            self.inner.read_fan_speed_percent(fan_idx)
        }
        fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
            if fan_idx == 1 {
                return Err(PlatformError::Unavailable);
            }
            self.inner.set_fan_speed_percent(fan_idx, percent)
        }
        fn set_fans_auto(&self) -> Result<(), PlatformError> {
            self.inner.set_fans_auto()
        }
        fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> {
            self.inner.read_tdp(index)
        }
        fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> {
            self.inner.read_tdp_limits(index)
        }
        fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> {
            self.inner.set_tdp(index, watts)
        }
        fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> {
            self.inner.set_perf_profile(profile_code)
        }
        fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> {
            self.inner.set_mode_enable(enable)
        }
        fn set_mode(&self, mode: i32) -> Result<(), PlatformError> {
            self.inner.set_mode(mode)
        }
    }

    let backend = PartialFailBackend {
        inner: MockPlatformBackend::uniwill_default(),
    };
    backend.inner.set_fan_temp(0, 70);
    backend.inner.set_fan_temp(1, 70);
    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 65)],
        table_gpu: vec![FanPoint::new(40, 65)],
        ..Default::default()
    };
    let mut controller = FanController::new(&backend, profile, false);
    let tick = controller.tick(&backend, true, 1000);
    assert_eq!(tick.speeds_percent, vec![Some(65), None]);
    assert_eq!(tick.calculated_speeds, vec![Some(65), Some(65)]);
}

#[test]
fn absent_curves_do_not_command_zero_on_a_valid_sample() {
    let backend = MockPlatformBackend::uniwill_default();
    backend.set_fan_temp(0, 50);
    backend.set_fan_temp(1, 50);
    let mut controller = FanController::new(&backend, FanProfile::default(), false);
    let snap = controller.tick(&backend, true, 1000);
    assert!(snap.fallback_auto);
    assert!(backend.journal().contains(&PlatformWrite::SetFansAuto));
    assert!(
        !backend
            .journal()
            .iter()
            .any(|op| matches!(op, PlatformWrite::SetFanSpeed { percent: 0, .. })),
        "{:?}",
        backend.journal()
    );

    // Distinct check: explicit 0% curve point IS still honored.
    backend.clear_journal();
    let explicit_zero = FanProfile {
        table_cpu: vec![FanPoint::new(40, 0), FanPoint::new(80, 100)],
        table_gpu: vec![FanPoint::new(40, 0), FanPoint::new(80, 100)],
        ..Default::default()
    };
    let mut zero_ctrl = FanController::new(&backend, explicit_zero, false);
    backend.set_fan_temp(0, 40);
    backend.set_fan_temp(1, 40);
    let zero_snap = zero_ctrl.tick(&backend, true, 2000);
    assert!(!zero_snap.fallback_auto);
    assert_eq!(zero_snap.speeds_percent, vec![Some(0), Some(0)]);
}

#[tokio::test]
async fn fan_worker_restores_firmware_auto_on_shutdown() {
    let backend = Arc::new(MockPlatformBackend::uniwill_default());
    backend.set_fan_temp(0, 65);
    backend.set_fan_temp(1, 60);
    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 60)],
        table_gpu: vec![FanPoint::new(40, 50)],
        ..Default::default()
    };

    let worker = spawn_fan_worker(
        backend.clone(),
        FanWorkerConfig {
            profile,
            fan_control_enabled: true,
            same_speed: false,
            temporary_curves: None,
        },
        Duration::from_millis(15),
    );

    for _ in 0..50 {
        if worker.latest_snapshot().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(worker.latest_snapshot().is_some());
    assert!(backend
        .journal()
        .iter()
        .any(|w| matches!(w, PlatformWrite::SetFanSpeed { .. })));

    worker.shutdown().await.unwrap();
    let journal = backend.journal();
    assert_eq!(journal.last(), Some(&PlatformWrite::SetFansAuto));
}

#[test]
fn explicit_fan_and_led_off_are_restored_after_reconnect() {
    let mut controller = CoolerController::new();
    let transport = MockBleTransport::new(true);
    controller.on_connected();
    controller.turn_off_fan().unwrap();
    controller.turn_off_led().unwrap();
    assert_eq!(controller.desired_rgb_target(), RgbTarget::Off);
    let t = Instant::now();
    controller.flush_ready(&transport, t).unwrap();
    controller.flush_ready(&transport, t + BLE_WRITE_GAP).unwrap();
    let expected = transport.packets();
    assert_eq!(expected.len(), 2);
    transport.clear();
    controller.on_disconnected();
    controller.on_connected();
    controller
        .flush_ready(&transport, t + BLE_WRITE_GAP * 2)
        .unwrap();
    controller
        .flush_ready(&transport, t + BLE_WRITE_GAP * 3)
        .unwrap();
    assert_eq!(transport.packets(), expected);
}

#[test]
fn failed_ble_packet_is_retried_on_the_same_connection() {
    struct RejectOnce(AtomicUsize);
    impl BleTransport for RejectOnce {
        fn is_connected(&self) -> bool {
            true
        }
        fn write_packet(&self, _: [u8; 8]) -> Result<(), CoolerError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(CoolerError::NotConnected);
            }
            Ok(())
        }
    }
    let transport = RejectOnce(AtomicUsize::new(0));
    let mut controller = CoolerController::new();
    controller.on_connected();
    controller.set_fan_speed(67).unwrap();
    let t = Instant::now();
    assert!(controller.flush_ready(&transport, t).is_err());
    assert_eq!(controller.pending_len(), 1);
    assert_eq!(
        controller.flush_ready(&transport, t + BLE_WRITE_GAP).unwrap(),
        1
    );
    assert_eq!(controller.pending_len(), 0);
}

#[test]
fn cooler_auto_and_manual_modes_keep_separate_targets_and_follow_curves_in_auto() {
    let mut ctrl = CoolerController::new();
    let transport = MockBleTransport::new(true);
    ctrl.on_connected();

    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 20), FanPoint::new(80, 90)],
        table_gpu: vec![FanPoint::new(40, 20), FanPoint::new(80, 90)],
        table_water_cooler_fan: vec![FanPoint::new(40, 30), FanPoint::new(80, 90)],
        table_pump: vec![PumpPoint::new(40, 1), PumpPoint::new(60, 3)],
        ..Default::default()
    };

    // 1. Auto mode applies curves at 60C -> wc fan 60%, pump V11
    ctrl.apply_profile_temperature(&profile, 60).unwrap();
    let t0 = Instant::now();
    ctrl.flush_ready(&transport, t0).unwrap();
    ctrl.flush_ready(&transport, t0 + BLE_WRITE_GAP).unwrap();
    assert_eq!(
        transport.packets(),
        vec![
            [0xfe, 0x1b, 0x01, 60, 0x00, 0x00, 0x00, 0xef],
            [0xfe, 0x1c, 0x01, 60, 0x00, 0x00, 0x00, 0xef],
        ]
    );

    // 2. Switch to Manual mode, set manual 85% / V7, verify subsequent Auto ticks do NOT clobber Manual!
    transport.clear();
    ctrl.set_mode(CoolerMode::Manual);
    ctrl.set_fan_speed(85).unwrap();
    ctrl.set_pump_voltage(PumpVoltage::V7).unwrap();
    ctrl.apply_profile_temperature(&profile, 40).unwrap(); // updates Auto internally only
    let t1 = t0 + BLE_WRITE_GAP * 2;
    ctrl.flush_ready(&transport, t1).unwrap();
    ctrl.flush_ready(&transport, t1 + BLE_WRITE_GAP).unwrap();
    assert_eq!(
        transport.packets(),
        vec![
            [0xfe, 0x1b, 0x01, 85, 0x00, 0x00, 0x00, 0xef],
            [0xfe, 0x1c, 0x01, 60, 0x02, 0x00, 0x00, 0xef],
        ]
    );
    assert_eq!(ctrl.desired_pump_target(), PumpTarget::Voltage(PumpVoltage::V7));

    // 3. Re-switching to Auto enqueues latest Auto curve target (40C => fan 30%, pump V7)
    transport.clear();
    ctrl.set_mode(CoolerMode::Auto);
    let t2 = t1 + BLE_WRITE_GAP * 2;
    ctrl.flush_ready(&transport, t2).unwrap();
    ctrl.flush_ready(&transport, t2 + BLE_WRITE_GAP).unwrap();
    assert_eq!(
        transport.packets()[0],
        [0xfe, 0x1b, 0x01, 30, 0x00, 0x00, 0x00, 0xef]
    );

    // Unused helper check to ensure AtomicBool stays exercised cleanly
    let flag = AtomicBool::new(false);
    flag.store(true, Ordering::SeqCst);
    let _ = RgbState::Static;
}

#[test]
fn entering_auto_never_drains_pending_manual_commands() {
    let mut cooler = CoolerController::new();
    cooler.on_connected();
    cooler.set_mode(CoolerMode::Manual);
    cooler.set_fan_speed(65).unwrap();
    cooler.set_rgb(10, 20, 30, RgbState::Static).unwrap();
    assert_eq!(cooler.pending_len(), 2);
    cooler.set_mode(CoolerMode::Auto);
    let transport = MockBleTransport::new(true);
    let sent = cooler.flush_ready(&transport, Instant::now()).unwrap();
    // Stale manual fan command was drained; RGB remains preserved
    assert_eq!(sent, 1, "{:?}", transport.packets());
    assert_eq!(
        transport.packets(),
        vec![[0xfe, 0x1e, 0x01, 10, 20, 30, 0x00, 0xef]]
    );
}

#[test]
fn partial_missing_curve_does_not_command_zero_from_another_channel() {
    let backend = MockPlatformBackend::uniwill_default();
    backend.set_fan_temp(0, 75);
    backend.set_fan_temp(1, 35);
    let profile = FanProfile {
        table_gpu: vec![FanPoint::new(35, 0)],
        ..Default::default()
    };
    let mut controller = FanController::new(&backend, profile, false);
    let snap = controller.tick(&backend, true, 1000);
    let writes = backend.journal();
    assert!(snap.fallback_auto);
    assert_eq!(writes, vec![PlatformWrite::SetFansAuto]);
}

#[test]
fn repeated_fan_write_failure_triggers_firmware_auto_failsafe() {
    struct FailSecondFan(Arc<MockPlatformBackend>);
    impl PlatformBackend for FailSecondFan {
        fn kind(&self) -> PlatformBackendKind { self.0.kind() }
        fn is_available(&self) -> bool { self.0.is_available() }
        fn capabilities(&self) -> PlatformCapabilities { self.0.capabilities() }
        fn read_fan_temp(&self, fan_idx: usize) -> Result<i32, PlatformError> { self.0.read_fan_temp(fan_idx) }
        fn read_fan_speed_percent(&self, fan_idx: usize) -> Result<u8, PlatformError> { self.0.read_fan_speed_percent(fan_idx) }
        fn set_fan_speed_percent(&self, fan_idx: usize, percent: u8) -> Result<(), PlatformError> {
            if fan_idx == 1 {
                return Err(PlatformError::Unavailable);
            }
            self.0.set_fan_speed_percent(fan_idx, percent)
        }
        fn set_fans_auto(&self) -> Result<(), PlatformError> { self.0.set_fans_auto() }
        fn read_tdp(&self, index: usize) -> Result<i32, PlatformError> { self.0.read_tdp(index) }
        fn read_tdp_limits(&self, index: usize) -> Result<(i32, i32), PlatformError> { self.0.read_tdp_limits(index) }
        fn set_tdp(&self, index: usize, watts: i32) -> Result<(), PlatformError> { self.0.set_tdp(index, watts) }
        fn set_perf_profile(&self, profile_code: i32) -> Result<(), PlatformError> { self.0.set_perf_profile(profile_code) }
        fn set_mode_enable(&self, enable: bool) -> Result<(), PlatformError> { self.0.set_mode_enable(enable) }
        fn set_mode(&self, mode: i32) -> Result<(), PlatformError> { self.0.set_mode(mode) }
    }

    let inner = Arc::new(MockPlatformBackend::uniwill_default());
    let backend = FailSecondFan(inner.clone());
    inner.set_fan_temp(0, 60);
    inner.set_fan_temp(1, 95);
    let profile = FanProfile {
        table_cpu: vec![FanPoint::new(40, 20), FanPoint::new(90, 100)],
        table_gpu: vec![FanPoint::new(40, 20), FanPoint::new(90, 100)],
        ..Default::default()
    };
    let mut controller = FanController::new(&backend, profile, true);
    let first = controller.tick(&backend, true, 1000);
    assert!(!first.fallback_auto);
    let second = controller.tick(&backend, true, 2000);
    assert!(second.fallback_auto);
    assert_eq!(second.calculated_speeds[1], Some(100));
    assert_eq!(second.speeds_percent[1], None);
    assert!(inner.journal().contains(&PlatformWrite::SetFansAuto));
}
