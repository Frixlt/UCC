// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use ucc_dbus::{
    decode_frame, dispatch_json_rpc, encode_frame, DefaultUccdBackend, IpcRequest, IpcResponse,
    IpcTransportKind, WINDOWS_NAMED_PIPE_PATH,
};

#[test]
fn transport_agnostic_json_rpc_dispatches_backend_methods() {
    assert!(WINDOWS_NAMED_PIPE_PATH.contains("ucc-rusted"));
    let _host_transport = IpcTransportKind::host_default();

    let backend = DefaultUccdBackend;
    let req = IpcRequest {
        id: 1,
        method: "GetDeviceName".to_string(),
        params: json!({}),
    };
    let wire = encode_frame(&req).unwrap();
    let decoded: IpcRequest = decode_frame(&wire).unwrap();
    let resp = dispatch_json_rpc(&backend, &decoded);
    assert!(resp.ok);
    assert_eq!(resp.result, Some(json!("ucc-rusted Laptop")));

    let mode_req = IpcRequest {
        id: 2,
        method: "SetManualPowerMode".to_string(),
        params: json!({ "mode": "bat" }),
    };
    let mode_resp: IpcResponse =
        decode_frame(&encode_frame(&dispatch_json_rpc(&backend, &mode_req)).unwrap()).unwrap();
    assert!(mode_resp.ok);
    assert_eq!(mode_resp.result, Some(json!(true)));

    let missing_param = IpcRequest {
        id: 3,
        method: "SetManualPowerMode".to_string(),
        params: json!({}),
    };
    let err_resp = dispatch_json_rpc(&backend, &missing_param);
    assert!(!err_resp.ok);
    assert!(err_resp.error.unwrap().contains("missing string param"));
}

