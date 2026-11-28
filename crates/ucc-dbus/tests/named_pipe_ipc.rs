// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::sync::Arc;
use ucc_dbus::{
    decode_frame, decode_frame_bounded, dispatch_json_rpc_authorized,
    dispatch_json_rpc_with_identity, encode_frame, is_valid_pump_voltage_code,
    required_access_level, CallerIdentity, DefaultUccdBackend, IpcAccessLevel, IpcFrameError,
    IpcRequest, IpcResponse, MockNamedPipeTransport, NamedPipeServer, NamedPipeTransport,
    PipeAclError, PipeAclPolicy, FORBIDDEN_PUMP_VOLTAGE_CODE, MAX_IPC_FRAME_BYTES,
    WINDOWS_NAMED_PIPE_PATH, WINDOWS_PIPE_SDDL,
};

#[test]
fn pipe_sddl_default_policy_allows_local_principals_and_denies_remote_or_anonymous() {
    let policy = PipeAclPolicy::parse_and_validate(WINDOWS_PIPE_SDDL).unwrap();
    assert_eq!(policy.sddl(), WINDOWS_PIPE_SDDL);
    assert_eq!(policy.aces().len(), 3);

    assert!(policy.allows_sid("SY"));
    assert!(policy.allows_sid("BA"));
    assert!(policy.allows_sid("IU"));
    assert!(PipeAclPolicy::is_privileged_sid("SY"));
    assert!(PipeAclPolicy::is_privileged_sid("BA"));
    assert!(!PipeAclPolicy::is_privileged_sid("IU"));

    assert!(policy.denies_or_omits_sid("WD"));
    assert!(policy.denies_or_omits_sid("AN"));
    assert!(policy.denies_or_omits_sid("NU"));
    assert!(policy.denies_or_omits_sid("AU"));
    assert!(policy.denies_or_omits_sid("BG"));
}

#[test]
fn pipe_sddl_validator_rejects_unprotected_or_overly_permissive_descriptors() {
    // Missing protected DACL flag 'P'
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)"),
        Err(PipeAclError::MissingProtectedDacl)
    );

    // Forbidden Everyone (WD)
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GR;;;WD)"),
        Err(PipeAclError::ForbiddenAllowSid("WD".to_string()))
    );

    // Forbidden Anonymous (AN)
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GR;;;AN)"),
        Err(PipeAclError::ForbiddenAllowSid("AN".to_string()))
    );

    // Forbidden Network (NU)
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GR;;;NU)"),
        Err(PipeAclError::ForbiddenAllowSid("NU".to_string()))
    );

    // Excessive rights for Interactive Users (GA instead of GRGW)
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)"),
        Err(PipeAclError::ExcessiveInteractiveRights("GA".to_string()))
    );

    // Missing required Built-in Administrators (BA)
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GRGW;;;IU)"),
        Err(PipeAclError::MissingRequiredSid("BA".to_string()))
    );
}

#[test]
fn ipc_frame_decoder_enforces_256_kib_maximum_frame_bound() {
    assert_eq!(MAX_IPC_FRAME_BYTES, 256 * 1024);

    let oversized_payload = "x".repeat(MAX_IPC_FRAME_BYTES + 128);
    let oversized_frame = format!(
        r#"{{"id":1,"method":"ApplyProfile","params":{{"profileJson":"{oversized_payload}"}}}}"#
    );

    let err_default: Result<IpcRequest, _> = decode_frame(&oversized_frame);
    assert!(err_default.is_err());
    assert!(err_default
        .unwrap_err()
        .to_string()
        .contains("exceeds maximum size"));

    let err_bounded: Result<IpcRequest, _> =
        decode_frame_bounded(&oversized_frame, MAX_IPC_FRAME_BYTES);
    assert!(matches!(
        err_bounded,
        Err(IpcFrameError::Oversized { max, .. }) if max == MAX_IPC_FRAME_BYTES
    ));

    let empty_err: Result<IpcRequest, _> = decode_frame_bounded("   \n", MAX_IPC_FRAME_BYTES);
    assert!(matches!(empty_err, Err(IpcFrameError::Empty)));
}

