//! Integration tests for UCC CLI (clap parsing, aliases, exit codes, completions, manpages).

use clap::Parser;
use ucc_cli::cli::{Cli, Commands, ProfileAction, CoolerAction, ChargingAction, KeyboardAction, FanAction};
use ucc_cli::format::*;
use ucc_cli::run_cli;

#[tokio::test]
async fn test_help_and_version_exit_code_zero() {
    assert_eq!(run_cli(["ucc-cli", "--help"]).await, 0);
    assert_eq!(run_cli(["ucc-cli", "-h"]).await, 0);
    assert_eq!(run_cli(["ucc-cli", "--version"]).await, 0);
    assert_eq!(run_cli(["ucc-cli", "-V"]).await, 0);
}

#[tokio::test]
async fn test_no_args_exit_code_one() {
    assert_eq!(run_cli(["ucc-cli"]).await, 1);
}

#[tokio::test]
async fn test_invalid_arguments_exit_code_one() {
    assert_eq!(run_cli(["ucc-cli", "--nonexistent-flag"]).await, 1);
    assert_eq!(run_cli(["ucc-cli", "unknown-command"]).await, 1);
    assert_eq!(run_cli(["ucc-cli", "profile", "nonexistent-sub"]).await, 1);
    // Missing required argument for set
    assert_eq!(run_cli(["ucc-cli", "profile", "set"]).await, 1);
    // Missing required arguments for statemap set
    assert_eq!(run_cli(["ucc-cli", "statemap", "set"]).await, 1);
    // Missing percent for cooler fan
    assert_eq!(run_cli(["ucc-cli", "cooler", "fan"]).await, 1);
    // Missing RGB for keyboard color
    assert_eq!(run_cli(["ucc-cli", "keyboard", "color"]).await, 1);
}

#[tokio::test]
async fn test_completions_offline_exit_code_zero() {
    assert_eq!(run_cli(["ucc-cli", "completions", "bash"]).await, 0);
    assert_eq!(run_cli(["ucc-cli", "completions", "zsh"]).await, 0);
    assert_eq!(run_cli(["ucc-cli", "completions", "fish"]).await, 0);
}

#[tokio::test]
async fn test_manpage_offline_exit_code_zero() {
    assert_eq!(run_cli(["ucc-cli", "man"]).await, 0);
}

#[test]
fn test_cli_parsing_aliases() {
    // prof alias
    let cli = Cli::try_parse_from(["ucc-cli", "prof", "list"]).expect("parse prof list");
    match cli.command {
        Some(Commands::Profile { action: ProfileAction::List }) => {}
        other => panic!("expected Profile::List, got {:?}", other),
    }

    // state-map alias
    let cli = Cli::try_parse_from(["ucc-cli", "state-map", "get"]).expect("parse state-map get");
    assert!(matches!(cli.command, Some(Commands::Statemap { .. })));

    // kb alias
    let cli = Cli::try_parse_from(["ucc-cli", "kb", "info"]).expect("parse kb info");
    assert!(matches!(cli.command, Some(Commands::Keyboard { .. })));

    // br alias
    let cli = Cli::try_parse_from(["ucc-cli", "br", "get"]).expect("parse br get");
    assert!(matches!(cli.command, Some(Commands::Brightness { .. })));

    // wc / watercooler aliases
    let cli_wc = Cli::try_parse_from(["ucc-cli", "wc", "status"]).expect("parse wc status");
    assert!(matches!(cli_wc.command, Some(Commands::Cooler { .. })));
    let cli_watercooler = Cli::try_parse_from(["ucc-cli", "watercooler", "status"]).expect("parse watercooler status");
    assert!(matches!(cli_watercooler.command, Some(Commands::Cooler { .. })));

    // charge alias
    let cli = Cli::try_parse_from(["ucc-cli", "charge", "status"]).expect("parse charge status");
    assert!(matches!(cli.command, Some(Commands::Charging { .. })));

    // mon alias
    let cli = Cli::try_parse_from(["ucc-cli", "mon", "-n", "5", "-i", "3"]).expect("parse mon");
    match cli.command {
        Some(Commands::Monitor(m)) => {
            assert_eq!(m.count, 5);
            assert_eq!(m.interval, 3);
        }
        other => panic!("expected Monitor, got {:?}", other),
    }

    // odm alias
    let cli = Cli::try_parse_from(["ucc-cli", "odm"]).expect("parse odm");
    assert!(matches!(cli.command, Some(Commands::PowerLimits)));

    // fn-lock alias
    let cli = Cli::try_parse_from(["ucc-cli", "fn-lock", "get"]).expect("parse fn-lock");
    assert!(matches!(cli.command, Some(Commands::Fnlock { .. })));
}

