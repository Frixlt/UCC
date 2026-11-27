// SPDX-License-Identifier: GPL-3.0-or-later

//! KDE (`kscreen-doctor`) and Hyprland (`hyprctl`) internal display refresh rate advisor.

use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshBackend {
    Unsupported,
    Kde,
    Hyprland,
}

impl RefreshBackend {
    pub fn detect_from_env() -> Self {
        let xdg = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let lower = xdg.to_ascii_lowercase();
        if lower.split(':').any(|p| p == "hyprland") {
            Self::Hyprland
        } else if lower.split(':').any(|p| p == "kde") {
            Self::Kde
        } else {
            Self::Unsupported
        }
    }

    pub fn command_name(&self) -> Option<&'static str> {
        match self {
            Self::Hyprland => Some("hyprctl"),
            Self::Kde => Some("kscreen-doctor"),
            Self::Unsupported => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshPlan {
    pub output: String,
    pub current_mode: String,
    pub target_mode: String,
    pub width: i32,
    pub height: i32,
    pub current_hz: f64,
    pub target_hz: f64,
}

fn valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '+' | '-'))
}

fn is_internal_panel(name: &str) -> bool {
    name.starts_with("eDP-") || name.starts_with("LVDS-")
}

fn nvidia_panel(name: &str, drm_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(drm_root) else {
        return false;
    };
    let suffix = format!("-{name}");
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(fname_str) = fname.to_str() else {
            continue;
        };
        if let Some(card) = fname_str.strip_suffix(&suffix) {
            if let Ok(vendor) = std::fs::read_to_string(drm_root.join(card).join("device/vendor")) {
                if vendor.trim() == "0x10de" {
                    return true;
                }
            }
        }
    }
    false
}

pub fn hyprland_configuration(raw_json: &[u8]) -> Option<Value> {
    let monitors: Value = serde_json::from_slice(raw_json).ok()?;
    let arr = monitors.as_array()?;
    let mut outputs = Vec::new();

    for m in arr {
        if m.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let mirror = m.get("mirrorOf").and_then(|v| v.as_str()).unwrap_or("");
        if !mirror.is_empty() && mirror != "none" {
            continue;
        }
        let width = m.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
        let height = m.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
        let cur_hz = m.get("refreshRate").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let mut modes = Vec::new();
        let mut current = String::new();
        if let Some(avail) = m.get("availableModes").and_then(|v| v.as_array()) {
            for mode_str in avail.iter().filter_map(|v| v.as_str()) {
                let Some(without_hz) = mode_str.strip_suffix("Hz") else {
                    continue;
                };
                let Some((res_part, hz_part)) = without_hz.split_once('@') else {
                    continue;
                };
                let Some((w_part, h_part)) = res_part.split_once('x') else {
                    continue;
                };
                let (Ok(w), Ok(h), Ok(hz)) = (
                    w_part.parse::<i64>(),
                    h_part.parse::<i64>(),
                    hz_part.parse::<f64>(),
                ) else {
                    continue;
                };
                if !hz.is_finite() {
                    continue;
                }
                let id = format!("{w}x{h}@{hz:.2}");
                if w == width && h == height && (hz - cur_hz).abs() < 0.1 {
                    current = id.clone();
                }
                modes.push(json!({
                    "id": id,
                    "refreshRate": hz,
                    "size": { "width": w, "height": h }
                }));
            }
        }

        let dpms = m
            .get("dpmsStatus")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        outputs.push(json!({
            "name": m.get("name").cloned().unwrap_or(Value::Null),
            "connected": true,
            "enabled": !current.is_empty() && dpms,
            "currentModeId": current,
            "modes": modes
        }));
    }

    Some(json!({ "outputs": outputs }))
}

fn current_mode(output: &Value) -> Option<&Value> {
    let cur_id = output.get("currentModeId")?;
    output
        .get("modes")?
        .as_array()?
        .iter()
        .find(|m| m.get("id") == Some(cur_id))
}

pub struct RefreshAdvisor;

