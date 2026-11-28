// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows USB HID feature-report transport (`HidD_SetFeature` / `HidD_GetFeature`) (`M15.4`, `ADR-06`).
//!
//! Implements [`HidDeviceTransport`] for ITE `048D:600B` (calibrated per-key RGB keyboard) and
//! ITE `048D:7001`/`048D:7000` (front Lightbar + lid logo LED zone) with a portable
//! [`WinHidSysApi`] seam, [`MockWinHidSysApi`] for cross-platform tests, and direct Windows
//! `hid.dll` / `setupapi.dll` / `kernel32.dll` FFI under `#[cfg(windows)]`.

use crate::hid_transport::{HidDeviceInfo, HidDeviceTransport};
use crate::transport::LightingError;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Minimum HID vendor-defined usage page (`0xFF00..=0xFFFF`) used to select ITE lighting interfaces
/// instead of the standard boot keyboard interface (`0x0001`).
pub const VENDOR_USAGE_PAGE_MIN: u16 = 0xFF00;

/// Metadata for a Windows HID device interface enumerated via SetupAPI + `hid.dll`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinHidDeviceRecord {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub feature_report_byte_length: usize,
}

impl WinHidDeviceRecord {
    /// Converts this Windows HID record into the cross-platform [`HidDeviceInfo`] descriptor.
    pub fn to_hid_device_info(&self) -> HidDeviceInfo {
        HidDeviceInfo {
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            path: self.path.clone(),
        }
    }

    /// Returns `true` when this interface belongs to a vendor-defined HID usage page (`>= 0xFF00`).
    pub fn is_vendor_lighting_interface(&self) -> bool {
        self.usage_page >= VENDOR_USAGE_PAGE_MIN
    }
}

/// Frames a HID feature report payload for Windows `HidD_SetFeature`.
///
/// Windows `HidD_SetFeature` expects a buffer of exact length `FeatureReportByteLength` whose first
/// byte is the HID Report ID (`0x00` for unnumbered reports). If `report` does not already begin
/// with `0x00`, a leading `0x00` Report ID byte is prepended. The resulting buffer is zero-padded
/// to `feature_report_byte_length`, rejecting empty payloads or payloads exceeding the device limit.
pub fn frame_windows_feature_report(
    report: &[u8],
    feature_report_byte_length: usize,
) -> Result<Vec<u8>, LightingError> {
    if report.is_empty() {
        return Err(LightingError::InvalidConfig(
            "HID feature report must not be empty".to_string(),
        ));
    }
    if feature_report_byte_length == 0 {
        return Err(LightingError::InvalidConfig(
            "HID device reports FeatureReportByteLength == 0".to_string(),
        ));
    }

    let mut framed = vec![0u8; feature_report_byte_length];
    if report[0] == 0x00 {
        if report.len() > feature_report_byte_length {
            return Err(LightingError::InvalidConfig(format!(
                "HID feature report length {} exceeds device FeatureReportByteLength {}",
                report.len(),
                feature_report_byte_length
            )));
        }
        framed[..report.len()].copy_from_slice(report);
    } else {
        let needed = report.len().saturating_add(1);
        if needed > feature_report_byte_length {
            return Err(LightingError::InvalidConfig(format!(
                "HID feature report {} (+1 Report ID) exceeds FeatureReportByteLength {}",
                report.len(),
                feature_report_byte_length
            )));
        }
        framed[0] = 0x00;
        framed[1..1 + report.len()].copy_from_slice(report);
    }

    Ok(framed)
}

/// Strips an optional leading `0x00` Report ID byte from a `HidD_GetFeature` buffer and resizes
/// cleanly to `requested_len` (truncating or zero-padding as needed).
pub fn unframe_windows_feature_report(raw: &[u8], requested_len: usize) -> Vec<u8> {
    let payload = if raw.first() == Some(&0x00) && raw.len() > 1 {
        &raw[1..]
    } else {
        raw
    };
    let mut out = vec![0u8; requested_len];
    let copy_len = payload.len().min(requested_len);
    out[..copy_len].copy_from_slice(&payload[..copy_len]);
    out
}

/// Low-level Windows HID system API seam (`SetupDi*` + `HidD_SetFeature` / `HidD_GetFeature`).
pub trait WinHidSysApi: Send + Sync {
    /// Enumerates all present HID device interfaces with VID, PID, UsagePage, and FeatureReportByteLength.
    fn enumerate_hid_devices(&self) -> Result<Vec<WinHidDeviceRecord>, LightingError>;

