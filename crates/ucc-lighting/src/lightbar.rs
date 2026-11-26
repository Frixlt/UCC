//! Front lightbar controller (`M6.3`–`M6.6`) ported from `LightbarController.hpp`.

use crate::transport::{LightbarCapabilities, LightingError, LightingTransport, RgbColor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Animation scheduler tick interval matching C++ `LightbarController` (`20 ms` / 50 Hz).
pub const LIGHTBAR_FRAME_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightbarAnimation {
    #[default]
    Pulse,
    Flash,
    Rainbow,
    PoliceStrobe,
    Disco,
}

impl LightbarAnimation {
    pub const ALL: [Self; 5] = [
        Self::Pulse,
        Self::Flash,
        Self::Rainbow,
        Self::PoliceStrobe,
        Self::Disco,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pulse => "Pulse (stays on)",
            Self::Flash => "Flash (stays off)",
            Self::Rainbow => "Rainbow",
            Self::PoliceStrobe => "Police Strobe",
            Self::Disco => "Disco",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Pulse (stays on)" => Some(Self::Pulse),
            "Flash (stays off)" => Some(Self::Flash),
            "Rainbow" => Some(Self::Rainbow),
            "Police Strobe" => Some(Self::PoliceStrobe),
            "Disco" => Some(Self::Disco),
            _ => None,
        }
    }

    pub const fn duration_ms(self) -> u64 {
        match self {
            Self::Rainbow => 3000,
            Self::PoliceStrobe => 1440,
            Self::Disco => 1350,
            Self::Pulse | Self::Flash => 1640,
        }
    }

    pub fn frame(self, elapsed_ms: u64, base: RgbColor) -> RgbColor {
        match self {
            Self::Rainbow => hue_to_rgb(elapsed_ms as f64 / 3000.0),
            Self::PoliceStrobe => match (elapsed_ms / 60) % 4 {
                0 => RgbColor::new(255, 0, 0),
                2 => RgbColor::new(0, 0, 255),
                _ => RgbColor::BLACK,
            },
            Self::Disco => {
                if elapsed_ms % 90 >= 60 {
                    RgbColor::BLACK
                } else {
                    hue_to_rgb(((elapsed_ms / 90 * 137 + 41) % 360) as f64 / 360.0)
                }
            }
            Self::Pulse | Self::Flash => {
                let pulse = (1.0 - elapsed_ms as f64 / 820.0).abs();
                let factor = if self == Self::Flash {
                    1.0 - pulse
                } else {
                    pulse
                }
                .clamp(0.0, 1.0);
                RgbColor::new(
                    (base.r as f64 * factor) as u8,
                    (base.g as f64 * factor) as u8,
                    (base.b as f64 * factor) as u8,
                )
            }
        }
    }
}

