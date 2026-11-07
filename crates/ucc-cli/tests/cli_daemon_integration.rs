use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;
use ucc_dbus::{register_uccd_service, AllowAllAuthorizer, UccdBackend};

struct CliTestBackend;

impl UccdBackend for CliTestBackend {
    fn get_power_state(&self) -> String {
        "power_ac".to_string()
    }

    fn get_active_profile_json(&self) -> String {
        r#"{"id":"office","name":"Office Mode","tdpPl1":45,"tdpPl2":65}"#.to_string()
    }

    fn get_profiles_json(&self) -> String {
        r#"[{"id":"office","name":"Office"},{"id":"balanced","name":"Balanced"},{"id":"performance","name":"Performance"}]"#.to_string()
    }

    fn get_display_brightness(&self) -> i32 {
        85
    }

    fn get_water_cooler_connected(&self) -> bool {
        false
    }

    fn get_current_charging_profile(&self) -> String {
        "balanced".to_string()
    }

    fn get_fan_profile_names(&self) -> String {
        r#"["office","balanced","turbo"]"#.to_string()
    }

    fn get_fan_profile(&self, name: &str) -> String {
        format!(r#"{{"name":"{name}","curve":[{{"temp":40,"speed":25}},{{"temp":80,"speed":95}}]}}"#)
    }
}

async fn start_test_daemon() -> (String, zbus::connection::Connection) {
    let unique_id = format!(
        "{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bus_name = format!("com.uniwill.uccd.cli_test_{unique_id}");

    let server_conn = zbus::connection::Builder::session()
        .expect("session builder")
        .name(bus_name.as_str())
        .expect("bus name")
        .build()
        .await
        .expect("server connection");

    register_uccd_service(
        &server_conn,
        Arc::new(CliTestBackend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .expect("register_uccd_service");

    (bus_name, server_conn)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_status_human_and_json() {
    let (bus_name, _server) = start_test_daemon().await;
    let cli_bin = env!("CARGO_BIN_EXE_ucc-cli");

    // 1. Text status output
    let output = Command::new(cli_bin)
        .arg("status")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli status");

    assert!(output.status.success(), "Exit code must be 0 for status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("=== UCC System Status ==="));
    assert!(stdout.contains("power_ac"));
    assert!(stdout.contains("85 %"));

    // 2. JSON status output
    let json_output = Command::new(cli_bin)
        .arg("status")
        .arg("--json")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli status --json");

    assert!(json_output.status.success(), "Exit code must be 0 for status --json");
    let json_str = String::from_utf8_lossy(&json_output.stdout);
    let json_val: serde_json::Value =
        serde_json::from_str(&json_str).expect("stdout must be valid JSON");

    assert_eq!(json_val["connected"].as_bool(), Some(true));
    assert_eq!(json_val["powerState"].as_str(), Some("power_ac"));
    assert_eq!(json_val["activeProfile"]["id"].as_str(), Some("office"));
    assert_eq!(json_val["activeProfile"]["name"].as_str(), Some("Office Mode"));
    assert_eq!(json_val["hardware"]["displayBrightness"].as_i64(), Some(85));
    assert_eq!(json_val["waterCooler"]["connected"].as_bool(), Some(false));
    assert_eq!(json_val["charging"]["profile"].as_str(), Some("balanced"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_profile_list_human_and_json() {
    let (bus_name, _server) = start_test_daemon().await;
    let cli_bin = env!("CARGO_BIN_EXE_ucc-cli");

    // 1. Text profile list output
    let output = Command::new(cli_bin)
        .arg("profile")
        .arg("list")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli profile list");

    assert!(output.status.success(), "Exit code must be 0 for profile list");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Available Profiles"));
    assert!(stdout.contains("Office"));
    assert!(stdout.contains("Balanced"));
    assert!(stdout.contains("Performance"));

    // 2. JSON profile list output
    let json_output = Command::new(cli_bin)
        .arg("profile")
        .arg("list")
        .arg("--json")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli profile list --json");

    assert!(json_output.status.success(), "Exit code must be 0 for profile list --json");
    let json_str = String::from_utf8_lossy(&json_output.stdout);
    let json_val: serde_json::Value =
        serde_json::from_str(&json_str).expect("stdout must be valid JSON");

    let arr = json_val.as_array().expect("must be a JSON array of profiles");
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["id"].as_str(), Some("office"));
    assert_eq!(arr[1]["id"].as_str(), Some("balanced"));
    assert_eq!(arr[2]["id"].as_str(), Some("performance"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_profile_get_human_and_json() {
    let (bus_name, _server) = start_test_daemon().await;
    let cli_bin = env!("CARGO_BIN_EXE_ucc-cli");

    // 1. Text profile get
    let output = Command::new(cli_bin)
        .arg("profile")
        .arg("get")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli profile get");

    assert!(output.status.success(), "Exit code must be 0 for profile get");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Active Profile:"));
    assert!(stdout.contains("office"));

    // 2. JSON profile get
    let json_output = Command::new(cli_bin)
        .arg("profile")
        .arg("get")
        .arg("--json")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli profile get --json");

    assert!(json_output.status.success(), "Exit code must be 0 for profile get --json");
    let json_str = String::from_utf8_lossy(&json_output.stdout);
    let json_val: serde_json::Value =
        serde_json::from_str(&json_str).expect("stdout must be valid JSON");

    assert_eq!(json_val["id"].as_str(), Some("office"));
    assert_eq!(json_val["name"].as_str(), Some("Office Mode"));
    assert_eq!(json_val["tdpPl1"].as_i64(), Some(45));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_fan_human_and_json() {
    let (bus_name, _server) = start_test_daemon().await;
    let cli_bin = env!("CARGO_BIN_EXE_ucc-cli");

    // 1. Text fan output
    let output = Command::new(cli_bin)
        .arg("fan")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli fan");

    assert!(output.status.success(), "Exit code must be 0 for fan");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("=== Fan Information ==="));
    assert!(stdout.contains("Available Profiles"));

    // 2. JSON fan output
    let json_output = Command::new(cli_bin)
        .arg("fan")
        .arg("--json")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run ucc-cli fan --json");

    assert!(json_output.status.success(), "Exit code must be 0 for fan --json");
    let json_str = String::from_utf8_lossy(&json_output.stdout);
    let json_val: serde_json::Value =
        serde_json::from_str(&json_str).expect("stdout must be valid JSON");

    assert!(json_val["availableFanProfiles"].is_array());
    assert_eq!(json_val["activeFanProfile"]["name"].as_str(), Some("office"));
    assert!(json_val["activeFanProfile"]["curve"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_exit_codes_on_errors() {
    let (bus_name, _server) = start_test_daemon().await;
    let cli_bin = env!("CARGO_BIN_EXE_ucc-cli");

    // 1. Unknown command -> non-zero exit code
    let unknown_cmd = Command::new(cli_bin)
        .arg("nonexistent_command_xyz")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run unknown command");

    assert!(!unknown_cmd.status.success(), "Exit code must be non-zero for unknown command");

    // 2. Unknown subcommand -> non-zero exit code
    let unknown_sub = Command::new(cli_bin)
        .arg("profile")
        .arg("unsupported_subcmd")
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .expect("run unknown subcommand");

    assert!(!unknown_sub.status.success(), "Exit code must be non-zero for unknown subcommand");

    // 3. Daemon unavailable / unreachable service -> non-zero exit code
    let offline_output = Command::new(cli_bin)
        .arg("status")
        .env("UCCD_DBUS_SERVICE", "com.uniwill.uccd.offline_daemon_xyz")
        .output()
        .expect("run with offline daemon");

    assert!(!offline_output.status.success(), "Exit code must be non-zero when daemon is unreachable");
}
