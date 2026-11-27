// SPDX-License-Identifier: GPL-3.0-or-later

//! Daemon startup ordering (`Type=dbus` readiness), exclusive name ownership,
//! `NameOwnerChanged` client eviction, signal dispatching, periodic worker loops,
//! and bounded shutdown (`M8.1`, `M8.2`, `M8.3`).

use crate::backend::{
    run_water_cooler_ble_loop, CpuDefaultsSnapshot, DbusSignalEvent, LiveUccdBackend,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{error, warn};
use ucc_dbus::{
    register_uccd_service, Authorizer, UccdBackend, UccdInterface, UccdLegacyInterface, DBUS_PATH,
    DBUS_PATH_ALT,
};
use zbus::export::futures_core::Stream;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShutdownOutcome {
    Pending,
    Completed(Option<CpuDefaultsSnapshot>),
    Failed(String),
}

type ShutdownWaiter = (u64, watch::Receiver<ShutdownOutcome>);
type ShutdownRegistryMap = HashMap<String, Vec<ShutdownWaiter>>;

static NEXT_SHUTDOWN_ID: AtomicU64 = AtomicU64::new(1);
static IN_FLIGHT_SHUTDOWNS: OnceLock<Mutex<ShutdownRegistryMap>> = OnceLock::new();

fn shutdown_registry() -> &'static Mutex<ShutdownRegistryMap> {
    IN_FLIGHT_SHUTDOWNS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct ShutdownHandoffGuard {
    tx: watch::Sender<ShutdownOutcome>,
    settled: bool,
}

impl ShutdownHandoffGuard {
    fn complete(mut self, unrestored_cpu_defaults: Option<CpuDefaultsSnapshot>) {
        self.settled = true;
        let _ = self
            .tx
            .send(ShutdownOutcome::Completed(unrestored_cpu_defaults));
    }

    fn fail(mut self, reason: String) {
        self.settled = true;
        let _ = self.tx.send(ShutdownOutcome::Failed(reason));
    }
}

impl Drop for ShutdownHandoffGuard {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.tx.send(ShutdownOutcome::Failed(
                "predecessor shutdown thread terminated without acknowledgement".to_string(),
            ));
        }
    }
}

fn register_in_flight_shutdown(bus_name: &str) -> ShutdownHandoffGuard {
    let id = NEXT_SHUTDOWN_ID.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = watch::channel(ShutdownOutcome::Pending);
    let mut map = shutdown_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = map.entry(bus_name.to_string()).or_default();
    entry.retain(|(_, r)| !matches!(*r.borrow(), ShutdownOutcome::Completed(None)));
    entry.push((id, rx));
    ShutdownHandoffGuard { tx, settled: false }
}

async fn wait_for_predecessor_shutdown(
    bus_name: &str,
) -> Result<Option<CpuDefaultsSnapshot>, String> {
    let mut first_error: Option<String> = None;
    let mut inherited_cpu_defaults: Option<CpuDefaultsSnapshot> = None;
    loop {
        let pending: Vec<(u64, watch::Receiver<ShutdownOutcome>)> = {
            let mut map = shutdown_registry()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(entry) = map.get_mut(bus_name) else {
                return match first_error {
                    Some(err) => Err(err),
                    None => Ok(inherited_cpu_defaults),
                };
            };
            if entry.is_empty() {
                map.remove(bus_name);
                return match first_error {
                    Some(err) => Err(err),
                    None => Ok(inherited_cpu_defaults),
                };
            }
            entry.clone()
        };

        let mut finished_ids = Vec::with_capacity(pending.len());
        for (id, mut rx) in pending {
            let outcome = {
                let current = rx.borrow().clone();
                match current {
                    ShutdownOutcome::Pending => {
                        match rx
                            .wait_for(|state| !matches!(state, ShutdownOutcome::Pending))
                            .await
                        {
                            Ok(final_state) => final_state.clone(),
                            Err(_) => ShutdownOutcome::Failed(
                                "predecessor shutdown channel closed unexpectedly".to_string(),
                            ),
                        }
                    }
                    other => other,
                }
            };
            match outcome {
                ShutdownOutcome::Completed(unrestored) => {
                    if inherited_cpu_defaults.is_none() {
                        inherited_cpu_defaults = unrestored;
                    }
                }
                ShutdownOutcome::Failed(reason) => {
                    if first_error.is_none() {
                        first_error = Some(reason);
                    }
                }
                ShutdownOutcome::Pending => {}
            }
            finished_ids.push(id);
        }

        let mut map = shutdown_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get_mut(bus_name) {
            entry.retain(|(id, _)| !finished_ids.contains(id));
            if entry.is_empty() {
                map.remove(bus_name);
            }
        }
    }
}

