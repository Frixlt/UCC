//! D-Bus service and client proxy interface for `com.uniwill.uccd` and `de.uniwill.uccd`.
//!
//! Provides D-Bus object server endpoints, client proxies, and signal definitions
//! matching the baseline `de.uniwill.uccd.xml` contract with full Polkit authorization.

pub mod auth;
pub mod backend;
pub mod interface;
pub mod ipc;
pub mod named_pipe;
pub mod proxy;

pub use auth::{
    AllowAllAuthorizer, AuthAction, Authorizer, DenyAllAuthorizer, PolkitAuthorizer,
    TrackingAuthorizer, ACTION_CONTROL, ACTION_MANAGE_HARDWARE, ACTION_READ, LEGACY_ACTION_CONTROL,
    LEGACY_ACTION_MANAGE_HARDWARE, LEGACY_ACTION_READ,
};
pub use backend::{DefaultUccdBackend, UccdBackend};
pub use interface::{UccdInterface, UccdLegacyInterface};
pub use ipc::{
    decode_frame, decode_frame_bounded, dispatch_json_rpc, dispatch_json_rpc_authorized,
    dispatch_json_rpc_with_identity, encode_frame, is_valid_pump_voltage_code,
    required_access_level, CallerIdentity, IpcAccessLevel, IpcFrameError, IpcRequest, IpcResponse,
    IpcTransportKind, FORBIDDEN_PUMP_VOLTAGE_CODE, MAX_IPC_FRAME_BYTES, WINDOWS_NAMED_PIPE_PATH,
};
#[cfg(windows)]
pub use named_pipe::{Win32NamedPipeClient, Win32NamedPipeConnection, Win32NamedPipeTransport};
pub use named_pipe::{
    MockNamedPipeConnection, MockNamedPipeTransport, NamedPipeConnection, NamedPipeError,
    NamedPipeServer, NamedPipeTransport, PipeAce, PipeAclError, PipeAclPolicy, ServerStats,
    WINDOWS_PIPE_SDDL,
};
pub use proxy::{UccdLegacyProxy, UccdProxy};

pub const DBUS_SERVICE: &str = "com.uniwill.uccd";
pub const DBUS_PATH: &str = "/com/uniwill/uccd";
pub const DBUS_PATH_ALT: &str = "/de/uniwill/uccd";
pub const DBUS_INTERFACE: &str = "com.uniwill.uccd";
pub const DBUS_INTERFACE_ALT: &str = "de.uniwill.uccd";

/// Helper to register both primary and legacy interfaces on the D-Bus connection.
pub async fn register_uccd_service<B: UccdBackend>(
    connection: &zbus::Connection,
    backend: std::sync::Arc<B>,
    authorizer: std::sync::Arc<dyn Authorizer>,
) -> zbus::Result<()> {
    let lanes = interface::OffloadLanes::new();
    let primary =
        UccdInterface::new_with_shared_lanes(backend.clone(), authorizer.clone(), lanes.clone());
    let legacy = UccdLegacyInterface::new_with_shared_lanes(
        backend.clone(),
        authorizer.clone(),
        lanes.clone(),
    );

    // Primary path /com/uniwill/uccd hosts both com.uniwill.uccd and de.uniwill.uccd
    connection.object_server().at(DBUS_PATH, primary).await?;
    connection.object_server().at(DBUS_PATH, legacy).await?;

    // Also register on legacy path /de/uniwill/uccd for full transparent alias
    let primary_alt =
        UccdInterface::new_with_shared_lanes(backend.clone(), authorizer.clone(), lanes.clone());
    let legacy_alt = UccdLegacyInterface::new_with_shared_lanes(backend, authorizer, lanes);
    connection
        .object_server()
        .at(DBUS_PATH_ALT, primary_alt)
        .await?;
    connection
        .object_server()
        .at(DBUS_PATH_ALT, legacy_alt)
        .await?;

    Ok(())
}
