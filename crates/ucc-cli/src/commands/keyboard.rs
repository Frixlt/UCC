//! Keyboard command implementation.

use serde_json::{json, Value};
use crate::cli::KeyboardAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_keyboard(client: &CliClient, action: &KeyboardAction, json_mode: bool) -> Result<(), i32> {
    match action {
        KeyboardAction::Info => {
            let info_raw = client.proxy().get_keyboard_backlight_capabilities_json().await.map_err(|_| 1)?;
            let val: Value = serde_json::from_str(&info_raw).unwrap_or(Value::Object(serde_json::Map::new()));

            if json_mode {
                print_json(&val);
                return Ok(());
            }

            println!("=== Keyboard Backlight ===");
            if let Some(z) = val.get("zones").and_then(|v| v.as_i64()) {
                println!("  {:<24} {}", "Zones:", z);
            }
            if let Some(mb) = val.get("maxBrightness").and_then(|v| v.as_i64()) {
                println!("  {:<24} {}", "Max brightness:", mb);
            }
            if let Some(mr) = val.get("maxRed").and_then(|v| v.as_i64()) {
                println!("  {:<24} {}", "Max red:", mr);
            }
            if let Some(mg) = val.get("maxGreen").and_then(|v| v.as_i64()) {
                println!("  {:<24} {}", "Max green:", mg);
            }
            if let Some(mbl) = val.get("maxBlue").and_then(|v| v.as_i64()) {
                println!("  {:<24} {}", "Max blue:", mbl);
            }
            if let Some(modes) = val.get("modes").and_then(|v| v.as_array()) {
                let mstrs: Vec<String> = modes.iter().map(|m| {
                    match m.as_i64().unwrap_or(0) {
                        0 => "static".to_string(),
                        1 => "breathe".to_string(),
                        2 => "colorful".to_string(),
                        3 => "breathe-color".to_string(),
                        other => other.to_string(),
                    }
                }).collect();
                println!("  {:<24} {}", "Modes:", mstrs.join(", "));
            }

            Ok(())
        }
        KeyboardAction::Get => {
            let states_raw = client.proxy().get_keyboard_backlight_states_json().await.map_err(|_| 1)?;
            let val: Value = serde_json::from_str(&states_raw).unwrap_or(Value::Object(serde_json::Map::new()));

            if json_mode {
                print_json(&val);
                return Ok(());
            }

            let brightness = val.get("brightness").and_then(|v| v.as_i64()).unwrap_or(0);
            let arr = val.get("states").and_then(|v| v.as_array()).cloned().unwrap_or_default();

            println!("=== Keyboard Backlight State ===");
            println!("  {:<24} {}", "Global brightness:", brightness);
            println!("  {:<24} {}", "Zones:", arr.len());

            if arr.is_empty() {
                return Ok(());
            }

            let first = &arr[0];
            let uniform = arr.iter().all(|z| {
                z.get("red") == first.get("red")
                    && z.get("green") == first.get("green")
                    && z.get("blue") == first.get("blue")
                    && z.get("mode") == first.get("mode")
                    && z.get("brightness") == first.get("brightness")
            });

            if uniform {
                let r = first.get("red").and_then(|v| v.as_i64()).unwrap_or(0);
                let g = first.get("green").and_then(|v| v.as_i64()).unwrap_or(0);
                let b = first.get("blue").and_then(|v| v.as_i64()).unwrap_or(0);
                let zb = first.get("brightness").and_then(|v| v.as_i64()).unwrap_or(0);
                let m = first.get("mode").and_then(|v| v.as_i64()).unwrap_or(0);
                let mode_str = match m {
                    0 => "static",
                    1 => "breathe",
                    2 => "colorful",
                    3 => "breathe-color",
                    _ => "unknown",
                };

                println!("  {:<24} uniform", "Pattern:");
                println!("  {:<24} rgb({}, {}, {})", "Color:", r, g, b);
                println!("  {:<24} {}", "Zone brightness:", zb);
                println!("  {:<24} {}", "Mode:", mode_str);
            } else {
                println!();
                println!("  {:<6} {:<6} {:<14} {:<12}", "Zone", "Mode", "Color", "Brightness");
                println!("  {:<6} {:<6} {:<14} {:<12}", "----", "----", "-----------", "----------");
                for (i, z) in arr.iter().enumerate() {
                    let r = z.get("red").and_then(|v| v.as_i64()).unwrap_or(0);
                    let g = z.get("green").and_then(|v| v.as_i64()).unwrap_or(0);
                    let b = z.get("blue").and_then(|v| v.as_i64()).unwrap_or(0);
                    let zb = z.get("brightness").and_then(|v| v.as_i64()).unwrap_or(0);
                    let m = z.get("mode").and_then(|v| v.as_i64()).unwrap_or(0);
                    let m_str = match m {
                        0 => "static",
                        1 => "breathe",
                        2 => "color",
                        3 => "br-color",
                        _ => "?",
                    };
                    let color = format!("({},{},{})", r, g, b);
                    println!("  {:<6} {:<6} {:<14} {:<12}", i + 1, m_str, color, zb);
                }
            }

            Ok(())
        }
        KeyboardAction::Set { json } => {
            let res = client.proxy().set_keyboard_backlight_states_json(json).await.unwrap_or(false);
            ok(res)
        }
        KeyboardAction::Color { r, g, b, brightness } => {
            let info_raw = client.proxy().get_keyboard_backlight_capabilities_json().await.unwrap_or_else(|_| "{}".to_string());
            let cap_val: Value = serde_json::from_str(&info_raw).unwrap_or(Value::Null);
            let zones = cap_val.get("zones").and_then(|v| v.as_i64()).unwrap_or(1).max(1) as usize;
            let br = brightness.unwrap_or(128);

            let mut states = Vec::new();
            for _ in 0..zones {
                states.push(json!({
                    "mode": 0,
                    "brightness": br,
                    "red": r,
                    "green": g,
                    "blue": b,
                }));
            }
            let root = json!({
                "brightness": br,
                "states": states,
            });
            let payload = serde_json::to_string(&root).unwrap_or_else(|_| "{}".to_string());
            let res = client.proxy().set_keyboard_backlight_states_json(&payload).await.unwrap_or(false);
            ok(res)
        }
        KeyboardAction::Profiles => {
            let assignments = client.load_local_assignments().await;
            let raw = client.proxy().get_custom_keyboard_profiles_json().await.unwrap_or_else(|_| "[]".to_string());
            let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Array(vec![]));

            if json_mode {
                print_json(&val);
                return Ok(());
            }

            let arr = match val.as_array() {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No custom keyboard profiles found.");
                    return Ok(());
                }
            };

            println!("Keyboard profiles:");
            for v in arr {
                let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                let tag = assignment_tag(assignments.kb_states.get(id).map(|v| v.as_slice()).unwrap_or(&[]));
                println!("  {:<36}  {}{}", id, name, tag);
            }

            Ok(())
        }
        KeyboardAction::Activate { id } => {
            let raw = client.proxy().get_custom_keyboard_profiles_json().await.map_err(|_| 1)?;
            let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Array(vec![]));
            if let Some(arr) = val.as_array() {
                for item in arr {
                    if item.get("id").and_then(|s| s.as_str()) == Some(id) {
                        if let Some(json_content) = item.get("json").and_then(|v| v.as_str()) {
                            let res = client.proxy().set_keyboard_backlight_states_json(json_content).await.unwrap_or(false);
                            return ok(res);
                        }
                    }
                }
            }
            eprintln!("Error: Keyboard profile not found");
            Err(1)
        }
    }
}
