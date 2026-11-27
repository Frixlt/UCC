//! Production async BlueZ D-Bus GATT client for LCT21001/LCT22002 water coolers (`W1-10`).
//!
//! Uses `org.freedesktop.DBus.ObjectManager`, `org.bluez.Device1`, and `org.bluez.GattCharacteristic1`
//! over `zbus` (configurable bus name so both production `"org.bluez"` and isolated session-bus
//! fake BlueZ test stands run through the exact same client entry points).

use crate::cooler::{
    CoolerController, CoolerError, CoolerPacketEncoder, NORDIC_UART_CHAR_RX, NORDIC_UART_CHAR_TX,
    NORDIC_UART_SERVICE_UUID,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

const DEFAULT_GATT_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const GATT_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Supported LCT liquid cooler hardware revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LctDeviceModel {
    Lct21001,
    Lct22002,
}

/// Maps a Bluetooth device advertised name to a supported [`LctDeviceModel`].
pub fn detect_lct_model(name: &str) -> Option<LctDeviceModel> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("lct22002") {
        Some(LctDeviceModel::Lct22002)
    } else if lower.contains("lct21001")
        || lower.contains("coolingsystem")
        || lower.contains("aquaris")
    {
        Some(LctDeviceModel::Lct21001)
    } else {
        None
    }
}

/// Persisted water cooler pairing identity (MAC address and/or advertised name).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedCoolerIdentity {
    pub address: Option<String>,
    pub name: Option<String>,
}

/// Discovered BlueZ device exposing LCT / Nordic UART identity and optional GATT characteristics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredCooler {
    pub device_path: String,
    pub address: String,
    pub name: String,
    pub model: LctDeviceModel,
    pub tx_char_path: Option<String>,
    pub rx_char_path: Option<String>,
    pub connected: bool,
}

/// Selects the trusted LCT cooler from discovered BlueZ devices (`M5.5`).
///
/// Matches exact MAC address first, then falls back to unique advertised name (handling rotating
/// random BLE addresses without allowing impersonation when multiple candidates share a name).
pub fn select_cooler<'a>(
    devices: &'a [DiscoveredCooler],
    saved: Option<&SavedCoolerIdentity>,
) -> Option<&'a DiscoveredCooler> {
    if devices.is_empty() {
        return None;
    }

    if let Some(id) = saved {
        if let Some(ref addr) = id.address {
            if let Some(found) = devices
                .iter()
                .find(|d| d.address.eq_ignore_ascii_case(addr))
            {
                return Some(found);
            }
        }
        if let Some(ref saved_name) = id.name {
            let name_matches: Vec<&DiscoveredCooler> = devices
                .iter()
                .filter(|d| d.name.eq_ignore_ascii_case(saved_name))
                .collect();
            if name_matches.len() == 1 {
                return Some(name_matches[0]);
            }
            return None;
        }
    }

    if devices.len() == 1 {
        Some(&devices[0])
    } else {
        None
    }
}

type ManagedObjectsMap = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// Production async BlueZ D-Bus GATT client (`org.bluez` by default).
#[derive(Clone)]
pub struct BluezCoolerClient {
    conn: Connection,
    bus_name: String,
}

impl BluezCoolerClient {
    /// Creates a client targeting `"org.bluez"` on the provided system D-Bus connection.
    pub fn new_system(conn: Connection) -> Self {
        Self::with_bus_name(conn, "org.bluez")
    }

    /// Creates a client targeting a custom D-Bus service name (for isolated fake BlueZ stands).
    pub fn with_bus_name(conn: Connection, bus_name: impl Into<String>) -> Self {
        Self {
            conn,
            bus_name: bus_name.into(),
        }
    }

    async fn get_managed_objects(&self) -> Result<ManagedObjectsMap, CoolerError> {
        let proxy = Proxy::new(
            &self.conn,
            self.bus_name.as_str(),
            "/",
            "org.freedesktop.DBus.ObjectManager",
        )
        .await
        .map_err(|_| CoolerError::NotConnected)?;

        proxy
            .call("GetManagedObjects", &())
            .await
            .map_err(|_| CoolerError::NotConnected)
    }

