//! Pluggable hardware transport seam (`M6.7` / `ADR-05`) for keyboard & lightbar LEDs.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use ucc_hardware::{RealSysfs, Sysfs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardCapabilities {
    pub zones: usize,
    pub max_brightness: u32,
    pub max_red: u8,
    pub max_green: u8,
    pub max_blue: u8,
    pub led_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightbarCapabilities {
    pub zones: usize,
    pub max_brightness: u32,
}

#[derive(Debug, Error)]
pub enum LightingError {
    #[error("Lighting I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lighting hardware unavailable")]
    Unavailable,
    #[error("Invalid lighting configuration: {0}")]
    InvalidConfig(String),
    #[error("Lighting write failed: {0}")]
    WriteFailed(String),
}

/// Abstract lighting transport (`M6.7`) decoupling controller logic from sysfs vs `hidraw`.
pub trait LightingTransport: Send + Sync {
    fn detect_keyboard(&self) -> Result<Option<KeyboardCapabilities>, LightingError>;
    fn set_keyboard_buffer_input(&self, enabled: bool) -> Result<(), LightingError>;
    fn write_keyboard_brightness(&self, brightness: u32) -> Result<(), LightingError>;
    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError>;
    fn read_keyboard_colors(&self) -> Result<Vec<RgbColor>, LightingError>;

    fn detect_lightbar(&self) -> Result<Option<LightbarCapabilities>, LightingError>;
    fn write_lightbar_brightness(&self, brightness: u32) -> Result<(), LightingError>;
    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError>;
}

#[derive(Debug, Default)]
struct DiscoveredSysfsPaths {
    kbd_led_paths: Vec<PathBuf>,
    kbd_buffer_input: Option<PathBuf>,
    kbd_is_rgb: bool,
    lightbar_path: Option<PathBuf>,
    lightbar_zones: usize,
}

/// Sysfs LED-class transport implementing C++ `KeyboardBacklightController` + `LightbarController` discovery.
pub struct SysfsLedTransport {
    sysfs: Arc<dyn Sysfs>,
    root: PathBuf,
    cache: Mutex<DiscoveredSysfsPaths>,
}

impl Default for SysfsLedTransport {
    fn default() -> Self {
        Self::new(Arc::new(RealSysfs), "/")
    }
}

impl SysfsLedTransport {
    pub fn new(sysfs: Arc<dyn Sysfs>, root: impl AsRef<Path>) -> Self {
        Self {
            sysfs,
            root: root.as_ref().to_path_buf(),
            cache: Mutex::new(DiscoveredSysfsPaths::default()),
        }
    }

    fn path_under_root(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }

    fn parse_zone_id(name: &str, fallback_idx: usize) -> u32 {
        if name == "rgb:kbd_backlight" {
            return 0;
        }
        if let Some(rest) = name.strip_prefix("rgb:kbd_backlight_") {
            if let Ok(num) = rest.parse::<u32>() {
                return num;
            }
        }
        fallback_idx as u32
    }

    fn read_trimmed(&self, path: &Path) -> Option<String> {
        self.sysfs.read(path).ok().map(|s| s.trim().to_string())
    }

    fn read_u32(&self, path: &Path) -> Option<u32> {
        self.read_trimmed(path)?.parse::<u32>().ok()
    }

    fn check_white_backlight(&self, rel_base: &str) -> Option<(Vec<PathBuf>, KeyboardCapabilities)> {
        let base = self.path_under_root(rel_base);
        let max_b = self.read_u32(&base.join("max_brightness"))?;
        let caps = KeyboardCapabilities {
            zones: 1,
            max_brightness: max_b,
            max_red: 0,
            max_green: 0,
            max_blue: 0,
            led_ids: vec![0],
        };
        Some((vec![base], caps))
    }

