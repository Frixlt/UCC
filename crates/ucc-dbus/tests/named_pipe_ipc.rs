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
