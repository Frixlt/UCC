//! Transport-agnostic JSON-RPC IPC framing and dispatch (`M15.1`, `ADR-06`).
//!
//! Decouples `UccdBackend` method invocation from Linux D-Bus so Windows builds can expose
//! identical daemon semantics over Windows Named Pipes (`\\.\pipe\ucc-rusted`) while Linux
//! continues serving `com.uniwill.uccd` over D-Bus without any changes.

use crate::backend::UccdBackend;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Standard Windows Named Pipe endpoint mirroring `com.uniwill.uccd`.
pub const WINDOWS_NAMED_PIPE_PATH: &str = r"\\.\pipe\ucc-rusted";

/// IPC transport selected for the current platform/daemon runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpcTransportKind {
    Dbus,
    NamedPipe,
}

impl IpcTransportKind {
    #[cfg(unix)]
    pub const fn host_default() -> Self {
        Self::Dbus
    }

    #[cfg(windows)]
    pub const fn host_default() -> Self {
        Self::NamedPipe
    }
}

/// Framed JSON-RPC request matching D-Bus [`UccdBackend`] method names and arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Framed JSON-RPC response returned to CLI / GUI / tray clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IpcResponse {
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }
}

/// Encodes any serializable IPC message as a newline-delimited UTF-8 JSON frame.
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    Ok(line)
}

/// Decodes a single newline-delimited UTF-8 JSON frame.
pub fn decode_frame<T: for<'de> Deserialize<'de>>(frame: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(frame.trim())
}

fn str_param(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn i32_param(params: &Value, key: &str) -> Option<i32> {
    params
        .get(key)
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
}

/// Dispatches a transport-agnostic [`IpcRequest`] onto [`UccdBackend`].
pub fn dispatch_json_rpc(backend: &dyn UccdBackend, req: &IpcRequest) -> IpcResponse {
    let p = &req.params;
    match req.method.as_str() {
        "GetDeviceName" => IpcResponse::success(req.id, json!(backend.get_device_name())),
        "UccdVersion" => IpcResponse::success(req.id, json!(backend.uccd_version())),
        "IsDeviceSupported" => IpcResponse::success(req.id, json!(backend.is_device_supported())),
        "GetSystemInfoJSON" => IpcResponse::success(req.id, json!(backend.get_system_info_json())),
        "GetActiveProfileJSON" => {
            IpcResponse::success(req.id, json!(backend.get_active_profile_json()))
        }
        "GetSettingsJSON" => IpcResponse::success(req.id, json!(backend.get_settings_json())),
        "GetPowerPolicyJSON" => {
            IpcResponse::success(req.id, json!(backend.get_power_policy_json()))
        }
        "SetManualPowerMode" => {
            let Some(mode) = str_param(p, "mode") else {
                return IpcResponse::failure(req.id, "missing string param 'mode'");
            };
            IpcResponse::success(req.id, json!(backend.set_manual_power_mode(&mode)))
        }
        "SetActiveProfile" => {
            let Some(id) = str_param(p, "id") else {
                return IpcResponse::failure(req.id, "missing string param 'id'");
            };
            IpcResponse::success(req.id, json!(backend.set_active_profile(&id)))
        }
        "ApplyProfile" => {
            let Some(profile_json) = str_param(p, "profileJson") else {
                return IpcResponse::failure(req.id, "missing string param 'profileJson'");
            };
            IpcResponse::success(req.id, json!(backend.apply_profile(&profile_json)))
        }
        "SetWaterCoolerFanSpeed" => {
            let Some(speed) = i32_param(p, "speed") else {
                return IpcResponse::failure(req.id, "missing int param 'speed'");
            };
            IpcResponse::success(req.id, json!(backend.set_water_cooler_fan_speed(speed)))
        }
        "SetWaterCoolerPumpVoltage" => {
            let Some(voltage) = i32_param(p, "voltage") else {
                return IpcResponse::failure(req.id, "missing int param 'voltage'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.set_water_cooler_pump_voltage(voltage)),
            )
        }
        "SetLightbarSettingsJSON" => {
            let Some(json_str) = str_param(p, "json") else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            IpcResponse::success(req.id, json!(backend.set_lightbar_settings_json(&json_str)))
        }
        other => IpcResponse::failure(req.id, format!("unknown IPC method '{other}'")),
    }
}
