// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows lid switch notification (`GUID_LIDSWITCH_STATE_CHANGE`) abstraction (`M15.3`).
//!
//! Parses `WM_POWERBROADCAST` / `PBT_POWERSETTINGCHANGE` (`POWERBROADCAST_SETTING`)
//! binary payloads, tracks deduplicated lid open/closed transitions, and manages
//! `RegisterPowerSettingNotification` / `UnregisterPowerSettingNotification` handles
//! with automatic RAII unregistration on drop.

use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Binary-compatible Windows `GUID` structure (`repr(C)`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LidGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl LidGuid {
    /// Constructs a `LidGuid` from canonical Windows GUID fields.
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }

    /// Encodes the GUID in Windows little-endian wire layout.
    pub const fn to_le_bytes(self) -> [u8; 16] {
        let d1 = self.data1.to_le_bytes();
        let d2 = self.data2.to_le_bytes();
        let d3 = self.data3.to_le_bytes();
        [
            d1[0],
            d1[1],
            d1[2],
            d1[3],
            d2[0],
            d2[1],
            d3[0],
            d3[1],
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7],
        ]
    }
}

/// Windows lid switch power setting GUID (`{BA3E0F4D-B817-4094-A2D1-D56379E6A0F3}`).
pub const GUID_LIDSWITCH_STATE_CHANGE: LidGuid = LidGuid::new(
    0xBA3E_0F4D,
    0xB817,
    0x4094,
    [0xA2, 0xD1, 0xD5, 0x63, 0x79, 0xE6, 0xA0, 0xF3],
);

/// Little-endian 16-byte wire representation of [`GUID_LIDSWITCH_STATE_CHANGE`].
pub const GUID_LIDSWITCH_STATE_CHANGE_BYTES: [u8; 16] = GUID_LIDSWITCH_STATE_CHANGE.to_le_bytes();

/// Win32 `DEVICE_NOTIFY_WINDOW_HANDLE` flag for `RegisterPowerSettingNotification`.
pub const DEVICE_NOTIFY_WINDOW_HANDLE: u32 = 0x0000_0000;

/// Win32 `DEVICE_NOTIFY_SERVICE_HANDLE` flag for `RegisterPowerSettingNotification`.
pub const DEVICE_NOTIFY_SERVICE_HANDLE: u32 = 0x0000_0001;

/// Physical laptop lid switch state reported by Windows `GUID_LIDSWITCH_STATE_CHANGE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum LidSwitchState {
    Closed = 0,
    Open = 1,
}

impl LidSwitchState {
    /// Converts a raw Windows lid switch `DWORD` (`0` = closed, `1` = open).
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Closed),
            1 => Some(Self::Open),
            _ => None,
        }
    }

    /// Returns the raw Windows `DWORD` value (`0` = closed, `1` = open).
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Decoded Win32 `POWERBROADCAST_SETTING` binary structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerBroadcastSetting {
    pub power_setting: [u8; 16],
    pub data_length: u32,
    pub data: Vec<u8>,
}

impl PowerBroadcastSetting {
    /// Parses a raw little-endian `POWERBROADCAST_SETTING` buffer (`16-byte GUID + u32 length + payload`).
    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < 20 {
            return None;
        }
        let mut power_setting = [0u8; 16];
        power_setting.copy_from_slice(&raw[0..16]);
        let data_length = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]);
        let end = 20usize.checked_add(data_length as usize)?;
        if raw.len() < end {
            return None;
        }
        Some(Self {
            power_setting,
            data_length,
            data: raw[20..end].to_vec(),
        })
    }

    /// Interprets this power broadcast setting as a [`LidSwitchState`] if its GUID matches.
    pub fn as_lid_state(&self) -> Option<LidSwitchState> {
        parse_lid_power_broadcast(&self.power_setting, &self.data)
    }
}

