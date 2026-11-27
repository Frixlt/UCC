//! Polkit authorization for UCC D-Bus methods.
//!
//! Provides asynchronous Polkit authority verification via `org.freedesktop.PolicyKit1.Authority`
//! and caller identification via `org.freedesktop.DBus`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Semaphore;
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

const MAX_CONCURRENT_READ_POLKIT: usize = 32;
const MAX_PER_SENDER_READ_AUTH: usize = 8;
const MAX_PER_SENDER_READ_QUEUED: usize = 64;
const PER_SENDER_READ_WAIT_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_PER_SENDER_INTERACTIVE_AUTH: usize = 2;

fn interactive_polkit_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(4)))
}

fn read_polkit_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_READ_POLKIT)))
}

#[derive(Debug)]
struct SenderAuthCounts {
    read_sem: Arc<Semaphore>,
    read_queued: usize,
    interactive: usize,
}

impl Default for SenderAuthCounts {
    fn default() -> Self {
        Self {
            read_sem: Arc::new(Semaphore::new(MAX_PER_SENDER_READ_AUTH)),
            read_queued: 0,
            interactive: 0,
        }
    }
}

fn sender_inflight_auth() -> &'static Mutex<HashMap<String, SenderAuthCounts>> {
    static MAP: OnceLock<Mutex<HashMap<String, SenderAuthCounts>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

struct SenderAuthGuard {
    sender: String,
    interactive: bool,
    _read_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

struct SenderQueueSlot {
    sender_key: String,
    sem: Arc<Semaphore>,
}

impl Drop for SenderQueueSlot {
    fn drop(&mut self) {
        let mut map = sender_inflight_auth()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(&self.sender_key) {
            entry.read_queued = entry.read_queued.saturating_sub(1);
            if entry.read_queued == 0 && entry.interactive == 0 {
                map.remove(&self.sender_key);
            }
        }
    }
}

impl SenderAuthGuard {
    async fn acquire(sender_key: &str, interactive: bool) -> Option<Self> {
        if interactive {
            let mut map = sender_inflight_auth()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = map.entry(sender_key.to_string()).or_default();
            if entry.interactive >= MAX_PER_SENDER_INTERACTIVE_AUTH {
                return None;
            }
            entry.interactive += 1;
            return Some(Self {
                sender: sender_key.to_string(),
                interactive: true,
                _read_permit: None,
            });
        }

        let queue_slot = {
            let mut map = sender_inflight_auth()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = map.entry(sender_key.to_string()).or_default();
            if entry.read_queued >= MAX_PER_SENDER_READ_QUEUED {
                return None;
            }
            entry.read_queued += 1;
            SenderQueueSlot {
                sender_key: sender_key.to_string(),
                sem: Arc::clone(&entry.read_sem),
            }
        };

        let permit = match tokio::time::timeout(
            PER_SENDER_READ_WAIT_TIMEOUT,
            Arc::clone(&queue_slot.sem).acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            _ => return None,
        };
        // Transfer queue slot cleanup to SenderAuthGuard::drop alongside the semaphore permit.
        std::mem::forget(queue_slot);
        Some(Self {
            sender: sender_key.to_string(),
            interactive: false,
            _read_permit: Some(permit),
        })
    }
}

impl Drop for SenderAuthGuard {
    fn drop(&mut self) {
        let mut map = sender_inflight_auth()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(&self.sender) {
            if self.interactive {
                entry.interactive = entry.interactive.saturating_sub(1);
            } else {
                entry.read_queued = entry.read_queued.saturating_sub(1);
            }
            if entry.read_queued == 0 && entry.interactive == 0 {
                map.remove(&self.sender);
            }
        }
    }
}

fn should_try_legacy_action(err: &zbus::Error) -> bool {
    let text = err.to_string();
    text.contains("Action") || text.contains("does not exist") || text.contains("NotFound")
}

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
            _ => return false,
        };

        let is_interactive = !matches!(action, AuthAction::Read);
        let sender_key = format!("{}:{sender}", conn.server_guid());
        let Some(_sender_guard) = SenderAuthGuard::acquire(&sender_key, is_interactive).await
        else {
            tracing::warn!(
                "Rejecting Polkit check for sender '{}': concurrent {} limit exceeded",
                sender,
                if is_interactive {
                    "interactive"
                } else {
                    "read"
                }
            );
            return false;
        };

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

        let uid = match dbus_proxy.get_connection_unix_user(bus_name).await {
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

        let authority = match PolicyKitAuthorityProxy::new(conn).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("Failed to connect to PolicyKit1 authority: {e}");
                return false;
            }
        };

        // Race-free D-Bus unique-name subject (CVE-2013-4288 safe, `ISSUE-17`)
        let mut subject_details = HashMap::new();
        subject_details.insert("name".to_string(), Value::from(sender));
        let subject = ("system-bus-name".to_string(), subject_details);
        let details = HashMap::new();
        let (flags, timeout) = match action {
            AuthAction::Read => (0u32, Duration::from_secs(5)),
            AuthAction::Control | AuthAction::ManageHardware => (0x1u32, Duration::from_secs(30)),
        };
        let cancellation_id = "";

        let (_interactive_permit, _read_permit) = if flags != 0 {
            match tokio::time::timeout(
                Duration::from_secs(5),
                Arc::clone(interactive_polkit_semaphore()).acquire_owned(),
            )
            .await
            {
                Ok(Ok(permit)) => (Some(permit), None),
                _ => return false,
            }
        } else {
            match tokio::time::timeout(
                Duration::from_secs(2),
                Arc::clone(read_polkit_semaphore()).acquire_owned(),
            )
            .await
            {
                Ok(Ok(permit)) => (None, Some(permit)),
                _ => return false,
            }
        };

        // Check primary action
        match tokio::time::timeout(
            timeout,
            authority.check_authorization(
                &subject,
                action.action_id(),
                &details,
                flags,
                cancellation_id,
            ),
        )
        .await
        {
            Ok(Ok((is_authorized, _, _))) => {
                return is_authorized;
            }
            Ok(Err(e)) if should_try_legacy_action(&e) => {
                tracing::debug!(
                    "Polkit primary check missing action '{}': {e}, trying legacy",
                    action.action_id()
                );
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    "Polkit primary check error for '{}': {e}",
                    action.action_id()
                );
                return false;
            }
            Err(_) => {
                tracing::warn!(
                    "Polkit primary check timed out for '{}'",
                    action.action_id()
                );
                return false;
            }
        }

        // Fallback to legacy action ID (`de.uniwill.uccd.*`) only when primary action is unregistered
        match tokio::time::timeout(
            timeout,
            authority.check_authorization(
                &subject,
                action.legacy_action_id(),
                &details,
                flags,
                cancellation_id,
            ),
        )
        .await
        {
            Ok(Ok((is_authorized, _, _))) => is_authorized,
            Ok(Err(e)) => {
                tracing::warn!(
                    "Polkit fallback check error for '{}': {e}",
                    action.legacy_action_id()
                );
                false
            }
            Err(_) => {
                tracing::warn!(
                    "Polkit fallback check timed out for '{}'",
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
        self.recorded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
