//! Profile command implementation.

use serde_json::Value;
use crate::cli::ProfileAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_profile(client: &CliClient, action: &ProfileAction, json_mode: bool) -> Result<(), i32> {
    match action {
        ProfileAction::List => {
            let assignments = client.load_local_assignments().await;
            let defaults_raw = client.proxy().get_default_profiles_json().await.unwrap_or_else(|_| "[]".to_string());
            let customs_raw = client.proxy().get_custom_profiles_json().await.unwrap_or_else(|_| "[]".to_string());
            let active = client.get_active_profile_json().await;

            let defaults_val: Value = serde_json::from_str(&defaults_raw).unwrap_or(Value::Array(vec![]));
            let customs_val: Value = serde_json::from_str(&customs_raw).unwrap_or(Value::Array(vec![]));

            if json_mode {
                let out = serde_json::json!({
                    "builtin": defaults_val,
                    "custom": customs_val,
                    "active": active,
                });
                print_json(&out);
                return Ok(());
            }

            if let Some(arr) = defaults_val.as_array() {
                println!("Built-in profiles:");
                for v in arr {
                    let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                    let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                    println!("  {:<36}  {}", id, name);
                }
            }

            if let Some(arr) = customs_val.as_array() {
                if !arr.is_empty() {
                    println!("\nCustom profiles:");
                    for v in arr {
                        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                        let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                        let tag = assignment_tag(assignments.profile_states.get(id).map(|v| v.as_slice()).unwrap_or(&[]));
                        println!("  {:<36}  {}{}", id, name, tag);
                    }
                }
            }

            if let Some(ref act) = active {
                println!("\nActive: {} ({})", profile_name(act), profile_id(act));
            }

            Ok(())
        }
        ProfileAction::Get { id } => {
            let prof = if let Some(ref prof_id) = id {
                // Find in defaults or customs
                let mut found = None;
                if let Ok(def_raw) = client.proxy().get_default_profiles_json().await {
                    if let Ok(val) = serde_json::from_str::<Value>(&def_raw) {
                        if let Some(arr) = val.as_array() {
                            for item in arr {
                                if item.get("id").and_then(|s| s.as_str()) == Some(prof_id) {
                                    found = Some(item.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                if found.is_none() {
                    if let Ok(cust_raw) = client.proxy().get_custom_profiles_json().await {
                        if let Ok(val) = serde_json::from_str::<Value>(&cust_raw) {
                            if let Some(arr) = val.as_array() {
                                for item in arr {
                                    if item.get("id").and_then(|s| s.as_str()) == Some(prof_id) {
                                        found = Some(item.clone());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                found
            } else {
                client.get_active_profile_json().await
            };

            let prof = match prof {
                Some(p) => p,
                None => {
                    eprintln!("Error: Could not retrieve profile");
                    return Err(1);
                }
            };

            if json_mode {
                print_json(&prof);
            } else {
                print_profile_summary(&prof, true);
            }
            Ok(())
        }
        ProfileAction::Set { id } => {
            let res = client.proxy().set_active_profile(id).await.unwrap_or(false);
            ok(res)
        }
        ProfileAction::Defaults => {
            let def_raw = client.proxy().get_default_profiles_json().await.map_err(|_| 1)?;
            let val: Value = serde_json::from_str(&def_raw).unwrap_or(Value::Array(vec![]));
            if json_mode {
                print_json(&val);
            } else if let Some(arr) = val.as_array() {
                println!("Built-in profiles ({}):", arr.len());
                for (i, p) in arr.iter().enumerate() {
                    if i > 0 { println!(); }
                    print_profile_summary(p, false);
                    if i + 1 < arr.len() {
                        println!("  ────────────────────────────");
                    }
                }
            } else {
                println!("{}", def_raw);
            }
            Ok(())
        }
        ProfileAction::Customs => {
            let cust_raw = client.proxy().get_custom_profiles_json().await.map_err(|_| 1)?;
            let val: Value = serde_json::from_str(&cust_raw).unwrap_or(Value::Array(vec![]));
            if json_mode {
                print_json(&val);
            } else if let Some(arr) = val.as_array() {
                if arr.is_empty() {
                    println!("No custom profiles.");
                } else {
                    println!("Custom profiles ({}):", arr.len());
                    for (i, p) in arr.iter().enumerate() {
                        if i > 0 { println!(); }
                        print_profile_summary(p, false);
                        if i + 1 < arr.len() {
                            println!("  ────────────────────────────");
                        }
                    }
                }
            } else {
                println!("{}", cust_raw);
            }
            Ok(())
        }
        ProfileAction::Apply { json } => {
            let res = client.proxy().apply_profile(json).await.unwrap_or(false);
            ok(res)
        }
        ProfileAction::Save { json } => {
            let res = client.proxy().save_custom_profile(json).await.unwrap_or(false);
            ok(res)
        }
        ProfileAction::Delete { id } => {
            let res = client.proxy().delete_custom_profile(id).await.unwrap_or(false);
            ok(res)
        }
    }
}

pub fn print_profile_summary(obj: &Value, show_header: bool) {
    let name = profile_name(obj);
    let id = profile_id(obj);

    if show_header {
        println!("=== Profile: {} ===", name);
    } else {
        println!("  {:<24} {}", "Name:", name);
    }
    println!("  {:<24} {}", "ID:", id);

    if let Some(desc) = obj.get("description").and_then(|v| v.as_str()) {
        if !desc.is_empty() {
            println!("  {:<24} {}", "Description:", desc);
        }
    }

    if let Some(cpu) = obj.get("cpu").and_then(|v| v.as_object()) {
        if let Some(gov) = cpu.get("governor").and_then(|v| v.as_str()) {
            println!("  {:<24} {}", "Governor:", gov);
        }
        if let Some(epp) = cpu.get("epp").and_then(|v| v.as_str()) {
            println!("  {:<24} {}", "EPP:", epp);
        }
    }

    if let Some(fan) = obj.get("fan").and_then(|v| v.as_object()) {
        if let Some(fp) = fan.get("fanProfile").and_then(|v| v.as_str()) {
            if !fp.is_empty() {
                println!("  {:<24} {}", "Fan profile:", fp);
            }
        }
    }

    if let Some(disp) = obj.get("display").and_then(|v| v.as_object()) {
        if let Some(b) = disp.get("brightness").and_then(|v| v.as_i64()) {
            println!("  {:<24} {} %", "Display brightness:", b);
        }
    }

    if let Some(limits) = obj.get("odmPowerLimits").and_then(|v| v.as_array()) {
        if !limits.is_empty() {
            let vals: Vec<String> = limits.iter().filter_map(|v| v.as_i64().map(|n| format!("{}W", n))).collect();
            println!("  {:<24} {}", "Power limits (PL1/2/4):", vals.join(" / "));
        }
    }

    if let Some(kp) = obj.get("selectedKeyboardProfile").and_then(|v| v.as_str()) {
        if !kp.is_empty() {
            println!("  {:<24} {}", "Keyboard profile:", kp);
        }
    }
}
