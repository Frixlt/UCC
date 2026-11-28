//! Liquid Cooled Technology (LCT) BLE Water Cooler protocol and state machine.
//!
//! Controls external LCT water coolers (e.g. LCT21001, LCT22002) over Nordic UART Service (NUS).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Nordic UART Service (NUS) UUIDs
pub const NORDIC_UART_SERVICE_UUID: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
pub const NORDIC_UART_CHAR_TX: &str = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";
pub const NORDIC_UART_CHAR_RX: &str = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";

/// Timing constants matching C++ implementation
pub const BLE_WRITE_GAP: Duration = Duration::from_millis(80);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
pub const KEEPALIVE_MISS_THRESHOLD: u32 = 3;
pub const FAST_RECONNECT_MAX_FAILURES: u32 = 3;
pub const GATT_CACHE_PURGE_FAILURE_THRESHOLD: u32 = 3;

/// Mandatory liquid-cooler overheat failsafe thresholds (enforced even in `CoolerMode::Manual`).
pub const OVERHEAT_TRIGGER_TEMP_C: i32 = 90;
pub const OVERHEAT_RELEASE_TEMP_C: i32 = 80;

/// Protocol command IDs
pub const CMD_RESET: u8 = 0x19;
pub const CMD_FAN: u8 = 0x1b;
pub const CMD_PUMP: u8 = 0x1c;
pub const CMD_RGB: u8 = 0x1e;

pub const PACKET_HEADER: u8 = 0xfe;
pub const PACKET_FOOTER: u8 = 0xef;
pub const PACKET_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PumpVoltage {
    V11 = 0x00,
    V12 = 0x01, // Note: forbidden on LCT coolers in production
    V7 = 0x02,
    V8 = 0x03,
    Off = 0x04,
}

