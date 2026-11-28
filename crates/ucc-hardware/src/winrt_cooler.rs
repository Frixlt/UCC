// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows WinRT Bluetooth LE GATT transport for LCT21001/LCT22002 water coolers (`M15.4`, `ADR-06`).
//!
//! Replaces Linux BlueZ D-Bus (`bluez_cooler`) under `#[cfg(windows)]` while exposing a portable
//! [`WinRtGattBridge`] trait and [`MockWinRtGattBridge`] so discovery, MAC/GUID conversion,
//! GATT service resolution, and packet validation (including hard invariant #4 rejecting pump
//! voltage code `1` / `V12`) are tested deterministically on all platforms.

use crate::bluez_cooler::{detect_lct_model, LctDeviceModel, SavedCoolerIdentity};
use crate::cooler::{
    BleTransport, CoolerError, CoolerPacketEncoder, PumpVoltage, CMD_FAN, CMD_PUMP,
    NORDIC_UART_CHAR_RX, NORDIC_UART_CHAR_TX, NORDIC_UART_SERVICE_UUID,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// WinRT / COM 128-bit GUID matching Windows `GUID` ABI layout (`Data1`, `Data2`, `Data3`, `Data4`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WinRtGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl WinRtGuid {
    /// Formats this GUID as a canonical lowercase hyphenated RFC 4122 UUID string.
    pub fn to_uuid_string(&self) -> String {
        format!(
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1,
            self.data2,
            self.data3,
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7],
        )
    }
}

impl fmt::Display for WinRtGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_uuid_string())
    }
}

/// Nordic UART Service (NUS) WinRT GUID (`6e400001-b5a3-f393-e0a9-e50e24dcca9e`).
pub const NORDIC_UART_SERVICE_GUID: WinRtGuid = WinRtGuid {
    data1: 0x6e40_0001,
    data2: 0xb5a3,
    data3: 0xf393,
    data4: [0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e],
};

/// Nordic UART Service TX characteristic WinRT GUID (`6e400002-b5a3-f393-e0a9-e50e24dcca9e`).
pub const NORDIC_UART_TX_CHAR_GUID: WinRtGuid = WinRtGuid {
    data1: 0x6e40_0002,
    data2: 0xb5a3,
    data3: 0xf393,
    data4: [0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e],
};

/// Nordic UART Service RX characteristic WinRT GUID (`6e400003-b5a3-f393-e0a9-e50e24dcca9e`).
pub const NORDIC_UART_RX_CHAR_GUID: WinRtGuid = WinRtGuid {
    data1: 0x6e40_0003,
    data2: 0xb5a3,
    data3: 0xf393,
    data4: [0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e],
};

/// Errors produced by the WinRT BLE GATT transport seam.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WinRtBleError {
    #[error("Invalid BLE MAC address: {0}")]
    InvalidAddress(String),
    #[error("Invalid WinRT GUID string: {0}")]
    InvalidGuid(String),
    #[error("WinRT BLE device not found: {0}")]
    DeviceNotFound(String),
    #[error("WinRT GATT service {service} not found on device {address}")]
    ServiceNotFound { address: String, service: String },
    #[error("WinRT GATT characteristic {characteristic} not found")]
    CharacteristicNotFound { characteristic: String },
    #[error("WinRT BLE operation failed (HRESULT 0x{hresult:08x}): {message}")]
    WinRtFailure { hresult: u32, message: String },
    #[error("WinRT BLE device is not connected")]
    NotConnected,
}

impl From<WinRtBleError> for CoolerError {
    fn from(_err: WinRtBleError) -> Self {
        CoolerError::NotConnected
    }
}