    fn extract_gatt_chars(objects: &ManagedObjectsMap) -> Vec<(String, String)> {
        let mut chars = Vec::new();
        for (path, ifaces) in objects {
            if let Some(ch_props) = ifaces.get("org.bluez.GattCharacteristic1") {
                if let Some(uuid) = ch_props
                    .get("UUID")
                    .and_then(|v| String::try_from(v.clone()).ok())
                {
                    chars.push((path.as_str().to_string(), uuid.to_ascii_lowercase()));
                }
            }
        }
        chars
    }

    async fn resolve_gatt_for_device(
        &self,
        device_path: &str,
    ) -> Result<(Option<String>, Option<String>), CoolerError> {
        let objects = self.get_managed_objects().await?;
        let chars = Self::extract_gatt_chars(&objects);
        let dev_prefix = format!("{device_path}/");
        let tx = chars
            .iter()
            .find(|(cp, uuid)| cp.starts_with(&dev_prefix) && uuid == NORDIC_UART_CHAR_TX)
            .map(|(cp, _)| cp.clone());
        let rx = chars
            .iter()
            .find(|(cp, uuid)| cp.starts_with(&dev_prefix) && uuid == NORDIC_UART_CHAR_RX)
            .map(|(cp, _)| cp.clone());
        Ok((tx, rx))
    }

    /// Enumerates BlueZ managed objects and returns discovered LCT water coolers.
    ///
    /// Handles missing/late adapters gracefully (returning an empty list rather than crashing).
    pub async fn discover_coolers(&self) -> Result<Vec<DiscoveredCooler>, CoolerError> {
        let objects = self.get_managed_objects().await?;
        let chars_by_prefix = Self::extract_gatt_chars(&objects);

        let mut coolers = Vec::new();
        for (path, ifaces) in &objects {
            let Some(dev_props) = ifaces.get("org.bluez.Device1") else {
                continue;
            };
            let name = dev_props
                .get("Name")
                .and_then(|v| String::try_from(v.clone()).ok())
                .unwrap_or_default();
            let uuids = dev_props
                .get("UUIDs")
                .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
                .unwrap_or_default();
            let _has_nus = uuids
                .iter()
                .any(|u| u.eq_ignore_ascii_case(NORDIC_UART_SERVICE_UUID));

            // Strict name match prevents auto-connecting to non-cooler Nordic UART BLE peripherals
            // (e.g. watches, microcontrollers) per ISSUE-36.
            let Some(model) = detect_lct_model(&name) else {
                continue;
            };

            let address = dev_props
                .get("Address")
                .and_then(|v| String::try_from(v.clone()).ok())
                .unwrap_or_default();
            let connected = dev_props
                .get("Connected")
                .and_then(|v| bool::try_from(v.clone()).ok())
                .unwrap_or(false);

            let dev_prefix = format!("{}/", path.as_str());
            let tx_char_path = chars_by_prefix
                .iter()
                .find(|(cp, uuid)| cp.starts_with(&dev_prefix) && uuid == NORDIC_UART_CHAR_TX)
                .map(|(cp, _)| cp.clone());
            let rx_char_path = chars_by_prefix
                .iter()
                .find(|(cp, uuid)| cp.starts_with(&dev_prefix) && uuid == NORDIC_UART_CHAR_RX)
                .map(|(cp, _)| cp.clone());

            coolers.push(DiscoveredCooler {
                device_path: path.as_str().to_string(),
                address,
                name,
                model,
                tx_char_path,
                rx_char_path,
                connected,
            });
        }

        coolers.sort_by(|a, b| a.device_path.cmp(&b.device_path));
        Ok(coolers)
    }