    fn find_per_key_rgb_leds(&self) -> Vec<(PathBuf, u32)> {
        let driver_dirs = [
            "sys/bus/hid/drivers/tuxedo-keyboard-ite",
            "sys/bus/hid/drivers/ite_829x",
            "sys/bus/hid/drivers/ite_8291",
            "sys/bus/platform/drivers/tuxedo_nb04_kbd_backlight",
        ];

        for rel_driver in driver_dirs {
            let driver_path = self.path_under_root(rel_driver);
            let Ok(entries) = self.sysfs.read_dir(&driver_path) else {
                continue;
            };
            let mut sorted_entries = entries;
            sorted_entries.sort();

            for dev_entry in sorted_entries {
                let leds_path = dev_entry.join("leds");
                let Ok(led_entries) = self.sysfs.read_dir(&leds_path) else {
                    continue;
                };

                let mut found: Vec<(PathBuf, u32)> = Vec::new();
                for (fallback_idx, led_path) in led_entries.into_iter().enumerate() {
                    let Some(name) = led_path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if !name.contains("rgb:kbd_backlight") {
                        continue;
                    }
                    let zone_id = Self::parse_zone_id(name, fallback_idx);
                    found.push((led_path, zone_id));
                }

                if !found.is_empty() {
                    found.sort_by_key(|(_, id)| *id);
                    return found;
                }
            }
        }
        Vec::new()
    }

    fn parse_rgb_triples(text: &str) -> Vec<RgbColor> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.is_empty() || !parts.len().is_multiple_of(3) || parts.len() > 3072 {
            return Vec::new();
        }
        let mut colors = Vec::with_capacity(parts.len() / 3);
        for chunk in parts.as_chunks::<3>().0 {
            let Ok(r) = chunk[0].parse::<i32>() else {
                return Vec::new();
            };
            let Ok(g) = chunk[1].parse::<i32>() else {
                return Vec::new();
            };
            let Ok(b) = chunk[2].parse::<i32>() else {
                return Vec::new();
            };
            if !(0..=255).contains(&r) || !(0..=255).contains(&g) || !(0..=255).contains(&b) {
                return Vec::new();
            }
            colors.push(RgbColor::new(r as u8, g as u8, b as u8));
        }
        colors
    }
}

impl LightingTransport for SysfsLedTransport {
    fn detect_keyboard(&self) -> Result<Option<KeyboardCapabilities>, LightingError> {
        // 1. White-only keyboard backlights
        for rel in [
            "sys/devices/platform/tuxedo_keyboard/leds/white:kbd_backlight",
            "sys/bus/platform/devices/tuxedo_nb05_kbd_backlight/leds/white:kbd_backlight",
        ] {
            if let Some((paths, caps)) = self.check_white_backlight(rel) {
                let mut cache = self.cache.lock().unwrap();
                cache.kbd_led_paths = paths;
                cache.kbd_buffer_input = None;
                cache.kbd_is_rgb = false;
                return Ok(Some(caps));
            }
        }

        // 2. Per-key RGB drivers (HID / NB04)
        let per_key = self.find_per_key_rgb_leds();
        if per_key.len() > 3 {
            let paths: Vec<PathBuf> = per_key.iter().map(|(p, _)| p.clone()).collect();
            let led_ids: Vec<u32> = per_key.iter().map(|(_, id)| *id).collect();
            let max_brightness = self
                .read_u32(&paths[0].join("max_brightness"))
                .unwrap_or(255);
            let buffer_path = paths[0].join("device/controls/buffer_input");
            let has_buf = self.sysfs.exists(&buffer_path);

            let caps = KeyboardCapabilities {
                zones: paths.len(),
                max_brightness,
                max_red: 255,
                max_green: 255,
                max_blue: 255,
                led_ids,
            };
            let mut cache = self.cache.lock().unwrap();
            cache.kbd_led_paths = paths;
            cache.kbd_buffer_input = has_buf.then_some(buffer_path);
            cache.kbd_is_rgb = true;
            return Ok(Some(caps));
        }

        // 3. Fallback 1..3-zone RGB
        let candidates = [
            "sys/devices/platform/tuxedo_keyboard/leds/rgb:kbd_backlight",
            "sys/devices/platform/tuxedo_keyboard/leds/rgb:kbd_backlight_1",
            "sys/devices/platform/tuxedo_keyboard/leds/rgb:kbd_backlight_2",
        ];
        let mut zone_paths = Vec::new();
        let mut led_ids = Vec::new();
        for (idx, rel) in candidates.iter().enumerate() {
            let p = self.path_under_root(rel);
            if self.sysfs.exists(&p.join("max_brightness")) {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                led_ids.push(Self::parse_zone_id(name, idx));
                zone_paths.push(p);
            }
        }

        if zone_paths.is_empty() {
            let mut cache = self.cache.lock().unwrap();
            cache.kbd_led_paths.clear();
            cache.kbd_buffer_input = None;
            cache.kbd_is_rgb = false;
            return Ok(None);
        }

        let max_brightness = self
            .read_u32(&zone_paths[0].join("max_brightness"))
            .unwrap_or(255);
        let buffer_path = zone_paths[0].join("device/controls/buffer_input");
        let has_buf = self.sysfs.exists(&buffer_path);

        let caps = KeyboardCapabilities {
            zones: zone_paths.len(),
            max_brightness,
            max_red: 255,
            max_green: 255,
            max_blue: 255,
            led_ids,
        };
        let mut cache = self.cache.lock().unwrap();
        cache.kbd_led_paths = zone_paths;
        cache.kbd_buffer_input = has_buf.then_some(buffer_path);
        cache.kbd_is_rgb = true;
        Ok(Some(caps))
    }

