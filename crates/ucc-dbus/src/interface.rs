//! D-Bus Interface definitions for `com.uniwill.uccd` and `de.uniwill.uccd`.

use crate::auth::{AuthAction, Authorizer, PolkitAuthorizer};
use crate::backend::{DefaultUccdBackend, UccdBackend};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

pub const MAX_DBUS_JSON_PAYLOAD_BYTES: usize = 256 * 1024;

/// Subsystem lanes isolating D-Bus offload threads so a stall in one subsystem
/// (e.g. Charging) never starves independent subsystems (`DEEP-02`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffloadLane {
    Read,
    Profile,
    Charging,
    Lighting,
    PowerGpu,
    Cooler,
    Display,
    Misc,
}

fn offload_queue_timeout() -> Duration {
    if let Ok(raw) = std::env::var("UCC_OFFLOAD_QUEUE_TIMEOUT_MS") {
        if let Ok(ms) = raw.trim().parse::<u64>() {
            return Duration::from_millis(ms.max(10));
        }
    }
    Duration::from_millis(500)
}

#[derive(Clone)]
pub(crate) struct OffloadLanes {
    read: Arc<Semaphore>,
    profile: Arc<Semaphore>,
    charging: Arc<Semaphore>,
    lighting: Arc<Semaphore>,
    power_gpu: Arc<Semaphore>,
    cooler: Arc<Semaphore>,
    display: Arc<Semaphore>,
    misc: Arc<Semaphore>,
}

impl OffloadLanes {
    pub(crate) fn new() -> Self {
        Self {
            read: Arc::new(Semaphore::new(16)),
            profile: Arc::new(Semaphore::new(4)),
            charging: Arc::new(Semaphore::new(4)),
            lighting: Arc::new(Semaphore::new(4)),
            power_gpu: Arc::new(Semaphore::new(4)),
            cooler: Arc::new(Semaphore::new(4)),
            display: Arc::new(Semaphore::new(4)),
            misc: Arc::new(Semaphore::new(8)),
        }
    }

    fn semaphore(&self, lane: OffloadLane) -> Arc<Semaphore> {
        match lane {
            OffloadLane::Read => Arc::clone(&self.read),
            OffloadLane::Profile => Arc::clone(&self.profile),
            OffloadLane::Charging => Arc::clone(&self.charging),
            OffloadLane::Lighting => Arc::clone(&self.lighting),
            OffloadLane::PowerGpu => Arc::clone(&self.power_gpu),
            OffloadLane::Cooler => Arc::clone(&self.cooler),
            OffloadLane::Display => Arc::clone(&self.display),
            OffloadLane::Misc => Arc::clone(&self.misc),
        }
    }
}

async fn acquire_lane_permit(
    lanes: &OffloadLanes,
    lane: OffloadLane,
    label: &'static str,
    live_check: Option<(&zbus::Connection, Option<&str>)>,
) -> Result<OwnedSemaphorePermit, String> {
    let wait_start = Instant::now();
    let timeout = offload_queue_timeout();
    let permit = match tokio::time::timeout(timeout, lanes.semaphore(lane).acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => {
            return Err(format!("{label} offload semaphore closed ({lane:?})"));
        }
        Err(_) => {
            return Err(format!(
                "{label} offload lane {lane:?} busy after {timeout:?}"
            ));
        }
    };
    if let Some((conn, Some(sender))) = live_check {
        if wait_start.elapsed() > Duration::from_millis(25) && !sender_is_alive(conn, sender).await
        {
            return Err(format!(
                "{label} caller {sender} disconnected before {lane:?} execution"
            ));
        }
    }
    Ok(permit)
}

fn payload_within_limit(method: &str, payload: &str) -> bool {
    if payload.len() <= MAX_DBUS_JSON_PAYLOAD_BYTES {
        return true;
    }
    tracing::warn!(
        "Rejecting oversized D-Bus JSON payload on {method}: {} bytes (limit {MAX_DBUS_JSON_PAYLOAD_BYTES})",
        payload.len()
    );
    false
}

fn has_active_charging_value(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::String(s) => !s.trim().is_empty(),
        serde_json::Value::Number(n) => n.as_i64() != Some(-1),
        _ => true,
    }
}

fn profile_requires_manage_hardware(profile_json: &str) -> bool {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(profile_json) else {
        return false;
    };
    let has_custom_tdp = match val.get("odmPowerLimits") {
        Some(serde_json::Value::Array(arr)) => !arr.is_empty(),
        Some(serde_json::Value::Object(obj)) => obj
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
    has_custom_tdp || disables_fan_control || has_top_charging || has_nested_charging
}

async fn sender_is_alive(conn: &zbus::Connection, sender: &str) -> bool {
    if !sender.starts_with(':') {
        return true;
    }
    let Ok(proxy) = zbus::fdo::DBusProxy::new(conn).await else {
        return false;
    };
    let Ok(unique) = zbus::names::BusName::try_from(sender) else {
        return false;
    };
    proxy.name_has_owner(unique).await.unwrap_or(false)
}

fn format_panic_payload(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic payload".to_string())
}

async fn offload_or_impl<B, F, R>(
    backend: &Arc<B>,
    lanes: &OffloadLanes,
    lane: OffloadLane,
    label: &'static str,
    live_check: Option<(&zbus::Connection, Option<&str>)>,
    fallback: R,
    f: F,
) -> R
where
    B: UccdBackend,
    F: FnOnce(&B) -> R + Send + 'static,
    R: Send + 'static,
{
    let permit = match acquire_lane_permit(lanes, lane, label, live_check).await {
        Ok(permit) => permit,
        Err(reason) => {
            tracing::warn!("{reason}");
            return fallback;
        }
    };
    let backend = Arc::clone(backend);
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<R, String>>();
    match std::thread::Builder::new()
        .name("uccd-dbus-io".to_string())
        .spawn(move || {
            let _permit = permit;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&backend)))
                .map_err(format_panic_payload);
            let _ = tx.send(outcome);
        }) {
        Ok(_) => match rx.await {
            Ok(Ok(val)) => val,
            Ok(Err(panic_msg)) => {
                tracing::error!("{label} offload thread panicked: {panic_msg}");
                fallback
            }
            Err(_) => {
                tracing::error!("{label} offload thread closed channel unexpectedly");
                fallback
            }
        },
        Err(err) => {
            tracing::error!("Failed to spawn {label} offload thread: {err}");
            fallback
        }
    }
}

