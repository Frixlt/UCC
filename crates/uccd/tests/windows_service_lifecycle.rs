// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::Ordering;
use uccd::windows_service::{
    MockServiceStatusReporter, PowerEventKind, ServiceControl, ServiceError, ServiceLifecycle,
    ServiceState, WindowsServiceConfig, DEFAULT_SHUTDOWN_HINT_MS, DEFAULT_STARTUP_HINT_MS,
    SERVICE_ACCEPT_POWEREVENT, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP,
    WINDOWS_SERVICE_DISPLAY_NAME, WINDOWS_SERVICE_NAME,
};

#[test]
fn windows_service_default_config_matches_uccd_service_identity() {
    let cfg = WindowsServiceConfig::default();
    assert_eq!(cfg.service_name, WINDOWS_SERVICE_NAME);
    assert_eq!(cfg.service_name, "uccd");
    assert_eq!(cfg.display_name, WINDOWS_SERVICE_DISPLAY_NAME);
    assert_eq!(cfg.display_name, "UCC Daemon (ucc-rusted)");
    assert_eq!(cfg.startup_wait_hint_ms, DEFAULT_STARTUP_HINT_MS);
    assert_eq!(cfg.shutdown_wait_hint_ms, DEFAULT_SHUTDOWN_HINT_MS);
    assert!(cfg.accepts_power_events);
}

#[test]
fn windows_service_lifecycle_completes_valid_scm_transitions_with_checkpoints() {
    let reporter = MockServiceStatusReporter::new();
    let mut lifecycle = ServiceLifecycle::new(WindowsServiceConfig::default(), reporter);

    assert_eq!(lifecycle.state(), ServiceState::Stopped);
    assert!(!lifecycle.is_stop_requested());

    lifecycle.begin_start().unwrap();
    assert_eq!(lifecycle.state(), ServiceState::StartPending);

    let cp2 = lifecycle.advance_start_checkpoint().unwrap();
    assert_eq!(cp2, 2);

    lifecycle.mark_running().unwrap();
    assert_eq!(lifecycle.state(), ServiceState::Running);

    let state_after_stop = lifecycle.handle_control(ServiceControl::Stop).unwrap();
    assert_eq!(state_after_stop, ServiceState::StopPending);
    assert!(lifecycle.is_stop_requested());
    assert!(lifecycle.stop_flag().load(Ordering::SeqCst));

    lifecycle.mark_stopped(0).unwrap();
    assert_eq!(lifecycle.state(), ServiceState::Stopped);

    let history = lifecycle.reporter().history();
    assert_eq!(history.len(), 5);
    assert_eq!(history[0].state, ServiceState::StartPending);
    assert_eq!(history[0].checkpoint, 1);
    assert_eq!(history[0].accepted_controls, 0);

    assert_eq!(history[1].state, ServiceState::StartPending);
    assert_eq!(history[1].checkpoint, 2);

    assert_eq!(history[2].state, ServiceState::Running);
    assert_eq!(history[2].checkpoint, 0);
    assert_eq!(
        history[2].accepted_controls,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_POWEREVENT
    );

    assert_eq!(history[3].state, ServiceState::StopPending);
    assert_eq!(history[3].checkpoint, 1);
    assert_eq!(history[3].accepted_controls, 0);

    assert_eq!(history[4].state, ServiceState::Stopped);
    assert_eq!(history[4].win32_exit_code, 0);
}

#[test]
fn windows_service_handles_interrogate_power_events_and_shutdown_controls() {
    let mut lifecycle = ServiceLifecycle::new(
        WindowsServiceConfig::default(),
        MockServiceStatusReporter::new(),
    );
    lifecycle.begin_start().unwrap();
    lifecycle.mark_running().unwrap();

    // Interrogate re-reports Running without mutating state.
    let interrogated = lifecycle.handle_control(ServiceControl::Interrogate).unwrap();
    assert_eq!(interrogated, ServiceState::Running);
    assert_eq!(lifecycle.reporter().last_state(), Some(ServiceState::Running));
    assert_eq!(lifecycle.reporter().history().len(), 3);

    // Power events are recorded while remaining Running.
    assert_eq!(
        lifecycle
            .handle_control(ServiceControl::PowerEvent(PowerEventKind::Suspend))
            .unwrap(),
        ServiceState::Running
    );
    assert_eq!(
        lifecycle
            .handle_control(ServiceControl::PowerEvent(PowerEventKind::ResumeAutomatic))
            .unwrap(),
        ServiceState::Running
    );
    assert_eq!(
        lifecycle.power_events(),
        &[PowerEventKind::Suspend, PowerEventKind::ResumeAutomatic]
    );

    // System Shutdown transitions Running -> StopPending and is idempotent if repeated.
    assert_eq!(
        lifecycle.handle_control(ServiceControl::Shutdown).unwrap(),
        ServiceState::StopPending
    );
    assert!(lifecycle.is_stop_requested());
    assert_eq!(
        lifecycle.handle_control(ServiceControl::Stop).unwrap(),
        ServiceState::StopPending
    );
}

