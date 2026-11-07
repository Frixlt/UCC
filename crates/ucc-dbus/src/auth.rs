//! Polkit authorization for UCC D-Bus methods.
//!
//! Provides asynchronous Polkit authority verification via `org.freedesktop.PolicyKit1.Authority`
//! and caller identification via `org.freedesktop.DBus`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use zbus::fdo::DBusProxy;
use zbus::zvariant::Value;

pub const ACTION_READ: &str = "com.uniwill.uccd.read";
pub const ACTION_CONTROL: &str = "com.uniwill.uccd.control";
pub const ACTION_MANAGE_HARDWARE: &str = "com.uniwill.uccd.manage-hardware";

pub const LEGACY_ACTION_READ: &str = "de.uniwill.uccd.read";
pub const LEGACY_ACTION_CONTROL: &str = "de.uniwill.uccd.control";
pub const LEGACY_ACTION_MANAGE_HARDWARE: &str = "de.uniwill.uccd.manage-hardware";

/// Authorization levels for UCC D-Bus methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthAction {
    /// Read-only queries (hardware status, telemetry, profiles read).
    Read,
    /// Standard control actions (switching profiles, keyboard LED, temporary modes, fan curves).
    Control,
    /// Dangerous/privileged operations (charging thresholds, manual pump voltage, TDP modification).
    ManageHardware,
}

impl AuthAction {
    /// Canonical action ID (`com.uniwill.uccd.*`).
    pub fn action_id(&self) -> &'static str {
        match self {
            Self::Read => ACTION_READ,
            Self::Control => ACTION_CONTROL,
            Self::ManageHardware => ACTION_MANAGE_HARDWARE,
        }
    }

    /// Legacy action ID (`de.uniwill.uccd.*`).
    pub fn legacy_action_id(&self) -> &'static str {
        match self {
            Self::Read => LEGACY_ACTION_READ,
            Self::Control => LEGACY_ACTION_CONTROL,
            Self::ManageHardware => LEGACY_ACTION_MANAGE_HARDWARE,
        }
    }
}

#[zbus::proxy(
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority",
    interface = "org.freedesktop.PolicyKit1.Authority"
)]
pub trait PolicyKitAuthority {
    fn check_authorization(
        &self,
        subject: &(String, HashMap<String, Value<'_>>),
        action_id: &str,
        details: &HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// Abstract authorizer trait for UCC D-Bus calls.
pub trait Authorizer: Send + Sync + 'static {
    fn check_auth<'a>(
        &'a self,
        conn: &'a zbus::Connection,
        sender: Option<&'a str>,
        action: AuthAction,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}

/// Production Polkit authorizer using D-Bus and Polkit Authority.
#[derive(Debug, Default, Clone, Copy)]
pub struct PolkitAuthorizer;

impl PolkitAuthorizer {
    pub fn new() -> Self {
        Self
    }

    async fn check_auth_async(
        conn: &zbus::Connection,
        sender: Option<&str>,
        action: AuthAction,
    ) -> bool {
        let sender = match sender {
            Some(s) if !s.is_empty() => s,
            _ => return true,
        };

        // Environment override for testing / headless dev environments
        if std::env::var("UCCD_DISABLE_AUTH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            return true;
        }

        let bus_name = match zbus::names::BusName::try_from(sender) {
            Ok(name) => name,
            Err(e) => {
                tracing::warn!("Invalid bus name '{sender}': {e}");
                return false;
            }
        };

        let dbus_proxy = match DBusProxy::new(conn).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to connect to D-Bus daemon for auth: {e}");
                return false;
            }
        };

        let uid = match dbus_proxy.get_connection_unix_user(bus_name.clone()).await {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("Failed to query Unix UID for sender '{sender}': {e}");
                return false;
            }
        };

        // Root is always authorized
        if uid == 0 {
            return true;
        }

        let pid = match dbus_proxy.get_connection_unix_process_id(bus_name).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to query Unix PID for sender '{sender}': {e}");
                return false;
            }
        };

        let authority = match PolicyKitAuthorityProxy::new(conn).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("Failed to connect to PolicyKit1 authority: {e}");
                return false;
            }
        };

        let mut subject_details = HashMap::new();
        subject_details.insert("pid".to_string(), Value::U32(pid));
        subject_details.insert("start-time".to_string(), Value::U64(0));
        let subject = ("unix-process".to_string(), subject_details);
        let details = HashMap::new();
        let flags: u32 = 0x1; // AllowUserInteraction
        let cancellation_id = "";

        // Check primary action
        match authority
            .check_authorization(&subject, action.action_id(), &details, flags, cancellation_id)
            .await
        {
            Ok((is_authorized, _, _)) => {
                if is_authorized {
                    return true;
                }
            }
            Err(e) => {
                tracing::debug!(
                    "Polkit primary check error for '{}': {e}, trying legacy",
                    action.action_id()
                );
            }
        }

        // Fallback to legacy action ID (de.uniwill.uccd.*)
        match authority
            .check_authorization(
                &subject,
                action.legacy_action_id(),
                &details,
                flags,
                cancellation_id,
            )
            .await
        {
            Ok((is_authorized, _, _)) => is_authorized,
            Err(e) => {
                tracing::warn!(
                    "Polkit fallback check error for '{}': {e}",
                    action.legacy_action_id()
                );
                false
            }
        }
    }
}

impl Authorizer for PolkitAuthorizer {
    fn check_auth<'a>(
        &'a self,
        conn: &'a zbus::Connection,
        sender: Option<&'a str>,
        action: AuthAction,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(Self::check_auth_async(conn, sender, action))
    }
}

/// Permissive authorizer allowing every caller (useful for unit tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllAuthorizer;

impl Authorizer for AllowAllAuthorizer {
    fn check_auth<'a>(
        &'a self,
        _conn: &'a zbus::Connection,
        _sender: Option<&'a str>,
        _action: AuthAction,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { true })
    }
}

/// Denying authorizer denying every caller (useful for auth rejection tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllAuthorizer;

impl Authorizer for DenyAllAuthorizer {
    fn check_auth<'a>(
        &'a self,
        _conn: &'a zbus::Connection,
        _sender: Option<&'a str>,
        _action: AuthAction,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { false })
    }
}

/// Authorizer that logs and records every auth query (useful for contract testing).
pub struct TrackingAuthorizer {
    pub allow: bool,
    pub recorded: Mutex<Vec<(Option<String>, AuthAction)>>,
}

impl TrackingAuthorizer {
    pub fn new(allow: bool) -> Self {
        Self {
            allow,
            recorded: Mutex::new(Vec::new()),
        }
    }

    pub fn recorded_actions(&self) -> Vec<(Option<String>, AuthAction)> {
        self.recorded.lock().unwrap().clone()
    }
}

impl Authorizer for TrackingAuthorizer {
    fn check_auth<'a>(
        &'a self,
        _conn: &'a zbus::Connection,
        sender: Option<&'a str>,
        action: AuthAction,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        let sender_str = sender.map(|s| s.to_string());
        if let Ok(mut lock) = self.recorded.lock() {
            lock.push((sender_str, action));
        }
        let allow = self.allow;
        Box::pin(async move { allow })
    }
}
