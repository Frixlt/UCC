// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-platform USB HID lighting transport seam (`M14.6` / `M15.4`, `ADR-05` / `ADR-06`).
//!
//! Shared protocol layer backing Linux `/dev/hidraw*` and Windows `hidapi` / `HidD_SetFeature`
//! for ITE `048D:600B` (calibrated per-key RGB keyboard), ITE `048D:CE00` (4-zone `0xFF12` or
//! per-key rev `0.03`), and ITE `048D:7001`/`048D:7000` (front Lightbar + lid logo LED zone).

use crate::hidraw_linux::HidrawDeviceTransport;
#[cfg(not(windows))]
use crate::transport::SysfsLedTransport;
use crate::transport::{
    KeyboardCapabilities, LightbarCapabilities, LightingError, LightingTransport, RgbColor,
};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const ITE_VENDOR_ID: u16 = 0x048D;
pub const ITE_KEYBOARD_PRODUCT_ID: u16 = 0x600B;
pub const ITE_KEYBOARD_PRODUCT_ID_CE00: u16 = 0xCE00;
pub const ITE_KEYBOARD_PRODUCT_IDS: [u16; 2] =
    [ITE_KEYBOARD_PRODUCT_ID, ITE_KEYBOARD_PRODUCT_ID_CE00];
pub const ITE_LIGHTBAR_PRODUCT_ID_4ZONE: u16 = 0x7001;
pub const ITE_LIGHTBAR_PRODUCT_ID_5ZONE: u16 = 0x7000;
pub const ITE_LIGHTBAR_PRODUCT_IDS: [u16; 2] =
    [ITE_LIGHTBAR_PRODUCT_ID_4ZONE, ITE_LIGHTBAR_PRODUCT_ID_5ZONE];

/// Non-LED vendor collection on `048D:600B` `MI_00` that must NEVER be selected for lighting.
pub const ITE_USAGE_PAGE_EXCLUDED_FF89: u16 = 0xFF89;
/// Primary ITE per-key / lightbar lighting Usage Page (`0xFF03`, highest priority).
pub const ITE_USAGE_PAGE_PER_KEY: u16 = 0xFF03;
/// ITE 4-zone keyboard lighting Usage Page (`0xFF12`, second priority).
pub const ITE_USAGE_PAGE_4ZONE: u16 = 0xFF12;
/// Legacy ITE keyboard lighting Usage Page (`0xFF02`, third priority).
pub const ITE_USAGE_PAGE_LEGACY: u16 = 0xFF02;

/// Number of front chassis Lightbar zones on ITE `048D:7001`.
pub const LIGHTBAR_FRONT_ZONES: usize = 4;
/// Legacy zone index constant for the lid logo LED.
pub const LIGHTBAR_LID_LOGO_ZONE_ID: u8 = 5;
/// Maximum hardware brightness register value accepted by ITE `048D:600B` / `048D:CE00` / `048D:7000` (`0..=50`).
pub const ITE_KEYBOARD_MAX_HW_BRIGHTNESS: u8 = 50;
/// Number of per-key LED matrix rows on ITE `048D:600B` / `048D:CE00` rev `0.03` (ITE 8291r3).
pub const ITE_KEYBOARD_ROWS: usize = 6;
/// Number of per-key LED matrix columns per row on ITE `048D:600B` / `048D:CE00` rev `0.03`.
pub const ITE_KEYBOARD_COLS: usize = 21;
/// Payload length (excluding Report ID `0x00`) of an ITE 8291r3 per-key row output report.
pub const ITE_KEYBOARD_ROW_REPORT_LEN: usize = 64;
/// Number of addressable RGB LED slots on ITE `048D:7001` (ITE 8233) Lightbar (`0..16`).
pub const ITE_LIGHTBAR_8233_SLOTS: usize = 16;

/// Uniwill EC register address controlling Lightbar AC power (`0x0748` `EC_ADDR_LIGHTBAR_AC_CTRL`).
pub const EC_LIGHTBAR_POWER_REG_PRIMARY: u16 = 0x0748;
pub const EC_LIGHTBAR_POWER_REG_SECONDARY: u16 = 0x0748;

/// Optional EC power-rail callback invoked when enabling/disabling Lightbar LED power
/// (`0x0748` `EC_ADDR_LIGHTBAR_AC_CTRL`) when integrated with `UwccBackend`.
pub type EcLightbarPowerHook = Arc<dyn Fn(bool) -> Result<(), LightingError> + Send + Sync>;