fn format_panic_payload(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic payload".to_string())
}

async fn run_detached_blocking<F>(thread_name: &'static str, f: F) -> Result<(), String>
where
    F: FnOnce() + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .map_err(format_panic_payload);
            let _ = tx.send(outcome);
        })
        .map_err(|err| format!("Failed spawning {thread_name}: {err}"))?;
    match rx.await {
        Ok(res) => res,
        Err(_) => Err(format!("{thread_name} channel closed unexpectedly")),
    }
}

fn startup_prime_timeout() -> Duration {
    if let Ok(raw) = std::env::var("UCC_PRIME_TIMEOUT_MS") {
        if let Ok(ms) = raw.trim().parse::<u64>() {
            return Duration::from_millis(ms.max(10));
        }
    }
    Duration::from_millis(1500)
}

fn startup_activate_timeout() -> Duration {
    if let Ok(raw) = std::env::var("UCC_ACTIVATE_TIMEOUT_MS") {
        if let Ok(ms) = raw.trim().parse::<u64>() {
            return Duration::from_millis(ms.max(20));
        }
    }
    Duration::from_millis(10_000)
}

fn spawn_periodic_worker(
    thread_name: &'static str,
    interval: Duration,
    worker_stop: Arc<(Mutex<bool>, Condvar)>,
    backend: Arc<LiveUccdBackend>,
    mut step: impl FnMut(&LiveUccdBackend) + Send + 'static,
) {
    if let Err(err) = std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || loop {
            {
                let mut stopped = worker_stop.0.lock().unwrap_or_else(|e| e.into_inner());
                if *stopped {
                    break;
                }
                let (guard, _) = worker_stop
                    .1
                    .wait_timeout(stopped, interval)
                    .unwrap_or_else(|e| e.into_inner());
                stopped = guard;
                if *stopped {
                    break;
                }
            }
            let Some(_op_guard) = backend.enter_worker_hw_op() else {
                break;
            };
            if let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step(&backend)))
            {
                error!(
                    "Periodic worker '{thread_name}' panicked: {}",
                    format_panic_payload(payload)
                );
            }
        })
    {
        warn!("Failed to spawn periodic worker '{thread_name}': {err}");
    }
}

/// Coordinates active background workers and D-Bus lifetime for `uccd`.
pub struct DaemonRuntime {
    connection: zbus::Connection,
    bus_name: String,
    backend: Arc<LiveUccdBackend>,
    stop_tx: watch::Sender<bool>,
    worker_stop: Arc<(Mutex<bool>, Condvar)>,
    tasks: Vec<JoinHandle<()>>,
}

async fn unregister_uccd_paths(connection: &zbus::Connection) {
    let server = connection.object_server();
    for path in [DBUS_PATH, DBUS_PATH_ALT] {
        let _ = server
            .remove::<UccdInterface<LiveUccdBackend>, _>(path)
            .await;
        let _ = server
            .remove::<UccdLegacyInterface<LiveUccdBackend>, _>(path)
            .await;
    }
}

