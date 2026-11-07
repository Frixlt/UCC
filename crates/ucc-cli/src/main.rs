use std::collections::HashMap;
use std::env;
use std::process::ExitCode;
use ucc_dbus::{UccdProxy, DBUS_PATH, DBUS_SERVICE};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: ucc-cli <command> [options]");
        eprintln!("Commands: status, profile, fan");
        return ExitCode::from(1);
    }

    let is_json = args.iter().any(|a| a == "--json");
    let cmd = args[1].as_str();

    let service_name = env::var("UCCD_DBUS_SERVICE").unwrap_or_else(|_| DBUS_SERVICE.to_string());
    let conn = match zbus::connection::Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error connecting to D-Bus session: {e}");
            return ExitCode::from(1);
        }
    };

    let proxy = match UccdProxy::builder(&conn)
        .destination(service_name.as_str())
        .map_err(|e| format!("{e}"))
        .and_then(|b| b.path(DBUS_PATH).map_err(|e| format!("{e}")))
    {
        Ok(b) => match b.build().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error building D-Bus proxy: {e}");
                return ExitCode::from(1);
            }
        },
        Err(e) => {
            eprintln!("Error configuring D-Bus proxy: {e}");
            return ExitCode::from(1);
        }
    };

    match cmd {
        "status" => cmd_status(&proxy, is_json).await,
        "profile" => {
            let sub = if args.len() > 2 && !args[2].starts_with("--") {
                args[2].as_str()
            } else {
                "list"
            };
            match sub {
                "list" => cmd_profile_list(&proxy, is_json).await,
                "get" => cmd_profile_get(&proxy, is_json).await,
                "set" => {
                    if args.len() < 4 {
                        eprintln!("Error: profile set requires a profile ID");
                        ExitCode::from(1)
                    } else {
                        cmd_profile_set(&proxy, &args[3]).await
                    }
                }
                unknown => {
                    eprintln!("Unknown profile subcommand: {unknown}");
                    ExitCode::from(1)
                }
            }
        }
        "fan" => {
            let sub = if args.len() > 2 && !args[2].starts_with("--") {
                args[2].as_str()
            } else {
                "list"
            };
            match sub {
                "list" | "get" => cmd_fan(&proxy, is_json).await,
                unknown => {
                    eprintln!("Unknown fan subcommand: {unknown}");
                    ExitCode::from(1)
                }
            }
        }
        "--help" | "-h" => {
            println!("ucc-cli — Command-line interface for Uniwill Control Center");
            println!("Usage: ucc-cli [command] [options]");
            println!("Commands:");
            println!("  status         Show system power, profile, fan, and hardware status");
            println!("  profile list   List built-in and custom profiles");
            println!("  profile get    Show active profile details");
            println!("  profile set    Activate profile by ID");
            println!("  fan            Show fan profiles and curve information");
            println!("Options:");
            println!("  --json         Output in machine-readable JSON format");
            ExitCode::SUCCESS
        }
        "--version" | "-v" => {
            println!("ucc-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        unknown => {
            eprintln!("Unknown command: {unknown}");
            ExitCode::from(1)
        }
    }
}

async fn cmd_status(proxy: &UccdProxy<'_>, is_json: bool) -> ExitCode {
    let power_state = match proxy.get_power_state().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error querying daemon status: {e}");
            return ExitCode::from(1);
        }
    };
    let active_profile_json = proxy.get_active_profile_json().await.unwrap_or_else(|_| "{}".to_string());
    let brightness = proxy.get_display_brightness().await.unwrap_or(-1);
    let wc_connected = proxy.get_water_cooler_connected().await.unwrap_or(false);
    let charging_profile = proxy.get_current_charging_profile().await.unwrap_or_else(|_| "unknown".to_string());

    if is_json {
        let active_profile: serde_json::Value =
            serde_json::from_str(&active_profile_json).unwrap_or(serde_json::json!({}));
        let status_json = serde_json::json!({
            "connected": true,
            "powerState": power_state,
            "activeProfile": active_profile,
            "hardware": {
                "displayBrightness": brightness
            },
            "waterCooler": {
                "connected": wc_connected
            },
            "charging": {
                "profile": charging_profile
            }
        });
        println!("{}", serde_json::to_string_pretty(&status_json).unwrap());
    } else {
        println!("=== UCC System Status ===");
        println!("  Power state:             {}", power_state);
        println!("  Display brightness:      {} %", if brightness >= 0 { brightness.to_string() } else { "n/a".to_string() });
        println!("  Water cooler:            {}", if wc_connected { "Connected" } else { "Disconnected" });
        println!("  Charging profile:        {}", charging_profile);
    }
    ExitCode::SUCCESS
}

async fn cmd_profile_list(proxy: &UccdProxy<'_>, is_json: bool) -> ExitCode {
    let profiles_json = match proxy.get_profiles_json().await {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Error retrieving profiles: {e}");
            return ExitCode::from(1);
        }
    };

    if is_json {
        let parsed: serde_json::Value = serde_json::from_str(&profiles_json).unwrap_or_else(|_| serde_json::json!([]));
        println!("{}", serde_json::to_string_pretty(&parsed).unwrap());
    } else {
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&profiles_json).unwrap_or_default();
        println!("Available Profiles ({}):", parsed.len());
        for p in parsed {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("Unnamed");
            println!("  - {:<20} (ID: {})", name, id);
        }
    }
    ExitCode::SUCCESS
}

async fn cmd_profile_get(proxy: &UccdProxy<'_>, is_json: bool) -> ExitCode {
    let profile_json = match proxy.get_active_profile_json().await {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Error retrieving active profile: {e}");
            return ExitCode::from(1);
        }
    };

    if is_json {
        let parsed: serde_json::Value = serde_json::from_str(&profile_json).unwrap_or_else(|_| serde_json::json!({}));
        println!("{}", serde_json::to_string_pretty(&parsed).unwrap());
    } else {
        let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(&profile_json).unwrap_or_default();
        println!("Active Profile:");
        for (k, v) in parsed {
            println!("  {:<20}: {}", k, v);
        }
    }
    ExitCode::SUCCESS
}

async fn cmd_profile_set(proxy: &UccdProxy<'_>, profile_id: &str) -> ExitCode {
    match proxy.set_active_profile(profile_id).await {
        Ok(true) => {
            println!("OK");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!("FAILED: Unknown profile or unauthorized");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("Error setting active profile: {e}");
            ExitCode::from(1)
        }
    }
}

async fn cmd_fan(proxy: &UccdProxy<'_>, is_json: bool) -> ExitCode {
    let fan_profile_names = match proxy.get_fan_profile_names().await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Error querying fan profile names: {e}");
            return ExitCode::from(1);
        }
    };
    let fan_profile = proxy.get_fan_profile("office").await.unwrap_or_else(|_| "{}".to_string());

    if is_json {
        let names: serde_json::Value = serde_json::from_str(&fan_profile_names).unwrap_or(serde_json::json!([]));
        let profile: serde_json::Value = serde_json::from_str(&fan_profile).unwrap_or(serde_json::json!({}));
        let out = serde_json::json!({
            "availableFanProfiles": names,
            "activeFanProfile": profile
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("=== Fan Information ===");
        println!("Available Profiles: {}", fan_profile_names);
        println!("Current Profile Data: {}", fan_profile);
    }
    ExitCode::SUCCESS
}
