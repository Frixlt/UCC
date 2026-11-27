//! Power devices command implementation.

use crate::cli::DevicesAction;
use crate::client::{with_rpc_timeout, CliClient};
use crate::format::*;
use serde_json::Value;

pub async fn run_devices(
    client: &CliClient,
    action: &DevicesAction,
    json_mode: bool,
) -> Result<(), i32> {
    match action {
        DevicesAction::Get => {
            let raw = match with_rpc_timeout(client.proxy().get_power_device_settings_json()).await
            {
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
        DevicesAction::Status => {
            let raw = match with_rpc_timeout(client.proxy().get_power_device_status_json()).await {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("Error: Could not retrieve power device status");
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
            let res = with_rpc_timeout(client.proxy().scan_power_devices())
                .await
                .unwrap_or(false);
            ok(res)
        }
        DevicesAction::Set { json } => {
            let res = with_rpc_timeout(client.proxy().set_power_device_settings_json(json))
                .await
                .unwrap_or(false);
            ok(res)
        }
    }
}