    /// Connects `org.bluez.Device1` FIRST, then resolves Nordic UART TX/RX GATT characteristics
    /// up to [`DEFAULT_GATT_RESOLVE_TIMEOUT`] when BlueZ exposes GATT services post-connect (`W1-10`).
    pub async fn connect_cooler(
        &self,
        cooler: &DiscoveredCooler,
    ) -> Result<BluezGattSession, CoolerError> {
        self.connect_cooler_with_timeout(cooler, DEFAULT_GATT_RESOLVE_TIMEOUT)
            .await
    }

    /// Connects `org.bluez.Device1`, waits up to `gatt_timeout` for NUS TX/RX resolution,
    /// enables RX notifications, and returns an active [`BluezGattSession`].
    pub async fn connect_cooler_with_timeout(
        &self,
        cooler: &DiscoveredCooler,
        gatt_timeout: Duration,
    ) -> Result<BluezGattSession, CoolerError> {
        let deadline = tokio::time::Instant::now() + gatt_timeout;
        let dev_proxy = zbus::proxy::Builder::<Proxy>::new(&self.conn)
            .destination(self.bus_name.clone())
            .map_err(|_| CoolerError::NotConnected)?
            .path(cooler.device_path.clone())
            .map_err(|_| CoolerError::NotConnected)?
            .interface("org.bluez.Device1")
            .map_err(|_| CoolerError::NotConnected)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|_| CoolerError::NotConnected)?;

        let remaining_connect = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining_connect.is_zero() {
            return Err(CoolerError::NotConnected);
        }
        match tokio::time::timeout(remaining_connect, dev_proxy.call_method("Connect", &())).await {
            Ok(Ok(_)) => {}
            _ => return Err(CoolerError::NotConnected),
        }

        let (mut tx_opt, mut rx_opt) = (cooler.tx_char_path.clone(), cooler.rx_char_path.clone());
        if tx_opt.is_none() {
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(
                    remaining,
                    self.resolve_gatt_for_device(&cooler.device_path),
                )
                .await
                {
                    Ok(Ok((tx, rx))) if tx.is_some() => {
                        tx_opt = tx;
                        rx_opt = rx_opt.or(rx);
                        break;
                    }
                    Ok(Ok(_)) => {}
                    _ => break,
                }
                let remaining_sleep =
                    deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining_sleep.is_zero() {
                    break;
                }
                tokio::time::sleep(GATT_POLL_INTERVAL.min(remaining_sleep)).await;
            }
        }

        let Some(tx_path) = tx_opt else {
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                dev_proxy.call_method("Disconnect", &()),
            )
            .await;
            return Err(CoolerError::NotConnected);
        };

        if let Some(ref rx_path) = rx_opt {
            let Ok(rx_proxy) = Proxy::new(
                &self.conn,
                self.bus_name.clone(),
                rx_path.clone(),
                "org.bluez.GattCharacteristic1",
            )
            .await
            else {
                let _ = tokio::time::timeout(
                    Duration::from_millis(500),
                    dev_proxy.call_method("Disconnect", &()),
                )
                .await;
                return Err(CoolerError::NotConnected);
            };
            let remaining_notify = deadline.saturating_duration_since(tokio::time::Instant::now());
            let notify_ok = !remaining_notify.is_zero()
                && matches!(
                    tokio::time::timeout(
                        remaining_notify,
                        rx_proxy.call_method("StartNotify", &())
                    )
                    .await,
                    Ok(Ok(_))
                );
            if !notify_ok {
                let _ = tokio::time::timeout(
                    Duration::from_millis(500),
                    dev_proxy.call_method("Disconnect", &()),
                )
                .await;
                return Err(CoolerError::NotConnected);
            }
        }

        let tx_proxy = Proxy::new(
            &self.conn,
            self.bus_name.clone(),
            tx_path,
            "org.bluez.GattCharacteristic1",
        )
        .await
        .map_err(|_| CoolerError::NotConnected)?;

        Ok(BluezGattSession {
            dev_proxy,
            tx_proxy,
            rx_path: rx_opt,
            conn: self.conn.clone(),
            bus_name: self.bus_name.clone(),
        })
    }

    /// Starts BlueZ LE/adapter discovery on any available `org.bluez.Adapter1` objects (`ISSUE-36`).
    pub async fn start_discovery(&self) -> Result<(), CoolerError> {
        let objects = self.get_managed_objects().await?;
        for (path, ifaces) in &objects {
            if !ifaces.contains_key("org.bluez.Adapter1") {
                continue;
            }
            if let Ok(adapter) = Proxy::new(
                &self.conn,
                self.bus_name.clone(),
                path.as_str().to_string(),
                "org.bluez.Adapter1",
            )
            .await
            {
                let _ = adapter.call_method("StartDiscovery", &()).await;
            }
        }
        Ok(())
    }

    /// Discovers coolers and retries with backoff until a trusted match connects (late-adapter lifecycle).
    pub async fn discover_and_connect(
        &self,
        saved: Option<&SavedCoolerIdentity>,
        max_attempts: usize,
        backoff: Duration,
    ) -> Result<(DiscoveredCooler, BluezGattSession), CoolerError> {
        let attempts = max_attempts.max(1);
        for attempt in 0..attempts {
            if let Ok(coolers) = self.discover_coolers().await {
                if let Some(chosen) = select_cooler(&coolers, saved).cloned() {
                    if let Ok(session) = self.connect_cooler(&chosen).await {
                        return Ok((chosen, session));
                    }
                }
            }
            if attempt + 1 < attempts {
                tokio::time::sleep(backoff).await;
            }
        }
        Err(CoolerError::NotConnected)
    }
}

