// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows Power Management API (`powrprof.dll`) CPU tuning abstraction (`M15.3`).
//!
//! Provides a portable trait boundary ([`WindowsPowerTransport`]) and
//! [`WindowsPowerController`] for managing Windows processor power settings
//! (Energy Performance Preference / EPP, minimum and maximum processor throttle
//! percentages, maximum frequency in MHz, and processor boost mode) across AC
//! and DC power sources.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Binary-compatible Windows `GUID` structure (`repr(C)`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PowerGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl PowerGuid {
    /// Constructs a `PowerGuid` from its canonical Windows GUID fields.
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

/// Processor power settings subgroup (`{54533251-82BE-4824-96C1-47B60B740D00}`).
pub const GUID_PROCESSOR_SETTINGS_SUBGROUP: PowerGuid = PowerGuid::new(
    0x5453_3251,
    0x82BE,
    0x4824,
    [0x96, 0xC1, 0x47, 0xB6, 0x0B, 0x74, 0x0D, 0x00],
);

/// Processor energy performance preference policy / EPP (`{36687F9E-E3A5-4DBF-B1DC-15EB381C6863}`).
pub const GUID_PROCESSOR_PERF_ENERGY_PREFERENCE: PowerGuid = PowerGuid::new(
    0x3668_7F9E,
    0xE3A5,
    0x4DBF,
    [0xB1, 0xDC, 0x15, 0xEB, 0x38, 0x1C, 0x68, 0x63],
);

/// Minimum processor state percentage (`{893DEE8E-2BEF-41E0-89C6-B55D0929964C}`).
pub const GUID_PROCESSOR_THROTTLE_MINIMUM: PowerGuid = PowerGuid::new(
    0x893D_EE8E,
    0x2BEF,
    0x41E0,
    [0x89, 0xC6, 0xB5, 0x5D, 0x09, 0x29, 0x96, 0x4C],
);

/// Maximum processor state percentage (`{BC5038F7-23E0-4960-96DA-33ABAF5935EC}`).
pub const GUID_PROCESSOR_THROTTLE_MAXIMUM: PowerGuid = PowerGuid::new(
    0xBC50_38F7,
    0x23E0,
    0x4960,
    [0x96, 0xDA, 0x33, 0xAB, 0xAF, 0x59, 0x35, 0xEC],
);

/// Maximum processor frequency in MHz; `0` means unlimited (`{75B0AE3F-BCE0-45A7-8C89-C9611C25E100}`).
pub const GUID_PROCESSOR_FREQUENCY_MAXIMUM: PowerGuid = PowerGuid::new(
    0x75B0_AE3F,
    0xBCE0,
    0x45A7,
    [0x8C, 0x89, 0xC9, 0x61, 0x1C, 0x25, 0xE1, 0x00],
);

/// Processor performance boost mode (`{BE337238-0D82-4146-A960-4F3749D470C7}`).
pub const GUID_PROCESSOR_PERF_BOOST_MODE: PowerGuid = PowerGuid::new(
    0xBE33_7238,
    0x0D82,
    0x4146,
    [0xA9, 0x60, 0x4F, 0x37, 0x49, 0xD4, 0x70, 0xC7],
);

/// Active Windows power source selector (AC adapter vs. DC battery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowsPowerSource {
    Ac,
    Dc,
}

/// Windows processor performance boost mode values (`GUID_PROCESSOR_PERF_BOOST_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum BoostMode {
    Disabled = 0,
    Enabled = 1,
    Aggressive = 2,
    EfficientEnabled = 3,
    EfficientAggressive = 4,
}

