//! Fan control orchestrator operating strictly against [`PlatformBackend`].

use crate::platform::{PlatformBackend, PlatformError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use ucc_core::{FanControlLogic, FanLogicType, FanProfile, TemporaryFanCurves};

/// Initial backoff before retrying manual fan writes after consecutive write failures.
pub const WRITE_FAILSAFE_INITIAL_BACKOFF_MS: i64 = 5_000;

/// Maximum exponential backoff cap for write-failsafe auto-recovery attempts.
pub const WRITE_FAILSAFE_MAX_BACKOFF_MS: i64 = 60_000;

/// Output of a single cooling control tick.
///
/// `speeds_percent` strictly reflects *confirmed applied* speeds (measured readback when available,
/// falling back to the confirmed target) or hardware readback speeds (auto mode), never unconfirmed
/// targets whose write failed (`W1-02`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanTickSnapshot {
    pub timestamp_ms: i64,
    pub temps_c: Vec<Option<i32>>,
    pub speeds_percent: Vec<Option<u8>>,
    pub calculated_speeds: Vec<Option<u8>>,
    pub fallback_auto: bool,
}

/// Manages per-fan [`FanControlLogic`] instances, `sameSpeed` sync, temporary curves,
/// and sensor/curve fail-safe fallback to automatic firmware control.
pub struct FanController {
    base_profile: FanProfile,
    fan_logics: Vec<FanControlLogic>,
    same_speed: bool,
    temporary_curves: Option<TemporaryFanCurves>,
    was_control_enabled: bool,
    in_failsafe_auto: bool,
    consecutive_write_failures: u32,
    write_failsafe_active: bool,
    write_failsafe_retry_after_ms: Option<i64>,
    write_failsafe_backoff_level: u32,
}

impl FanController {
    /// Initializes fan control logic from [`PlatformBackend`] capabilities.
    pub fn new(backend: &dyn PlatformBackend, profile: FanProfile, same_speed: bool) -> Self {
        let caps = backend.capabilities();
        let mut fan_count = caps.device.number_of_fans;

        // Match C++ FanControlWorker fallback when fan count query yields 0 but fan 0 sensor responds.
        if fan_count == 0 && backend.is_available() {
            if let Ok(temp) = backend.read_fan_temp(0) {
                if (0..=150).contains(&temp) {
                    fan_count = 2;
                }
            }
        }

        let mut fan_logics = Vec::with_capacity(fan_count);
        for i in 0..fan_count {
            let logic_type = if i == 0 {
                FanLogicType::Cpu
            } else {
                FanLogicType::Gpu
            };
            let mut logic = FanControlLogic::new(profile.clone(), logic_type);
            logic.set_fans_min_speed_hw_limit(caps.device.fans_min_speed);
            logic.set_fans_off_available(caps.device.fans_off_available);
            fan_logics.push(logic);
        }

        Self {
            base_profile: profile,
            fan_logics,
            same_speed,
            temporary_curves: None,
            was_control_enabled: false,
            in_failsafe_auto: false,
            consecutive_write_failures: 0,
            write_failsafe_active: false,
            write_failsafe_retry_after_ms: None,
            write_failsafe_backoff_level: 0,
        }
    }

    pub fn fan_count(&self) -> usize {
        self.fan_logics.len()
    }

    pub fn same_speed(&self) -> bool {
        self.same_speed
    }

    pub fn set_same_speed(&mut self, same: bool) {
        self.same_speed = same;
    }

    pub fn set_profile(&mut self, profile: FanProfile) {
        if self.base_profile == profile {
            return;
        }
        self.base_profile = profile;
        self.reset_write_failsafe();
        self.sync_profiles();
    }

    pub fn apply_temporary_curves(&mut self, curves: TemporaryFanCurves) {
        if self.temporary_curves.as_ref() == Some(&curves) {
            return;
        }
        self.temporary_curves = Some(curves);
        self.reset_write_failsafe();
        self.sync_profiles();
    }

    pub fn clear_temporary_curves(&mut self) {
        if self.temporary_curves.is_none() {
            return;
        }
        self.temporary_curves = None;
        self.reset_write_failsafe();
        self.sync_profiles();
    }

    pub fn has_temporary_curves(&self) -> bool {
        self.temporary_curves.is_some()
    }

    fn reset_write_failsafe(&mut self) {
        self.consecutive_write_failures = 0;
        self.write_failsafe_active = false;
        self.write_failsafe_retry_after_ms = None;
        self.write_failsafe_backoff_level = 0;
    }

