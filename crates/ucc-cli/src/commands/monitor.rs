//! Monitor command implementation.

use crate::cli::MonitorArgs;
use crate::client::CliClient;
use serde_json::json;
use std::time::Duration;

pub async fn run_monitor(
    client: &CliClient,
    args: &MonitorArgs,
    json_mode: bool,
) -> Result<(), i32> {
    if args.count < 0 {
        eprintln!("Error: --count must be >= 0 (0 = continuous)");
        return Err(1);
    }
    let mut remaining = args.count;
    let interval = Duration::from_secs(args.interval.max(1));
    let mut first = true;

    while args.count <= 0 || remaining > 0 {
        if !first {
            tokio::time::sleep(interval).await;
        }
        first = false;

        let (cpu, gpu) = tokio::join!(client.get_cpu_metrics(), client.get_gpu_metrics(),);

        if json_mode {
            let row = json!({
                "cpu": {
                    "temperature": cpu.temperature,
                    "frequency": cpu.frequency,
                    "power": cpu.power,
                    "fanPercent": cpu.fan_percent,
                },
                "gpu": {
                    "temperature": gpu.temperature,
                    "frequency": gpu.frequency,
                    "power": gpu.power,
                    "fanPercent": gpu.fan_percent,
                }
            });
            println!("{}", row);
        } else {
            let fmt_i = |v: Option<i32>| -> String {
                v.map_or_else(|| "N/A".to_string(), |n| n.to_string())
            };
            let fmt_f = |v: Option<f64>| -> String {
                v.map_or_else(|| "N/A".to_string(), |n| format!("{n:.1}"))
            };
            println!(
                "CPU: {:>3}°C  {:>5}MHz  {:>5}W  Fan:{:>3}%  |  GPU: {:>3}°C  {:>5}MHz  {:>5}W  Fan:{:>3}%",
                fmt_i(cpu.temperature),
                fmt_i(cpu.frequency),
                fmt_f(cpu.power),
                fmt_i(cpu.fan_percent),
                fmt_i(gpu.temperature),
                fmt_i(gpu.frequency),
                fmt_f(gpu.power),
                fmt_i(gpu.fan_percent),
            );
        }

        if args.count > 0 {
            remaining -= 1;
        }
    }

    Ok(())
}
