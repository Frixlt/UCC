// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-platform USB HID lighting transport seam (`M14.6` / `M15.4`, `ADR-05` / `ADR-06`).
//!
//! Shared protocol layer backing Linux `/dev/hidraw*` and Windows `hidapi` / `HidD_SetFeature`
//! for ITE `048D:600B` (calibrated per-key RGB keyboard) and ITE `048D:7001`/`048D:7000`
//! (front Lightbar + lid logo LED zone).

use crate::hidraw_linux::HidrawDeviceTransport;
#[cfg(not(windows))]
use crate::transport::SysfsLedTransport;
use crate::transport::{
    KeyboardCapabilities, LightbarCapabilities, LightingError, LightingTransport, RgbColor,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const ITE_VENDOR_ID: u16 = 0x048D;
pub const ITE_KEYBOARD_PRODUCT_ID: u16 = 0x600B;
pub const ITE_LIGHTBAR_PRODUCT_ID_4ZONE: u16 = 0x7001;
pub const ITE_LIGHTBAR_PRODUCT_ID_5ZONE: u16 = 0x7000;
pub const ITE_LIGHTBAR_PRODUCT_IDS: [u16; 2] =
    [ITE_LIGHTBAR_PRODUCT_ID_4ZONE, ITE_LIGHTBAR_PRODUCT_ID_5ZONE];

/// Number of front chassis Lightbar zones on ITE `048D:7001` and `048D:7000`.
pub const LIGHTBAR_FRONT_ZONES: usize = 4;
/// Zone index for the lid logo LED present on 5-zone ITE `048D:7000` controllers.
pub const LIGHTBAR_LID_LOGO_ZONE_ID: u8 = 5;
/// Maximum hardware brightness register value accepted by ITE `048D:600B` / `048D:7001` (`0..=50`).
pub const ITE_KEYBOARD_MAX_HW_BRIGHTNESS: u8 = 50;
/// Number of per-key LED matrix rows on ITE `048D:600B` (ITE 8291r3).
pub const ITE_KEYBOARD_ROWS: usize = 6;
/// Number of per-key LED matrix columns per row on ITE `048D:600B` (ITE 8291r3).
pub const ITE_KEYBOARD_COLS: usize = 21;
/// Payload length (excluding Report ID `0x00`) of an ITE 8291r3 per-key row output report.
pub const ITE_KEYBOARD_ROW_REPORT_LEN: usize = 64;
/// Number of addressable RGB LED slots on ITE `048D:7001` (ITE 8233) Lightbar (`0..16`).
pub const ITE_LIGHTBAR_8233_SLOTS: usize = 16;

/// Uniwill EC register address controlling Lightbar AC power (`0x0748` `EC_ADDR_LIGHTBAR_AC_CTRL`).
pub const EC_LIGHTBAR_POWER_REG_PRIMARY: u16 = 0x0748;
pub const EC_LIGHTBAR_POWER_REG_SECONDARY: u16 = 0x0748;

/// Optional EC power-rail callback invoked when enabling/disabling Lightbar LED power
/// (`0x0748` `EC_ADDR_LIGHTBAR_AC_CTRL` `BIT(0)`) when integrated with `UwccBackend`.
pub type EcLightbarPowerHook = Arc<dyn Fn(bool) -> Result<(), LightingError> + Send + Sync>;

/// Optional EC persistence callback for lid logo LED (`0x0767`, `0x07C5`, `0x0769..=0x076B`,
/// `0x07AB`, `0x078C`) paired with `048D:7001` HID Zone 3 commands.
pub type EcLidLogoHook = Arc<dyn Fn(bool, RgbColor, u8) -> Result<(), LightingError> + Send + Sync>;

/// USB HID device identity enumerated by host transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub path: String,
}

/// Abstract HID feature-report transport (`hidraw` on Linux, `HidD_SetFeature` on Windows).
pub trait HidDeviceTransport: Send + Sync {
    fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError>;
    fn send_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError>;
    fn get_feature_report(&self, path: &str, len: usize) -> Result<Vec<u8>, LightingError>;