    fn arm_write_failsafe(&mut self, timestamp_ms: i64) {
        let shift = self.write_failsafe_backoff_level.min(4);
        let backoff_ms = WRITE_FAILSAFE_INITIAL_BACKOFF_MS
            .saturating_mul(1i64 << shift)
            .min(WRITE_FAILSAFE_MAX_BACKOFF_MS);
        self.write_failsafe_active = true;
        self.write_failsafe_retry_after_ms = Some(timestamp_ms.saturating_add(backoff_ms));
        self.write_failsafe_backoff_level =
            self.write_failsafe_backoff_level.saturating_add(1).min(4);
    }

    /// Restores firmware automatic fan control; clears active-control tracking only after success (`W1-01`).
    pub fn restore_auto(&mut self, backend: &dyn PlatformBackend) -> Result<(), PlatformError> {
        backend.set_fans_auto()?;
        self.was_control_enabled = false;
        self.in_failsafe_auto = false;
        self.reset_write_failsafe();
        Ok(())
    }

    /// Runs one periodic cooling tick (~1s cadence).
    ///
    /// - Fail-safe (`M5.3` / `W1-01` / `W1-03`): when `fan_control_enabled` is `true` and either
    ///   no temperature sensors are valid OR curves are empty/absent, this NEVER commands `0%` fan speed;
    ///   instead it calls `backend.set_fans_auto()` and retries on subsequent ticks if that write fails.
    /// - Honest reporting (`W1-02`): failed `set_fan_speed_percent` writes yield `None` in
    ///   `speeds_percent` rather than pretending calculated speeds were applied.
    pub fn tick(
        &mut self,
        backend: &dyn PlatformBackend,
        fan_control_enabled: bool,
        timestamp_ms: i64,
    ) -> FanTickSnapshot {
        self.tick_cancellable(backend, fan_control_enabled, timestamp_ms, || false)
    }