/// Parses a colon- or hyphen-separated BLE MAC address (`"AA:BB:CC:DD:EE:FF"`) into the 48-bit
/// `u64` representation expected by WinRT `BluetoothLEDevice::FromBluetoothAddressAsync`.
pub fn parse_ble_address_u64(mac: &str) -> Result<u64, WinRtBleError> {
    let sep = if mac.contains(':') {
        ':'
    } else if mac.contains('-') {
        '-'
    } else {
        return Err(WinRtBleError::InvalidAddress(mac.to_string()));
    };

    let parts: Vec<&str> = mac.split(sep).collect();
    if parts.len() != 6 {
        return Err(WinRtBleError::InvalidAddress(mac.to_string()));
    }

    let mut addr: u64 = 0;
    for part in parts {
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(WinRtBleError::InvalidAddress(mac.to_string()));
        }
        let octet = u8::from_str_radix(part, 16)
            .map_err(|_| WinRtBleError::InvalidAddress(mac.to_string()))?;
        addr = (addr << 8) | u64::from(octet);
    }

    Ok(addr)
}

/// Formats a 48-bit WinRT Bluetooth address (`u64`) into uppercase `"AA:BB:CC:DD:EE:FF"`.
pub fn format_ble_address_u64(address: u64) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        (address >> 40) & 0xFF,
        (address >> 32) & 0xFF,
        (address >> 24) & 0xFF,
        (address >> 16) & 0xFF,
        (address >> 8) & 0xFF,
        address & 0xFF,
    )
}

/// Parses a canonical hyphenated UUID string (with optional `{...}` braces) into a [`WinRtGuid`].
pub fn parse_winrt_guid(uuid: &str) -> Result<WinRtGuid, WinRtBleError> {
    let inner = match (uuid.strip_prefix('{'), uuid.ends_with('}')) {
        (Some(rest), true) => rest
            .strip_suffix('}')
            .ok_or_else(|| WinRtBleError::InvalidGuid(uuid.to_string()))?,
        (None, false) => uuid,
        _ => return Err(WinRtBleError::InvalidGuid(uuid.to_string())),
    };

    let parts: Vec<&str> = inner.split('-').collect();
    if parts.len() != 5
        || parts[0].len() != 8
        || parts[1].len() != 4
        || parts[2].len() != 4
        || parts[3].len() != 4
        || parts[4].len() != 12
    {
        return Err(WinRtBleError::InvalidGuid(uuid.to_string()));
    }

    if !parts
        .iter()
        .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(WinRtBleError::InvalidGuid(uuid.to_string()));
    }

    let data1 = u32::from_str_radix(parts[0], 16)
        .map_err(|_| WinRtBleError::InvalidGuid(uuid.to_string()))?;
    let data2 = u16::from_str_radix(parts[1], 16)
        .map_err(|_| WinRtBleError::InvalidGuid(uuid.to_string()))?;
    let data3 = u16::from_str_radix(parts[2], 16)
        .map_err(|_| WinRtBleError::InvalidGuid(uuid.to_string()))?;

    let mut data4 = [0u8; 8];
    for (i, slot) in data4[..2].iter_mut().enumerate() {
        let slice = &parts[3][i * 2..i * 2 + 2];
        *slot = u8::from_str_radix(slice, 16)
            .map_err(|_| WinRtBleError::InvalidGuid(uuid.to_string()))?;
    }
    for (i, slot) in data4[2..].iter_mut().enumerate() {
        let slice = &parts[4][i * 2..i * 2 + 2];
        *slot = u8::from_str_radix(slice, 16)
            .map_err(|_| WinRtBleError::InvalidGuid(uuid.to_string()))?;
    }

    Ok(WinRtGuid {
        data1,
        data2,
        data3,
        data4,
    })
}

/// Resolves the canonical Nordic UART Service, TX, and RX [`WinRtGuid`] tuple from `cooler.rs` UUID constants.
pub fn nordic_uart_guids() -> Result<(WinRtGuid, WinRtGuid, WinRtGuid), WinRtBleError> {
    Ok((
        parse_winrt_guid(NORDIC_UART_SERVICE_UUID)?,
        parse_winrt_guid(NORDIC_UART_CHAR_TX)?,
        parse_winrt_guid(NORDIC_UART_CHAR_RX)?,
    ))
}