    /// Sends a pre-framed feature report buffer via `HidD_SetFeature`.
    fn set_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError>;

    /// Reads a feature report buffer via `HidD_GetFeature` (caller sets `buf[0]` to Report ID).
    fn get_feature_report(&self, path: &str, buf: &mut [u8]) -> Result<(), LightingError>;
}

/// Thread-safe in-memory mock [`WinHidSysApi`] enforcing Windows HID feature report sizing.
#[derive(Debug, Default)]
pub struct MockWinHidSysApi {
    devices: Mutex<Vec<WinHidDeviceRecord>>,
    sent_reports: Mutex<Vec<(String, Vec<u8>)>>,
    feature_buffers: Mutex<BTreeMap<String, Vec<u8>>>,
    fail_writes: Mutex<bool>,
    fail_reads: Mutex<bool>,
}

impl MockWinHidSysApi {
    pub fn new(devices: Vec<WinHidDeviceRecord>) -> Self {
        Self {
            devices: Mutex::new(devices),
            sent_reports: Mutex::new(Vec::new()),
            feature_buffers: Mutex::new(BTreeMap::new()),
            fail_writes: Mutex::new(false),
            fail_reads: Mutex::new(false),
        }
    }

    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    pub fn set_fail_reads(&self, fail: bool) {
        *self.fail_reads.lock().unwrap_or_else(|e| e.into_inner()) = fail;
    }

    pub fn sent_reports(&self) -> Vec<(String, Vec<u8>)> {
        self.sent_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn seed_feature_buffer(&self, path: &str, buf: Vec<u8>) {
        self.feature_buffers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_ascii_lowercase(), buf);
    }

    fn find_device(&self, path: &str) -> Option<WinHidDeviceRecord> {
        self.devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|d| d.path.eq_ignore_ascii_case(path))
            .cloned()
    }
}

impl WinHidSysApi for MockWinHidSysApi {
    fn enumerate_hid_devices(&self) -> Result<Vec<WinHidDeviceRecord>, LightingError> {
        Ok(self
            .devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    fn set_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        if *self.fail_writes.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(LightingError::WriteFailed(
                "Simulated HidD_SetFeature failure".to_string(),
            ));
        }
        let dev = self.find_device(path).ok_or(LightingError::Unavailable)?;
        if report.len() != dev.feature_report_byte_length {
            return Err(LightingError::WriteFailed(format!(
                "HidD_SetFeature buffer length {} != device FeatureReportByteLength {}",
                report.len(),
                dev.feature_report_byte_length
            )));
        }
        self.sent_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((dev.path.clone(), report.to_vec()));
        self.feature_buffers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(dev.path.to_ascii_lowercase(), report.to_vec());
        Ok(())
    }

    fn get_feature_report(&self, path: &str, buf: &mut [u8]) -> Result<(), LightingError> {
        if *self.fail_reads.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(LightingError::WriteFailed(
                "Simulated HidD_GetFeature failure".to_string(),
            ));
        }
        let dev = self.find_device(path).ok_or(LightingError::Unavailable)?;
        if buf.len() != dev.feature_report_byte_length {
            return Err(LightingError::InvalidConfig(format!(
                "HidD_GetFeature buffer length {} != device FeatureReportByteLength {}",
                buf.len(),
                dev.feature_report_byte_length
            )));
        }
        let stored = self
            .feature_buffers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&dev.path.to_ascii_lowercase())
            .cloned();
        buf.fill(0);
        if let Some(src) = stored {
            let n = src.len().min(buf.len());
            buf[..n].copy_from_slice(&src[..n]);
        }
        Ok(())
    }
}

/// Windows USB HID [`HidDeviceTransport`] backed by [`WinHidSysApi`].
pub struct WindowsHidTransport {
    sys: Arc<dyn WinHidSysApi>,
    device_cache: Mutex<BTreeMap<String, WinHidDeviceRecord>>,
}

impl WindowsHidTransport {
    pub fn new(sys: Arc<dyn WinHidSysApi>) -> Self {
        Self {
            sys,
            device_cache: Mutex::new(BTreeMap::new()),
        }
    }

    fn refresh_cache(&self) -> Result<Vec<WinHidDeviceRecord>, LightingError> {
        let records = self.sys.enumerate_hid_devices()?;
        let mut cache = self.device_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.clear();
        for rec in &records {
            cache.insert(rec.path.to_ascii_lowercase(), rec.clone());
        }
        Ok(records)
    }

