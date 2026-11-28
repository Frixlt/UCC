// SPDX-License-Identifier: GPL-3.0-or-later
//! Combined USB RGB+IR UVC webcam toggle over [`Sysfs`].
//!
//! Uniwill chassis (such as `0x1A`) do not expose a Clevo-style `W_CL_WEBCAM_SW` EC switch.
//! Instead, this module safely discovers USB Video Class (`bInterfaceClass == "0e"`) devices
//! under `/sys/bus/usb/devices/*` and toggles their `authorized` sysfs attribute without ever
//! touching non-camera USB devices (hubs, HID keyboards/mice, wireless adapters, storage).

use crate::platform::PlatformError;
use crate::traits::Sysfs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Linux sysfs root directory for USB devices and interfaces.
pub const USB_DEVICES_ROOT: &str = "/sys/bus/usb/devices";

/// USB Video Class (`UVC`) interface class hex code (`0x0E`).
pub const USB_CLASS_VIDEO: &str = "0e";

/// Default TTL for caching `/sys/bus/usb/devices` webcam scan results.
pub const DEFAULT_WEBCAM_CACHE_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
struct WebcamCache {
    devices: Vec<UsbWebcamDevice>,
    last_scan: Option<Instant>,
}

/// Discovered USB UVC webcam device node in sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbWebcamDevice {
    pub device_path: PathBuf,
    pub vendor_id: String,
    pub product_id: String,
    pub product_name: Option<String>,
}

/// Sysfs-backed controller for toggling combined USB RGB+IR UVC webcams via `authorized`.
pub struct UsbWebcamController {
    sysfs: Arc<dyn Sysfs>,
    usb_root: PathBuf,
    vid_pid_filter: Option<(String, String)>,
    cache_ttl: Duration,
    cached_devices: Mutex<WebcamCache>,
}

impl UsbWebcamController {
    /// Creates a webcam controller scanning `/sys/bus/usb/devices` via `sysfs`.
    pub fn new(sysfs: Arc<dyn Sysfs>) -> Self {
        Self {
            sysfs,
            usb_root: PathBuf::from(USB_DEVICES_ROOT),
            vid_pid_filter: None,
            cache_ttl: DEFAULT_WEBCAM_CACHE_TTL,
            cached_devices: Mutex::new(WebcamCache::default()),
        }
    }

    /// Overrides the scan cache TTL (defaults to 5 seconds).
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Invalidates the scan TTL timestamp so the next discovery rescans sysfs
    /// while preserving previously discovered deauthorized parent devices.
    pub fn invalidate_cache(&self) {
        let mut cache = self
            .cached_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cache.last_scan = None;
    }

    /// Restricts toggling to a specific USB `(idVendor, idProduct)` pair (case-insensitive hex).
    pub fn with_vid_pid_filter(
        mut self,
        vendor_id: impl Into<String>,
        product_id: impl Into<String>,
    ) -> Self {
        self.vid_pid_filter = Some((
            vendor_id.into().trim().to_ascii_lowercase(),
            product_id.into().trim().to_ascii_lowercase(),
        ));
        self
    }