#[test]
fn authorization_blocks_unprivileged_manage_hardware_and_allows_read_control() {
    let backend = DefaultUccdBackend;

    // Interactive (unprivileged) caller can read and perform standard Control calls.
    let read_req = IpcRequest {
        id: 10,
        method: "GetChargeStartThreshold".to_string(),
        params: json!({}),
    };
    assert_eq!(required_access_level(&read_req), IpcAccessLevel::Read);
    let read_resp = dispatch_json_rpc_authorized(&backend, &read_req, false);
    assert!(read_resp.ok);

    let control_req = IpcRequest {
        id: 11,
        method: "SetDisplayBrightness".to_string(),
        params: json!({ "brightness": 75 }),
    };
    assert_eq!(required_access_level(&control_req), IpcAccessLevel::Control);
    let control_resp = dispatch_json_rpc_authorized(&backend, &control_req, false);
    assert!(control_resp.ok);
    assert_eq!(control_resp.result, Some(json!(true)));

    // Privileged methods must be rejected when caller_is_privileged == false.
    let privileged_requests = vec![
        IpcRequest {
            id: 20,
            method: "SetChargeStartThreshold".to_string(),
            params: json!({ "value": 40 }),
        },
        IpcRequest {
            id: 21,
            method: "SetChargeEndThreshold".to_string(),
            params: json!({ "value": 80 }),
        },
        IpcRequest {
            id: 22,
            method: "SetChargeType".to_string(),
            params: json!({ "chargeType": "Custom" }),
        },
        IpcRequest {
            id: 23,
            method: "SetNVIDIAPowerOffset".to_string(),
            params: json!({ "offset": 15 }),
        },
        IpcRequest {
            id: 24,
            method: "SetWaterCoolerPumpVoltage".to_string(),
            params: json!({ "voltage": 2 }),
        },
        IpcRequest {
            id: 25,
            method: "ApplyProfile".to_string(),
            params: json!({
                "profileJson": r#"{"odmPowerLimits":{"tdpValues":[45,55,65]}}"#
            }),
        },
        IpcRequest {
            id: 26,
            method: "ApplyProfile".to_string(),
            params: json!({
                "profileJson": r#"{"fan":{"useControl":false}}"#
            }),
        },
        IpcRequest {
            id: 27,
            method: "ApplyProfile".to_string(),
            params: json!({
                "profileJson": r#"{"charging":{"endThreshold":80}}"#
            }),
        },
        IpcRequest {
            id: 28,
            method: "ApplyProfile".to_string(),
            params: json!({
                "profileJson": r#"{"nvidiaPowerCTRLProfile":{"cTGPOffset":10}}"#
            }),
        },
    ];

    for req in &privileged_requests {
        assert_eq!(
            required_access_level(req),
            IpcAccessLevel::ManageHardware,
            "method '{}' must require ManageHardware",
            req.method
        );
        let denied = dispatch_json_rpc_authorized(&backend, req, false);
        assert!(
            !denied.ok,
            "unprivileged caller must be denied for '{}'",
            req.method
        );
        assert!(denied
            .error
            .as_deref()
            .unwrap_or("")
            .contains("authorization denied"));
    }

    // Privileged caller succeeds on direct hardware mutations.
    for req in &privileged_requests[..5] {
        let allowed = dispatch_json_rpc_authorized(&backend, req, true);
        assert!(
            allowed.ok,
            "privileged caller must succeed for '{}': {:?}",
            req.method, allowed.error
        );
    }

    // Remote callers are always rejected even for read-only calls.
    let remote = CallerIdentity::remote("NU");
    let remote_resp = dispatch_json_rpc_with_identity(&backend, &read_req, &remote);
    assert!(!remote_resp.ok);
    assert!(remote_resp
        .error
        .unwrap()
        .contains("remote callers are not permitted"));
}

#[test]
fn pump_voltage_code_1_v12_is_strictly_forbidden_even_for_privileged_callers() {
    assert_eq!(FORBIDDEN_PUMP_VOLTAGE_CODE, 1);
    assert!(!is_valid_pump_voltage_code(1));
    assert!(!is_valid_pump_voltage_code(-1));
    assert!(!is_valid_pump_voltage_code(5));
    for valid in [0, 2, 3, 4] {
        assert!(is_valid_pump_voltage_code(valid));
    }

    let backend = DefaultUccdBackend;
    let forbidden_req = IpcRequest {
        id: 100,
        method: "SetWaterCoolerPumpVoltage".to_string(),
        params: json!({ "voltage": 1 }),
    };
    let forbidden_resp = dispatch_json_rpc_authorized(&backend, &forbidden_req, true);
    assert!(!forbidden_resp.ok);
    assert!(forbidden_resp.error.unwrap().contains("V12"));

    for code in [0, 2, 3, 4] {
        let req = IpcRequest {
            id: 101 + code as u64,
            method: "SetWaterCoolerPumpVoltage".to_string(),
            params: json!({ "voltage": code }),
        };
        let resp = dispatch_json_rpc_authorized(&backend, &req, true);
        assert!(resp.ok, "valid pump voltage code {code} must succeed");
        assert_eq!(resp.result, Some(json!(true)));
    }
}