    fn resolve_record(&self, path: &str) -> Result<WinHidDeviceRecord, LightingError> {
        let key = path.to_ascii_lowercase();
        if let Some(rec) = self
            .device_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
        {
            return Ok(rec);
        }
        let records = self.refresh_cache()?;
        records
            .into_iter()
            .find(|r| r.path.eq_ignore_ascii_case(path))
            .ok_or(LightingError::Unavailable)
    }
}

impl HidDeviceTransport for WindowsHidTransport {
    fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError> {
        let records = self.refresh_cache()?;
        Ok(records
            .into_iter()
            .map(|r| r.to_hid_device_info())
            .collect())
    }

    fn send_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        let rec = self.resolve_record(path)?;
        let framed = frame_windows_feature_report(report, rec.feature_report_byte_length)?;
        self.sys.set_feature_report(&rec.path, &framed)
    }

    fn get_feature_report(&self, path: &str, len: usize) -> Result<Vec<u8>, LightingError> {
        let rec = self.resolve_record(path)?;
        if rec.feature_report_byte_length == 0 {
            return Err(LightingError::InvalidConfig(
                "HID device reports FeatureReportByteLength == 0".to_string(),
            ));
        }
        let mut raw = vec![0u8; rec.feature_report_byte_length];
        raw[0] = 0x00;
        self.sys.get_feature_report(&rec.path, &mut raw)?;
        Ok(unframe_windows_feature_report(&raw, len))
    }

    fn device_usage_page(&self, path: &str) -> Option<u16> {
        self.resolve_record(path).ok().map(|r| r.usage_page)
    }

    fn device_interface_number(&self, _path: &str) -> Option<i32> {
        None
    }
}

#[cfg(windows)]
mod win_hid_ffi {
    use super::{WinHidDeviceRecord, WinHidSysApi};
    use crate::transport::LightingError;
    use core::ffi::c_void;
    use std::mem::size_of;
    use std::ptr;

    type Handle = *mut c_void;
    type HDevInfo = *mut c_void;
    type PreparsedData = *mut c_void;
    type Bool = i32;
    type Boolean = u8;
    type NtStatus = i32;

    const INVALID_HANDLE_VALUE: Handle = (-1isize) as Handle;
    const DIGCF_PRESENT: u32 = 0x0000_0002;
    const DIGCF_DEVICEINTERFACE: u32 = 0x0000_0010;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const HIDP_STATUS_SUCCESS: NtStatus = 0x0011_0000_u32 as i32;