fn hue_to_rgb(h: f64) -> RgbColor {
    let v = (h - h.floor()) * 6.0;
    let sector = v as i32;
    let up = ((v - sector as f64) * 255.0) as u8;
    let down = 255u8.saturating_sub(up);
    match sector {
        0 => RgbColor::new(255, up, 0),
        1 => RgbColor::new(down, 255, 0),
        2 => RgbColor::new(0, 255, up),
        3 => RgbColor::new(0, down, 255),
        4 => RgbColor::new(up, 0, 255),
        _ => RgbColor::new(255, 0, down),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightbarPowerMode {
    #[default]
    Ac,
    TypeC,
    Bat,
}

impl LightbarPowerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ac => "ac",
            Self::TypeC => "type_c",
            Self::Bat => "bat",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ac" => Some(Self::Ac),
            "type_c" => Some(Self::TypeC),
            "bat" => Some(Self::Bat),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightbarColorMode {
    #[default]
    Auto,
    Fixed(RgbColor),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightbarPowerProfile {
    pub enabled: bool,
    pub brightness_val: u8,
    pub mode: LightbarAnimation,
    pub color_mode: LightbarColorMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightbarSettings {
    pub managed: bool,
    pub silent_mode: bool,
    pub welcome_anim: bool,
    pub ac: LightbarPowerProfile,
    pub type_c: LightbarPowerProfile,
    pub bat: LightbarPowerProfile,
}

impl Default for LightbarSettings {
    fn default() -> Self {
        Self {
            managed: false,
            silent_mode: false,
            welcome_anim: false,
            ac: LightbarPowerProfile {
                enabled: true,
                brightness_val: 255,
                mode: LightbarAnimation::Pulse,
                color_mode: LightbarColorMode::Auto,
            },
            type_c: LightbarPowerProfile {
                enabled: true,
                brightness_val: 150,
                mode: LightbarAnimation::Pulse,
                color_mode: LightbarColorMode::Auto,
            },
            bat: LightbarPowerProfile {
                enabled: false,
                brightness_val: 50,
                mode: LightbarAnimation::Pulse,
                color_mode: LightbarColorMode::Auto,
            },
        }
    }
}

impl LightbarSettings {
    pub fn profile(&self, mode: LightbarPowerMode) -> &LightbarPowerProfile {
        match mode {
            LightbarPowerMode::Ac => &self.ac,
            LightbarPowerMode::TypeC => &self.type_c,
            LightbarPowerMode::Bat => &self.bat,
        }
    }

    fn profile_mut(&mut self, mode: LightbarPowerMode) -> &mut LightbarPowerProfile {
        match mode {
            LightbarPowerMode::Ac => &mut self.ac,
            LightbarPowerMode::TypeC => &mut self.type_c,
            LightbarPowerMode::Bat => &mut self.bat,
        }
    }

    pub fn normalize_json(input: &Value) -> Result<Self, LightingError> {
        let obj = input.as_object().ok_or_else(|| {
            LightingError::InvalidConfig("Lightbar settings must be an object".into())
        })?;
        let mut result = Self::default();

        if let Some(v) = obj.get("managed") {
            result.managed = v
                .as_bool()
                .ok_or_else(|| LightingError::InvalidConfig("Invalid lightbar flag".into()))?;
        }
        if let Some(v) = obj.get("silent_mode") {
            result.silent_mode = v
                .as_bool()
                .ok_or_else(|| LightingError::InvalidConfig("Invalid lightbar flag".into()))?;
        }
        if let Some(v) = obj.get("welcome_anim") {
            result.welcome_anim = v
                .as_bool()
                .ok_or_else(|| LightingError::InvalidConfig("Invalid lightbar flag".into()))?;
        }

        if let Some(ps) = obj.get("power_settings") {
            let ps_obj = ps.as_object().ok_or_else(|| {
                LightingError::InvalidConfig("Invalid lightbar power settings".into())
            })?;
            for (mode_key, prof_val) in ps_obj {
                let power_mode = LightbarPowerMode::parse(mode_key).ok_or_else(|| {
                    LightingError::InvalidConfig("Invalid lightbar power mode".into())
                })?;
                let prof_obj = prof_val.as_object().ok_or_else(|| {
                    LightingError::InvalidConfig("Invalid lightbar power mode".into())
                })?;
                let target = result.profile_mut(power_mode);

                if let Some(en) = prof_obj.get("enabled") {
                    target.enabled = en.as_bool().ok_or_else(|| {
                        LightingError::InvalidConfig("Invalid enabled flag".into())
                    })?;
                }
                if let Some(br) = prof_obj.get("brightness_val") {
                    let b = br.as_i64().filter(|_| !br.is_boolean()).ok_or_else(|| {
                        LightingError::InvalidConfig("Brightness must be 0..255".into())
                    })?;
                    if !(0..=255).contains(&b) {
                        return Err(LightingError::InvalidConfig(
                            "Brightness must be 0..255".into(),
                        ));
                    }
                    target.brightness_val = b as u8;
                }
                if let Some(m) = prof_obj.get("mode") {
                    let s = m.as_str().ok_or_else(|| {
                        LightingError::InvalidConfig("Invalid lightbar animation".into())
                    })?;
                    target.mode = LightbarAnimation::parse(s).ok_or_else(|| {
                        LightingError::InvalidConfig("Invalid lightbar animation".into())
                    })?;
                }
                if let Some(cm) = prof_obj.get("color_mode") {
                    if cm.as_str() == Some("Auto") {
                        target.color_mode = LightbarColorMode::Auto;
                    } else {
                        let arr = cm.as_array().ok_or_else(|| {
                            LightingError::InvalidConfig("Invalid lightbar color".into())
                        })?;
                        if arr.len() != 3 {
                            return Err(LightingError::InvalidConfig(
                                "Invalid lightbar color".into(),
                            ));
                        }
                        let mut channels = [0u8; 3];
                        for (idx, ch) in arr.iter().enumerate() {
                            let val = ch.as_i64().filter(|_| !ch.is_boolean()).ok_or_else(|| {
                                LightingError::InvalidConfig("Invalid RGB channel".into())
                            })?;
                            if !(0..=255).contains(&val) {
                                return Err(LightingError::InvalidConfig(
                                    "Invalid RGB channel".into(),
                                ));
                            }
                            channels[idx] = val as u8;
                        }
                        target.color_mode = LightbarColorMode::Fixed(RgbColor::new(
                            channels[0],
                            channels[1],
                            channels[2],
                        ));
                    }
                }
            }
        }

        Ok(result)
    }

    pub fn to_json(&self) -> Value {
        let profile_to_json = |p: &LightbarPowerProfile| -> Value {
            let color_mode = match p.color_mode {
                LightbarColorMode::Auto => json!("Auto"),
                LightbarColorMode::Fixed(c) => json!([c.r, c.g, c.b]),
            };
            json!({
                "enabled": p.enabled,
                "brightness_val": p.brightness_val,
                "mode": p.mode.as_str(),
                "color_mode": color_mode,
            })
        };

        json!({
            "managed": self.managed,
            "silent_mode": self.silent_mode,
            "welcome_anim": self.welcome_anim,
            "power_settings": {
                "ac": profile_to_json(&self.ac),
                "type_c": profile_to_json(&self.type_c),
                "bat": profile_to_json(&self.bat),
            }
        })
    }
}

/// Front lightbar controller supporting animations, `Flash (stays off)`, `Auto` majority color,
/// `Silent` mode, and deterministic animation stepping.
pub struct LightbarController {
    transport: Arc<dyn LightingTransport>,
    config: LightbarSettings,
    caps: Option<LightbarCapabilities>,
    power_mode: LightbarPowerMode,
    active_animation: LightbarAnimation,
    base_color: RgbColor,
    animating: bool,
    anim_started: Option<Instant>,
    flash_off: bool,
    welcome_done: bool,
    last_ok: bool,
    last_error: String,
}

impl LightbarController {
    pub fn new(transport: Arc<dyn LightingTransport>) -> Self {
        let mut ctrl = Self {
            transport,
            config: LightbarSettings::default(),
            caps: None,
            power_mode: LightbarPowerMode::Ac,
            active_animation: LightbarAnimation::Pulse,
            base_color: RgbColor::WHITE,
            animating: false,
            anim_started: None,
            flash_off: false,
            welcome_done: false,
            last_ok: true,
            last_error: String::new(),
        };
        ctrl.discover();
        ctrl
    }

    pub fn discover(&mut self) {
        match self.transport.detect_lightbar() {
            Ok(caps) => self.caps = caps,
            Err(e) => {
                self.caps = None;
                self.last_ok = false;
                self.last_error = e.to_string();
            }
        }
    }

    /// Validates, stores, and applies lightbar configuration (`W1-07`).
    ///
    /// Returns `Err` if applying hardware state fails (`last_ok` becomes `false` with `last_error`),
    /// while preserving normalized configuration for retry via [`Self::apply_current`].
    pub fn configure(&mut self, input: &Value) -> Result<(), LightingError> {
        let normalized = LightbarSettings::normalize_json(input)?;
        let was_managed = self.config.managed;
        self.animating = false;
        self.anim_started = None;
        self.flash_off = false;
        self.config = normalized;
        self.discover();

        if self.config.managed {
            self.apply_idle_result()?;
            if !self.welcome_done {
                self.welcome_done = true;
                if self.config.welcome_anim {
                    self.trigger_result(true)?;
                }
            }
        } else if was_managed {
            self.write_color_result(RgbColor::BLACK)?;
        }
        Ok(())
    }

    /// Re-applies currently stored configuration (e.g. retry after transient hardware I/O error).
    pub fn apply_current(&mut self) -> Result<(), LightingError> {
        if self.config.managed {
            self.discover();
            self.apply_idle_result()
        } else {
            Ok(())
        }
    }

    /// Handles system resume: refreshes discovery and restores idle state without repeating welcome animation (`W1-13`).
    pub fn on_resume(&mut self) -> Result<(), LightingError> {
        self.animating = false;
        self.anim_started = None;
        self.discover();
        if self.config.managed {
            self.apply_idle_result()
        } else {
            Ok(())
        }
    }

    pub fn shutdown(&mut self) -> Result<(), LightingError> {
        self.animating = false;
        self.anim_started = None;
        if self.config.managed {
            self.config.managed = false;
            return self.write_color_result(RgbColor::BLACK);
        }
        Ok(())
    }

    pub fn skip_welcome(&mut self) {
        self.welcome_done = true;
    }

    pub fn is_animating(&self) -> bool {
        self.animating
    }

    pub fn is_flash_off(&self) -> bool {
        self.flash_off
    }

    pub fn settings(&self) -> &LightbarSettings {
        &self.config
    }

    pub fn settings_json(&self) -> Value {
        self.config.to_json()
    }

    pub fn status_json(&self) -> Value {
        let (zones, max_brightness) = self
            .caps
            .as_ref()
            .map(|c| (c.zones, c.max_brightness))
            .unwrap_or((0, 0));
        json!({
            "available": self.caps.is_some(),
            "managed": self.config.managed,
            "mode": self.power_mode.as_str(),
            "animating": self.animating,
            "zones": zones,
            "maxBrightness": max_brightness,
            "ok": self.last_ok,
            "error": self.last_error,
        })
    }

    pub fn set_power_mode(&mut self, mode: LightbarPowerMode) {
        if self.power_mode == mode {
            return;
        }
        self.animating = false;
        self.anim_started = None;
        self.flash_off = false;
        self.power_mode = mode;
        if self.config.managed {
            self.apply_idle();
        }
    }

    pub fn keyboard_changed(&mut self) {
        if self.config.managed
            && !self.animating
            && self.current_profile().color_mode == LightbarColorMode::Auto
            && !self.flash_off
        {
            self.apply_idle();
        }
    }

    pub fn trigger(&mut self, force: bool) -> bool {
        matches!(self.trigger_result(force), Ok(true))
    }

    pub fn trigger_result(&mut self, force: bool) -> Result<bool, LightingError> {
        let prof = self.current_profile().clone();
        if !self.config.managed
            || !prof.enabled
            || self.animating
            || (!force && self.config.silent_mode)
        {
            return Ok(false);
        }
        self.discover();
        if self.caps.is_none() {
            self.last_ok = false;
            self.last_error = "Lightbar hardware unavailable".into();
            return Err(LightingError::Unavailable);
        }

        self.active_animation = prof.mode;
        self.base_color = self.resolve_color();
        self.last_ok = true;
        self.last_error.clear();

        self.write_brightness_result(prof.brightness_val)?;
        self.animating = true;
        self.anim_started = Some(Instant::now());
        self.flash_off = false;
        self.advance(0);
        if !self.last_ok {
            return Err(LightingError::WriteFailed(self.last_error.clone()));
        }
        Ok(true)
    }

    /// Advances an active animation using the internal start timestamp.
    pub fn tick_now(&mut self) {
        if !self.animating {
            self.anim_started = None;
            return;
        }
        let origin = self.anim_started.get_or_insert_with(Instant::now);
        let elapsed_ms = origin.elapsed().as_millis() as u64;
        self.advance(elapsed_ms);
        if !self.animating {
            self.anim_started = None;
        }
    }

    /// Steps the active animation deterministically by `elapsed_ms`.
    pub fn advance(&mut self, elapsed_ms: u64) {
        if !self.animating {
            self.anim_started = None;
            return;
        }
        if elapsed_ms >= self.active_animation.duration_ms() {
            self.animating = false;
            self.anim_started = None;
            self.flash_off = self.active_animation == LightbarAnimation::Flash;
            let final_color = if self.flash_off {
                RgbColor::BLACK
            } else {
                self.resolve_color()
            };
            let _ = self.write_color(final_color);
            return;
        }

        let frame_rgb = self.active_animation.frame(elapsed_ms, self.base_color);
        if !self.write_color(frame_rgb) {
            self.animating = false;
        }
    }

    /// Resolves either the configured fixed RGB or the majority non-black keyboard LED color.
    pub fn resolve_color(&self) -> RgbColor {
        if let LightbarColorMode::Fixed(rgb) = self.current_profile().color_mode {
            return rgb;
        }
        let Ok(colors) = self.transport.read_keyboard_colors() else {
            return RgbColor::WHITE;
        };
        if colors.is_empty() {
            return RgbColor::WHITE;
        }
        let mut counts: BTreeMap<RgbColor, usize> = BTreeMap::new();
        for c in colors {
            *counts.entry(c).or_insert(0) += 1;
        }
        if counts.len() > 1 {
            counts.remove(&RgbColor::BLACK);
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(rgb, _)| rgb)
            .unwrap_or(RgbColor::WHITE)
    }

    fn current_profile(&self) -> &LightbarPowerProfile {
        self.config.profile(self.power_mode)
    }

    fn apply_idle(&mut self) {
        let _ = self.apply_idle_result();
    }

    fn apply_idle_result(&mut self) -> Result<(), LightingError> {
        let prof = self.current_profile().clone();
        self.flash_off = prof.mode == LightbarAnimation::Flash;
        self.last_ok = true;
        self.last_error.clear();

        if self.caps.is_none() {
            self.discover();
        }
        if self.caps.is_none() {
            self.last_ok = false;
            self.last_error = "Lightbar hardware unavailable".into();
            return Err(LightingError::Unavailable);
        }

        let idle_color = if prof.enabled && !self.flash_off {
            self.resolve_color()
        } else {
            RgbColor::BLACK
        };
        // Clear Flash's idle color before restoring brightness.
        self.write_color_result(idle_color)?;
        self.write_brightness_result(prof.brightness_val)
    }

    fn write_color(&mut self, rgb: RgbColor) -> bool {
        self.write_color_result(rgb).is_ok()
    }

    fn write_color_result(&mut self, rgb: RgbColor) -> Result<(), LightingError> {
        match self.transport.write_lightbar_color(rgb) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.last_ok = false;
                self.last_error = e.to_string();
                Err(e)
            }
        }
    }

    fn write_brightness_result(&mut self, brightness_val: u8) -> Result<(), LightingError> {
        let Some(caps) = self.caps.as_ref() else {
            self.last_ok = false;
            self.last_error = "Lightbar hardware unavailable".into();
            return Err(LightingError::Unavailable);
        };
        let scaled = ((brightness_val as f64) * (caps.max_brightness as f64) / 255.0).round() as u32;
        match self.transport.write_lightbar_brightness(scaled) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.last_ok = false;
                self.last_error = e.to_string();
                Err(e)
            }
        }
    }
}
