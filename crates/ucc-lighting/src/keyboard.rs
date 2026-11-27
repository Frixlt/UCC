//! Per-key / multi-zone keyboard backlight controller (`M6.1`–`M6.2`).

use crate::transport::{KeyboardCapabilities, LightingError, LightingTransport, RgbColor};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ucc_core::KeyboardBacklightState;

/// Calibrated Mechrevo / Uniwill 0x1A per-key physical LED ID constants (`M6.1`).
pub const MECHREVO_LCTRL_LED_ID: u32 = 0;
pub const MECHREVO_LSHIFT_LED_ID: u32 = 21;
pub const MECHREVO_ENTER_LED_ID: u32 = 56;
pub const MECHREVO_EXPECTED_ZONES: usize = 126;
pub const MECHREVO_PHYSICAL_KEY_COUNT: usize = 101;

#[derive(Debug, Deserialize)]
struct KeyboardStateDto {
    #[serde(default)]
    brightness: i64,
    #[serde(default)]
    red: i64,
    #[serde(default)]
    green: i64,
    #[serde(default)]
    blue: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardCapabilitiesJson<'a> {
    modes: [u8; 1],
    zones: usize,
    max_brightness: u32,
    #[serde(skip_serializing_if = "is_zero_u8")]
    max_red: u8,
    #[serde(skip_serializing_if = "is_zero_u8")]
    max_green: u8,
    #[serde(skip_serializing_if = "is_zero_u8")]
    max_blue: u8,
    led_ids: &'a [u32],
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

/// RAII guard guaranteeing `buffer_input` is disabled on scope exit while reporting enable/release I/O errors (`W1-06`).
pub struct BufferInputGuard {
    transport: Arc<dyn LightingTransport>,
    released: bool,
}

impl BufferInputGuard {
    pub fn acquire(transport: Arc<dyn LightingTransport>) -> Result<Self, LightingError> {
        transport.set_keyboard_buffer_input(true)?;
        Ok(Self {
            transport,
            released: false,
        })
    }

    pub fn release(mut self) -> Result<(), LightingError> {
        self.transport.set_keyboard_buffer_input(false)?;
        self.released = true;
        Ok(())
    }
}

impl Drop for BufferInputGuard {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.transport.set_keyboard_buffer_input(false);
        }
    }
}

