//! Cooler (liquid cooling / LCT) command implementation.

use serde_json::json;
use crate::cli::CoolerAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_cooler(client: &CliClient, action: &CoolerAction, json_mode: bool) -> Result<(), i32> {
    match action {
        CoolerAction::Status => {
            let wc = client.get_water_cooler_metrics().await;

            if json_mode {
                let out = json!({
                    "supported": wc.supported,
                    "enabled": wc.enabled.unwrap_or(false),
                    "fanSpeed": wc.fan_speed,
                    "pumpLevel": wc.pump_level,
                });
                print_json(&out);
                return Ok(());
            }

            if !wc.supported {
                println!("Water cooler: not supported");
                return Ok(());
            }

            println!("=== Water Cooler ===");
            print_val_bool("Supported:", Some(wc.supported));
            print_val_bool("Enabled:", wc.enabled);
            if wc.enabled.unwrap_or(false) {
                if let Some(s) = wc.fan_speed {
                    println!("  {:<24} {} %", "Fan speed:", s);
                } else {
                    println!("  {:<24} not connected", "Fan speed:");
                }
                if let Some(p) = wc.pump_level {
                    println!("  {:<24} {}", "Pump level:", p);
                } else {
                    println!("  {:<24} not connected", "Pump level:");
                }
            }

            Ok(())
        }
        CoolerAction::Enable => {
            let res = client.proxy().enable_water_cooler(true).await.unwrap_or(false);
            ok(res)
        }
        CoolerAction::Disable => {
            let res = client.proxy().enable_water_cooler(false).await.unwrap_or(false);
            ok(res)
        }
        CoolerAction::Fan { percent } => {
            let res = client.proxy().set_water_cooler_fan_speed(*percent).await.unwrap_or(false);
            ok(res)
        }
        CoolerAction::Pump { code } => {
            let res = client.proxy().set_water_cooler_pump_voltage(*code).await.unwrap_or(false);
            ok(res)
        }
        CoolerAction::Led { r, g, b, mode } => {
            let res = client.proxy().set_water_cooler_led_color(*r, *g, *b, *mode).await.unwrap_or(false);
            ok(res)
        }
        CoolerAction::LedOff => {
            let res = client.proxy().turn_off_water_cooler_led().await.unwrap_or(false);
            ok(res)
        }
    }
}
