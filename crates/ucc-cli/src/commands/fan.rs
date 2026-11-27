//! Fan command implementation.

use crate::cli::FanAction;
use crate::client::{with_rpc_timeout, with_write_rpc_timeout, CliClient};
use crate::format::*;
use serde_json::Value;

fn extract_json_field_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(v).ok(),
        _ => None,
    }
}

fn is_empty_fan_profile_payload(raw: Option<&str>) -> bool {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    if trimmed == "{}" {
        return true;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .is_some_and(|v| v.as_object().is_some_and(|m| m.is_empty()))
}

pub async fn run_fan(client: &CliClient, action: &FanAction, json_mode: bool) -> Result<(), i32> {
    match action {
        FanAction::List => {
            let (assignments, names_res, customs_res) = tokio::join!(
                client.load_local_assignments(),
                with_rpc_timeout(client.proxy().get_fan_profile_names()),
                with_rpc_timeout(client.proxy().get_custom_fan_profiles_json()),
            );

            let names_raw = names_res.map_err(|_| {
                eprintln!("Error: Could not retrieve fan profiles");
                1
            })?;
            let customs_raw = customs_res.unwrap_or_else(|_| "[]".to_string());

            let names_val: Value = serde_json::from_str(&names_raw).unwrap_or(Value::Array(vec![]));
            let customs_val: Value =
                serde_json::from_str(&customs_raw).unwrap_or(Value::Array(vec![]));

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
                    let (id, name) = if let Some(s) = v.as_str() {
                        (s, s)
                    } else {
                        (
                            v.get("id").and_then(|s| s.as_str()).unwrap_or(""),
                            v.get("name").and_then(|s| s.as_str()).unwrap_or(""),
                        )
                    };
                    let tag = assignment_tag(
                        assignments
                            .fan_states
                            .get(id)
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]),
                    );
                    println!("  {:<36}  {}{}", id, name, tag);
                }
            }

            if let Some(arr) = customs_val.as_array() {
                if !arr.is_empty() {
                    println!("\nCustom fan profiles:");
                    for v in arr {
                        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                        let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                        let tag = assignment_tag(
                            assignments
                                .fan_states
                                .get(id)
                                .map(|v| v.as_slice())
                                .unwrap_or(&[]),
                        );
                        println!("  {:<36}  {}{}", id, name, tag);
                    }
                }
            }

            Ok(())
        }
        FanAction::Get { id } => {
            let mut prof_json = with_rpc_timeout(client.proxy().get_fan_profile(id))
                .await
                .ok();
            let mut display_name = id.clone();

            if is_empty_fan_profile_payload(prof_json.as_deref()) {
                // Check custom profiles
                if let Ok(cust_raw) =
                    with_rpc_timeout(client.proxy().get_custom_fan_profiles_json()).await
                {
                    if let Ok(val) = serde_json::from_str::<Value>(&cust_raw) {
                        if let Some(arr) = val.as_array() {
                            for item in arr {
                                if item.get("id").and_then(|s| s.as_str()) == Some(id) {
                                    if let Some(n) = item.get("name").and_then(|s| s.as_str()) {
                                        display_name = n.to_string();
                                    }
                                    prof_json =
                                        item.get("json").and_then(extract_json_field_string);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if is_empty_fan_profile_payload(prof_json.as_deref()) {
                eprintln!("Error: Fan profile '{id}' not found");
                return Err(1);
            }

            let json_str = prof_json.unwrap_or_default();
            let val: Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("Error: Invalid fan profile JSON");
                    return Err(1);
                }
            };
            if val.as_object().is_some_and(|m| m.is_empty()) {
                eprintln!("Error: Fan profile '{id}' not found");
                return Err(1);
            }

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
            let mut found_json = with_rpc_timeout(client.proxy().get_fan_profile(id))
                .await
                .ok();
            if found_json.as_deref() == Some("{}") || found_json.is_none() {
                if let Ok(cust_raw) =
                    with_rpc_timeout(client.proxy().get_custom_fan_profiles_json()).await
                {
                    if let Ok(val) = serde_json::from_str::<Value>(&cust_raw) {
                        if let Some(arr) = val.as_array() {
                            for item in arr {
                                if item.get("id").and_then(|s| s.as_str()) == Some(id) {
                                    found_json =
                                        item.get("json").and_then(extract_json_field_string);
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

            // Remap keys: tableCPU->cpu, tableGPU->gpu, etc. per C++ baseline cmdFanSet
            let val = match serde_json::from_str::<Value>(&profile_content) {
                Ok(v) if v.is_object() => v,
                _ => {
                    eprintln!("Error: Invalid fan profile JSON");
                    return Err(1);
                }
            };

            let mut dst = serde_json::Map::new();
            if let Some(v) = val.get("tableCPU") {
                dst.insert("cpu".into(), v.clone());
            }
            if let Some(v) = val.get("tableGPU") {
                dst.insert("gpu".into(), v.clone());
            }
            if let Some(v) = val.get("tablePump") {
                dst.insert("pump".into(), v.clone());
            }
            if let Some(v) = val.get("tableWaterCoolerFan") {
                dst.insert("waterCoolerFan".into(), v.clone());
            }
            if let Some(v) = val.get("cpu") {
                dst.insert("cpu".into(), v.clone());
            }
            if let Some(v) = val.get("gpu") {
                dst.insert("gpu".into(), v.clone());
            }
            if let Some(v) = val.get("pump") {
                dst.insert("pump".into(), v.clone());
            }
            if let Some(v) = val.get("waterCoolerFan") {
                dst.insert("waterCoolerFan".into(), v.clone());
            }

            let apply_json = serde_json::to_string(&Value::Object(dst)).unwrap_or_default();
            let res = with_write_rpc_timeout(client.proxy().apply_fan_profiles(&apply_json))
                .await
                .unwrap_or(false);
            ok(res)
        }
        FanAction::Apply { json } => {
            let res = with_write_rpc_timeout(client.proxy().apply_fan_profiles(json))
                .await
                .unwrap_or(false);
            ok(res)
        }
        FanAction::Revert => {
            let res = with_write_rpc_timeout(client.proxy().revert_fan_profiles())
                .await
                .unwrap_or(false);
            ok(res)
        }
    }
}

fn print_curve_table(label: &str, arr: &[Value]) {
    if arr.is_empty() {
        return;
    }
    println!("\n  {}:", label);
    println!("    {:<10} Speed (%)", "Temp (°C)");
    println!("    {:<10} ---------", "--------");
    for item in arr {
        let temp = item.get("temp").and_then(|v| v.as_i64()).unwrap_or(0);
        let speed = item.get("speed").and_then(|v| v.as_i64()).unwrap_or(0);
        println!("    {:<10} {}", temp, speed);
    }
}
