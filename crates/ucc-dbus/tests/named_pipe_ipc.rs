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
    assert!(err_default.unwrap_err().to_string().contains("exceeds maximum size"));

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
            method: "SetNvidiaPowerOffset".to_string(),
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
    assert_eq!(interactive_resps[0].result, Some(json!("ucc-rusted Laptop")));
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