impl RefreshAdvisor {
    pub fn plan(
        configuration: &Value,
        source: &str,
        drm_root: &Path,
        requested_hz: i32,
    ) -> Option<RefreshPlan> {
        if requested_hz <= 0 && !matches!(source, "ac" | "type_c" | "bat") {
            return None;
        }
        let outputs = configuration.get("outputs")?.as_array()?;
        for output in outputs {
            let name = output.get("name")?.as_str()?;
            if !is_internal_panel(name)
                || !valid_identifier(name)
                || !output
                    .get("connected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                || !output
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            {
                continue;
            }
            let current = current_mode(output)?;
            let size = current.get("size")?.as_object()?;
            let w = size.get("width")?.as_i64()? as i32;
            let h = size.get("height")?.as_i64()? as i32;
            let current_hz = current.get("refreshRate")?.as_f64()?;
            if w <= 0 || h <= 0 || !current_hz.is_finite() || current_hz <= 0.0 {
                continue;
            }

            let preferred = if requested_hz > 0 {
                requested_hz as f64
            } else if source == "bat" {
                60.0
            } else if nvidia_panel(name, drm_root) {
                300.0
            } else {
                240.0
            };

            let mut selected: Option<(&Value, f64)> = None;
            for mode in output.get("modes")?.as_array()? {
                let m_size = match mode.get("size").and_then(|v| v.as_object()) {
                    Some(s) => s,
                    None => continue,
                };
                let mw = m_size.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let mh = m_size.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let m_id = mode.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let hz = mode
                    .get("refreshRate")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(f64::NAN);
                if mw != w || mh != h || !valid_identifier(m_id) || !hz.is_finite() {
                    continue;
                }
                let matches_band = if requested_hz > 0 {
                    (hz - preferred).abs() <= 2.0
                } else if source == "bat" {
                    (hz - 60.0).abs() <= 2.0
                } else {
                    hz >= 100.0 && hz <= preferred + 2.0
                };
                if !matches_band {
                    continue;
                }
                let better = match selected {
                    None => true,
                    Some((_, best_hz)) => {
                        if requested_hz > 0 || source == "bat" {
                            (hz - preferred).abs() < (best_hz - preferred).abs()
                        } else {
                            hz > best_hz
                        }
                    }
                };
                if better {
                    selected = Some((mode, hz));
                }
            }

            let (sel_mode, best_hz) = selected?;
            if (current_hz - best_hz).abs() < 2.0 {
                continue;
            }
            return Some(RefreshPlan {
                output: name.to_string(),
                current_mode: current.get("id")?.as_str()?.to_string(),
                target_mode: sel_mode.get("id")?.as_str()?.to_string(),
                width: w,
                height: h,
                current_hz,
                target_hz: best_hz,
            });
        }
        None
    }

    pub fn applied(configuration: &Value, plan: &RefreshPlan) -> bool {
        let Some(outputs) = configuration.get("outputs").and_then(|v| v.as_array()) else {
            return false;
        };
        for output in outputs {
            if output.get("name").and_then(|v| v.as_str()) != Some(&plan.output)
                || !output
                    .get("connected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                || !output
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            {
                continue;
            }
            let Some(mode) = current_mode(output) else {
                continue;
            };
            let Some(size) = mode.get("size").and_then(|v| v.as_object()) else {
                continue;
            };
            let w = size.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let h = size.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let hz = mode
                .get("refreshRate")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if mode.get("id").and_then(|v| v.as_str()) == Some(&plan.target_mode)
                && w == plan.width
                && h == plan.height
                && (hz - plan.target_hz).abs() < 2.0
            {
                return true;
            }
        }
        false
    }

    /// Builds standard backend command arguments (no mandatory Lua `eval` dependency on Hyprland).
    pub fn apply_command_args(backend: RefreshBackend, plan: &RefreshPlan) -> Vec<String> {
        match backend {
            RefreshBackend::Kde => {
                vec![format!("output.{}.mode.{}", plan.output, plan.target_mode)]
            }
            RefreshBackend::Hyprland => vec![
                "keyword".to_string(),
                "monitor".to_string(),
                format!(
                    "{},{}x{}@{},auto,auto",
                    plan.output,
                    plan.width,
                    plan.height,
                    plan.target_hz.round() as i32
                ),
            ],
            RefreshBackend::Unsupported => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_refresh_switch_for_battery_and_ac() {
        let cfg = json!({
            "outputs": [{
                "name": "eDP-1",
                "connected": true,
                "enabled": true,
                "currentModeId": "240hz",
                "modes": [
                    { "id": "60hz", "refreshRate": 60.0, "size": { "width": 2560, "height": 1600 } },
                    { "id": "240hz", "refreshRate": 240.0, "size": { "width": 2560, "height": 1600 } }
                ]
            }]
        });

        let p = RefreshAdvisor::plan(&cfg, "bat", Path::new("/nonexistent"), 0).unwrap();
        assert_eq!(p.target_mode, "60hz");
        assert!(!RefreshAdvisor::applied(&cfg, &p));

        let args = RefreshAdvisor::apply_command_args(RefreshBackend::Hyprland, &p);
        assert_eq!(
            args,
            vec!["keyword", "monitor", "eDP-1,2560x1600@60,auto,auto"]
        );
    }

    #[test]
    fn parses_hyprland_monitors_json() {
        let raw = br#"[{"name":"eDP-1","disabled":false,"mirrorOf":"none","width":2560,"height":1600,"refreshRate":240.0,"dpmsStatus":true,"availableModes":["2560x1600@240.00Hz","2560x1600@60.00Hz"]}]"#;
        let parsed = hyprland_configuration(raw).unwrap();
        let outputs = parsed["outputs"].as_array().unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["currentModeId"], "2560x1600@240.00");
    }
}
