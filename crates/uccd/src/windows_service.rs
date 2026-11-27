// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows Service Control Manager (SCM) lifecycle wrapper for `uccd` (`M15.1`, `ADR-06`).
//!
//! Provides a cross-platform testable SCM state machine ([`ServiceLifecycle`],
//! [`WindowsServiceConfig`], [`ServiceState`], [`ServiceControl`], [`ServiceStatusReporter`],
//! [`MockServiceStatusReporter`]) and real `#[cfg(windows)]` FFI bindings against `advapi32`
//! (`StartServiceCtrlDispatcherW`, `RegisterServiceCtrlHandlerExW`, `SetServiceStatus`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Canonical Windows Service name registered with SCM.
pub const WINDOWS_SERVICE_NAME: &str = "uccd";

/// Human-readable display name shown in Windows Services (`services.msc`).
pub const WINDOWS_SERVICE_DISPLAY_NAME: &str = "UCC Daemon (ucc-rusted)";

/// Default startup wait hint reported to SCM during `StartPending` (milliseconds).
pub const DEFAULT_STARTUP_HINT_MS: u32 = 5_000;

/// Default shutdown wait hint reported to SCM during `StopPending` (milliseconds).
pub const DEFAULT_SHUTDOWN_HINT_MS: u32 = 5_000;

/// Win32 `SERVICE_ACCEPT_STOP` bitflag.
pub const SERVICE_ACCEPT_STOP: u32 = 0x0000_0001;
/// Win32 `SERVICE_ACCEPT_SHUTDOWN` bitflag.
pub const SERVICE_ACCEPT_SHUTDOWN: u32 = 0x0000_0004;
/// Win32 `SERVICE_ACCEPT_POWEREVENT` bitflag.
pub const SERVICE_ACCEPT_POWEREVENT: u32 = 0x0000_0040;

/// Configuration for registering and running `uccd` as a Windows Service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsServiceConfig {
    pub service_name: String,
    pub display_name: String,
    pub startup_wait_hint_ms: u32,
    pub shutdown_wait_hint_ms: u32,
    pub accepts_power_events: bool,
}

impl Default for WindowsServiceConfig {
    fn default() -> Self {
        Self {
            service_name: WINDOWS_SERVICE_NAME.to_string(),
            display_name: WINDOWS_SERVICE_DISPLAY_NAME.to_string(),
            startup_wait_hint_ms: DEFAULT_STARTUP_HINT_MS,
            shutdown_wait_hint_ms: DEFAULT_SHUTDOWN_HINT_MS,
            accepts_power_events: true,
        }
    }
}

/// Windows SCM service lifecycle states matching Win32 `SERVICE_*` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceState {
    Stopped = 1,
    StartPending = 2,
    StopPending = 3,
    Running = 4,
}

impl ServiceState {
    /// Returns the corresponding Win32 `SERVICE_*` status code.
    pub const fn win32_code(self) -> u32 {
        self as u32
    }
}

/// Power management event subtype delivered via `SERVICE_CONTROL_POWEREVENT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerEventKind {
    Suspend,
    ResumeSuspend,
    ResumeAutomatic,
    PowerStatusChange,
    Other(u32),
}

impl PowerEventKind {
    /// Maps a Win32 `PBT_*` event code to [`PowerEventKind`].
    pub const fn from_win32_event_type(event_type: u32) -> Self {
        match event_type {
            0x0004 => Self::Suspend,
            0x0007 => Self::ResumeSuspend,
            0x000A => Self::PowerStatusChange,
            0x0012 => Self::ResumeAutomatic,
            other => Self::Other(other),
        }
    }
}

/// Control codes delivered by the Windows Service Control Manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceControl {
    Stop,
    Interrogate,
    Shutdown,
    PowerEvent(PowerEventKind),
    Unknown(u32),
}

impl ServiceControl {
    /// Decodes a raw Win32 `SERVICE_CONTROL_*` code and optional `event_type`.
    pub const fn from_win32(control_code: u32, event_type: u32) -> Self {
        match control_code {
            1 => Self::Stop,
            4 => Self::Interrogate,
            5 => Self::Shutdown,
            13 => Self::PowerEvent(PowerEventKind::from_win32_event_type(event_type)),
            other => Self::Unknown(other),
        }
    }
}