impl BoostMode {
    /// Converts a raw Windows `DWORD` boost mode value into [`BoostMode`].
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Disabled),
            1 => Some(Self::Enabled),
            2 => Some(Self::Aggressive),
            3 => Some(Self::EfficientEnabled),
            4 => Some(Self::EfficientAggressive),
            _ => None,
        }
    }

    /// Returns the raw Windows `DWORD` boost mode value.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Errors returned by Windows Power Management API operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WindowsPowerError {
    #[error("Windows Power Management API is unavailable")]
    Unavailable,
    #[error("Invalid power setting argument: {0}")]
    InvalidArgument(&'static str),
    #[error("Unsupported power setting value: {0}")]
    Unsupported(&'static str),
    #[error("Win32 {api} failed with error code {code:#010x}")]
    Win32Error { api: &'static str, code: u32 },
}

/// Maps a UCC EPP preference string to the standard Windows EPP percentage (`0..=100`).
pub fn epp_str_to_percent(epp: &str) -> Result<u32, WindowsPowerError> {
    match epp.trim().to_ascii_lowercase().as_str() {
        "performance" => Ok(0),
        "balance_performance" => Ok(33),
        "balance_power" => Ok(66),
        "power" => Ok(100),
        _ => Err(WindowsPowerError::InvalidArgument(
            "EPP string must be one of performance, balance_performance, balance_power, power",
        )),
    }
}

/// Maps a Windows EPP percentage (`0..=100`) to the nearest UCC EPP preference string.
pub fn epp_percent_to_str(percent: u32) -> Result<&'static str, WindowsPowerError> {
    match percent {
        0..=16 => Ok("performance"),
        17..=49 => Ok("balance_performance"),
        50..=83 => Ok("balance_power"),
        84..=100 => Ok("power"),
        _ => Err(WindowsPowerError::InvalidArgument(
            "EPP percentage must be within 0..=100",
        )),
    }
}

/// Recorded write operation for deterministic mock verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsPowerWrite {
    WriteValue {
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
        value: u32,
    },
    ApplyActiveScheme,
}

/// Low-level transport seam over Windows `powrprof.dll` power-scheme reads and writes.
pub trait WindowsPowerTransport: Send + Sync {
    /// Returns `true` when the active Windows power scheme is accessible.
    fn is_available(&self) -> bool;

    /// Reads a `DWORD` value index for `subgroup`/`setting` under `source` (AC or DC).
    fn read_value_index(
        &self,
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
    ) -> Result<u32, WindowsPowerError>;

    /// Writes a `DWORD` value index for `subgroup`/`setting` under `source` (AC or DC).
    fn write_value_index(
        &self,
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
        value: u32,
    ) -> Result<(), WindowsPowerError>;

    /// Re-applies the active Windows power scheme so written values take immediate effect.
    fn apply_active_scheme(&self) -> Result<(), WindowsPowerError>;
}

type PowerSettingKey = (WindowsPowerSource, PowerGuid, PowerGuid);

/// Deterministic in-memory mock [`WindowsPowerTransport`] with write journal.
#[derive(Debug)]
pub struct MockWindowsPowerTransport {
    available: Mutex<bool>,
    fail_writes: Mutex<bool>,
    values: Mutex<BTreeMap<PowerSettingKey, u32>>,
    journal: Mutex<Vec<WindowsPowerWrite>>,
}

impl Default for MockWindowsPowerTransport {
    fn default() -> Self {
        Self::new(true)
    }
}

