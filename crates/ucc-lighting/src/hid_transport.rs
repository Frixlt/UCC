//! Cross-platform USB HID lighting transport seam (`M14.6` / `M15.4`, `ADR-06`).
//!
//! Shared protocol layer backing Linux `/dev/hidraw*` and Windows `hidapi` / `HidD_SetFeature`
//! for ITE `048D:600B` (calibrated per-key RGB keyboard) and ITE `048D:7001`/`048D:7000`
//! (front Lightbar + lid logo LED zone).

use crate::transport::{
    KeyboardCapabilities, LightbarCapabilities, LightingError, LightingTransport, RgbColor,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const ITE_VENDOR_ID: u16 = 0x048D;
pub const ITE_KEYBOARD_PRODUCT_ID: u16 = 0x600B;
pub const ITE_LIGHTBAR_PRODUCT_IDS: [u16; 2] = [0x7001, 0x7000];

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
}

/// Thread-safe in-memory HID feature-report mock for protocol & calibration testing.
#[derive(Debug, Default)]
pub struct MockHidTransport {
    devices: Mutex<Vec<HidDeviceInfo>>,
    reports: Mutex<Vec<(String, Vec<u8>)>>,
    feature_state: Mutex<BTreeMap<String, Vec<u8>>>,
    fail_writes: Mutex<bool>,
}

impl MockHidTransport {
    pub fn new(devices: Vec<HidDeviceInfo>) -> Self {
        Self {
            devices: Mutex::new(devices),
            reports: Mutex::new(Vec::new()),
            feature_state: Mutex::new(BTreeMap::new()),
            fail_writes: Mutex::new(false),
        }
    }

    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap() = fail;
    }

    pub fn recorded_reports(&self) -> Vec<(String, Vec<u8>)> {
        self.reports.lock().unwrap().clone()
    }
}

impl HidDeviceTransport for MockHidTransport {
    fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError> {
        Ok(self.devices.lock().unwrap().clone())
    }

    fn send_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        if *self.fail_writes.lock().unwrap() {
            return Err(LightingError::WriteFailed(
                "Simulated HID feature report failure".into(),
            ));
        }
        self.reports
            .lock()
            .unwrap()
            .push((path.to_string(), report.to_vec()));
        self.feature_state
            .lock()
            .unwrap()
            .insert(path.to_string(), report.to_vec());
        Ok(())
    }

    fn get_feature_report(&self, path: &str, len: usize) -> Result<Vec<u8>, LightingError> {
        let state = self.feature_state.lock().unwrap();
        let mut buf = state.get(path).cloned().unwrap_or_default();
        buf.resize(len, 0);
        Ok(buf)
    }
}

/// Uniwill / ITE direct USB HID [`LightingTransport`] supporting `048D:600B` keyboards and
/// `048D:7001`/`048D:7000` Lightbar + lid logo controllers.
pub struct UniwillHidTransport {
    hid: Arc<dyn HidDeviceTransport>,
    cached_colors: Mutex<Vec<RgbColor>>,
}

impl UniwillHidTransport {
    pub fn new(hid: Arc<dyn HidDeviceTransport>) -> Self {
        Self {
            hid,
            cached_colors: Mutex::new(Vec::new()),
        }
    }

    fn find_device(&self, product_ids: &[u16]) -> Result<Option<HidDeviceInfo>, LightingError> {
        Ok(self
            .hid
            .enumerate_devices()?
            .into_iter()
            .find(|d| d.vendor_id == ITE_VENDOR_ID && product_ids.contains(&d.product_id)))
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

    fn write_keyboard_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Err(LightingError::Unavailable);
        };
        let level = brightness.min(255) as u8;
        let packet = [0x08, 0x02, 0x01, 0x05, level, 0, 0, 0];
        self.hid.send_feature_report(&dev.path, &packet)
    }

    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&[ITE_KEYBOARD_PRODUCT_ID])? else {
            return Err(LightingError::Unavailable);
        };
        let mut frame = Vec::with_capacity(2 + colors.len() * 3);
        frame.push(0x14);
        frame.push(colors.len().min(255) as u8);
        for c in colors {
            frame.extend_from_slice(&[c.r, c.g, c.b]);
        }
        self.hid.send_feature_report(&dev.path, &frame)?;
        *self.cached_colors.lock().unwrap() = colors.to_vec();
        Ok(())
    }

    fn read_keyboard_colors(&self) -> Result<Vec<RgbColor>, LightingError> {
        Ok(self.cached_colors.lock().unwrap().clone())
    }

    fn detect_lightbar(&self) -> Result<Option<LightbarCapabilities>, LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Ok(None);
        };
        let zones = if dev.product_id == 0x7000 { 5 } else { 4 };
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
        let packet = [0x08, 0x01, level, 0, 0, 0, 0, 0];
        self.hid.send_feature_report(&dev.path, &packet)
    }

    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        let Some(dev) = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)? else {
            return Err(LightingError::Unavailable);
        };
        let packet = [0x14, 0x01, rgb.r, rgb.g, rgb.b, 0, 0, 0];
        self.hid.send_feature_report(&dev.path, &packet)
    }
}