async fn dispatch_signal_event(connection: &zbus::Connection, event: &DbusSignalEvent) {
    let server = connection.object_server();
    for path in [DBUS_PATH, DBUS_PATH_ALT] {
        if let Ok(iface) = server
            .interface::<_, UccdInterface<LiveUccdBackend>>(path)
            .await
        {
            let emitter = iface.signal_emitter();
            match event {
                DbusSignalEvent::ProfileChanged {
                    profile_id,
                    keyboard_profile_id,
                    fan_profile_id,
                } => {
                    let _ = UccdInterface::<LiveUccdBackend>::profile_changed(
                        emitter,
                        profile_id,
                        keyboard_profile_id,
                        fan_profile_id,
                    )
                    .await;
                }
                DbusSignalEvent::ModeReapplyPendingChanged { pending } => {
                    let _ = UccdInterface::<LiveUccdBackend>::mode_reapply_pending_changed(
                        emitter, *pending,
                    )
                    .await;
                }
                DbusSignalEvent::PowerStateChanged { state } => {
                    let _ =
                        UccdInterface::<LiveUccdBackend>::power_state_changed(emitter, state).await;
                }
                DbusSignalEvent::PowerPolicyChanged { json } => {
                    let _ =
                        UccdInterface::<LiveUccdBackend>::power_policy_changed(emitter, json).await;
                }
                DbusSignalEvent::PowerDeviceStatusChanged { json } => {
                    let _ = UccdInterface::<LiveUccdBackend>::power_device_status_changed(
                        emitter, json,
                    )
                    .await;
                }
                DbusSignalEvent::LightbarSettingsChanged { json } => {
                    let _ =
                        UccdInterface::<LiveUccdBackend>::lightbar_settings_changed(emitter, json)
                            .await;
                }
                DbusSignalEvent::WaterCoolerStatusChanged { status } => {
                    let _ = UccdInterface::<LiveUccdBackend>::water_cooler_status_changed(
                        emitter, status,
                    )
                    .await;
                }
            }
        }

        if let Ok(iface) = server
            .interface::<_, UccdLegacyInterface<LiveUccdBackend>>(path)
            .await
        {
            let emitter = iface.signal_emitter();
            match event {
                DbusSignalEvent::ProfileChanged {
                    profile_id,
                    keyboard_profile_id,
                    fan_profile_id,
                } => {
                    let _ = UccdLegacyInterface::<LiveUccdBackend>::profile_changed(
                        emitter,
                        profile_id,
                        keyboard_profile_id,
                        fan_profile_id,
                    )
                    .await;
                }
                DbusSignalEvent::ModeReapplyPendingChanged { pending } => {
                    let _ = UccdLegacyInterface::<LiveUccdBackend>::mode_reapply_pending_changed(
                        emitter, *pending,
                    )
                    .await;
                }
                DbusSignalEvent::PowerStateChanged { state } => {
                    let _ =
                        UccdLegacyInterface::<LiveUccdBackend>::power_state_changed(emitter, state)
                            .await;
                }
                DbusSignalEvent::PowerPolicyChanged { json } => {
                    let _ =
                        UccdLegacyInterface::<LiveUccdBackend>::power_policy_changed(emitter, json)
                            .await;
                }
                DbusSignalEvent::PowerDeviceStatusChanged { json } => {
                    let _ = UccdLegacyInterface::<LiveUccdBackend>::power_device_status_changed(
                        emitter, json,
                    )
                    .await;
                }
                DbusSignalEvent::LightbarSettingsChanged { json } => {
                    let _ = UccdLegacyInterface::<LiveUccdBackend>::lightbar_settings_changed(
                        emitter, json,
                    )
                    .await;
                }
                DbusSignalEvent::WaterCoolerStatusChanged { status } => {
                    let _ = UccdLegacyInterface::<LiveUccdBackend>::water_cooler_status_changed(
                        emitter, status,
                    )
                    .await;
                }
            }
        }
    }
}

impl DaemonRuntime {
    /// Starts `LiveUccdBackend` on `connection` obeying `Type=dbus` readiness order (`M8.2`):
    ///
    /// 1. Verify public `bus_name` is unowned (`M8-PROD-01`, no synthetic lock name required).
    /// 2. Prime telemetry/profile/fan snapshots strictly read-only (`prime_read_only`), performing
    ///    ZERO hardware/sysfs/config writes (`M8-PROD-02`).
    /// 3. Export `/com/uniwill/uccd` and `/de/uniwill/uccd` D-Bus objects.
    /// 4. Acquire `bus_name` exclusively (`DoNotQueue`); if lost or denied, unregister and abort
    ///    with ZERO hardware writes.
    /// 5. Await any timed-out predecessor shutdown on `bus_name` (`M8-PROD-03`).
    /// 6. Activate hardware (`activate_hardware`) once exclusive ownership is confirmed.
    /// 7. Spawn signal dispatcher, `NameOwnerChanged` watcher, and optional periodic worker loops.
    pub async fn start_on_connection(
        connection: zbus::Connection,
        bus_name: impl Into<String>,
        backend: Arc<LiveUccdBackend>,
        authorizer: Arc<dyn Authorizer>,
        spawn_workers: bool,
    ) -> zbus::Result<Self> {
        Self::start_on_connection_cancellable(
            connection,
            bus_name,
            backend,
            authorizer,
            spawn_workers,
            std::future::pending::<()>(),
        )
        .await?
        .ok_or_else(|| zbus::Error::Failure("Daemon startup cancelled".to_string()))
    }