/// Optional EC persistence callback for lid logo LED (`0x07C5`, `0x0769..=0x076B`, `0x07AB`,
/// `0x078C`) paired with `048D:7001` HID Zone 3 commands.
pub type EcLidLogoHook = Arc<dyn Fn(bool, RgbColor, u8) -> Result<(), LightingError> + Send + Sync>;

/// Ranks a HID Usage Page for ITE lighting interface selection.
///
/// Priority order: `0xFF03 (4) > 0xFF12 (3) > 0xFF02 (2) > other >= 0xFF00 (1)`.
/// Explicitly excludes `0xFF89` (returns `0`) and all standard non-vendor pages `< 0xFF00`.
pub const fn usage_page_priority(usage_page: u16) -> u8 {
    match usage_page {
        ITE_USAGE_PAGE_PER_KEY => 4,
        ITE_USAGE_PAGE_4ZONE => 3,
        ITE_USAGE_PAGE_LEGACY => 2,
        ITE_USAGE_PAGE_EXCLUDED_FF89 => 0,
        up if up >= 0xFF00 => 1,
        _ => 0,
    }
}

/// Detected ITE USB HID keyboard protocol variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IteKeyboardKind {
    /// Per-key 126-LED matrix (`048D:600B` or `048D:CE00` rev `0.03` with 65-byte output reports).
    PerKey126,
    /// 4-zone RGB keyboard (`048D:CE00` on Usage Page `0xFF12`).
    FourZone,
}

/// Addressable LED zones on ITE `048D:7000` (`HIDLightbar4`), discovered via `0xA2` probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ite7000Zone {
    /// Hinge lightbar (`0x14` byte 2 = `1`, `0x08` ctrl = `0x21`).
    Hinge,
    /// Front/base lightbar (`0x14` byte 2 = `2`, `0x08` ctrl = `0x22`).
    Base,
    /// Lid logo (`0x14` byte 2 = `3`, `0x08` ctrl = `0x23`).
    Logo,
}

impl Ite7000Zone {
    /// Zone selector written into byte 1 of `0x14` (`[0x14, flag, palette_idx, R, G, B, 0, 0]`).
    pub const fn color_flag(self) -> u8 {
        match self {
            Self::Hinge => 1,
            Self::Base => 2,
            Self::Logo => 3,
        }
    }

    /// Control byte written into byte 1 of `0x08` (`[0x08, ctrl, effect, speed, level, color, 0, 0]`).
    pub const fn effect_ctrl(self) -> u8 {
        match self {
            Self::Hinge => 0x21,
            Self::Base => 0x22,
            Self::Logo => 0x23,
        }
    }
}

/// Decodes the `048D:7000` `0xA2` topology code (unframed payload byte 1, wire byte 2).
///
/// Mapping per `HIDLightbar4` firmware specification:
/// - `0` | `5` -> `[Hinge]`
/// - `1` -> `[Hinge, Logo]`
/// - `4` -> `[Hinge, Base, Logo]`
/// - `6` -> `[Hinge, Base]`
pub fn decode_ite7000_topology(code: u8) -> Option<Vec<Ite7000Zone>> {
    match code {
        0 | 5 => Some(vec![Ite7000Zone::Hinge]),
        1 => Some(vec![Ite7000Zone::Hinge, Ite7000Zone::Logo]),
        4 => Some(vec![
            Ite7000Zone::Hinge,
            Ite7000Zone::Base,
            Ite7000Zone::Logo,
        ]),
        6 => Some(vec![Ite7000Zone::Hinge, Ite7000Zone::Base]),
        _ => None,
    }
}

fn default_ite7000_topology() -> Vec<Ite7000Zone> {
    vec![Ite7000Zone::Hinge, Ite7000Zone::Base, Ite7000Zone::Logo]
}

fn is_transient_io_error(err: &LightingError) -> bool {
    match err {
        LightingError::Io(io_err) => {
            if matches!(
                io_err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) {
                return true;
            }
            #[cfg(unix)]
            if matches!(
                io_err.raw_os_error(),
                Some(code) if code == libc::EAGAIN || code == libc::ETIMEDOUT
            ) {
                return true;
            }
            false
        }
        _ => false,
    }
}

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

    /// Optional HID usage metadata (e.g. Windows `HidpCaps.usage`, preferring `0x0002` on `048D:7001`).
    fn device_usage(&self, _path: &str) -> Option<u16> {
        None
    }

    /// Optional HID device version number (`HiddAttributes.version_number`, e.g. `0x0003` for `048D:CE00` per-key).
    fn device_version_number(&self, _path: &str) -> Option<u16> {
        None
    }

    /// Optional HID output report byte length (e.g. `65` for `048D:CE00` per-key rev `0.03`).
    fn device_output_report_byte_length(&self, _path: &str) -> Option<usize> {
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

    /// Returns `true` when missing usage-page and interface-number metadata must reject a device
    /// rather than falling back to `true` (prevents opening boot keyboard `0x0001` on `report_descriptor`
    /// read failure on real Linux/Windows transports).
    fn requires_explicit_interface_metadata(&self) -> bool {
        false
    }
}

