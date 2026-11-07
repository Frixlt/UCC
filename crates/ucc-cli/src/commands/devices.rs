//! Power devices command implementation.

use serde_json::Value;
use crate::cli::DevicesAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_devices(client: &CliClient, action: &DevicesAction, json_mode: bool) -> Result<(), i32> {
    match action {
        DevicesAction::Get => {
            let raw = match client.proxy().get_power_device_settings_json().await {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("Error: Could not retrieve power device settings");
                    return Err(1);
                }
            };
            if json_mode {
                let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                print_json(&val);
            } else {
                println!("{}", raw);
            }
            Ok(())
        }
        DevicesAction::Scan => {
            let res = client.proxy().scan_power_devices().await.unwrap_or(false);
            ok(res)
        }
        DevicesAction::Set { json } => {
            let res = client.proxy().set_power_device_settings_json(json).await.unwrap_or(false);
            ok(res)
        }
    }
}
