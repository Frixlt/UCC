// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux `/dev/hidraw*` USB HID feature-report transport (`M14` / `ADR-05`).
//!
//! Enumerates `/sys/class/hidraw/hidraw*` (or a test-supplied sysfs root), parses `HID_ID` from
//! `device/uevent`, resolves USB `bInterfaceNumber` and HID report descriptor usage pages, and
//! sends/receives HID feature reports via Linux `HIDIOCSFEATURE` / `HIDIOCGFEATURE` ioctls with
//! `O_RDWR | O_NOFOLLOW | O_CLOEXEC`.

use crate::hid_transport::{read_sysfs_hidraw_interface_number, HidDeviceInfo, HidDeviceTransport};
use crate::transport::LightingError;
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};

/// Default Linux sysfs root for `hidraw` class devices.
pub const DEFAULT_SYSFS_HIDRAW_ROOT: &str = "/sys/class/hidraw";
/// Default Linux device node directory for `hidraw*` nodes.
pub const DEFAULT_DEV_ROOT: &str = "/dev";

/// Maximum HID feature report payload length representable by Linux 14-bit `_IOC_SIZEBITS`.
pub const HIDRAW_MAX_FEATURE_REPORT_LEN: usize = 0x3FFF;

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
const IOC_RDWR: u32 = IOC_WRITE | IOC_READ;
const HIDRAW_IOC_MAGIC: u32 = b'H' as u32;
const HIDRAW_NR_SFEATURE: u32 = 0x06;
const HIDRAW_NR_GFEATURE: u32 = 0x07;

/// Computes Linux `HIDIOCSFEATURE(len)` (`_IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, len)`).
pub const fn hidiocsfeature_request(len: usize) -> Option<u32> {
    if len == 0 || len > HIDRAW_MAX_FEATURE_REPORT_LEN {
        return None;
    }
    Some((IOC_RDWR << 30) | ((len as u32) << 16) | (HIDRAW_IOC_MAGIC << 8) | HIDRAW_NR_SFEATURE)
}

/// Computes Linux `HIDIOCGFEATURE(len)` (`_IOC(_IOC_WRITE|_IOC_READ, 'H', 0x07, len)`).
pub const fn hidiocgfeature_request(len: usize) -> Option<u32> {
    if len == 0 || len > HIDRAW_MAX_FEATURE_REPORT_LEN {
        return None;
    }
    Some((IOC_RDWR << 30) | ((len as u32) << 16) | (HIDRAW_IOC_MAGIC << 8) | HIDRAW_NR_GFEATURE)
}

/// Parses `(vendor_id, product_id)` from a Linux HID sysfs `device/uevent` file.
///
/// Expects a line of the form `HID_ID=BBBB:VVVVVVVV:PPPPPPPP` (hexadecimal bus, VID, PID).
pub fn parse_hid_id_from_uevent(uevent: &str) -> Option<(u16, u16)> {
    for line in uevent.lines() {
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix("HID_ID=") else {
            continue;
        };
        let mut parts = value.split(':');
        let bus_hex = parts.next()?;
        let vid_hex = parts.next()?;
        let pid_hex = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let _bus = u32::from_str_radix(bus_hex, 16).ok()?;
        let vid = u32::from_str_radix(vid_hex, 16).ok()?;
        let pid = u32::from_str_radix(pid_hex, 16).ok()?;
        let vid_u16 = u16::try_from(vid).ok()?;
        let pid_u16 = u16::try_from(pid).ok()?;
        return Some((vid_u16, pid_u16));
    }
    None
}

/// Parses the primary `Usage Page` global item from a raw USB HID report descriptor.
///
/// Follows USB HID 1.11 item encoding: Global `Usage Page` has prefix `0x05` (1-byte),
/// `0x06` (2-byte little-endian), or `0x07` (4-byte little-endian).
pub fn parse_hid_report_descriptor_usage_page(descriptor: &[u8]) -> Option<u16> {
    let mut idx = 0usize;
    while idx < descriptor.len() {
        let prefix = descriptor[idx];
        if prefix == 0xFE {
            // Long item: 0xFE, bDataSize, bLongItemTag, data[bDataSize]
            let data_len = usize::from(*descriptor.get(idx + 1)?);
            idx = idx.checked_add(3)?.checked_add(data_len)?;
            continue;
        }

        let size_code = prefix & 0x03;
        let data_len = if size_code == 3 {
            4usize
        } else {
            usize::from(size_code)
        };
        let next_idx = idx.checked_add(1)?.checked_add(data_len)?;
        if next_idx > descriptor.len() {
            break;
        }

        // Global item (bType = 1 => bits 3..2 == 0b01) with Usage Page tag (bTag = 0)
        if (prefix & 0xFC) == 0x04 {
            return match data_len {
                1 => Some(u16::from(descriptor[idx + 1])),
                2 => Some(u16::from_le_bytes([
                    descriptor[idx + 1],
                    descriptor[idx + 2],
                ])),
                4 => {
                    let raw = u32::from_le_bytes([
                        descriptor[idx + 1],
                        descriptor[idx + 2],
                        descriptor[idx + 3],
                        descriptor[idx + 4],
                    ]);
                    u16::try_from(raw).ok()
                }
                _ => None,
            };
        }

        idx = next_idx;
    }
    None
}

