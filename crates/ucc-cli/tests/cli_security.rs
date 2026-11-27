// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use ucc_dbus::{register_uccd_service, AllowAllAuthorizer, UccdBackend, DBUS_PATH};
use zbus::connection::Builder;
use zbus::zvariant::OwnedValue;

fn unique_service_name(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("org.test.uccd.cli_{tag}_{}_{}", std::process::id(), nanos)
}

#[derive(Default)]
struct AuditCliMockBackend {
    kb_written: Mutex<Vec<String>>,
    pump_written: Mutex<Vec<i32>>,
    fail_dbus_getters: bool,
    brightness_val: i32,
}

impl UccdBackend for AuditCliMockBackend {
    fn get_keyboard_backlight_capabilities_json(&self) -> String {
        json!({
            "zones": 1,
            "maxBrightness": 255,
            "maxRed": 255,
            "maxGreen": 255,
            "maxBlue": 255,
            "modes": [0, 1]
        })
        .to_string()
    }

    fn get_keyboard_backlight_states_json(&self) -> String {
        // Real uccd schema: JSON array of zone state objects
        json!([{
            "mode": 0,
            "brightness": 200,
            "red": 10,
            "green": 20,
            "blue": 30
        }])
        .to_string()
    }

    fn set_keyboard_backlight_states_json(&self, keyboard_backlight_states_json: &str) -> bool {
        self.kb_written
            .lock()
            .unwrap()
            .push(keyboard_backlight_states_json.to_string());
        true
    }

    fn get_custom_keyboard_profiles_json(&self) -> String {
        // Real uccd schema: embedded `json` Value, not double-encoded JSON string
        json!([{
            "id": "kb_prof1",
            "name": "Cyber",
            "json": {
                "keyboardProfileId": "kb_prof1",
                "states": [{
                    "mode": 0,
                    "brightness": 200,
                    "red": 10,
                    "green": 20,
                    "blue": 30
                }]
            }
        }])
        .to_string()
    }

    fn get_lightbar_settings_json(&self) -> String {
        json!({ "kind": "settings", "managed": false }).to_string()
    }

    fn get_lightbar_status_json(&self) -> String {
        json!({ "kind": "status", "available": true, "animating": false }).to_string()
    }

    fn get_power_device_settings_json(&self) -> String {
        json!({ "kind": "settings", "enabled": false }).to_string()
    }

    fn get_power_device_status_json(&self) -> String {
        json!({ "kind": "status", "mode": "ac", "devices": [] }).to_string()
    }

    fn odm_power_limits_json(&self) -> String {
        // Real uccd schema: array of channel descriptors
        json!([
            { "index": 0, "min": 15, "max": 90, "current": 55 },
            { "index": 1, "min": 20, "max": 120, "current": 80 }
        ])
        .to_string()
    }

    fn get_active_profile_json(&self) -> String {
        json!({
            "id": "custom_ac",
            "name": "AC Gaming",
            "display": { "useBrightness": false, "brightness": 50, "useRefRate": false, "refreshRate": 60 },
            "cpu": { "energyPerformancePreference": "balance_performance", "governor": "powersave" },
            "odmPowerLimits": { "tdpValues": [55, 80, 105] },
            "fan": { "fanProfile": "Balanced", "useControl": true }
        })
        .to_string()
    }

    fn get_profiles_json(&self) -> String {
        json!([
            {
                "id": "custom_ac",
                "name": "AC Gaming",
                "display": { "useBrightness": false, "brightness": 50 },
                "cpu": { "energyPerformancePreference": "balance_performance" },
                "odmPowerLimits": { "tdpValues": [55, 80, 105] }
            }
        ])
        .to_string()
    }

    fn get_settings_json(&self) -> String {
        json!({
            "profiles": {
                "power_ac": "custom_ac",
                "power_bat": "Quiet"
            },
            "stateMap": {
                "power_ac": "custom_ac",
                "power_bat": "Quiet"
            }
        })
        .to_string()
    }

    fn get_fan_profile_names(&self) -> String {
        json!(["Balanced", "Quiet"]).to_string()
    }

    fn set_water_cooler_pump_voltage(&self, voltage: i32) -> bool {
        // Real protocol: 0(11V), 2(7V), 3(8V), 4(Off); 1(12V) rejected
        if matches!(voltage, 0 | 2 | 3 | 4) {
            self.pump_written.lock().unwrap().push(voltage);
            true
        } else {
            false
        }
    }

