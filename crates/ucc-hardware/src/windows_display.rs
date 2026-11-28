// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows display refresh-rate and monitor brightness abstraction (`M15.3`).
//!
//! Provides a portable trait boundary ([`WindowsDisplayTransport`]) and
//! [`WindowsDisplayController`] for enumerating display modes, switching refresh
//! rates safely via `DM_DISPLAYFREQUENCY` only (preserving active resolution and
//! color depth), and reading/writing monitor brightness (`0..=100`).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Display mode specification reported by Windows `EnumDisplaySettingsExW`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayModeSpec {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub bits_per_pel: u32,
}

/// Errors returned by Windows display and brightness operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WindowsDisplayError {
    #[error("Windows display interface is unavailable")]
    Unavailable,
    #[error("Invalid display argument: {0}")]
    InvalidArgument(&'static str),
    #[error(
        "Refresh rate {requested_hz} Hz is unsupported for {width}x{height}@{bits_per_pel}bpp"
    )]
    UnsupportedRefreshRate {
        requested_hz: u32,
        width: u32,
        height: u32,
        bits_per_pel: u32,
    },
    #[error("Unsupported display operation: {0}")]
    Unsupported(&'static str),
    #[error("Win32 {api} failed with code {code}")]
    Win32Error { api: &'static str, code: i32 },
}

/// Recorded display mutation for deterministic mock verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsDisplayWrite {
    SetRefreshRate {
        device_name: Option<String>,
        refresh_hz: u32,
    },
    SetBrightness {
        percent: u8,
    },
}

/// Low-level transport seam over Windows display mode (`user32.dll`) and brightness APIs.
pub trait WindowsDisplayTransport: Send + Sync {
    /// Returns `true` when the display transport is accessible.
    fn is_available(&self) -> bool;

    /// Queries the active display mode for `device_name` (`None` = primary display).
    fn current_mode(
        &self,
        device_name: Option<&str>,
    ) -> Result<DisplayModeSpec, WindowsDisplayError>;

    /// Enumerates all supported display modes for `device_name` (`None` = primary display).
    fn enumerate_modes(
        &self,
        device_name: Option<&str>,
    ) -> Result<Vec<DisplayModeSpec>, WindowsDisplayError>;

    /// Applies `refresh_hz` using `DM_DISPLAYFREQUENCY` only, preserving resolution and color depth.
    fn apply_refresh_rate(
        &self,
        device_name: Option<&str>,
        refresh_hz: u32,
    ) -> Result<(), WindowsDisplayError>;

    /// Reads current monitor brightness percentage (`0..=100`).
    fn read_brightness(&self) -> Result<u8, WindowsDisplayError>;

    /// Writes monitor brightness percentage (`0..=100`, matching `WmiMonitorBrightnessMethods::WmiSetBrightness`).
    fn write_brightness(&self, percent: u8) -> Result<(), WindowsDisplayError>;
}

/// Deterministic in-memory mock [`WindowsDisplayTransport`] with write journal.
#[derive(Debug)]
pub struct MockWindowsDisplayTransport {
    available: Mutex<bool>,
    fail_writes: Mutex<bool>,
    current: Mutex<DisplayModeSpec>,
    modes: Mutex<Vec<DisplayModeSpec>>,
    brightness: Mutex<u8>,
    journal: Mutex<Vec<WindowsDisplayWrite>>,
}

impl MockWindowsDisplayTransport {
    /// Creates an available mock display transport with an initial mode and supported mode list.
    pub fn new(current: DisplayModeSpec, modes: Vec<DisplayModeSpec>) -> Self {
        Self {
            available: Mutex::new(true),
            fail_writes: Mutex::new(false),
            current: Mutex::new(current),
            modes: Mutex::new(modes),
            brightness: Mutex::new(80),
            journal: Mutex::new(Vec::new()),
        }
    }

    /// Toggles mock availability.
    pub fn set_available(&self, available: bool) {
        *self.available.lock().unwrap_or_else(|e| e.into_inner()) = available;
    }