#[test]
fn named_pipe_server_serves_sessions_enforces_acl_bounds_and_shuts_down_cleanly() {
    let transport = MockNamedPipeTransport::default_secure();
    assert_eq!(transport.pipe_path(), WINDOWS_NAMED_PIPE_PATH);
    assert_eq!(transport.acl_policy().sddl(), WINDOWS_PIPE_SDDL);

    let read_wire = encode_frame(&IpcRequest {
        id: 1,
        method: "GetDeviceName".to_string(),
        params: json!({}),
    })
    .unwrap();
    let denied_wire = encode_frame(&IpcRequest {
        id: 2,
        method: "SetChargeEndThreshold".to_string(),
        params: json!({ "value": 80 }),
    })
    .unwrap();
    let oversized_wire = format!(
        r#"{{"id":3,"method":"GetDeviceName","params":{{"pad":"{}"}}}}"#,
        "A".repeat(MAX_IPC_FRAME_BYTES + 64)
    );
    let priv_wire = encode_frame(&IpcRequest {
        id: 4,
        method: "SetChargeEndThreshold".to_string(),
        params: json!({ "value": 80 }),
    })
    .unwrap();

    let interactive_out = transport.enqueue_session(
        CallerIdentity::interactive("IU"),
        vec![read_wire, denied_wire, oversized_wire],
    );
    let admin_out = transport.enqueue_session(CallerIdentity::privileged("BA"), vec![priv_wire]);

    let server = NamedPipeServer::new(transport, Arc::new(DefaultUccdBackend));
    let stats = server.run_until_shutdown().unwrap();

    assert_eq!(stats.connections_served, 2);
    assert_eq!(stats.requests_served, 3);
    assert_eq!(stats.rejected_frames, 1);

    let interactive_resps: Vec<IpcResponse> = interactive_out
        .lock()
        .unwrap()
        .iter()
        .map(|w| decode_frame(w).unwrap())
        .collect();
    assert_eq!(interactive_resps.len(), 3);
    assert!(interactive_resps[0].ok);
    assert_eq!(
        interactive_resps[0].result,
        Some(json!("ucc-rusted Laptop"))
    );
    assert!(!interactive_resps[1].ok);
    assert!(interactive_resps[1]
        .error
        .as_deref()
        .unwrap()
        .contains("authorization denied"));
    assert!(!interactive_resps[2].ok);
    assert!(interactive_resps[2]
        .error
        .as_deref()
        .unwrap()
        .contains("exceeds maximum size"));

    let admin_resps: Vec<IpcResponse> = admin_out
        .lock()
        .unwrap()
        .iter()
        .map(|w| decode_frame(w).unwrap())
        .collect();
    assert_eq!(admin_resps.len(), 1);
    assert!(admin_resps[0].ok);
    assert_eq!(admin_resps[0].result, Some(json!(true)));

    // Cooperative shutdown prevents processing any newly queued sessions.
    server.request_shutdown();
    assert!(server.is_shutdown());
    let skipped_out = server.transport().enqueue_session(
        CallerIdentity::privileged("SY"),
        vec![encode_frame(&IpcRequest {
            id: 99,
            method: "UccdVersion".to_string(),
            params: json!({}),
        })
        .unwrap()],
    );
    let after_shutdown = server.run_until_shutdown().unwrap();
    assert_eq!(after_shutdown.connections_served, 0);
    assert!(skipped_out.lock().unwrap().is_empty());
}

