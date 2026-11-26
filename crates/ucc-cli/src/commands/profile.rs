//! Profile command implementation.

use serde_json::Value;
use crate::cli::ProfileAction;
use crate::client::{with_rpc_timeout, CliClient};
use crate::format::*;

pub async fn run_profile(client: &CliClient, action: &ProfileAction, json_mode: bool) -> Result<(), i32> {
    match action {
        ProfileAction::List => {
            let (assignments, defaults_res, customs_res, all_res, active) = tokio::join!(
                client.load_local_assignments(),
                with_rpc_timeout(client.proxy().get_default_profiles_json()),
                with_rpc_timeout(client.proxy().get_custom_profiles_json()),
                with_rpc_timeout(client.proxy().get_profiles_json()),
                client.get_active_profile_json(),
            );

            if defaults_res.is_err() && customs_res.is_err() && all_res.is_err() {
                eprintln!("Error: Could not retrieve profiles");
                return Err(1);
            }

            let defaults_arr: Vec<Value> = defaults_res
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let mut customs_arr: Vec<Value> = customs_res
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();

            if let Ok(all_raw) = all_res {
                if let Some(all_arr) = serde_json::from_str::<Value>(&all_raw)
                    .ok()
                    .and_then(|v| v.as_array().cloned())
                {
                    let mut seen: std::collections::HashSet<String> = defaults_arr
                        .iter()
                        .chain(customs_arr.iter())
                        .map(|p| {
                            let id = profile_id(p);
                            if id.is_empty() { profile_name(p) } else { id }
                        })
                        .collect();
                    for item in all_arr {
                        let key = {
                            let id = profile_id(&item);
                            if id.is_empty() { profile_name(&item) } else { id }
                        };
                        if !key.is_empty() && seen.insert(key) {
                            customs_arr.push(item);
                        }
                    }
                }
            }

            let defaults_val = Value::Array(defaults_arr.clone());
            let customs_val = Value::Array(customs_arr.clone());

            if json_mode {
                let out = serde_json::json!({
                    "builtin": defaults_val,
                    "custom": customs_val,
                    "active": active,
                });
                print_json(&out);
                return Ok(());
            }

            println!("Built-in profiles:");
            for v in &defaults_arr {
                let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                let states = assignments
                    .profile_states
                    .get(id)
                    .or_else(|| assignments.profile_states.get(name))
                    .map(|s| s.as_slice())
                    .unwrap_or(&[]);
                let tag = assignment_tag(states);
                println!("  {:<36}  {}{}", id, name, tag);
            }

            if !customs_arr.is_empty() {
                println!("\nCustom profiles:");
                for v in &customs_arr {
                    let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("");
                    let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
                    let states = assignments
                        .profile_states
                        .get(id)
                        .or_else(|| assignments.profile_states.get(name))
                        .map(|s| s.as_slice())
                        .unwrap_or(&[]);
                    let tag = assignment_tag(states);
                    println!("  {:<36}  {}{}", id, name, tag);
                }
            }

            if let Some(ref act) = active {
                println!("\nActive: {} ({})", profile_name(act), profile_id(act));
            }

            Ok(())
        }
        ProfileAction::Get { id } => {
            let prof = if let Some(ref prof_id) = id {
                let find_in = |raw: Result<String, zbus::Error>| -> Option<Value> {
                    let s = raw.ok()?;
                    let val: Value = serde_json::from_str(&s).ok()?;
                    val.as_array()?
                        .iter()
                        .find(|item| item.get("id").and_then(|s| s.as_str()) == Some(prof_id))
                        .cloned()
                };
                let def_res = with_rpc_timeout(client.proxy().get_default_profiles_json()).await;
                let mut found = find_in(def_res);
                if found.is_none() {
                    let cust_res = with_rpc_timeout(client.proxy().get_custom_profiles_json()).await;
                    found = find_in(cust_res);
                }
                if found.is_none() {
                    let all_res = with_rpc_timeout(client.proxy().get_profiles_json()).await;
                    found = find_in(all_res);
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
            let res = with_rpc_timeout(client.proxy().set_active_profile(id))
                .await
                .unwrap_or(false);
            ok(res)
        }
        ProfileAction::Defaults => {
            let def_raw = with_rpc_timeout(client.proxy().get_default_profiles_json())
                .await
                .map_err(|_| 1)?;
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
            let cust_raw = with_rpc_timeout(client.proxy().get_custom_profiles_json())
                .await
                .map_err(|_| 1)?;
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
            let res = with_rpc_timeout(client.proxy().apply_profile(json))
                .await
                .unwrap_or(false);
            ok(res)
        }
        ProfileAction::Save { json } => {
            let res = with_rpc_timeout(client.proxy().save_custom_profile(json))
                .await
                .unwrap_or(false);
            ok(res)
        }
        ProfileAction::Delete { id } => {
            let res = with_rpc_timeout(client.proxy().delete_custom_profile(id))
                .await
                .unwrap_or(false);
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
        if let Some(epp) = cpu
            .get("energyPerformancePreference")
            .or_else(|| cpu.get("epp"))
            .and_then(|v| v.as_str())
        {
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
        let use_brightness = disp
            .get("useBrightness")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if use_brightness {
            if let Some(b) = disp.get("brightness").and_then(|v| v.as_i64()) {
                println!("  {:<24} {} %", "Display brightness:", b);
            }
        }
    }

    if let Some(limits) = obj.get("odmPowerLimits").and_then(|v| {
        v.as_array()
            .or_else(|| v.get("tdpValues").and_then(|t| t.as_array()))
    }) {
        if !limits.is_empty() {
            let vals: Vec<String> = limits
                .iter()
                .filter_map(|v| v.as_i64().map(|n| format!("{}W", n)))
                .collect();
            println!("  {:<24} {}", "Power limits (PL1/2/4):", vals.join(" / "));
        }
    }

    if let Some(kp) = obj.get("selectedKeyboardProfile").and_then(|v| v.as_str()) {
        if !kp.is_empty() {
            println!("  {:<24} {}", "Keyboard profile:", kp);
        }
    }
}