    fn set_keyboard_buffer_input(&self, enabled: bool) -> Result<(), LightingError> {
        let buffer_path = self.cache.lock().unwrap().kbd_buffer_input.clone();
        if let Some(path) = buffer_path {
            if self.sysfs.exists(&path) {
                self.sysfs.write(&path, if enabled { "1" } else { "0" })?;
            }
        }
        Ok(())
    }

    fn write_keyboard_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        let paths = self.cache.lock().unwrap().kbd_led_paths.clone();
        if paths.is_empty() {
            return Err(LightingError::Unavailable);
        }
        let val = brightness.to_string();
        for p in paths {
            self.sysfs.write(&p.join("brightness"), &val)?;
        }
        Ok(())
    }

    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError> {
        let (paths, is_rgb) = {
            let cache = self.cache.lock().unwrap();
            (cache.kbd_led_paths.clone(), cache.kbd_is_rgb)
        };
        if paths.is_empty() {
            return Err(LightingError::Unavailable);
        }
        if !is_rgb {
            return Ok(());
        }
        for (p, color) in paths.iter().zip(colors.iter()) {
            let mi_path = p.join("multi_intensity");
            if !self.sysfs.exists(&mi_path) {
                return Err(LightingError::Unavailable);
            }
            let payload = format!("{} {} {}", color.r, color.g, color.b);
            self.sysfs.write(&mi_path, &payload)?;
        }
        Ok(())
    }

    fn read_keyboard_colors(&self) -> Result<Vec<RgbColor>, LightingError> {
        let leds_dir = self.path_under_root("sys/class/leds");
        let mut colors = Vec::new();
        if let Ok(mut entries) = self.sysfs.read_dir(&leds_dir) {
            entries.sort();
            for entry in entries {
                let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.starts_with("rgb:kbd_backlight") {
                    continue;
                }
                if let Some(text) = self.read_trimmed(&entry.join("multi_intensity")) {
                    let parsed = Self::parse_rgb_triples(&text);
                    colors.extend(parsed);
                }
            }
        }
        if colors.is_empty() {
            let cached = self.cache.lock().unwrap().kbd_led_paths.clone();
            for p in cached {
                if let Some(text) = self.read_trimmed(&p.join("multi_intensity")) {
                    colors.extend(Self::parse_rgb_triples(&text));
                }
            }
        }
        Ok(colors)
    }

    fn detect_lightbar(&self) -> Result<Option<LightbarCapabilities>, LightingError> {
        let leds_dir = self.path_under_root("sys/class/leds");
        let Ok(mut entries) = self.sysfs.read_dir(&leds_dir) else {
            let mut cache = self.cache.lock().unwrap();
            cache.lightbar_path = None;
            cache.lightbar_zones = 0;
            return Ok(None);
        };
        entries.sort();

        // Prioritize `lightbar` then `rgb:lightbar` matching C++ LightbarController::discover()
        for preferred in ["rgb:lightbar", "lightbar"] {
            if let Some(pos) = entries
                .iter()
                .position(|p| p.file_name().and_then(|n| n.to_str()) == Some(preferred))
            {
                let item = entries.remove(pos);
                entries.insert(0, item);
            }
        }

        for path in entries {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.contains("keyboard")
                || name.contains("kbd")
                || name.contains("input")
                || (!name.contains("lightbar") && !name.contains("front"))
            {
                continue;
            }
            if !self.sysfs.exists(&path.join("brightness")) {
                continue;
            }
            let Some(max_b) = self.read_u32(&path.join("max_brightness")) else {
                continue;
            };
            if max_b == 0 || max_b > 65535 {
                continue;
            }
            let Some(mi_text) = self.read_trimmed(&path.join("multi_intensity")) else {
                continue;
            };
            let triples = Self::parse_rgb_triples(&mi_text);
            if triples.is_empty() {
                continue;
            }
            let zones = triples.len();
            let mut cache = self.cache.lock().unwrap();
            cache.lightbar_path = Some(path);
            cache.lightbar_zones = zones;
            return Ok(Some(LightbarCapabilities {
                zones,
                max_brightness: max_b,
            }));
        }

        let mut cache = self.cache.lock().unwrap();
        cache.lightbar_path = None;
        cache.lightbar_zones = 0;
        Ok(None)
    }

    fn write_lightbar_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        let path = self
            .cache
            .lock()
            .unwrap()
            .lightbar_path
            .clone()
            .ok_or(LightingError::Unavailable)?;
        self.sysfs
            .write(&path.join("brightness"), &brightness.to_string())?;
        Ok(())
    }

    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        let (path, zones) = {
            let cache = self.cache.lock().unwrap();
            (cache.lightbar_path.clone(), cache.lightbar_zones)
        };
        let path = path.ok_or(LightingError::Unavailable)?;
        if zones == 0 {
            return Err(LightingError::Unavailable);
        }
        let mut parts = Vec::with_capacity(zones * 3);
        let triplet = format!("{} {} {}", rgb.r, rgb.g, rgb.b);
        for _ in 0..zones {
            parts.push(triplet.as_str());
        }
        let payload = parts.join(" ");
        self.sysfs.write(&path.join("multi_intensity"), &payload)?;
        Ok(())
    }
}