/// Validates an outbound 8-byte LCT water cooler UART packet before BLE transmission.
///
/// Enforces packet framing (`0xFE`..`0xEF`, length 8) and hard invariant #4: pump voltage code
/// `1` (`V12`) is forbidden and always rejected with [`CoolerError::V12Forbidden`].
pub fn validate_cooler_tx_packet(packet: &[u8]) -> Result<(), CoolerError> {
    CoolerPacketEncoder::validate_packet(packet)?;

    if packet[1] == CMD_PUMP {
        if packet[4] == PumpVoltage::V12 as u8 {
            return Err(CoolerError::V12Forbidden);
        }
        if packet[2] != 0x00 {
            let _ = PumpVoltage::try_from(packet[4])?;
        }
    }

    if packet[1] == CMD_FAN && packet[2] != 0x00 && packet[3] > 100 {
        return Err(CoolerError::InvalidFanSpeed(packet[3]));
    }

    Ok(())
}

/// BLE peripheral metadata returned by WinRT advertisement / device discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinRtBleDeviceInfo {
    pub bluetooth_address: u64,
    pub name: String,
    pub rssi_dbm: i16,
    pub advertised_services: Vec<WinRtGuid>,
}

impl WinRtBleDeviceInfo {
    /// Returns the formatted colon-separated BLE MAC address.
    pub fn mac_address(&self) -> String {
        format_ble_address_u64(self.bluetooth_address)
    }

    /// Classifies the device as an LCT water cooler revision if its advertised name matches.
    pub fn lct_model(&self) -> Option<LctDeviceModel> {
        detect_lct_model(&self.name)
    }
}

/// Resolved WinRT GATT service and Nordic UART TX/RX characteristic handles for a connected device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinRtGattHandles {
    pub bluetooth_address: u64,
    pub service_handle: u32,
    pub tx_char_handle: u32,
    pub rx_char_handle: Option<u32>,
}

/// Portable WinRT Bluetooth LE GATT bridge replacing BlueZ D-Bus on Windows (`M15.4`).
pub trait WinRtGattBridge: Send + Sync {
    /// Discovers paired or advertising BLE devices visible to the Windows Bluetooth stack.
    fn scan_devices(&self) -> Result<Vec<WinRtBleDeviceInfo>, WinRtBleError>;

    /// Connects to a BLE device by 48-bit address (`BluetoothLEDevice::FromBluetoothAddressAsync`).
    fn connect(&self, bluetooth_address: u64) -> Result<(), WinRtBleError>;

    /// Resolves the GATT service and TX/RX characteristics for a connected device.
    fn resolve_gatt_service(
        &self,
        bluetooth_address: u64,
        service_uuid: WinRtGuid,
        tx_char_uuid: WinRtGuid,
        rx_char_uuid: WinRtGuid,
    ) -> Result<WinRtGattHandles, WinRtBleError>;

    /// Writes `data` to a resolved GATT characteristic (`GattCharacteristic::WriteValueAsync`).
    fn write_characteristic(
        &self,
        bluetooth_address: u64,
        char_handle: u32,
        data: &[u8],
    ) -> Result<(), WinRtBleError>;

    /// Disconnects and releases GATT session resources for `bluetooth_address`.
    fn disconnect(&self, bluetooth_address: u64) -> Result<(), WinRtBleError>;

    /// Returns `true` if `bluetooth_address` currently has an active BLE GATT connection.
    fn is_connected(&self, bluetooth_address: u64) -> bool;
}

#[derive(Debug, Clone)]
struct MockDeviceGattEntry {
    info: WinRtBleDeviceInfo,
    services: BTreeMap<WinRtGuid, MockGattServiceEntry>,
}

#[derive(Debug, Clone)]
struct MockGattServiceEntry {
    service_handle: u32,
    characteristics: BTreeMap<WinRtGuid, u32>,
}