    /// Sends a raw HID output report (used for ITE 8291r3 64-byte per-key row frames).
    ///
    /// Defaults to [`Self::send_feature_report`] for lightweight test mocks that do not
    /// distinguish output reports from feature reports.
    fn send_output_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        self.send_feature_report(path, report)
    }

    /// Optional HID usage page metadata for interface disambiguation (`P0-12`).
    fn device_usage_page(&self, _path: &str) -> Option<u16> {
        None
    }

    /// Optional USB HID interface number for interface disambiguation (`P0-12`).
    ///
    /// Defaults to `None` so in-memory mocks and non-Linux transports remain completely isolated
    /// from host `/sys/class/hidraw`. Linux [`HidrawDeviceTransport`] overrides this method to
    /// query its configured sysfs root.
    fn device_interface_number(&self, _path: &str) -> Option<i32> {
        None
    }

    /// Returns `true` if this transport queries the host's `/sys/class/hidraw` tree.
    fn uses_host_sysfs(&self) -> bool {
        false
    }
}

/// Thread-safe in-memory HID feature-report mock for protocol & calibration testing.
#[derive(Debug, Default)]
pub struct MockHidTransport {
    devices: Mutex<Vec<HidDeviceInfo>>,
    reports: Mutex<Vec<(String, Vec<u8>)>>,
    feature_state: Mutex<BTreeMap<String, Vec<u8>>>,
    usage_pages: Mutex<BTreeMap<String, u16>>,
    interface_numbers: Mutex<BTreeMap<String, i32>>,
    fail_writes: Mutex<bool>,
}