    /// Starts `LiveUccdBackend` on `connection`, racing all slow startup stages against `shutdown_fut`
    /// so an early `SIGINT`/`SIGTERM` aborts initialization and still runs bounded hardware cleanup (`TA-03`).
    pub async fn start_on_connection_cancellable<F>(
        connection: zbus::Connection,
        bus_name: impl Into<String>,
        backend: Arc<LiveUccdBackend>,
        authorizer: Arc<dyn Authorizer>,
        spawn_workers: bool,
        shutdown_fut: F,
    ) -> zbus::Result<Option<Self>>
    where
        F: std::future::Future<Output = ()> + Send,
    {
        let bus_name = bus_name.into();
        let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
        let mut shutdown_fut = std::pin::pin!(shutdown_fut);

        // Reject early with ZERO hardware writes if another owner already holds the public name.
        if let Ok(dbus_proxy) = zbus::fdo::DBusProxy::new(&connection).await {
            if let Ok(wb) = zbus::names::WellKnownName::try_from(bus_name.as_str()) {
                if dbus_proxy.name_has_owner(wb.into()).await.unwrap_or(false) {
                    return Err(zbus::Error::Failure(format!(
                        "Refusing to start: bus name '{bus_name}' already owned"
                    )));
                }
            }
        }

        // Mark activation pending BEFORE exporting D-Bus endpoints so premature write RPCs
        // cannot be acknowledged or overwritten during predecessor handoff/activation (`M8-HANDOFF-01`).
        backend.set_activation_pending(true);

        let abort_startup = |conn: zbus::Connection,
                             name: String,
                             backend: Arc<LiveUccdBackend>,
                             release_name: bool| async move {
            backend.begin_shutdown();
            backend.set_activation_pending(false);
            let handoff_guard = register_in_flight_shutdown(&name);
            if release_name {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    unregister_uccd_paths(&conn),
                )
                .await;
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    conn.release_name(name.as_str()),
                )
                .await;
            }
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            let backend_timeout = Arc::clone(&backend);
            let spawn_err_tx = handoff_guard.tx.clone();
            match std::thread::Builder::new()
                .name("uccd-startup-abort".to_string())
                .spawn(move || {
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        backend.wait_in_flight_hw_ops();
                        backend.shutdown();
                        backend.take_unrestored_cpu_defaults()
                    }));
                    match res {
                        Ok(unrestored_cpu) => handoff_guard.complete(unrestored_cpu),
                        Err(payload) => {
                            let msg = format_panic_payload(payload);
                            error!("Startup-abort hardware shutdown panicked: {msg}");
                            handoff_guard.fail(format!("startup-abort cleanup panicked: {msg}"));
                        }
                    }
                    let _ = done_tx.send(());
                }) {
                Ok(_) => {
                    if tokio::time::timeout(Duration::from_secs(5), done_rx)
                        .await
                        .is_err()
                    {
                        backend_timeout.mark_shutdown_wait_expired();
                        error!(
                            "Timed out waiting for hardware cleanup after startup signal; continuing exit"
                        );
                    }
                }
                Err(err) => {
                    let _ = spawn_err_tx.send(ShutdownOutcome::Failed(format!(
                        "failed to spawn startup-abort thread: {err}"
                    )));
                }
            }
        };

        // Perform strictly read-only sleep snapshot load and bounded priming BEFORE exporting
        // objects or requesting bus_name (`FULL-27-01`, `DEEP-03`).
        let mut sig_rx = backend.subscribe_signals();
        let prime_timeout = startup_prime_timeout();
        let sleep_load_backend = Arc::clone(&backend);
        tokio::select! {
            biased;
            _ = &mut shutdown_fut => {
                abort_startup(connection, bus_name, backend, false).await;
                return Ok(None);
            }
            res = tokio::time::timeout(
                prime_timeout,
                run_detached_blocking("uccd-sleep-load", move || {
                    let _ = sleep_load_backend.load_sleep_state_read_only();
                }),
            ) => {
                if let Err(err) = res {
                    warn!("Timed out loading sleep snapshot after {prime_timeout:?}: {err}");
                }
            }
        }

        let prime_backend = Arc::clone(&backend);
        tokio::select! {
            biased;
            _ = &mut shutdown_fut => {
                abort_startup(connection, bus_name, backend, false).await;
                return Ok(None);
            }
            prime_res = tokio::time::timeout(
                prime_timeout,
                run_detached_blocking("uccd-prime", move || {
                    prime_backend.prime_read_only();
                }),
            ) => {
                match prime_res {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        backend.mark_prime_degraded(true);
                        warn!("Daemon read-only priming failed ({err}); continuing in degraded startup");
                    }
                    Err(_) => {
                        backend.mark_prime_degraded(true);
                        warn!(
                            "Startup priming exceeded {:?}; publishing D-Bus service in degraded mode",
                            prime_timeout
                        );
                    }
                }
            }
        }

        if let Err(err) = register_uccd_service(&connection, Arc::clone(&backend), authorizer).await
        {
            backend.set_activation_pending(false);
            return Err(err);
        }

        // Re-check right before acquiring public name to avoid any race while priming.
        if let Ok(dbus_proxy) = zbus::fdo::DBusProxy::new(&connection).await {
            if let Ok(wb) = zbus::names::WellKnownName::try_from(bus_name.as_str()) {
                if dbus_proxy.name_has_owner(wb.into()).await.unwrap_or(false) {
                    unregister_uccd_paths(&connection).await;
                    backend.set_activation_pending(false);
                    return Err(zbus::Error::Failure(format!(
                        "Refusing to start: bus name '{bus_name}' acquired by another owner during startup"
                    )));
                }
            }
        }

        tokio::select! {
            biased;
            _ = &mut shutdown_fut => {
                unregister_uccd_paths(&connection).await;
                abort_startup(connection, bus_name, backend, false).await;
                return Ok(None);
            }
            req_res = connection.request_name_with_flags(bus_name.as_str(), flags) => {
                match req_res {
                    Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {}
                    Ok(reply) => {
                        unregister_uccd_paths(&connection).await;
                        backend.set_activation_pending(false);
                        return Err(zbus::Error::Failure(format!(
                            "Failed to acquire exclusive D-Bus name '{bus_name}': {reply:?}"
                        )));
                    }
                    Err(err) => {
                        unregister_uccd_paths(&connection).await;
                        backend.set_activation_pending(false);
                        return Err(err);
                    }
                }
            }
        }

        // Wait until EVERY timed-out predecessor cleanup + in-flight hardware op on this bus_name
        // settles before touching hardware (`M8-HANDOFF-02`, `03`, `M8-LIFE-02`).
        let inherited_cpu_defaults = tokio::select! {
            biased;
            _ = &mut shutdown_fut => {
                abort_startup(connection, bus_name, backend, true).await;
                return Ok(None);
            }
            pred_res = wait_for_predecessor_shutdown(&bus_name) => {
                match pred_res {
                    Ok(defaults) => defaults,
                    Err(reason) => {
                        unregister_uccd_paths(&connection).await;
                        let _ = connection.release_name(bus_name.as_str()).await;
                        backend.set_activation_pending(false);
                        return Err(zbus::Error::Failure(format!(
                            "Refusing to start on '{bus_name}': predecessor cleanup failed ({reason})"
                        )));
                    }
                }
            }
        };

        let activate_timeout = startup_activate_timeout();
        let activate_backend = Arc::clone(&backend);
        tokio::select! {
            biased;
            _ = &mut shutdown_fut => {
                abort_startup(connection, bus_name, backend, true).await;
                return Ok(None);
            }
            act_res = tokio::time::timeout(
                activate_timeout,
                run_detached_blocking("uccd-activate", move || {
                    let _ = activate_backend.load_sleep_state_read_only();
                    activate_backend.seed_inherited_cpu_defaults(inherited_cpu_defaults);
                    activate_backend.activate_hardware();
                    activate_backend.clear_consumed_sleep_state();
                }),
            ) => {
                match act_res {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        unregister_uccd_paths(&connection).await;
                        let _ = connection.release_name(bus_name.as_str()).await;
                        backend.set_activation_pending(false);
                        return Err(zbus::Error::Failure(format!(
                            "Daemon hardware activation failed: {err}"
                        )));
                    }
                    Err(_) => {
                        backend.mark_startup_degraded(true);
                        if !backend.is_shutting_down() {
                            backend.set_activation_pending(false);
                        }
                        warn!(
                            "Startup hardware activation exceeded {:?}; continuing in degraded startupState",
                            activate_timeout
                        );
                    }
                }
            }
        }

        let (stop_tx, stop_rx) = watch::channel(false);
        let worker_stop = Arc::new((Mutex::new(false), Condvar::new()));
        let mut tasks = Vec::new();

        // Always dispatch D-Bus signals emitted by backend methods (even when periodic workers are off).
        let sig_conn = connection.clone();
        let mut rx_sig_stop = stop_rx.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = rx_sig_stop.changed() => break,
                    recv = sig_rx.recv() => match recv {
                        Ok(event) => dispatch_signal_event(&sig_conn, &event).await,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("D-Bus signal dispatcher lagged behind by {n} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }));

        // Always watch NameOwnerChanged to evict disconnected clients' power requests immediately
        // and offload any resulting hardware reconciliation away from Tokio (`M8-RECHECK-10`, `M8-LIFE-01`).
        if let Ok(dbus_proxy) = zbus::fdo::DBusProxy::new(&connection).await {
            if let Ok(noc_stream) = dbus_proxy.receive_name_owner_changed().await {
                let b_noc = Arc::clone(&backend);
                let mut rx_noc_stop = stop_rx.clone();
                tasks.push(tokio::spawn(async move {
                    let mut noc_stream = std::pin::pin!(noc_stream);
                    loop {
                        tokio::select! {
                            biased;
                            _ = rx_noc_stop.changed() => break,
                            maybe_msg = std::future::poll_fn(|cx| noc_stream.as_mut().poll_next(cx)) => {
                                let Some(msg) = maybe_msg else { break; };
                                if let Ok(args) = msg.args() {
                                    let name = args.name.as_str();
                                    let old_owner = args.old_owner.as_ref().map(|s| s.as_str()).unwrap_or("");
                                    let new_owner = args.new_owner.as_ref().map(|s| s.as_str()).unwrap_or("");
                                    if name.starts_with(':')
                                        && name == old_owner
                                        && new_owner.is_empty()
                                        && b_noc.record_dbus_client_disconnected(old_owner)
                                    {
                                        let b_reconcile = Arc::clone(&b_noc);
                                        let _ = std::thread::Builder::new()
                                            .name("uccd-disconnect".to_string())
                                            .spawn(move || {
                                                if let Some(_g) = b_reconcile.enter_worker_hw_op() {
                                                    b_reconcile.reconcile_power_state(false);
                                                }
                                            });
                                    }
                                }
                            }
                        }
                    }
                }));
            }
        }

        if spawn_workers {
            spawn_periodic_worker(
                "uccd-worker-tel",
                Duration::from_millis(800),
                Arc::clone(&worker_stop),
                Arc::clone(&backend),
                |b| {
                    b.poll_telemetry_once();
                },
            );
            spawn_periodic_worker(
                "uccd-worker-fan",
                Duration::from_secs(1),
                Arc::clone(&worker_stop),
                Arc::clone(&backend),
                |b| {
                    b.tick_fans_once();
                },
            );
            spawn_periodic_worker(
                "uccd-worker-pow",
                Duration::from_millis(200),
                Arc::clone(&worker_stop),
                Arc::clone(&backend),
                move |b| {
                    b.reconcile_power_state(false);
                },
            );
            spawn_periodic_worker(
                "uccd-worker-cpu",
                Duration::from_secs(10),
                Arc::clone(&worker_stop),
                Arc::clone(&backend),
                move |b| {
                    b.validate_and_reapply_cpu_once();
                },
            );
            let mut anim_started: Option<Instant> = None;
            spawn_periodic_worker(
                "uccd-worker-lightbar",
                ucc_lighting::LIGHTBAR_FRAME_INTERVAL,
                Arc::clone(&worker_stop),
                Arc::clone(&backend),
                move |b| {
                    b.tick_lightbar_animation(&mut anim_started);
                },
            );

            if backend.get_water_cooler_supported() {
                let ble_conn = connection.clone();
                let ble_backend = Arc::clone(&backend);
                let ble_stop = stop_rx.clone();
                tasks.push(tokio::spawn(async move {
                    run_water_cooler_ble_loop(ble_conn, ble_backend, ble_stop).await;
                }));
            }

            let logind_conn = connection.clone();
            let logind_backend = Arc::clone(&backend);
            let mut logind_stop = stop_rx;
            tasks.push(tokio::spawn(async move {
                run_logind_monitor_loop(logind_conn, logind_backend, &mut logind_stop).await;
            }));
        }

        Ok(Some(Self {
            connection,
            bus_name,
            backend,
            stop_tx,
            worker_stop,
            tasks,
        }))
    }

    pub fn backend(&self) -> &Arc<LiveUccdBackend> {
        &self.backend
    }

    pub fn connection(&self) -> &zbus::Connection {
        &self.connection
    }

    /// Signals background workers to abort, immediately unexports D-Bus endpoints and releases
    /// `bus_name`, and runs hardware teardown on a detached OS thread strictly within `timeout`
    /// so even `tokio::runtime::Runtime` drop is bounded (`M8-RECHECK-02`, `M8-LIFE-01`, `02`).
    pub async fn shutdown(self, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        self.backend.begin_shutdown();
        {
            let mut stopped = self.worker_stop.0.lock().unwrap_or_else(|e| e.into_inner());
            *stopped = true;
            self.worker_stop.1.notify_all();
        }
        let _ = self.stop_tx.send(true);

        let mut handles = self.tasks;
        for handle in &handles {
            handle.abort();
        }
        let abort_deadline = (tokio::time::Instant::now() + Duration::from_millis(5)).min(deadline);
        let _ = tokio::time::timeout_at(abort_deadline, async {
            for handle in &mut handles {
                let _ = handle.await;
            }
        })
        .await;

        // Register predecessor handoff token BEFORE unexporting or releasing bus_name so a
        // replacement racing to acquire bus_name immediately sees the in-flight cleanup (`M8-HANDOFF-02`).
        let handoff_guard = register_in_flight_shutdown(&self.bus_name);

        // Unexport objects and release the public well-known name BEFORE slow hardware cleanup
        // so clients/shared connections observe clean D-Bus teardown even under tight timeouts.
        let _ = tokio::time::timeout_at(deadline, unregister_uccd_paths(&self.connection)).await;
        if tokio::time::Instant::now() < deadline {
            let _ = tokio::time::timeout_at(
                deadline,
                self.connection.release_name(self.bus_name.as_str()),
            )
            .await;
        }

        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let backend_timeout = Arc::clone(&self.backend);
        let backend = self.backend;
        let spawn_err_tx = handoff_guard.tx.clone();
        match std::thread::Builder::new()
            .name("uccd-shutdown".to_string())
            .spawn(move || {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.wait_in_flight_hw_ops();
                    backend.shutdown();
                    backend.take_unrestored_cpu_defaults()
                }));
                match res {
                    Ok(unrestored_cpu) => handoff_guard.complete(unrestored_cpu),
                    Err(payload) => {
                        let msg = format_panic_payload(payload);
                        error!("Hardware shutdown thread panicked: {msg}");
                        handoff_guard.fail(format!("predecessor cleanup panicked: {msg}"));
                    }
                }
                let _ = done_tx.send(());
            }) {
            Ok(_) => {
                if tokio::time::timeout_at(deadline, done_rx).await.is_err() {
                    backend_timeout.mark_shutdown_wait_expired();
                    error!(
                        "Timed out after {:?} waiting for hardware shutdown/fan-auto restore; continuing exit",
                        timeout
                    );
                }
            }
            Err(err) => {
                let _ = spawn_err_tx.send(ShutdownOutcome::Failed(format!(
                    "failed to spawn shutdown thread: {err}"
                )));
                warn!("Failed to spawn detached shutdown thread ({err}); skipping blocking hardware teardown past deadline");
            }
        }
    }
}