/// Maps `states` to hardware zone colors matching physical `caps.led_ids` (`W1-13`).
///
/// - When `states.len() > caps.zones` (e.g. a 126-zone profile applied to hardware missing optional
///   keys), indexes by physical `led_id` so `LCtrl (0)`, `LShift (21)`, and `Enter (56)` never
///   shift to neighboring keys.
/// - When `states.len() <= caps.zones` (partial/compact legacy update), returns only `states.len()`
///   colors so untouched trailing zones keep their existing hardware colors.
pub fn map_states_by_led_id(
    caps: &KeyboardCapabilities,
    states: &[KeyboardBacklightState],
) -> Vec<RgbColor> {
    if states.len() > caps.zones && caps.led_ids.len() == caps.zones {
        caps.led_ids
            .iter()
            .map(|&led_id| {
                states
                    .get(led_id as usize)
                    .map(|s| RgbColor::new(s.red, s.green, s.blue))
                    .unwrap_or(RgbColor::BLACK)
            })
            .collect()
    } else {
        states
            .iter()
            .take(caps.zones)
            .map(|s| RgbColor::new(s.red, s.green, s.blue))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardHardwareSnapshot {
    pub colors: Vec<RgbColor>,
    pub brightness: Option<u32>,
    pub states: Vec<KeyboardBacklightState>,
}

/// Synchronous keyboard backlight controller matching `KeyboardBacklightController.hpp`.
pub struct KeyboardBacklightController {
    transport: Arc<dyn LightingTransport>,
    capabilities: Option<KeyboardCapabilities>,
    current_states: Vec<KeyboardBacklightState>,
}

impl KeyboardBacklightController {
    pub fn new(transport: Arc<dyn LightingTransport>) -> Self {
        Self {
            transport,
            capabilities: None,
            current_states: Vec::new(),
        }
    }

    /// Probes hardware via [`LightingTransport`] and returns serialized capabilities JSON (`"null"` if absent).
    pub fn init(&mut self) -> Result<String, LightingError> {
        self.capabilities = self.transport.detect_keyboard()?;
        Ok(self.capabilities_json())
    }

    /// Populates `current_states` from physical LED hardware when no state has been recorded yet.
    pub fn hydrate_from_hardware(&mut self) {
        if !self.current_states.is_empty() {
            return;
        }
        let Some(caps) = self.capabilities.as_ref().filter(|c| c.zones > 0) else {
            return;
        };
        let colors = self.transport.read_keyboard_colors().unwrap_or_default();
        if colors.len() != caps.zones {
            return;
        }
        let brightness = self
            .transport
            .read_keyboard_brightness()
            .ok()
            .flatten()
            .unwrap_or(caps.max_brightness)
            .min(caps.max_brightness)
            .min(u8::MAX as u32) as u8;
        self.current_states = colors
            .into_iter()
            .map(|c| KeyboardBacklightState {
                brightness,
                red: c.r,
                green: c.g,
                blue: c.b,
            })
            .collect();
    }

    pub fn is_available(&self) -> bool {
        self.capabilities
            .as_ref()
            .is_some_and(|caps| caps.zones > 0)
    }

    pub fn snapshot_hardware_state(&mut self) -> Option<KeyboardHardwareSnapshot> {
        if !self.is_available() {
            let _ = self.init();
        }
        if !self.is_available() {
            return None;
        }
        let colors = self.transport.read_keyboard_colors().unwrap_or_default();
        let brightness = self
            .transport
            .read_keyboard_brightness()
            .ok()
            .flatten()
            .or_else(|| self.current_states.first().map(|s| s.brightness as u32));
        Some(KeyboardHardwareSnapshot {
            colors,
            brightness,
            states: self.current_states.clone(),
        })
    }

    pub fn restore_hardware_state(
        &mut self,
        snapshot: &KeyboardHardwareSnapshot,
    ) -> Result<(), LightingError> {
        if !self.is_available() {
            let _ = self.init();
        }
        let caps = self
            .capabilities
            .as_ref()
            .filter(|c| c.zones > 0)
            .ok_or(LightingError::Unavailable)?
            .clone();

        let buffer_guard = BufferInputGuard::acquire(Arc::clone(&self.transport))?;
        let write_res = (|| -> Result<(), LightingError> {
            if let Some(brightness) = snapshot.brightness {
                self.transport
                    .write_keyboard_brightness(brightness.min(caps.max_brightness))?;
            }
            if caps.max_red > 0 && !snapshot.colors.is_empty() {
                self.transport.write_keyboard_zones(&snapshot.colors)?;
            }
            Ok(())
        })();
        let release_res = buffer_guard.release();
        write_res?;
        release_res?;

        self.current_states = snapshot.states.clone();
        Ok(())
    }

    pub fn capabilities(&self) -> Option<&KeyboardCapabilities> {
        self.capabilities.as_ref()
    }

    pub fn capabilities_json(&self) -> String {
        let Some(caps) = self.capabilities.as_ref().filter(|c| c.zones > 0) else {
            return "null".to_string();
        };
        let payload = KeyboardCapabilitiesJson {
            modes: [0],
            zones: caps.zones,
            max_brightness: caps.max_brightness,
            max_red: caps.max_red,
            max_green: caps.max_green,
            max_blue: caps.max_blue,
            led_ids: &caps.led_ids,
        };
        serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_string())
    }

    pub fn current_states(&self) -> &[KeyboardBacklightState] {
        &self.current_states
    }

    pub fn current_states_json(&self) -> String {
        serde_json::to_string(&self.current_states).unwrap_or_else(|_| "[]".to_string())
    }

    pub fn build_default_states_json(&self) -> String {
        let Some(caps) = self.capabilities.as_ref().filter(|c| c.zones > 0) else {
            return "[]".to_string();
        };
        let defaults: Vec<KeyboardBacklightState> = (0..caps.zones)
            .map(|_| KeyboardBacklightState {
                brightness: caps.max_brightness.min(u8::MAX as u32) as u8,
                red: caps.max_red,
                green: caps.max_green,
                blue: caps.max_blue,
            })
            .collect();
        serde_json::to_string(&defaults).unwrap_or_else(|_| "[]".to_string())
    }

    /// Applies states inside an RAII [`BufferInputGuard`], propagating buffer toggle errors (`W1-06`)
    /// and mapping full profiles by physical LED ID without index shifting (`W1-13`).
    pub fn apply_states(&mut self, states: &[KeyboardBacklightState]) -> Result<(), LightingError> {
        let caps = self
            .capabilities
            .as_ref()
            .filter(|c| c.zones > 0)
            .ok_or(LightingError::Unavailable)?
            .clone();
        if states.is_empty() {
            return Err(LightingError::InvalidConfig(
                "Keyboard states array cannot be empty".into(),
            ));
        }

        let prev_brightness = self
            .transport
            .read_keyboard_brightness()
            .ok()
            .flatten()
            .or_else(|| self.current_states.first().map(|s| s.brightness as u32));
        let buffer_guard = BufferInputGuard::acquire(Arc::clone(&self.transport))?;

        let write_res = (|| -> Result<(), LightingError> {
            let target_brightness = (states[0].brightness as u32).min(caps.max_brightness);
            self.transport
                .write_keyboard_brightness(target_brightness)?;

            if caps.max_red > 0 {
                let zone_colors = map_states_by_led_id(&caps, states);
                if let Err(err) = self.transport.write_keyboard_zones(&zone_colors) {
                    if let Some(prev_b) = prev_brightness {
                        let _ = self.transport.write_keyboard_brightness(prev_b);
                    }
                    return Err(err);
                }
            }
            Ok(())
        })();

        let release_res = buffer_guard.release();
        write_res?;
        release_res?;

        self.current_states = states.to_vec();
        Ok(())
    }

    pub fn apply_states_json(&mut self, states_json: &str) -> Result<(), LightingError> {
        let caps = self
            .capabilities
            .as_ref()
            .filter(|c| c.zones > 0)
            .ok_or(LightingError::Unavailable)?
            .clone();

        let raw: Vec<KeyboardStateDto> = serde_json::from_str(states_json)
            .map_err(|e| LightingError::InvalidConfig(e.to_string()))?;
        if raw.is_empty() {
            return Err(LightingError::InvalidConfig(
                "Keyboard states JSON cannot be empty".into(),
            ));
        }

        let max_b = caps.max_brightness.min(u8::MAX as u32) as i64;
        let normalized: Vec<KeyboardBacklightState> = raw
            .into_iter()
            .map(|entry| KeyboardBacklightState {
                brightness: entry.brightness.clamp(0, max_b) as u8,
                red: entry.red.clamp(0, 255) as u8,
                green: entry.green.clamp(0, 255) as u8,
                blue: entry.blue.clamp(0, 255) as u8,
            })
            .collect();

        self.apply_states(&normalized)
    }

    pub fn apply_profile_keyboard_json(
        &mut self,
        keyboard_data_json: &str,
    ) -> Result<(), LightingError> {
        let value: serde_json::Value = serde_json::from_str(keyboard_data_json)
            .map_err(|e| LightingError::InvalidConfig(e.to_string()))?;
        let states_val = value
            .get("states")
            .ok_or_else(|| LightingError::InvalidConfig("Missing keyboard states field".into()))?;
        let states_str = serde_json::to_string(states_val)
            .map_err(|e| LightingError::InvalidConfig(e.to_string()))?;
        self.apply_states_json(&states_str)
    }
}