    /// Runs one periodic cooling tick (~1s cadence), aborting before further hardware reads or
    /// writes if `is_cancelled()` becomes `true` (e.g. during daemon shutdown).
    pub fn tick_cancellable(
        &mut self,
        backend: &dyn PlatformBackend,
        fan_control_enabled: bool,
        timestamp_ms: i64,
        is_cancelled: impl Fn() -> bool,
    ) -> FanTickSnapshot {
        let count = self.fan_logics.len();
        if count == 0 {
            return FanTickSnapshot {
                timestamp_ms,
                temps_c: Vec::new(),
                speeds_percent: Vec::new(),
                calculated_speeds: Vec::new(),
                fallback_auto: false,
            };
        }

        let mut temps_c = Vec::with_capacity(count);
        let mut calculated_speeds = vec![None; count];

        for (i, slot) in calculated_speeds.iter_mut().enumerate().take(count) {
            if is_cancelled() {
                temps_c.resize(count, None);
                return FanTickSnapshot {
                    timestamp_ms,
                    temps_c,
                    speeds_percent: vec![None; count],
                    calculated_speeds,
                    fallback_auto: false,
                };
            }
            match backend.read_fan_temp(i) {
                Ok(temp) if (0..=150).contains(&temp) => {
                    temps_c.push(Some(temp));
                    *slot = self.fan_logics[i].report_temperature_checked(temp);
                }
                _ => temps_c.push(None),
            }
            if is_cancelled() {
                temps_c.resize(count, None);
                return FanTickSnapshot {
                    timestamp_ms,
                    temps_c,
                    speeds_percent: vec![None; count],
                    calculated_speeds,
                    fallback_auto: false,
                };
            }
        }

        if !fan_control_enabled {
            self.reset_write_failsafe();
            if self.was_control_enabled && !is_cancelled() && backend.set_fans_auto().is_ok() {
                self.was_control_enabled = false;
                self.in_failsafe_auto = false;
            }
            let mut speeds_percent = Vec::with_capacity(count);
            for (i, temp) in temps_c.iter().enumerate() {
                if temp.is_some() && !is_cancelled() {
                    speeds_percent.push(backend.read_fan_speed_percent(i).ok());
                } else {
                    speeds_percent.push(None);
                }
            }
            return FanTickSnapshot {
                timestamp_ms,
                temps_c,
                speeds_percent,
                calculated_speeds,
                fallback_auto: false,
            };
        }

        self.was_control_enabled = true;

        // Fail-safe check (`W1-03`, `TA-01`): if ANY fan channel's sensor failed/is out-of-range
        // or lacks a curve, fall back the entire fan bank to firmware auto rather than commanding
        // 0% to an unmonitored channel or borrowing another channel's cold temperature.
        let has_failed_sensor = temps_c.iter().any(Option::is_none);
        let has_missing_curve = temps_c
            .iter()
            .zip(calculated_speeds.iter())
            .any(|(t, s)| t.is_some() && s.is_none());
        let highest_speed = if has_failed_sensor || has_missing_curve {
            None
        } else {
            calculated_speeds.iter().flatten().copied().max()
        };
        let Some(highest_speed) = highest_speed else {
            if !self.in_failsafe_auto && !is_cancelled() && backend.set_fans_auto().is_ok() {
                self.in_failsafe_auto = true;
            }
            return FanTickSnapshot {
                timestamp_ms,
                temps_c,
                speeds_percent: vec![None; count],
                calculated_speeds,
                fallback_auto: self.in_failsafe_auto,
            };
        };

        let mut target_speeds = Vec::with_capacity(count);
        for calc in &calculated_speeds {
            let target = match calc {
                Some(spd) if !self.same_speed => *spd,
                _ => highest_speed,
            };
            target_speeds.push(target);
        }

        let probing_recovery = if self.write_failsafe_active {
            let recovery_due = self
                .write_failsafe_retry_after_ms
                .is_some_and(|retry_at| timestamp_ms >= retry_at);
            if !recovery_due {
                if !self.in_failsafe_auto && !is_cancelled() && backend.set_fans_auto().is_ok() {
                    self.in_failsafe_auto = true;
                }
                return FanTickSnapshot {
                    timestamp_ms,
                    temps_c,
                    speeds_percent: vec![None; count],
                    calculated_speeds: target_speeds.into_iter().map(Some).collect(),
                    fallback_auto: self.in_failsafe_auto,
                };
            }
            true
        } else {
            false
        };

        self.in_failsafe_auto = false;

        let mut speeds_percent = Vec::with_capacity(count);
        let mut any_write_failed = false;
        for (i, &target) in target_speeds.iter().enumerate() {
            if is_cancelled() {
                speeds_percent.resize(count, None);
                return FanTickSnapshot {
                    timestamp_ms,
                    temps_c,
                    speeds_percent,
                    calculated_speeds: target_speeds.into_iter().map(Some).collect(),
                    fallback_auto: false,
                };
            }
            match backend.set_fan_speed_percent(i, target) {
                Ok(()) => {
                    let measured = backend.read_fan_speed_percent(i).ok().unwrap_or(target);
                    speeds_percent.push(temps_c[i].map(|_| measured));
                }
                Err(_) => {
                    speeds_percent.push(None);
                    any_write_failed = true;
                }
            }
        }

        if any_write_failed {
            self.consecutive_write_failures = self.consecutive_write_failures.saturating_add(1);
            if probing_recovery || self.consecutive_write_failures >= 2 {
                self.arm_write_failsafe(timestamp_ms);
                if !is_cancelled() && backend.set_fans_auto().is_ok() {
                    self.in_failsafe_auto = true;
                }
                return FanTickSnapshot {
                    timestamp_ms,
                    temps_c,
                    speeds_percent: vec![None; count],
                    calculated_speeds: target_speeds.into_iter().map(Some).collect(),
                    fallback_auto: self.in_failsafe_auto,
                };
            }
        } else {
            self.reset_write_failsafe();
        }

        FanTickSnapshot {
            timestamp_ms,
            temps_c,
            speeds_percent,
            calculated_speeds: target_speeds.into_iter().map(Some).collect(),
            fallback_auto: false,
        }
    }

    fn sync_profiles(&mut self) {
        for (idx, logic) in self.fan_logics.iter_mut().enumerate() {
            let effective = match &self.temporary_curves {
                Some(curves) => curves.apply_to_profile(&self.base_profile, idx),
                None => self.base_profile.clone(),
            };
            logic.update_fan_profile(effective);
        }
    }
}

/// Runtime configuration passed to the background [`spawn_fan_worker`] loop (`M5.3` / `W1-09`).
#[derive(Debug, Clone)]
pub struct FanWorkerConfig {
    pub profile: FanProfile,
    pub fan_control_enabled: bool,
    pub same_speed: bool,
    pub temporary_curves: Option<TemporaryFanCurves>,
}

struct SharedWorkerState {
    config: FanWorkerConfig,
    generation: u64,
}

/// Handle controlling a running background [`FanController`] loop with guaranteed firmware-auto
/// restoration on explicit shutdown, backend errors, or worker cancellation (`W1-09`).
pub struct FanWorkerHandle {
    backend: Arc<dyn PlatformBackend>,
    shared: Arc<(Mutex<SharedWorkerState>, Condvar)>,
    stop_flag: Arc<AtomicBool>,
    latest_snapshot: Arc<Mutex<Option<FanTickSnapshot>>>,
    join_handle: Option<JoinHandle<Result<(), PlatformError>>>,
    shutdown_completed: bool,
}