    fn get_display_brightness(&self) -> i32 {
        self.brightness_val
    }

    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("speed".to_string(), OwnedValue::from(55_i32)),
            ("temp".to_string(), OwnedValue::from(60_i32)),
            ("timestamp".to_string(), OwnedValue::from(0_i64)),
        ])
    }

    fn get_system_info_json(&self) -> String {
        if self.fail_dbus_getters {
            String::new()
        } else {
            "{}".to_string()
        }
    }
}

async fn start_mock_service(
    tag: &str,
    backend: Arc<AuditCliMockBackend>,
) -> (zbus::Connection, String) {
    let bus_name = unique_service_name(tag);
    let conn = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&conn, backend, Arc::new(AllowAllAuthorizer))
        .await
        .unwrap();
    (conn, bus_name)
}

fn run_cli(bus_name: &str, args: &[&str]) -> std::process::Output {
    Command::new("timeout")
        .arg("5s")
        .arg(env!("CARGO_BIN_EXE_ucc-cli"))
        .args(args)
        .env("UCCD_DBUS_SERVICE", bus_name)
        .output()
        .expect("spawn ucc-cli")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_01_keyboard_color_get_activate_and_info_wire_format() {
    let backend = Arc::new(AuditCliMockBackend::default());
    let (_conn, bus_name) = start_mock_service("kb", backend.clone()).await;

    let out_get = run_cli(&bus_name, &["keyboard", "get"]);
    assert!(out_get.status.success(), "{out_get:?}");
    let stdout_get = String::from_utf8_lossy(&out_get.stdout);
    assert!(
        !stdout_get.contains("Zones:                   0"),
        "keyboard get failed to parse real array payload: {stdout_get}"
    );

    let out_act = run_cli(&bus_name, &["keyboard", "activate", "kb_prof1"]);
    assert!(
        out_act.status.success(),
        "keyboard activate must accept structured `json` field in custom keyboard profile: {}",
        String::from_utf8_lossy(&out_act.stderr)
    );

    let out_color = run_cli(&bus_name, &["keyboard", "color", "10", "20", "30", "200"]);
    assert!(out_color.status.success(), "{out_color:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_02_lightbar_and_devices_status_vs_get() {
    let backend = Arc::new(AuditCliMockBackend::default());
    let (_conn, bus_name) = start_mock_service("status_get", backend).await;

    let lb_status: Value =
        serde_json::from_slice(&run_cli(&bus_name, &["lightbar", "status", "--json"]).stdout)
            .unwrap();
    let lb_get: Value =
        serde_json::from_slice(&run_cli(&bus_name, &["lightbar", "get", "--json"]).stdout).unwrap();
    assert_eq!(
        lb_status["kind"], "status",
        "lightbar status must call GetLightbarStatusJSON"
    );
    assert_eq!(
        lb_get["kind"], "settings",
        "lightbar get must call GetLightbarSettingsJSON"
    );

    let dev_status: Value =
        serde_json::from_slice(&run_cli(&bus_name, &["devices", "status", "--json"]).stdout)
            .unwrap();
    let dev_get: Value =
        serde_json::from_slice(&run_cli(&bus_name, &["devices", "get", "--json"]).stdout).unwrap();
    assert_eq!(
        dev_status["kind"], "status",
        "devices status must call GetPowerDeviceStatusJSON"
    );
    assert_eq!(
        dev_get["kind"], "settings",
        "devices get must call GetPowerDeviceSettingsJSON"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_03_power_limits_parses_channel_array_json() {
    let backend = Arc::new(AuditCliMockBackend::default());
    let (_conn, bus_name) = start_mock_service("tdp", backend).await;

    let out = run_cli(&bus_name, &["power-limits"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("55") && stdout.contains("80"),
        "power-limits must parse [{{index,min,max,current}}] JSON and display 55/80 W:\n{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_04_profile_get_fields_and_ac_tags() {
    let backend = Arc::new(AuditCliMockBackend::default());
    let (_conn, bus_name) = start_mock_service("prof", backend).await;

    let out_prof = run_cli(&bus_name, &["profile", "get"]);
    assert!(out_prof.status.success(), "{out_prof:?}");
    let stdout = String::from_utf8_lossy(&out_prof.stdout);
    assert!(
        stdout.contains("balance_performance") && stdout.contains("55"),
        "profile get must extract energyPerformancePreference and odmPowerLimits.tdpValues:\n{stdout}"
    );
    assert!(
        !stdout.contains("50%"),
        "profile get must not report 50% active brightness when useBrightness is false:\n{stdout}"
    );

    let out_list = run_cli(&bus_name, &["profile", "list"]);
    assert!(out_list.status.success(), "{out_list:?}");
    let stdout_list = String::from_utf8_lossy(&out_list.stdout);
    assert!(
        stdout_list.contains("AC"),
        "profile list must show [AC] assignment tag from settings['profiles']:\n{stdout_list}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_05_cooler_pump_protocol_codes() {
    let backend = Arc::new(AuditCliMockBackend::default());
    let (_conn, bus_name) = start_mock_service("pump", backend.clone()).await;

    for code in ["0", "2", "3", "4"] {
        let out = run_cli(&bus_name, &["cooler", "pump", code]);
        assert!(
            out.status.success(),
            "cooler pump {code} must succeed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let bad = run_cli(&bus_name, &["cooler", "pump", "1"]);
    assert!(
        !bad.status.success(),
        "cooler pump 1 (prohibited 12V) must be rejected with non-zero exit"
    );
    assert_eq!(*backend.pump_written.lock().unwrap(), vec![0, 2, 3, 4]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_06_nonzero_exit_on_brightness_minus_one_and_dbus_errors() {
    let backend = Arc::new(AuditCliMockBackend {
        brightness_val: -1,
        ..Default::default()
    });
    let (_conn, bus_name) = start_mock_service("err_bright", backend).await;

    let out = run_cli(&bus_name, &["brightness", "get"]);
    assert!(
        !out.status.success(),
        "brightness get returning -1 sentinel must exit non-zero"
    );

    // Nonexistent D-Bus service / unreachable peer must make statemap/fan/profile exit non-zero
    let dead_bus = unique_service_name("dead");
    for args in [
        &["statemap", "get"][..],
        &["fan", "list"][..],
        &["profile", "list"][..],
    ] {
        let res = run_cli(&dead_bus, args);
        assert!(
            !res.status.success(),
            "subcommand {args:?} must exit non-zero on D-Bus failure"
        );
    }
}

struct HangingProfilesService;

#[zbus::interface(name = "com.uniwill.uccd")]
impl HangingProfilesService {
    #[zbus(name = "GetDeviceName")]
    fn device_name(&self) -> &str {
        "audit"
    }

    #[zbus(name = "GetProfilesJSON")]
    async fn profiles(&self) -> String {
        std::future::pending().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_07_rpc_deadline_applies_to_non_status_subcommands() {
    let bus_name = unique_service_name("hang_prof");
    let conn = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    conn.object_server()
        .at(DBUS_PATH, HangingProfilesService)
        .await
        .unwrap();

    let out = run_cli(&bus_name, &["profile", "list"]);
    assert_ne!(
        out.status.code(),
        Some(124),
        "non-status RPC hung past 5s without CLI timeout deadline"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_cli_08_version_subcommand_bools_negative_count_and_zero_timestamp_fans() {
    let out_ver = Command::new(env!("CARGO_BIN_EXE_ucc-cli"))
        .arg("version")
        .output()
        .unwrap();
    assert!(
        out_ver.status.success(),
        "`ucc-cli version` subcommand must succeed: {}",
        String::from_utf8_lossy(&out_ver.stderr)
    );

    let backend = Arc::new(AuditCliMockBackend::default());
    let (_conn, bus_name) = start_mock_service("misc", backend).await;

    let neg = run_cli(&bus_name, &["monitor", "--count", "-1"]);
    assert!(
        !neg.status.success(),
        "monitor --count -1 must be rejected rather than running infinite loop"
    );

    let out_status = run_cli(&bus_name, &["status", "--json"]);
    assert!(out_status.status.success(), "{out_status:?}");
    let data: Value = serde_json::from_slice(&out_status.stdout).unwrap();
    assert!(
        data["cpu"]["fanPercent"].is_null(),
        "fan sample with timestamp==0 must be filtered as stale/inactive: {}",
        data["cpu"]
    );
}
