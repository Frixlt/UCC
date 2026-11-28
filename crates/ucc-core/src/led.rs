use crate::error::ValidationError;
use serde::{Deserialize, Serialize};

/// LED lighting settings for lightbar, keyboard zones, or liquid cooler indicators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedSettings {
    /// Brightness level (0..=255)
    pub brightness: u8,
    /// Red color intensity (0..=255)
    pub red: u8,
    /// Green color intensity (0..=255)
    pub green: u8,
    /// Blue color intensity (0..=255)
    pub blue: u8,
    /// Animation mode or color strategy name
    pub mode: Option<String>,
}

impl Default for LedSettings {
    fn default() -> Self {
        Self {
            brightness: 255,
            red: 255,
            green: 255,
            blue: 255,
            mode: Some("Pulse (stays on)".to_string()),
        }
    }
}

impl LedSettings {
    pub const VALID_ANIMATIONS: [&'static str; 6] = [
        "Pulse (stays on)",
        "Pulse (stays off)",
        "Flash (stays off)",
        "Rainbow",
        "Police Strobe",
        "Disco",
    ];

    /// Single source of truth for all valid LED modes (animations and static/strategy modes).
    pub const VALID_MODES: [&'static str; 12] = [
        "Pulse (stays on)",
        "Pulse (stays off)",
        "Flash (stays off)",
        "Rainbow",
        "Police Strobe",
        "Disco",
        "Static",
        "Auto",
        "Breathe",
        "Colorful",
        "BreatheColor",
        "Temperature",
    ];

    pub fn new(brightness: u8, red: u8, green: u8, blue: u8, mode: Option<String>) -> Self {
        Self {
            brightness,
            red,
            green,
            blue,
            mode,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(ref m) = self.mode {
            if !Self::VALID_MODES.contains(&m.as_str()) {
                return Err(ValidationError::InvalidLedMode(m.clone()));
            }
        }
        Ok(())
    }
}

/// Single zone keyboard backlight color and brightness state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeyboardBacklightState {
    pub brightness: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl KeyboardBacklightState {
    pub const fn new(brightness: u8, red: u8, green: u8, blue: u8) -> Self {
        Self {
            brightness,
            red,
            green,
            blue,
        }
    }
}
