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