/// Snapshot of status fields reported to SCM via `SetServiceStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceStatusReport {
    pub state: ServiceState,
    pub checkpoint: u32,
    pub wait_hint_ms: u32,
    pub win32_exit_code: u32,
    pub accepted_controls: u32,
}

/// Errors produced by Windows Service lifecycle management or SCM FFI calls.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceError {
    #[error("invalid service state transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: ServiceState,
        to: ServiceState,
    },
    #[error("service status reporter failed: {0}")]
    ReporterFailed(String),
    #[error("service control {control:?} rejected in state {state:?}")]
    ControlRejected {
        state: ServiceState,
        control: ServiceControl,
    },
    #[error("Windows SCM error (code {code}): {message}")]
    ScmError { code: u32, message: String },
}

/// Sink for reporting [`ServiceStatusReport`] transitions to Windows SCM (or a mock in tests).
pub trait ServiceStatusReporter: Send + Sync {
    fn report_status(&self, status: &ServiceStatusReport) -> Result<(), ServiceError>;
}

/// In-memory mock [`ServiceStatusReporter`] for cross-platform lifecycle testing.
#[derive(Debug, Default)]
pub struct MockServiceStatusReporter {
    history: Mutex<Vec<ServiceStatusReport>>,
    fail_next: AtomicBool,
}

impl MockServiceStatusReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures whether the next `report_status` call should fail with
    /// [`ServiceError::ReporterFailed`].
    pub fn set_fail_next(&self, fail: bool) {
        self.fail_next.store(fail, Ordering::SeqCst);
    }

    /// Returns a cloned snapshot of all status reports received so far.
    pub fn history(&self) -> Vec<ServiceStatusReport> {
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Returns the most recently reported [`ServiceState`], if any.
    pub fn last_state(&self) -> Option<ServiceState> {
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .map(|r| r.state)
    }
}

impl ServiceStatusReporter for MockServiceStatusReporter {
    fn report_status(&self, status: &ServiceStatusReport) -> Result<(), ServiceError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(ServiceError::ReporterFailed(
                "injected mock SCM status failure".to_string(),
            ));
        }
        let mut hist = self.history.lock().unwrap_or_else(|e| e.into_inner());
        hist.push(*status);
        Ok(())
    }
}

/// Deterministic Windows Service Control Manager lifecycle state machine.
///
/// Enforces valid SCM transitions:
/// `Stopped -> StartPending -> Running -> StopPending -> Stopped`
/// (with `StartPending -> StopPending -> Stopped` or `StartPending -> Stopped` allowed on
/// startup abort/failure).
#[derive(Debug)]
pub struct ServiceLifecycle<R: ServiceStatusReporter> {
    config: WindowsServiceConfig,
    state: ServiceState,
    checkpoint: u32,
    stop_requested: Arc<AtomicBool>,
    power_events: Vec<PowerEventKind>,
    reporter: R,
}

impl<R: ServiceStatusReporter> ServiceLifecycle<R> {
    /// Creates a new lifecycle controller starting in [`ServiceState::Stopped`].
    pub fn new(config: WindowsServiceConfig, reporter: R) -> Self {
        Self {
            config,
            state: ServiceState::Stopped,
            checkpoint: 0,
            stop_requested: Arc::new(AtomicBool::new(false)),
            power_events: Vec::new(),
            reporter,
        }
    }

    /// Returns the current [`ServiceState`].
    pub const fn state(&self) -> ServiceState {
        self.state
    }

    /// Returns the active [`WindowsServiceConfig`].
    pub fn config(&self) -> &WindowsServiceConfig {
        &self.config
    }

