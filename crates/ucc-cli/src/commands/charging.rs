//! Charging command implementation.

use serde_json::{json, Value};
use crate::cli::ChargingAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_charging(client: &CliClient, action: &ChargingAction, json_mode: bool) -> Result<(), i32> {
    match action {
        ChargingAction::Status => {
            let ch = client.get_charging_metrics().await;
            let profiles_avail_raw = client.proxy().get_charging_profiles_available().await.unwrap_or_else(|_| "[]".to_string());
            let priorities_avail_raw = client.proxy().get_charging_priorities_available().await.unwrap_or_else(|_| "[]".to_string());

            let profiles_list = parse_json_array_to_list(&profiles_avail_raw);
            let priorities_list = parse_json_array_to_list(&priorities_avail_raw);

            if json_mode {
                let out = json!({
                    "available": ch.available,
                    "profile": ch.profile,
                    "priority": ch.priority,
                    "type": ch.charge_type,
                    "startThreshold": ch.start_threshold,
                    "endThreshold": ch.end_threshold,
                    "availableProfiles": serde_json::from_str::<Value>(&profiles_avail_raw).unwrap_or(Value::Array(vec![])),
                    "availablePriorities": serde_json::from_str::<Value>(&priorities_avail_raw).unwrap_or(Value::Array(vec![])),
                });
                print_json(&out);
                return Ok(());
            }

            if profiles_list.is_empty() {
                println!("Charging control: not available on this hardware");
                return Ok(());
            }

            println!("=== Charging ===");
            if let Some(ref p) = ch.profile {
                println!("  {:<24} {}", "Charging profile:", p);
            }
            println!("  {:<24} {}", "Available profiles:", profiles_list);

            if let Some(ref pr) = ch.priority {
                println!("  {:<24} {}", "Charging priority:", pr);
            }
            if !priorities_list.is_empty() {
                println!("  {:<24} {}", "Available priorities:", priorities_list);
            }

            if let Some(ref t) = ch.charge_type {
                println!("  {:<24} {}", "Charge type:", t);
            }

            if let Some(s) = ch.start_threshold {
                println!("  {:<24} {} %", "Charge start thr.:", s);
            }
            if let Some(e) = ch.end_threshold {
                println!("  {:<24} {} %", "Charge end thr.:", e);
            }

            Ok(())
        }
        ChargingAction::SetProfile { profile } => {
            let res = client.proxy().set_charging_profile(profile).await.unwrap_or(false);
            ok(res)
        }
        ChargingAction::SetPriority { priority } => {
            let res = client.proxy().set_charging_priority(priority).await.unwrap_or(false);
            ok(res)
        }
        ChargingAction::SetThresholds { start, end } => {
            let s = client.proxy().set_charge_start_threshold(*start).await.unwrap_or(false);
            let e = client.proxy().set_charge_end_threshold(*end).await.unwrap_or(false);
            ok(s && e)
        }
    }
}

fn parse_json_array_to_list(raw: &str) -> String {
    if let Ok(val) = serde_json::from_str::<Value>(raw) {
        if let Some(arr) = val.as_array() {
            let items: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
            return items.join(", ");
        }
    }
    String::new()
}