/// Reads and parses the HID report descriptor usage page from sysfs
/// (`{sysfs_hidraw_root}/{hidraw_name}/device/report_descriptor`).
pub fn read_sysfs_hidraw_usage_page(sysfs_hidraw_root: &Path, hidraw_name: &str) -> Option<u16> {
    if !is_valid_hidraw_name(hidraw_name) {
        return None;
    }
    let desc_path = sysfs_hidraw_root
        .join(hidraw_name)
        .join("device")
        .join("report_descriptor");
    let bytes = fs::read(desc_path).ok()?;
    parse_hid_report_descriptor_usage_page(&bytes)
}

pub(crate) fn is_valid_hidraw_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("hidraw") else {
        return false;
    };
    !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
}

fn hidraw_numeric_index(name: &str) -> u32 {
    name.strip_prefix("hidraw")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(u32::MAX)
}

/// Real Linux `/dev/hidraw*` implementation of [`HidDeviceTransport`].
#[derive(Debug, Clone)]
pub struct HidrawDeviceTransport {
    sysfs_hidraw_root: PathBuf,
    dev_root: PathBuf,
    #[cfg(target_os = "linux")]
    open_files: Arc<Mutex<BTreeMap<String, Arc<Mutex<Option<fs::File>>>>>>,
}