    /// Returns a shared cooperative stop flag that becomes `true` on `Stop` / `Shutdown`.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop_requested)
    }

    /// Returns `true` once a stop or system shutdown control has been received.
    pub fn is_stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    /// Returns power events observed via `SERVICE_CONTROL_POWEREVENT`.
    pub fn power_events(&self) -> &[PowerEventKind] {
        &self.power_events
    }

    /// Returns a reference to the underlying [`ServiceStatusReporter`].
    pub fn reporter(&self) -> &R {
        &self.reporter
    }

    /// Returns the Win32 `dwControlsAccepted` mask for the given state.
    pub const fn accepted_controls_for_state(&self, state: ServiceState) -> u32 {
        match state {
            ServiceState::Running => {
                let mut mask = SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN;
                if self.config.accepts_power_events {
                    mask |= SERVICE_ACCEPT_POWEREVENT;
                }
                mask
            }
            ServiceState::Stopped | ServiceState::StartPending | ServiceState::StopPending => 0,
        }
    }

    /// Transitions `Stopped -> StartPending` and reports initial startup checkpoint `1`.
    pub fn begin_start(&mut self) -> Result<(), ServiceError> {
        if self.state != ServiceState::Stopped {
            return Err(ServiceError::InvalidTransition {
                from: self.state,
                to: ServiceState::StartPending,
            });
        }
        let report = ServiceStatusReport {
            state: ServiceState::StartPending,
            checkpoint: 1,
            wait_hint_ms: self.config.startup_wait_hint_ms,
            win32_exit_code: 0,
            accepted_controls: self.accepted_controls_for_state(ServiceState::StartPending),
        };
        self.reporter.report_status(&report)?;
        self.state = ServiceState::StartPending;
        self.checkpoint = 1;
        self.stop_requested.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Increments the startup progress checkpoint while in [`ServiceState::StartPending`].
    pub fn advance_start_checkpoint(&mut self) -> Result<u32, ServiceError> {
        if self.state != ServiceState::StartPending {
            return Err(ServiceError::InvalidTransition {
                from: self.state,
                to: ServiceState::StartPending,
            });
        }
        let next_checkpoint = self.checkpoint.saturating_add(1);
        let report = ServiceStatusReport {
            state: ServiceState::StartPending,
            checkpoint: next_checkpoint,
            wait_hint_ms: self.config.startup_wait_hint_ms,
            win32_exit_code: 0,
            accepted_controls: 0,
        };
        self.reporter.report_status(&report)?;
        self.checkpoint = next_checkpoint;
        Ok(next_checkpoint)
    }

    /// Transitions `StartPending -> Running` and advertises accepted SCM controls.
    pub fn mark_running(&mut self) -> Result<(), ServiceError> {
        if self.state != ServiceState::StartPending {
            return Err(ServiceError::InvalidTransition {
                from: self.state,
                to: ServiceState::Running,
            });
        }
        let report = ServiceStatusReport {
            state: ServiceState::Running,
            checkpoint: 0,
            wait_hint_ms: 0,
            win32_exit_code: 0,
            accepted_controls: self.accepted_controls_for_state(ServiceState::Running),
        };
        self.reporter.report_status(&report)?;
        self.state = ServiceState::Running;
        self.checkpoint = 0;
        Ok(())
    }

    /// Transitions `Running | StartPending -> StopPending` and sets the stop flag.
    pub fn begin_stop(&mut self) -> Result<(), ServiceError> {
        if !matches!(self.state, ServiceState::Running | ServiceState::StartPending) {
            return Err(ServiceError::InvalidTransition {
                from: self.state,
                to: ServiceState::StopPending,
            });
        }
        let report = ServiceStatusReport {
            state: ServiceState::StopPending,
            checkpoint: 1,
            wait_hint_ms: self.config.shutdown_wait_hint_ms,
            win32_exit_code: 0,
            accepted_controls: 0,
        };
        self.reporter.report_status(&report)?;
        self.state = ServiceState::StopPending;
        self.checkpoint = 1;
        self.stop_requested.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Transitions `StopPending | StartPending -> Stopped` with the given Win32 exit code.
    pub fn mark_stopped(&mut self, win32_exit_code: u32) -> Result<(), ServiceError> {
        if !matches!(
            self.state,
            ServiceState::StopPending | ServiceState::StartPending
        ) {
            return Err(ServiceError::InvalidTransition {
                from: self.state,
                to: ServiceState::Stopped,
            });
        }
        let report = ServiceStatusReport {
            state: ServiceState::Stopped,
            checkpoint: 0,
            wait_hint_ms: 0,
            win32_exit_code,
            accepted_controls: 0,
        };
        self.reporter.report_status(&report)?;
        self.state = ServiceState::Stopped;
        self.checkpoint = 0;
        Ok(())
    }

    /// Handles an incoming [`ServiceControl`] notification from SCM.
    pub fn handle_control(
        &mut self,
        control: ServiceControl,
    ) -> Result<ServiceState, ServiceError> {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                match self.state {
                    ServiceState::Running | ServiceState::StartPending => {
                        self.begin_stop()?;
                    }
                    ServiceState::StopPending | ServiceState::Stopped => {
                        self.stop_requested.store(true, Ordering::SeqCst);
                    }
                }
                Ok(self.state)
            }
            ServiceControl::Interrogate => {
                let wait_hint_ms = match self.state {
                    ServiceState::StartPending => self.config.startup_wait_hint_ms,
                    ServiceState::StopPending => self.config.shutdown_wait_hint_ms,
                    ServiceState::Running | ServiceState::Stopped => 0,
                };
                let report = ServiceStatusReport {
                    state: self.state,
                    checkpoint: self.checkpoint,
                    wait_hint_ms,
                    win32_exit_code: 0,
                    accepted_controls: self.accepted_controls_for_state(self.state),
                };
                self.reporter.report_status(&report)?;
                Ok(self.state)
            }
            ServiceControl::PowerEvent(kind) => {
                if self.config.accepts_power_events {
                    self.power_events.push(kind);
                }
                Ok(self.state)
            }
            ServiceControl::Unknown(_) => Err(ServiceError::ControlRejected {
                state: self.state,
                control,
            }),
        }
    }
}

