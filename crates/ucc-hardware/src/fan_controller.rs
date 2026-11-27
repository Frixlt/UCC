//! Fan control orchestrator operating strictly against [`PlatformBackend`].

use crate::platform::{PlatformBackend, PlatformError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use ucc_core::{FanControlLogic, FanLogicType, FanProfile, TemporaryFanCurves};

/// Output of a single cooling control tick.
///
/// `speeds_percent` strictly reflects *confirmed applied* speeds (manual mode) or hardware
/// readback speeds (auto mode), never unconfirmed targets whose write failed (`W1-02`).
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
        self.base_profile = profile;
        self.consecutive_write_failures = 0;
        self.write_failsafe_active = false;
        self.sync_profiles();
    }

    pub fn apply_temporary_curves(&mut self, curves: TemporaryFanCurves) {
        self.temporary_curves = Some(curves);
        self.consecutive_write_failures = 0;
        self.write_failsafe_active = false;
        self.sync_profiles();
    }

    pub fn clear_temporary_curves(&mut self) {
        self.temporary_curves = None;
        self.consecutive_write_failures = 0;
        self.write_failsafe_active = false;
        self.sync_profiles();
    }

    pub fn has_temporary_curves(&self) -> bool {
        self.temporary_curves.is_some()
    }

    /// Restores firmware automatic fan control; clears active-control tracking only after success (`W1-01`).
    pub fn restore_auto(&mut self, backend: &dyn PlatformBackend) -> Result<(), PlatformError> {
        backend.set_fans_auto()?;
        self.was_control_enabled = false;
        self.in_failsafe_auto = false;
        self.consecutive_write_failures = 0;
        self.write_failsafe_active = false;
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
            self.consecutive_write_failures = 0;
            self.write_failsafe_active = false;
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

        // Fail-safe check (`W1-03`): if any active sensor lacks a curve OR no sensor is valid,
        // fall back the entire fan bank to firmware auto rather than borrowing 0% from another channel.
        let has_missing_curve = temps_c
            .iter()
            .zip(calculated_speeds.iter())
            .any(|(t, s)| t.is_some() && s.is_none());
        let highest_speed = if has_missing_curve {
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

        if self.write_failsafe_active {
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
                Ok(()) => speeds_percent.push(temps_c[i].map(|_| target)),
                Err(_) => {
                    speeds_percent.push(None);
                    any_write_failed = true;
                }
            }
        }

        if any_write_failed {
            self.consecutive_write_failures = self.consecutive_write_failures.saturating_add(1);
            if self.consecutive_write_failures >= 2 {
                self.write_failsafe_active = true;
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
            self.consecutive_write_failures = 0;
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

/// Runtime configuration passed to the asynchronous [`spawn_fan_worker`] loop (`M5.3` / `W1-09`).
#[derive(Debug, Clone)]
pub struct FanWorkerConfig {
    pub profile: FanProfile,
    pub fan_control_enabled: bool,
    pub same_speed: bool,
    pub temporary_curves: Option<TemporaryFanCurves>,
}

/// Handle controlling a running background [`FanController`] loop with guaranteed firmware-auto
/// restoration on explicit shutdown, backend errors, or worker cancellation (`W1-09`).
pub struct FanWorkerHandle {
    config_tx: watch::Sender<FanWorkerConfig>,
    stop_tx: Option<oneshot::Sender<()>>,
    latest_snapshot: Arc<Mutex<Option<FanTickSnapshot>>>,
    join_handle: Option<JoinHandle<Result<(), PlatformError>>>,
}

impl FanWorkerHandle {
    pub fn update_config(&self, config: FanWorkerConfig) {
        let _ = self.config_tx.send(config);
    }

    pub fn latest_snapshot(&self) -> Option<FanTickSnapshot> {
        self.latest_snapshot.lock().unwrap().clone()
    }

    /// Gracefully stops the worker loop and awaits firmware-auto restoration.
    pub async fn shutdown(mut self) -> Result<(), PlatformError> {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            match handle.await {
                Ok(res) => res,
                Err(_) => Err(PlatformError::Unavailable),
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for FanWorkerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Spawns a periodic asynchronous fan worker that applies updates, handles transient failures, and
/// restores firmware auto control upon shutdown or channel cancellation (`W1-09`).
pub fn spawn_fan_worker(
    backend: Arc<dyn PlatformBackend>,
    initial: FanWorkerConfig,
    interval: Duration,
) -> FanWorkerHandle {
    let (config_tx, mut config_rx) = watch::channel(initial.clone());
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let latest_snapshot = Arc::new(Mutex::new(None));
    let snapshot_ref = Arc::clone(&latest_snapshot);

    let join_handle = tokio::spawn(async move {
        let mut controller = FanController::new(
            backend.as_ref(),
            initial.profile.clone(),
            initial.same_speed,
        );
        if let Some(ref curves) = initial.temporary_curves {
            controller.apply_temporary_curves(curves.clone());
        }
        let mut current_cfg = initial;
        let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(10)));

        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    break;
                }
                changed = config_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    current_cfg = config_rx.borrow().clone();
                    controller.set_same_speed(current_cfg.same_speed);
                    controller.set_profile(current_cfg.profile.clone());
                    match &current_cfg.temporary_curves {
                        Some(curves) => controller.apply_temporary_curves(curves.clone()),
                        None => controller.clear_temporary_curves(),
                    }
                }
                _ = ticker.tick() => {
                    let snap = controller.tick(
                        backend.as_ref(),
                        current_cfg.fan_control_enabled,
                        current_epoch_ms(),
                    );
                    *snapshot_ref.lock().unwrap() = Some(snap);
                }
            }
        }

        // Bounded restoration attempts on exit
        let mut last_err = None;
        for _ in 0..3 {
            match controller.restore_auto(backend.as_ref()) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or(PlatformError::Unavailable))
    });

    FanWorkerHandle {
        config_tx,
        stop_tx: Some(stop_tx),
        latest_snapshot,
        join_handle: Some(join_handle),
    }
}
