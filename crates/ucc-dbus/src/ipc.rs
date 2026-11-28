//! Transport-agnostic JSON-RPC IPC framing and dispatch (`M15.1`, `ADR-06`).
//!
//! Decouples `UccdBackend` method invocation from Linux D-Bus so Windows builds can expose
//! identical daemon semantics over Windows Named Pipes (`\\.\pipe\ucc-rusted`) while Linux
//! continues serving `com.uniwill.uccd` over D-Bus without any changes.

use crate::auth::AuthAction;
use crate::backend::UccdBackend;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

/// Standard Windows Named Pipe endpoint mirroring `com.uniwill.uccd`.
pub const WINDOWS_NAMED_PIPE_PATH: &str = r"\\.\pipe\ucc-rusted";

/// Maximum permitted IPC frame size in bytes (256 KiB, matching `MAX_DBUS_JSON_PAYLOAD_BYTES`).
pub const MAX_IPC_FRAME_BYTES: usize = 256 * 1024;

/// Forbidden manual water-cooler pump voltage code (`V12` is banned across all interfaces).
pub const FORBIDDEN_PUMP_VOLTAGE_CODE: i32 = 1;

/// Returns `true` when `code` is an allowed manual water-cooler pump voltage code.
///
/// Valid manual codes are `0` (`11V`), `2` (`7V`), `3` (`8V`), and `4` (`Off`).
/// Code `1` (`V12`) is strictly forbidden.
pub const fn is_valid_pump_voltage_code(code: i32) -> bool {
    matches!(code, 0 | 2 | 3 | 4)
}

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

/// Authorization tier required to invoke a given IPC method, mirroring Polkit [`AuthAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpcAccessLevel {
    /// Read-only hardware telemetry, profile inspection, and capability queries.
    Read,
    /// Standard local control operations (profile switching, fan curves, lighting, brightness).
    Control,
    /// Privileged hardware mutations (TDP, `fan.useControl == false`, charging thresholds/type,
    /// cTGP / NVIDIA power offset, manual pump voltage).
    ManageHardware,
}

impl From<IpcAccessLevel> for AuthAction {
    fn from(level: IpcAccessLevel) -> Self {
        match level {
            IpcAccessLevel::Read => Self::Read,
            IpcAccessLevel::Control => Self::Control,
            IpcAccessLevel::ManageHardware => Self::ManageHardware,
        }
    }
}

impl From<AuthAction> for IpcAccessLevel {
    fn from(action: AuthAction) -> Self {
        match action {
            AuthAction::Read => Self::Read,
            AuthAction::Control => Self::Control,
            AuthAction::ManageHardware => Self::ManageHardware,
        }
    }
}

/// Identity and privilege context of an IPC caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerIdentity {
    /// Principal identifier (e.g. Windows SID string, username, or test label).
    pub principal: String,
    /// Whether the caller holds administrator / LocalSystem / root privileges.
    pub is_privileged: bool,
    /// Whether the caller connected from a local interactive/service session (never remote).
    pub is_local: bool,
}

impl CallerIdentity {
    /// Constructs a local privileged caller (LocalSystem / built-in Administrator / root).
    pub fn privileged(principal: impl Into<String>) -> Self {
        Self {
            principal: principal.into(),
            is_privileged: true,
            is_local: true,
        }
    }

    /// Constructs a local interactive non-privileged user caller.
    pub fn interactive(principal: impl Into<String>) -> Self {
        Self {
            principal: principal.into(),
            is_privileged: false,
            is_local: true,
        }
    }

    /// Constructs a remote/network caller (always denied by local IPC policy).
    pub fn remote(principal: impl Into<String>) -> Self {
        Self {
            principal: principal.into(),
            is_privileged: false,
            is_local: false,
        }
    }

    /// Returns `true` if this caller is permitted to perform an operation at `level`.
    pub const fn allows(&self, level: IpcAccessLevel) -> bool {
        if !self.is_local {
            return false;
        }
        match level {
            IpcAccessLevel::Read | IpcAccessLevel::Control => true,
            IpcAccessLevel::ManageHardware => self.is_privileged,
        }
    }
}