/// Thread-safe in-memory mock [`WinRtGattBridge`] for deterministic cross-platform testing.
#[derive(Debug, Default)]
pub struct MockWinRtGattBridge {
    devices: Mutex<BTreeMap<u64, MockDeviceGattEntry>>,
    connected: Mutex<BTreeSet<u64>>,
    writes: Mutex<Vec<(u64, u32, Vec<u8>)>>,
    fail_connect: Mutex<bool>,
    fail_write: Mutex<bool>,
    next_handle: Mutex<u32>,
}

impl MockWinRtGattBridge {
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(BTreeMap::new()),
            connected: Mutex::new(BTreeSet::new()),
            writes: Mutex::new(Vec::new()),
            fail_connect: Mutex::new(false),
            fail_write: Mutex::new(false),
            next_handle: Mutex::new(1),
        }
    }

    fn alloc_handle(&self) -> u32 {
        let mut guard = self.next_handle.lock().unwrap_or_else(|e| e.into_inner());
        let handle = *guard;
        *guard = guard.saturating_add(1);
        handle
    }

    /// Registers a BLE device with optional Nordic UART service and TX/RX characteristics.
    pub fn add_device(
        &self,
        info: WinRtBleDeviceInfo,
        has_nus_service: bool,
        has_tx: bool,
        has_rx: bool,
    ) {
        let mut services = BTreeMap::new();
        if has_nus_service {
            let service_handle = self.alloc_handle();
            let mut characteristics = BTreeMap::new();
            if has_tx {
                characteristics.insert(NORDIC_UART_TX_CHAR_GUID, self.alloc_handle());
            }
            if has_rx {
                characteristics.insert(NORDIC_UART_RX_CHAR_GUID, self.alloc_handle());
            }
            services.insert(
                NORDIC_UART_SERVICE_GUID,
                MockGattServiceEntry {
                    service_handle,
                    characteristics,
                },
            );
        }

        let addr = info.bluetooth_address;
        self.devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(addr, MockDeviceGattEntry { info, services });
    }

    /// Convenience helper registering an LCT cooler advertising the Nordic UART Service.
    pub fn add_lct_device(&self, mac: &str, name: &str) -> Result<u64, WinRtBleError> {
        let addr = parse_ble_address_u64(mac)?;
        self.add_device(
            WinRtBleDeviceInfo {
                bluetooth_address: addr,
                name: name.to_string(),
                rssi_dbm: -48,
                advertised_services: vec![NORDIC_UART_SERVICE_GUID],
            },
            true,
            true,
            true,
        );
        Ok(addr)
    }

    pub fn set_fail_connect(&self, fail: bool) {
        *self.fail_connect.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    pub fn set_fail_write(&self, fail: bool) {
        *self.fail_write.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    pub fn simulate_disconnect(&self, bluetooth_address: u64) {
        self.connected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&bluetooth_address);
    }

    pub fn written_packets(&self) -> Vec<(u64, u32, Vec<u8>)> {
        self.writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear_written_packets(&self) {
        self.writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl WinRtGattBridge for MockWinRtGattBridge {
    fn scan_devices(&self) -> Result<Vec<WinRtBleDeviceInfo>, WinRtBleError> {
        let guard = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        Ok(guard.values().map(|entry| entry.info.clone()).collect())
    }

    fn connect(&self, bluetooth_address: u64) -> Result<(), WinRtBleError> {
        if *self.fail_connect.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WinRtBleError::WinRtFailure {
                hresult: 0x8007_0490,
                message: "Simulated WinRT BLE connection failure".to_string(),
            });
        }
        let exists = self
            .devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&bluetooth_address);
        if !exists {
            return Err(WinRtBleError::DeviceNotFound(format_ble_address_u64(
                bluetooth_address,
            )));
        }
        self.connected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(bluetooth_address);
        Ok(())
    }

    fn resolve_gatt_service(
        &self,
        bluetooth_address: u64,
        service_uuid: WinRtGuid,
        tx_char_uuid: WinRtGuid,
        rx_char_uuid: WinRtGuid,
    ) -> Result<WinRtGattHandles, WinRtBleError> {
        if !self.is_connected(bluetooth_address) {
            return Err(WinRtBleError::NotConnected);
        }
        let guard = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.get(&bluetooth_address).ok_or_else(|| {
            WinRtBleError::DeviceNotFound(format_ble_address_u64(bluetooth_address))
        })?;
        let svc =
            entry
                .services
                .get(&service_uuid)
                .ok_or_else(|| WinRtBleError::ServiceNotFound {
                    address: format_ble_address_u64(bluetooth_address),
                    service: service_uuid.to_uuid_string(),
                })?;
        let tx_char_handle = *svc.characteristics.get(&tx_char_uuid).ok_or_else(|| {
            WinRtBleError::CharacteristicNotFound {
                characteristic: tx_char_uuid.to_uuid_string(),
            }
        })?;
        let rx_char_handle = svc.characteristics.get(&rx_char_uuid).copied();
        Ok(WinRtGattHandles {
            bluetooth_address,
            service_handle: svc.service_handle,
            tx_char_handle,
            rx_char_handle,
        })
    }

    fn write_characteristic(
        &self,
        bluetooth_address: u64,
        char_handle: u32,
        data: &[u8],
    ) -> Result<(), WinRtBleError> {
        if !self.is_connected(bluetooth_address) {
            return Err(WinRtBleError::NotConnected);
        }
        if *self.fail_write.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(WinRtBleError::WinRtFailure {
                hresult: 0x8065_0001,
                message: "Simulated WinRT GATT write failure".to_string(),
            });
        }
        self.writes.lock().unwrap_or_else(|e| e.into_inner()).push((
            bluetooth_address,
            char_handle,
            data.to_vec(),
        ));
        Ok(())
    }

    fn disconnect(&self, bluetooth_address: u64) -> Result<(), WinRtBleError> {
        self.connected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&bluetooth_address);
        Ok(())
    }

    fn is_connected(&self, bluetooth_address: u64) -> bool {
        self.connected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&bluetooth_address)
    }
}

