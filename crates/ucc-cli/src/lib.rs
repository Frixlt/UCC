//! UCC command-line client library and runner.

pub mod cli;
pub mod client;
pub mod commands;
pub mod format;

use std::ffi::OsString;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use crate::cli::{Cli, Commands};
use crate::client::CliClient;

/// Execute the CLI with provided arguments, returning an exit code.
/// 0 = success
/// 1 = argument or execution error
/// 2 = D-Bus connection failure
pub async fn run_cli<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(err) => {
            if err.kind() == ErrorKind::DisplayHelp || err.kind() == ErrorKind::DisplayVersion {
                print!("{}", err);
                return 0;
            } else {
                eprint!("{}", err);
                return 1;
            }
        }
    };

    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            let mut cmd = Cli::command();
            let _ = cmd.print_help();
            println!();
            return 1;
        }
    };

    // Commands that don't need D-Bus connection
    match &command {
        Commands::Completions { shell } => {
            cli::generate_completions(*shell, &mut std::io::stdout());
            return 0;
        }
        Commands::Man => {
            if let Err(e) = cli::generate_manpage(&mut std::io::stdout()) {
                eprintln!("Error generating manpage: {}", e);
                return 1;
            }
            return 0;
        }
        _ => {}
    }

    // Connect to D-Bus
    let client = match CliClient::connect().await {
        Ok(c) => c,
        Err(code) => return code,
    };

    let result = match &command {
        Commands::Status => commands::status::run_status(&client, cli.json).await,
        Commands::Monitor(ref m) => commands::monitor::run_monitor(&client, m, cli.json).await,
        Commands::Profile { ref action } => {
            let act = action.as_ref().unwrap_or(&cli::ProfileAction::List);
            commands::profile::run_profile(&client, act, cli.json).await
        }
        Commands::Statemap { ref action } => {
            let act = action.as_ref().unwrap_or(&cli::StatemapAction::Get);
            commands::statemap::run_statemap(&client, act, cli.json).await
        }
        Commands::Fan { ref action } => {
            let act = action.as_ref().unwrap_or(&cli::FanAction::List);
            commands::fan::run_fan(&client, act, cli.json).await
        }
        Commands::Keyboard { ref action } => {
            let act = action.as_ref().unwrap_or(&cli::KeyboardAction::Info);
            commands::keyboard::run_keyboard(&client, act, cli.json).await
        }
        Commands::Cooler { ref action } => {
            let act = action.as_ref().unwrap_or(&cli::CoolerAction::Status);
            commands::cooler::run_cooler(&client, act, cli.json).await
        }
        Commands::Charging { ref action } => {
            let act = action.as_ref().unwrap_or(&cli::ChargingAction::Status);
            commands::charging::run_charging(&client, act, cli.json).await
        }
        Commands::Cpu { ref action } => {
            commands::cpu::run_cpu(&client, action, cli.json).await
        }
        Commands::Gpu => commands::gpu::run_gpu(&client, cli.json).await,
        Commands::PowerLimits => commands::power_limits::run_power_limits(&client, cli.json).await,
        Commands::Brightness { ref action } => commands::hardware::run_brightness(&client, action, cli.json).await,
        Commands::Webcam { ref action } => commands::hardware::run_webcam(&client, action, cli.json).await,
        Commands::Fnlock { ref action } => commands::hardware::run_fnlock(&client, action, cli.json).await,
        Commands::Lightbar { ref action } => commands::lightbar::run_lightbar(&client, action, cli.json).await,
        Commands::Devices { ref action } => commands::devices::run_devices(&client, action, cli.json).await,
        Commands::Power { ref action } => commands::power::run_power(&client, action, cli.json).await,
        Commands::Completions { .. } | Commands::Man => unreachable!(),
    };

    match result {
        Ok(()) => 0,
        Err(code) => code,
    }
}