impl MockWindowsPowerTransport {
    /// Creates a mock power transport seeded with standard Windows defaults for AC and DC.
    pub fn new(available: bool) -> Self {
        let mut values = BTreeMap::new();
        for source in [WindowsPowerSource::Ac, WindowsPowerSource::Dc] {
            let epp_default = match source {
                WindowsPowerSource::Ac => 33,
                WindowsPowerSource::Dc => 66,
            };
            values.insert(
                (
                    source,
                    GUID_PROCESSOR_SETTINGS_SUBGROUP,
                    GUID_PROCESSOR_PERF_ENERGY_PREFERENCE,
                ),
                epp_default,
            );
            values.insert(
                (
                    source,
                    GUID_PROCESSOR_SETTINGS_SUBGROUP,
                    GUID_PROCESSOR_THROTTLE_MINIMUM,
                ),
                5,
            );
            values.insert(
                (
                    source,
                    GUID_PROCESSOR_SETTINGS_SUBGROUP,
                    GUID_PROCESSOR_THROTTLE_MAXIMUM,
                ),
                100,
            );
            values.insert(
                (
                    source,
                    GUID_PROCESSOR_SETTINGS_SUBGROUP,
                    GUID_PROCESSOR_FREQUENCY_MAXIMUM,
                ),
                0,
            );
            values.insert(
                (
                    source,
                    GUID_PROCESSOR_SETTINGS_SUBGROUP,
                    GUID_PROCESSOR_PERF_BOOST_MODE,
                ),
                BoostMode::Aggressive.as_raw(),
            );
        }

        Self {
            available: Mutex::new(available),
            fail_writes: Mutex::new(false),
            values: Mutex::new(values),
            journal: Mutex::new(Vec::new()),
        }
    }

    /// Toggles transport availability for error-path testing.
    pub fn set_available(&self, available: bool) {
        *self.available.lock().unwrap_or_else(|e| e.into_inner()) = available;
    }

    /// Forces subsequent writes and scheme applies to fail with [`WindowsPowerError::Unavailable`].
    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    /// Directly seeds a setting value without recording a journal entry.
    pub fn seed_value(
        &self,
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
        value: u32,
    ) {
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((source, subgroup, setting), value);
    }

    /// Inspects the stored value for a setting without going through the controller.
    pub fn get_value(
        &self,
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
    ) -> Option<u32> {
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(source, subgroup, setting))
            .copied()
    }

    /// Returns a snapshot of recorded write operations in exact execution order.
    pub fn journal(&self) -> Vec<WindowsPowerWrite> {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Clears the recorded write journal.
    pub fn clear_journal(&self) {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl WindowsPowerTransport for MockWindowsPowerTransport {
    fn is_available(&self) -> bool {
        *self.available.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read_value_index(
        &self,
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
    ) -> Result<u32, WindowsPowerError> {
        if !self.is_available() {
            return Err(WindowsPowerError::Unavailable);
        }
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(source, subgroup, setting))
            .copied()
            .ok_or(WindowsPowerError::Unsupported(
                "Requested power setting is not seeded in mock transport",
            ))
    }

    fn write_value_index(
        &self,
        source: WindowsPowerSource,
        subgroup: PowerGuid,
        setting: PowerGuid,
        value: u32,
    ) -> Result<(), WindowsPowerError> {
        if !self.is_available() || *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WindowsPowerError::Unavailable);
        }
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((source, subgroup, setting), value);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WindowsPowerWrite::WriteValue {
                source,
                subgroup,
                setting,
                value,
            });
        Ok(())
    }

    fn apply_active_scheme(&self) -> Result<(), WindowsPowerError> {
        if !self.is_available() || *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WindowsPowerError::Unavailable);
        }
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WindowsPowerWrite::ApplyActiveScheme);
        Ok(())
    }
}

/// High-level controller for Windows processor power management settings.
pub struct WindowsPowerController {
    transport: Arc<dyn WindowsPowerTransport>,
}

impl WindowsPowerController {
    /// Creates a controller backed by `transport`.
    pub fn new(transport: Arc<dyn WindowsPowerTransport>) -> Self {
        Self { transport }
    }

