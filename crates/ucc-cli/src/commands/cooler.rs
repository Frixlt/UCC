//! Cooler (liquid cooling / LCT) command implementation.

use crate::cli::CoolerAction;
use crate::client::{with_write_rpc_timeout, CliClient};
use crate::format::*;
use serde_json::json;

pub async fn run_cooler(
    client: &CliClient,
    action: &CoolerAction,
    json_mode: bool,
) -> Result<(), i32> {
    match action {
        CoolerAction::Status => {
            let wc = client.get_water_cooler_metrics().await;

            if json_mode {
                let out = json!({
                    "supported": wc.supported,
                    "connected": wc.connected,
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
            print_val_bool("Connected:", Some(wc.connected));
            print_val_bool("Enabled:", wc.enabled);
            if wc.connected || wc.enabled.unwrap_or(false) {
                if let Some(s) = wc.fan_speed.filter(|_| wc.connected) {
                    println!("  {:<24} {} %", "Fan speed:", s);
                } else {
                    println!("  {:<24} not connected", "Fan speed:");
                }
                if let Some(p) = wc.pump_level.filter(|_| wc.connected) {
                    println!("  {:<24} {}", "Pump level:", p);
                } else {
                    println!("  {:<24} not connected", "Pump level:");
                }
            }

            Ok(())
        }
        CoolerAction::Enable => {
            let res = with_write_rpc_timeout(client.proxy().enable_water_cooler(true))
                .await
                .unwrap_or(false);
            ok(res)
        }
        CoolerAction::Disable => {
            let res = with_write_rpc_timeout(client.proxy().enable_water_cooler(false))
                .await
                .unwrap_or(false);
            ok(res)
        }
        CoolerAction::Fan { percent } => {
            let res = with_write_rpc_timeout(client.proxy().set_water_cooler_fan_speed(*percent))
                .await
                .unwrap_or(false);
            ok(res)
        }
        CoolerAction::Pump { code } => {
            if !matches!(*code, 0 | 2 | 3 | 4) {
                eprintln!(
                    "Error: invalid pump voltage code {code} (allowed: 0=11V High, 2=7V Low, 3=8V Med, 4=Off; 1=12V rejected)"
                );
                return Err(1);
            }
            let res = with_write_rpc_timeout(client.proxy().set_water_cooler_pump_voltage(*code))
                .await
                .unwrap_or(false);
            ok(res)
        }
        CoolerAction::Led { r, g, b, mode } => {
            let res = with_write_rpc_timeout(
                client.proxy().set_water_cooler_led_color(*r, *g, *b, *mode),
            )
            .await
            .unwrap_or(false);
            ok(res)
        }
        CoolerAction::LedOff => {
            let res = with_write_rpc_timeout(client.proxy().turn_off_water_cooler_led())
                .await
                .unwrap_or(false);
            ok(res)
        }
    }
}
