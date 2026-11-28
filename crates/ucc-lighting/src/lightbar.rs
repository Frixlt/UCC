// SPDX-License-Identifier: GPL-3.0-or-later

//! Front lightbar controller (`M6.3`–`M6.6`) ported from `LightbarController.hpp`.

use crate::hid_transport::LIGHTBAR_FRONT_ZONES;
use crate::transport::{LightbarCapabilities, LightingError, LightingTransport, RgbColor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Animation scheduler tick interval (`33 ms` / ~30 Hz) for smooth LED fades without USB HID saturation.
pub const LIGHTBAR_FRAME_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightbarAnimation {
    #[default]
    Pulse,
    PulseOff,
    Flash,
    Rainbow,
    PoliceStrobe,
    Disco,
}

impl LightbarAnimation {
    pub const ALL: [Self; 6] = [
        Self::Pulse,
        Self::PulseOff,
        Self::Flash,
        Self::Rainbow,
        Self::PoliceStrobe,
        Self::Disco,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pulse => "Pulse (stays on)",
            Self::PulseOff => "Pulse (stays off)",
            Self::Flash => "Flash (stays off)",
            Self::Rainbow => "Rainbow",
            Self::PoliceStrobe => "Police Strobe",
            Self::Disco => "Disco",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Pulse (stays on)" => Some(Self::Pulse),
            "Pulse (stays off)" => Some(Self::PulseOff),
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
            Self::Pulse | Self::PulseOff | Self::Flash => 1640,
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
                    let hue = (((elapsed_ms / 90) % 360) * 137 + 41) % 360;
                    hue_to_rgb(hue as f64 / 360.0)
                }
            }
            Self::Pulse | Self::PulseOff | Self::Flash => {
                let factor = match self {
                    Self::PulseOff => {
                        let phase = (elapsed_ms as f64 / 1640.0).clamp(0.0, 1.0);
                        0.5 * (1.0 - (std::f64::consts::TAU * phase).cos())
                    }
                    Self::Pulse => {
                        let phase = (elapsed_ms as f64 / 1640.0).clamp(0.0, 1.0);
                        0.5 * (1.0 + (std::f64::consts::TAU * phase).cos())
                    }
                    _ => (1.0 - (1.0 - elapsed_ms as f64 / 820.0).abs()).clamp(0.0, 1.0),
                }
                .clamp(0.0, 1.0);
                RgbColor::new(
                    (base.r as f64 * factor).round() as u8,
                    (base.g as f64 * factor).round() as u8,
                    (base.b as f64 * factor).round() as u8,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LidLogoSettings {
    pub enabled: bool,
    pub brightness: u8,
    pub color: RgbColor,
}

impl Default for LidLogoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            brightness: 255,
            color: RgbColor::WHITE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightbarSettings {
    pub managed: bool,
    pub silent_mode: bool,
    pub welcome_anim: bool,
    pub ac: LightbarPowerProfile,
    pub type_c: LightbarPowerProfile,
    pub bat: LightbarPowerProfile,
    pub lid_logo: Option<LidLogoSettings>,
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
            lid_logo: None,
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
                            let val =
                                ch.as_i64().filter(|_| !ch.is_boolean()).ok_or_else(|| {
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

        if let Some(logo_val) = obj.get("lid_logo") {
            let logo_obj = logo_val
                .as_object()
                .ok_or_else(|| LightingError::InvalidConfig("lid_logo must be an object".into()))?;
            let mut logo = result.lid_logo.unwrap_or_default();
            if let Some(en) = logo_obj.get("enabled") {
                logo.enabled = en.as_bool().ok_or_else(|| {
                    LightingError::InvalidConfig("Invalid lid_logo enabled flag".into())
                })?;
            }
            if let Some(br) = logo_obj.get("brightness") {
                let b = br.as_i64().filter(|_| !br.is_boolean()).ok_or_else(|| {
                    LightingError::InvalidConfig("lid_logo brightness must be 0..255".into())
                })?;
                if !(0..=255).contains(&b) {
                    return Err(LightingError::InvalidConfig(
                        "lid_logo brightness must be 0..255".into(),
                    ));
                }
                logo.brightness = b as u8;
            }
            if let Some(col) = logo_obj.get("color") {
                let arr = col
                    .as_array()
                    .ok_or_else(|| LightingError::InvalidConfig("Invalid lid_logo color".into()))?;
                if arr.len() != 3 {
                    return Err(LightingError::InvalidConfig(
                        "Invalid lid_logo color".into(),
                    ));
                }
                let mut chs = [0u8; 3];
                for (idx, ch) in arr.iter().enumerate() {
                    let val = ch.as_i64().filter(|_| !ch.is_boolean()).ok_or_else(|| {
                        LightingError::InvalidConfig("Invalid lid_logo RGB channel".into())
                    })?;
                    if !(0..=255).contains(&val) {
                        return Err(LightingError::InvalidConfig(
                            "Invalid lid_logo RGB channel".into(),
                        ));
                    }
                    chs[idx] = val as u8;
                }
                logo.color = RgbColor::new(chs[0], chs[1], chs[2]);
            }
            result.lid_logo = Some(logo);
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

        let mut out = json!({
            "managed": self.managed,
            "silent_mode": self.silent_mode,
            "welcome_anim": self.welcome_anim,
            "power_settings": {
                "ac": profile_to_json(&self.ac),
                "type_c": profile_to_json(&self.type_c),
                "bat": profile_to_json(&self.bat),
            }
        });
        if let Some(logo) = self.lid_logo {
            out["lid_logo"] = json!({
                "enabled": logo.enabled,
                "brightness": logo.brightness,
                "color": [logo.color.r, logo.color.g, logo.color.b],
            });
        }
        out
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
    last_written_color: Option<RgbColor>,
    last_written_brightness: Option<u32>,
    consecutive_anim_errors: u8,
    last_ok: bool,
    last_error: String,
}

const MAX_CONSECUTIVE_ANIM_ERRORS: u8 = 5;

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
            last_written_color: None,
            last_written_brightness: None,
            consecutive_anim_errors: 0,
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
    pub fn restore_config_preserving_error(&mut self, prev: LightbarSettings) {
        self.config = prev;
    }

    /// Seeds persisted settings in memory without performing hardware I/O (used during daemon startup).
    pub fn seed_settings(&mut self, input: &Value) {
        if let Ok(normalized) = LightbarSettings::normalize_json(input) {
            self.config = normalized;
        }
    }

    pub fn configure(&mut self, input: &Value) -> Result<(), LightingError> {
        let normalized = LightbarSettings::normalize_json(input)?;
        let was_managed = self.config.managed;
        let had_lid_logo = self.config.lid_logo.is_some();
        self.animating = false;
        self.anim_started = None;
        self.flash_off = false;
        self.last_written_color = None;
        self.last_written_brightness = None;
        self.consecutive_anim_errors = 0;
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
        } else {
            self.last_ok = true;
            self.last_error.clear();
            let has_logo = self.transport.has_lid_logo().unwrap_or(false);
            if was_managed {
                if self.config.lid_logo.is_none() && had_lid_logo && has_logo {
                    let _ = self.transport.write_lid_logo(false, RgbColor::BLACK, 0);
                }
                self.write_color_result(RgbColor::BLACK)?;
            }
            self.apply_unmanaged_lid_logo_result()?;
        }
        Ok(())
    }

    fn apply_unmanaged_lid_logo_result(&mut self) -> Result<(), LightingError> {
        let Some(logo) = self.config.lid_logo else {
            return Ok(());
        };
        if !self.transport.has_lid_logo().unwrap_or(false) {
            return Ok(());
        }
        let on = logo.enabled && logo.brightness > 0;
        let rgb = if on { logo.color } else { RgbColor::BLACK };
        let br = if on { logo.brightness } else { 0 };
        if let Err(e) = self.transport.write_lid_logo(on, rgb, br) {
            self.last_ok = false;
            self.last_error = e.to_string();
            return Err(e);
        }
        self.last_ok = true;
        self.last_error.clear();
        Ok(())
    }

    /// Re-applies currently stored configuration (e.g. retry after transient hardware I/O error).
    pub fn apply_current(&mut self) -> Result<(), LightingError> {
        self.last_written_brightness = None;
        self.last_written_color = None;
        if self.config.managed {
            self.discover();
            self.apply_idle_result()
        } else {
            self.apply_unmanaged_lid_logo_result()
        }
    }

    /// Handles system resume: refreshes discovery and restores idle state without repeating welcome animation (`W1-13`).
    pub fn on_resume(&mut self) -> Result<(), LightingError> {
        self.animating = false;
        self.anim_started = None;
        self.last_written_color = None;
        self.last_written_brightness = None;
        self.consecutive_anim_errors = 0;
        self.discover();
        if self.config.managed {
            self.apply_idle_result()
        } else {
            self.apply_unmanaged_lid_logo_result()
        }
    }

    pub fn shutdown(&mut self) -> Result<(), LightingError> {
        self.animating = false;
        self.anim_started = None;
        self.last_written_color = None;
        self.last_written_brightness = None;
        self.consecutive_anim_errors = 0;
        if self.config.managed {
            self.config.managed = false;
            if self.config.lid_logo.is_some() && self.transport.has_lid_logo().unwrap_or(false) {
                let _ = self.transport.write_lid_logo(false, RgbColor::BLACK, 0);
            }
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
            "hasLidLogo": self.transport.has_lid_logo().unwrap_or(false),
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
        self.consecutive_anim_errors = 0;
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
        if self.caps.is_none() {
            self.discover();
        }
        if self.caps.is_none() {
            self.last_ok = false;
            self.last_error = "Lightbar hardware unavailable".into();
            return Err(LightingError::Unavailable);
        }

        self.active_animation = prof.mode;
        self.base_color = self.resolve_color();
        self.last_ok = true;
        self.last_error.clear();

        self.last_written_color = None;
        self.consecutive_anim_errors = 0;
        self.write_brightness_result(prof.brightness_val)?;
        self.animating = true;
        self.anim_started = Some(Instant::now());
        self.flash_off = false;
        if let Err(err) = self.advance_result(0) {
            self.animating = false;
            self.anim_started = None;
            self.consecutive_anim_errors = 0;
            return Err(err);
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
    ///
    /// Records any hardware write failure into `status_json()["ok"]` / `status_json()["error"]`
    /// without silently swallowing errors.
    pub fn advance(&mut self, elapsed_ms: u64) {
        let _ = self.advance_result(elapsed_ms);
    }

    /// Steps the active animation deterministically by `elapsed_ms`, returning any hardware write error.
    pub fn advance_result(&mut self, elapsed_ms: u64) -> Result<(), LightingError> {
        if !self.animating {
            self.anim_started = None;
            return Ok(());
        }
        if elapsed_ms >= self.active_animation.duration_ms() {
            self.animating = false;
            self.anim_started = None;
            self.consecutive_anim_errors = 0;
            self.flash_off = matches!(
                self.active_animation,
                LightbarAnimation::Flash | LightbarAnimation::PulseOff
            );
            let final_color = if self.flash_off {
                RgbColor::BLACK
            } else {
                self.resolve_color()
            };
            self.write_color_result(final_color)?;
            if let Some(logo) = self.config.lid_logo {
                if self.transport.has_lid_logo().unwrap_or(false) {
                    let shared_zone_logo = self
                        .caps
                        .as_ref()
                        .is_some_and(|c| c.zones > LIGHTBAR_FRONT_ZONES);
                    let logo_res = if logo.enabled && logo.brightness > 0 {
                        self.transport
                            .write_lid_logo(true, logo.color, logo.brightness)
                    } else if shared_zone_logo {
                        self.transport.write_lid_logo(false, RgbColor::BLACK, 0)
                    } else {
                        Ok(())
                    };
                    if let Err(err) = logo_res {
                        self.last_ok = false;
                        self.last_error = err.to_string();
                        return Err(err);
                    }
                }
            }
            self.last_ok = true;
            self.last_error.clear();
            return Ok(());
        }

        let frame_rgb = self.active_animation.frame(elapsed_ms, self.base_color);
        if self.last_written_color == Some(frame_rgb) {
            return Ok(());
        }
        match self.write_color_result(frame_rgb) {
            Ok(()) => {
                self.consecutive_anim_errors = 0;
                self.last_ok = true;
                self.last_error.clear();
                Ok(())
            }
            Err(err) => {
                self.consecutive_anim_errors = self.consecutive_anim_errors.saturating_add(1);
                if self.consecutive_anim_errors >= MAX_CONSECUTIVE_ANIM_ERRORS {
                    self.animating = false;
                    self.anim_started = None;
                }
                Err(err)
            }
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
        self.flash_off = matches!(
            prof.mode,
            LightbarAnimation::Flash | LightbarAnimation::PulseOff
        );
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
        let has_logo = self.transport.has_lid_logo().unwrap_or(false);
        let shared_zone_logo = self
            .caps
            .as_ref()
            .is_some_and(|c| c.zones > LIGHTBAR_FRONT_ZONES);
        // Disable non-shared lid logo BEFORE enabling front lightbar so 048D:7001 stop_fx (0x08 0x05)
        // never extinguishes the front bar.
        if let Some(logo) = self.config.lid_logo {
            if has_logo && !shared_zone_logo && (!logo.enabled || logo.brightness == 0) {
                if let Err(e) = self.transport.write_lid_logo(false, RgbColor::BLACK, 0) {
                    self.last_ok = false;
                    self.last_error = e.to_string();
                    return Err(e);
                }
                // 048D:7001 stop_fx extinguishes the front bar until brightness is re-sent.
                self.last_written_brightness = None;
            }
        }
        // Clear Flash/PulseOff idle color before restoring front lightbar brightness.
        self.write_color_result(idle_color)?;
        self.write_brightness_result(prof.brightness_val)?;
        // Apply enabled lid logo (or disabled shared-zone 048D:7000 zone 5) AFTER front lightbar
        // so 048D:7000 zone 5 is not overwritten by write_lightbar_color.
        if let Some(logo) = self.config.lid_logo {
            if has_logo {
                if logo.enabled && logo.brightness > 0 {
                    if let Err(e) = self
                        .transport
                        .write_lid_logo(true, logo.color, logo.brightness)
                    {
                        self.last_ok = false;
                        self.last_error = e.to_string();
                        return Err(e);
                    }
                } else if shared_zone_logo {
                    if let Err(e) = self.transport.write_lid_logo(false, RgbColor::BLACK, 0) {
                        self.last_ok = false;
                        self.last_error = e.to_string();
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    fn write_color_result(&mut self, rgb: RgbColor) -> Result<(), LightingError> {
        match self.transport.write_lightbar_color(rgb) {
            Ok(()) => {
                self.last_written_color = Some(rgb);
                Ok(())
            }
            Err(e) => {
                self.last_written_color = None;
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
        let scaled =
            ((brightness_val as f64) * (caps.max_brightness as f64) / 255.0).round() as u32;
        if self.last_written_brightness == Some(scaled) {
            return Ok(());
        }
        match self.transport.write_lightbar_brightness(scaled) {
            Ok(()) => {
                self.last_written_brightness = Some(scaled);
                Ok(())
            }
            Err(e) => {
                self.last_written_brightness = None;
                self.last_ok = false;
                self.last_error = e.to_string();
                Err(e)
            }
        }
    }
}