async fn offload_result_impl<B, F>(
    backend: &Arc<B>,
    lanes: &OffloadLanes,
    lane: OffloadLane,
    label: &'static str,
    live_check: Option<(&zbus::Connection, Option<&str>)>,
    f: F,
) -> zbus::fdo::Result<()>
where
    B: UccdBackend,
    F: FnOnce(&B) + Send + 'static,
{
    let permit = match acquire_lane_permit(lanes, lane, label, live_check).await {
        Ok(permit) => permit,
        Err(reason) => {
            tracing::warn!("{reason}");
            return Err(zbus::fdo::Error::Failed(reason));
        }
    };
    let backend = Arc::clone(backend);
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    match std::thread::Builder::new()
        .name("uccd-dbus-io".to_string())
        .spawn(move || {
            let _permit = permit;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&backend)))
                .map_err(format_panic_payload);
            let _ = tx.send(outcome);
        }) {
        Ok(_) => match rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(panic_msg)) => {
                tracing::error!("{label} offload thread panicked: {panic_msg}");
                Err(zbus::fdo::Error::Failed(format!(
                    "{label} offload thread panicked: {panic_msg}"
                )))
            }
            Err(_) => {
                tracing::error!("{label} offload thread closed channel unexpectedly");
                Err(zbus::fdo::Error::Failed(format!(
                    "{label} offload thread terminated unexpectedly"
                )))
            }
        },
        Err(err) => {
            tracing::error!("Failed to spawn {label} offload thread: {err}");
            Err(zbus::fdo::Error::Failed(format!(
                "Failed to spawn {label} offload thread: {err}"
            )))
        }
    }
}

async fn offload_fdo_result_impl<B, F>(
    backend: &Arc<B>,
    lanes: &OffloadLanes,
    lane: OffloadLane,
    label: &'static str,
    live_check: Option<(&zbus::Connection, Option<&str>)>,
    f: F,
) -> zbus::fdo::Result<()>
where
    B: UccdBackend,
    F: FnOnce(&B) -> Result<(), String> + Send + 'static,
{
    let permit = match acquire_lane_permit(lanes, lane, label, live_check).await {
        Ok(permit) => permit,
        Err(reason) => {
            tracing::warn!("{reason}");
            return Err(zbus::fdo::Error::Failed(reason));
        }
    };
    let backend = Arc::clone(backend);
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Result<(), String>, String>>();
    match std::thread::Builder::new()
        .name("uccd-dbus-io".to_string())
        .spawn(move || {
            let _permit = permit;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&backend)))
                .map_err(format_panic_payload);
            let _ = tx.send(outcome);
        }) {
        Ok(_) => match rx.await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(msg))) => Err(zbus::fdo::Error::Failed(msg)),
            Ok(Err(panic_msg)) => {
                tracing::error!("{label} offload thread panicked: {panic_msg}");
                Err(zbus::fdo::Error::Failed(format!(
                    "{label} offload thread panicked: {panic_msg}"
                )))
            }
            Err(_) => {
                tracing::error!("{label} offload thread closed channel unexpectedly");
                Err(zbus::fdo::Error::Failed(format!(
                    "{label} offload thread terminated unexpectedly"
                )))
            }
        },
        Err(err) => {
            tracing::error!("Failed to spawn {label} offload thread: {err}");
            Err(zbus::fdo::Error::Failed(format!(
                "Failed to spawn {label} offload thread: {err}"
            )))
        }
    }
}

/// D-Bus interface handler for primary service `com.uniwill.uccd`.
pub struct UccdInterface<B: UccdBackend = DefaultUccdBackend> {
    backend: Arc<B>,
    authorizer: Arc<dyn Authorizer>,
    lanes: OffloadLanes,
}

impl<B: UccdBackend> UccdInterface<B> {
    pub fn new(backend: B) -> Self {
        Self::with_authorizer(backend, Arc::new(PolkitAuthorizer::new()))
    }