#[test]
fn pipe_sddl_validator_enforces_case_insensitivity_strict_sids_and_iu_rights_allowlist() {
    // Lowercase ACE types, rights, and SIDs normalize cleanly to uppercase.
    let lower = PipeAclPolicy::parse_and_validate("D:P(a;;ga;;;sy)(a;;ga;;;ba)(a;;grgw;;;iu)")
        .expect("lowercase valid SDDL should normalize and pass");
    assert!(lower.allows_sid("SY"));
    assert!(lower.allows_sid("BA"));
    assert!(lower.allows_sid("IU"));

    // Lowercase forbidden SID (wd) is rejected after normalization.
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(a;;gr;;;wd)"),
        Err(PipeAclError::ForbiddenAllowSid("WD".to_string()))
    );

    // Raw numeric Everyone SID (S-1-1-0) is rejected.
    assert_eq!(
        PipeAclPolicy::parse_and_validate(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GR;;;S-1-1-0)"
        ),
        Err(PipeAclError::ForbiddenAllowSid("S-1-1-0".to_string()))
    );

    // Object Allow ACE (OA) for a non-allowlisted SID is rejected.
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(OA;;GR;;;WD)"),
        Err(PipeAclError::ForbiddenAllowSid("WD".to_string()))
    );

    // Hex access mask (0x10000000 = GENERIC_ALL) for IU is rejected by strict allowlist.
    assert_eq!(
        PipeAclPolicy::parse_and_validate("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x10000000;;;IU)"),
        Err(PipeAclError::ExcessiveInteractiveRights(
            "0X10000000".to_string()
        ))
    );
}

#[test]
fn profile_nvidia_ctgp_offset_and_manual_pump_voltage_require_manage_hardware() {
    let backend = DefaultUccdBackend;

    let ctgp_req = IpcRequest {
        id: 200,
        method: "ApplyProfile".to_string(),
        params: json!({
            "profileJson": r#"{"nvidiaCTGPOffset":15}"#
        }),
    };
    assert_eq!(
        required_access_level(&ctgp_req),
        IpcAccessLevel::ManageHardware
    );
    let ctgp_denied = dispatch_json_rpc_authorized(&backend, &ctgp_req, false);
    assert!(!ctgp_denied.ok);
    assert!(ctgp_denied
        .error
        .as_deref()
        .unwrap_or("")
        .contains("authorization denied"));

    let pump_req = IpcRequest {
        id: 201,
        method: "ApplyProfile".to_string(),
        params: json!({
            "profileJson": r#"{"fan":{"manualPumpVoltage":2}}"#
        }),
    };
    assert_eq!(
        required_access_level(&pump_req),
        IpcAccessLevel::ManageHardware
    );
    let pump_denied = dispatch_json_rpc_authorized(&backend, &pump_req, false);
    assert!(!pump_denied.ok);
    assert!(pump_denied
        .error
        .as_deref()
        .unwrap_or("")
        .contains("authorization denied"));

    // Zero cTGP offset and null manualPumpVoltage remain Control-tier.
    let benign_req = IpcRequest {
        id: 202,
        method: "ApplyProfile".to_string(),
        params: json!({
            "profileJson": r#"{"nvidiaCTGPOffset":0,"fan":{"manualPumpVoltage":null}}"#
        }),
    };
    assert_eq!(required_access_level(&benign_req), IpcAccessLevel::Control);
}

#[test]
fn canonical_dbus_method_casing_and_display_refresh_rate_dispatch() {
    let backend = DefaultUccdBackend;

    let refresh_req = IpcRequest {
        id: 300,
        method: "SetDisplayRefreshRate".to_string(),
        params: json!({ "display": "eDP-1", "refreshRate": 144 }),
    };
    assert_eq!(required_access_level(&refresh_req), IpcAccessLevel::Control);
    let refresh_resp = dispatch_json_rpc_authorized(&backend, &refresh_req, false);
    assert!(refresh_resp.ok);
    assert_eq!(refresh_resp.result, Some(json!(false)));

    let dgpu_req = IpcRequest {
        id: 301,
        method: "GetDGpuInfoValuesJSON".to_string(),
        params: json!({}),
    };
    assert_eq!(required_access_level(&dgpu_req), IpcAccessLevel::Read);
    let dgpu_resp = dispatch_json_rpc_authorized(&backend, &dgpu_req, false);
    assert!(dgpu_resp.ok);
    assert_eq!(dgpu_resp.result, Some(json!("{}")));

    let epps_req = IpcRequest {
        id: 302,
        method: "GetAvailableEPPs".to_string(),
        params: json!({}),
    };
    assert_eq!(required_access_level(&epps_req), IpcAccessLevel::Read);
    let epps_resp = dispatch_json_rpc_authorized(&backend, &epps_req, false);
    assert!(epps_resp.ok);
    assert_eq!(epps_resp.result, Some(json!("[]")));

    let led_req = IpcRequest {
        id: 303,
        method: "SetWaterCoolerLEDColor".to_string(),
        params: json!({ "red": 255, "green": 64, "blue": 0, "mode": 1 }),
    };
    assert_eq!(required_access_level(&led_req), IpcAccessLevel::Control);
    let led_resp = dispatch_json_rpc_authorized(&backend, &led_req, false);
    assert!(led_resp.ok);
    assert_eq!(led_resp.result, Some(json!(true)));
}