/// Active BlueZ GATT NUS session connected to a single cooler device.
#[derive(Debug)]
pub struct BluezGattSession {
    dev_proxy: Proxy<'static>,
    tx_proxy: Proxy<'static>,
    rx_path: Option<String>,
    conn: Connection,
    bus_name: String,
}

impl BluezGattSession {
    pub async fn is_connected(&self) -> bool {
        self.dev_proxy
            .get_property::<bool>("Connected")
            .await
            .unwrap_or(false)
    }

    pub async fn write_packet(&self, packet: [u8; 8]) -> Result<(), CoolerError> {
        CoolerPacketEncoder::validate_packet(&packet)?;
        let opts: HashMap<String, Value<'_>> = HashMap::new();
        self.tx_proxy
            .call_method("WriteValue", &(packet.to_vec(), opts))
            .await
            .map_err(|_| CoolerError::NotConnected)?;
        Ok(())
    }

    /// Flushes ready commands from [`CoolerController`] over this BlueZ session while preserving
    /// the 80ms gap and keeping any failed packet queued for retry (`W1-05`).
    pub async fn flush_controller(
        &self,
        controller: &mut CoolerController,
        now: Instant,
    ) -> Result<usize, CoolerError> {
        if !self.is_connected().await {
            return Err(CoolerError::NotConnected);
        }
        let mut sent = 0usize;
        while let Some(queued) = controller.peek_ready_at(now) {
            let pkt = queued.command.encode()?;
            match self.write_packet(pkt).await {
                Ok(()) => {
                    controller.commit_sent_at(now);
                    sent += 1;
                }
                Err(err) => {
                    controller.record_write_attempt_at(now);
                    return Err(err);
                }
            }
        }
        Ok(sent)
    }

    pub async fn disconnect(&self) -> Result<(), CoolerError> {
        if let Some(ref rx_path) = self.rx_path {
            if let Ok(rx_proxy) = Proxy::new(
                &self.conn,
                self.bus_name.clone(),
                rx_path.clone(),
                "org.bluez.GattCharacteristic1",
            )
            .await
            {
                let _ = rx_proxy.call_method("StopNotify", &()).await;
            }
        }
        self.dev_proxy
            .call_method("Disconnect", &())
            .await
            .map_err(|_| CoolerError::NotConnected)?;
        Ok(())
    }
}