    pub fn with_authorizer(backend: B, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            backend: Arc::new(backend),
            authorizer,
            lanes: OffloadLanes::new(),
        }
    }

    pub fn new_with_backend_authorizer(backend: Arc<B>, authorizer: Arc<dyn Authorizer>) -> Self {
        Self::new_with_shared_lanes(backend, authorizer, OffloadLanes::new())
    }

    pub(crate) fn new_with_shared_lanes(
        backend: Arc<B>,
        authorizer: Arc<dyn Authorizer>,
        lanes: OffloadLanes,
    ) -> Self {
        Self {
            backend,
            authorizer,
            lanes,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        &*self.authorizer
    }

    /// Runs potentially blocking hardware/config RPCs on a detached OS thread so neither `zbus`'s
    /// internal executor nor `tokio::runtime::Runtime` shutdown wait on stalled I/O (`M8-HANDOFF-03`),
    /// catching worker panics and spawn failures cleanly (`M8-LIFE-06`).
    async fn offload_or<F, R>(&self, lane: OffloadLane, fallback: R, f: F) -> R
    where
        F: FnOnce(&B) -> R + Send + 'static,
        R: Send + 'static,
    {
        offload_or_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdInterface",
            None,
            fallback,
            f,
        )
        .await
    }

    async fn offload_or_checked<F, R>(
        &self,
        lane: OffloadLane,
        conn: &zbus::Connection,
        sender: Option<&str>,
        fallback: R,
        f: F,
    ) -> R
    where
        F: FnOnce(&B) -> R + Send + 'static,
        R: Send + 'static,
    {
        offload_or_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdInterface",
            Some((conn, sender)),
            fallback,
            f,
        )
        .await
    }

    async fn offload_result<F>(&self, lane: OffloadLane, f: F) -> zbus::fdo::Result<()>
    where
        F: FnOnce(&B) + Send + 'static,
    {
        offload_result_impl(&self.backend, &self.lanes, lane, "UccdInterface", None, f).await
    }

    async fn offload_fdo_result<F>(
        &self,
        lane: OffloadLane,
        conn: &zbus::Connection,
        sender: Option<&str>,
        f: F,
    ) -> zbus::fdo::Result<()>
    where
        F: FnOnce(&B) -> Result<(), String> + Send + 'static,
    {
        offload_fdo_result_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdInterface",
            Some((conn, sender)),
            f,
        )
        .await
    }
}

impl Default for UccdInterface<DefaultUccdBackend> {
    fn default() -> Self {
        Self::new(DefaultUccdBackend)
    }
}

/// D-Bus interface handler for legacy alias `de.uniwill.uccd`.
pub struct UccdLegacyInterface<B: UccdBackend = DefaultUccdBackend> {
    backend: Arc<B>,
    authorizer: Arc<dyn Authorizer>,
    lanes: OffloadLanes,
}

impl<B: UccdBackend> UccdLegacyInterface<B> {
    pub fn new(backend: B) -> Self {
        Self::with_authorizer(backend, Arc::new(PolkitAuthorizer::new()))
    }

    pub fn with_authorizer(backend: B, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            backend: Arc::new(backend),
            authorizer,
            lanes: OffloadLanes::new(),
        }
    }

    pub fn new_with_backend_authorizer(backend: Arc<B>, authorizer: Arc<dyn Authorizer>) -> Self {
        Self::new_with_shared_lanes(backend, authorizer, OffloadLanes::new())
    }

    pub(crate) fn new_with_shared_lanes(
        backend: Arc<B>,
        authorizer: Arc<dyn Authorizer>,
        lanes: OffloadLanes,
    ) -> Self {
        Self {
            backend,
            authorizer,
            lanes,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        &*self.authorizer
    }

    async fn offload_or<F, R>(&self, lane: OffloadLane, fallback: R, f: F) -> R
    where
        F: FnOnce(&B) -> R + Send + 'static,
        R: Send + 'static,
    {
        offload_or_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdLegacyInterface",
            None,
            fallback,
            f,
        )
        .await
    }

    async fn offload_or_checked<F, R>(
        &self,
        lane: OffloadLane,
        conn: &zbus::Connection,
        sender: Option<&str>,
        fallback: R,
        f: F,
    ) -> R
    where
        F: FnOnce(&B) -> R + Send + 'static,
        R: Send + 'static,
    {
        offload_or_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdLegacyInterface",
            Some((conn, sender)),
            fallback,
            f,
        )
        .await
    }

    async fn offload_result<F>(&self, lane: OffloadLane, f: F) -> zbus::fdo::Result<()>
    where
        F: FnOnce(&B) + Send + 'static,
    {
        offload_result_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdLegacyInterface",
            None,
            f,
        )
        .await
    }

    async fn offload_fdo_result<F>(
        &self,
        lane: OffloadLane,
        conn: &zbus::Connection,
        sender: Option<&str>,
        f: F,
    ) -> zbus::fdo::Result<()>
    where
        F: FnOnce(&B) -> Result<(), String> + Send + 'static,
    {
        offload_fdo_result_impl(
            &self.backend,
            &self.lanes,
            lane,
            "UccdLegacyInterface",
            Some((conn, sender)),
            f,
        )
        .await
    }
}

impl Default for UccdLegacyInterface<DefaultUccdBackend> {
    fn default() -> Self {
        Self::new(DefaultUccdBackend)
    }
}

