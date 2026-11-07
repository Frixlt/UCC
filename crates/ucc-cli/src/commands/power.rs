//! Power command implementation.

use serde_json::Value;
use crate::cli::PowerAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_power(client: &CliClient, action: &PowerAction, json_mode: bool) -> Result<(), i32> {
    match action {
        PowerAction::Status => {
            let raw = match client.proxy().get_power_policy_json().await {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("Error: Could not retrieve power policy");
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
        PowerAction::Mode { mode } => {
            let res = client.proxy().set_manual_power_mode(mode).await.unwrap_or(false);
            ok(res)
        }
    }
}
