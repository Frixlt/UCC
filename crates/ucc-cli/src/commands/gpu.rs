//! GPU command implementation.

use serde_json::json;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_gpu(client: &CliClient, json_mode: bool) -> Result<(), i32> {
    let gpu = client.get_gpu_metrics().await;
    let igpu = client.get_igpu_metrics().await;

    let ctgp_avail = client.proxy().get_nvidia_power_ctrl_available().await.ok();
    let ctgp_max = client.proxy().get_nvidia_power_ctrl_max_power_limit().await.ok().filter(|&v| v > 0);
    let ctgp_def = client.proxy().get_nvidia_power_ctrl_default_power_limit().await.ok().filter(|&v| v > 0);

    let active_prof = client.get_active_profile_json().await;
    let ctgp_off = active_prof.as_ref().and_then(|p| p.get("nvidiaCTGPOffset")).and_then(|v| v.as_i64()).map(|n| n as i32);

    if json_mode {
        let out = json!({
            "dgpu": {
                "temperature": gpu.temperature,
                "frequency": gpu.frequency,
                "power": gpu.power,
                "fanPercent": gpu.fan_percent,
                "fanRPM": gpu.fan_rpm,
            },
            "nvidiaPowerControl": {
                "available": ctgp_avail,
                "maxPowerLimit": ctgp_max,
                "defaultPowerLimit": ctgp_def,
                "ctgpOffset": ctgp_off,
            },
            "igpu": {
                "temperature": igpu.temperature,
                "frequency": igpu.frequency,
                "power": igpu.power,
            }
        });
        print_json(&out);
        return Ok(());
    }

    println!("=== GPU (dGPU) ===");
    println!();
    print_val_opt("Temperature:", gpu.temperature, "°C");
    print_val_opt("Frequency:", gpu.frequency, "MHz");
    print_val_f64("Power:", gpu.power, "W");
    print_val_opt("Fan speed:", gpu.fan_percent, "%");
    print_val_opt("Fan RPM:", gpu.fan_rpm, "RPM");

    println!("\n--- NVIDIA Power Control ---");
    print_val_bool("cTGP available:", ctgp_avail);
    print_val_opt("Max power limit:", ctgp_max, "W");
    print_val_opt("Default power limit:", ctgp_def, "W");
    print_val_opt("cTGP offset:", ctgp_off, "W");

    println!("\n=== iGPU ===");
    println!();
    print_val_opt("Temperature:", igpu.temperature, "°C");
    print_val_opt("Frequency:", igpu.frequency, "MHz");
    print_val_f64("Power:", igpu.power, "W");

    Ok(())
}