#[test]
fn ipc_dispatch_covers_core_methods_and_param_aliases() {
    use std::sync::Mutex;
    use ucc_dbus::UccdBackend;

    #[derive(Default)]
    struct AliasSpyBackend {
        calls: Mutex<Vec<String>>,
    }

    impl UccdBackend for AliasSpyBackend {
        fn set_manual_power_mode(&self, mode_val: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("manual_mode:{mode_val}"));
            true
        }

        fn set_active_profile(&self, id: &str) -> bool {
            self.calls.lock().unwrap().push(format!("active:{id}"));
            true
        }

        fn set_temp_profile(&self, profile_name: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("temp:{profile_name}"));
            true
        }

        fn set_display_refresh_rate(&self, display: &str, refresh_rate: i32) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("refresh:{display}:{refresh_rate}"));
            true
        }

        fn set_state_map(&self, state: &str, profile_id: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("state_map:{state}:{profile_id}"));
            true
        }

        fn set_charge_type(&self, type_val: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("charge_type:{type_val}"));
            true
        }

        fn set_water_cooler_pump_voltage(&self, voltage: i32) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("pump_voltage:{voltage}"));
            true
        }

        fn set_water_cooler_fan_speed(&self, duty_cycle_percent: i32) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("wc_fan:{duty_cycle_percent}"));
            true
        }

        fn set_water_cooler_led_color(&self, red: i32, green: i32, blue: i32, mode_val: i32) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("wc_led:{red},{green},{blue}:{mode_val}"));
            true
        }

        fn get_monitor_data_since(&self, since_timestamp_ms: i64) -> Vec<u8> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("monitor:{since_timestamp_ms}"));
            vec![7, 8]
        }
    }

    let backend = AliasSpyBackend::default();

    // camelCase and snake_case parameter aliases across core methods
    let valid_cases = [
        ("SetManualPowerMode", json!({ "mode": "ac" })),
        ("SetManualPowerMode", json!({ "mode_val": "bat" })),
        ("SetActiveProfile", json!({ "id": "prof-a" })),
        ("SetActiveProfile", json!({ "profile_id": "prof-b" })),
        ("SetTempProfile", json!({ "profileName": "tmp-a" })),
        ("SetTempProfile", json!({ "profile_name": "tmp-b" })),
        (
            "SetDisplayRefreshRate",
            json!({ "display": "eDP-1", "refreshRate": 144 }),
        ),
        (
            "SetDisplayRefreshRate",
            json!({ "display": "eDP-1", "refresh_rate": 60 }),
        ),
        (
            "SetStateMap",
            json!({ "state": "power_ac", "profileId": "p1" }),
        ),
        (
            "SetStateMap",
            json!({ "state": "power_bat", "profile_id": "p2" }),
        ),
        ("SetChargeType", json!({ "chargeType": "Custom" })),
        ("SetChargeType", json!({ "type_val": "Standard" })),
        ("SetWaterCoolerPumpVoltage", json!({ "voltage": 2 })),
        ("SetWaterCoolerPumpVoltage", json!({ "voltage_code": 3 })),
        ("SetWaterCoolerFanSpeed", json!({ "speed": 50 })),
        (
            "SetWaterCoolerFanSpeed",
            json!({ "duty_cycle_percent": 75 }),
        ),
        (
            "SetWaterCoolerLEDColor",
            json!({ "red": 10, "green": 20, "blue": 30, "mode": 1 }),
        ),
        (
            "SetWaterCoolerLEDColor",
            json!({ "red": 40, "green": 50, "blue": 60, "mode_val": 2 }),
        ),
        ("GetMonitorDataSince", json!({ "sinceTimestampMs": 100 })),
        ("GetMonitorDataSince", json!({ "since_timestamp_ms": 200 })),
    ];

    for (idx, (method, params)) in valid_cases.into_iter().enumerate() {
        let resp = dispatch_json_rpc(
            &backend,
            &IpcRequest {
                id: (idx + 1) as u64,
                method: method.to_string(),
                params,
            },
        );
        assert!(
            resp.ok,
            "method '{method}' should succeed, got error: {:?}",
            resp.error
        );
    }

    assert_eq!(
        *backend.calls.lock().unwrap(),
        vec![
            "manual_mode:ac",
            "manual_mode:bat",
            "active:prof-a",
            "active:prof-b",
            "temp:tmp-a",
            "temp:tmp-b",
            "refresh:eDP-1:144",
            "refresh:eDP-1:60",
            "state_map:power_ac:p1",
            "state_map:power_bat:p2",
            "charge_type:Custom",
            "charge_type:Standard",
            "pump_voltage:2",
            "pump_voltage:3",
            "wc_fan:50",
            "wc_fan:75",
            "wc_led:10,20,30:1",
            "wc_led:40,50,60:2",
            "monitor:100",
            "monitor:200",
        ]
    );

    // Wrong parameter types
    let wrong_type_cases = [
        ("SetManualPowerMode", json!({ "mode": 42 }), "missing string param 'mode'"),
        (
            "SetDisplayBrightness",
            json!({ "brightness": "bright" }),
            "missing int param 'brightness'",
        ),
        (
            "SetDisplayBrightness",
            json!({ "brightness": 9_999_999_999_i64 }),
            "missing int param 'brightness'",
        ),
        ("SetWebcam", json!({ "enabled": "yes" }), "missing bool param 'enabled'"),
        (
            "GetMonitorDataSince",
            json!({ "sinceTimestampMs": true }),
            "missing int param 'sinceTimestampMs'",
        ),
    ];

    for (idx, (method, params, expected_err)) in wrong_type_cases.into_iter().enumerate() {
        let resp = dispatch_json_rpc(
            &backend,
            &IpcRequest {
                id: 100 + idx as u64,
                method: method.to_string(),
                params,
            },
        );
        assert!(!resp.ok, "wrong param type for '{method}' must fail");
        assert!(
            resp.error.as_deref().unwrap_or("").contains(expected_err),
            "unexpected error for '{method}': {:?}",
            resp.error
        );
    }

    // Missing required params
    let missing_param_cases = [
        ("SetDisplayBrightness", json!({}), "missing int param 'brightness'"),
        (
            "SetDisplayRefreshRate",
            json!({ "display": "eDP-1" }),
            "missing int param 'refreshRate'",
        ),
        (
            "SetWaterCoolerLEDColor",
            json!({ "red": 1, "green": 2, "blue": 3 }),
            "missing int param 'mode'",
        ),
        ("SetWebcam", json!({}), "missing bool param 'enabled'"),
    ];

    for (idx, (method, params, expected_err)) in missing_param_cases.into_iter().enumerate() {
        let resp = dispatch_json_rpc(
            &backend,
            &IpcRequest {
                id: 200 + idx as u64,
                method: method.to_string(),
                params,
            },
        );
        assert!(!resp.ok, "missing param for '{method}' must fail");
        assert!(
            resp.error.as_deref().unwrap_or("").contains(expected_err),
            "unexpected error for '{method}': {:?}",
            resp.error
        );
    }

    // Unknown method rejection
    let unknown_resp = dispatch_json_rpc(
        &backend,
        &IpcRequest {
            id: 300,
            method: "NonExistentMethod".to_string(),
            params: json!({}),
        },
    );
    assert!(!unknown_resp.ok);
    assert!(unknown_resp
        .error
        .as_deref()
        .unwrap_or("")
        .contains("unknown IPC method 'NonExistentMethod'"));
}
