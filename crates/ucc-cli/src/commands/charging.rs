//! Charging command implementation.

use crate::cli::ChargingAction;
use crate::client::CliClient;
use crate::format::*;
use serde_json::{json, Value};

pub async fn run_charging(
    client: &CliClient,
    action: &ChargingAction,
    json_mode: bool,
) -> Result<(), i32> {
    match action {
        ChargingAction::Status => {
            let ch = client.get_charging_metrics().await;
            let profiles_avail_raw =
                crate::client::with_rpc_timeout(client.proxy().get_charging_profiles_available())
                    .await
                    .unwrap_or_else(|_| "[]".to_string());
            let priorities_avail_raw =
                crate::client::with_rpc_timeout(client.proxy().get_charging_priorities_available())
                    .await
                    .unwrap_or_else(|_| "[]".to_string());

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
            let res = crate::client::unwrap_write_rpc(
                crate::client::with_write_rpc_timeout(client.proxy().set_charging_profile(profile))
                    .await,
            );
            ok(res)
        }
        ChargingAction::SetPriority { priority } => {
            let res = crate::client::unwrap_write_rpc(
                crate::client::with_write_rpc_timeout(
                    client.proxy().set_charging_priority(priority),
                )
                .await,
            );
            ok(res)
        }
        ChargingAction::SetThresholds { start, end } => {
            if *start < 0 || *end < 1 || *start > 100 || *end > 100 || *start >= *end {
                eprintln!(
                    "Error: start threshold ({start}) must be >= 0 and strictly less than end threshold ({end} <= 100)"
                );
                return Err(1);
            }
            let prev_start =
                crate::client::with_rpc_timeout(client.proxy().get_charge_start_threshold())
                    .await
                    .ok()
                    .filter(|&v| v >= 0);
            let prev_end =
                crate::client::with_rpc_timeout(client.proxy().get_charge_end_threshold())
                    .await
                    .ok()
                    .filter(|&v| v >= 0);

            if *end >= prev_end.unwrap_or(0) {
                let e = crate::client::unwrap_write_rpc(
                    crate::client::with_write_rpc_timeout(
                        client.proxy().set_charge_end_threshold(*end),
                    )
                    .await,
                );
                if !e {
                    eprintln!("Error: failed to set charge end threshold");
                    return Err(1);
                }
                let s = crate::client::unwrap_write_rpc(
                    crate::client::with_write_rpc_timeout(
                        client.proxy().set_charge_start_threshold(*start),
                    )
                    .await,
                );
                if !s {
                    eprintln!(
                        "Error: failed to set charge start threshold, rolling back end threshold"
                    );
                    if let Some(prev) = prev_end {
                        match crate::client::with_write_rpc_timeout(
                            client.proxy().set_charge_end_threshold(prev),
                        )
                        .await
                        {
                            Ok(true) => {}
                            Ok(false) => eprintln!(
                                "Error: failed to roll back end threshold to {prev} (daemon rejected)"
                            ),
                            Err(err) => eprintln!(
                                "Error: failed to roll back end threshold to {prev}: {err}"
                            ),
                        }
                    }
                    return Err(1);
                }
            } else {
                let s = crate::client::unwrap_write_rpc(
                    crate::client::with_write_rpc_timeout(
                        client.proxy().set_charge_start_threshold(*start),
                    )
                    .await,
                );
                if !s {
                    eprintln!("Error: failed to set charge start threshold");
                    return Err(1);
                }
                let e = crate::client::unwrap_write_rpc(
                    crate::client::with_write_rpc_timeout(
                        client.proxy().set_charge_end_threshold(*end),
                    )
                    .await,
                );
                if !e {
                    eprintln!(
                        "Error: failed to set charge end threshold, rolling back start threshold"
                    );
                    if let Some(prev) = prev_start {
                        match crate::client::with_write_rpc_timeout(
                            client.proxy().set_charge_start_threshold(prev),
                        )
                        .await
                        {
                            Ok(true) => {}
                            Ok(false) => eprintln!(
                                "Error: failed to roll back start threshold to {prev} (daemon rejected)"
                            ),
                            Err(err) => eprintln!(
                                "Error: failed to roll back start threshold to {prev}: {err}"
                            ),
                        }
                    }
                    return Err(1);
                }
            }
            println!("OK");
            Ok(())
        }
    }
}

fn parse_json_array_to_list(raw: &str) -> String {
    if let Ok(val) = serde_json::from_str::<Value>(raw) {
        if let Some(arr) = val.as_array() {
            let items: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            return items.join(", ");
        }
    }
    String::new()
}