    /// Forces subsequent write operations to fail with [`WindowsDisplayError::Unavailable`].
    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    /// Replaces the current active display mode in mock state.
    pub fn set_current_mode(&self, mode: DisplayModeSpec) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }

    /// Replaces the enumerated mode list in mock state.
    pub fn set_modes(&self, modes: Vec<DisplayModeSpec>) {
        *self.modes.lock().unwrap_or_else(|e| e.into_inner()) = modes;
    }

    /// Seeds the brightness value in mock state without recording a journal entry.
    pub fn seed_brightness(&self, percent: u8) {
        *self.brightness.lock().unwrap_or_else(|e| e.into_inner()) = percent;
    }

    /// Returns a snapshot of recorded display writes in execution order.
    pub fn journal(&self) -> Vec<WindowsDisplayWrite> {
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

impl WindowsDisplayTransport for MockWindowsDisplayTransport {
    fn is_available(&self) -> bool {
        *self.available.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_mode(
        &self,
        _device_name: Option<&str>,
    ) -> Result<DisplayModeSpec, WindowsDisplayError> {
        if !self.is_available() {
            return Err(WindowsDisplayError::Unavailable);
        }
        Ok(*self.current.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn enumerate_modes(
        &self,
        _device_name: Option<&str>,
    ) -> Result<Vec<DisplayModeSpec>, WindowsDisplayError> {
        if !self.is_available() {
            return Err(WindowsDisplayError::Unavailable);
        }
        Ok(self.modes.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn apply_refresh_rate(
        &self,
        device_name: Option<&str>,
        refresh_hz: u32,
    ) -> Result<(), WindowsDisplayError> {
        if !self.is_available() || *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WindowsDisplayError::Unavailable);
        }
        {
            let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
            cur.refresh_hz = refresh_hz;
        }
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WindowsDisplayWrite::SetRefreshRate {
                device_name: device_name.map(str::to_owned),
                refresh_hz,
            });
        Ok(())
    }

    fn read_brightness(&self) -> Result<u8, WindowsDisplayError> {
        if !self.is_available() {
            return Err(WindowsDisplayError::Unavailable);
        }
        Ok(*self.brightness.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn write_brightness(&self, percent: u8) -> Result<(), WindowsDisplayError> {
        if !self.is_available() || *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WindowsDisplayError::Unavailable);
        }
        *self.brightness.lock().unwrap_or_else(|e| e.into_inner()) = percent;
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WindowsDisplayWrite::SetBrightness { percent });
        Ok(())
    }
}

/// High-level controller for Windows display refresh-rate switching and brightness control.
pub struct WindowsDisplayController {
    transport: Arc<dyn WindowsDisplayTransport>,
}

impl WindowsDisplayController {
    /// Creates a controller backed by `transport`.
    pub fn new(transport: Arc<dyn WindowsDisplayTransport>) -> Self {
        Self { transport }
    }

    /// Returns `true` when the underlying display transport is available.
    pub fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    /// Returns the active display mode for `device_name` (`None` = primary display).
    pub fn current_mode(
        &self,
        device_name: Option<&str>,
    ) -> Result<DisplayModeSpec, WindowsDisplayError> {
        self.transport.current_mode(device_name)
    }

    /// Enumerates all display modes reported for `device_name`.
    pub fn enumerate_modes(
        &self,
        device_name: Option<&str>,
    ) -> Result<Vec<DisplayModeSpec>, WindowsDisplayError> {
        self.transport.enumerate_modes(device_name)
    }

    /// Returns sorted, deduplicated refresh rates supported at the active resolution and color depth.
    pub fn supported_refresh_rates(
        &self,
        device_name: Option<&str>,
    ) -> Result<Vec<u32>, WindowsDisplayError> {
        let current = self.transport.current_mode(device_name)?;
        let modes = self.transport.enumerate_modes(device_name)?;
        let mut rates = BTreeSet::new();
        for mode in modes {
            if mode.width == current.width
                && mode.height == current.height
                && mode.bits_per_pel == current.bits_per_pel
                && mode.refresh_hz > 0
            {
                rates.insert(mode.refresh_hz);
            }
        }
        Ok(rates.into_iter().collect())
    }

    /// Switches the display refresh rate to `target_hz` while preserving current resolution and color depth.
    ///
    /// Rejects unsupported refresh rates before touching the display driver.
    pub fn set_refresh_rate(
        &self,
        device_name: Option<&str>,
        target_hz: u32,
    ) -> Result<DisplayModeSpec, WindowsDisplayError> {
        if target_hz == 0 {
            return Err(WindowsDisplayError::InvalidArgument(
                "Refresh rate must be greater than 0 Hz",
            ));
        }

        let current = self.transport.current_mode(device_name)?;
        let modes = self.transport.enumerate_modes(device_name)?;
        let supported = modes.iter().any(|mode| {
            mode.width == current.width
                && mode.height == current.height
                && mode.bits_per_pel == current.bits_per_pel
                && mode.refresh_hz == target_hz
        });

        if !supported {
            return Err(WindowsDisplayError::UnsupportedRefreshRate {
                requested_hz: target_hz,
                width: current.width,
                height: current.height,
                bits_per_pel: current.bits_per_pel,
            });
        }

        self.transport.apply_refresh_rate(device_name, target_hz)?;
        Ok(DisplayModeSpec {
            width: current.width,
            height: current.height,
            refresh_hz: target_hz,
            bits_per_pel: current.bits_per_pel,
        })
    }

    /// Reads current monitor brightness (`0..=100`, clamped to `100` if firmware reports higher).
    pub fn read_brightness(&self) -> Result<u8, WindowsDisplayError> {
        Ok(self.transport.read_brightness()?.min(100))
    }

    /// Sets monitor brightness (`0..=100`), rejecting values greater than `100`.
    pub fn set_brightness(&self, percent: u8) -> Result<(), WindowsDisplayError> {
        if percent > 100 {
            return Err(WindowsDisplayError::InvalidArgument(
                "Brightness percentage must be within 0..=100",
            ));
        }
        self.transport.write_brightness(percent)
    }

    /// Sets monitor brightness after clamping `percent` into `0..=100`, returning the applied value.
    pub fn set_brightness_clamped(&self, percent: u8) -> Result<u8, WindowsDisplayError> {
        let clamped = percent.min(100);
        self.transport.write_brightness(clamped)?;
        Ok(clamped)
    }
}

#[cfg(windows)]
mod win32 {
    use super::{DisplayModeSpec, WindowsDisplayError, WindowsDisplayTransport};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr;

    const ENUM_CURRENT_SETTINGS: u32 = u32::MAX;
    const DM_DISPLAYFREQUENCY: u32 = 0x0040_0000;
    const CDS_UPDATEREGISTRY: u32 = 0x0000_0001;
    const CDS_TEST: u32 = 0x0000_0002;
    const DISP_CHANGE_SUCCESSFUL: i32 = 0;
    const MONITOR_DEFAULTTOPRIMARY: u32 = 0x0000_0001;
    const PHYSICAL_MONITOR_DESCRIPTION_SIZE: usize = 128;

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DevModeW {
        dm_device_name: [u16; 32],
        dm_spec_version: u16,
        dm_driver_version: u16,
        dm_size: u16,
        dm_driver_extra: u16,
        dm_fields: u32,
        dm_union: [u32; 4],
        dm_color: i16,
        dm_duplex: i16,
        dm_y_resolution: i16,
        dm_tt_option: i16,
        dm_collate: i16,
        dm_form_name: [u16; 32],
        dm_log_pixels: u16,
        dm_bits_per_pel: u32,
        dm_pels_width: u32,
        dm_pels_height: u32,
        dm_display_flags: u32,
        dm_display_frequency: u32,
        dm_icm_method: u32,
        dm_icm_intent: u32,
        dm_media_type: u32,
        dm_dither_type: u32,
        dm_reserved1: u32,
        dm_reserved2: u32,
        dm_panning_width: u32,
        dm_panning_height: u32,
    }

    impl DevModeW {
        fn zeroed() -> Self {
            Self {
                dm_device_name: [0; 32],
                dm_spec_version: 0,
                dm_driver_version: 0,
                dm_size: size_of::<Self>() as u16,
                dm_driver_extra: 0,
                dm_fields: 0,
                dm_union: [0; 4],
                dm_color: 0,
                dm_duplex: 0,
                dm_y_resolution: 0,
                dm_tt_option: 0,
                dm_collate: 0,
                dm_form_name: [0; 32],
                dm_log_pixels: 0,
                dm_bits_per_pel: 0,
                dm_pels_width: 0,
                dm_pels_height: 0,
                dm_display_flags: 0,
                dm_display_frequency: 0,
                dm_icm_method: 0,
                dm_icm_intent: 0,
                dm_media_type: 0,
                dm_dither_type: 0,
                dm_reserved1: 0,
                dm_reserved2: 0,
                dm_panning_width: 0,
                dm_panning_height: 0,
            }
        }

        fn to_spec(&self) -> DisplayModeSpec {
            DisplayModeSpec {
                width: self.dm_pels_width,
                height: self.dm_pels_height,
                refresh_hz: self.dm_display_frequency,
                bits_per_pel: self.dm_bits_per_pel,
            }
        }
    }

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PhysicalMonitor {
        h_physical_monitor: *mut c_void,
        sz_physical_monitor_description: [u16; PHYSICAL_MONITOR_DESCRIPTION_SIZE],
    }

    impl PhysicalMonitor {
        fn zeroed() -> Self {
            Self {
                h_physical_monitor: ptr::null_mut(),
                sz_physical_monitor_description: [0; PHYSICAL_MONITOR_DESCRIPTION_SIZE],
            }
        }
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplaySettingsExW(
            lpsz_device_name: *const u16,
            i_mode_num: u32,
            lp_dev_mode: *mut DevModeW,
            dw_flags: u32,
        ) -> i32;
        fn ChangeDisplaySettingsExW(
            lpsz_device_name: *const u16,
            lp_dev_mode: *mut DevModeW,
            hwnd: *mut c_void,
            dw_flags: u32,
            lparam: *mut c_void,
        ) -> i32;
        fn MonitorFromPoint(pt: Point, dw_flags: u32) -> *mut c_void;
    }

    #[link(name = "dxva2")]
    extern "system" {
        fn GetNumberOfPhysicalMonitorsFromHMONITOR(
            h_monitor: *mut c_void,
            pdw_number_of_physical_monitors: *mut u32,
        ) -> i32;
        fn GetPhysicalMonitorsFromHMONITOR(
            h_monitor: *mut c_void,
            dw_physical_monitor_array_size: u32,
            p_physical_monitor_array: *mut PhysicalMonitor,
        ) -> i32;
        fn DestroyPhysicalMonitors(
            dw_physical_monitor_array_size: u32,
            p_physical_monitor_array: *mut PhysicalMonitor,
        ) -> i32;
        fn GetMonitorBrightness(
            h_monitor: *mut c_void,
            pdw_minimum_brightness: *mut u32,
            pdw_current_brightness: *mut u32,
            pdw_maximum_brightness: *mut u32,
        ) -> i32;
        fn SetMonitorBrightness(h_monitor: *mut c_void, dw_new_brightness: u32) -> i32;
    }

    fn encode_wide_optional(device_name: Option<&str>) -> Option<Vec<u16>> {
        device_name.map(|name| name.encode_utf16().chain(std::iter::once(0)).collect())
    }

    struct PhysicalMonitorsGuard {
        monitors: Vec<PhysicalMonitor>,
    }

    impl PhysicalMonitorsGuard {
        fn for_primary() -> Result<Self, WindowsDisplayError> {
            let hmon = unsafe { MonitorFromPoint(Point { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
            if hmon.is_null() {
                return Err(WindowsDisplayError::Unavailable);
            }
            let mut count: u32 = 0;
            let ok = unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count) };
            if ok == 0 || count == 0 {
                return Err(WindowsDisplayError::Win32Error {
                    api: "GetNumberOfPhysicalMonitorsFromHMONITOR",
                    code: ok,
                });
            }
            let mut monitors = vec![PhysicalMonitor::zeroed(); count as usize];
            let ok =
                unsafe { GetPhysicalMonitorsFromHMONITOR(hmon, count, monitors.as_mut_ptr()) };
            if ok == 0 {
                return Err(WindowsDisplayError::Win32Error {
                    api: "GetPhysicalMonitorsFromHMONITOR",
                    code: ok,
                });
            }
            Ok(Self { monitors })
        }

        fn primary_handle(&self) -> Option<*mut c_void> {
            self.monitors
                .first()
                .map(|m| m.h_physical_monitor)
                .filter(|h| !h.is_null())
        }
    }

    impl Drop for PhysicalMonitorsGuard {
        fn drop(&mut self) {
            if !self.monitors.is_empty() {
                unsafe {
                    let _ = DestroyPhysicalMonitors(
                        self.monitors.len() as u32,
                        self.monitors.as_mut_ptr(),
                    );
                }
            }
        }
    }

    /// Optional bridge for `WmiMonitorBrightnessMethods::WmiSetBrightness` on internal eDP panels
    /// where DDC/CI (`dxva2.dll`) is unsupported.
    pub trait WmiBrightnessBridge: Send + Sync {
        fn read_wmi_brightness(&self) -> Result<u8, WindowsDisplayError>;
        fn write_wmi_brightness(&self, percent: u8) -> Result<(), WindowsDisplayError>;
    }

    /// Native Windows `user32.dll` + `dxva2.dll` (with optional WMI brightness bridge) transport.
    #[derive(Default)]
    pub struct Win32DisplayTransport {
        wmi_brightness: Option<Box<dyn WmiBrightnessBridge>>,
    }

    impl Win32DisplayTransport {
        pub fn new() -> Self {
            Self {
                wmi_brightness: None,
            }
        }

        pub fn with_wmi_brightness(bridge: Box<dyn WmiBrightnessBridge>) -> Self {
            Self {
                wmi_brightness: Some(bridge),
            }
        }
    }

    impl WindowsDisplayTransport for Win32DisplayTransport {
        fn is_available(&self) -> bool {
            self.current_mode(None).is_ok()
        }

        fn current_mode(
            &self,
            device_name: Option<&str>,
        ) -> Result<DisplayModeSpec, WindowsDisplayError> {
            let wide = encode_wide_optional(device_name);
            let dev_ptr = wide.as_ref().map_or(ptr::null(), |v| v.as_ptr());
            let mut mode = DevModeW::zeroed();
            let ok =
                unsafe { EnumDisplaySettingsExW(dev_ptr, ENUM_CURRENT_SETTINGS, &mut mode, 0) };
            if ok == 0 {
                return Err(WindowsDisplayError::Win32Error {
                    api: "EnumDisplaySettingsExW",
                    code: ok,
                });
            }
            Ok(mode.to_spec())
        }

        fn enumerate_modes(
            &self,
            device_name: Option<&str>,
        ) -> Result<Vec<DisplayModeSpec>, WindowsDisplayError> {
            let wide = encode_wide_optional(device_name);
            let dev_ptr = wide.as_ref().map_or(ptr::null(), |v| v.as_ptr());
            let mut modes = Vec::new();
            let mut idx: u32 = 0;
            loop {
                let mut mode = DevModeW::zeroed();
                let ok = unsafe { EnumDisplaySettingsExW(dev_ptr, idx, &mut mode, 0) };
                if ok == 0 {
                    break;
                }
                modes.push(mode.to_spec());
                idx = idx.saturating_add(1);
                if idx == u32::MAX {
                    break;
                }
            }
            if modes.is_empty() {
                return Err(WindowsDisplayError::Unavailable);
            }
            Ok(modes)
        }

        fn apply_refresh_rate(
            &self,
            device_name: Option<&str>,
            refresh_hz: u32,
        ) -> Result<(), WindowsDisplayError> {
            let wide = encode_wide_optional(device_name);
            let dev_ptr = wide.as_ref().map_or(ptr::null(), |v| v.as_ptr());
            let mut mode = DevModeW::zeroed();
            let ok =
                unsafe { EnumDisplaySettingsExW(dev_ptr, ENUM_CURRENT_SETTINGS, &mut mode, 0) };
            if ok == 0 {
                return Err(WindowsDisplayError::Win32Error {
                    api: "EnumDisplaySettingsExW",
                    code: ok,
                });
            }

            // Modify DM_DISPLAYFREQUENCY only so resolution and color depth stay untouched.
            mode.dm_fields = DM_DISPLAYFREQUENCY;
            mode.dm_display_frequency = refresh_hz;

            let test_rc = unsafe {
                ChangeDisplaySettingsExW(
                    dev_ptr,
                    &mut mode,
                    ptr::null_mut(),
                    CDS_TEST,
                    ptr::null_mut(),
                )
            };
            if test_rc != DISP_CHANGE_SUCCESSFUL {
                return Err(WindowsDisplayError::Win32Error {
                    api: "ChangeDisplaySettingsExW(CDS_TEST)",
                    code: test_rc,
                });
            }

            let apply_rc = unsafe {
                ChangeDisplaySettingsExW(
                    dev_ptr,
                    &mut mode,
                    ptr::null_mut(),
                    CDS_UPDATEREGISTRY,
                    ptr::null_mut(),
                )
            };
            if apply_rc != DISP_CHANGE_SUCCESSFUL {
                return Err(WindowsDisplayError::Win32Error {
                    api: "ChangeDisplaySettingsExW",
                    code: apply_rc,
                });
            }
            Ok(())
        }

        fn read_brightness(&self) -> Result<u8, WindowsDisplayError> {
            if let Some(ref wmi) = self.wmi_brightness {
                if let Ok(val) = wmi.read_wmi_brightness() {
                    return Ok(val.min(100));
                }
            }
            let guard = PhysicalMonitorsGuard::for_primary()?;
            let handle = guard
                .primary_handle()
                .ok_or(WindowsDisplayError::Unavailable)?;
            let mut min_b: u32 = 0;
            let mut cur_b: u32 = 0;
            let mut max_b: u32 = 0;
            let ok = unsafe { GetMonitorBrightness(handle, &mut min_b, &mut cur_b, &mut max_b) };
            if ok == 0 || max_b <= min_b {
                return Err(WindowsDisplayError::Win32Error {
                    api: "GetMonitorBrightness",
                    code: ok,
                });
            }
            let span = max_b - min_b;
            let clamped = cur_b.clamp(min_b, max_b) - min_b;
            Ok(((u64::from(clamped) * 100) / u64::from(span)).min(100) as u8)
        }

        fn write_brightness(&self, percent: u8) -> Result<(), WindowsDisplayError> {
            let pct = percent.min(100);
            if let Some(ref wmi) = self.wmi_brightness {
                if wmi.write_wmi_brightness(pct).is_ok() {
                    return Ok(());
                }
            }
            let guard = PhysicalMonitorsGuard::for_primary()?;
            let handle = guard
                .primary_handle()
                .ok_or(WindowsDisplayError::Unavailable)?;
            let mut min_b: u32 = 0;
            let mut cur_b: u32 = 0;
            let mut max_b: u32 = 0;
            let ok = unsafe { GetMonitorBrightness(handle, &mut min_b, &mut cur_b, &mut max_b) };
            if ok == 0 || max_b <= min_b {
                return Err(WindowsDisplayError::Win32Error {
                    api: "GetMonitorBrightness",
                    code: ok,
                });
            }
            let span = max_b - min_b;
            let target = min_b.saturating_add(((u64::from(pct) * u64::from(span)) / 100) as u32);
            let set_ok = unsafe { SetMonitorBrightness(handle, target) };
            if set_ok == 0 {
                return Err(WindowsDisplayError::Win32Error {
                    api: "SetMonitorBrightness",
                    code: set_ok,
                });
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use win32::{Win32DisplayTransport, WmiBrightnessBridge};