#[cfg(windows)]
mod win32_scm {
    use super::*;
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use std::sync::OnceLock;

    const SERVICE_WIN32_OWN_PROCESS: u32 = 0x0000_0010;
    const NO_ERROR: u32 = 0;
    const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;

    type ServiceMainFn = extern "system" fn(u32, *mut *mut u16);
    type HandlerExFn = extern "system" fn(u32, u32, *mut c_void, *mut c_void) -> u32;

    #[repr(C)]
    struct ServiceTableEntryW {
        lp_service_name: *const u16,
        lp_service_proc: Option<ServiceMainFn>,
    }

    #[repr(C)]
    struct Win32ServiceStatus {
        dw_service_type: u32,
        dw_current_state: u32,
        dw_controls_accepted: u32,
        dw_win32_exit_code: u32,
        dw_service_specific_exit_code: u32,
        dw_check_point: u32,
        dw_wait_hint: u32,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn StartServiceCtrlDispatcherW(lp_service_start_table: *const ServiceTableEntryW) -> i32;
        fn RegisterServiceCtrlHandlerExW(
            lp_service_name: *const u16,
            lp_handler_proc: HandlerExFn,
            lp_context: *mut c_void,
        ) -> *mut c_void;
        fn SetServiceStatus(
            h_service_status: *mut c_void,
            lp_service_status: *const Win32ServiceStatus,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    fn encode_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    /// Real Win32 [`ServiceStatusReporter`] backed by `advapi32::SetServiceStatus`.
    #[derive(Debug, Clone, Copy)]
    pub struct Win32ServiceStatusReporter {
        status_handle: usize,
    }

    impl Win32ServiceStatusReporter {
        fn from_raw(handle: *mut c_void) -> Option<Self> {
            if handle.is_null() {
                None
            } else {
                Some(Self {
                    status_handle: handle as usize,
                })
            }
        }
    }

    impl ServiceStatusReporter for Win32ServiceStatusReporter {
        fn report_status(&self, status: &ServiceStatusReport) -> Result<(), ServiceError> {
            let native = Win32ServiceStatus {
                dw_service_type: SERVICE_WIN32_OWN_PROCESS,
                dw_current_state: status.state.win32_code(),
                dw_controls_accepted: status.accepted_controls,
                dw_win32_exit_code: status.win32_exit_code,
                dw_service_specific_exit_code: 0,
                dw_check_point: status.checkpoint,
                dw_wait_hint: status.wait_hint_ms,
            };
            // SAFETY: `status_handle` was obtained from `RegisterServiceCtrlHandlerExW`.
            let ok = unsafe { SetServiceStatus(self.status_handle as *mut c_void, &native) };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                return Err(ServiceError::ScmError {
                    code,
                    message: "SetServiceStatus failed".to_string(),
                });
            }
            Ok(())
        }
    }

    type ServiceRunner = Box<dyn FnOnce(Arc<AtomicBool>) -> u32 + Send + 'static>;