#[test]
fn profile_negative_manual_pump_voltage_sentinel_is_not_escalated_and_v12_is_rejected() {
    let backend = DefaultUccdBackend;
    let interactive = CallerIdentity::interactive("IU:pid:4242");
    let privileged = CallerIdentity::privileged("BA:pid:100");

    // -1 sentinel means unset manual pump voltage (not ManageHardware)
    let unset_profile = json!({
        "name": "Balanced",
        "fan": { "useControl": true, "manualPumpVoltage": -1 }
    })
    .to_string();
    let req_unset = IpcRequest {
        id: 201,
        method: "ApplyProfile".into(),
        params: json!({ "profile_json": unset_profile }),
    };
    assert_eq!(required_access_level(&req_unset), IpcAccessLevel::Control);
    let resp_unset = dispatch_json_rpc_with_identity(&backend, &req_unset, &interactive);
    assert!(
        resp_unset.ok,
        "expected unset -1 sentinel to succeed for IU"
    );

    // Code 1 (V12) must be rejected in ApplyProfile and SaveCustomProfile even for privileged callers
    let v12_profile = json!({
        "name": "UnsafeV12",
        "fan": { "useControl": true, "manualPumpVoltage": 1 }
    })
    .to_string();
    for method in ["ApplyProfile", "SaveCustomProfile"] {
        let req_v12 = IpcRequest {
            id: 202,
            method: method.into(),
            params: json!({ "profile_json": v12_profile }),
        };
        assert_eq!(
            required_access_level(&req_v12),
            IpcAccessLevel::ManageHardware
        );
        let resp_v12 = dispatch_json_rpc_with_identity(&backend, &req_v12, &privileged);
        assert!(
            !resp_v12.ok,
            "{method} must reject manualPumpVoltage == 1 (V12)"
        );
    }

    // Canonical snake_case parameter fallbacks work
    let req_fan = IpcRequest {
        id: 203,
        method: "SetWaterCoolerFanSpeed".into(),
        params: json!({ "duty_cycle_percent": 65 }),
    };
    assert!(dispatch_json_rpc_with_identity(&backend, &req_fan, &interactive).ok);
}

