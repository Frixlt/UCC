//! Lightbar command implementation.

use serde_json::Value;
use crate::cli::LightbarAction;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_lightbar(client: &CliClient, action: &LightbarAction, json_mode: bool) -> Result<(), i32> {
    match action {
        LightbarAction::Get => {
            let raw = match client.proxy().get_lightbar_settings_json().await {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("Error: Could not retrieve lightbar settings");
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
        LightbarAction::Set { json } => {
            let res = client.proxy().set_lightbar_settings_json(json).await.unwrap_or(false);
            ok(res)
        }
        LightbarAction::Test => {
            let res = client.proxy().trigger_lightbar_animation(true).await.unwrap_or(false);
            ok(res)
        }
        LightbarAction::Notify => {
            let res = client.proxy().trigger_lightbar_animation(false).await.unwrap_or(false);
            ok(res)
        }
    }
}
