// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-platform USB HID lighting transport seam (`M14.6` / `M15.4`, `ADR-05` / `ADR-06`).
//!
//! Shared protocol layer backing Linux `/dev/hidraw*` and Windows `hidapi` / `HidD_SetFeature`
//! for ITE `048D:600B` (calibrated per-key RGB keyboard) and ITE `048D:7001`/`048D:7000`
//! (front Lightbar + lid logo LED zone).

use crate::hidraw_linux::HidrawDeviceTransport;
use crate::transport::{
    KeyboardCapabilities, LightbarCapabilities, LightingError, LightingTransport, RgbColor,
    SysfsLedTransport,
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
/// Maximum hardware brightness register value accepted by ITE `048D:600B` (`0..=50`).
pub const ITE_KEYBOARD_MAX_HW_BRIGHTNESS: u8 = 50;

/// Uniwill EC register addresses controlling Lightbar / chassis LED rail power (`ADR-05`).
pub const EC_LIGHTBAR_POWER_REG_PRIMARY: u16 = 0x0767;
pub const EC_LIGHTBAR_POWER_REG_SECONDARY: u16 = 0x0768;

/// Optional EC power-rail callback invoked when enabling/disabling Lightbar LED power
/// (`0x767` / `0x768`) when integrated with `UwccBackend`.
pub type EcLightbarPowerHook = Arc<dyn Fn(bool) -> Result<(), LightingError> + Send + Sync>;

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
    last_ec_lightbar_power: Mutex<Option<bool>>,
}

impl UniwillHidTransport {
    pub fn new(hid: Arc<dyn HidDeviceTransport>) -> Self {
        Self {
            hid,
            cached_colors: Mutex::new(Vec::new()),
            cached_keyboard_brightness: Mutex::new(None),
            ec_lightbar_power_hook: None,
            last_ec_lightbar_power: Mutex::new(None),
        }
    }

    /// Attaches an optional EC power-rail hook (`0x767`/`0x768`) for Lightbar / chassis LEDs.
    pub fn with_ec_lightbar_power_hook(mut self, hook: EcLightbarPowerHook) -> Self {
        self.ec_lightbar_power_hook = Some(hook);
        self
    }

    /// Returns the underlying [`HidDeviceTransport`].
    pub fn hid_transport(&self) -> &Arc<dyn HidDeviceTransport> {
        &self.hid
    }

    /// Scales logical keyboard brightness (`0..=255`) to ITE `048D:600B` hardware range (`0..=50`).
    pub fn scale_keyboard_brightness(brightness: u32) -> u8 {
        let max_hw = u32::from(ITE_KEYBOARD_MAX_HW_BRIGHTNESS);
        ((brightness.min(255) * max_hw + 127) / 255).min(max_hw) as u8
    }

    /// Invokes the optional EC Lightbar power-rail callback (`0x767`/`0x768`).
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

    /// Returns `true` when an ITE `048D:7000` controller with a lid logo LED (zone 5) is present.
    pub fn has_lid_logo(&self) -> Result<bool, LightingError> {
        Ok(self
            .find_device(&ITE_LIGHTBAR_PRODUCT_IDS)?
            .is_some_and(|d| d.product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE))
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
            let packet = [0x14, zone_id, rgb.r, rgb.g, rgb.b, 0, 0, 0];
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
            LIGHTBAR_LID_LOGO_ZONE_ID,
            rgb.r,
            rgb.g,
            rgb.b,
            0,
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

    fn set_keyboard_buffer_input(&self, enabled: bool) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Err(LightingError::Unavailable);
        };
        let packet = [0x09, 0x02, u8::from(enabled), 0, 0, 0, 0, 0];
        self.hid.send_feature_report(&dev.path, &packet)
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
        // ITE 048D:600B hardware brightness register accepts 0..=50 (`P0-11`).
        let level = Self::scale_keyboard_brightness(brightness);
        let packet = [0x08, 0x02, 0x01, 0x05, level, 0, 0, 0];
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
        // Chunk into <=64-byte USB HID feature reports (max 16 RGB zones per 51-byte packet, `P0-10`).
        const ZONES_PER_CHUNK: usize = 16;
        for (chunk_idx, chunk) in colors.chunks(ZONES_PER_CHUNK).enumerate() {
            let mut frame = Vec::with_capacity(3 + chunk.len() * 3);
            frame.push(0x14);
            frame.push(chunk_idx as u8);
            frame.push(chunk.len() as u8);
            for c in chunk {
                frame.extend_from_slice(&[c.r, c.g, c.b]);
            }
            self.hid.send_feature_report(&dev.path, &frame)?;
        }
        *self.cached_colors.lock().unwrap_or_else(|e| e.into_inner()) = colors.to_vec();
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
        let level = brightness.min(255) as u8;
        if level > 0 {
            self.set_lightbar_ec_power(true)?;
        }
        let packet = [0x08, 0x01, level, 0, 0, 0, 0, 0];
        self.hid.send_feature_report(&dev.path, &packet)?;
        if level == 0 {
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
        let zones = Self::lightbar_zone_count(dev.product_id) as u8;
        for zone in 1..=zones {
            let packet = [0x14, zone, rgb.r, rgb.g, rgb.b, 0, 0, 0];
            self.hid.send_feature_report(&dev.path, &packet)?;
        }
        Ok(())
    }

    fn write_lightbar_zones(&self, zones: &[RgbColor]) -> Result<(), LightingError> {
        UniwillHidTransport::write_lightbar_zones(self, zones)
    }
}

/// Creates a direct Linux `/dev/hidraw*` [`UniwillHidTransport`] for `uwcc` mode
/// (where `ite_8291` and `ite_8291_lb` kernel modules are blacklisted).
pub fn create_hidraw_lighting_transport() -> UniwillHidTransport {
    UniwillHidTransport::new(Arc::new(HidrawDeviceTransport::new()))
}

/// Creates a direct Linux `/dev/hidraw*` [`UniwillHidTransport`] with an EC Lightbar
/// power-rail callback (`0x767`/`0x768`).
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