#[test]
fn test_subcommand_aliases() {
    // profile aliases
    let cli = Cli::try_parse_from(["ucc-cli", "profile", "ls"]).expect("profile ls");
    assert!(matches!(cli.command, Some(Commands::Profile { action: ProfileAction::List })));

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "show"]).expect("profile show");
    assert!(matches!(cli.command, Some(Commands::Profile { action: ProfileAction::Get { .. } })));

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "active"]).expect("profile active");
    assert!(matches!(cli.command, Some(Commands::Profile { action: ProfileAction::Get { .. } })));

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "activate", "p1"]).expect("profile activate");
    match cli.command {
        Some(Commands::Profile { action: ProfileAction::Set { id } }) => assert_eq!(id, "p1"),
        other => panic!("expected Set p1, got {:?}", other),
    }

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "default"]).expect("profile default");
    assert!(matches!(cli.command, Some(Commands::Profile { action: ProfileAction::Defaults })));

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "custom"]).expect("profile custom");
    assert!(matches!(cli.command, Some(Commands::Profile { action: ProfileAction::Customs })));

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "del", "p2"]).expect("profile del");
    match cli.command {
        Some(Commands::Profile { action: ProfileAction::Delete { id } }) => assert_eq!(id, "p2"),
        other => panic!("expected Delete p2, got {:?}", other),
    }

    let cli = Cli::try_parse_from(["ucc-cli", "profile", "rm", "p3"]).expect("profile rm");
    match cli.command {
        Some(Commands::Profile { action: ProfileAction::Delete { id } }) => assert_eq!(id, "p3"),
        other => panic!("expected Delete p3, got {:?}", other),
    }

    // fan aliases
    let cli = Cli::try_parse_from(["ucc-cli", "fan", "ls"]).expect("fan ls");
    assert!(matches!(cli.command, Some(Commands::Fan { action: FanAction::List })));

    let cli = Cli::try_parse_from(["ucc-cli", "fan", "show", "f1"]).expect("fan show");
    match cli.command {
        Some(Commands::Fan { action: FanAction::Get { id } }) => assert_eq!(id, "f1"),
        other => panic!("expected Fan::Get f1, got {:?}", other),
    }

    // keyboard aliases
    let cli = Cli::try_parse_from(["ucc-cli", "keyboard", "caps"]).expect("keyboard caps");
    assert!(matches!(cli.command, Some(Commands::Keyboard { action: KeyboardAction::Info })));

    let cli = Cli::try_parse_from(["ucc-cli", "keyboard", "profile-list"]).expect("keyboard profile-list");
    assert!(matches!(cli.command, Some(Commands::Keyboard { action: KeyboardAction::Profiles })));

    let cli = Cli::try_parse_from(["ucc-cli", "keyboard", "use", "k1"]).expect("keyboard use");
    match cli.command {
        Some(Commands::Keyboard { action: KeyboardAction::Activate { id } }) => assert_eq!(id, "k1"),
        other => panic!("expected Keyboard::Activate k1, got {:?}", other),
    }

    // cooler aliases
    let cli = Cli::try_parse_from(["ucc-cli", "cooler", "on"]).expect("cooler on");
    assert!(matches!(cli.command, Some(Commands::Cooler { action: CoolerAction::Enable })));

    let cli = Cli::try_parse_from(["ucc-cli", "cooler", "off"]).expect("cooler off");
    assert!(matches!(cli.command, Some(Commands::Cooler { action: CoolerAction::Disable })));

    let cli = Cli::try_parse_from(["ucc-cli", "cooler", "led_off"]).expect("cooler led_off");
    assert!(matches!(cli.command, Some(Commands::Cooler { action: CoolerAction::LedOff })));

    // charging aliases
    let cli = Cli::try_parse_from(["ucc-cli", "charging", "set_profile", "bal"]).expect("charging set_profile");
    match cli.command {
        Some(Commands::Charging { action: ChargingAction::SetProfile { profile } }) => assert_eq!(profile, "bal"),
        other => panic!("expected SetProfile bal, got {:?}", other),
    }
}