/// Recorded lighting write operation on [`MockLightingTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightingOp {
    SetBufferInput(bool),
    WriteKeyboardBrightness(u32),
    WriteKeyboardZones(Vec<RgbColor>),
    WriteLightbarBrightness(u32),
    WriteLightbarColor(RgbColor),
}

/// Deterministic in-memory mock [`LightingTransport`] for unit tests.
#[derive(Debug, Default)]
pub struct MockLightingTransport {
    keyboard_caps: Mutex<Option<KeyboardCapabilities>>,
    keyboard_colors: Mutex<Vec<RgbColor>>,
    lightbar_caps: Mutex<Option<LightbarCapabilities>>,
    fail_on_buffer_enable: Mutex<bool>,
    fail_on_buffer_disable: Mutex<bool>,
    fail_on_keyboard_zones: Mutex<bool>,
    fail_on_lightbar_write: Mutex<bool>,
    ops: Arc<Mutex<Vec<LightingOp>>>,
}

impl MockLightingTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_keyboard_caps(&self, caps: Option<KeyboardCapabilities>) {
        *self.keyboard_caps.lock().unwrap() = caps;
    }

    pub fn set_keyboard_colors(&self, colors: Vec<RgbColor>) {
        *self.keyboard_colors.lock().unwrap() = colors;
    }

    pub fn set_lightbar_caps(&self, caps: Option<LightbarCapabilities>) {
        *self.lightbar_caps.lock().unwrap() = caps;
    }

    pub fn set_fail_on_buffer_input(&self, fail_enable: bool, fail_disable: bool) {
        *self.fail_on_buffer_enable.lock().unwrap() = fail_enable;
        *self.fail_on_buffer_disable.lock().unwrap() = fail_disable;
    }

    pub fn set_fail_on_keyboard_zones(&self, fail: bool) {
        *self.fail_on_keyboard_zones.lock().unwrap() = fail;
    }

    pub fn set_fail_on_lightbar_write(&self, fail: bool) {
        *self.fail_on_lightbar_write.lock().unwrap() = fail;
    }

    pub fn ops(&self) -> Vec<LightingOp> {
        self.ops.lock().unwrap().clone()
    }

    pub fn clear_ops(&self) {
        self.ops.lock().unwrap().clear();
    }
}