impl MockHidTransport {
    pub fn new(devices: Vec<HidDeviceInfo>) -> Self {
        Self {
            devices: Mutex::new(devices),
            reports: Mutex::new(Vec::new()),
            feature_state: Mutex::new(BTreeMap::new()),
            usage_pages: Mutex::new(BTreeMap::new()),
            interface_numbers: Mutex::new(BTreeMap::new()),
            fail_writes: Mutex::new(false),
        }
    }

    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    pub fn set_device_usage_page(&self, path: impl Into<String>, usage_page: u16) {
        self.usage_pages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), usage_page);
    }

    pub fn set_device_interface_number(&self, path: impl Into<String>, interface_number: i32) {
        self.interface_numbers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), interface_number);
    }

    pub fn recorded_reports(&self) -> Vec<(String, Vec<u8>)> {
        self.reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear_reports(&self) {
        self.reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl HidDeviceTransport for MockHidTransport {
    fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError> {
        Ok(self
            .devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    fn send_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        if *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(LightingError::WriteFailed(
                "Simulated HID feature report failure".into(),
            ));
        }
        self.reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((path.to_string(), report.to_vec()));
        self.feature_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_string(), report.to_vec());
        Ok(())
    }

    fn get_feature_report(&self, path: &str, len: usize) -> Result<Vec<u8>, LightingError> {
        let state = self.feature_state.lock().unwrap_or_else(|e| e.into_inner());
        let mut buf = state.get(path).cloned().unwrap_or_default();
        buf.resize(len, 0);
        Ok(buf)
    }

    fn device_usage_page(&self, path: &str) -> Option<u16> {
        self.usage_pages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .copied()
    }

    fn device_interface_number(&self, path: &str) -> Option<i32> {
        self.interface_numbers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .copied()
    }

    fn uses_host_sysfs(&self) -> bool {
        false
    }
}

/// Reads the USB `bInterfaceNumber` for a Linux `hidraw` device from sysfs.
///
/// Checks `{root}/{hidraw_name}/device/../bInterfaceNumber` first (standard Linux USB HID
/// hierarchy where `device` points to the HID child of the USB interface), falling back to
/// `{root}/{hidraw_name}/device/bInterfaceNumber`.
pub fn read_sysfs_hidraw_interface_number(
    sysfs_hidraw_root: &std::path::Path,
    hidraw_name: &str,
) -> Option<i32> {
    if !crate::hidraw_linux::is_valid_hidraw_name(hidraw_name) {
        return None;
    }
    let device_dir = sysfs_hidraw_root.join(hidraw_name).join("device");
    let candidates = [
        device_dir.join("..").join("bInterfaceNumber"),
        device_dir.join("bInterfaceNumber"),
    ];
    for iface_path in candidates {
        if let Ok(raw) = std::fs::read_to_string(&iface_path) {
            if let Ok(iface_num) = i32::from_str_radix(raw.trim(), 16) {
                return Some(iface_num);
            }
        }
    }
    None
}

/// Uniwill / ITE direct USB HID [`LightingTransport`] supporting `048D:600B` keyboards and
/// `048D:7001`/`048D:7000` Lightbar + lid logo controllers.
pub struct UniwillHidTransport {
    hid: Arc<dyn HidDeviceTransport>,
    cached_colors: Mutex<Vec<RgbColor>>,
    cached_keyboard_brightness: Mutex<Option<u32>>,
    ec_lightbar_power_hook: Option<EcLightbarPowerHook>,
    ec_lid_logo_hook: Option<EcLidLogoHook>,
    last_ec_lightbar_power: Mutex<Option<bool>>,
}

impl UniwillHidTransport {
    pub fn new(hid: Arc<dyn HidDeviceTransport>) -> Self {
        Self {
            hid,
            cached_colors: Mutex::new(Vec::new()),
            cached_keyboard_brightness: Mutex::new(None),
            ec_lightbar_power_hook: None,
            ec_lid_logo_hook: None,
            last_ec_lightbar_power: Mutex::new(None),
        }
    }

    /// Attaches an optional EC power-rail hook (`0x0748`) for Lightbar / chassis LEDs.
    pub fn with_ec_lightbar_power_hook(mut self, hook: EcLightbarPowerHook) -> Self {
        self.ec_lightbar_power_hook = Some(hook);
        self
    }

    /// Attaches an optional EC lid-logo persistence hook for `048D:7001` Zone 3 lid logos.
    pub fn with_ec_lid_logo_hook(mut self, hook: EcLidLogoHook) -> Self {
        self.ec_lid_logo_hook = Some(hook);
        self
    }

    /// Returns the underlying [`HidDeviceTransport`].
    pub fn hid_transport(&self) -> &Arc<dyn HidDeviceTransport> {
        &self.hid
    }

    /// Scales logical keyboard brightness (`0..=255`) to ITE `048D:600B` hardware range (`0..=50`).
    ///
    /// Non-zero inputs (`1..=255`) always map to at least `1` so dim settings never power off LEDs.
    pub fn scale_keyboard_brightness(brightness: u32) -> u8 {
        if brightness == 0 {
            return 0;
        }
        let max_hw = u32::from(ITE_KEYBOARD_MAX_HW_BRIGHTNESS);
        ((brightness.min(255) * max_hw + 127) / 255).clamp(1, max_hw) as u8
    }

    /// Invokes the optional EC Lightbar power-rail callback (`0x0748`).
    pub fn set_lightbar_ec_power(&self, enabled: bool) -> Result<(), LightingError> {
        if let Some(hook) = &self.ec_lightbar_power_hook {
            hook(enabled)?;
            *self
                .last_ec_lightbar_power
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(enabled);
        }
        Ok(())
    }

    fn ensure_lightbar_ec_power_on(&self) -> Result<(), LightingError> {
        if self.ec_lightbar_power_hook.is_none() {
            return Ok(());
        }
        let already_on = *self
            .last_ec_lightbar_power
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            == Some(true);
        if !already_on {
            self.set_lightbar_ec_power(true)?;
        }
        Ok(())
    }

    fn is_vendor_lighting_interface(&self, dev: &HidDeviceInfo) -> bool {
        if let Some(usage_page) = self.hid.device_usage_page(&dev.path) {
            return usage_page >= 0xFF00;
        }
        if let Some(interface_number) = self.hid.device_interface_number(&dev.path) {
            return interface_number == 1;
        }
        true
    }

    fn find_device(&self, product_ids: &[u16]) -> Result<Option<HidDeviceInfo>, LightingError> {
        Ok(self.hid.enumerate_devices()?.into_iter().find(|d| {
            d.vendor_id == ITE_VENDOR_ID
                && product_ids.contains(&d.product_id)
                && self.is_vendor_lighting_interface(d)
        }))
    }

    fn lightbar_zone_count(product_id: u16) -> usize {
        if product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
            LIGHTBAR_FRONT_ZONES + 1
        } else {
            LIGHTBAR_FRONT_ZONES
        }
    }

    /// Returns `true` when a lid logo LED is supported (`048D:7000` zone 5 or `048D:7001` + EC).
    pub fn has_lid_logo(&self) -> Result<bool, LightingError> {
        Ok(self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)?.is_some_and(|d| {
            d.product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE
                || (d.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE
                    && self.ec_lid_logo_hook.is_some())
        }))
    }

    /// Writes the lid logo LED (`048D:7001` HID Zone 3 + EC persistence, or `048D:7000` zone 5).
    pub fn write_lid_logo(
        &self,
        enabled: bool,
        rgb: RgbColor,
        brightness: u8,
    ) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Err(LightingError::Unavailable);
        };
        if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
            let color = if enabled && brightness > 0 {
                let scale = |c: u8| ((u16::from(c) * u16::from(brightness) + 127) / 255) as u8;
                RgbColor::new(scale(rgb.r), scale(rgb.g), scale(rgb.b))
            } else {
                RgbColor::BLACK
            };
            return self.write_lid_logo_color(color);
        }
        if self.ec_lid_logo_hook.is_none() {
            return Err(LightingError::Unavailable);
        }
        if enabled && brightness > 0 {
            let lb_level = (((u32::from(brightness) * 100) + 127) / 255).clamp(1, 100) as u8;
            let pwr_on = [0x1A, 0x03, 0x01, 0x04, 0x00, 0x00, 0x00, 0x01];
            let set_col = [0x14, 0x03, 0x01, rgb.r, rgb.g, rgb.b, 0x00, 0x00];
            let apply = [0x08, 0x23, 0x01, 0x01, lb_level, 0x01, 0x00, 0x00];
            self.hid.send_feature_report(&dev.path, &pwr_on)?;
            self.hid.send_feature_report(&dev.path, &set_col)?;
            self.hid.send_feature_report(&dev.path, &apply)?;
            if let Some(ref hook) = self.ec_lid_logo_hook {
                hook(true, rgb, brightness)?;
            }
        } else {
            let reset = [0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00];
            let stop_fx = [0x08, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
            let clear_z3 = [0x08, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00];
            let pwr_off = [0x1A, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
            self.hid.send_feature_report(&dev.path, &reset)?;
            self.hid.send_feature_report(&dev.path, &stop_fx)?;
            self.hid.send_feature_report(&dev.path, &clear_z3)?;
            self.hid.send_feature_report(&dev.path, &pwr_off)?;
            if let Some(ref hook) = self.ec_lid_logo_hook {
                hook(false, RgbColor::BLACK, 0)?;
            }
        }
        Ok(())
    }

    /// Writes individual per-zone RGB colors to front Lightbar (`1..=4`) and optional lid logo (`5`).
    pub fn write_lightbar_zones(&self, zone_colors: &[RgbColor]) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Err(LightingError::Unavailable);
        };
        let max_zones = Self::lightbar_zone_count(dev.product_id);
        if zone_colors.is_empty() || zone_colors.len() > max_zones {
            return Err(LightingError::InvalidConfig(format!(
                "Lightbar zone count {} out of range 1..={max_zones}",
                zone_colors.len()
            )));
        }
        if zone_colors.iter().any(|&c| c != RgbColor::BLACK) {
            self.ensure_lightbar_ec_power_on()?;
        }
        for (idx, rgb) in zone_colors.iter().enumerate() {
            let zone_id = (idx + 1) as u8;
            let packet = [0x14, 0x00, zone_id, rgb.r, rgb.g, rgb.b, 0, 0];
            self.hid.send_feature_report(&dev.path, &packet)?;
        }
        Ok(())
    }

    /// Writes the lid logo LED color (zone 5) on ITE `048D:7000` controllers.
    ///
    /// Returns [`LightingError::Unavailable`] if no Lightbar controller is present or if the
    /// attached controller is a 4-zone `048D:7001` without a lid logo LED.
    pub fn write_lid_logo_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Err(LightingError::Unavailable);
        };
        if dev.product_id != ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
            return Err(LightingError::Unavailable);
        }
        if rgb != RgbColor::BLACK {
            self.ensure_lightbar_ec_power_on()?;
        }
        let packet = [
            0x14,
            0x00,
            LIGHTBAR_LID_LOGO_ZONE_ID,
            rgb.r,
            rgb.g,
            rgb.b,
            0,
            0,
        ];
        self.hid.send_feature_report(&dev.path, &packet)
    }
}