/// Thread-safe in-memory HID feature-report mock for protocol & calibration testing.
#[derive(Debug, Default)]
pub struct MockHidTransport {
    devices: Mutex<Vec<HidDeviceInfo>>,
    reports: Mutex<Vec<(String, Vec<u8>)>>,
    feature_state: Mutex<BTreeMap<String, Vec<u8>>>,
    feature_responses: Mutex<BTreeMap<(String, u8), Vec<u8>>>,
    usage_pages: Mutex<BTreeMap<String, u16>>,
    usages: Mutex<BTreeMap<String, u16>>,
    version_numbers: Mutex<BTreeMap<String, u16>>,
    output_report_byte_lengths: Mutex<BTreeMap<String, usize>>,
    interface_numbers: Mutex<BTreeMap<String, i32>>,
    fail_writes: Mutex<bool>,
}

impl MockHidTransport {
    pub fn new(devices: Vec<HidDeviceInfo>) -> Self {
        Self {
            devices: Mutex::new(devices),
            reports: Mutex::new(Vec::new()),
            feature_state: Mutex::new(BTreeMap::new()),
            feature_responses: Mutex::new(BTreeMap::new()),
            usage_pages: Mutex::new(BTreeMap::new()),
            usages: Mutex::new(BTreeMap::new()),
            version_numbers: Mutex::new(BTreeMap::new()),
            output_report_byte_lengths: Mutex::new(BTreeMap::new()),
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

    pub fn set_device_usage(&self, path: impl Into<String>, usage: u16) {
        self.usages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), usage);
    }