macro_rules! impl_uccd_methods {
    ($struct_name:ident, $iface_name:literal) => {
        #[zbus::interface(name = $iface_name)]
        impl<B: UccdBackend> $struct_name<B> {
            // Signals
            #[zbus(signal)]
            pub async fn profile_changed(
                emitter: &SignalEmitter<'_>,
                profile_id: &str,
                keyboard_profile_id: &str,
                fan_profile_id: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn mode_reapply_pending_changed(
                emitter: &SignalEmitter<'_>,
                pending: bool,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn power_state_changed(
                emitter: &SignalEmitter<'_>,
                state: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn power_policy_changed(
                emitter: &SignalEmitter<'_>,
                json: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn power_device_status_changed(
                emitter: &SignalEmitter<'_>,
                json: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn lightbar_settings_changed(
                emitter: &SignalEmitter<'_>,
                json: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn water_cooler_status_changed(
                emitter: &SignalEmitter<'_>,
                status: &str,
            ) -> zbus::Result<()>;

            // Methods (122)
            #[zbus(name = "GetDeviceName")]
            pub async fn get_device_name(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.backend.get_device_name()
            }

            #[zbus(name = "GetSystemInfoJSON")]
            pub async fn get_system_info_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_system_info_json()
            }

            #[zbus(name = "IsDeviceSupported")]
            pub async fn is_device_supported(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.is_device_supported()
            }

            #[zbus(name = "GetDisplayModesJSON")]
            pub async fn get_display_modes_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_display_modes_json()
            }

            #[zbus(name = "GetIsX11")]
            pub async fn get_is_x11(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_is_x11()
            }

            #[zbus(name = "TuxedoWmiAvailable")]
            pub async fn tuxedo_wmi_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.tuxedo_wmi_available()
            }

            #[zbus(name = "FanHwmonAvailable")]
            pub async fn fan_hwmon_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.fan_hwmon_available()
            }

            #[zbus(name = "UccdVersion")]
            pub async fn uccd_version(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.backend.uccd_version()
            }

            #[zbus(name = "GetFanDataCPU")]
            pub async fn get_fan_data_cpu(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> HashMap<String, OwnedValue> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return HashMap::new();
                }
                self.backend.get_fan_data_cpu()
            }

            #[zbus(name = "GetFanDataGPU1")]
            pub async fn get_fan_data_gpu1(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> HashMap<String, OwnedValue> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return HashMap::new();
                }
                self.backend.get_fan_data_gpu1()
            }

            #[zbus(name = "GetFanDataGPU2")]
            pub async fn get_fan_data_gpu2(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> HashMap<String, OwnedValue> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return HashMap::new();
                }
                self.backend.get_fan_data_gpu2()
            }

            #[zbus(name = "WebcamSWAvailable")]
            pub async fn webcam_sw_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.webcam_sw_available()
            }

            #[zbus(name = "GetWebcamSWStatus")]
            pub async fn get_webcam_sw_status(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_webcam_sw_status()
            }

            #[zbus(name = "SetWebcam")]
            pub async fn set_webcam(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                enabled: bool,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Display,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_webcam(enabled),
                )
                .await
            }

            #[zbus(name = "GetForceYUV420OutputSwitchAvailable")]
            pub async fn get_force_yuv420_output_switch_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_force_yuv420_output_switch_available()
            }

            #[zbus(name = "GetDisplayBrightness")]
            pub async fn get_display_brightness(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return -1;
                }
                self.backend.get_display_brightness()
            }

            #[zbus(name = "GetKeyboardBacklightControlSupported")]
            pub async fn get_keyboard_backlight_control_supported(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_keyboard_backlight_control_supported()
            }

            #[zbus(name = "SetDisplayBrightness")]
            pub async fn set_display_brightness(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                brightness: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Display,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_display_brightness(brightness),
                )
                .await
            }

            #[zbus(name = "SetDisplayRefreshRate")]
            pub async fn set_display_refresh_rate(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                display: &str,
                refresh_rate: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let display = display.to_owned();
                self.offload_or_checked(
                    OffloadLane::Display,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_display_refresh_rate(&display, refresh_rate),
                )
                .await
            }

            #[zbus(name = "GetDGpuInfoValuesJSON")]
            pub async fn get_dgpu_info_values_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_dgpu_info_values_json()
            }

            #[zbus(name = "GetIGpuInfoValuesJSON")]
            pub async fn get_igpu_info_values_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_igpu_info_values_json()
            }

            #[zbus(name = "GetCpuPowerValuesJSON")]
            pub async fn get_cpu_power_values_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_cpu_power_values_json()
            }

            #[zbus(name = "GetPrimeState")]
            pub async fn get_prime_state(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.offload_or(OffloadLane::PowerGpu, String::new(), |b| {
                    b.get_prime_state()
                })
                .await
            }

            #[zbus(name = "ConsumeModeReapplyPending")]
            pub async fn consume_mode_reapply_pending(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.backend.consume_mode_reapply_pending()
            }

            #[zbus(name = "GetActiveProfileJSON")]
            pub async fn get_active_profile_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_active_profile_json()
            }

            #[zbus(name = "GetPowerState")]
            pub async fn get_power_state(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.backend.get_power_state()
            }

            #[zbus(name = "GetPowerPolicyJSON")]
            pub async fn get_power_policy_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_power_policy_json()
            }

            #[zbus(name = "GetPowerDeviceSettingsJSON")]
            pub async fn get_power_device_settings_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_power_device_settings_json()
            }

            #[zbus(name = "GetPowerDeviceStatusJSON")]
            pub async fn get_power_device_status_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_power_device_status_json()
            }

            #[zbus(name = "SetPowerDeviceSettingsJSON")]
            pub async fn set_power_device_settings_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                json: &str,
            ) -> bool {
                if !payload_within_limit("SetPowerDeviceSettingsJSON", json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let json = json.to_owned();
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_power_device_settings_json(&json),
                )
                .await
            }

            #[zbus(name = "ScanPowerDevices")]
            pub async fn scan_power_devices(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    |b| b.scan_power_devices(),
                )
                .await
            }

            #[zbus(name = "GetLightbarSettingsJSON")]
            pub async fn get_lightbar_settings_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_lightbar_settings_json()
            }

            #[zbus(name = "GetLightbarStatusJSON")]
            pub async fn get_lightbar_status_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_lightbar_status_json()
            }

            #[zbus(name = "GetDriverHealthJSON")]
            pub async fn get_driver_health_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_driver_health_json()
            }

            #[zbus(name = "GetMuxStateJSON")]
            pub async fn get_mux_state_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return r#"{"available":false}"#.to_string();
                }
                self.offload_or_checked(
                    OffloadLane::Read,
                    conn,
                    sender,
                    r#"{"available":false}"#.to_string(),
                    |b| b.get_mux_state_json(),
                )
                .await
            }

            #[zbus(name = "SetMuxBootMode")]
            pub async fn set_mux_boot_mode(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                mode: &str,
            ) -> bool {
                if !payload_within_limit("SetMuxBootMode", mode) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let mode = mode.to_owned();
                self.offload_or_checked(OffloadLane::PowerGpu, conn, sender, false, move |b| {
                    b.set_mux_boot_mode(&mode)
                })
                .await
            }

            #[zbus(name = "SetLightbarSettingsJSON")]
            pub async fn set_lightbar_settings_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                json: &str,
            ) -> bool {
                if !payload_within_limit("SetLightbarSettingsJSON", json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let json = json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Lighting,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_lightbar_settings_json(&json),
                )
                .await
            }

            #[zbus(name = "TriggerLightbarAnimation")]
            pub async fn trigger_lightbar_animation(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                force: bool,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Lighting,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.trigger_lightbar_animation(force),
                )
                .await
            }

            #[zbus(name = "SetManualPowerMode")]
            pub async fn set_manual_power_mode(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                mode_val: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let mode_val = mode_val.to_owned();
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_manual_power_mode(&mode_val),
                )
                .await
            }

            #[zbus(name = "RegisterPowerRequest")]
            pub async fn register_power_request(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                name: &str,
                reason: &str,
                mode_val: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let sender_owned = sender.unwrap_or("").to_owned();
                if !sender_is_alive(conn, &sender_owned).await {
                    return false;
                }
                let sender_for_req = sender_owned.clone();
                let name_owned = name.to_owned();
                let name_for_req = name_owned.clone();
                let reason = reason.to_owned();
                let mode_val = mode_val.to_owned();
                let ok = self
                    .offload_or_checked(
                        OffloadLane::PowerGpu,
                        conn,
                        header.sender().map(|s| s.as_str()),
                        false,
                        move |b| {
                            b.register_power_request_with_sender(
                                &sender_for_req,
                                &name_for_req,
                                &reason,
                                &mode_val,
                            )
                        },
                    )
                    .await;
                if ok && !sender_is_alive(conn, &sender_owned).await {
                    let sender_cleanup = sender_owned;
                    let name_cleanup = name_owned;
                    self.offload_or(OffloadLane::PowerGpu, false, move |b| {
                        b.unregister_power_request_with_sender(&sender_cleanup, &name_cleanup)
                    })
                    .await;
                    return false;
                }
                ok
            }

            #[zbus(name = "UnregisterPowerRequest")]
            pub async fn unregister_power_request(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                name: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let sender_owned = sender.unwrap_or("").to_owned();
                let name = name.to_owned();
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.unregister_power_request_with_sender(&sender_owned, &name),
                )
                .await
            }

            #[zbus(name = "SetTempProfile")]
            pub async fn set_temp_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_name: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let profile_name = profile_name.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_temp_profile(&profile_name),
                )
                .await
            }

            #[zbus(name = "SetActiveProfile")]
            pub async fn set_active_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                id: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let id = id.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_active_profile(&id),
                )
                .await
            }

            #[zbus(name = "ApplyProfile")]
            pub async fn apply_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_json: &str,
            ) -> bool {
                if !payload_within_limit("ApplyProfile", profile_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                let action = if profile_requires_manage_hardware(profile_json) {
                    AuthAction::ManageHardware
                } else {
                    AuthAction::Control
                };
                if !self.authorizer.check_auth(conn, sender, action).await {
                    return false;
                }
                let profile_json = profile_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.apply_profile(&profile_json),
                )
                .await
            }

            #[zbus(name = "GetProfilesJSON")]
            pub async fn get_profiles_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_profiles_json()
            }

            #[zbus(name = "GetCustomProfilesJSON")]
            pub async fn get_custom_profiles_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_custom_profiles_json()
            }

            #[zbus(name = "SetFanProfileCPU")]
            pub async fn set_fan_profile_cpu(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                points_json: &str,
            ) -> bool {
                if !payload_within_limit("SetFanProfileCPU", points_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let points_json = points_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_fan_profile_cpu(&points_json),
                )
                .await
            }

            #[zbus(name = "SetFanProfileDGPU")]
            pub async fn set_fan_profile_dgpu(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                points_json: &str,
            ) -> bool {
                if !payload_within_limit("SetFanProfileDGPU", points_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let points_json = points_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_fan_profile_dgpu(&points_json),
                )
                .await
            }

            #[zbus(name = "ApplyFanProfiles")]
            pub async fn apply_fan_profiles(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                fan_profiles_json: &str,
            ) -> bool {
                if !payload_within_limit("ApplyFanProfiles", fan_profiles_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let fan_profiles_json = fan_profiles_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.apply_fan_profiles(&fan_profiles_json),
                )
                .await
            }

            #[zbus(name = "RevertFanProfiles")]
            pub async fn revert_fan_profiles(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    |b| b.revert_fan_profiles(),
                )
                .await
            }

            #[zbus(name = "GetDefaultProfilesJSON")]
            pub async fn get_default_profiles_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_default_profiles_json()
            }

            #[zbus(name = "GetCpuFrequencyLimitsJSON")]
            pub async fn get_cpu_frequency_limits_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_cpu_frequency_limits_json()
            }

            #[zbus(name = "GetDefaultValuesProfileJSON")]
            pub async fn get_default_values_profile_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_default_values_profile_json()
            }

            #[zbus(name = "AddCustomProfile")]
            pub async fn add_custom_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_json: &str,
            ) -> bool {
                if !payload_within_limit("AddCustomProfile", profile_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                let action = if profile_requires_manage_hardware(profile_json) {
                    AuthAction::ManageHardware
                } else {
                    AuthAction::Control
                };
                if !self.authorizer.check_auth(conn, sender, action).await {
                    return false;
                }
                let profile_json = profile_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.add_custom_profile(&profile_json),
                )
                .await
            }

            #[zbus(name = "SaveCustomProfile")]
            pub async fn save_custom_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_json: &str,
            ) -> bool {
                if !payload_within_limit("SaveCustomProfile", profile_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                let action = if profile_requires_manage_hardware(profile_json) {
                    AuthAction::ManageHardware
                } else {
                    AuthAction::Control
                };
                if !self.authorizer.check_auth(conn, sender, action).await {
                    return false;
                }
                let profile_json = profile_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.save_custom_profile(&profile_json),
                )
                .await
            }

            #[zbus(name = "DeleteCustomProfile")]
            pub async fn delete_custom_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_id: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let profile_id = profile_id.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.delete_custom_profile(&profile_id),
                )
                .await
            }

            #[zbus(name = "UpdateCustomProfile")]
            pub async fn update_custom_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_json: &str,
            ) -> bool {
                if !payload_within_limit("UpdateCustomProfile", profile_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                let action = if profile_requires_manage_hardware(profile_json) {
                    AuthAction::ManageHardware
                } else {
                    AuthAction::Control
                };
                if !self.authorizer.check_auth(conn, sender, action).await {
                    return false;
                }
                let profile_json = profile_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.update_custom_profile(&profile_json),
                )
                .await
            }

            #[zbus(name = "GetFanProfile")]
            pub async fn get_fan_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                name: &str,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_fan_profile(name)
            }

            #[zbus(name = "GetFanProfileNames")]
            pub async fn get_fan_profile_names(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_fan_profile_names()
            }

            #[zbus(name = "SetFanProfile")]
            pub async fn set_fan_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                name: &str,
                json: &str,
            ) -> bool {
                if !payload_within_limit("SetFanProfile", json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let name = name.to_owned();
                let json = json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_fan_profile(&name, &json),
                )
                .await
            }

            #[zbus(name = "GetSettingsJSON")]
            pub async fn get_settings_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_settings_json()
            }

            #[zbus(name = "SetStateMap")]
            pub async fn set_state_map(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                state: &str,
                profile_id: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let state = state.to_owned();
                let profile_id = profile_id.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_state_map(&state, &profile_id),
                )
                .await
            }

            #[zbus(name = "SetBatchStateMap")]
            pub async fn set_batch_state_map(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                state_map_json: &str,
            ) -> bool {
                if !payload_within_limit("SetBatchStateMap", state_map_json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let state_map_json = state_map_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_batch_state_map(&state_map_json),
                )
                .await
            }

            #[zbus(name = "ODMProfilesAvailable")]
            pub async fn odm_profiles_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> Vec<String> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return Vec::new();
                }
                self.backend.odm_profiles_available()
            }

            #[zbus(name = "ODMPowerLimitsJSON")]
            pub async fn odm_power_limits_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.odm_power_limits_json()
            }

            #[zbus(name = "GetKeyboardBacklightCapabilitiesJSON")]
            pub async fn get_keyboard_backlight_capabilities_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "{}".to_string();
                }
                self.backend.get_keyboard_backlight_capabilities_json()
            }

            #[zbus(name = "GetKeyboardBacklightStatesJSON")]
            pub async fn get_keyboard_backlight_states_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_keyboard_backlight_states_json()
            }

            #[zbus(name = "SetKeyboardBacklightStatesJSON")]
            pub async fn set_keyboard_backlight_states_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                keyboard_backlight_states_json: &str,
            ) -> bool {
                if !payload_within_limit(
                    "SetKeyboardBacklightStatesJSON",
                    keyboard_backlight_states_json,
                ) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let keyboard_backlight_states_json = keyboard_backlight_states_json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Lighting,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_keyboard_backlight_states_json(&keyboard_backlight_states_json),
                )
                .await
            }

            #[zbus(name = "GetCustomKeyboardProfilesJSON")]
            pub async fn get_custom_keyboard_profiles_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_custom_keyboard_profiles_json()
            }

            #[zbus(name = "SaveCustomKeyboardProfile")]
            pub async fn save_custom_keyboard_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                id: &str,
                name: &str,
                json: &str,
            ) -> bool {
                if !payload_within_limit("SaveCustomKeyboardProfile", json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let id = id.to_owned();
                let name = name.to_owned();
                let json = json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Lighting,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.save_custom_keyboard_profile(&id, &name, &json),
                )
                .await
            }

            #[zbus(name = "DeleteCustomKeyboardProfile")]
            pub async fn delete_custom_keyboard_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                id: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let id = id.to_owned();
                self.offload_or_checked(
                    OffloadLane::Lighting,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.delete_custom_keyboard_profile(&id),
                )
                .await
            }

            #[zbus(name = "GetCustomFanProfilesJSON")]
            pub async fn get_custom_fan_profiles_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_custom_fan_profiles_json()
            }

            #[zbus(name = "SaveCustomFanProfile")]
            pub async fn save_custom_fan_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                id: &str,
                name: &str,
                json: &str,
            ) -> bool {
                if !payload_within_limit("SaveCustomFanProfile", json) {
                    return false;
                }
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let id = id.to_owned();
                let name = name.to_owned();
                let json = json.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.save_custom_fan_profile(&id, &name, &json),
                )
                .await
            }

            #[zbus(name = "DeleteCustomFanProfile")]
            pub async fn delete_custom_fan_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                id: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let id = id.to_owned();
                self.offload_or_checked(
                    OffloadLane::Profile,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.delete_custom_fan_profile(&id),
                )
                .await
            }

            #[zbus(name = "GetFansMinSpeed")]
            pub async fn get_fans_min_speed(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return 0;
                }
                self.backend.get_fans_min_speed()
            }

            #[zbus(name = "GetFansOffAvailable")]
            pub async fn get_fans_off_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_fans_off_available()
            }

            #[zbus(name = "GetChargingProfilesAvailable")]
            pub async fn get_charging_profiles_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_charging_profiles_available()
            }

            #[zbus(name = "GetCurrentChargingProfile")]
            pub async fn get_current_charging_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.backend.get_current_charging_profile()
            }

            #[zbus(name = "SetChargingProfile")]
            pub async fn set_charging_profile(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                profile_descriptor: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let profile_descriptor = profile_descriptor.to_owned();
                self.offload_or_checked(
                    OffloadLane::Charging,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_charging_profile(&profile_descriptor),
                )
                .await
            }

            #[zbus(name = "GetChargingPrioritiesAvailable")]
            pub async fn get_charging_priorities_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_charging_priorities_available()
            }

            #[zbus(name = "GetCurrentChargingPriority")]
            pub async fn get_current_charging_priority(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.backend.get_current_charging_priority()
            }

            #[zbus(name = "SetChargingPriority")]
            pub async fn set_charging_priority(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                priority_descriptor: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                let priority_descriptor = priority_descriptor.to_owned();
                self.offload_or_checked(
                    OffloadLane::Charging,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_charging_priority(&priority_descriptor),
                )
                .await
            }

            #[zbus(name = "GetChargeStartAvailableThresholds")]
            pub async fn get_charge_start_available_thresholds(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_charge_start_available_thresholds()
            }

            #[zbus(name = "GetChargeEndAvailableThresholds")]
            pub async fn get_charge_end_available_thresholds(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_charge_end_available_thresholds()
            }

            #[zbus(name = "GetChargeStartThreshold")]
            pub async fn get_charge_start_threshold(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return -1;
                }
                self.backend.get_charge_start_threshold()
            }

            #[zbus(name = "GetChargeEndThreshold")]
            pub async fn get_charge_end_threshold(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return -1;
                }
                self.backend.get_charge_end_threshold()
            }

            #[zbus(name = "SetChargeStartThreshold")]
            pub async fn set_charge_start_threshold(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                value: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::ManageHardware)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Charging,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_charge_start_threshold(value),
                )
                .await
            }

            #[zbus(name = "SetChargeEndThreshold")]
            pub async fn set_charge_end_threshold(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                value: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::ManageHardware)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Charging,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_charge_end_threshold(value),
                )
                .await
            }

            #[zbus(name = "GetChargeType")]
            pub async fn get_charge_type(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return String::new();
                }
                self.backend.get_charge_type()
            }

            #[zbus(name = "SetChargeType")]
            pub async fn set_charge_type(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                type_val: &str,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::ManageHardware)
                    .await
                {
                    return false;
                }
                let type_val = type_val.to_owned();
                self.offload_or_checked(
                    OffloadLane::Charging,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_charge_type(&type_val),
                )
                .await
            }

            #[zbus(name = "GetFnLockSupported")]
            pub async fn get_fn_lock_supported(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_fn_lock_supported()
            }

            #[zbus(name = "GetFnLockStatus")]
            pub async fn get_fn_lock_status(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_fn_lock_status()
            }

            #[zbus(name = "SetFnLockStatus")]
            pub async fn set_fn_lock_status(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                status: bool,
            ) -> zbus::fdo::Result<()> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return Err(zbus::fdo::Error::AccessDenied(
                        "Polkit authorization denied".into(),
                    ));
                }
                self.offload_fdo_result(
                    OffloadLane::Misc,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    move |b| b.set_fn_lock_status(status),
                )
                .await
            }

            #[zbus(name = "SetSensorDataCollectionStatus")]
            pub async fn set_sensor_data_collection_status(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                status: bool,
            ) -> zbus::fdo::Result<()> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return Err(zbus::fdo::Error::AccessDenied(
                        "Polkit authorization denied".into(),
                    ));
                }
                self.offload_result(OffloadLane::Misc, move |b| {
                    b.set_sensor_data_collection_status(status)
                })
                .await?;
                Ok(())
            }

            #[zbus(name = "GetSensorDataCollectionStatus")]
            pub async fn get_sensor_data_collection_status(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_sensor_data_collection_status()
            }

            #[zbus(name = "SetDGpuD0Metrics")]
            pub async fn set_dgpu_d0_metrics(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                status: bool,
            ) -> zbus::fdo::Result<()> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return Err(zbus::fdo::Error::AccessDenied(
                        "Polkit authorization denied".into(),
                    ));
                }
                self.offload_result(OffloadLane::PowerGpu, move |b| {
                    b.set_dgpu_d0_metrics(status)
                })
                .await?;
                Ok(())
            }

            #[zbus(name = "GetNVIDIAPowerCTRLDefaultPowerLimit")]
            pub async fn get_nvidia_power_ctrl_default_power_limit(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return 0;
                }
                self.backend.get_nvidia_power_ctrl_default_power_limit()
            }

            #[zbus(name = "GetNVIDIAPowerCTRLMaxPowerLimit")]
            pub async fn get_nvidia_power_ctrl_max_power_limit(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return 0;
                }
                self.backend.get_nvidia_power_ctrl_max_power_limit()
            }

            #[zbus(name = "GetNVIDIAPowerCTRLAvailable")]
            pub async fn get_nvidia_power_ctrl_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_nvidia_power_ctrl_available()
            }

            #[zbus(name = "SetNVIDIAPowerOffset")]
            pub async fn set_nvidia_power_offset(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                offset: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::PowerGpu,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_nvidia_power_offset(offset),
                )
                .await
            }

            #[zbus(name = "GetAvailableGovernors")]
            pub async fn get_available_governors(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_available_governors()
            }

            #[zbus(name = "GetAvailableEPPs")]
            pub async fn get_available_epps(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_available_epps()
            }

            #[zbus(name = "GetCpuCoreCount")]
            pub async fn get_cpu_core_count(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return 0;
                }
                self.backend.get_cpu_core_count()
            }

            #[zbus(name = "GetCpuTopologyJSON")]
            pub async fn get_cpu_topology_json(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return "[]".to_string();
                }
                self.backend.get_cpu_topology_json()
            }

            #[zbus(name = "GetWaterCoolerAvailable")]
            pub async fn get_water_cooler_available(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_water_cooler_available()
            }

            #[zbus(name = "GetWaterCoolerConnected")]
            pub async fn get_water_cooler_connected(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_water_cooler_connected()
            }

            #[zbus(name = "GetWaterCoolerFanSpeed")]
            pub async fn get_water_cooler_fan_speed(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return -1;
                }
                self.backend.get_water_cooler_fan_speed()
            }

            #[zbus(name = "GetWaterCoolerPumpLevel")]
            pub async fn get_water_cooler_pump_level(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return -1;
                }
                self.backend.get_water_cooler_pump_level()
            }

            #[zbus(name = "EnableWaterCooler")]
            pub async fn enable_water_cooler(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                enable: bool,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.enable_water_cooler(enable),
                )
                .await
            }

            #[zbus(name = "IsWaterCoolerEnabled")]
            pub async fn is_water_cooler_enabled(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.is_water_cooler_enabled()
            }

            #[zbus(name = "SetWaterCoolerFanSpeed")]
            pub async fn set_water_cooler_fan_speed(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                duty_cycle_percent: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_water_cooler_fan_speed(duty_cycle_percent),
                )
                .await
            }

            #[zbus(name = "SetWaterCoolerPumpVoltage")]
            pub async fn set_water_cooler_pump_voltage(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                voltage: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_water_cooler_pump_voltage(voltage),
                )
                .await
            }

            #[zbus(name = "SetWaterCoolerLEDColor")]
            pub async fn set_water_cooler_led_color(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                red: i32,
                green: i32,
                blue: i32,
                mode_val: i32,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_water_cooler_led_color(red, green, blue, mode_val),
                )
                .await
            }

            #[zbus(name = "TurnOffWaterCoolerLED")]
            pub async fn turn_off_water_cooler_led(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    |b| b.turn_off_water_cooler_led(),
                )
                .await
            }

            #[zbus(name = "TurnOffWaterCoolerFan")]
            pub async fn turn_off_water_cooler_fan(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    |b| b.turn_off_water_cooler_fan(),
                )
                .await
            }

            #[zbus(name = "TurnOffWaterCoolerPump")]
            pub async fn turn_off_water_cooler_pump(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    |b| b.turn_off_water_cooler_pump(),
                )
                .await
            }

            #[zbus(name = "IsWaterCoolerAutoControlEnabled")]
            pub async fn is_water_cooler_auto_control_enabled(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.is_water_cooler_auto_control_enabled()
            }

            #[zbus(name = "SetWaterCoolerAutoControl")]
            pub async fn set_water_cooler_auto_control(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                enabled: bool,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return false;
                }
                self.offload_or_checked(
                    OffloadLane::Cooler,
                    conn,
                    header.sender().map(|s| s.as_str()),
                    false,
                    move |b| b.set_water_cooler_auto_control(enabled),
                )
                .await
            }

            #[zbus(name = "GetWaterCoolerSupported")]
            pub async fn get_water_cooler_supported(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_water_cooler_supported()
            }

            #[zbus(name = "GetCTGPAdjustmentSupported")]
            pub async fn get_ctgp_adjustment_supported(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_ctgp_adjustment_supported()
            }

            #[zbus(name = "GetHwpDynamicBoostSupported")]
            pub async fn get_hwp_dynamic_boost_supported(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return false;
                }
                self.backend.get_hwp_dynamic_boost_supported()
            }

            #[zbus(name = "GetMonitorDataSince")]
            pub async fn get_monitor_data_since(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                since_timestamp_ms: i64,
            ) -> Vec<u8> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return Vec::new();
                }
                self.backend.get_monitor_data_since(since_timestamp_ms)
            }

            #[zbus(name = "SetMonitorHistoryHorizon")]
            pub async fn set_monitor_history_horizon(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
                seconds: i32,
            ) -> zbus::fdo::Result<()> {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Control)
                    .await
                {
                    return Err(zbus::fdo::Error::AccessDenied(
                        "Polkit authorization denied".into(),
                    ));
                }
                self.offload_result(OffloadLane::Read, move |b| {
                    b.set_monitor_history_horizon(seconds)
                })
                .await?;
                Ok(())
            }

            #[zbus(name = "GetMonitorHistoryHorizon")]
            pub async fn get_monitor_history_horizon(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return 0;
                }
                self.backend.get_monitor_history_horizon()
            }

            #[zbus(name = "GetCpuFrequencyMHz")]
            pub async fn get_cpu_frequency_mhz(
                &self,
                #[zbus(header)] header: zbus::message::Header<'_>,
                #[zbus(connection)] conn: &zbus::Connection,
            ) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self
                    .authorizer
                    .check_auth(conn, sender, AuthAction::Read)
                    .await
                {
                    return 0;
                }
                self.backend.get_cpu_frequency_mhz()
            }
        }
    };
}

impl_uccd_methods!(UccdInterface, "com.uniwill.uccd");
impl_uccd_methods!(UccdLegacyInterface, "de.uniwill.uccd");