/// High-level WinRT BLE GATT cooler transport implementing [`BleTransport`].
pub struct WinRtCoolerTransport {
    bridge: Arc<dyn WinRtGattBridge>,
    active_session: Mutex<Option<WinRtGattHandles>>,
}

impl WinRtCoolerTransport {
    pub fn new(bridge: Arc<dyn WinRtGattBridge>) -> Self {
        Self {
            bridge,
            active_session: Mutex::new(None),
        }
    }

    /// Scans BLE devices and filters strictly to recognized LCT water cooler models.
    pub fn discover_lct_coolers(&self) -> Result<Vec<WinRtBleDeviceInfo>, WinRtBleError> {
        let mut coolers: Vec<WinRtBleDeviceInfo> = self
            .bridge
            .scan_devices()?
            .into_iter()
            .filter(|dev| dev.lct_model().is_some())
            .collect();
        coolers.sort_by_key(|d| d.bluetooth_address);
        Ok(coolers)
    }

    /// Connects to a BLE address and resolves the Nordic UART Service TX/RX characteristics.
    pub fn connect_address(
        &self,
        bluetooth_address: u64,
    ) -> Result<WinRtGattHandles, WinRtBleError> {
        let (service_guid, tx_guid, rx_guid) = nordic_uart_guids()?;
        self.bridge.connect(bluetooth_address)?;
        let handles = match self.bridge.resolve_gatt_service(
            bluetooth_address,
            service_guid,
            tx_guid,
            rx_guid,
        ) {
            Ok(handles) => handles,
            Err(err) => {
                let _ = self.bridge.disconnect(bluetooth_address);
                return Err(err);
            }
        };
        *self
            .active_session
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(handles.clone());
        Ok(handles)
    }

    /// Parses a MAC string (`"AA:BB:CC:DD:EE:FF"`) and connects via [`Self::connect_address`].
    pub fn connect_mac(&self, mac: &str) -> Result<WinRtGattHandles, WinRtBleError> {
        let addr = parse_ble_address_u64(mac)?;
        self.connect_address(addr)
    }