    /// Discovers all parent USB devices under `/sys/bus/usb/devices` that expose at least one
    /// UVC video interface (`bInterfaceClass == "0e"`) and an `authorized` control attribute.
    pub fn discover_webcams(&self) -> Result<Vec<UsbWebcamDevice>, PlatformError> {
        {
            let cache = self
                .cached_devices
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(last_scan) = cache.last_scan {
                if last_scan.elapsed() < self.cache_ttl {
                    return Ok(cache.devices.clone());
                }
            }
        }

        let entries = self.sysfs.read_dir(&self.usb_root).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                PlatformError::Unsupported("USB sysfs root unavailable for webcam control")
            } else {
                tracing::warn!(
                    path = %self.usb_root.display(),
                    error = %source,
                    "Failed to enumerate USB sysfs root for webcam control"
                );
                PlatformError::Io {
                    context: "Reading USB sysfs root directory failed",
                    source,
                }
            }
        })?;

        let mut parent_devices: Vec<PathBuf> = Vec::new();
        let mut uvc_parent_names: HashSet<String> = HashSet::new();

        for entry in &entries {
            let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("usb") {
                // Skip root hubs (`usb1`, `usb2`, ...).
                continue;
            }
            if let Some((parent_name, _iface)) = name.split_once(':') {
                if uvc_parent_names.contains(parent_name) {
                    continue;
                }
                let class_path = entry.join("bInterfaceClass");
                if let Ok(class_raw) = self.sysfs.read(&class_path) {
                    if class_raw.trim().eq_ignore_ascii_case(USB_CLASS_VIDEO) {
                        uvc_parent_names.insert(parent_name.to_string());
                    }
                }
            } else {
                parent_devices.push(entry.clone());
            }
        }

        parent_devices.sort();

        let mut webcams = Vec::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        for dev_path in parent_devices {
            let Some(dev_name) = dev_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !uvc_parent_names.contains(dev_name) {
                continue;
            }
            if !self.sysfs.exists(&dev_path.join("authorized")) {
                continue;
            }

            let vendor_id = self
                .sysfs
                .read(&dev_path.join("idVendor"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let product_id = self
                .sysfs
                .read(&dev_path.join("idProduct"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();

            if let Some((ref expected_vid, ref expected_pid)) = self.vid_pid_filter {
                if &vendor_id != expected_vid || &product_id != expected_pid {
                    continue;
                }
            }

            let product_name = self
                .sysfs
                .read(&dev_path.join("product"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            seen_paths.insert(dev_path.clone());
            webcams.push(UsbWebcamDevice {
                device_path: dev_path,
                vendor_id,
                product_id,
                product_name,
            });
        }

        // Retain previously discovered UVC parent devices while deauthorized (`authorized == "0"`),
        // because Linux `usb_deauthorize_device` unregisters `<parent>:<iface>` child entries from
        // sysfs while keeping `<parent>/authorized`, `idVendor`, and `idProduct`.
        let mut cache = self
            .cached_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for cached in &cache.devices {
            if seen_paths.contains(&cached.device_path) {
                continue;
            }
            if !self.sysfs.exists(&cached.device_path.join("authorized")) {
                continue;
            }
            let current_vid = self
                .sysfs
                .read(&cached.device_path.join("idVendor"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let current_pid = self
                .sysfs
                .read(&cached.device_path.join("idProduct"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if current_vid != cached.vendor_id || current_pid != cached.product_id {
                continue;
            }
            if let Some((ref expected_vid, ref expected_pid)) = self.vid_pid_filter {
                if &current_vid != expected_vid || &current_pid != expected_pid {
                    continue;
                }
            }
            seen_paths.insert(cached.device_path.clone());
            webcams.push(cached.clone());
        }
        webcams.sort_by(|a, b| a.device_path.cmp(&b.device_path));
        cache.devices = webcams.clone();
        cache.last_scan = Some(Instant::now());

        Ok(webcams)
    }

    /// Reads whether any discovered USB UVC webcam is currently authorized (`"1"`).
    pub fn is_enabled(&self) -> Result<bool, PlatformError> {
        let webcams = self.discover_webcams()?;
        if webcams.is_empty() {
            return Err(PlatformError::Unsupported("No USB UVC webcam detected"));
        }
        let mut any_read_ok = false;
        let mut last_err: Option<std::io::Error> = None;
        for cam in &webcams {
            let auth_path = cam.device_path.join("authorized");
            match self.sysfs.read(&auth_path) {
                Ok(raw) => {
                    any_read_ok = true;
                    if raw.trim() == "1" {
                        return Ok(true);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        path = %auth_path.display(),
                        error = %err,
                        "Failed to read USB webcam authorized state"
                    );
                    last_err = Some(err);
                }
            }
        }
        if !any_read_ok {
            if let Some(source) = last_err {
                return Err(PlatformError::Io {
                    context: "Reading USB webcam authorized sysfs attribute failed",
                    source,
                });
            }
        }
        Ok(false)
    }

    /// Sets `authorized` (`"1"` or `"0"`) on all discovered USB UVC webcam parent devices
    /// (covering combined RGB + IR composite webcams). Never touches non-UVC USB devices.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        let webcams = self.discover_webcams()?;
        if webcams.is_empty() {
            return Err(PlatformError::Unsupported("No USB UVC webcam detected"));
        }
        let val = if enabled { "1" } else { "0" };
        for cam in &webcams {
            let auth_path = cam.device_path.join("authorized");
            self.sysfs.write(&auth_path, val).map_err(|source| {
                tracing::warn!(
                    path = %auth_path.display(),
                    value = val,
                    error = %source,
                    "Failed to write USB webcam authorized state"
                );
                PlatformError::Io {
                    context: "Writing USB webcam authorized sysfs attribute failed",
                    source,
                }
            })?;
        }
        Ok(())
    }

    /// Returns the configured USB sysfs root path.
    pub fn usb_root(&self) -> &Path {
        &self.usb_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::MockSysfs;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FaultyAuthSysfs {
        inner: MockSysfs,
        fail_auth_reads: AtomicBool,
        fail_auth_writes: AtomicBool,
    }

    impl Sysfs for FaultyAuthSysfs {
        fn read(&self, path: &Path) -> std::io::Result<String> {
            if self.fail_auth_reads.load(Ordering::SeqCst)
                && path.file_name().and_then(|n| n.to_str()) == Some("authorized")
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated sysfs read EACCES",
                ));
            }
            self.inner.read(path)
        }

        fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
            if self.fail_auth_writes.load(Ordering::SeqCst)
                && path.file_name().and_then(|n| n.to_str()) == Some("authorized")
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated sysfs write EACCES",
                ));
            }
            self.inner.write(path, content)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
            self.inner.read_dir(path)
        }
    }

    #[test]
    fn is_enabled_and_set_enabled_preserve_io_error_causes() {
        let inner = MockSysfs::new();
        inner.set_file("/sys/bus/usb/devices/1-6/authorized", "1");
        inner.set_file("/sys/bus/usb/devices/1-6/idVendor", "04f2");
        inner.set_file("/sys/bus/usb/devices/1-6/idProduct", "b751");
        inner.set_file("/sys/bus/usb/devices/1-6:1.0/bInterfaceClass", "0e");

        let sysfs = Arc::new(FaultyAuthSysfs {
            inner,
            fail_auth_reads: AtomicBool::new(true),
            fail_auth_writes: AtomicBool::new(true),
        });
        let controller = UsbWebcamController::new(Arc::clone(&sysfs) as _);

        // When all sysfs.read calls fail, is_enabled() MUST NOT return Ok(false)
        let read_err = controller
            .is_enabled()
            .expect_err("all-failed authorized reads must return Err, not Ok(false)");
        assert!(
            matches!(read_err, PlatformError::Io { ref source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied),
            "expected PlatformError::Io(PermissionDenied), got {read_err:?}"
        );

        // set_enabled() must preserve underlying I/O error instead of collapsing to Unavailable
        let write_err = controller
            .set_enabled(false)
            .expect_err("authorized write failure must return Err");
        assert!(
            matches!(write_err, PlatformError::Io { ref source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied),
            "expected PlatformError::Io(PermissionDenied), got {write_err:?}"
        );
    }
}