    /// Returns `true` if the underlying power transport is currently available.
    pub fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    /// Reads the raw Windows EPP percentage (`0..=100`) for `source`.
    pub fn read_epp_percent(&self, source: WindowsPowerSource) -> Result<u32, WindowsPowerError> {
        let raw = self.transport.read_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_PERF_ENERGY_PREFERENCE,
        )?;
        if raw > 100 {
            return Err(WindowsPowerError::Unsupported(
                "Windows returned EPP percentage outside 0..=100",
            ));
        }
        Ok(raw)
    }

    /// Reads the active EPP setting as a canonical UCC EPP string.
    pub fn read_epp_str(
        &self,
        source: WindowsPowerSource,
    ) -> Result<&'static str, WindowsPowerError> {
        let percent = self.read_epp_percent(source)?;
        epp_percent_to_str(percent)
    }

    /// Writes a validated Windows EPP percentage (`0..=100`) and re-applies the active scheme.
    pub fn set_epp_percent(
        &self,
        source: WindowsPowerSource,
        percent: u32,
    ) -> Result<(), WindowsPowerError> {
        if percent > 100 {
            return Err(WindowsPowerError::InvalidArgument(
                "EPP percentage must be within 0..=100",
            ));
        }
        self.transport.write_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_PERF_ENERGY_PREFERENCE,
            percent,
        )?;
        self.transport.apply_active_scheme()
    }

    /// Writes a UCC EPP preference string (`performance`, `balance_performance`, `balance_power`, `power`).
    pub fn set_epp_str(
        &self,
        source: WindowsPowerSource,
        epp: &str,
    ) -> Result<(), WindowsPowerError> {
        let percent = epp_str_to_percent(epp)?;
        self.set_epp_percent(source, percent)
    }

    /// Reads `(min_percent, max_percent)` processor throttle bounds for `source`.
    pub fn read_throttle_bounds(
        &self,
        source: WindowsPowerSource,
    ) -> Result<(u32, u32), WindowsPowerError> {
        let min = self.transport.read_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_THROTTLE_MINIMUM,
        )?;
        let max = self.transport.read_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_THROTTLE_MAXIMUM,
        )?;
        if min > 100 || max > 100 {
            return Err(WindowsPowerError::Unsupported(
                "Windows returned throttle percentage outside 0..=100",
            ));
        }
        Ok((min, max))
    }

    /// Sets CPU minimum and maximum throttle percentages (`0..=100`, `min <= max`).
    ///
    /// Orders writes safely so the active minimum throttle never transiently exceeds
    /// the active maximum throttle: raises maximum first when `min_percent > current_max`,
    /// otherwise writes minimum first.
    pub fn set_throttle_bounds(
        &self,
        source: WindowsPowerSource,
        min_percent: u32,
        max_percent: u32,
    ) -> Result<(), WindowsPowerError> {
        if min_percent > 100 || max_percent > 100 {
            return Err(WindowsPowerError::InvalidArgument(
                "Throttle percentages must be within 0..=100",
            ));
        }
        if min_percent > max_percent {
            return Err(WindowsPowerError::InvalidArgument(
                "Minimum throttle percentage must not exceed maximum throttle percentage",
            ));
        }

        let current_max = self
            .transport
            .read_value_index(
                source,
                GUID_PROCESSOR_SETTINGS_SUBGROUP,
                GUID_PROCESSOR_THROTTLE_MAXIMUM,
            )
            .unwrap_or(100);

        if min_percent > current_max {
            self.transport.write_value_index(
                source,
                GUID_PROCESSOR_SETTINGS_SUBGROUP,
                GUID_PROCESSOR_THROTTLE_MAXIMUM,
                max_percent,
            )?;
            self.transport.write_value_index(
                source,
                GUID_PROCESSOR_SETTINGS_SUBGROUP,
                GUID_PROCESSOR_THROTTLE_MINIMUM,
                min_percent,
            )?;
        } else {
            self.transport.write_value_index(
                source,
                GUID_PROCESSOR_SETTINGS_SUBGROUP,
                GUID_PROCESSOR_THROTTLE_MINIMUM,
                min_percent,
            )?;
            self.transport.write_value_index(
                source,
                GUID_PROCESSOR_SETTINGS_SUBGROUP,
                GUID_PROCESSOR_THROTTLE_MAXIMUM,
                max_percent,
            )?;
        }
        self.transport.apply_active_scheme()
    }

    /// Reads the configured maximum processor frequency in MHz (`0` = unlimited).
    pub fn read_max_frequency_mhz(
        &self,
        source: WindowsPowerSource,
    ) -> Result<u32, WindowsPowerError> {
        self.transport.read_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_FREQUENCY_MAXIMUM,
        )
    }

    /// Sets the maximum processor frequency in MHz (`0` = unlimited) and re-applies the active scheme.
    pub fn set_max_frequency_mhz(
        &self,
        source: WindowsPowerSource,
        max_mhz: u32,
    ) -> Result<(), WindowsPowerError> {
        self.transport.write_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_FREQUENCY_MAXIMUM,
            max_mhz,
        )?;
        self.transport.apply_active_scheme()
    }

    /// Reads the active processor performance boost mode for `source`.
    pub fn read_boost_mode(
        &self,
        source: WindowsPowerSource,
    ) -> Result<BoostMode, WindowsPowerError> {
        let raw = self.transport.read_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_PERF_BOOST_MODE,
        )?;
        BoostMode::from_raw(raw).ok_or(WindowsPowerError::Unsupported(
            "Windows returned unrecognized boost mode value",
        ))
    }

    /// Writes the processor performance boost mode and re-applies the active scheme.
    pub fn set_boost_mode(
        &self,
        source: WindowsPowerSource,
        mode: BoostMode,
    ) -> Result<(), WindowsPowerError> {
        self.transport.write_value_index(
            source,
            GUID_PROCESSOR_SETTINGS_SUBGROUP,
            GUID_PROCESSOR_PERF_BOOST_MODE,
            mode.as_raw(),
        )?;
        self.transport.apply_active_scheme()
    }

    /// Convenience toggle for processor turbo/boost (`true` = `Aggressive`, `false` = `Disabled`).
    pub fn set_turbo_enabled(
        &self,
        source: WindowsPowerSource,
        enabled: bool,
    ) -> Result<(), WindowsPowerError> {
        let mode = if enabled {
            BoostMode::Aggressive
        } else {
            BoostMode::Disabled
        };
        self.set_boost_mode(source, mode)
    }
}