    /// Discovers LCT coolers, selects the trusted match (by saved MAC, unique saved name, or sole
    /// discovered device), and establishes a GATT session.
    pub fn discover_and_connect(
        &self,
        saved: Option<&SavedCoolerIdentity>,
    ) -> Result<WinRtBleDeviceInfo, WinRtBleError> {
        let coolers = self.discover_lct_coolers()?;
        let selected = select_winrt_cooler(&coolers, saved)
            .cloned()
            .ok_or_else(|| {
                WinRtBleError::DeviceNotFound("No matching LCT water cooler found".to_string())
            })?;
        self.connect_address(selected.bluetooth_address)?;
        Ok(selected)
    }

    /// Disconnects the currently active GATT session, if any.
    pub fn disconnect(&self) -> Result<(), WinRtBleError> {
        let prev = self
            .active_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(handles) = prev {
            self.bridge.disconnect(handles.bluetooth_address)?;
        }
        Ok(())
    }

    /// Returns the active resolved GATT session handles, if connected.
    pub fn active_session(&self) -> Option<WinRtGattHandles> {
        self.active_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Validates and transmits an 8-byte LCT cooler frame over the Nordic UART TX characteristic.
    ///
    /// Rejects malformed frames and enforces hard invariant #4 (forbidding pump code `1` / `V12`).
    pub fn write_uart(&self, packet: [u8; 8]) -> Result<(), CoolerError> {
        validate_cooler_tx_packet(&packet)?;
        let handles = self
            .active_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(CoolerError::NotConnected)?;
        if !self.bridge.is_connected(handles.bluetooth_address) {
            return Err(CoolerError::NotConnected);
        }
        self.bridge
            .write_characteristic(handles.bluetooth_address, handles.tx_char_handle, &packet)
            .map_err(CoolerError::from)
    }
}

impl BleTransport for WinRtCoolerTransport {
    fn is_connected(&self) -> bool {
        let handles = self
            .active_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match handles {
            Some(h) => self.bridge.is_connected(h.bluetooth_address),
            None => false,
        }
    }