/// Errors produced when decoding a bounded IPC JSON frame.
#[derive(Debug, Error)]
pub enum IpcFrameError {
    #[error("IPC frame exceeds maximum size of {max} bytes (got {actual} bytes)")]
    Oversized { actual: usize, max: usize },
    #[error("IPC frame is empty")]
    Empty,
    #[error("IPC frame JSON error: {0}")]
    Json(#[from] serde_json::Error),
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

/// Decodes a single newline-delimited UTF-8 JSON frame, enforcing [`MAX_IPC_FRAME_BYTES`].
pub fn decode_frame<T: for<'de> Deserialize<'de>>(frame: &str) -> Result<T, serde_json::Error> {
    if frame.len() > MAX_IPC_FRAME_BYTES {
        return Err(serde_json::Error::custom(format!(
            "IPC frame exceeds maximum size of {MAX_IPC_FRAME_BYTES} bytes (got {} bytes)",
            frame.len()
        )));
    }
    serde_json::from_str(frame.trim())
}

/// Decodes a single newline-delimited UTF-8 JSON frame with an explicit byte bound.
pub fn decode_frame_bounded<T: for<'de> Deserialize<'de>>(
    frame: &str,
    max_bytes: usize,
) -> Result<T, IpcFrameError> {
    if frame.len() > max_bytes {
        return Err(IpcFrameError::Oversized {
            actual: frame.len(),
            max: max_bytes,
        });
    }
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return Err(IpcFrameError::Empty);
    }
    serde_json::from_str(trimmed).map_err(IpcFrameError::Json)
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

fn i64_param(params: &Value, key: &str) -> Option<i64> {
    params.get(key).and_then(|v| v.as_i64())
}

fn bool_param(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|v| v.as_bool())
}

fn zvariant_to_json(val: &zbus::zvariant::Value<'_>) -> Value {
    match val {
        zbus::zvariant::Value::U8(v) => Value::from(*v),
        zbus::zvariant::Value::Bool(v) => Value::from(*v),
        zbus::zvariant::Value::I16(v) => Value::from(*v),
        zbus::zvariant::Value::U16(v) => Value::from(*v),
        zbus::zvariant::Value::I32(v) => Value::from(*v),
        zbus::zvariant::Value::U32(v) => Value::from(*v),
        zbus::zvariant::Value::I64(v) => Value::from(*v),
        zbus::zvariant::Value::U64(v) => Value::from(*v),
        zbus::zvariant::Value::F64(v) => serde_json::Number::from_f64(*v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        zbus::zvariant::Value::Str(s) => Value::String(s.as_str().to_string()),
        zbus::zvariant::Value::Array(arr) => {
            Value::Array(arr.iter().map(zvariant_to_json).collect())
        }
        zbus::zvariant::Value::Dict(dict) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in dict.iter() {
                let key = match k {
                    zbus::zvariant::Value::Str(s) => s.as_str().to_string(),
                    other => format!("{other:?}"),
                };
                obj.insert(key, zvariant_to_json(v));
            }
            Value::Object(obj)
        }
        zbus::zvariant::Value::Value(inner) => zvariant_to_json(inner),
        _ => Value::Null,
    }
}

fn fan_data_to_json(map: std::collections::HashMap<String, zbus::zvariant::OwnedValue>) -> Value {
    let mut obj = serde_json::Map::with_capacity(map.len());
    for (k, v) in map {
        obj.insert(k, zvariant_to_json(&v));
    }
    Value::Object(obj)
}

fn has_active_charging_value(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Number(n) => n.as_i64() != Some(-1),
        _ => true,
    }
}

fn profile_requires_manage_hardware(profile_json: &str) -> bool {
    let Ok(val) = serde_json::from_str::<Value>(profile_json) else {
        return false;
    };
    let has_custom_tdp = match val.get("odmPowerLimits") {
        Some(Value::Array(arr)) => !arr.is_empty(),
        Some(Value::Object(obj)) => obj
            .get("tdpValues")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| !arr.is_empty()),
        _ => false,
    };
    let disables_fan_control = val
        .get("fan")
        .and_then(|f| f.get("useControl"))
        .and_then(|v| v.as_bool())
        == Some(false);
    let has_top_charging = [
        "chargingProfile",
        "chargingPriority",
        "chargeStartThreshold",
        "chargeEndThreshold",
        "chargeType",
    ]
    .iter()
    .any(|k| val.get(*k).is_some_and(has_active_charging_value));
    let has_nested_charging = val
        .get("charging")
        .and_then(|c| c.as_object())
        .is_some_and(|obj| {
            [
                "chargingProfile",
                "chargingPriority",
                "startThreshold",
                "endThreshold",
                "chargeType",
            ]
            .iter()
            .any(|k| obj.get(*k).is_some_and(has_active_charging_value))
        });
    let has_custom_ctgp = val
        .get("nvidiaPowerCTRLProfile")
        .and_then(|n| n.get("cTGPOffset"))
        .and_then(|v| v.as_i64())
        .is_some_and(|offset| offset != 0)
        || val
            .get("ctgpOffset")
            .and_then(|v| v.as_i64())
            .is_some_and(|offset| offset != 0)
        || val
            .get("nvidiaCTGPOffset")
            .and_then(|v| v.as_i64())
            .is_some_and(|offset| offset != 0);
    let has_manual_pump_voltage = val
        .get("fan")
        .and_then(|f| f.get("manualPumpVoltage"))
        .is_some_and(|v| match v {
            Value::Null => false,
            Value::Number(n) => n.as_i64().is_none_or(|code| code >= 0),
            _ => true,
        });

    has_custom_tdp
        || disables_fan_control
        || has_top_charging
        || has_nested_charging
        || has_custom_ctgp
        || has_manual_pump_voltage
}

/// Validates that `fan.manualPumpVoltage` in a profile JSON payload never carries
/// the forbidden code `1` (`V12`) or any out-of-range non-negative pump voltage code.
fn validate_profile_pump_voltage(req_id: u64, profile_json: &str) -> Result<(), IpcResponse> {
    let Ok(val) = serde_json::from_str::<Value>(profile_json) else {
        return Ok(());
    };
    let Some(raw) = val.get("fan").and_then(|f| f.get("manualPumpVoltage")) else {
        return Ok(());
    };
    if raw.is_null() {
        return Ok(());
    }
    let Some(code_i64) = raw.as_i64() else {
        return Err(IpcResponse::failure(
            req_id,
            "invalid fan.manualPumpVoltage; valid manual codes are 0, 2, 3, 4",
        ));
    };
    if code_i64 < 0 {
        return Ok(());
    }
    let Ok(code) = i32::try_from(code_i64) else {
        return Err(IpcResponse::failure(
            req_id,
            format!("invalid pump voltage code {code_i64}; valid manual codes are 0, 2, 3, 4"),
        ));
    };
    if code == FORBIDDEN_PUMP_VOLTAGE_CODE {
        return Err(IpcResponse::failure(
            req_id,
            "pump voltage code 1 (V12) is forbidden; valid manual codes are 0, 2, 3, 4",
        ));
    }
    if !is_valid_pump_voltage_code(code) {
        return Err(IpcResponse::failure(
            req_id,
            format!("invalid pump voltage code {code}; valid manual codes are 0, 2, 3, 4"),
        ));
    }
    Ok(())
}

