//! Statemap command implementation.

use crate::cli::StatemapAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_statemap(client: &CliClient, action: &StatemapAction, json_mode: bool) -> Result<(), i32> {
    match action {
        StatemapAction::Get => {
            let settings = match client.get_settings_json().await {
                Some(s) => s,
                None => {
                    if json_mode {
                        println!("{{}}");
                    } else {
                        println!("No settings available");
                    }
                    return Ok(());
                }
            };

            if json_mode {
                print_json(&settings);
                return Ok(());
            }

            println!("=== Settings ===");

            if let Some(sm) = settings.get("stateMap").and_then(|v| v.as_object()) {
                println!("\n  Power state → Profile mapping:");
                for (state, prof_id) in sm {
                    let id_str = prof_id.as_str().unwrap_or("");
                    println!("    {:<24} {}", power_state_label(state), id_str);
                }
            }

            println!("\n  Feature controls:");
            if let Some(cpu) = settings.get("cpuSettingsEnabled").and_then(|v| v.as_bool()) {
                println!("    {:<24} {}", "CPU settings:", if cpu { "enabled" } else { "disabled" });
            }
            if let Some(fan) = settings.get("fanControlEnabled").and_then(|v| v.as_bool()) {
                println!("    {:<24} {}", "Fan control:", if fan { "enabled" } else { "disabled" });
            }
            if let Some(kb) = settings.get("keyboardBacklightControlSupported").and_then(|v| v.as_bool()) {
                println!("    {:<24} {}", "Keyboard backlight:", if kb { "enabled" } else { "disabled" });
            }
            if let Some(f) = settings.get("fahrenheit").and_then(|v| v.as_bool()) {
                println!("    {:<24} {}", "Temperature unit:", if f { "Fahrenheit" } else { "Celsius" });
            }

            if let Some(cp) = settings.get("chargingProfile").and_then(|v| v.as_str()) {
                if !cp.is_empty() {
                    println!("    {:<24} {}", "Charging profile:", cp);
                }
            }
            if let Some(pr) = settings.get("chargingPriority").and_then(|v| v.as_str()) {
                if !pr.is_empty() {
                    println!("    {:<24} {}", "Charging priority:", pr);
                }
            }

            Ok(())
        }
        StatemapAction::Set { state, profile_id } => {
            let res = client.proxy().set_state_map(state, profile_id).await.unwrap_or(false);
            ok(res)
        }
    }
}