    pub fn set_device_version_number(&self, path: impl Into<String>, version_number: u16) {
        self.version_numbers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), version_number);
    }

    pub fn set_device_output_report_byte_length(&self, path: impl Into<String>, len: usize) {
        self.output_report_byte_lengths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), len);
    }

    pub fn set_device_interface_number(&self, path: impl Into<String>, interface_number: i32) {
        self.interface_numbers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), interface_number);
    }

    pub fn set_feature_response(&self, path: impl Into<String>, command: u8, response: Vec<u8>) {
        self.feature_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((path.into(), command), response);
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
        let cmd = report.first().copied();
        let seeded = cmd.and_then(|c| {
            self.feature_responses
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(path.to_string(), c))
                .cloned()
        });
        if let Some(resp) = seeded {
            self.feature_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path.to_string(), resp);
        } else if cmd != Some(0xA2) {
            self.feature_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path.to_string(), report.to_vec());
        }
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

    fn device_usage(&self, path: &str) -> Option<u16> {
        self.usages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .copied()
    }

    fn device_version_number(&self, path: &str) -> Option<u16> {
        self.version_numbers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .copied()
    }

    fn device_output_report_byte_length(&self, path: &str) -> Option<usize> {
        self.output_report_byte_lengths
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

/// Uniwill / ITE direct USB HID [`LightingTransport`] supporting `048D:600B` and `048D:CE00`
/// keyboards and `048D:7001`/`048D:7000` Lightbar + lid logo controllers.
pub struct UniwillHidTransport {
    hid: Arc<dyn HidDeviceTransport>,
    keyboard_io_lock: Mutex<()>,
    cached_colors: Mutex<Vec<RgbColor>>,
    cached_keyboard_brightness: Mutex<Option<u32>>,
    cached_keyboard_dev: Mutex<Option<HidDeviceInfo>>,
    cached_lightbar_dev: Mutex<Option<HidDeviceInfo>>,
    cached_lightbar_brightness: Mutex<Option<u32>>,
    lightbar_7001_powered: Mutex<bool>,
    cached_ite7000_zones: Mutex<Option<Vec<Ite7000Zone>>>,
    ec_lightbar_power_hook: Option<EcLightbarPowerHook>,
    ec_lid_logo_hook: Option<EcLidLogoHook>,
    last_ec_lightbar_power: Mutex<Option<bool>>,
}

impl UniwillHidTransport {
    pub fn new(hid: Arc<dyn HidDeviceTransport>) -> Self {
        Self {
            hid,
            keyboard_io_lock: Mutex::new(()),
            cached_colors: Mutex::new(Vec::new()),
            cached_keyboard_brightness: Mutex::new(None),
            cached_keyboard_dev: Mutex::new(None),
            cached_lightbar_dev: Mutex::new(None),
            cached_lightbar_brightness: Mutex::new(None),
            lightbar_7001_powered: Mutex::new(false),
            cached_ite7000_zones: Mutex::new(None),
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

    /// Scales logical keyboard brightness (`0..=255`) to ITE `048D:600B` / `048D:CE00` / `048D:7000`
    /// hardware range (`0..=50`).
    ///
    /// Non-zero inputs (`1..=255`) always map to at least `1` so dim settings never power off LEDs.
    pub fn scale_keyboard_brightness(brightness: u32) -> u8 {
        if brightness == 0 {
            return 0;
        }
        let max_hw = u32::from(ITE_KEYBOARD_MAX_HW_BRIGHTNESS);
        ((brightness.min(255) * max_hw + 127) / 255).clamp(1, max_hw) as u8
    }

    /// Scales logical brightness (`0..=255`) to ITE `048D:7001` hardware range (`0..=100`).
    pub fn scale_lightbar_7001_brightness(brightness: u32) -> u8 {
        if brightness == 0 {
            return 0;
        }
        (((brightness.min(255) * 100) + 127) / 255).clamp(1, 100) as u8
    }

    /// Classifies an enumerated ITE keyboard HID device into per-key (126 LEDs) vs 4-zone (`0xFF12`).
    pub fn classify_keyboard_device(&self, dev: &HidDeviceInfo) -> IteKeyboardKind {
        if dev.product_id == ITE_KEYBOARD_PRODUCT_ID_CE00 {
            let usage_page = self.hid.device_usage_page(&dev.path);
            let version = self.hid.device_version_number(&dev.path);
            let output_len = self.hid.device_output_report_byte_length(&dev.path);
            if version == Some(0x0003)
                || output_len == Some(65)
                || usage_page == Some(ITE_USAGE_PAGE_PER_KEY)
            {
                return IteKeyboardKind::PerKey126;
            }
            return IteKeyboardKind::FourZone;
        }
        IteKeyboardKind::PerKey126
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

    fn ensure_7001_zone2_power_on(&self, dev: &HidDeviceInfo) -> Result<(), LightingError> {
        let already_on = *self
            .lightbar_7001_powered
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !already_on {
            let pwr_on = [0x1A, 0x02, 0x01, 0x04, 0x00, 0x00, 0x00, 0x01];
            self.hid.send_feature_report(&dev.path, &pwr_on)?;
            *self
                .lightbar_7001_powered
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = true;
        }
        Ok(())
    }

    fn is_vendor_lighting_interface(&self, dev: &HidDeviceInfo) -> bool {
        if let Some(usage_page) = self.hid.device_usage_page(&dev.path) {
            return usage_page_priority(usage_page) > 0;
        }
        if let Some(interface_number) = self.hid.device_interface_number(&dev.path) {
            return interface_number == 1;
        }
        !self.hid.requires_explicit_interface_metadata()
    }

    fn find_device(&self, product_ids: &[u16]) -> Result<Option<HidDeviceInfo>, LightingError> {
        let best = self
            .hid
            .enumerate_devices()?
            .into_iter()
            .enumerate()
            .filter(|(_, d)| {
                d.vendor_id == ITE_VENDOR_ID
                    && product_ids.contains(&d.product_id)
                    && self.is_vendor_lighting_interface(d)
            })
            .max_by_key(|(idx, d)| {
                let up_prio = self
                    .hid
                    .device_usage_page(&d.path)
                    .map(usage_page_priority)
                    .unwrap_or(0);
                let prefer_usage_2 = u8::from(
                    d.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE
                        && self.hid.device_usage(&d.path) == Some(0x0002),
                );
                let prefer_iface_1 = u8::from(self.hid.device_interface_number(&d.path) == Some(1));
                (up_prio, prefer_usage_2, prefer_iface_1, Reverse(*idx))
            })
            .map(|(_, d)| d);
        Ok(best)
    }

    fn find_keyboard_cached(&self) -> Result<Option<HidDeviceInfo>, LightingError> {
        if let Some(dev) = self
            .cached_keyboard_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(Some(dev));
        }
        let dev = self.find_device(&ITE_KEYBOARD_PRODUCT_IDS)?;
        *self
            .cached_keyboard_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dev.clone();
        Ok(dev)
    }

    fn invalidate_keyboard_cache(&self) {
        *self
            .cached_keyboard_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn find_lightbar_cached(&self) -> Result<Option<HidDeviceInfo>, LightingError> {
        if let Some(dev) = self
            .cached_lightbar_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(Some(dev));
        }
        let dev = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)?;
        *self
            .cached_lightbar_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dev.clone();
        Ok(dev)
    }

    fn invalidate_lightbar_cache(&self) {
        *self
            .cached_lightbar_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .last_ec_lightbar_power
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .lightbar_7001_powered
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = false;
        *self
            .cached_ite7000_zones
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn probe_ite7000_topology(&self, dev: &HidDeviceInfo) -> Vec<Ite7000Zone> {
        let probe = [0xA2, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        if self.hid.send_feature_report(&dev.path, &probe).is_err() {
            return default_ite7000_topology();
        }
        match self.hid.get_feature_report(&dev.path, 8) {
            Ok(resp) if resp.len() >= 2 && resp[0] == 0xA2 => {
                decode_ite7000_topology(resp[1]).unwrap_or_else(default_ite7000_topology)
            }
            _ => default_ite7000_topology(),
        }
    }

    fn ite7000_zones_for(&self, dev: &HidDeviceInfo) -> Vec<Ite7000Zone> {
        if let Some(zones) = self
            .cached_ite7000_zones
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return zones;
        }
        let zones = self.probe_ite7000_topology(dev);
        *self
            .cached_ite7000_zones
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(zones.clone());
        zones
    }

    fn ite7000_front_zones(&self, dev: &HidDeviceInfo) -> Vec<Ite7000Zone> {
        let front: Vec<Ite7000Zone> = self
            .ite7000_zones_for(dev)
            .into_iter()
            .filter(|&z| z != Ite7000Zone::Logo)
            .collect();
        if front.is_empty() {
            vec![Ite7000Zone::Hinge]
        } else {
            front
        }
    }

    fn with_lightbar_retry<T, F>(&self, mut op: F) -> Result<T, LightingError>
    where
        F: FnMut(&HidDeviceInfo) -> Result<T, LightingError>,
    {
        let was_cached = self
            .cached_lightbar_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let Some(dev) = self.find_lightbar_cached()? else {
            return Err(LightingError::Unavailable);
        };
        match op(&dev) {
            Ok(val) => Ok(val),
            Err(first_err) => {
                if matches!(
                    first_err,
                    LightingError::InvalidConfig(_) | LightingError::Unavailable
                ) {
                    return Err(first_err);
                }
                let transient = is_transient_io_error(&first_err);
                self.invalidate_lightbar_cache();
                if !was_cached && !transient {
                    return Err(first_err);
                }
                if transient {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let Some(fresh) = self.find_lightbar_cached()? else {
                    return Err(first_err);
                };
                let retry_res = op(&fresh);
                if retry_res.is_err() {
                    self.invalidate_lightbar_cache();
                }
                retry_res
            }
        }
    }

    fn with_keyboard_retry<T, F>(&self, mut op: F) -> Result<T, LightingError>
    where
        F: FnMut(&HidDeviceInfo) -> Result<T, LightingError>,
    {
        let was_cached = self
            .cached_keyboard_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let Some(dev) = self.find_keyboard_cached()? else {
            return Err(LightingError::Unavailable);
        };
        match op(&dev) {
            Ok(val) => Ok(val),
            Err(first_err) => {
                if matches!(
                    first_err,
                    LightingError::InvalidConfig(_) | LightingError::Unavailable
                ) {
                    return Err(first_err);
                }
                let transient = is_transient_io_error(&first_err);
                self.invalidate_keyboard_cache();
                if !was_cached && !transient {
                    return Err(first_err);
                }
                if transient {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let Some(fresh) = self.find_keyboard_cached()? else {
                    return Err(first_err);
                };
                let retry_res = op(&fresh);
                if retry_res.is_err() {
                    self.invalidate_keyboard_cache();
                }
                retry_res
            }
        }
    }

    fn send_lid_logo_color_for(
        &self,
        dev: &HidDeviceInfo,
        rgb: RgbColor,
    ) -> Result<(), LightingError> {
        if dev.product_id != ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
            return Err(LightingError::Unavailable);
        }
        if !self.ite7000_zones_for(dev).contains(&Ite7000Zone::Logo) {
            return Err(LightingError::Unavailable);
        }
        if rgb != RgbColor::BLACK {
            self.ensure_lightbar_ec_power_on()?;
        }
        let packet = [
            0x14,
            Ite7000Zone::Logo.color_flag(),
            0x01,
            rgb.r,
            rgb.g,
            rgb.b,
            0x00,
            0x00,
        ];
        self.hid.send_feature_report(&dev.path, &packet)
    }

    /// Returns `true` when a lid logo LED is supported (`048D:7000` with `Logo` topology or `048D:7001` + EC).
    pub fn has_lid_logo(&self) -> Result<bool, LightingError> {
        let Some(dev) = self.find_lightbar_cached()? else {
            return Ok(false);
        };
        if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
            return Ok(self.ite7000_zones_for(&dev).contains(&Ite7000Zone::Logo));
        }
        Ok(dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE && self.ec_lid_logo_hook.is_some())
    }

    /// Writes the lid logo LED (`048D:7001` HID Zone 3 + EC persistence, or `048D:7000` `Logo` zone).
    pub fn write_lid_logo(
        &self,
        enabled: bool,
        rgb: RgbColor,
        brightness: u8,
    ) -> Result<(), LightingError> {
        self.with_lightbar_retry(|dev| {
            if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
                if !self.ite7000_zones_for(dev).contains(&Ite7000Zone::Logo) {
                    return Err(LightingError::Unavailable);
                }
                if enabled && brightness > 0 {
                    self.ensure_lightbar_ec_power_on()?;
                    let level = Self::scale_keyboard_brightness(u32::from(brightness));
                    let set_col = [
                        0x14,
                        Ite7000Zone::Logo.color_flag(),
                        0x01,
                        rgb.r,
                        rgb.g,
                        rgb.b,
                        0x00,
                        0x00,
                    ];
                    let apply = [
                        0x08,
                        Ite7000Zone::Logo.effect_ctrl(),
                        0x01,
                        0x01,
                        level,
                        0x01,
                        0x00,
                        0x00,
                    ];
                    self.hid.send_feature_report(&dev.path, &set_col)?;
                    self.hid.send_feature_report(&dev.path, &apply)?;
                } else {
                    let stop_fx = [0x08, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                    let off_logo = [
                        0x08,
                        0x01,
                        Ite7000Zone::Logo.color_flag(),
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                        0x00,
                    ];
                    self.hid.send_feature_report(&dev.path, &stop_fx)?;
                    self.hid.send_feature_report(&dev.path, &off_logo)?;
                }
                return Ok(());
            }
            if self.ec_lid_logo_hook.is_none() {
                return Err(LightingError::Unavailable);
            }
            if enabled && brightness > 0 {
                let lb_level = Self::scale_lightbar_7001_brightness(u32::from(brightness));
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
                // `0x08 0x05` extinguishes Zone 2 as a side effect; force Zone 2 power_on next write.
                *self
                    .lightbar_7001_powered
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = false;
                if let Some(ref hook) = self.ec_lid_logo_hook {
                    hook(false, RgbColor::BLACK, 0)?;
                }
            }
            Ok(())
        })
    }

    /// Writes individual per-zone RGB colors to Lightbar zones (`048D:7001` Zone 2 or `048D:7000` topology).
    pub fn write_lightbar_zones(&self, zone_colors: &[RgbColor]) -> Result<(), LightingError> {
        self.with_lightbar_retry(|dev| {
            if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE {
                let max_zones = LIGHTBAR_FRONT_ZONES;
                if zone_colors.is_empty() || zone_colors.len() > max_zones {
                    return Err(LightingError::InvalidConfig(format!(
                        "Lightbar zone count {} out of range 1..={max_zones}",
                        zone_colors.len()
                    )));
                }
                if zone_colors.iter().any(|&c| c != RgbColor::BLACK) {
                    self.ensure_lightbar_ec_power_on()?;
                }
                self.ensure_7001_zone2_power_on(dev)?;
                for (idx, rgb) in zone_colors.iter().enumerate() {
                    let slot = (idx + 1) as u8;
                    let packet = [0x14, 0x02, slot, rgb.r, rgb.g, rgb.b, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &packet)?;
                }
                let lb_level = self
                    .cached_lightbar_brightness
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .map(Self::scale_lightbar_7001_brightness)
                    .unwrap_or(100);
                if lb_level > 0 {
                    let apply = [0x08, 0x22, 0x01, 0x01, lb_level, 0x01, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &apply)?;
                }
                return Ok(());
            }

            let topology = self.ite7000_zones_for(dev);
            let max_zones = topology.len();
            if zone_colors.is_empty() || zone_colors.len() > max_zones {
                return Err(LightingError::InvalidConfig(format!(
                    "Lightbar zone count {} out of range 1..={max_zones}",
                    zone_colors.len()
                )));
            }
            if zone_colors.iter().any(|&c| c != RgbColor::BLACK) {
                self.ensure_lightbar_ec_power_on()?;
            }
            for (zone, rgb) in topology.iter().zip(zone_colors.iter()) {
                let packet = [
                    0x14,
                    zone.color_flag(),
                    0x01,
                    rgb.r,
                    rgb.g,
                    rgb.b,
                    0x00,
                    0x00,
                ];
                self.hid.send_feature_report(&dev.path, &packet)?;
            }
            Ok(())
        })
    }

    /// Writes the lid logo LED color on ITE `048D:7000` controllers with `Logo` topology.
    ///
    /// Returns [`LightingError::Unavailable`] if no Lightbar controller is present, if the
    /// attached controller is `048D:7001`, or if `048D:7000` does not report a `Logo` zone.
    pub fn write_lid_logo_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        self.with_lightbar_retry(|dev| self.send_lid_logo_color_for(dev, rgb))
    }
}

impl LightingTransport for UniwillHidTransport {
    fn detect_keyboard(&self) -> Result<Option<KeyboardCapabilities>, LightingError> {
        self.invalidate_keyboard_cache();
        let dev = self.find_device(&ITE_KEYBOARD_PRODUCT_IDS)?;
        *self
            .cached_keyboard_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dev.clone();
        let Some(dev) = dev else {
            return Ok(None);
        };
        let zones = match self.classify_keyboard_device(&dev) {
            IteKeyboardKind::PerKey126 => 126,
            IteKeyboardKind::FourZone => 4,
        };
        Ok(Some(KeyboardCapabilities {
            zones,
            max_brightness: 255,
            max_red: 255,
            max_green: 255,
            max_blue: 255,
            led_ids: (0..zones as u32).collect(),
        }))
    }

    fn set_keyboard_buffer_input(&self, _enabled: bool) -> Result<(), LightingError> {
        // Sysfs `buffer_input` is only needed to batch per-LED sysfs writes in `ite_8291.ko`.
        // Direct HID already uploads all rows/zones atomically in `write_keyboard_zones`; sending
        // command `0x09` (`SET_BRIGHTNESS`) here would overwrite hardware brightness to 0/1.
        let Some(_dev) = self.find_keyboard_cached()? else {
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
        // ITE 048D:600B / 048D:CE00 hardware brightness register accepts 0..=50.
        let level = Self::scale_keyboard_brightness(brightness);
        self.with_keyboard_retry(|dev| {
            let packet = if level == 0 {
                [0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
            } else {
                match self.classify_keyboard_device(dev) {
                    IteKeyboardKind::PerKey126 => [0x08, 0x02, 0x33, 0x00, level, 0x00, 0x00, 0x00],
                    IteKeyboardKind::FourZone => [0x08, 0x02, 0x01, 0x00, level, 0x08, 0x00, 0x00],
                }
            };
            self.hid.send_feature_report(&dev.path, &packet)
        })?;
        *self
            .cached_keyboard_brightness
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(brightness.min(255));
        Ok(())
    }

    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError> {
        let _io_guard = self
            .keyboard_io_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if colors.is_empty() {
            if self.find_keyboard_cached()?.is_none() {
                return Err(LightingError::Unavailable);
            }
            *self.cached_colors.lock().unwrap_or_else(|e| e.into_inner()) = Vec::new();
            return Ok(());
        }
        let mut merged = self
            .cached_colors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if merged.len() < colors.len() {
            merged.resize(colors.len(), RgbColor::BLACK);
        }
        merged[..colors.len()].copy_from_slice(colors);

        let cached_b = *self
            .cached_keyboard_brightness
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let level = match cached_b {
            Some(b) => Self::scale_keyboard_brightness(b),
            None => ITE_KEYBOARD_MAX_HW_BRIGHTNESS,
        };

        self.with_keyboard_retry(|dev| match self.classify_keyboard_device(dev) {
            IteKeyboardKind::FourZone => {
                for (idx, c) in merged.iter().take(4).enumerate() {
                    let pkt = [0x14, 0x00, (idx + 1) as u8, c.r, c.g, c.b, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &pkt)?;
                }
                if level > 0 {
                    let apply = [0x08, 0x02, 0x01, 0x00, level, 0x08, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &apply)?;
                }
                Ok(())
            }
            IteKeyboardKind::PerKey126 => {
                // ITE 8291r3 per-key matrix protocol: 6 rows x 21 columns.
                // Each row is selected via 8-byte feature report `SET_ROW_INDEX` (`0x16 0x00 row ...`)
                // followed by a 64-byte planar output report:
                //   buf[0]       = 0x00 (pad)
                //   buf[1..22]   = Blue  (21 cols)
                //   buf[22..43]  = Green (21 cols)
                //   buf[43..64]  = Red   (21 cols)
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
                if level > 0 {
                    let commit_user = [0x08, 0x02, 0x33, 0x00, level, 0x00, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &commit_user)?;
                }
                Ok(())
            }
        })?;
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
        self.invalidate_lightbar_cache();
        let dev = self.find_device(&ITE_LIGHTBAR_PRODUCT_IDS)?;
        *self
            .cached_lightbar_dev
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dev.clone();
        let Some(dev) = dev else {
            return Ok(None);
        };
        let zones = if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_5ZONE {
            self.ite7000_zones_for(&dev).len()
        } else {
            LIGHTBAR_FRONT_ZONES
        };
        Ok(Some(LightbarCapabilities {
            zones,
            max_brightness: 255,
        }))
    }

    fn write_lightbar_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        self.with_lightbar_retry(|dev| {
            if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE {
                let lb_level = Self::scale_lightbar_7001_brightness(brightness);
                if lb_level > 0 {
                    self.ensure_lightbar_ec_power_on()?;
                    self.ensure_7001_zone2_power_on(dev)?;
                    let apply = [0x08, 0x22, 0x01, 0x01, lb_level, 0x01, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &apply)?;
                } else {
                    // Full 4-packet power-off sequence for 048D:7001 Zone 2.
                    let reset = [0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00];
                    let stop_fx = [0x08, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                    let clear_z2 = [0x08, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00];
                    let pwr_off = [0x1A, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
                    self.hid.send_feature_report(&dev.path, &reset)?;
                    self.hid.send_feature_report(&dev.path, &stop_fx)?;
                    self.hid.send_feature_report(&dev.path, &clear_z2)?;
                    self.hid.send_feature_report(&dev.path, &pwr_off)?;
                    *self
                        .lightbar_7001_powered
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = false;
                    self.set_lightbar_ec_power(false)?;
                }
            } else {
                let level = Self::scale_keyboard_brightness(brightness);
                let front_zones = self.ite7000_front_zones(dev);
                if level > 0 {
                    self.ensure_lightbar_ec_power_on()?;
                    for zone in front_zones {
                        let pkt = [
                            0x08,
                            zone.effect_ctrl(),
                            0x01,
                            0x01,
                            level,
                            0x01,
                            0x00,
                            0x00,
                        ];
                        self.hid.send_feature_report(&dev.path, &pkt)?;
                    }
                } else {
                    for zone in front_zones {
                        let pkt = [0x08, 0x01, zone.color_flag(), 0x00, 0x00, 0x00, 0x00, 0x00];
                        self.hid.send_feature_report(&dev.path, &pkt)?;
                    }
                    self.set_lightbar_ec_power(false)?;
                }
            }
            Ok(())
        })?;
        *self
            .cached_lightbar_brightness
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(brightness.min(255));
        Ok(())
    }

    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        self.with_lightbar_retry(|dev| {
            if rgb != RgbColor::BLACK {
                self.ensure_lightbar_ec_power_on()?;
            }
            if dev.product_id == ITE_LIGHTBAR_PRODUCT_ID_4ZONE {
                self.ensure_7001_zone2_power_on(dev)?;
                let color_pkt = [0x14, 0x02, 0x01, rgb.r, rgb.g, rgb.b, 0x00, 0x00];
                self.hid.send_feature_report(&dev.path, &color_pkt)?;
                let lb_level = self
                    .cached_lightbar_brightness
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .map(Self::scale_lightbar_7001_brightness)
                    .unwrap_or(100);
                if lb_level > 0 {
                    let apply = [0x08, 0x22, 0x01, 0x01, lb_level, 0x01, 0x00, 0x00];
                    self.hid.send_feature_report(&dev.path, &apply)?;
                }
            } else {
                for zone in self.ite7000_front_zones(dev) {
                    let packet = [
                        0x14,
                        zone.color_flag(),
                        0x01,
                        rgb.r,
                        rgb.g,
                        rgb.b,
                        0x00,
                        0x00,
                    ];
                    self.hid.send_feature_report(&dev.path, &packet)?;
                }
            }
            Ok(())
        })
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