impl TryFrom<u8> for PumpVoltage {
    type Error = CoolerError;

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0x00 => Ok(PumpVoltage::V11),
            0x01 => Ok(PumpVoltage::V12),
            0x02 => Ok(PumpVoltage::V7),
            0x03 => Ok(PumpVoltage::V8),
            0x04 => Ok(PumpVoltage::Off),
            _ => Err(CoolerError::InvalidVoltageCode(val)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RgbState {
    Static = 0x00,
    Breathe = 0x01,
    Colorful = 0x02,
    BreatheColor = 0x03,
    Temperature = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolerState {
    Disconnected,
    Discovering,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoolerError {
    #[error("Invalid packet length: expected 8 bytes, got {0}")]
    InvalidPacketLength(usize),
    #[error("Invalid packet framing: header=0x{0:02x}, footer=0x{1:02x}")]
    InvalidFraming(u8, u8),
    #[error("Invalid fan speed percentage: {0} (must be 0..=100)")]
    InvalidFanSpeed(u8),
    #[error("V12 voltage is forbidden for this device")]
    V12Forbidden,
    #[error("Invalid pump voltage code: {0}")]
    InvalidVoltageCode(u8),
    #[error("Device not connected")]
    NotConnected,
    #[error("Overheat protection is active; cooling cannot be reduced or disabled")]
    OverheatProtectionActive,
}

/// Pure protocol packet encoder
pub struct CoolerPacketEncoder;

impl CoolerPacketEncoder {
    /// Encode a reset command: `fe 19 00 01 00 00 00 ef`
    pub fn reset() -> [u8; 8] {
        [
            PACKET_HEADER,
            CMD_RESET,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            PACKET_FOOTER,
        ]
    }

    /// Encode fan duty cycle percentage (0..=100)
    pub fn fan_speed(duty_cycle_percent: u8) -> Result<[u8; 8], CoolerError> {
        if duty_cycle_percent > 100 {
            return Err(CoolerError::InvalidFanSpeed(duty_cycle_percent));
        }
        Ok([
            PACKET_HEADER,
            CMD_FAN,
            0x01, // Enable
            duty_cycle_percent,
            0x00,
            0x00,
            0x00,
            PACKET_FOOTER,
        ])
    }

    /// Encode pump voltage
    pub fn pump_voltage(voltage: PumpVoltage) -> Result<[u8; 8], CoolerError> {
        if voltage == PumpVoltage::V12 {
            return Err(CoolerError::V12Forbidden);
        }
        if voltage == PumpVoltage::Off {
            return Ok(Self::turn_off(CMD_PUMP));
        }
        Ok([
            PACKET_HEADER,
            CMD_PUMP,
            0x01, // Enable
            60,   // Fixed duty cycle 60%
            voltage as u8,
            0x00,
            0x00,
            PACKET_FOOTER,
        ])
    }

    /// Encode RGB LED color and effect
    pub fn rgb(red: u8, green: u8, blue: u8, state: RgbState) -> [u8; 8] {
        [
            PACKET_HEADER,
            CMD_RGB,
            0x01, // Enable
            red,
            green,
            blue,
            state as u8,
            PACKET_FOOTER,
        ]
    }

    /// Encode turn off command for a specific subsystem
    pub fn turn_off(cmd: u8) -> [u8; 8] {
        [
            PACKET_HEADER,
            cmd,
            0x00, // Disable
            0x00,
            0x00,
            0x00,
            0x00,
            PACKET_FOOTER,
        ]
    }

    /// Validate and parse an incoming packet
    pub fn validate_packet(raw: &[u8]) -> Result<(), CoolerError> {
        if raw.len() != PACKET_LEN {
            return Err(CoolerError::InvalidPacketLength(raw.len()));
        }
        if raw[0] != PACKET_HEADER || raw[7] != PACKET_FOOTER {
            return Err(CoolerError::InvalidFraming(raw[0], raw[7]));
        }
        Ok(())
    }
}

/// BLE Write rate limiter ensuring at least `BLE_WRITE_GAP` (80 ms) between writes
pub struct BleWriteLimiter {
    last_write: Option<Instant>,
    min_gap: Duration,
}

impl Default for BleWriteLimiter {
    fn default() -> Self {
        Self::new(BLE_WRITE_GAP)
    }
}

impl BleWriteLimiter {
    pub fn new(min_gap: Duration) -> Self {
        Self {
            last_write: None,
            min_gap,
        }
    }

    /// Calculates required delay before next write can be sent using wall clock.
    pub fn required_delay(&self) -> Duration {
        self.required_delay_at(Instant::now())
    }

    /// Deterministic delay calculation relative to a supplied `now` instant.
    pub fn required_delay_at(&self, now: Instant) -> Duration {
        match self.last_write {
            Some(last) => {
                let elapsed = now.saturating_duration_since(last);
                if elapsed < self.min_gap {
                    self.min_gap - elapsed
                } else {
                    Duration::ZERO
                }
            }
            None => Duration::ZERO,
        }
    }

    /// Record that a write was sent now.
    pub fn record_write(&mut self) {
        self.record_write_at(Instant::now());
    }

    /// Deterministic write timestamp recording for tests.
    pub fn record_write_at(&mut self, now: Instant) {
        self.last_write = Some(now);
    }
}

/// State machine tracking connection health, reconnect backoff, and keepalives
pub struct CoolerStateMachine {
    state: CoolerState,
    consecutive_failures: u32,
    missed_keepalives: u32,
    last_keepalive_sent: Option<Instant>,
}

impl Default for CoolerStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl CoolerStateMachine {
    pub fn new() -> Self {
        Self {
            state: CoolerState::Disconnected,
            consecutive_failures: 0,
            missed_keepalives: 0,
            last_keepalive_sent: None,
        }
    }

    pub fn state(&self) -> CoolerState {
        self.state
    }

    pub fn is_connected(&self) -> bool {
        self.state == CoolerState::Connected
    }

    pub fn on_connected(&mut self) {
        self.state = CoolerState::Connected;
        self.consecutive_failures = 0;
        self.missed_keepalives = 0;
        self.last_keepalive_sent = Some(Instant::now());
    }

    pub fn on_disconnected(&mut self) {
        self.state = CoolerState::Disconnected;
    }

    pub fn on_connection_failure(&mut self) -> bool {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= FAST_RECONNECT_MAX_FAILURES {
            self.state = CoolerState::Failed;
            true // Trigger full scan / cache purge
        } else {
            self.state = CoolerState::Reconnecting;
            false
        }
    }

    pub fn should_send_keepalive(&self) -> bool {
        if self.state != CoolerState::Connected {
            return false;
        }
        match self.last_keepalive_sent {
            Some(last) => last.elapsed() >= KEEPALIVE_INTERVAL,
            None => true,
        }
    }

    pub fn on_keepalive_sent(&mut self) {
        self.last_keepalive_sent = Some(Instant::now());
    }

    pub fn on_keepalive_ack(&mut self) {
        self.missed_keepalives = 0;
    }

    pub fn on_keepalive_missed(&mut self) -> bool {
        self.missed_keepalives += 1;
        if self.missed_keepalives >= KEEPALIVE_MISS_THRESHOLD {
            self.state = CoolerState::Disconnected;
            true // Declared dead, trigger reconnect
        } else {
            false
        }
    }
}

/// Control mode for the external LCT liquid cooler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoolerMode {
    #[default]
    Auto,
    Manual,
}

/// High-level LCT cooler command queued for throttled BLE transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolerCommand {
    FanSpeed(u8),
    PumpVoltage(PumpVoltage),
    Rgb {
        red: u8,
        green: u8,
        blue: u8,
        state: RgbState,
    },
    TurnOffFan,
    TurnOffPump,
    TurnOffLed,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandSlot {
    Fan,
    Pump,
    Rgb,
    Reset,
}

impl CoolerCommand {
    pub fn validate(self) -> Result<(), CoolerError> {
        match self {
            Self::FanSpeed(percent) if percent > 100 => Err(CoolerError::InvalidFanSpeed(percent)),
            Self::PumpVoltage(PumpVoltage::V12) => Err(CoolerError::V12Forbidden),
            _ => Ok(()),
        }
    }

    pub fn encode(self) -> Result<[u8; 8], CoolerError> {
        match self {
            Self::FanSpeed(percent) => CoolerPacketEncoder::fan_speed(percent),
            Self::PumpVoltage(voltage) => CoolerPacketEncoder::pump_voltage(voltage),
            Self::Rgb {
                red,
                green,
                blue,
                state,
            } => Ok(CoolerPacketEncoder::rgb(red, green, blue, state)),
            Self::TurnOffFan => Ok(CoolerPacketEncoder::turn_off(CMD_FAN)),
            Self::TurnOffPump => Ok(CoolerPacketEncoder::turn_off(CMD_PUMP)),
            Self::TurnOffLed => Ok(CoolerPacketEncoder::turn_off(CMD_RGB)),
            Self::Reset => Ok(CoolerPacketEncoder::reset()),
        }
    }

    fn slot(self) -> CommandSlot {
        match self {
            Self::FanSpeed(_) | Self::TurnOffFan => CommandSlot::Fan,
            Self::PumpVoltage(_) | Self::TurnOffPump => CommandSlot::Pump,
            Self::Rgb { .. } | Self::TurnOffLed => CommandSlot::Rgb,
            Self::Reset => CommandSlot::Reset,
        }
    }
}

/// Queued command stamped with a connection generation to cancel stale pre-disconnect writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedCoolerCommand {
    pub generation: u64,
    pub command: CoolerCommand,
}

/// Coalescing, generation-aware, 80ms rate-limited command queue (`M5.6`).
pub struct CoolerCommandQueue {
    generation: u64,
    queue: VecDeque<QueuedCoolerCommand>,
    limiter: BleWriteLimiter,
}

impl Default for CoolerCommandQueue {
    fn default() -> Self {
        Self::new(BLE_WRITE_GAP)
    }
}

impl CoolerCommandQueue {
    pub fn new(min_gap: Duration) -> Self {
        Self {
            generation: 0,
            queue: VecDeque::new(),
            limiter: BleWriteLimiter::new(min_gap),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn pending_len(&self) -> usize {
        self.queue.len()
    }

    /// Removes any queued fan/pump commands while preserving pending RGB/Reset and the 80ms rate-limiter (`W1-11`).
    pub fn clear_fan_and_pump(&mut self) {
        self.queue
            .retain(|entry| !matches!(entry.command.slot(), CommandSlot::Fan | CommandSlot::Pump));
    }

    /// Cancels all pending commands and invalidates any previously stamped generation.
    pub fn bump_generation(&mut self) -> u64 {
        self.generation += 1;
        self.queue.clear();
        self.generation
    }

    /// Validates and enqueues a command, coalescing superseded pending writes for the same subsystem.
    pub fn enqueue(&mut self, command: CoolerCommand) -> Result<(), CoolerError> {
        command.validate()?;
        let slot = command.slot();
        if let Some(existing) = self
            .queue
            .iter_mut()
            .find(|entry| entry.command.slot() == slot)
        {
            existing.command = command;
            existing.generation = self.generation;
        } else {
            self.queue.push_back(QueuedCoolerCommand {
                generation: self.generation,
                command,
            });
        }
        Ok(())
    }

    pub fn next_delay_at(&self, now: Instant) -> Duration {
        self.limiter.required_delay_at(now)
    }

    /// Peeks at the next ready command without removing it from the queue (`W1-05`).
    pub fn peek_ready_at(&mut self, now: Instant) -> Option<QueuedCoolerCommand> {
        while let Some(front) = self.queue.front() {
            if front.generation != self.generation {
                self.queue.pop_front();
                continue;
            }
            if !self.limiter.required_delay_at(now).is_zero() {
                return None;
            }
            return Some(*front);
        }
        None
    }

    /// Removes the head command after a confirmed transport write and records the throttle timestamp.
    pub fn commit_sent_at(&mut self, now: Instant) {
        self.queue.pop_front();
        self.limiter.record_write_at(now);
    }

    /// Leaves the failed command queued for retry on the same connection while advancing the 80ms
    /// BLE write-gap timer (`W1-05`).
    pub fn record_write_attempt_at(&mut self, now: Instant) {
        self.limiter.record_write_at(now);
    }

    /// Pops the next queued command if its generation is current and the 80ms BLE write gap elapsed.
    pub fn pop_ready_at(&mut self, now: Instant) -> Option<QueuedCoolerCommand> {
        let ready = self.peek_ready_at(now)?;
        self.commit_sent_at(now);
        Some(ready)
    }
}

/// Persisted fan target distinguishing unset from explicit `Off` (`W1-04`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FanTarget {
    #[default]
    Unset,
    Off,
    Speed(u8),
}

/// Persisted pump voltage target (`PumpVoltage::Off` is explicit off).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PumpTarget {
    #[default]
    Unset,
    Voltage(PumpVoltage),
}

/// Persisted RGB target distinguishing unset from explicit `Off` (`W1-04`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RgbTarget {
    #[default]
    Unset,
    Off,
    Color {
        red: u8,
        green: u8,
        blue: u8,
        state: RgbState,
    },
}

fn from_core_pump_voltage(v: ucc_core::PumpVoltage) -> PumpVoltage {
    match v {
        ucc_core::PumpVoltage::V11 => PumpVoltage::V11,
        ucc_core::PumpVoltage::V12 => PumpVoltage::V12,
        ucc_core::PumpVoltage::V7 => PumpVoltage::V7,
        ucc_core::PumpVoltage::V8 => PumpVoltage::V8,
        ucc_core::PumpVoltage::Off => PumpVoltage::Off,
    }
}

/// Abstract BLE NUS TX transport for LCT21001/LCT22002 water coolers.
pub trait BleTransport: Send + Sync {
    fn is_connected(&self) -> bool;
    fn write_packet(&self, packet: [u8; 8]) -> Result<(), CoolerError>;
}

/// In-memory mock [`BleTransport`] recording all 8-byte packets written.
#[derive(Debug, Default)]
pub struct MockBleTransport {
    connected: std::sync::atomic::AtomicBool,
    packets: Arc<Mutex<Vec<[u8; 8]>>>,
}

impl MockBleTransport {
    pub fn new(connected: bool) -> Self {
        Self {
            connected: std::sync::atomic::AtomicBool::new(connected),
            packets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected
            .store(connected, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn packets(&self) -> Vec<[u8; 8]> {
        self.packets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear(&self) {
        self.packets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl BleTransport for MockBleTransport {
    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn write_packet(&self, packet: [u8; 8]) -> Result<(), CoolerError> {
        if !self.is_connected() {
            return Err(CoolerError::NotConnected);
        }
        CoolerPacketEncoder::validate_packet(&packet)?;
        self.packets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(packet);
        Ok(())
    }
}

/// High-level LCT cooler controller combining connection state, coalescing queue,
/// manual/auto mode, retry-safe queue flush, suspend cancellation, and reconnect restore (`M5.4`–`M5.7`).
pub struct CoolerController {
    state_machine: CoolerStateMachine,
    queue: CoolerCommandQueue,
    mode: CoolerMode,
    auto_fan: FanTarget,
    auto_pump: PumpTarget,
    manual_fan: FanTarget,
    manual_pump: PumpTarget,
    desired_rgb: RgbTarget,
    suspending: bool,
    overheat_override: bool,
}

impl Default for CoolerController {
    fn default() -> Self {
        Self::new()
    }
}

impl CoolerController {
    pub fn new() -> Self {
        Self {
            state_machine: CoolerStateMachine::new(),
            queue: CoolerCommandQueue::default(),
            mode: CoolerMode::Auto,
            auto_fan: FanTarget::Unset,
            auto_pump: PumpTarget::Unset,
            manual_fan: FanTarget::Unset,
            manual_pump: PumpTarget::Unset,
            desired_rgb: RgbTarget::Unset,
            suspending: false,
            overheat_override: false,
        }
    }

    pub fn mode(&self) -> CoolerMode {
        self.mode
    }

    /// Switches between [`CoolerMode::Auto`] and [`CoolerMode::Manual`], purging stale queued
    /// fan/pump commands from the previous mode before enqueuing active targets (`W1-11`).
    pub fn set_mode(&mut self, mode: CoolerMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.queue.clear_fan_and_pump();
        self.enqueue_active_fan_and_pump();
    }

    pub fn state(&self) -> CoolerState {
        self.state_machine.state()
    }

    pub fn pending_len(&self) -> usize {
        self.queue.pending_len()
    }

    pub fn generation(&self) -> u64 {
        self.queue.generation()
    }

    pub fn is_overheat_override_active(&self) -> bool {
        self.overheat_override
    }

    pub fn desired_fan_target(&self) -> FanTarget {
        if self.overheat_override {
            return FanTarget::Speed(100);
        }
        match self.mode {
            CoolerMode::Auto => self.auto_fan,
            CoolerMode::Manual => self.manual_fan,
        }
    }

    pub fn desired_pump_target(&self) -> PumpTarget {
        if self.overheat_override {
            return PumpTarget::Voltage(PumpVoltage::V11);
        }
        match self.mode {
            CoolerMode::Auto => self.auto_pump,
            CoolerMode::Manual => self.manual_pump,
        }
    }

    pub fn desired_rgb_target(&self) -> RgbTarget {
        self.desired_rgb
    }

    /// Evaluates water cooler fan & pump curves for `temp_c` (`W1-11`).
    ///
    /// Always keeps the Auto targets up to date; only enqueues hardware commands when
    /// `mode() == CoolerMode::Auto` (never clobbering `Manual` overrides).
    pub fn apply_profile_temperature(
        &mut self,
        profile: &ucc_core::FanProfile,
        temp_c: i32,
    ) -> Result<(), CoolerError> {
        self.apply_profile_temperature_force(profile, temp_c, false)
    }

    /// Evaluates water cooler fan & pump curves for `temp_c`, optionally forcing hardware enqueue even
    /// when targets match cached `auto_fan`/`auto_pump` (used after connect or profile switch) (`M5.9`).
    pub fn apply_profile_temperature_force(
        &mut self,
        profile: &ucc_core::FanProfile,
        temp_c: i32,
        force: bool,
    ) -> Result<(), CoolerError> {
        let sensor_invalid = !(0..=150).contains(&temp_c);
        let mut fan_changed = false;
        let mut pump_changed = false;

        if !sensor_invalid {
            let curve_fan = profile.get_water_cooler_fan_speed_for_temp(temp_c);
            let next_fan = if curve_fan >= 0 {
                FanTarget::Speed(curve_fan.clamp(0, 100) as u8)
            } else {
                self.auto_fan
            };

            let next_pump = if !profile.table_pump.is_empty() {
                PumpTarget::Voltage(from_core_pump_voltage(
                    profile.get_pump_speed_for_temp(temp_c),
                ))
            } else {
                self.auto_pump
            };

            fan_changed = next_fan != self.auto_fan;
            pump_changed = next_pump != self.auto_pump;
            self.auto_fan = next_fan;
            self.auto_pump = next_pump;
        }

        let was_overheat = self.overheat_override;
        if sensor_invalid || temp_c >= OVERHEAT_TRIGGER_TEMP_C {
            self.overheat_override = true;
        } else if self.overheat_override && temp_c < OVERHEAT_RELEASE_TEMP_C {
            self.overheat_override = false;
        }

        if self.overheat_override {
            if !was_overheat || force {
                self.queue.clear_fan_and_pump();
                self.queue.enqueue(CoolerCommand::FanSpeed(100))?;
                self.queue
                    .enqueue(CoolerCommand::PumpVoltage(PumpVoltage::V11))?;
            }
            return Ok(());
        }

        if was_overheat {
            self.queue.clear_fan_and_pump();
            self.enqueue_active_fan_and_pump();
            return Ok(());
        }

        if self.mode == CoolerMode::Auto {
            if force || fan_changed {
                Self::enqueue_fan_target(&mut self.queue, self.auto_fan)?;
            }
            if force || pump_changed {
                Self::enqueue_pump_target(&mut self.queue, self.auto_pump)?;
            }
        }
        Ok(())
    }

    /// Synchronizes manual fan/pump targets from the active profile (`M5.9`).
    ///
    /// Keeps `manual_fan`/`manual_pump` updated even while `Auto` mode is active, and
    /// immediately purges & re-enqueues active targets when already in `Manual` mode.
    pub fn apply_manual_profile_targets(
        &mut self,
        fan_speed: Option<u8>,
        pump_voltage_code: Option<i32>,
    ) {
        if let Some(spd) = fan_speed {
            self.manual_fan = if spd == 0 {
                FanTarget::Off
            } else {
                FanTarget::Speed(spd.min(100))
            };
        }
        if let Some(code) = pump_voltage_code {
            if let Ok(core_v) = ucc_core::PumpVoltage::from_discrete_code(code) {
                let hw_v = from_core_pump_voltage(core_v);
                if CoolerCommand::PumpVoltage(hw_v).validate().is_ok() {
                    self.manual_pump = PumpTarget::Voltage(hw_v);
                }
            }
        }
        if self.mode == CoolerMode::Manual {
            self.queue.clear_fan_and_pump();
            self.enqueue_active_fan_and_pump();
        }
    }

    /// Seeds `desired_rgb` from persisted config when no explicit RGB state has been set yet (`M5.9`).
    pub fn seed_desired_rgb_if_unset(&mut self, red: u8, green: u8, blue: u8, state: RgbState) {
        if self.desired_rgb == RgbTarget::Unset {
            self.desired_rgb = RgbTarget::Color {
                red,
                green,
                blue,
                state,
            };
        }
    }

    /// Re-enqueues current active fan, pump, and RGB targets (e.g. after debounced reconnect or profile switch) (`M5.9`).
    pub fn force_reapply_active_targets(&mut self) {
        self.queue.clear_fan_and_pump();
        self.enqueue_active_fan_and_pump();
        self.enqueue_desired_rgb();
    }

    pub fn set_fan_speed(&mut self, percent: u8) -> Result<(), CoolerError> {
        let cmd = CoolerCommand::FanSpeed(percent);
        cmd.validate()?;
        if self.overheat_override && percent < 100 {
            return Err(CoolerError::OverheatProtectionActive);
        }
        match self.mode {
            CoolerMode::Auto => self.auto_fan = FanTarget::Speed(percent),
            CoolerMode::Manual => self.manual_fan = FanTarget::Speed(percent),
        }
        self.queue.enqueue(cmd)
    }

    pub fn set_pump_voltage(&mut self, voltage: PumpVoltage) -> Result<(), CoolerError> {
        let cmd = CoolerCommand::PumpVoltage(voltage);
        cmd.validate()?;
        if self.overheat_override && voltage != PumpVoltage::V11 {
            return Err(CoolerError::OverheatProtectionActive);
        }
        match self.mode {
            CoolerMode::Auto => self.auto_pump = PumpTarget::Voltage(voltage),
            CoolerMode::Manual => self.manual_pump = PumpTarget::Voltage(voltage),
        }
        self.queue.enqueue(cmd)
    }

    /// Atomically validates `percent`, switches to [`CoolerMode::Manual`], and enqueues the command.
    ///
    /// Never switches away from [`CoolerMode::Auto`] if validation or queueing fails (`TA-02`).
    pub fn set_manual_fan_speed(&mut self, percent: u8) -> Result<(), CoolerError> {
        self.apply_manual_fan_cmd(CoolerCommand::FanSpeed(percent), FanTarget::Speed(percent))
    }

    /// Atomically validates `voltage`, switches to [`CoolerMode::Manual`], and enqueues the command (`TA-02`).
    pub fn set_manual_pump_voltage(&mut self, voltage: PumpVoltage) -> Result<(), CoolerError> {
        self.apply_manual_pump_cmd(
            CoolerCommand::PumpVoltage(voltage),
            PumpTarget::Voltage(voltage),
        )
    }

    /// Atomically switches to [`CoolerMode::Manual`] and turns off the cooler fan (`TA-02`).
    pub fn turn_off_manual_fan(&mut self) -> Result<(), CoolerError> {
        self.apply_manual_fan_cmd(CoolerCommand::TurnOffFan, FanTarget::Off)
    }

    /// Atomically switches to [`CoolerMode::Manual`] and turns off the cooler pump (`TA-02`).
    pub fn turn_off_manual_pump(&mut self) -> Result<(), CoolerError> {
        self.apply_manual_pump_cmd(
            CoolerCommand::TurnOffPump,
            PumpTarget::Voltage(PumpVoltage::Off),
        )
    }

    fn apply_manual_fan_cmd(
        &mut self,
        cmd: CoolerCommand,
        target: FanTarget,
    ) -> Result<(), CoolerError> {
        cmd.validate()?;
        if self.overheat_override && !matches!(cmd, CoolerCommand::FanSpeed(100)) {
            return Err(CoolerError::OverheatProtectionActive);
        }
        let prev_mode = self.mode;
        let prev_target = self.manual_fan;
        if prev_mode != CoolerMode::Manual {
            self.queue.clear_fan_and_pump();
            self.mode = CoolerMode::Manual;
            let pump = self.desired_pump_target();
            let _ = Self::enqueue_pump_target(&mut self.queue, pump);
        }
        self.manual_fan = target;
        if let Err(err) = self.queue.enqueue(cmd) {
            self.manual_fan = prev_target;
            self.mode = prev_mode;
            return Err(err);
        }
        Ok(())
    }

    fn apply_manual_pump_cmd(
        &mut self,
        cmd: CoolerCommand,
        target: PumpTarget,
    ) -> Result<(), CoolerError> {
        cmd.validate()?;
        if self.overheat_override && !matches!(cmd, CoolerCommand::PumpVoltage(PumpVoltage::V11)) {
            return Err(CoolerError::OverheatProtectionActive);
        }
        let prev_mode = self.mode;
        let prev_target = self.manual_pump;
        if prev_mode != CoolerMode::Manual {
            self.queue.clear_fan_and_pump();
            self.mode = CoolerMode::Manual;
            let fan = self.desired_fan_target();
            let _ = Self::enqueue_fan_target(&mut self.queue, fan);
        }
        self.manual_pump = target;
        if let Err(err) = self.queue.enqueue(cmd) {
            self.manual_pump = prev_target;
            self.mode = prev_mode;
            return Err(err);
        }
        Ok(())
    }

    pub fn set_rgb(
        &mut self,
        red: u8,
        green: u8,
        blue: u8,
        state: RgbState,
    ) -> Result<(), CoolerError> {
        self.desired_rgb = RgbTarget::Color {
            red,
            green,
            blue,
            state,
        };
        self.queue.enqueue(CoolerCommand::Rgb {
            red,
            green,
            blue,
            state,
        })
    }

    pub fn turn_off_fan(&mut self) -> Result<(), CoolerError> {
        if self.overheat_override {
            return Err(CoolerError::OverheatProtectionActive);
        }
        match self.mode {
            CoolerMode::Auto => self.auto_fan = FanTarget::Off,
            CoolerMode::Manual => self.manual_fan = FanTarget::Off,
        }
        self.queue.enqueue(CoolerCommand::TurnOffFan)
    }

    pub fn turn_off_pump(&mut self) -> Result<(), CoolerError> {
        if self.overheat_override {
            return Err(CoolerError::OverheatProtectionActive);
        }
        match self.mode {
            CoolerMode::Auto => self.auto_pump = PumpTarget::Voltage(PumpVoltage::Off),
            CoolerMode::Manual => self.manual_pump = PumpTarget::Voltage(PumpVoltage::Off),
        }
        self.queue.enqueue(CoolerCommand::TurnOffPump)
    }

    pub fn turn_off_led(&mut self) -> Result<(), CoolerError> {
        self.desired_rgb = RgbTarget::Off;
        self.queue.enqueue(CoolerCommand::TurnOffLed)
    }

    /// Resets `desired_rgb` back to `RgbTarget::Unset` if it is currently `RgbTarget::Off` (`TA-05`).
    ///
    /// Used when restoring lid lighting after closing while the cooler LED state was `Unset`.
    pub fn clear_desired_rgb_if_off(&mut self) {
        if self.desired_rgb == RgbTarget::Off {
            self.desired_rgb = RgbTarget::Unset;
        }
    }

    /// Marks the cooler connected, clears stale queued items, and re-enqueues active
    /// fan/pump/RGB targets, including explicit `Off` states (`W1-04`, `M5.7`).
    pub fn on_connected(&mut self) {
        self.state_machine.on_connected();
        self.queue.bump_generation();
        self.enqueue_active_fan_and_pump();
        self.enqueue_desired_rgb();
    }

    pub fn on_disconnected(&mut self) {
        self.state_machine.on_disconnected();
        self.queue.bump_generation();
    }

    pub fn on_prepare_for_sleep(&mut self, suspending: bool) {
        self.suspending = suspending;
        if suspending {
            self.queue.bump_generation();
            self.state_machine.on_disconnected();
        }
    }

    /// Pops and transmits ready commands respecting the 80ms gap; leaves any failed packet
    /// at the head of the queue for retry on the same connection (`W1-05`).
    pub fn flush_ready(
        &mut self,
        transport: &dyn BleTransport,
        now: Instant,
    ) -> Result<usize, CoolerError> {
        if self.suspending || !self.state_machine.is_connected() || !transport.is_connected() {
            return Ok(0);
        }
        let mut sent = 0usize;
        while let Some(queued) = self.queue.peek_ready_at(now) {
            let pkt = queued.command.encode()?;
            match transport.write_packet(pkt) {
                Ok(()) => {
                    self.queue.commit_sent_at(now);
                    sent += 1;
                }
                Err(err) => {
                    self.queue.record_write_attempt_at(now);
                    return Err(err);
                }
            }
        }
        Ok(sent)
    }

    /// Exposed for async BlueZ transport flushing without duplicating queue/state logic.
    pub(crate) fn peek_ready_at(&mut self, now: Instant) -> Option<QueuedCoolerCommand> {
        if self.suspending || !self.state_machine.is_connected() {
            return None;
        }
        self.queue.peek_ready_at(now)
    }

    pub(crate) fn commit_sent_at(&mut self, now: Instant) {
        self.queue.commit_sent_at(now);
    }

    pub(crate) fn record_write_attempt_at(&mut self, now: Instant) {
        self.queue.record_write_attempt_at(now);
    }

    pub fn next_flush_delay_at(&self, now: Instant) -> Duration {
        self.queue.next_delay_at(now)
    }

    fn enqueue_fan_target(
        queue: &mut CoolerCommandQueue,
        target: FanTarget,
    ) -> Result<(), CoolerError> {
        match target {
            FanTarget::Unset => Ok(()),
            FanTarget::Off => queue.enqueue(CoolerCommand::TurnOffFan),
            FanTarget::Speed(spd) => queue.enqueue(CoolerCommand::FanSpeed(spd)),
        }
    }

    fn enqueue_pump_target(
        queue: &mut CoolerCommandQueue,
        target: PumpTarget,
    ) -> Result<(), CoolerError> {
        match target {
            PumpTarget::Unset => Ok(()),
            PumpTarget::Voltage(PumpVoltage::Off) => queue.enqueue(CoolerCommand::TurnOffPump),
            PumpTarget::Voltage(v) => queue.enqueue(CoolerCommand::PumpVoltage(v)),
        }
    }

    fn enqueue_active_fan_and_pump(&mut self) {
        let fan = self.desired_fan_target();
        let pump = self.desired_pump_target();
        let _ = Self::enqueue_fan_target(&mut self.queue, fan);
        let _ = Self::enqueue_pump_target(&mut self.queue, pump);
    }

    fn enqueue_desired_rgb(&mut self) {
        let _ = match self.desired_rgb {
            RgbTarget::Unset => Ok(()),
            RgbTarget::Off => self.queue.enqueue(CoolerCommand::TurnOffLed),
            RgbTarget::Color {
                red,
                green,
                blue,
                state,
            } => self.queue.enqueue(CoolerCommand::Rgb {
                red,
                green,
                blue,
                state,
            }),
        };
    }
}
