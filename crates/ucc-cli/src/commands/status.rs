//! Status command implementation.

use serde_json::json;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_status(client: &CliClient, json_mode: bool) -> Result<(), i32> {
    let sys_info = client.get_system_info_json().await;
    let power_state = client.get_power_state().await;
    let active_profile = client.get_active_profile_json().await;
    let power_policy = client.get_power_policy_json().await;
    let cpu = client.get_cpu_metrics().await;
    let gpu = client.get_gpu_metrics().await;
    let igpu = client.get_igpu_metrics().await;
    let hw = client.get_hardware_metrics().await;
    let wc = client.get_water_cooler_metrics().await;
    let ch = client.get_charging_metrics().await;

    if json_mode {
        let mut root = json!({
            "connected": true,
        });

        if let Some(ref ps) = power_state {
            root["powerState"] = json!(ps);
        }
        if let Some(ref prof) = active_profile {
            root["activeProfile"] = prof.clone();
        }
        if let Some(ref pol) = power_policy {
            root["powerPolicy"] = pol.clone();
        }

        let mut cpu_obj = json!({});
        if let Some(t) = cpu.temperature { cpu_obj["temperature"] = json!(t); }
        if let Some(f) = cpu.frequency { cpu_obj["frequency"] = json!(f); }
        if let Some(p) = cpu.power { cpu_obj["power"] = json!(p); }
        if let Some(fp) = cpu.fan_percent { cpu_obj["fanPercent"] = json!(fp); }
        if let Some(fr) = cpu.fan_rpm { cpu_obj["fanRPM"] = json!(fr); }
        root["cpu"] = cpu_obj;

        let mut gpu_obj = json!({});
        if let Some(t) = gpu.temperature { gpu_obj["temperature"] = json!(t); }
        if let Some(f) = gpu.frequency { gpu_obj["frequency"] = json!(f); }
        if let Some(p) = gpu.power { gpu_obj["power"] = json!(p); }
        if let Some(fp) = gpu.fan_percent { gpu_obj["fanPercent"] = json!(fp); }
        if let Some(fr) = gpu.fan_rpm { gpu_obj["fanRPM"] = json!(fr); }
        root["gpu"] = gpu_obj;

        let mut igpu_obj = json!({});
        if let Some(t) = igpu.temperature { igpu_obj["temperature"] = json!(t); }
        if let Some(f) = igpu.frequency { igpu_obj["frequency"] = json!(f); }
        if let Some(p) = igpu.power { igpu_obj["power"] = json!(p); }
        root["igpu"] = igpu_obj;

        let mut hw_obj = json!({});
        if let Some(b) = hw.display_brightness { hw_obj["displayBrightness"] = json!(b); }
        if let Some(w) = hw.webcam_enabled { hw_obj["webcamEnabled"] = json!(w); }
        if let Some(f) = hw.fn_lock { hw_obj["fnLock"] = json!(f); }
        root["hardware"] = hw_obj;

        let mut wc_obj = json!({
            "connected": wc.connected,
        });
        if wc.supported {
            wc_obj["supported"] = json!(true);
            if let Some(e) = wc.enabled { wc_obj["enabled"] = json!(e); }
            if let Some(s) = wc.fan_speed { wc_obj["fanSpeed"] = json!(s); }
            if let Some(p) = wc.pump_level { wc_obj["pumpLevel"] = json!(p); }
        }
        root["waterCooler"] = wc_obj;

        if ch.available {
            let mut ch_obj = json!({});
            if let Some(ref p) = ch.profile { ch_obj["profile"] = json!(p); }
            if let Some(ref pr) = ch.priority { ch_obj["priority"] = json!(pr); }
            if let Some(ref t) = ch.charge_type { ch_obj["type"] = json!(t); }
            if let Some(s) = ch.start_threshold { ch_obj["startThreshold"] = json!(s); }
            if let Some(e) = ch.end_threshold { ch_obj["endThreshold"] = json!(e); }
            root["charging"] = ch_obj;
        }

        print_json(&root);
        return Ok(());
    }

    println!("=== UCC System Status ===");
    println!();

    if let Some(ref doc) = sys_info {
        let laptop = doc.get("laptopModel").and_then(|v| v.as_str()).unwrap_or("");
        let cpu_m = doc.get("cpuModel").and_then(|v| v.as_str()).unwrap_or("");
        let dgpu_m = doc.get("dGpuModel").and_then(|v| v.as_str()).unwrap_or("");
        let igpu_m = doc.get("iGpuModel").and_then(|v| v.as_str()).unwrap_or("");

        if !laptop.is_empty() || !cpu_m.is_empty() || !dgpu_m.is_empty() || !igpu_m.is_empty() {
            println!("--- System ---");
            if !laptop.is_empty() { println!("  {:<24} {}", "Laptop model:", laptop); }
            if !cpu_m.is_empty() { println!("  {:<24} {}", "CPU:", cpu_m); }
            if !dgpu_m.is_empty() { println!("  {:<24} {}", "GPU (discrete):", dgpu_m); }
            if !igpu_m.is_empty() { println!("  {:<24} {}", "GPU (integrated):", igpu_m); }
            println!();
        }
    }

    println!("  {:<24} yes", "Daemon connected:");
    if let Some(ref ps) = power_state {
        println!("  {:<24} {} ({})", "Power state:", power_state_label(ps), ps);
    }
    if let Some(ref prof) = active_profile {
        println!("  {:<24} {}", "Active profile:", profile_name(prof));
        println!("  {:<24} {}", "Profile ID:", profile_id(prof));
    }

    println!();
    println!("--- CPU ---");
    print_val_opt("Temperature:", cpu.temperature, "°C");
    print_val_opt("Frequency:", cpu.frequency, "MHz");
    print_val_f64("Power:", cpu.power, "W");
    print_val_opt("Fan speed:", cpu.fan_percent, "%");
    print_val_opt("Fan RPM:", cpu.fan_rpm, "RPM");

    println!();
    println!("--- GPU ---");
    print_val_opt("Temperature:", gpu.temperature, "°C");
    print_val_opt("Frequency:", gpu.frequency, "MHz");
    print_val_f64("Power:", gpu.power, "W");
    print_val_opt("Fan speed:", gpu.fan_percent, "%");
    print_val_opt("Fan RPM:", gpu.fan_rpm, "RPM");

    println!();
    println!("--- iGPU ---");
    print_val_opt("Temperature:", igpu.temperature, "°C");
    print_val_opt("Frequency:", igpu.frequency, "MHz");
    print_val_f64("Power:", igpu.power, "W");

    println!();
    println!("--- Hardware ---");
    print_val_opt("Display brightness:", hw.display_brightness, "%");
    print_val_bool("Webcam enabled:", hw.webcam_enabled);
    print_val_bool("Fn Lock:", hw.fn_lock);

    if wc.supported {
        println!();
        println!("--- Water Cooler ---");
        print_val_bool("Enabled:", wc.enabled);
        if wc.enabled.unwrap_or(false) {
            print_val_opt("Fan speed:", wc.fan_speed, "%");
            print_val_opt("Pump level:", wc.pump_level, "");
        }
    }

    if ch.available {
        println!();
        println!("--- Charging ---");
        if let Some(ref p) = ch.profile { println!("  {:<24} {}", "Charging profile:", p); }
        if let Some(ref pr) = ch.priority { println!("  {:<24} {}", "Charging priority:", pr); }
        if let Some(ref t) = ch.charge_type { println!("  {:<24} {}", "Charge type:", t); }
        if let Some(s) = ch.start_threshold { println!("  {:<24} {} %", "Charge start thr.:", s); }
        if let Some(e) = ch.end_threshold { println!("  {:<24} {} %", "Charge end thr.:", e); }
    }

    println!();
    Ok(())
}