impl Default for HidrawDeviceTransport {
    fn default() -> Self {
        Self {
            sysfs_hidraw_root: PathBuf::from(DEFAULT_SYSFS_HIDRAW_ROOT),
            dev_root: PathBuf::from(DEFAULT_DEV_ROOT),
            #[cfg(target_os = "linux")]
            open_files: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl HidrawDeviceTransport {
    /// Creates a transport bound to Linux `/sys/class/hidraw` and `/dev`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a transport with a custom sysfs `hidraw` root (for deterministic tests).
    pub fn with_sysfs_root(sysfs_hidraw_root: impl Into<PathBuf>) -> Self {
        Self {
            sysfs_hidraw_root: sysfs_hidraw_root.into(),
            dev_root: PathBuf::from(DEFAULT_DEV_ROOT),
            #[cfg(target_os = "linux")]
            open_files: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Creates a transport with custom sysfs `hidraw` and `/dev` roots.
    pub fn with_roots(sysfs_hidraw_root: impl Into<PathBuf>, dev_root: impl Into<PathBuf>) -> Self {
        Self {
            sysfs_hidraw_root: sysfs_hidraw_root.into(),
            dev_root: dev_root.into(),
            #[cfg(target_os = "linux")]
            open_files: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn sysfs_hidraw_root(&self) -> &Path {
        &self.sysfs_hidraw_root
    }

    pub fn dev_root(&self) -> &Path {
        &self.dev_root
    }

    fn hidraw_name_from_path<'a>(&self, path: &'a str) -> Option<&'a str> {
        let p = Path::new(path);
        if let Ok(rel) = p.strip_prefix(&self.dev_root) {
            if let Some(name) = rel.to_str() {
                if is_valid_hidraw_name(name) {
                    return Some(name);
                }
            }
        }
        if self.dev_root == Path::new(DEFAULT_DEV_ROOT) {
            if let Some(name) = path.strip_prefix("/dev/") {
                if is_valid_hidraw_name(name) {
                    return Some(name);
                }
            }
        }
        None
    }

    #[cfg(target_os = "linux")]
    fn open_hidraw_file(&self, path: &str) -> Result<fs::File, LightingError> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

        let name = self.hidraw_name_from_path(path).ok_or_else(|| {
            LightingError::InvalidConfig(format!("Invalid hidraw device path: {path}"))
        })?;
        let target = self.dev_root.join(name);

        let mut opts = OpenOptions::new();
        opts.read(true)
            .write(true)
            .custom_flags(libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = opts.open(&target).map_err(LightingError::Io)?;
        let meta = file.metadata().map_err(LightingError::Io)?;
        if !meta.file_type().is_char_device() {
            return Err(LightingError::InvalidConfig(format!(
                "Refusing ioctl on non-character device: {}",
                target.display()
            )));
        }
        Ok(file)
    }

    #[cfg(target_os = "linux")]
    fn evict_open_file_slot(&self, path: &str, slot: &Arc<Mutex<Option<fs::File>>>) {
        let mut map = self.open_files.lock().unwrap_or_else(|e| e.into_inner());
        if map
            .get(path)
            .is_some_and(|cur| Arc::ptr_eq(cur, slot) && cur.try_lock().is_ok_and(|g| g.is_none()))
        {
            map.remove(path);
        }
    }

    #[cfg(target_os = "linux")]
    fn with_hidraw_file<T, F>(&self, path: &str, mut op: F) -> Result<T, LightingError>
    where
        F: FnMut(&mut fs::File) -> Result<T, LightingError>,
    {
        const MAX_OPEN_HIDRAW_FILES: usize = 16;

        let slot = {
            let mut map = self.open_files.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = map.get(path) {
                Arc::clone(existing)
            } else {
                if map.len() >= MAX_OPEN_HIDRAW_FILES {
                    let empty_keys: Vec<String> = map
                        .iter()
                        .filter(|(_, v)| {
                            Arc::strong_count(v) == 1 && v.try_lock().is_ok_and(|g| g.is_none())
                        })
                        .map(|(k, _)| k.clone())
                        .collect();
                    for k in empty_keys {
                        map.remove(&k);
                    }
                    if map.len() >= MAX_OPEN_HIDRAW_FILES {
                        if let Some(evict_key) = map
                            .iter()
                            .find(|(_, v)| Arc::strong_count(v) == 1)
                            .or_else(|| map.iter().next())
                            .map(|(k, _)| k.clone())
                        {
                            map.remove(&evict_key);
                        }
                    }
                }
                let entry = Arc::new(Mutex::new(None));
                map.insert(path.to_string(), Arc::clone(&entry));
                entry
            }
        };
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        let had_cached = guard.is_some();
        if !had_cached {
            match self.open_hidraw_file(path) {
                Ok(file) => *guard = Some(file),
                Err(err) => {
                    drop(guard);
                    self.evict_open_file_slot(path, &slot);
                    return Err(err);
                }
            }
        }
        let first_res = op(guard.as_mut().expect("cached hidraw handle"));
        if first_res.is_ok() {
            return first_res;
        }
        if !had_cached {
            *guard = None;
            drop(guard);
            self.evict_open_file_slot(path, &slot);
            return first_res;
        }
        *guard = None;
        let mut fresh = match self.open_hidraw_file(path) {
            Ok(file) => file,
            Err(err) => {
                drop(guard);
                self.evict_open_file_slot(path, &slot);
                return Err(err);
            }
        };
        let retry_res = op(&mut fresh);
        if retry_res.is_ok() {
            *guard = Some(fresh);
        } else {
            drop(guard);
            self.evict_open_file_slot(path, &slot);
        }
        retry_res
    }
}

impl HidDeviceTransport for HidrawDeviceTransport {
    fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError> {
        let entries = match fs::read_dir(&self.sysfs_hidraw_root) {
            Ok(iter) => iter,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(LightingError::Io(err)),
        };

        let mut hidraw_names: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if is_valid_hidraw_name(name) {
                    hidraw_names.push(name.to_string());
                }
            }
        }
        hidraw_names.sort_by(|a, b| {
            hidraw_numeric_index(a)
                .cmp(&hidraw_numeric_index(b))
                .then_with(|| a.cmp(b))
        });

        let mut devices = Vec::new();
        for name in hidraw_names {
            let uevent_path = self.sysfs_hidraw_root.join(&name).join("device/uevent");
            let Ok(uevent) = fs::read_to_string(&uevent_path) else {
                continue;
            };
            let Some((vendor_id, product_id)) = parse_hid_id_from_uevent(&uevent) else {
                continue;
            };
            let dev_path = self.dev_root.join(&name).to_string_lossy().into_owned();
            devices.push(HidDeviceInfo {
                vendor_id,
                product_id,
                path: dev_path,
            });
        }
        Ok(devices)
    }

    #[cfg(target_os = "linux")]
    fn send_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        use std::os::unix::io::AsRawFd;

        if report.is_empty() {
            return Err(LightingError::InvalidConfig(
                "Invalid HID feature report length: 0".to_string(),
            ));
        }
        // Linux `hidraw` `HIDIOCSFEATURE` always treats `buf[0]` as the HID Report ID.
        // Uniwill/ITE controllers (`048D:600B`, `048D:7001`, `048D:7000`) use unnumbered
        // feature reports (Report ID `0x00`, `wValue = 0x0300`), so prepend `0x00`.
        let total_len = report.len().saturating_add(1);
        let req = hidiocsfeature_request(total_len).ok_or_else(|| {
            LightingError::InvalidConfig(format!(
                "Invalid HID feature report length: {}",
                report.len()
            ))
        })?;
        let mut stack_buf = [0u8; 65];
        let mut heap_buf = if total_len > stack_buf.len() {
            vec![0u8; total_len]
        } else {
            Vec::new()
        };
        self.with_hidraw_file(path, |file| {
            // Re-initialize from `report` before each attempt because `HIDIOCSFEATURE`
            // uses `_IOC_RDWR` and the kernel may mutate the buffer on a failed attempt.
            let ptr = if total_len > stack_buf.len() {
                heap_buf[0] = 0x00;
                heap_buf[1..total_len].copy_from_slice(report);
                heap_buf.as_mut_ptr()
            } else {
                stack_buf[0] = 0x00;
                stack_buf[1..total_len].copy_from_slice(report);
                stack_buf.as_mut_ptr()
            };
            // SAFETY: `file` is a validated `/dev/hidraw*` character device descriptor,
            // `req` is `HIDIOCSFEATURE(total_len)`, and `ptr` points to `total_len` valid bytes.
            let ret = unsafe { libc::ioctl(file.as_raw_fd(), req as libc::Ioctl, ptr) };
            if ret < 0 {
                return Err(LightingError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn send_feature_report(&self, _path: &str, _report: &[u8]) -> Result<(), LightingError> {
        Err(LightingError::Unavailable)
    }

    #[cfg(target_os = "linux")]
    fn send_output_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
        use std::io::Write;

        if report.is_empty() || report.len() > 64 {
            return Err(LightingError::InvalidConfig(format!(
                "Invalid HID output report length: {}",
                report.len()
            )));
        }
        // Linux `hidraw` `write()` treats `buf[0]` as the HID Report ID (`0x00` for unnumbered
        // ITE 8291r3 64-byte per-key row output reports).
        let mut buf = [0u8; 65];
        buf[0] = 0x00;
        buf[1..=report.len()].copy_from_slice(report);
        let slice = &buf[..=report.len()];
        self.with_hidraw_file(path, |file| {
            file.write_all(slice).map_err(LightingError::Io)
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn send_output_report(&self, _path: &str, _report: &[u8]) -> Result<(), LightingError> {
        Err(LightingError::Unavailable)
    }

    #[cfg(target_os = "linux")]
    fn get_feature_report(&self, path: &str, len: usize) -> Result<Vec<u8>, LightingError> {
        use std::os::unix::io::AsRawFd;

        let req = hidiocgfeature_request(len).ok_or_else(|| {
            LightingError::InvalidConfig(format!("Invalid HID feature report length: {len}"))
        })?;
        self.with_hidraw_file(path, |file| {
            let mut buf = vec![0u8; len];
            // SAFETY: `file` is a validated `/dev/hidraw*` character device descriptor,
            // `req` is `HIDIOCGFEATURE(len)`, and `buf` is a valid mutable byte buffer.
            let ret =
                unsafe { libc::ioctl(file.as_raw_fd(), req as libc::Ioctl, buf.as_mut_ptr()) };
            if ret < 0 {
                return Err(LightingError::Io(std::io::Error::last_os_error()));
            }
            buf.truncate((ret as usize).min(len));
            Ok(buf)
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn get_feature_report(&self, _path: &str, _len: usize) -> Result<Vec<u8>, LightingError> {
        Err(LightingError::Unavailable)
    }

    fn device_usage_page(&self, path: &str) -> Option<u16> {
        let name = self.hidraw_name_from_path(path)?;
        read_sysfs_hidraw_usage_page(&self.sysfs_hidraw_root, name)
    }

    fn device_interface_number(&self, path: &str) -> Option<i32> {
        let name = self.hidraw_name_from_path(path)?;
        read_sysfs_hidraw_interface_number(&self.sysfs_hidraw_root, name)
    }

    fn uses_host_sysfs(&self) -> bool {
        self.sysfs_hidraw_root == Path::new(DEFAULT_SYSFS_HIDRAW_ROOT)
    }
}