/// Parses a `GUID_LIDSWITCH_STATE_CHANGE` power broadcast payload into [`LidSwitchState`].
///
/// Accepts the standard 4-byte little-endian Windows `DWORD` payload (and single-byte
/// `0`/`1` payloads), returning `None` for any non-lid GUID, malformed length, or
/// unrecognized state code.
pub fn parse_lid_power_broadcast(guid_bytes: &[u8; 16], data: &[u8]) -> Option<LidSwitchState> {
    if guid_bytes != &GUID_LIDSWITCH_STATE_CHANGE_BYTES {
        return None;
    }
    match data.len() {
        4 => {
            let value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            LidSwitchState::from_raw(value)
        }
        1 => LidSwitchState::from_raw(u32::from(data[0])),
        _ => None,
    }
}

/// Errors returned by Windows lid switch notification registration and lifecycle operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WindowsLidError {
    #[error("Windows lid switch notification transport is unavailable")]
    Unavailable,
    #[error("Lid switch notification is already registered")]
    AlreadyRegistered,
    #[error("Lid switch notification is not currently registered")]
    NotRegistered,
    #[error("Invalid lid notification argument: {0}")]
    InvalidArgument(&'static str),
    #[error("Win32 {api} failed with error code {code:#010x}")]
    Win32Error { api: &'static str, code: u32 },
}

/// Opaque power setting notification handle returned by `RegisterPowerSettingNotification`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LidRegistrationHandle(pub usize);

/// Low-level transport seam over `user32.dll` power setting notifications.
pub trait WindowsLidTransport: Send + Sync {
    /// Returns `true` when notification registration is available.
    fn is_available(&self) -> bool;

    /// Registers for `GUID_LIDSWITCH_STATE_CHANGE` notifications on `recipient_handle`.
    fn register_lid_notification(
        &self,
        recipient_handle: usize,
        flags: u32,
    ) -> Result<LidRegistrationHandle, WindowsLidError>;

    /// Unregisters an active power setting notification handle.
    fn unregister_lid_notification(
        &self,
        handle: LidRegistrationHandle,
    ) -> Result<(), WindowsLidError>;
}

/// Deterministic in-memory mock [`WindowsLidTransport`] tracking registration handles.
#[derive(Debug)]
pub struct MockWindowsLidTransport {
    available: Mutex<bool>,
    fail_register: Mutex<bool>,
    next_handle: Mutex<usize>,
    active_handles: Mutex<Vec<LidRegistrationHandle>>,
    unregistered_handles: Mutex<Vec<LidRegistrationHandle>>,
}

impl Default for MockWindowsLidTransport {
    fn default() -> Self {
        Self::new(true)
    }
}

impl MockWindowsLidTransport {
    /// Creates a mock lid transport with initial availability state.
    pub fn new(available: bool) -> Self {
        Self {
            available: Mutex::new(available),
            fail_register: Mutex::new(false),
            next_handle: Mutex::new(1),
            active_handles: Mutex::new(Vec::new()),
            unregistered_handles: Mutex::new(Vec::new()),
        }
    }

    /// Toggles transport availability.
    pub fn set_available(&self, available: bool) {
        *self.available.lock().unwrap_or_else(|e| e.into_inner()) = available;
    }

    /// Forces registration attempts to fail.
    pub fn set_fail_register(&self, fail: bool) {
        *self.fail_register.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    /// Returns all currently active (registered and not yet unregistered) handles.
    pub fn active_handles(&self) -> Vec<LidRegistrationHandle> {
        self.active_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Returns all handles that have been unregistered.
    pub fn unregistered_handles(&self) -> Vec<LidRegistrationHandle> {
        self.unregistered_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl WindowsLidTransport for MockWindowsLidTransport {
    fn is_available(&self) -> bool {
        *self.available.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn register_lid_notification(
        &self,
        recipient_handle: usize,
        _flags: u32,
    ) -> Result<LidRegistrationHandle, WindowsLidError> {
        if !self.is_available() || *self.fail_register.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WindowsLidError::Unavailable);
        }
        if recipient_handle == 0 {
            return Err(WindowsLidError::InvalidArgument(
                "Recipient handle must be non-zero",
            ));
        }
        let handle = {
            let mut next = self.next_handle.lock().unwrap_or_else(|e| e.into_inner());
            let current = *next;
            *next = next.saturating_add(1);
            LidRegistrationHandle(current)
        };
        self.active_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
        Ok(handle)
    }

    fn unregister_lid_notification(
        &self,
        handle: LidRegistrationHandle,
    ) -> Result<(), WindowsLidError> {
        let mut active = self
            .active_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(pos) = active.iter().position(|h| *h == handle) else {
            return Err(WindowsLidError::NotRegistered);
        };
        active.remove(pos);
        self.unregistered_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
        Ok(())
    }
}

type LidStateCallback = Arc<dyn Fn(LidSwitchState) + Send + Sync>;

/// Monitors Windows lid switch power broadcasts with deduplicated state tracking and RAII cleanup.
pub struct WindowsLidMonitor {
    transport: Arc<dyn WindowsLidTransport>,
    handle: Mutex<Option<LidRegistrationHandle>>,
    current_state: Mutex<Option<LidSwitchState>>,
    transitions: Mutex<Vec<LidSwitchState>>,
    callbacks: Mutex<Vec<LidStateCallback>>,
}

impl WindowsLidMonitor {
    /// Creates an unregistered lid monitor backed by `transport`.
    pub fn new(transport: Arc<dyn WindowsLidTransport>) -> Self {
        Self {
            transport,
            handle: Mutex::new(None),
            current_state: Mutex::new(None),
            transitions: Mutex::new(Vec::new()),
            callbacks: Mutex::new(Vec::new()),
        }
    }

    /// Registers for Windows lid switch notifications on `recipient_handle`.
    pub fn register(
        &self,
        recipient_handle: usize,
        flags: u32,
    ) -> Result<LidRegistrationHandle, WindowsLidError> {
        let mut guard = self.handle.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Err(WindowsLidError::AlreadyRegistered);
        }
        let handle = self
            .transport
            .register_lid_notification(recipient_handle, flags)?;
        *guard = Some(handle);
        Ok(handle)
    }

    /// Unregisters the active lid switch notification handle if registered.
    pub fn unregister(&self) -> Result<(), WindowsLidError> {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or(WindowsLidError::NotRegistered)?;
        self.transport.unregister_lid_notification(handle)
    }

    /// Returns `true` while a notification handle is actively registered.
    pub fn is_registered(&self) -> bool {
        self.handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Returns the active registration handle if registered.
    pub fn handle(&self) -> Option<LidRegistrationHandle> {
        *self.handle.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Attaches a callback invoked whenever the lid state changes (deduplicated).
    pub fn on_state_change<F>(&self, callback: F)
    where
        F: Fn(LidSwitchState) + Send + Sync + 'static,
    {
        self.callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::new(callback));
    }

    /// Processes a `(guid_bytes, data)` pair from `PBT_POWERSETTINGCHANGE`.
    ///
    /// Returns `Some(new_state)` only when the payload is a valid lid switch state
    /// and differs from the last observed state; duplicate states are suppressed.
    pub fn handle_power_broadcast(
        &self,
        guid_bytes: &[u8; 16],
        data: &[u8],
    ) -> Option<LidSwitchState> {
        let next_state = parse_lid_power_broadcast(guid_bytes, data)?;
        {
            let mut cur = self.current_state.lock().unwrap_or_else(|e| e.into_inner());
            if *cur == Some(next_state) {
                return None;
            }
            *cur = Some(next_state);
        }
        self.transitions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(next_state);

        let callbacks: Vec<LidStateCallback> = self
            .callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for cb in callbacks {
            cb(next_state);
        }
        Some(next_state)
    }

    /// Parses and processes a raw `POWERBROADCAST_SETTING` binary buffer.
    pub fn handle_raw_broadcast(&self, raw: &[u8]) -> Option<LidSwitchState> {
        let setting = PowerBroadcastSetting::parse(raw)?;
        self.handle_power_broadcast(&setting.power_setting, &setting.data)
    }

    /// Returns the most recently observed lid switch state, if any.
    pub fn current_state(&self) -> Option<LidSwitchState> {
        *self.current_state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns the deduplicated history of observed lid switch transitions.
    pub fn transitions(&self) -> Vec<LidSwitchState> {
        self.transitions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for WindowsLidMonitor {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = self.transport.unregister_lid_notification(handle);
        }
    }
}

#[cfg(windows)]
mod win32 {
    use super::{
        LidGuid, LidRegistrationHandle, WindowsLidError, WindowsLidTransport,
        GUID_LIDSWITCH_STATE_CHANGE,
    };
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn RegisterPowerSettingNotification(
            h_recipient: *mut c_void,
            power_setting_guid: *const LidGuid,
            flags: u32,
        ) -> *mut c_void;
        fn UnregisterPowerSettingNotification(handle: *mut c_void) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    /// Native Windows `user32.dll` [`WindowsLidTransport`] implementation.
    ///
    /// Pair with [`super::WindowsLidMonitor`] or [`Win32LidNotificationGuard`] for
    /// automatic RAII unregistration on drop.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct Win32LidTransport;

    impl Win32LidTransport {
        pub fn new() -> Self {
            Self
        }
    }

    impl WindowsLidTransport for Win32LidTransport {
        fn is_available(&self) -> bool {
            true
        }

        fn register_lid_notification(
            &self,
            recipient_handle: usize,
            flags: u32,
        ) -> Result<LidRegistrationHandle, WindowsLidError> {
            if recipient_handle == 0 {
                return Err(WindowsLidError::InvalidArgument(
                    "Recipient handle must be non-zero",
                ));
            }
            let handle = unsafe {
                RegisterPowerSettingNotification(
                    recipient_handle as *mut c_void,
                    &GUID_LIDSWITCH_STATE_CHANGE,
                    flags,
                )
            };
            if handle.is_null() {
                let code = unsafe { GetLastError() };
                return Err(WindowsLidError::Win32Error {
                    api: "RegisterPowerSettingNotification",
                    code,
                });
            }
            Ok(LidRegistrationHandle(handle as usize))
        }

        fn unregister_lid_notification(
            &self,
            handle: LidRegistrationHandle,
        ) -> Result<(), WindowsLidError> {
            if handle.0 == 0 {
                return Err(WindowsLidError::InvalidArgument(
                    "Notification handle must be non-zero",
                ));
            }
            let ok = unsafe { UnregisterPowerSettingNotification(handle.0 as *mut c_void) };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                return Err(WindowsLidError::Win32Error {
                    api: "UnregisterPowerSettingNotification",
                    code,
                });
            }
            Ok(())
        }
    }

    /// Standalone RAII guard around a Win32 `HPOWERNOTIFY` handle.
    pub struct Win32LidNotificationGuard {
        handle: LidRegistrationHandle,
    }

    impl Win32LidNotificationGuard {
        /// Registers a lid switch notification and wraps the handle in an RAII guard.
        pub fn register(recipient_handle: usize, flags: u32) -> Result<Self, WindowsLidError> {
            let handle = Win32LidTransport.register_lid_notification(recipient_handle, flags)?;
            Ok(Self { handle })
        }

        /// Returns the underlying registration handle.
        pub fn handle(&self) -> LidRegistrationHandle {
            self.handle
        }
    }

    impl Drop for Win32LidNotificationGuard {
        fn drop(&mut self) {
            if self.handle.0 != 0 {
                let _ = Win32LidTransport.unregister_lid_notification(self.handle);
                self.handle = LidRegistrationHandle(0);
            }
        }
    }
}

#[cfg(windows)]
pub use win32::{Win32LidNotificationGuard, Win32LidTransport};