impl LightingTransport for UniwillHidTransport {
    fn detect_keyboard(&self) -> Result<Option<KeyboardCapabilities>, LightingError> {
        let Some(_dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Ok(None);
        };
        Ok(Some(KeyboardCapabilities {
            zones: 126,
            max_brightness: 255,
            max_red: 255,
            max_green: 255,
            max_blue: 255,
            led_ids: (0..126).collect(),
        }))
    }

    fn set_keyboard_buffer_input(&self, _enabled: bool) -> Result<(), LightingError> {
        // Sysfs `buffer_input` is only needed to batch per-LED sysfs writes in `ite_8291.ko`.
        // Direct HID already uploads all 6 rows atomically in `write_keyboard_zones`; sending
        // command `0x09` (`SET_BRIGHTNESS`) here would overwrite hardware brightness to 0/1.
        let Some(_dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Err(LightingError::Unavailable);
        };
        Ok(())
    }

    fn read_keyboard_brightness(&self) -> Result<Option<u32>, LightingError> {
        Ok(*self
            .cached_keyboard_brightness
            .lock()
            .unwrap_or_else(|e| e.into_inner()))
    }

    fn write_keyboard_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Err(LightingError::Unavailable);
        };
        // ITE 048D:600B (ITE 8291r3) hardware brightness register accepts 0..=50.
        // Use UserEffect (0x33) when enabled so per-key row colors remain active, and SetOffOp
        // (0x01) when brightness == 0.
        let level = Self::scale_keyboard_brightness(brightness);
        let packet = if level == 0 {
            [0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        } else {
            [0x08, 0x02, 0x33, 0x00, level, 0x00, 0x00, 0x00]
        };
        self.hid.send_feature_report(&dev.path, &packet)?;
        *self
            .cached_keyboard_brightness
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(brightness.min(255));
        Ok(())
    }

    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Err(LightingError::Unavailable);
        };
        if colors.is_empty() {
            *self.cached_colors.lock().unwrap_or_else(|e| e.into_inner()) = Vec::new();
            return Ok(());
        }
        // ITE 8291r3 per-key matrix protocol: 6 rows x 21 columns.
        // Each row is selected via 8-byte feature report `SET_ROW_INDEX` (`0x16 0x00 row ...`)
        // followed by a 64-byte planar output report:
        //   buf[0]       = 0x00 (pad)
        //   buf[1..22]   = Blue  (21 cols)
        //   buf[22..43]  = Green (21 cols)
        //   buf[43..64]  = Red   (21 cols)
        let mut merged = self
            .cached_colors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if merged.len() < colors.len() {
            merged.resize(colors.len(), RgbColor::BLACK);
        }
        merged[..colors.len()].copy_from_slice(colors);

        let mut rows = [[0u8; ITE_KEYBOARD_ROW_REPORT_LEN]; ITE_KEYBOARD_ROWS];
        for (idx, c) in merged
            .iter()
            .take(ITE_KEYBOARD_ROWS * ITE_KEYBOARD_COLS)
            .enumerate()
        {
            let row = idx / ITE_KEYBOARD_COLS;
            let col = idx % ITE_KEYBOARD_COLS;
            rows[row][1 + col] = c.b;
            rows[row][1 + ITE_KEYBOARD_COLS + col] = c.g;
            rows[row][1 + 2 * ITE_KEYBOARD_COLS + col] = c.r;
        }
        for (row_idx, row_buf) in rows.iter().enumerate() {
            let select_row = [0x16, 0x00, row_idx as u8, 0x00, 0x00, 0x00, 0x00, 0x00];
            self.hid.send_feature_report(&dev.path, &select_row)?;
            self.hid.send_output_report(&dev.path, row_buf)?;
        }
        let cached_b = *self
            .cached_keyboard_brightness
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let level = match cached_b {
            Some(b) => Self::scale_keyboard_brightness(b),
            None => ITE_KEYBOARD_MAX_HW_BRIGHTNESS,
        };
        if level > 0 {
            let commit_user = [0x08, 0x02, 0x33, 0x00, level, 0x00, 0x00, 0x00];
            self.hid.send_feature_report(&dev.path, &commit_user)?;
        }
        *self.cached_colors.lock().unwrap_or_else(|e| e.into_inner()) = merged;
        Ok(())
    }

    fn read_keyboard_colors(&self) -> Result<Vec<RgbColor>, LightingError> {
        Ok(self
            .cached_colors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    fn detect_lightbar(&self) -> Result<Option<LightbarCapabilities>, LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Ok(None);
        };
        let zones = Self::lightbar_zone_count(dev.product_id);
        Ok(Some(LightbarCapabilities {
            zones,
            max_brightness: 255,
        }))
    }

    fn write_lightbar_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Err(LightingError::Unavailable);
        };
        let level = Self::scale_keyboard_brightness(brightness);
        if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE {
            let lb_level = if brightness == 0 {
                0
            } else {
                (((brightness.min(255) * 100) + 127) / 255).clamp(1, 100) as u8
            };
            if lb_level > 0 {
                self.set_lightbar_ec_power(true)?;
                let on_packet = [0x08, 0x02, 0x01, 0x05, 50, 0x01, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &on_packet)?;
                let br_packet = [0x09, 0x02, lb_level, 0x00, 0x00, 0x00, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &br_packet)?;
            } else {
                let br_packet = [0x09, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &br_packet)?;
                let off_packet = [0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &off_packet)?;
                self.set_lightbar_ec_power(false)?;
            }
        } else if level > 0 {
            self.set_lightbar_ec_power(true)?;
            let packet = [0x08, 0x02, 0x01, 0x05, level, 0x01, 0x00, 0x00];
            self.hid.send_feature_report(&dev.path, &packet)?;
        } else {
            let packet = [0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
            self.hid.send_feature_report(&dev.path, &packet)?;
            self.set_lightbar_ec_power(false)?;
        }
        Ok(())
    }

    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Err(LightingError::Unavailable);
        };
        if rgb != RgbColor::BLACK {
            self.ensure_lightbar_ec_power_on()?;
        }
        if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE {
            for slot in 0..=1u8 {
                let packet = [0x14, 0x00, slot, rgb.r, rgb.g, rgb.b, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &packet)?;
            }
        } else {
            let zones = Self::lightbar_zone_count(dev.product_id) as u8;
            for zone in 1..=zones {
                let packet = [0x14, 0x00, zone, rgb.r, rgb.g, rgb.b, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &packet)?;
            }
        }
        Ok(())
    }

    fn write_lightbar_zones(&self, zones: &[RgbColor]) -> Result<(), LightingError> {
        UniwillHidTransport::write_lightbar_zones(self, zones)
    }

    fn has_lid_logo(&self) -> Result<bool, LightingError> {
        UniwillHidTransport::has_lid_logo(self)
    }

    fn write_lid_logo(
        &self,
        enabled: bool,
        rgb: RgbColor,
        brightness: u8,
    ) -> Result<(), LightingError> {
        UniwillHidTransport::write_lid_logo(self, enabled, rgb, brightness)
    }
}