impl FanWorkerHandle {
    pub fn update_config(&self, config: FanWorkerConfig) {
        let (lock, cvar) = &*self.shared;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        guard.config = config;
        guard.generation = guard.generation.wrapping_add(1);
        cvar.notify_all();
    }

    pub fn latest_snapshot(&self) -> Option<FanTickSnapshot> {
        self.latest_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Gracefully stops the worker loop and awaits firmware-auto restoration.
    pub async fn shutdown(mut self) -> Result<(), PlatformError> {
        self.stop_flag.store(true, Ordering::Release);
        self.shared.1.notify_all();
        let res = if let Some(handle) = self.join_handle.take() {
            match handle.await {
                Ok(res) => res,
                Err(_) => Err(PlatformError::Unavailable),
            }
        } else {
            Ok(())
        };
        self.shutdown_completed = true;
        res
    }
}

impl Drop for FanWorkerHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        self.shared.1.notify_all();
        if !self.shutdown_completed {
            let _ = self.backend.set_fans_auto();
        }
    }
}

fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Spawns a periodic fan worker on Tokio's blocking thread pool (off async worker threads) that
/// applies updates, handles transient failures, and restores firmware auto control upon shutdown
/// or handle drop (`W1-09`).
pub fn spawn_fan_worker(
    backend: Arc<dyn PlatformBackend>,
    initial: FanWorkerConfig,
    interval: Duration,
) -> FanWorkerHandle {
    let shared = Arc::new((
        Mutex::new(SharedWorkerState {
            config: initial.clone(),
            generation: 0,
        }),
        Condvar::new(),
    ));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let latest_snapshot = Arc::new(Mutex::new(None));

    let worker_backend = Arc::clone(&backend);
    let worker_shared = Arc::clone(&shared);
    let worker_stop = Arc::clone(&stop_flag);
    let snapshot_ref = Arc::clone(&latest_snapshot);
    let tick_interval = interval.max(Duration::from_millis(10));

    let join_handle = tokio::task::spawn_blocking(move || {
        let mut controller = FanController::new(
            worker_backend.as_ref(),
            initial.profile.clone(),
            initial.same_speed,
        );
        if let Some(ref curves) = initial.temporary_curves {
            controller.apply_temporary_curves(curves.clone());
        }
        let mut current_cfg = initial;
        let mut seen_generation = 0u64;

        loop {
            if worker_stop.load(Ordering::Acquire) {
                break;
            }

            // Pick up any pending config update before ticking.
            {
                let (lock, _) = &*worker_shared;
                let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                if guard.generation != seen_generation {
                    seen_generation = guard.generation;
                    let next_cfg = guard.config.clone();
                    controller.set_same_speed(next_cfg.same_speed);
                    if next_cfg.profile != current_cfg.profile {
                        controller.set_profile(next_cfg.profile.clone());
                    }
                    if next_cfg.temporary_curves != current_cfg.temporary_curves {
                        match &next_cfg.temporary_curves {
                            Some(curves) => controller.apply_temporary_curves(curves.clone()),
                            None => controller.clear_temporary_curves(),
                        }
                    }
                    current_cfg = next_cfg;
                }
            }

            let snap = controller.tick_cancellable(
                worker_backend.as_ref(),
                current_cfg.fan_control_enabled,
                current_epoch_ms(),
                || worker_stop.load(Ordering::Acquire),
            );
            *snapshot_ref.lock().unwrap_or_else(|e| e.into_inner()) = Some(snap);

            if worker_stop.load(Ordering::Acquire) {
                break;
            }

            // Sleep until next tick interval, waking immediately on stop or config update.
            let (lock, cvar) = &*worker_shared;
            let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            if !worker_stop.load(Ordering::Acquire) && guard.generation == seen_generation {
                let _ = cvar.wait_timeout(guard, tick_interval);
            }
        }

        // Bounded restoration attempts with exponential backoff on exit
        let mut last_err = None;
        for attempt in 0..3u32 {
            match controller.restore_auto(worker_backend.as_ref()) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_err = Some(e);
                    if attempt < 2 {
                        let backoff = Duration::from_millis(25 * (1u64 << attempt));
                        std::thread::sleep(backoff);
                    }
                }
            }
        }
        Err(last_err.unwrap_or(PlatformError::Unavailable))
    });

    FanWorkerHandle {
        backend,
        shared,
        stop_flag,
        latest_snapshot,
        join_handle: Some(join_handle),
        shutdown_completed: false,
    }
}