#[test]
fn ipc_monitor_data_since_and_fan_data_zvariant_conversion() {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use ucc_dbus::UccdBackend;
    use zbus::zvariant::{OwnedValue, Value};

    struct TelemetryAndFnLockBackend {
        since_calls: Mutex<Vec<i64>>,
    }

    impl UccdBackend for TelemetryAndFnLockBackend {
        fn get_monitor_data_since(&self, since_timestamp_ms: i64) -> Vec<u8> {
            self.since_calls.lock().unwrap().push(since_timestamp_ms);
            if since_timestamp_ms >= 1_000 {
                vec![0x10, 0x20, 0x30, 0x40]
            } else {
                vec![0x01, 0x02]
            }
        }

        fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
            let mut map = HashMap::new();
            map.insert("u8_val".to_string(), OwnedValue::from(7u8));
            map.insert("active".to_string(), OwnedValue::from(true));
            map.insert("i16_val".to_string(), OwnedValue::from(-12i16));
            map.insert("u16_val".to_string(), OwnedValue::from(3400u16));
            map.insert("temp_c".to_string(), OwnedValue::from(68i32));
            map.insert("rpm".to_string(), OwnedValue::from(4200u32));
            map.insert("ts_ms".to_string(), OwnedValue::from(1_700_000_000_000i64));
            map.insert("ticks".to_string(), OwnedValue::from(99_999u64));
            map.insert("duty_f64".to_string(), OwnedValue::from(55.5f64));
            map.insert("nan_f64".to_string(), OwnedValue::from(f64::NAN));
            map.insert(
                "label".to_string(),
                OwnedValue::try_from(Value::from("cpu-fan")).unwrap(),
            );
            map.insert(
                "curve".to_string(),
                OwnedValue::try_from(Value::from(vec![30i32, 50, 80])).unwrap(),
            );

            let mut nested_dict: HashMap<String, OwnedValue> = HashMap::new();
            nested_dict.insert(
                "mode".to_string(),
                OwnedValue::try_from(Value::from("auto")).unwrap(),
            );
            nested_dict.insert("min_rpm".to_string(), OwnedValue::from(1200u32));
            map.insert(
                "meta".to_string(),
                OwnedValue::try_from(Value::from(nested_dict)).unwrap(),
            );

            let wrapped = Value::Value(Box::new(Value::from("nested-variant")));
            map.insert(
                "wrapped".to_string(),
                OwnedValue::try_from(wrapped).unwrap(),
            );
            map
        }

        fn get_fan_data_gpu1(&self) -> HashMap<String, OwnedValue> {
            let mut map = HashMap::new();
            map.insert("speed".to_string(), OwnedValue::from(62i32));
            map.insert("temp".to_string(), OwnedValue::from(71.25f64));
            let mut tags: HashMap<String, String> = HashMap::new();
            tags.insert("sensor".to_string(), "dgpu1".to_string());
            map.insert(
                "tags".to_string(),
                OwnedValue::try_from(Value::from(tags)).unwrap(),
            );
            map
        }

        fn get_fan_data_gpu2(&self) -> HashMap<String, OwnedValue> {
            let mut map = HashMap::new();
            map.insert("rpm".to_string(), OwnedValue::from(2800u32));
            map.insert(
                "samples".to_string(),
                OwnedValue::try_from(Value::from(vec![25u16, 40, 60])).unwrap(),
            );
            map
        }

        fn set_fn_lock_status(&self, status: bool) -> Result<(), String> {
            if status {
                Ok(())
            } else {
                Err("fn-lock hardware write failed".to_string())
            }
        }
    }

    let backend = TelemetryAndFnLockBackend {
        since_calls: Mutex::new(Vec::new()),
    };
    let caller = CallerIdentity::interactive("IU:pid:5001");

    // GetMonitorDataSince with camelCase and snake_case params
    let mon_camel = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 1,
            method: "GetMonitorDataSince".to_string(),
            params: json!({ "sinceTimestampMs": 1_500 }),
        },
        &caller,
    );
    assert!(mon_camel.ok);
    assert_eq!(mon_camel.result, Some(json!([16, 32, 48, 64])));

    let mon_snake = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 2,
            method: "GetMonitorDataSince".to_string(),
            params: json!({ "since_timestamp_ms": 500 }),
        },
        &caller,
    );
    assert!(mon_snake.ok);
    assert_eq!(mon_snake.result, Some(json!([1, 2])));
    assert_eq!(*backend.since_calls.lock().unwrap(), vec![1_500, 500]);

    let mon_missing = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 3,
            method: "GetMonitorDataSince".to_string(),
            params: json!({}),
        },
        &caller,
    );
    assert!(!mon_missing.ok);
    assert!(mon_missing
        .error
        .as_deref()
        .unwrap_or("")
        .contains("sinceTimestampMs"));

    // GetFanDataCPU nested zvariant::Value conversion
    let cpu_resp = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 4,
            method: "GetFanDataCPU".to_string(),
            params: json!({}),
        },
        &caller,
    );
    assert!(cpu_resp.ok);
    let cpu_obj = cpu_resp.result.expect("cpu fan result");
    assert_eq!(cpu_obj["u8_val"], json!(7));
    assert_eq!(cpu_obj["active"], json!(true));
    assert_eq!(cpu_obj["i16_val"], json!(-12));
    assert_eq!(cpu_obj["u16_val"], json!(3400));
    assert_eq!(cpu_obj["temp_c"], json!(68));
    assert_eq!(cpu_obj["rpm"], json!(4200));
    assert_eq!(cpu_obj["ts_ms"], json!(1_700_000_000_000_i64));
    assert_eq!(cpu_obj["ticks"], json!(99_999));
    assert_eq!(cpu_obj["duty_f64"], json!(55.5));
    assert_eq!(cpu_obj["nan_f64"], json!(null));
    assert_eq!(cpu_obj["label"], json!("cpu-fan"));
    assert_eq!(cpu_obj["curve"], json!([30, 50, 80]));
    assert_eq!(cpu_obj["meta"]["mode"], json!("auto"));
    assert_eq!(cpu_obj["meta"]["min_rpm"], json!(1200));
    assert_eq!(cpu_obj["wrapped"], json!("nested-variant"));

    // GetFanDataGPU1 & GetFanDataGPU2
    let gpu1_resp = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 5,
            method: "GetFanDataGPU1".to_string(),
            params: json!({}),
        },
        &caller,
    );
    assert!(gpu1_resp.ok);
    let gpu1_obj = gpu1_resp.result.expect("gpu1 fan result");
    assert_eq!(gpu1_obj["speed"], json!(62));
    assert_eq!(gpu1_obj["temp"], json!(71.25));
    assert_eq!(gpu1_obj["tags"]["sensor"], json!("dgpu1"));

    let gpu2_resp = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 6,
            method: "GetFanDataGPU2".to_string(),
            params: json!({}),
        },
        &caller,
    );
    assert!(gpu2_resp.ok);
    let gpu2_obj = gpu2_resp.result.expect("gpu2 fan result");
    assert_eq!(gpu2_obj["rpm"], json!(2800));
    assert_eq!(gpu2_obj["samples"], json!([25, 40, 60]));

    // SetFnLockStatus Ok and Err propagation over IPC
    let fn_ok = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 7,
            method: "SetFnLockStatus".to_string(),
            params: json!({ "status": true }),
        },
        &caller,
    );
    assert!(fn_ok.ok);
    assert_eq!(fn_ok.result, Some(json!(true)));
    assert!(fn_ok.error.is_none());

    let fn_err = dispatch_json_rpc_with_identity(
        &backend,
        &IpcRequest {
            id: 8,
            method: "SetFnLockStatus".to_string(),
            params: json!({ "status": false }),
        },
        &caller,
    );
    assert!(!fn_err.ok);
    assert!(fn_err.result.is_none());
    assert_eq!(
        fn_err.error.as_deref(),
        Some("fn-lock hardware write failed")
    );
}