/// Creates a direct Linux `/dev/hidraw*` [`UniwillHidTransport`] for `uwcc` mode
/// (where `ite_8291` and `ite_8291_lb` kernel modules are blacklisted).
pub fn create_hidraw_lighting_transport() -> UniwillHidTransport {
    UniwillHidTransport::new(Arc::new(HidrawDeviceTransport::new()))
}

/// Creates a direct Linux `/dev/hidraw*` [`UniwillHidTransport`] with an EC Lightbar
/// power-rail callback (`0x0748`).
pub fn create_hidraw_lighting_transport_with_ec_power(
    hook: EcLightbarPowerHook,
) -> UniwillHidTransport {
    create_hidraw_lighting_transport().with_ec_lightbar_power_hook(hook)
}

fn transport_detects_any_hardware(transport: &dyn LightingTransport) -> bool {
    transport.detect_keyboard().ok().flatten().is_some()
        || transport.detect_lightbar().ok().flatten().is_some()
}

/// Pure transport-selection helper used by [`open_best_lighting_transport`].
///
/// When `prefer_hidraw` is `true` (e.g. `uccd` running on `UwccBackend`), prefers `hidraw` if it
/// detects keyboard or lightbar hardware, falling back to `sysfs` only if `sysfs` detects hardware
/// and `hidraw` does not. When `prefer_hidraw` is `false`, prefers `sysfs` first with fallback to
/// `hidraw`.
pub fn select_best_lighting_transport(
    prefer_hidraw: bool,
    hidraw: Arc<dyn LightingTransport>,
    sysfs: Arc<dyn LightingTransport>,
) -> Arc<dyn LightingTransport> {
    if prefer_hidraw {
        if transport_detects_any_hardware(hidraw.as_ref()) {
            return hidraw;
        }
        if transport_detects_any_hardware(sysfs.as_ref()) {
            return sysfs;
        }
        hidraw
    } else {
        if transport_detects_any_hardware(sysfs.as_ref()) {
            return sysfs;
        }
        if transport_detects_any_hardware(hidraw.as_ref()) {
            return hidraw;
        }
        sysfs
    }
}

/// Opens the best available [`LightingTransport`] for the active platform backend.
///
/// Pass `prefer_hidraw = true` when running on `UwccBackend` (where `ite_8291`/`ite_8291_lb` are
/// blacklisted), or `false` when running on legacy `tuxedo-drivers` sysfs LED drivers.
pub fn open_best_lighting_transport(prefer_hidraw: bool) -> Arc<dyn LightingTransport> {
    #[cfg(windows)]
    {
        let _ = prefer_hidraw;
        let win_hid = Arc::new(crate::windows_hid::WindowsHidTransport::new(Arc::new(
            crate::windows_hid::LiveWinHidSysApi::new(),
        )));
        Arc::new(UniwillHidTransport::new(win_hid))
    }
    #[cfg(not(windows))]
    {
        let hidraw: Arc<dyn LightingTransport> = Arc::new(create_hidraw_lighting_transport());
        let sysfs: Arc<dyn LightingTransport> = Arc::new(SysfsLedTransport::default());
        select_best_lighting_transport(prefer_hidraw, hidraw, sysfs)
    }
}
