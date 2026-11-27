// SPDX-License-Identifier: GPL-3.0-or-later

//! Lid-switch internal panel DPMS controller using standard Hyprland/KDE commands.

use crate::refresh_advisor::RefreshBackend;
use serde_json::{Map, Value};

pub struct LidDisplayHandler {
    backend: RefreshBackend,
    source: String,
    lid_config: Map<String, Value>,
    lid_closed: bool,
    initialized: bool,
}

impl LidDisplayHandler {
    pub fn new(backend: RefreshBackend) -> Self {
        Self {
            backend,
            source: "ac".to_string(),
            lid_config: Map::new(),
            lid_closed: false,
            initialized: false,
        }
    }

    pub fn policy_changed(&mut self, json: &str) {
        if let Ok(val) = serde_json::from_str::<Value>(json) {
            if let Some(src) = val.get("real_power_source").and_then(|v| v.as_str()) {
                if matches!(src, "ac" | "type_c" | "bat") {
                    self.source = src.to_string();
                }
            }
        }
    }

    pub fn device_settings_changed(&mut self, json: &str) {
        if let Ok(val) = serde_json::from_str::<Value>(json) {
            self.lid_config = val
                .get("lid")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
        }
    }

    pub fn action_for_source(&self, source: &str) -> String {
        if let Some(s) = self.lid_config.get(source).and_then(|v| v.as_str()) {
            return s.to_string();
        }
        if source == "bat" {
            "suspend".to_string()
        } else {
            "screen_off".to_string()
        }
    }

    pub fn is_lid_closed(&self) -> bool {
        self.lid_closed
    }

    /// Processes a lid state transition and returns `(program, args)` commands to execute.
    pub fn on_lid_state_changed(
        &mut self,
        closed: bool,
        internal_monitors: &[String],
    ) -> Vec<(&'static str, Vec<String>)> {
        if !self.initialized {
            self.initialized = true;
            self.lid_closed = closed;
            return Vec::new();
        }
        if closed == self.lid_closed {
            return Vec::new();
        }
        self.lid_closed = closed;

        let enable = if closed {
            let act = self.action_for_source(&self.source);
            if act != "screen_off" && act != "lock" {
                return Vec::new();
            }
            false
        } else {
            true
        };

        let mut cmds = Vec::new();
        for mon in internal_monitors {
            let upper = mon.to_ascii_uppercase();
            if !(upper.starts_with("EDP-")
                || upper.starts_with("LVDS-")
                || upper.starts_with("DSI-"))
            {
                continue;
            }
            match self.backend {
                RefreshBackend::Hyprland => {
                    let dpms = if enable { "on" } else { "off" };
                    cmds.push((
                        "hyprctl",
                        vec![
                            "dispatch".to_string(),
                            "dpms".to_string(),
                            dpms.to_string(),
                            mon.clone(),
                        ],
                    ));
                }
                RefreshBackend::Kde => {
                    let state = if enable { "enable" } else { "disable" };
                    cmds.push(("kscreen-doctor", vec![format!("output.{mon}.{state}")]));
                }
                RefreshBackend::Unsupported => {}
            }
        }
        cmds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_standard_hyprctl_dispatch_dpms_on_close_and_open() {
        let mut handler = LidDisplayHandler::new(RefreshBackend::Hyprland);
        let monitors = vec!["eDP-1".to_string(), "HDMI-A-1".to_string()];

        // Initial seed produces no commands
        assert!(handler.on_lid_state_changed(false, &monitors).is_empty());

        let close_cmds = handler.on_lid_state_changed(true, &monitors);
        assert_eq!(close_cmds.len(), 1);
        assert_eq!(close_cmds[0].0, "hyprctl");
        assert_eq!(close_cmds[0].1, vec!["dispatch", "dpms", "off", "eDP-1"]);

        let open_cmds = handler.on_lid_state_changed(false, &monitors);
        assert_eq!(open_cmds.len(), 1);
        assert_eq!(open_cmds[0].1, vec!["dispatch", "dpms", "on", "eDP-1"]);
    }
}