#[cfg(windows)]
mod win32 {
    use super::{PowerGuid, WindowsPowerError, WindowsPowerSource, WindowsPowerTransport};
    use std::ffi::c_void;
    use std::ptr;

    const ERROR_SUCCESS: u32 = 0;

    #[link(name = "powrprof")]
    extern "system" {
        fn PowerGetActiveScheme(
            user_root_power_key: *const c_void,
            active_policy_guid: *mut *mut PowerGuid,
        ) -> u32;
        fn PowerReadACValueIndex(
            root_power_key: *const c_void,
            scheme_guid: *const PowerGuid,
            sub_group_of_power_settings_guid: *const PowerGuid,
            power_setting_guid: *const PowerGuid,
            ac_value_index: *mut u32,
        ) -> u32;
        fn PowerReadDCValueIndex(
            root_power_key: *const c_void,
            scheme_guid: *const PowerGuid,
            sub_group_of_power_settings_guid: *const PowerGuid,
            power_setting_guid: *const PowerGuid,
            dc_value_index: *mut u32,
        ) -> u32;
        fn PowerWriteACValueIndex(
            root_power_key: *const c_void,
            scheme_guid: *const PowerGuid,
            sub_group_of_power_settings_guid: *const PowerGuid,
            power_setting_guid: *const PowerGuid,
            ac_value_index: u32,
        ) -> u32;
        fn PowerWriteDCValueIndex(
            root_power_key: *const c_void,
            scheme_guid: *const PowerGuid,
            sub_group_of_power_settings_guid: *const PowerGuid,
            power_setting_guid: *const PowerGuid,
            dc_value_index: u32,
        ) -> u32;
        fn PowerSetActiveScheme(
            user_root_power_key: *const c_void,
            scheme_guid: *const PowerGuid,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(hmem: *mut c_void) -> *mut c_void;
    }

    struct ActiveSchemeGuard {
        ptr: *mut PowerGuid,
    }

    impl ActiveSchemeGuard {
        fn query() -> Result<Self, WindowsPowerError> {
            let mut scheme_ptr: *mut PowerGuid = ptr::null_mut();
            let code = unsafe { PowerGetActiveScheme(ptr::null(), &mut scheme_ptr) };
            if code != ERROR_SUCCESS || scheme_ptr.is_null() {
                return Err(WindowsPowerError::Win32Error {
                    api: "PowerGetActiveScheme",
                    code,
                });
            }
            Ok(Self { ptr: scheme_ptr })
        }

        fn as_ptr(&self) -> *const PowerGuid {
            self.ptr as *const PowerGuid
        }
    }

    impl Drop for ActiveSchemeGuard {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                unsafe {
                    let _ = LocalFree(self.ptr.cast::<c_void>());
                }
                self.ptr = ptr::null_mut();
            }
        }
    }

    /// Native Windows `powrprof.dll` [`WindowsPowerTransport`] implementation.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct Win32PowerTransport;

    impl Win32PowerTransport {
        pub fn new() -> Self {
            Self
        }
    }

    impl WindowsPowerTransport for Win32PowerTransport {
        fn is_available(&self) -> bool {
            ActiveSchemeGuard::query().is_ok()
        }

        fn read_value_index(
            &self,
            source: WindowsPowerSource,
            subgroup: PowerGuid,
            setting: PowerGuid,
        ) -> Result<u32, WindowsPowerError> {
            let scheme = ActiveSchemeGuard::query()?;
            let mut value: u32 = 0;
            let (api, code) = match source {
                WindowsPowerSource::Ac => (
                    "PowerReadACValueIndex",
                    unsafe {
                        PowerReadACValueIndex(
                            ptr::null(),
                            scheme.as_ptr(),
                            &subgroup,
                            &setting,
                            &mut value,
                        )
                    },
                ),
                WindowsPowerSource::Dc => (
                    "PowerReadDCValueIndex",
                    unsafe {
                        PowerReadDCValueIndex(
                            ptr::null(),
                            scheme.as_ptr(),
                            &subgroup,
                            &setting,
                            &mut value,
                        )
                    },
                ),
            };
            if code != ERROR_SUCCESS {
                return Err(WindowsPowerError::Win32Error { api, code });
            }
            Ok(value)
        }

        fn write_value_index(
            &self,
            source: WindowsPowerSource,
            subgroup: PowerGuid,
            setting: PowerGuid,
            value: u32,
        ) -> Result<(), WindowsPowerError> {
            let scheme = ActiveSchemeGuard::query()?;
            let (api, code) = match source {
                WindowsPowerSource::Ac => (
                    "PowerWriteACValueIndex",
                    unsafe {
                        PowerWriteACValueIndex(
                            ptr::null(),
                            scheme.as_ptr(),
                            &subgroup,
                            &setting,
                            value,
                        )
                    },
                ),
                WindowsPowerSource::Dc => (
                    "PowerWriteDCValueIndex",
                    unsafe {
                        PowerWriteDCValueIndex(
                            ptr::null(),
                            scheme.as_ptr(),
                            &subgroup,
                            &setting,
                            value,
                        )
                    },
                ),
            };
            if code != ERROR_SUCCESS {
                return Err(WindowsPowerError::Win32Error { api, code });
            }
            Ok(())
        }

        fn apply_active_scheme(&self) -> Result<(), WindowsPowerError> {
            let scheme = ActiveSchemeGuard::query()?;
            let code = unsafe { PowerSetActiveScheme(ptr::null(), scheme.as_ptr()) };
            if code != ERROR_SUCCESS {
                return Err(WindowsPowerError::Win32Error {
                    api: "PowerSetActiveScheme",
                    code,
                });
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use win32::Win32PowerTransport;