impl LightingTransport for MockLightingTransport {
    fn detect_keyboard(&self) -> Result<Option<KeyboardCapabilities>, LightingError> {
        Ok(self.keyboard_caps.lock().unwrap().clone())
    }

    fn set_keyboard_buffer_input(&self, enabled: bool) -> Result<(), LightingError> {
        if enabled && *self.fail_on_buffer_enable.lock().unwrap() {
            return Err(LightingError::WriteFailed(
                "Simulated buffer_input enable failure".into(),
            ));
        }
        if !enabled && *self.fail_on_buffer_disable.lock().unwrap() {
            return Err(LightingError::WriteFailed(
                "Simulated buffer_input flush/disable failure".into(),
            ));
        }
        self.ops
            .lock()
            .unwrap()
            .push(LightingOp::SetBufferInput(enabled));
        Ok(())
    }

    fn write_keyboard_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        self.ops
            .lock()
            .unwrap()
            .push(LightingOp::WriteKeyboardBrightness(brightness));
        Ok(())
    }

    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError> {
        if *self.fail_on_keyboard_zones.lock().unwrap() {
            return Err(LightingError::WriteFailed(
                "Simulated keyboard zone write failure".into(),
            ));
        }
        self.ops
            .lock()
            .unwrap()
            .push(LightingOp::WriteKeyboardZones(colors.to_vec()));
        *self.keyboard_colors.lock().unwrap() = colors.to_vec();
        Ok(())
    }

    fn read_keyboard_colors(&self) -> Result<Vec<RgbColor>, LightingError> {
        Ok(self.keyboard_colors.lock().unwrap().clone())
    }

    fn detect_lightbar(&self) -> Result<Option<LightbarCapabilities>, LightingError> {
        Ok(self.lightbar_caps.lock().unwrap().clone())
    }

    fn write_lightbar_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        if *self.fail_on_lightbar_write.lock().unwrap() {
            return Err(LightingError::WriteFailed(
                "Simulated lightbar brightness failure".into(),
            ));
        }
        self.ops
            .lock()
            .unwrap()
            .push(LightingOp::WriteLightbarBrightness(brightness));
        Ok(())
    }

    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        if *self.fail_on_lightbar_write.lock().unwrap() {
            return Err(LightingError::WriteFailed(
                "Simulated lightbar color failure".into(),
            ));
        }
        self.ops
            .lock()
            .unwrap()
            .push(LightingOp::WriteLightbarColor(rgb));
        Ok(())
    }
}