/// Determines the [`IpcAccessLevel`] required to invoke `req`.
pub fn required_access_level(req: &IpcRequest) -> IpcAccessLevel {
    match req.method.as_str() {
        "SetChargeStartThreshold"
        | "SetChargeEndThreshold"
        | "SetChargeType"
        | "SetChargingProfile"
        | "SetChargingPriority"
        | "SetNVIDIAPowerOffset"
        | "SetWaterCoolerPumpVoltage" => IpcAccessLevel::ManageHardware,

        "ApplyProfile" | "AddCustomProfile" | "SaveCustomProfile" | "UpdateCustomProfile" => {
            if let Some(profile_json) = str_param(&req.params, "profileJson")
                .or_else(|| str_param(&req.params, "profile_json"))
            {
                if profile_requires_manage_hardware(&profile_json) {
                    return IpcAccessLevel::ManageHardware;
                }
            }
            IpcAccessLevel::Control
        }

        "SetManualPowerMode"
        | "SetActiveProfile"
        | "SetTempProfile"
        | "DeleteCustomProfile"
        | "SetDisplayBrightness"
        | "SetDisplayRefreshRate"
        | "ConsumeModeReapplyPending"
        | "SetWebcam"
        | "SetFnLockStatus"
        | "SetStateMap"
        | "SetBatchStateMap"
        | "SetFanProfileCPU"
        | "SetFanProfileDGPU"
        | "ApplyFanProfiles"
        | "RevertFanProfiles"
        | "SetFanProfile"
        | "SaveCustomFanProfile"
        | "DeleteCustomFanProfile"
        | "SetKeyboardBacklightStatesJSON"
        | "SaveCustomKeyboardProfile"
        | "DeleteCustomKeyboardProfile"
        | "SetLightbarSettingsJSON"
        | "TriggerLightbarAnimation"
        | "SetPowerDeviceSettingsJSON"
        | "ScanPowerDevices"
        | "RegisterPowerRequest"
        | "UnregisterPowerRequest"
        | "EnableWaterCooler"
        | "SetWaterCoolerFanSpeed"
        | "SetWaterCoolerLEDColor"
        | "TurnOffWaterCoolerLED"
        | "TurnOffWaterCoolerFan"
        | "TurnOffWaterCoolerPump"
        | "SetWaterCoolerAutoControl"
        | "SetSensorDataCollectionStatus"
        | "SetDGpuD0Metrics"
        | "SetMonitorHistoryHorizon" => IpcAccessLevel::Control,

        _ => IpcAccessLevel::Read,
    }
}

fn check_payload_size(req_id: u64, param_name: &str, payload: &str) -> Result<(), IpcResponse> {
    if payload.len() > MAX_IPC_FRAME_BYTES {
        let actual = payload.len();
        return Err(IpcResponse::failure(
            req_id,
            format!(
                "param '{param_name}' exceeds max size of {MAX_IPC_FRAME_BYTES} bytes ({actual})"
            ),
        ));
    }
    Ok(())
}

/// Dispatches a transport-agnostic [`IpcRequest`] onto [`UccdBackend`] with caller authorization.
///
/// Privileged hardware mutations (`ManageHardware`: TDP, `fan.useControl == false`, charging
/// thresholds/type, cTGP / NVIDIA power offset, manual pump voltage) require
/// `caller_is_privileged == true`.
pub fn dispatch_json_rpc_authorized(
    backend: &dyn UccdBackend,
    req: &IpcRequest,
    caller_is_privileged: bool,
) -> IpcResponse {
    let caller = if caller_is_privileged {
        CallerIdentity::privileged("privileged")
    } else {
        CallerIdentity::interactive("interactive")
    };
    dispatch_json_rpc_with_identity(backend, req, &caller)
}

/// Dispatches a transport-agnostic [`IpcRequest`] onto [`UccdBackend`] using a full
/// [`CallerIdentity`].
pub fn dispatch_json_rpc_with_identity(
    backend: &dyn UccdBackend,
    req: &IpcRequest,
    caller: &CallerIdentity,
) -> IpcResponse {
    if !caller.is_local {
        return IpcResponse::failure(
            req.id,
            "authorization denied: remote callers are not permitted",
        );
    }

    let required = required_access_level(req);
    if !caller.allows(required) {
        return IpcResponse::failure(
            req.id,
            format!(
                "authorization denied: method '{}' requires privileged ManageHardware access",
                req.method
            ),
        );
    }

    dispatch_inner(backend, req, caller)
}

