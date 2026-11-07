//! Hardware toggle commands (brightness, webcam, fnlock).

use serde_json::json;
use crate::cli::{BrightnessAction, ToggleAction};
use crate::client::CliClient;
use crate::format::*;

pub async fn run_brightness(client: &CliClient, action: &BrightnessAction, json_mode: bool) -> Result<(), i32> {
    match action {
        BrightnessAction::Get => {
            let b = match client.proxy().get_display_brightness().await {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("Error: Could not read display brightness");
                    return Err(1);
                }
            };
            if json_mode {
                println!("{}", json!({ "brightness": b }));
            } else {
                println!("{}", b);
            }
            Ok(())
        }
        BrightnessAction::Set { value } => {
            let res = client.proxy().set_display_brightness(*value).await.unwrap_or(false);
            ok(res)
        }
    }
}

pub async fn run_webcam(client: &CliClient, action: &ToggleAction, json_mode: bool) -> Result<(), i32> {
    match action {
        ToggleAction::Get => {
            let status = match client.proxy().get_webcam_sw_status().await {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("Error: Could not read webcam status");
                    return Err(1);
                }
            };
            if json_mode {
                println!("{}", json!({ "webcamEnabled": status }));
            } else {
                println!("{}", if status { "enabled" } else { "disabled" });
            }
            Ok(())
        }
        ToggleAction::Set { value } => {
            let enable = match parse_bool_arg(value) {
                Some(b) => b,
                None => {
                    eprintln!("Error: expected on/off");
                    return Err(1);
                }
            };
            let res = client.proxy().set_webcam(enable).await.unwrap_or(false);
            ok(res)
        }
    }
}

pub async fn run_fnlock(client: &CliClient, action: &ToggleAction, json_mode: bool) -> Result<(), i32> {
    match action {
        ToggleAction::Get => {
            let status = match client.proxy().get_fn_lock_status().await {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("Error: Could not read Fn Lock status");
                    return Err(1);
                }
            };
            if json_mode {
                println!("{}", json!({ "fnLock": status }));
            } else {
                println!("{}", if status { "on" } else { "off" });
            }
            Ok(())
        }
        ToggleAction::Set { value } => {
            let enable = match parse_bool_arg(value) {
                Some(b) => b,
                None => {
                    eprintln!("Error: expected on/off");
                    return Err(1);
                }
            };
            let res = client.proxy().set_fn_lock_status(enable).await.is_ok();
            ok(res)
        }
    }
}