#[test]
fn ipc_register_power_request_passes_caller_principal() {
    use std::sync::Mutex;
    use ucc_dbus::UccdBackend;

    struct PowerRequestSpyBackend {
        registered: Mutex<Vec<(String, String, String, String)>>,
        unregistered: Mutex<Vec<(String, String)>>,
    }

    impl UccdBackend for PowerRequestSpyBackend {
        fn register_power_request_with_sender(
            &self,
            sender: &str,
            name: &str,
            reason: &str,
            mode_val: &str,
        ) -> bool {
            self.registered.lock().unwrap().push((
                sender.to_string(),
                name.to_string(),
                reason.to_string(),
                mode_val.to_string(),
            ));
            true
        }

        fn unregister_power_request_with_sender(&self, sender: &str, name: &str) -> bool {
            self.unregistered
                .lock()
                .unwrap()
                .push((sender.to_string(), name.to_string()));
            true
        }
    }

    let backend = Arc::new(PowerRequestSpyBackend {
        registered: Mutex::new(Vec::new()),
        unregistered: Mutex::new(Vec::new()),
    });

    // Direct dispatch with interactive caller principal
    let interactive = CallerIdentity::interactive("IU:pid:9876");
    let reg_resp = dispatch_json_rpc_with_identity(
        backend.as_ref(),
        &IpcRequest {
            id: 1,
            method: "RegisterPowerRequest".to_string(),
            params: json!({
                "name": "game-boost",
                "reason": "shader compilation",
                "mode": "performance"
            }),
        },
        &interactive,
    );
    assert!(reg_resp.ok);
    assert_eq!(reg_resp.result, Some(json!(true)));

    let unreg_resp = dispatch_json_rpc_with_identity(
        backend.as_ref(),
        &IpcRequest {
            id: 2,
            method: "UnregisterPowerRequest".to_string(),
            params: json!({ "name": "game-boost" }),
        },
        &interactive,
    );
    assert!(unreg_resp.ok);
    assert_eq!(unreg_resp.result, Some(json!(true)));

    // Through NamedPipeServer session with privileged caller principal and mode_val alias
    let transport = MockNamedPipeTransport::default_secure();
    let reg_wire = encode_frame(&IpcRequest {
        id: 3,
        method: "RegisterPowerRequest".to_string(),
        params: json!({
            "name": "backup-job",
            "reason": "nightly snapshot",
            "mode_val": "balanced"
        }),
    })
    .unwrap();
    let unreg_wire = encode_frame(&IpcRequest {
        id: 4,
        method: "UnregisterPowerRequest".to_string(),
        params: json!({ "name": "backup-job" }),
    })
    .unwrap();
    let out = transport.enqueue_session(
        CallerIdentity::privileged("BA:pid:4321"),
        vec![reg_wire, unreg_wire],
    );

    let server = NamedPipeServer::new(transport, backend.clone());
    let stats = server.run_until_shutdown().unwrap();
    assert_eq!(stats.connections_served, 1);
    assert_eq!(stats.requests_served, 2);
    assert_eq!(stats.rejected_frames, 0);

    let resps: Vec<IpcResponse> = out
        .lock()
        .unwrap()
        .iter()
        .map(|w| decode_frame(w).unwrap())
        .collect();
    assert_eq!(resps.len(), 2);
    assert!(resps[0].ok);
    assert!(resps[1].ok);

    assert_eq!(
        *backend.registered.lock().unwrap(),
        vec![
            (
                "IU:pid:9876".to_string(),
                "game-boost".to_string(),
                "shader compilation".to_string(),
                "performance".to_string(),
            ),
            (
                "BA:pid:4321".to_string(),
                "backup-job".to_string(),
                "nightly snapshot".to_string(),
                "balanced".to_string(),
            ),
        ]
    );
    assert_eq!(
        *backend.unregistered.lock().unwrap(),
        vec![
            ("IU:pid:9876".to_string(), "game-boost".to_string()),
            ("BA:pid:4321".to_string(), "backup-job".to_string()),
        ]
    );
}

