//! Fan command implementation.

use serde_json::Value;
use crate::cli::FanAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_fan(client: &CliClient, action: &FanAction, json_mode: bool) -> Result<(), i32> {
    match action {
        FanAction::List => {
            let assignments = client.load_local_assignments().await;
            let names_raw = client.proxy().get_fan_profile_names().await.unwrap_or_else(|_| "[]".to_string());
            let customs_raw = client.proxy().get_custom_fan_profiles_json().await.unwrap_or_else(|_| "[]".to_string());

            let names_val: Value = serde_json::from_str(&names_raw).unwrap_or(Value::Array(vec![]));
            let customs_val: Value = serde_json::from_str(&customs_raw).unwrap_or(Value::Array(vec![]));

            if json_mode {
                let out = serde_json::json!({
                    "builtin": names_val,
                    "custom": customs_val,
                });
                print_json(&out);
                return Ok(());
            }

            if let Some(arr) = names_val.as_array() {
                println!("Fan profiles:");
                for v in arr {
                    let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                    let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                    let tag = assignment_tag(assignments.fan_states.get(id).map(|v| v.as_slice()).unwrap_or(&[]));
                    println!("  {:<36}  {}{}", id, name, tag);
                }
            }

            if let Some(arr) = customs_val.as_array() {
                if !arr.is_empty() {
                    println!("\nCustom fan profiles:");
                    for v in arr {
                        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                        let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                        let tag = assignment_tag(assignments.fan_states.get(id).map(|v| v.as_slice()).unwrap_or(&[]));
                        println!("  {:<36}  {}{}", id, name, tag);
                    }
                }
            }

            Ok(())
        }
        FanAction::Get { id } => {
            let mut prof_json = client.proxy().get_fan_profile(id).await.ok();
            let mut display_name = id.clone();

            if prof_json.as_deref() == Some("{}") || prof_json.is_none() {
                // Check custom profiles
                if let Ok(cust_raw) = client.proxy().get_custom_fan_profiles_json().await {
                    if let Ok(val) = serde_json::from_str::<Value>(&cust_raw) {
                        if let Some(arr) = val.as_array() {
                            for item in arr {
                                if item.get("id").and_then(|s| s.as_str()) == Some(id) {
                                    if let Some(n) = item.get("name").and_then(|s| s.as_str()) {
                                        display_name = n.to_string();
                                    }
                                    prof_json = item.get("json").and_then(|v| v.as_str()).map(|s| s.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            let json_str = match prof_json {
                Some(s) if !s.is_empty() => s,
                _ => {
                    eprintln!("Error: Could not retrieve fan profile");
                    return Err(1);
                }
            };

            let val: Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("Error: Invalid fan profile JSON");
                    return Err(1);
                }
            };

            if json_mode {
                print_json(&val);
                return Ok(());
            }

            println!("=== Fan Profile: {} ===", display_name);
            println!("  {:<24} {}", "ID:", id);

            if let Some(arr) = val.get("tableCPU").and_then(|v| v.as_array()) {
                print_curve_table("CPU fan curve", arr);
            }
            if let Some(arr) = val.get("tableGPU").and_then(|v| v.as_array()) {
                print_curve_table("GPU fan curve", arr);
            }
            if let Some(arr) = val.get("tablePump").and_then(|v| v.as_array()) {
                print_curve_table("Pump curve", arr);
            }
            if let Some(arr) = val.get("tableWaterCoolerFan").and_then(|v| v.as_array()) {
                print_curve_table("Water cooler fan curve", arr);
            }

            Ok(())
        }
        FanAction::Set { id } => {
            // Find fan profile JSON and apply
            let mut found_json = client.proxy().get_fan_profile(id).await.ok();
            if found_json.as_deref() == Some("{}") || found_json.is_none() {
                if let Ok(cust_raw) = client.proxy().get_custom_fan_profiles_json().await {
                    if let Ok(val) = serde_json::from_str::<Value>(&cust_raw) {
                        if let Some(arr) = val.as_array() {
                            for item in arr {
                                if item.get("id").and_then(|s| s.as_str()) == Some(id) {
                                    found_json = item.get("json").and_then(|v| v.as_str()).map(|s| s.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            let profile_content = match found_json {
                Some(s) if !s.is_empty() && s != "{}" => s,
                _ => {
                    eprintln!("Error: Fan profile not found");
                    return Err(1);
                }
            };

            let res = client.proxy().set_fan_profile(id, &profile_content).await.unwrap_or(false);
            ok(res)
        }
        FanAction::Apply { json } => {
            let res = client.proxy().apply_fan_profiles(json).await.unwrap_or(false);
            ok(res)
        }
        FanAction::Revert => {
            let res = client.proxy().revert_fan_profiles().await.unwrap_or(false);
            ok(res)
        }
    }
}

fn print_curve_table(label: &str, arr: &[Value]) {
    if arr.is_empty() { return; }
    println!("\n  {}:", label);
    println!("    {:<10} Speed (%)", "Temp (°C)");
    println!("    {:<10} ---------", "--------");
    for item in arr {
        let temp = item.get("temp").and_then(|v| v.as_i64()).unwrap_or(0);
        let speed = item.get("speed").and_then(|v| v.as_i64()).unwrap_or(0);
        println!("    {:<10} {}", temp, speed);
    }
}