#[test]
fn test_global_json_flag_parsing() {
    let cli = Cli::try_parse_from(["ucc-cli", "--json", "status"]).expect("parse --json status");
    assert!(cli.json);
    assert!(matches!(cli.command, Some(Commands::Status)));

    let cli_short = Cli::try_parse_from(["ucc-cli", "-j", "status"]).expect("parse -j status");
    assert!(cli_short.json);
}

#[test]
fn test_formatting_and_parsing_helpers() {
    assert_eq!(tdp_label(0), "PL1 (Sustained):");
    assert_eq!(tdp_label(1), "PL2 (Boost):");
    assert_eq!(tdp_label(2), "PL4 (Peak):");

    assert_eq!(power_state_label("power_ac"), "AC");
    assert_eq!(power_state_label("power_bat"), "Battery");
    assert_eq!(power_state_label("power_wc"), "AC w/ Water Cooler");
    assert_eq!(power_state_label("power_type_c"), "Type-C");

    assert_eq!(state_short_label("power_ac"), "AC");
    assert_eq!(state_short_label("power_wc"), "WC");

    let tags = assignment_tag(&["power_ac".to_string(), "power_wc".to_string()]);
    assert_eq!(tags, " [AC, WC]");

    let empty_tag = assignment_tag(&[]);
    assert_eq!(empty_tag, "");

    assert_eq!(parse_bool_arg("on"), Some(true));
    assert_eq!(parse_bool_arg("true"), Some(true));
    assert_eq!(parse_bool_arg("1"), Some(true));
    assert_eq!(parse_bool_arg("off"), Some(false));
    assert_eq!(parse_bool_arg("false"), Some(false));
    assert_eq!(parse_bool_arg("0"), Some(false));
    assert_eq!(parse_bool_arg("invalid"), None);
}

#[test]
fn test_completion_generation_output() {
    let mut buf = Vec::new();
    ucc_cli::cli::generate_completions(clap_complete::Shell::Bash, &mut buf);
    let out = String::from_utf8(buf).expect("valid utf-8 bash completions");
    assert!(out.contains("ucc__cli"));
    assert!(out.contains("status"));
    assert!(out.contains("monitor"));
    assert!(out.contains("profile"));
    assert!(out.contains("cooler"));
    assert!(out.contains("charging"));
    assert!(out.contains("--json"));
}

#[test]
fn test_manpage_generation_output() {
    let mut buf = Vec::new();
    ucc_cli::cli::generate_manpage(&mut buf).expect("generate manpage");
    let out = String::from_utf8(buf).expect("valid utf-8 manpage");
    assert!(out.contains(".TH ucc-cli 1"));
    assert!(out.contains("Command\\-line interface for Frixit / Uniwill Control Center"));
    assert!(out.contains("status"));
    assert!(out.contains("profile"));
    assert!(out.contains("cooler"));
}