async fn next_signal_opt(
    stream: Option<&mut std::pin::Pin<Box<zbus::proxy::SignalStream<'_>>>>,
) -> Option<zbus::Message> {
    let Some(s) = stream else {
        return std::future::pending().await;
    };
    std::future::poll_fn(|cx| s.as_mut().poll_next(cx)).await
}

fn extract_lid_closed_change(
    msg: &zbus::Message,
    expected_iface: &str,
    expected_prop: &str,
) -> Option<bool> {
    let (iface, changed, _invalidated): (
        String,
        HashMap<String, zbus::zvariant::OwnedValue>,
        Vec<String>,
    ) = msg.body().deserialize().ok()?;
    if iface != expected_iface {
        return None;
    }
    let val = changed.get(expected_prop)?;
    bool::try_from(val).ok()
}

/// Best-effort systemd-logind & UPower monitor handling `PrepareForSleep` resume notifications,
/// `handle-lid-switch` inhibitor coordination (`ISSUE-03`), and lid keyboard/cooler lighting (`M8.10`).
async fn run_logind_monitor_loop(
    connection: zbus::Connection,
    backend: Arc<LiveUccdBackend>,
    stop_rx: &mut watch::Receiver<bool>,
) {
    let Ok(login_proxy) = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    else {
        return;
    };

    if let Ok(closed) = login_proxy.get_property::<bool>("LidClosed").await {
        backend.seed_initial_lid_state(closed);
    }

    let mut inhibitor_fd: Option<zbus::zvariant::OwnedFd> = None;
    let pd_settings: serde_json::Value =
        serde_json::from_str(&backend.get_power_device_settings_json()).unwrap_or_default();
    if pd_settings.get("lid").is_some() {
        if let Ok(fd) = login_proxy
            .call::<_, _, zbus::zvariant::OwnedFd>(
                "Inhibit",
                &(
                    "handle-lid-switch",
                    "uccd",
                    "Managing configured lid-switch power policy",
                    "block",
                ),
            )
            .await
        {
            inhibitor_fd = Some(fd);
        }
    }

    let Ok(sleep_stream) = login_proxy.receive_signal("PrepareForSleep").await else {
        let _ = stop_rx.changed().await;
        drop(inhibitor_fd);
        return;
    };
    let mut sleep_stream = std::pin::pin!(sleep_stream);

    let mut login_props_stream = match zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.DBus.Properties",
    )
    .await
    {
        Ok(p) => p
            .receive_signal("PropertiesChanged")
            .await
            .ok()
            .map(Box::pin),
        Err(_) => None,
    };

    let mut upower_props_stream = match zbus::Proxy::new(
        &connection,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower",
        "org.freedesktop.DBus.Properties",
    )
    .await
    {
        Ok(p) => p
            .receive_signal("PropertiesChanged")
            .await
            .ok()
            .map(Box::pin),
        Err(_) => None,
    };

    loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            maybe_sig = std::future::poll_fn(|cx| sleep_stream.as_mut().poll_next(cx)) => {
                let Some(msg) = maybe_sig else { break; };
                if let Ok((sleeping,)) = msg.body().deserialize::<(bool,)>() {
                    let b = Arc::clone(&backend);
                    if sleeping {
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Some(_g) = b.enter_worker_hw_op() {
                                b.on_system_suspend();
                            }
                        }).await;
                    } else {
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Some(_g) = b.enter_worker_hw_op() {
                                b.on_system_resume();
                                b.reconcile_power_state(true);
                            }
                        }).await;
                    }
                }
            }
            maybe_msg = next_signal_opt(login_props_stream.as_mut()) => {
                let Some(msg) = maybe_msg else {
                    login_props_stream = None;
                    continue;
                };
                if let Some(closed) = extract_lid_closed_change(&msg, "org.freedesktop.login1.Manager", "LidClosed") {
                    let b = Arc::clone(&backend);
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Some(_g) = b.enter_worker_hw_op() {
                            b.on_lid_state_changed(closed);
                        }
                    }).await;
                }
            }
            maybe_msg = next_signal_opt(upower_props_stream.as_mut()) => {
                let Some(msg) = maybe_msg else {
                    upower_props_stream = None;
                    continue;
                };
                if let Some(closed) = extract_lid_closed_change(&msg, "org.freedesktop.UPower", "LidIsClosed") {
                    let b = Arc::clone(&backend);
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Some(_g) = b.enter_worker_hw_op() {
                            b.on_lid_state_changed(closed);
                        }
                    }).await;
                }
            }
        }
    }
    drop(inhibitor_fd);
}
