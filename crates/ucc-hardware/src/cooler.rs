//! Liquid Cooled Technology (LCT) BLE Water Cooler protocol and state machine.
//!
//! Controls external LCT water coolers (e.g. LCT21001, LCT22002) over Nordic UART Service (NUS).

use std::time::{Duration, Instant};
use thiserror::Error;

/// Nordic UART Service (NUS) UUIDs
pub const NORDIC_UART_SERVICE_UUID: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
pub const NORDIC_UART_CHAR_TX: &str = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";
pub const NORDIC_UART_CHAR_RX: &str = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";
pub const CLIENT_CHAR_CONFIG_DESCRIPTOR_UUID: &str = "00002902-0000-1000-8000-00805f9b34fb";

/// Timing constants matching C++ implementation
pub const BLE_WRITE_GAP: Duration = Duration::from_millis(80);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
pub const KEEPALIVE_MISS_THRESHOLD: u32 = 3;
pub const FAST_RECONNECT_MAX_FAILURES: u32 = 3;
pub const GATT_CACHE_PURGE_FAILURE_THRESHOLD: u32 = 3;

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

    /// Calculates required delay before next write can be sent
    pub fn required_delay(&self) -> Duration {
        match self.last_write {
            Some(last) => {
                let elapsed = last.elapsed();
                if elapsed < self.min_gap {
                    self.min_gap - elapsed
                } else {
                    Duration::ZERO
                }
            }
            None => Duration::ZERO,
        }
    }

    /// Record that a write was sent now
    pub fn record_write(&mut self) {
        self.last_write = Some(Instant::now());
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