#[test]
fn windows_service_rejects_invalid_transitions_and_unknown_controls() {
    let mut lifecycle = ServiceLifecycle::new(
        WindowsServiceConfig::default(),
        MockServiceStatusReporter::new(),
    );

    // Cannot jump directly from Stopped to Running or Stopped.
    assert_eq!(
        lifecycle.mark_running(),
        Err(ServiceError::InvalidTransition {
            from: ServiceState::Stopped,
            to: ServiceState::Running,
        })
    );
    assert_eq!(
        lifecycle.mark_stopped(0),
        Err(ServiceError::InvalidTransition {
            from: ServiceState::Stopped,
            to: ServiceState::Stopped,
        })
    );

    lifecycle.begin_start().unwrap();
    lifecycle.mark_running().unwrap();

    // Cannot skip StopPending when moving Running -> Stopped.
    assert_eq!(
        lifecycle.mark_stopped(0),
        Err(ServiceError::InvalidTransition {
            from: ServiceState::Running,
            to: ServiceState::Stopped,
        })
    );

    // Cannot re-enter StartPending from Running.
    assert_eq!(
        lifecycle.begin_start(),
        Err(ServiceError::InvalidTransition {
            from: ServiceState::Running,
            to: ServiceState::StartPending,
        })
    );

    // Unknown control code is rejected.
    assert_eq!(
        lifecycle.handle_control(ServiceControl::Unknown(99)),
        Err(ServiceError::ControlRejected {
            state: ServiceState::Running,
            control: ServiceControl::Unknown(99),
        })
    );
}

#[test]
fn windows_service_reporter_failure_preserves_previous_state() {
    let reporter = MockServiceStatusReporter::new();
    reporter.set_fail_next(true);
    let mut lifecycle = ServiceLifecycle::new(WindowsServiceConfig::default(), reporter);

    let err = lifecycle.begin_start().unwrap_err();
    assert!(matches!(err, ServiceError::ReporterFailed(_)));
    assert_eq!(lifecycle.state(), ServiceState::Stopped);
    assert!(lifecycle.reporter().history().is_empty());

    // Subsequent call succeeds once reporter recovers.
    lifecycle.begin_start().unwrap();
    assert_eq!(lifecycle.state(), ServiceState::StartPending);
}

#[test]
fn win32_control_and_power_event_decoders_map_standard_codes() {
    assert_eq!(ServiceControl::from_win32(1, 0), ServiceControl::Stop);
    assert_eq!(ServiceControl::from_win32(4, 0), ServiceControl::Interrogate);
    assert_eq!(ServiceControl::from_win32(5, 0), ServiceControl::Shutdown);
    assert_eq!(
        ServiceControl::from_win32(13, 0x0004),
        ServiceControl::PowerEvent(PowerEventKind::Suspend)
    );
    assert_eq!(
        ServiceControl::from_win32(13, 0x0007),
        ServiceControl::PowerEvent(PowerEventKind::ResumeSuspend)
    );
    assert_eq!(
        ServiceControl::from_win32(13, 0x000A),
        ServiceControl::PowerEvent(PowerEventKind::PowerStatusChange)
    );
    assert_eq!(
        ServiceControl::from_win32(13, 0x0012),
        ServiceControl::PowerEvent(PowerEventKind::ResumeAutomatic)
    );
    assert_eq!(
        ServiceControl::from_win32(255, 0),
        ServiceControl::Unknown(255)
    );
}