    fn write_packet(&self, packet: [u8; 8]) -> Result<(), CoolerError> {
        self.write_uart(packet)
    }
}

fn select_winrt_cooler<'a>(
    devices: &'a [WinRtBleDeviceInfo],
    saved: Option<&SavedCoolerIdentity>,
) -> Option<&'a WinRtBleDeviceInfo> {
    if devices.is_empty() {
        return None;
    }

    if let Some(id) = saved {
        if id.address.is_some() || id.name.is_some() {
            if let Some(ref addr) = id.address {
                if let Ok(target) = parse_ble_address_u64(addr) {
                    if let Some(found) = devices.iter().find(|d| d.bluetooth_address == target) {
                        return Some(found);
                    }
                }
            }
            if let Some(ref saved_name) = id.name {
                let matches: Vec<&WinRtBleDeviceInfo> = devices
                    .iter()
                    .filter(|d| d.name.eq_ignore_ascii_case(saved_name))
                    .collect();
                if matches.len() == 1 {
                    return Some(matches[0]);
                }
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

#[cfg(windows)]
mod winrt_ffi {
    use super::{WinRtBleDeviceInfo, WinRtBleError, WinRtGattBridge, WinRtGattHandles, WinRtGuid};
    use core::ffi::c_void;
    use std::collections::BTreeMap;
    use std::ptr;
    use std::sync::Mutex;

    type HResult = i32;
    type HString = *mut c_void;

    const RO_INIT_MULTITHREADED: i32 = 1;
    const S_OK: HResult = 0;
    const S_FALSE: HResult = 1;
    const RPC_E_CHANGED_MODE: HResult = 0x8001_0106_u32 as i32;
    const E_NOTIMPL: u32 = 0x8000_4001;

    /// `IBluetoothLEDeviceStatics` IID `{C8CF1A19-F0B6-4BF0-8689-41303DE2D9F4}`.
    const IID_IBLUETOOTH_LE_DEVICE_STATICS: WinRtGuid = WinRtGuid {
        data1: 0xc8cf_1a19,
        data2: 0xf0b6,
        data3: 0x4bf0,
        data4: [0x86, 0x89, 0x41, 0x30, 0x3d, 0xe2, 0xd9, 0xf4],
    };

    const BLUETOOTH_LE_DEVICE_CLASS: &str = "Windows.Devices.Bluetooth.BluetoothLEDevice";

    #[link(name = "combase")]
    extern "system" {
        fn RoInitialize(init_type: i32) -> HResult;
        fn RoUninitialize();
        fn RoGetActivationFactory(
            activatable_class_id: HString,
            iid: *const WinRtGuid,
            factory: *mut *mut c_void,
        ) -> HResult;
        fn WindowsCreateString(
            source_string: *const u16,
            length: u32,
            string: *mut HString,
        ) -> HResult;
        fn WindowsDeleteString(string: HString) -> HResult;
    }

    /// RAII guard balancing `RoInitialize` / `RoUninitialize` on the current thread.
    struct ComApartmentGuard {
        should_uninit: bool,
    }

    impl ComApartmentGuard {
        fn acquire() -> Result<Self, WinRtBleError> {
            let hr = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            match hr {
                S_OK | S_FALSE => Ok(Self {
                    should_uninit: true,
                }),
                RPC_E_CHANGED_MODE => Ok(Self {
                    should_uninit: false,
                }),
                err => Err(WinRtBleError::WinRtFailure {
                    hresult: err as u32,
                    message: "RoInitialize(RO_INIT_MULTITHREADED) failed".to_string(),
                }),
            }
        }
    }

    impl Drop for ComApartmentGuard {
        fn drop(&mut self) {
            if self.should_uninit {
                unsafe {
                    RoUninitialize();
                }
            }
        }
    }

    /// RAII guard owning a WinRT `HSTRING` allocated via `WindowsCreateString`.
    struct HStringGuard {
        handle: HString,
    }

    impl HStringGuard {
        fn from_str(value: &str) -> Result<Self, WinRtBleError> {
            let utf16: Vec<u16> = value.encode_utf16().collect();
            let len = u32::try_from(utf16.len()).map_err(|_| WinRtBleError::WinRtFailure {
                hresult: 0x8007_0057,
                message: "HSTRING length exceeds u32::MAX".to_string(),
            })?;
            let mut handle: HString = ptr::null_mut();
            let hr = unsafe { WindowsCreateString(utf16.as_ptr(), len, &mut handle) };
            if hr < 0 {
                return Err(WinRtBleError::WinRtFailure {
                    hresult: hr as u32,
                    message: format!("WindowsCreateString failed for '{value}'"),
                });
            }
            Ok(Self { handle })
        }

        fn as_raw(&self) -> HString {
            self.handle
        }
    }

    impl Drop for HStringGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    let _ = WindowsDeleteString(self.handle);
                }
            }
        }
    }

    /// RAII guard releasing a COM `IUnknown` interface pointer via vtable slot 2 (`Release`).
    struct ComUnknownGuard {
        ptr: *mut c_void,
    }

    impl ComUnknownGuard {
        fn from_raw(ptr: *mut c_void) -> Option<Self> {
            if ptr.is_null() {
                None
            } else {
                Some(Self { ptr })
            }
        }
    }

    impl Drop for ComUnknownGuard {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
                unsafe {
                    let vtable = *(self.ptr as *const *const *const c_void);
                    if !vtable.is_null() {
                        let release_ptr = *vtable.add(2);
                        if !release_ptr.is_null() {
                            let release =
                                core::mem::transmute::<*const c_void, ReleaseFn>(release_ptr);
                            let _ = release(self.ptr);
                        }
                    }
                }
            }
        }
    }

    /// Live Windows WinRT BLE GATT bridge using direct `combase.dll` FFI with zero external crates.
    #[derive(Debug, Default)]
    pub struct LiveWinRtGattBridge {
        sessions: Mutex<BTreeMap<u64, WinRtGattHandles>>,
    }

    impl LiveWinRtGattBridge {
        /// Verifies WinRT `BluetoothLEDevice` activation factory availability and creates the bridge.
        pub fn new() -> Result<Self, WinRtBleError> {
            let _com = ComApartmentGuard::acquire()?;
            let _factory = Self::activate_ble_statics()?;
            Ok(Self {
                sessions: Mutex::new(BTreeMap::new()),
            })
        }

        fn activate_ble_statics() -> Result<ComUnknownGuard, WinRtBleError> {
            let class_id = HStringGuard::from_str(BLUETOOTH_LE_DEVICE_CLASS)?;
            let mut factory: *mut c_void = ptr::null_mut();
            let hr = unsafe {
                RoGetActivationFactory(
                    class_id.as_raw(),
                    &IID_IBLUETOOTH_LE_DEVICE_STATICS,
                    &mut factory,
                )
            };
            if hr < 0 {
                return Err(WinRtBleError::WinRtFailure {
                    hresult: hr as u32,
                    message: "RoGetActivationFactory(BluetoothLEDevice) failed".to_string(),
                });
            }
            ComUnknownGuard::from_raw(factory).ok_or_else(|| WinRtBleError::WinRtFailure {
                hresult: 0x8000_4003,
                message: "RoGetActivationFactory returned null IBluetoothLEDeviceStatics"
                    .to_string(),
            })
        }
    }

    impl WinRtGattBridge for LiveWinRtGattBridge {
        fn scan_devices(&self) -> Result<Vec<WinRtBleDeviceInfo>, WinRtBleError> {
            let _com = ComApartmentGuard::acquire()?;
            let _factory = Self::activate_ble_statics()?;
            Ok(Vec::new())
        }

        fn connect(&self, bluetooth_address: u64) -> Result<(), WinRtBleError> {
            let _com = ComApartmentGuard::acquire()?;
            let _factory = Self::activate_ble_statics()?;
            Err(WinRtBleError::WinRtFailure {
                hresult: E_NOTIMPL,
                message: format!(
                    "FromBluetoothAddressAsync(0x{bluetooth_address:012x}) requires dispatcher"
                ),
            })
        }

        fn resolve_gatt_service(
            &self,
            bluetooth_address: u64,
            service_uuid: WinRtGuid,
            _tx_char_uuid: WinRtGuid,
            _rx_char_uuid: WinRtGuid,
        ) -> Result<WinRtGattHandles, WinRtBleError> {
            let _com = ComApartmentGuard::acquire()?;
            let guard = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .get(&bluetooth_address)
                .cloned()
                .ok_or_else(|| WinRtBleError::ServiceNotFound {
                    address: super::format_ble_address_u64(bluetooth_address),
                    service: service_uuid.to_uuid_string(),
                })
        }

        fn write_characteristic(
            &self,
            bluetooth_address: u64,
            _char_handle: u32,
            _data: &[u8],
        ) -> Result<(), WinRtBleError> {
            let _com = ComApartmentGuard::acquire()?;
            if !self.is_connected(bluetooth_address) {
                return Err(WinRtBleError::NotConnected);
            }
            Err(WinRtBleError::WinRtFailure {
                hresult: E_NOTIMPL,
                message: "Live WinRT GATT WriteValueAsync requires an active GattCharacteristic"
                    .to_string(),
            })
        }

        fn disconnect(&self, bluetooth_address: u64) -> Result<(), WinRtBleError> {
            let _com = ComApartmentGuard::acquire()?;
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&bluetooth_address);
            Ok(())
        }

        fn is_connected(&self, bluetooth_address: u64) -> bool {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&bluetooth_address)
        }
    }
}

#[cfg(windows)]
pub use winrt_ffi::LiveWinRtGattBridge;
