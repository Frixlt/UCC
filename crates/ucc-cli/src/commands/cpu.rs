//! CPU command implementation.

use serde_json::{json, Value};
use crate::cli::CpuAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_cpu(client: &CliClient, action: &Option<CpuAction>, json_mode: bool) -> Result<(), i32> {
    match action {
        None | Some(CpuAction::Status) => {
            let cores = client.proxy().get_cpu_core_count().await.ok().filter(|&c| c > 0);
            let cpu = client.get_cpu_metrics().await;
            let govs_raw = client.proxy().get_available_governors().await.unwrap_or_else(|_| "[]".to_string());
            let epps_raw = client.proxy().get_available_epps().await.unwrap_or_else(|_| "[]".to_string());
            let limits_raw = client.proxy().get_cpu_frequency_limits_json().await.unwrap_or_else(|_| "{}".to_string());

            let govs_val: Value = serde_json::from_str(&govs_raw).unwrap_or(Value::Array(vec![]));
            let epps_val: Value = serde_json::from_str(&epps_raw).unwrap_or(Value::Array(vec![]));
            let limits_val: Value = serde_json::from_str(&limits_raw).unwrap_or(Value::Object(serde_json::Map::new()));

            let min_mhz = limits_val.get("min").and_then(|v| v.as_i64()).map(|k| (k / 1000) as i32);
            let max_mhz = limits_val.get("max").and_then(|v| v.as_i64()).map(|k| (k / 1000) as i32);

            if json_mode {
                let out = json!({
                    "coreCount": cores,
                    "temperature": cpu.temperature,
                    "frequency": cpu.frequency,
                    "power": cpu.power,
                    "availableGovernors": govs_val,
                    "availableEPPs": epps_val,
                    "minFrequencyMHz": min_mhz,
                    "maxFrequencyMHz": max_mhz,
                });
                print_json(&out);
                return Ok(());
            }

            println!("=== CPU Info ===");
            print_val_opt("Core count:", cores, "");
            print_val_opt("Temperature:", cpu.temperature, "°C");
            print_val_opt("Frequency:", cpu.frequency, "MHz");
            print_val_f64("Power:", cpu.power, "W");

            if let Some(arr) = govs_val.as_array() {
                let items: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                if !items.is_empty() {
                    println!("  {:<24} {}", "Available governors:", items.join(", "));
                }
            }

            if let Some(arr) = epps_val.as_array() {
                let items: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                if !items.is_empty() {
                    println!("  {:<24} {}", "Available EPPs:", items.join(", "));
                }
            }

            if let Some(min) = min_mhz {
                println!("  {:<24} {} MHz", "Min frequency:", min);
            }
            if let Some(max) = max_mhz {
                println!("  {:<24} {} MHz", "Max frequency:", max);
            }

            Ok(())
        }
        Some(CpuAction::Governor { name }) => {
            if let Some(ref gov) = name {
                // If setting governor, send request or apply
                let res = client.proxy().set_manual_power_mode(gov).await.unwrap_or(false);
                ok(res)
            } else {
                let govs_raw = client.proxy().get_available_governors().await.map_err(|_| 1)?;
                let val: Value = serde_json::from_str(&govs_raw).unwrap_or(Value::Array(vec![]));
                if json_mode {
                    print_json(&val);
                } else if let Some(arr) = val.as_array() {
                    let items: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                    println!("{}", items.join(", "));
                }
                Ok(())
            }
        }
        Some(CpuAction::Epp { preference }) => {
            if let Some(ref _epp) = preference {
                // EPP setting
                ok(true)
            } else {
                let epps_raw = client.proxy().get_available_epps().await.map_err(|_| 1)?;
                let val: Value = serde_json::from_str(&epps_raw).unwrap_or(Value::Array(vec![]));
                if json_mode {
                    print_json(&val);
                } else if let Some(arr) = val.as_array() {
                    let items: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                    println!("{}", items.join(", "));
                }
                Ok(())
            }
        }
        Some(CpuAction::Frequency { min, max }) => {
            println!("Setting CPU frequency: {} - {} MHz", min, max);
            ok(true)
        }
    }
}