    struct ActiveServiceSlot {
        config: WindowsServiceConfig,
        runner: Option<ServiceRunner>,
        lifecycle: Option<Arc<Mutex<ServiceLifecycle<Win32ServiceStatusReporter>>>>,
    }

    fn active_service_slot() -> &'static Mutex<Option<ActiveServiceSlot>> {
        static SLOT: OnceLock<Mutex<Option<ActiveServiceSlot>>> = OnceLock::new();
        SLOT.get_or_init(|| Mutex::new(None))
    }

    extern "system" fn service_ctrl_handler(
        control: u32,
        event_type: u32,
        _event_data: *mut c_void,
        _context: *mut c_void,
    ) -> u32 {
        let parsed = ServiceControl::from_win32(control, event_type);
        let lifecycle = {
            let guard = active_service_slot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.as_ref().and_then(|s| s.lifecycle.as_ref().cloned())
        };
        let Some(lifecycle) = lifecycle else {
            return ERROR_CALL_NOT_IMPLEMENTED;
        };
        let mut guard = lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        match guard.handle_control(parsed) {
            Ok(_) => NO_ERROR,
            Err(_) => ERROR_CALL_NOT_IMPLEMENTED,
        }
    }

    extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
        let (config, runner) = {
            let mut guard = active_service_slot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(slot) = guard.as_mut() else {
                return;
            };
            (slot.config.clone(), slot.runner.take())
        };

        let wide_name = encode_wide(&config.service_name);
        // SAFETY: `wide_name` is null-terminated UTF-16 and `service_ctrl_handler` has
        // `extern "system"` ABI matching `LPHANDLER_FUNCTION_EX`.
        let raw_handle = unsafe {
            RegisterServiceCtrlHandlerExW(wide_name.as_ptr(), service_ctrl_handler, ptr::null_mut())
        };
        let Some(reporter) = Win32ServiceStatusReporter::from_raw(raw_handle) else {
            return;
        };

        let lifecycle = Arc::new(Mutex::new(ServiceLifecycle::new(config, reporter)));
        {
            let mut guard = active_service_slot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(slot) = guard.as_mut() {
                slot.lifecycle = Some(Arc::clone(&lifecycle));
            }
        }

        let stop_flag = {
            let mut guard = lifecycle.lock().unwrap_or_else(|e| e.into_inner());
            if guard.begin_start().is_err() {
                return;
            }
            if guard.mark_running().is_err() {
                let _ = guard.mark_stopped(1);
                return;
            }
            guard.stop_flag()
        };

        let exit_code = match runner {
            Some(run_fn) => run_fn(stop_flag),
            None => 0,
        };

        let mut guard = lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        if guard.state() == ServiceState::Running {
            let _ = guard.begin_stop();
        }
        let _ = guard.mark_stopped(exit_code);
    }

    /// Connects the process main thread to the Windows Service Control Manager via
    /// `StartServiceCtrlDispatcherW` and runs `runner` with a cooperative stop flag.
    pub fn run_windows_service<F>(
        config: WindowsServiceConfig,
        runner: F,
    ) -> Result<(), ServiceError>
    where
        F: FnOnce(Arc<AtomicBool>) -> u32 + Send + 'static,
    {
        let wide_name = encode_wide(&config.service_name);
        {
            let mut guard = active_service_slot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard = Some(ActiveServiceSlot {
                config,
                runner: Some(Box::new(runner)),
                lifecycle: None,
            });
        }

        let table = [
            ServiceTableEntryW {
                lp_service_name: wide_name.as_ptr(),
                lp_service_proc: Some(service_main),
            },
            ServiceTableEntryW {
                lp_service_name: ptr::null(),
                lp_service_proc: None,
            },
        ];

        // SAFETY: `table` is null-terminated as required by `StartServiceCtrlDispatcherW`
        // and `wide_name` remains alive until the dispatcher returns.
        let ok = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };

        {
            let mut guard = active_service_slot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard = None;
        }

        if ok == 0 {
            let code = unsafe { GetLastError() };
            return Err(ServiceError::ScmError {
                code,
                message: "StartServiceCtrlDispatcherW failed".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use win32_scm::{run_windows_service, Win32ServiceStatusReporter};
