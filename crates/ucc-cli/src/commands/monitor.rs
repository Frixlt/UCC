//! Monitor command implementation.

use std::time::Duration;
use serde_json::json;
use crate::cli::MonitorArgs;
use crate::client::CliClient;

pub async fn run_monitor(client: &CliClient, args: &MonitorArgs, json_mode: bool) -> Result<(), i32> {
    let mut remaining = args.count;
    let interval = Duration::from_secs(args.interval.max(1));
    let mut first = true;

    while args.count <= 0 || remaining > 0 {
        if !first {
            tokio::time::sleep(interval).await;
        }
        first = false;

        let cpu = client.get_cpu_metrics().await;
        let gpu = client.get_gpu_metrics().await;

        if json_mode {
            let row = json!({
                "cpu": {
                    "temperature": cpu.temperature.unwrap_or(0),
                    "frequency": cpu.frequency.unwrap_or(0),
                    "power": cpu.power.unwrap_or(0.0),
                    "fanPercent": cpu.fan_percent.unwrap_or(0),
                },
                "gpu": {
                    "temperature": gpu.temperature.unwrap_or(0),
                    "frequency": gpu.frequency.unwrap_or(0),
                    "power": gpu.power.unwrap_or(0.0),
                    "fanPercent": gpu.fan_percent.unwrap_or(0),
                }
            });
            println!("{}", row);
        } else {
            println!(
                "CPU: {:3}°C  {:5}MHz  {:5.1}W  Fan:{:3}%  |  GPU: {:3}°C  {:5}MHz  {:5.1}W  Fan:{:3}%",
                cpu.temperature.unwrap_or(0),
                cpu.frequency.unwrap_or(0),
                cpu.power.unwrap_or(0.0),
                cpu.fan_percent.unwrap_or(0),
                gpu.temperature.unwrap_or(0),
                gpu.frequency.unwrap_or(0),
                gpu.power.unwrap_or(0.0),
                gpu.fan_percent.unwrap_or(0),
            );
        }

        if args.count > 0 {
            remaining -= 1;
        }
    }

    Ok(())
}