    #[cfg(target_pointer_width = "64")]
    const SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE: u32 = 8;
    #[cfg(target_pointer_width = "32")]
    const SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE: u32 = 6;

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Default)]
    struct HidGuid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct HiddAttributes {
        size: u32,
        vendor_id: u16,
        product_id: u16,
        version_number: u16,
    }

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Default)]
    struct HidpCaps {
        usage: u16,
        usage_page: u16,
        input_report_byte_length: u16,
        output_report_byte_length: u16,
        feature_report_byte_length: u16,
        reserved: [u16; 17],
        number_link_collection_nodes: u16,
        number_input_button_caps: u16,
        number_input_value_caps: u16,
        number_input_data_indices: u16,
        number_output_button_caps: u16,
        number_output_value_caps: u16,
        number_output_data_indices: u16,
        number_feature_button_caps: u16,
        number_feature_value_caps: u16,
        number_feature_data_indices: u16,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct SpDeviceInterfaceData {
        cb_size: u32,
        interface_class_guid: HidGuid,
        flags: u32,
        reserved: usize,
    }

    #[link(name = "hid")]
    extern "system" {
        fn HidD_GetHidGuid(hid_guid: *mut HidGuid);
        fn HidD_GetAttributes(
            hid_device_object: Handle,
            attributes: *mut HiddAttributes,
        ) -> Boolean;
        fn HidD_GetPreparsedData(
            hid_device_object: Handle,
            preparsed_data: *mut PreparsedData,
        ) -> Boolean;
        fn HidD_FreePreparsedData(preparsed_data: PreparsedData) -> Boolean;
        fn HidP_GetCaps(preparsed_data: PreparsedData, capabilities: *mut HidpCaps) -> NtStatus;
        fn HidD_SetFeature(
            hid_device_object: Handle,
            report_buffer: *const c_void,
            report_buffer_length: u32,
        ) -> Boolean;
        fn HidD_GetFeature(
            hid_device_object: Handle,
            report_buffer: *mut c_void,
            report_buffer_length: u32,
        ) -> Boolean;
    }

    #[link(name = "setupapi")]
    extern "system" {
        fn SetupDiGetClassDevsW(
            class_guid: *const HidGuid,
            enumerator: *const u16,
            hwnd_parent: Handle,
            flags: u32,
        ) -> HDevInfo;
        fn SetupDiEnumDeviceInterfaces(
            device_info_set: HDevInfo,
            device_info_data: *const c_void,
            interface_class_guid: *const HidGuid,
            member_index: u32,
            device_interface_data: *mut SpDeviceInterfaceData,
        ) -> Bool;
        fn SetupDiGetDeviceInterfaceDetailW(
            device_info_set: HDevInfo,
            device_interface_data: *const SpDeviceInterfaceData,
            device_interface_detail_data: *mut c_void,
            device_interface_detail_data_size: u32,
            required_size: *mut u32,
            device_info_data: *mut c_void,
        ) -> Bool;
        fn SetupDiDestroyDeviceInfoList(device_info_set: HDevInfo) -> Bool;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: Handle,
        ) -> Handle;
        fn CloseHandle(object: Handle) -> Bool;
    }

    struct WinHandleGuard {
        handle: Handle,
    }

    impl WinHandleGuard {
        fn open(path: &str, desired_access: u32) -> Result<Self, LightingError> {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    desired_access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    ptr::null(),
                    OPEN_EXISTING,
                    0,
                    ptr::null_mut(),
                )
            };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(LightingError::Io(std::io::Error::last_os_error()));
            }
            Ok(Self { handle })
        }

        fn as_raw(&self) -> Handle {
            self.handle
        }
    }

    impl Drop for WinHandleGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
                unsafe {
                    let _ = CloseHandle(self.handle);
                }
            }
        }
    }

    struct DeviceInfoSetGuard {
        set: HDevInfo,
    }

    impl DeviceInfoSetGuard {
        fn for_hid_class(guid: &HidGuid) -> Result<Self, LightingError> {
            let set = unsafe {
                SetupDiGetClassDevsW(
                    guid,
                    ptr::null(),
                    ptr::null_mut(),
                    DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
                )
            };
            if set.is_null() || set == INVALID_HANDLE_VALUE {
                return Err(LightingError::Io(std::io::Error::last_os_error()));
            }
            Ok(Self { set })
        }

        fn as_raw(&self) -> HDevInfo {
            self.set
        }
    }

    impl Drop for DeviceInfoSetGuard {
        fn drop(&mut self) {
            if !self.set.is_null() && self.set != INVALID_HANDLE_VALUE {
                unsafe {
                    let _ = SetupDiDestroyDeviceInfoList(self.set);
                }
            }
        }
    }

    struct PreparsedDataGuard {
        ptr: PreparsedData,
    }

    impl PreparsedDataGuard {
        fn acquire(handle: Handle) -> Option<Self> {
            let mut ptr: PreparsedData = ptr::null_mut();
            let ok = unsafe { HidD_GetPreparsedData(handle, &mut ptr) };
            if ok != 0 && !ptr.is_null() {
                Some(Self { ptr })
            } else {
                None
            }
        }

        fn as_raw(&self) -> PreparsedData {
            self.ptr
        }
    }

    impl Drop for PreparsedDataGuard {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                unsafe {
                    let _ = HidD_FreePreparsedData(self.ptr);
                }
            }
        }
    }

    /// Live Windows HID system API backed by `hid.dll`, `setupapi.dll`, and `kernel32.dll`.
    #[derive(Debug, Default)]
    pub struct LiveWinHidSysApi;

    impl LiveWinHidSysApi {
        pub fn new() -> Self {
            Self
        }

        fn read_interface_path(
            dev_info: HDevInfo,
            iface_data: &SpDeviceInterfaceData,
        ) -> Option<String> {
            let mut required_size: u32 = 0;
            unsafe {
                let _ = SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    iface_data,
                    ptr::null_mut(),
                    0,
                    &mut required_size,
                    ptr::null_mut(),
                );
            }
            let min_size = size_of::<u32>() as u32;
            if required_size <= min_size {
                return None;
            }

            let required_bytes = required_size as usize;
            let mut buf = vec![0u32; required_bytes.div_ceil(4)];
            buf[0] = SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE;

            let ok = unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    iface_data,
                    buf.as_mut_ptr().cast::<c_void>(),
                    required_size,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return None;
            }

            let byte_len = required_bytes.saturating_sub(4);
            let mut wide = Vec::with_capacity(byte_len / 2);
            let mut remaining = byte_len;
            'decode: for &word in buf.get(1..).unwrap_or(&[]) {
                let bytes = word.to_ne_bytes();
                for pair in bytes.chunks_exact(2) {
                    if remaining < 2 {
                        break 'decode;
                    }
                    remaining -= 2;
                    let code_unit = u16::from_ne_bytes([pair[0], pair[1]]);
                    if code_unit == 0 {
                        break 'decode;
                    }
                    wide.push(code_unit);
                }
            }
            if wide.is_empty() {
                None
            } else {
                Some(String::from_utf16_lossy(&wide))
            }
        }

        fn query_device_record(path: String) -> Option<WinHidDeviceRecord> {
            // Opening with desired_access = 0 permits metadata queries even on exclusive devices.
            let guard = WinHandleGuard::open(&path, 0).ok()?;
            let mut attrs = HiddAttributes {
                size: size_of::<HiddAttributes>() as u32,
                vendor_id: 0,
                product_id: 0,
                version_number: 0,
            };
            let attr_ok = unsafe { HidD_GetAttributes(guard.as_raw(), &mut attrs) };
            if attr_ok == 0 {
                return None;
            }

            let preparsed = PreparsedDataGuard::acquire(guard.as_raw())?;
            let mut caps = HidpCaps::default();
            let caps_status = unsafe { HidP_GetCaps(preparsed.as_raw(), &mut caps) };
            if caps_status != HIDP_STATUS_SUCCESS {
                return None;
            }

            Some(WinHidDeviceRecord {
                path,
                vendor_id: attrs.vendor_id,
                product_id: attrs.product_id,
                usage_page: caps.usage_page,
                usage: caps.usage,
                feature_report_byte_length: usize::from(caps.feature_report_byte_length),
            })
        }
    }

    impl WinHidSysApi for LiveWinHidSysApi {
        fn enumerate_hid_devices(&self) -> Result<Vec<WinHidDeviceRecord>, LightingError> {
            let mut hid_guid = HidGuid::default();
            unsafe {
                HidD_GetHidGuid(&mut hid_guid);
            }
            let dev_info = DeviceInfoSetGuard::for_hid_class(&hid_guid)?;

            let mut records = Vec::new();
            let mut index: u32 = 0;
            loop {
                let mut iface_data = SpDeviceInterfaceData {
                    cb_size: size_of::<SpDeviceInterfaceData>() as u32,
                    interface_class_guid: HidGuid::default(),
                    flags: 0,
                    reserved: 0,
                };
                let ok = unsafe {
                    SetupDiEnumDeviceInterfaces(
                        dev_info.as_raw(),
                        ptr::null(),
                        &hid_guid,
                        index,
                        &mut iface_data,
                    )
                };
                if ok == 0 {
                    break;
                }
                index = index.saturating_add(1);

                if let Some(path) = Self::read_interface_path(dev_info.as_raw(), &iface_data) {
                    if let Some(rec) = Self::query_device_record(path) {
                        records.push(rec);
                    }
                }
            }

            Ok(records)
        }

        fn set_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
            let len = u32::try_from(report.len()).map_err(|_| {
                LightingError::InvalidConfig("HID feature report length exceeds u32::MAX".into())
            })?;
            let handle = WinHandleGuard::open(path, GENERIC_READ | GENERIC_WRITE)?;
            let ok =
                unsafe { HidD_SetFeature(handle.as_raw(), report.as_ptr() as *const c_void, len) };
            if ok == 0 {
                return Err(LightingError::WriteFailed(format!(
                    "HidD_SetFeature failed on {path}: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }

        fn get_feature_report(&self, path: &str, buf: &mut [u8]) -> Result<(), LightingError> {
            let len = u32::try_from(buf.len()).map_err(|_| {
                LightingError::InvalidConfig("HID feature report length exceeds u32::MAX".into())
            })?;
            let handle = WinHandleGuard::open(path, GENERIC_READ | GENERIC_WRITE)?;
            let ok =
                unsafe { HidD_GetFeature(handle.as_raw(), buf.as_mut_ptr() as *mut c_void, len) };
            if ok == 0 {
                return Err(LightingError::WriteFailed(format!(
                    "HidD_GetFeature failed on {path}: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use win_hid_ffi::LiveWinHidSysApi;