#[cfg(windows)]
#[test]
fn win32_named_pipe_roundtrip_and_sddl_enforcement() {
    use std::thread;
    use std::time::Duration;
    use ucc_dbus::{NamedPipeError, Win32NamedPipeClient, Win32NamedPipeTransport};

    let pipe_path = format!(r"\\.\pipe\ucc-rusted-test-{}", std::process::id());

    // Reject overly permissive SDDL (Everyone / WD) before creating any Win32 pipe handle.
    let bad_sddl = Win32NamedPipeTransport::new(
        &pipe_path,
        "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GR;;;WD)",
    );
    assert!(matches!(
        bad_sddl,
        Err(NamedPipeError::InvalidAcl(PipeAclError::ForbiddenAllowSid(sid))) if sid == "WD"
    ));

    let transport = Win32NamedPipeTransport::new(&pipe_path, WINDOWS_PIPE_SDDL)
        .expect("valid SDDL must construct Win32NamedPipeTransport");
    assert_eq!(transport.pipe_path(), pipe_path);
    assert_eq!(transport.acl_policy().sddl(), WINDOWS_PIPE_SDDL);

    let server = NamedPipeServer::new(transport, Arc::new(DefaultUccdBackend));
    let server_handle = thread::spawn(move || {
        let mut conn = server
            .transport()
            .accept()
            .expect("accept should succeed")
            .expect("Win32 accept should yield a connection");
        server
            .serve_connection(&mut conn)
            .expect("serve_connection should finish cleanly on client EOF")
    });

    let mut client_opt = None;
    for _ in 0..50 {
        match Win32NamedPipeClient::connect(&pipe_path) {
            Ok(c) => {
                client_opt = Some(c);
                break;
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut client = client_opt.expect("Win32NamedPipeClient should connect to test pipe");

    // Multi-frame roundtrip on the same Win32 Named Pipe connection
    let resp1 = client
        .call(&IpcRequest {
            id: 1,
            method: "GetDeviceName".to_string(),
            params: json!({}),
        })
        .expect("GetDeviceName roundtrip");
    assert!(resp1.ok);
    assert_eq!(resp1.id, 1);
    assert_eq!(resp1.result, Some(json!("ucc-rusted Laptop")));

    let resp2 = client
        .call(&IpcRequest {
            id: 2,
            method: "UccdVersion".to_string(),
            params: json!({}),
        })
        .expect("UccdVersion roundtrip");
    assert!(resp2.ok);
    assert_eq!(resp2.id, 2);
    assert!(resp2
        .result
        .as_ref()
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty()));

    let resp3 = client
        .call(&IpcRequest {
            id: 3,
            method: "SetDisplayBrightness".to_string(),
            params: json!({ "brightness": 65 }),
        })
        .expect("SetDisplayBrightness roundtrip");
    assert!(resp3.ok);
    assert_eq!(resp3.id, 3);
    assert_eq!(resp3.result, Some(json!(true)));

    // Dropping the client closes the pipe handle and triggers clean EOF on the server.
    drop(client);
    let (requests_served, rejected_frames) = server_handle
        .join()
        .expect("Win32 Named Pipe server thread should join cleanly");
    assert_eq!(requests_served, 3);
    assert_eq!(rejected_frames, 0);
}