/// Dispatches a transport-agnostic [`IpcRequest`] onto [`UccdBackend`] in a trusted/privileged
/// caller context.
///
/// Use [`dispatch_json_rpc_authorized`] or [`dispatch_json_rpc_with_identity`] when serving
/// untrusted or interactive client connections.
pub fn dispatch_json_rpc(backend: &dyn UccdBackend, req: &IpcRequest) -> IpcResponse {
    dispatch_json_rpc_authorized(backend, req, true)
}

fn dispatch_inner(
    backend: &dyn UccdBackend,
    req: &IpcRequest,
    caller: &CallerIdentity,
) -> IpcResponse {
    let p = &req.params;
    match req.method.as_str() {
        // --- Read / Telemetry / Capabilities ---
        "GetDeviceName" => IpcResponse::success(req.id, json!(backend.get_device_name())),
        "UccdVersion" => IpcResponse::success(req.id, json!(backend.uccd_version())),
        "IsDeviceSupported" => IpcResponse::success(req.id, json!(backend.is_device_supported())),
        "GetSystemInfoJSON" => IpcResponse::success(req.id, json!(backend.get_system_info_json())),
        "GetDisplayModesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_display_modes_json()))
        }
        "GetDisplayBrightness" => {
            IpcResponse::success(req.id, json!(backend.get_display_brightness()))
        }
        "GetPrimeState" => IpcResponse::success(req.id, json!(backend.get_prime_state())),
        "GetDGpuInfoValuesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_dgpu_info_values_json()))
        }
        "GetIGpuInfoValuesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_igpu_info_values_json()))
        }
        "GetCpuPowerValuesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_cpu_power_values_json()))
        }
        "GetCpuFrequencyLimitsJSON" => {
            IpcResponse::success(req.id, json!(backend.get_cpu_frequency_limits_json()))
        }
        "GetCpuCoreCount" => IpcResponse::success(req.id, json!(backend.get_cpu_core_count())),
        "GetCpuTopologyJSON" => {
            IpcResponse::success(req.id, json!(backend.get_cpu_topology_json()))
        }
        "GetCpuFrequencyMHz" => {
            IpcResponse::success(req.id, json!(backend.get_cpu_frequency_mhz()))
        }
        "GetAvailableGovernors" => {
            IpcResponse::success(req.id, json!(backend.get_available_governors()))
        }
        "GetAvailableEPPs" => IpcResponse::success(req.id, json!(backend.get_available_epps())),
        "GetActiveProfileJSON" => {
            IpcResponse::success(req.id, json!(backend.get_active_profile_json()))
        }
        "GetProfilesJSON" => IpcResponse::success(req.id, json!(backend.get_profiles_json())),
        "GetCustomProfilesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_custom_profiles_json()))
        }
        "GetDefaultProfilesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_default_profiles_json()))
        }
        "GetDefaultValuesProfileJSON" => {
            IpcResponse::success(req.id, json!(backend.get_default_values_profile_json()))
        }
        "GetSettingsJSON" => IpcResponse::success(req.id, json!(backend.get_settings_json())),
        "GetPowerState" => IpcResponse::success(req.id, json!(backend.get_power_state())),
        "GetPowerPolicyJSON" => {
            IpcResponse::success(req.id, json!(backend.get_power_policy_json()))
        }
        "GetPowerDeviceSettingsJSON" => {
            IpcResponse::success(req.id, json!(backend.get_power_device_settings_json()))
        }
        "GetPowerDeviceStatusJSON" => {
            IpcResponse::success(req.id, json!(backend.get_power_device_status_json()))
        }
        "GetLightbarSettingsJSON" => {
            IpcResponse::success(req.id, json!(backend.get_lightbar_settings_json()))
        }
        "GetLightbarStatusJSON" => {
            IpcResponse::success(req.id, json!(backend.get_lightbar_status_json()))
        }
        "GetDriverHealthJSON" => {
            IpcResponse::success(req.id, json!(backend.get_driver_health_json()))
        }
        "GetMuxStateJSON" => IpcResponse::success(req.id, json!(backend.get_mux_state_json())),
        "SetMuxBootMode" => {
            let Some(mode) = str_param(p, "mode") else {
                return IpcResponse::failure(req.id, "missing string param 'mode'");
            };
            IpcResponse::success(req.id, json!(backend.set_mux_boot_mode(&mode)))
        }
        "GetFanProfileNames" => {
            IpcResponse::success(req.id, json!(backend.get_fan_profile_names()))
        }
        "GetFanProfile" => {
            let Some(name) = str_param(p, "name") else {
                return IpcResponse::failure(req.id, "missing string param 'name'");
            };
            IpcResponse::success(req.id, json!(backend.get_fan_profile(&name)))
        }
        "GetFansMinSpeed" => IpcResponse::success(req.id, json!(backend.get_fans_min_speed())),
        "GetFansOffAvailable" => {
            IpcResponse::success(req.id, json!(backend.get_fans_off_available()))
        }
        "GetChargingProfilesAvailable" => {
            IpcResponse::success(req.id, json!(backend.get_charging_profiles_available()))
        }
        "GetCurrentChargingProfile" => {
            IpcResponse::success(req.id, json!(backend.get_current_charging_profile()))
        }
        "GetChargingPrioritiesAvailable" => {
            IpcResponse::success(req.id, json!(backend.get_charging_priorities_available()))
        }
        "GetCurrentChargingPriority" => {
            IpcResponse::success(req.id, json!(backend.get_current_charging_priority()))
        }
        "GetChargeStartAvailableThresholds" => IpcResponse::success(
            req.id,
            json!(backend.get_charge_start_available_thresholds()),
        ),
        "GetChargeEndAvailableThresholds" => {
            IpcResponse::success(req.id, json!(backend.get_charge_end_available_thresholds()))
        }
        "GetChargeStartThreshold" => {
            IpcResponse::success(req.id, json!(backend.get_charge_start_threshold()))
        }
        "GetChargeEndThreshold" => {
            IpcResponse::success(req.id, json!(backend.get_charge_end_threshold()))
        }
        "GetChargeType" => IpcResponse::success(req.id, json!(backend.get_charge_type())),
        "GetFnLockSupported" => {
            IpcResponse::success(req.id, json!(backend.get_fn_lock_supported()))
        }
        "GetFnLockStatus" => IpcResponse::success(req.id, json!(backend.get_fn_lock_status())),
        "GetNVIDIAPowerCTRLAvailable" => {
            IpcResponse::success(req.id, json!(backend.get_nvidia_power_ctrl_available()))
        }
        "GetNVIDIAPowerCTRLDefaultPowerLimit" => IpcResponse::success(
            req.id,
            json!(backend.get_nvidia_power_ctrl_default_power_limit()),
        ),
        "GetNVIDIAPowerCTRLMaxPowerLimit" => IpcResponse::success(
            req.id,
            json!(backend.get_nvidia_power_ctrl_max_power_limit()),
        ),
        "GetCTGPAdjustmentSupported" => {
            IpcResponse::success(req.id, json!(backend.get_ctgp_adjustment_supported()))
        }
        "GetHwpDynamicBoostSupported" => {
            IpcResponse::success(req.id, json!(backend.get_hwp_dynamic_boost_supported()))
        }
        "GetWaterCoolerAvailable" => {
            IpcResponse::success(req.id, json!(backend.get_water_cooler_available()))
        }
        "GetWaterCoolerSupported" => {
            IpcResponse::success(req.id, json!(backend.get_water_cooler_supported()))
        }
        "GetWaterCoolerConnected" => {
            IpcResponse::success(req.id, json!(backend.get_water_cooler_connected()))
        }
        "GetWaterCoolerFanSpeed" => {
            IpcResponse::success(req.id, json!(backend.get_water_cooler_fan_speed()))
        }
        "GetWaterCoolerPumpLevel" => {
            IpcResponse::success(req.id, json!(backend.get_water_cooler_pump_level()))
        }
        "IsWaterCoolerEnabled" => {
            IpcResponse::success(req.id, json!(backend.is_water_cooler_enabled()))
        }
        "IsWaterCoolerAutoControlEnabled" => IpcResponse::success(
            req.id,
            json!(backend.is_water_cooler_auto_control_enabled()),
        ),
        "GetKeyboardBacklightCapabilitiesJSON" => IpcResponse::success(
            req.id,
            json!(backend.get_keyboard_backlight_capabilities_json()),
        ),
        "GetKeyboardBacklightStatesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_keyboard_backlight_states_json()))
        }
        "ODMProfilesAvailable" => {
            IpcResponse::success(req.id, json!(backend.odm_profiles_available()))
        }
        "ODMPowerLimitsJSON" => {
            IpcResponse::success(req.id, json!(backend.odm_power_limits_json()))
        }
        "GetMonitorHistoryHorizon" => {
            IpcResponse::success(req.id, json!(backend.get_monitor_history_horizon()))
        }
        "GetSensorDataCollectionStatus" => {
            IpcResponse::success(req.id, json!(backend.get_sensor_data_collection_status()))
        }
        "GetFanDataCPU" => {
            IpcResponse::success(req.id, fan_data_to_json(backend.get_fan_data_cpu()))
        }
        "GetFanDataGPU1" => {
            IpcResponse::success(req.id, fan_data_to_json(backend.get_fan_data_gpu1()))
        }
        "GetFanDataGPU2" => {
            IpcResponse::success(req.id, fan_data_to_json(backend.get_fan_data_gpu2()))
        }
        "GetCustomFanProfilesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_custom_fan_profiles_json()))
        }
        "GetCustomKeyboardProfilesJSON" => {
            IpcResponse::success(req.id, json!(backend.get_custom_keyboard_profiles_json()))
        }
        "GetMonitorDataSince" => {
            let Some(since_timestamp_ms) =
                i64_param(p, "sinceTimestampMs").or_else(|| i64_param(p, "since_timestamp_ms"))
            else {
                return IpcResponse::failure(req.id, "missing int param 'sinceTimestampMs'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.get_monitor_data_since(since_timestamp_ms)),
            )
        }
        "GetWebcamSWStatus" => IpcResponse::success(req.id, json!(backend.get_webcam_sw_status())),
        "WebcamSWAvailable" => IpcResponse::success(req.id, json!(backend.webcam_sw_available())),
        "GetKeyboardBacklightControlSupported" => IpcResponse::success(
            req.id,
            json!(backend.get_keyboard_backlight_control_supported()),
        ),
        "GetForceYUV420OutputSwitchAvailable" => IpcResponse::success(
            req.id,
            json!(backend.get_force_yuv420_output_switch_available()),
        ),
        "GetIsX11" => IpcResponse::success(req.id, json!(backend.get_is_x11())),
        "TuxedoWmiAvailable" => IpcResponse::success(req.id, json!(backend.tuxedo_wmi_available())),
        "FanHwmonAvailable" => IpcResponse::success(req.id, json!(backend.fan_hwmon_available())),

        // --- Control ---
        "SetManualPowerMode" => {
            let Some(mode) = str_param(p, "mode").or_else(|| str_param(p, "mode_val")) else {
                return IpcResponse::failure(req.id, "missing string param 'mode'");
            };
            IpcResponse::success(req.id, json!(backend.set_manual_power_mode(&mode)))
        }
        "SetActiveProfile" => {
            let Some(id) = str_param(p, "id").or_else(|| str_param(p, "profile_id")) else {
                return IpcResponse::failure(req.id, "missing string param 'id'");
            };
            IpcResponse::success(req.id, json!(backend.set_active_profile(&id)))
        }
        "SetTempProfile" => {
            let Some(profile_name) =
                str_param(p, "profileName").or_else(|| str_param(p, "profile_name"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileName'");
            };
            IpcResponse::success(req.id, json!(backend.set_temp_profile(&profile_name)))
        }
        "ApplyProfile" => {
            let Some(profile_json) =
                str_param(p, "profileJson").or_else(|| str_param(p, "profile_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "profileJson", &profile_json) {
                return resp;
            }
            if let Err(resp) = validate_profile_pump_voltage(req.id, &profile_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.apply_profile(&profile_json)))
        }
        "AddCustomProfile" => {
            let Some(profile_json) =
                str_param(p, "profileJson").or_else(|| str_param(p, "profile_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "profileJson", &profile_json) {
                return resp;
            }
            if let Err(resp) = validate_profile_pump_voltage(req.id, &profile_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.add_custom_profile(&profile_json)))
        }
        "SaveCustomProfile" => {
            let Some(profile_json) =
                str_param(p, "profileJson").or_else(|| str_param(p, "profile_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "profileJson", &profile_json) {
                return resp;
            }
            if let Err(resp) = validate_profile_pump_voltage(req.id, &profile_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.save_custom_profile(&profile_json)))
        }
        "UpdateCustomProfile" => {
            let Some(profile_json) =
                str_param(p, "profileJson").or_else(|| str_param(p, "profile_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "profileJson", &profile_json) {
                return resp;
            }
            if let Err(resp) = validate_profile_pump_voltage(req.id, &profile_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.update_custom_profile(&profile_json)))
        }
        "DeleteCustomProfile" => {
            let Some(profile_id) = str_param(p, "profileId").or_else(|| str_param(p, "profile_id"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileId'");
            };
            IpcResponse::success(req.id, json!(backend.delete_custom_profile(&profile_id)))
        }
        "SetDisplayBrightness" => {
            let Some(brightness) = i32_param(p, "brightness") else {
                return IpcResponse::failure(req.id, "missing int param 'brightness'");
            };
            IpcResponse::success(req.id, json!(backend.set_display_brightness(brightness)))
        }
        "SetDisplayRefreshRate" => {
            let Some(display) = str_param(p, "display") else {
                return IpcResponse::failure(req.id, "missing string param 'display'");
            };
            let Some(refresh_rate) =
                i32_param(p, "refreshRate").or_else(|| i32_param(p, "refresh_rate"))
            else {
                return IpcResponse::failure(req.id, "missing int param 'refreshRate'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.set_display_refresh_rate(&display, refresh_rate)),
            )
        }
        "ConsumeModeReapplyPending" => {
            IpcResponse::success(req.id, json!(backend.consume_mode_reapply_pending()))
        }
        "SaveCustomFanProfile" => {
            let Some(id) = str_param(p, "id") else {
                return IpcResponse::failure(req.id, "missing string param 'id'");
            };
            let Some(name) = str_param(p, "name") else {
                return IpcResponse::failure(req.id, "missing string param 'name'");
            };
            let Some(json_str) = str_param(p, "json") else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            if let Err(resp) = check_payload_size(req.id, "json", &json_str) {
                return resp;
            }
            IpcResponse::success(
                req.id,
                json!(backend.save_custom_fan_profile(&id, &name, &json_str)),
            )
        }
        "DeleteCustomFanProfile" => {
            let Some(id) = str_param(p, "id") else {
                return IpcResponse::failure(req.id, "missing string param 'id'");
            };
            IpcResponse::success(req.id, json!(backend.delete_custom_fan_profile(&id)))
        }
        "SaveCustomKeyboardProfile" => {
            let Some(id) = str_param(p, "id") else {
                return IpcResponse::failure(req.id, "missing string param 'id'");
            };
            let Some(name) = str_param(p, "name") else {
                return IpcResponse::failure(req.id, "missing string param 'name'");
            };
            let Some(json_str) = str_param(p, "json") else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            if let Err(resp) = check_payload_size(req.id, "json", &json_str) {
                return resp;
            }
            IpcResponse::success(
                req.id,
                json!(backend.save_custom_keyboard_profile(&id, &name, &json_str)),
            )
        }
        "DeleteCustomKeyboardProfile" => {
            let Some(id) = str_param(p, "id") else {
                return IpcResponse::failure(req.id, "missing string param 'id'");
            };
            IpcResponse::success(req.id, json!(backend.delete_custom_keyboard_profile(&id)))
        }
        "SetWebcam" => {
            let Some(enabled) = bool_param(p, "enabled") else {
                return IpcResponse::failure(req.id, "missing bool param 'enabled'");
            };
            IpcResponse::success(req.id, json!(backend.set_webcam(enabled)))
        }
        "SetFnLockStatus" => {
            let Some(status) = bool_param(p, "status") else {
                return IpcResponse::failure(req.id, "missing bool param 'status'");
            };
            match backend.set_fn_lock_status(status) {
                Ok(()) => IpcResponse::success(req.id, json!(true)),
                Err(err) => IpcResponse::failure(req.id, err),
            }
        }
        "SetStateMap" => {
            let Some(state) = str_param(p, "state") else {
                return IpcResponse::failure(req.id, "missing string param 'state'");
            };
            let Some(profile_id) = str_param(p, "profileId").or_else(|| str_param(p, "profile_id"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profileId'");
            };
            IpcResponse::success(req.id, json!(backend.set_state_map(&state, &profile_id)))
        }
        "SetBatchStateMap" => {
            let Some(state_map_json) =
                str_param(p, "stateMapJson").or_else(|| str_param(p, "state_map_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'stateMapJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "stateMapJson", &state_map_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.set_batch_state_map(&state_map_json)))
        }
        "SetFanProfileCPU" => {
            let Some(points_json) =
                str_param(p, "pointsJson").or_else(|| str_param(p, "points_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'pointsJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "pointsJson", &points_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.set_fan_profile_cpu(&points_json)))
        }
        "SetFanProfileDGPU" => {
            let Some(points_json) =
                str_param(p, "pointsJson").or_else(|| str_param(p, "points_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'pointsJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "pointsJson", &points_json) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.set_fan_profile_dgpu(&points_json)))
        }
        "ApplyFanProfiles" => {
            let Some(fan_profiles_json) =
                str_param(p, "fanProfilesJson").or_else(|| str_param(p, "fan_profiles_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'fanProfilesJson'");
            };
            if let Err(resp) = check_payload_size(req.id, "fanProfilesJson", &fan_profiles_json) {
                return resp;
            }
            IpcResponse::success(
                req.id,
                json!(backend.apply_fan_profiles(&fan_profiles_json)),
            )
        }
        "RevertFanProfiles" => IpcResponse::success(req.id, json!(backend.revert_fan_profiles())),
        "SetFanProfile" => {
            let Some(name) = str_param(p, "name") else {
                return IpcResponse::failure(req.id, "missing string param 'name'");
            };
            let Some(json_str) = str_param(p, "json") else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            if let Err(resp) = check_payload_size(req.id, "json", &json_str) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.set_fan_profile(&name, &json_str)))
        }
        "SetKeyboardBacklightStatesJSON" => {
            let Some(json_str) =
                str_param(p, "json").or_else(|| str_param(p, "keyboard_backlight_states_json"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            if let Err(resp) = check_payload_size(req.id, "json", &json_str) {
                return resp;
            }
            IpcResponse::success(
                req.id,
                json!(backend.set_keyboard_backlight_states_json(&json_str)),
            )
        }
        "SetLightbarSettingsJSON" => {
            let Some(json_str) = str_param(p, "json") else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            if let Err(resp) = check_payload_size(req.id, "json", &json_str) {
                return resp;
            }
            IpcResponse::success(req.id, json!(backend.set_lightbar_settings_json(&json_str)))
        }
        "TriggerLightbarAnimation" => {
            let Some(force) = bool_param(p, "force") else {
                return IpcResponse::failure(req.id, "missing bool param 'force'");
            };
            IpcResponse::success(req.id, json!(backend.trigger_lightbar_animation(force)))
        }
        "SetPowerDeviceSettingsJSON" => {
            let Some(json_str) = str_param(p, "json") else {
                return IpcResponse::failure(req.id, "missing string param 'json'");
            };
            if let Err(resp) = check_payload_size(req.id, "json", &json_str) {
                return resp;
            }
            IpcResponse::success(
                req.id,
                json!(backend.set_power_device_settings_json(&json_str)),
            )
        }
        "ScanPowerDevices" => IpcResponse::success(req.id, json!(backend.scan_power_devices())),
        "RegisterPowerRequest" => {
            let Some(name) = str_param(p, "name") else {
                return IpcResponse::failure(req.id, "missing string param 'name'");
            };
            let Some(reason) = str_param(p, "reason") else {
                return IpcResponse::failure(req.id, "missing string param 'reason'");
            };
            let Some(mode) = str_param(p, "mode").or_else(|| str_param(p, "mode_val")) else {
                return IpcResponse::failure(req.id, "missing string param 'mode'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.register_power_request_with_sender(
                    &caller.principal,
                    &name,
                    &reason,
                    &mode
                )),
            )
        }
        "UnregisterPowerRequest" => {
            let Some(name) = str_param(p, "name") else {
                return IpcResponse::failure(req.id, "missing string param 'name'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.unregister_power_request_with_sender(&caller.principal, &name)),
            )
        }
        "EnableWaterCooler" => {
            let Some(enable) = bool_param(p, "enable") else {
                return IpcResponse::failure(req.id, "missing bool param 'enable'");
            };
            IpcResponse::success(req.id, json!(backend.enable_water_cooler(enable)))
        }
        "SetWaterCoolerFanSpeed" => {
            let Some(speed) = i32_param(p, "speed").or_else(|| i32_param(p, "duty_cycle_percent"))
            else {
                return IpcResponse::failure(req.id, "missing int param 'speed'");
            };
            IpcResponse::success(req.id, json!(backend.set_water_cooler_fan_speed(speed)))
        }
        "SetWaterCoolerLEDColor" => {
            let Some(red) = i32_param(p, "red") else {
                return IpcResponse::failure(req.id, "missing int param 'red'");
            };
            let Some(green) = i32_param(p, "green") else {
                return IpcResponse::failure(req.id, "missing int param 'green'");
            };
            let Some(blue) = i32_param(p, "blue") else {
                return IpcResponse::failure(req.id, "missing int param 'blue'");
            };
            let Some(mode) = i32_param(p, "mode").or_else(|| i32_param(p, "mode_val")) else {
                return IpcResponse::failure(req.id, "missing int param 'mode'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.set_water_cooler_led_color(red, green, blue, mode)),
            )
        }
        "TurnOffWaterCoolerLED" => {
            IpcResponse::success(req.id, json!(backend.turn_off_water_cooler_led()))
        }
        "TurnOffWaterCoolerFan" => {
            IpcResponse::success(req.id, json!(backend.turn_off_water_cooler_fan()))
        }
        "TurnOffWaterCoolerPump" => {
            IpcResponse::success(req.id, json!(backend.turn_off_water_cooler_pump()))
        }
        "SetWaterCoolerAutoControl" => {
            let Some(enabled) = bool_param(p, "enabled") else {
                return IpcResponse::failure(req.id, "missing bool param 'enabled'");
            };
            IpcResponse::success(
                req.id,
                json!(backend.set_water_cooler_auto_control(enabled)),
            )
        }
        "SetSensorDataCollectionStatus" => {
            let Some(status) = bool_param(p, "status") else {
                return IpcResponse::failure(req.id, "missing bool param 'status'");
            };
            backend.set_sensor_data_collection_status(status);
            IpcResponse::success(req.id, json!(true))
        }
        "SetDGpuD0Metrics" => {
            let Some(status) = bool_param(p, "status") else {
                return IpcResponse::failure(req.id, "missing bool param 'status'");
            };
            backend.set_dgpu_d0_metrics(status);
            IpcResponse::success(req.id, json!(true))
        }
        "SetMonitorHistoryHorizon" => {
            let Some(seconds) = i32_param(p, "seconds") else {
                return IpcResponse::failure(req.id, "missing int param 'seconds'");
            };
            backend.set_monitor_history_horizon(seconds);
            IpcResponse::success(req.id, json!(true))
        }

        // --- Privileged (ManageHardware) ---
        "SetChargingProfile" => {
            let Some(profile) =
                str_param(p, "profile").or_else(|| str_param(p, "profile_descriptor"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'profile'");
            };
            IpcResponse::success(req.id, json!(backend.set_charging_profile(&profile)))
        }
        "SetChargingPriority" => {
            let Some(priority) =
                str_param(p, "priority").or_else(|| str_param(p, "priority_descriptor"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'priority'");
            };
            IpcResponse::success(req.id, json!(backend.set_charging_priority(&priority)))
        }
        "SetChargeStartThreshold" => {
            let Some(value) = i32_param(p, "value") else {
                return IpcResponse::failure(req.id, "missing int param 'value'");
            };
            IpcResponse::success(req.id, json!(backend.set_charge_start_threshold(value)))
        }
        "SetChargeEndThreshold" => {
            let Some(value) = i32_param(p, "value") else {
                return IpcResponse::failure(req.id, "missing int param 'value'");
            };
            IpcResponse::success(req.id, json!(backend.set_charge_end_threshold(value)))
        }
        "SetChargeType" => {
            let Some(charge_type) = str_param(p, "chargeType").or_else(|| str_param(p, "type_val"))
            else {
                return IpcResponse::failure(req.id, "missing string param 'chargeType'");
            };
            IpcResponse::success(req.id, json!(backend.set_charge_type(&charge_type)))
        }
        "SetNVIDIAPowerOffset" => {
            let Some(offset) = i32_param(p, "offset") else {
                return IpcResponse::failure(req.id, "missing int param 'offset'");
            };
            IpcResponse::success(req.id, json!(backend.set_nvidia_power_offset(offset)))
        }
        "SetWaterCoolerPumpVoltage" => {
            let Some(voltage) = i32_param(p, "voltage").or_else(|| i32_param(p, "voltage_code"))
            else {
                return IpcResponse::failure(req.id, "missing int param 'voltage'");
            };
            if voltage == FORBIDDEN_PUMP_VOLTAGE_CODE {
                return IpcResponse::failure(
                    req.id,
                    "pump voltage code 1 (V12) is forbidden; valid manual codes are 0, 2, 3, 4",
                );
            }
            if !is_valid_pump_voltage_code(voltage) {
                return IpcResponse::failure(
                    req.id,
                    format!(
                        "invalid pump voltage code {voltage}; valid manual codes are 0, 2, 3, 4"
                    ),
                );
            }
            IpcResponse::success(
                req.id,
                json!(backend.set_water_cooler_pump_voltage(voltage)),
            )
        }

        other => IpcResponse::failure(req.id, format!("unknown IPC method '{other}'")),
    }
}
