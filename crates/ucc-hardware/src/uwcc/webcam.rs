// SPDX-License-Identifier: GPL-3.0-or-later
//! Combined USB RGB+IR UVC webcam toggle over [`Sysfs`].
//!
//! Uniwill chassis (such as `0x1A`) do not expose a Clevo-style `W_CL_WEBCAM_SW` EC switch.
//! Instead, this module safely discovers USB Video Class (`bInterfaceClass == "0e"`) devices
//! under `/sys/bus/usb/devices/*` and toggles their `authorized` sysfs attribute without ever
//! touching non-camera USB devices (hubs, HID keyboards/mice, wireless adapters, storage).

use crate::platform::PlatformError;
use crate::traits::Sysfs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Linux sysfs root directory for USB devices and interfaces.
pub const USB_DEVICES_ROOT: &str = "/sys/bus/usb/devices";

/// USB Video Class (`UVC`) interface class hex code (`0x0E`).
pub const USB_CLASS_VIDEO: &str = "0e";

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
}

impl UsbWebcamController {
    /// Creates a webcam controller scanning `/sys/bus/usb/devices` via `sysfs`.
    pub fn new(sysfs: Arc<dyn Sysfs>) -> Self {
        Self {
            sysfs,
            usb_root: PathBuf::from(USB_DEVICES_ROOT),
            vid_pid_filter: None,
        }
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
        let entries = self.sysfs.read_dir(&self.usb_root).map_err(|_| {
            PlatformError::Unsupported("USB sysfs root unavailable for webcam control")
        })?;

        let mut parent_devices: Vec<PathBuf> = Vec::new();
        let mut uvc_parent_names: Vec<String> = Vec::new();

        for entry in &entries {
            let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("usb") {
                // Skip root hubs (`usb1`, `usb2`, ...).
                continue;
            }
            if let Some((parent_name, _iface)) = name.split_once(':') {
                let class_path = entry.join("bInterfaceClass");
                if let Ok(class_raw) = self.sysfs.read(&class_path) {
                    if class_raw.trim().eq_ignore_ascii_case(USB_CLASS_VIDEO)
                        && !uvc_parent_names.iter().any(|p| p == parent_name)
                    {
                        uvc_parent_names.push(parent_name.to_string());
                    }
                }
            } else {
                parent_devices.push(entry.clone());
            }
        }

        parent_devices.sort();

        let mut webcams = Vec::new();
        for dev_path in parent_devices {
            let Some(dev_name) = dev_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !uvc_parent_names.iter().any(|p| p == dev_name) {
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

            webcams.push(UsbWebcamDevice {
                device_path: dev_path,
                vendor_id,
                product_id,
                product_name,
            });
        }

        Ok(webcams)
    }

    /// Reads whether any discovered USB UVC webcam is currently authorized (`"1"`).
    pub fn is_enabled(&self) -> Result<bool, PlatformError> {
        let webcams = self.discover_webcams()?;
        if webcams.is_empty() {
            return Err(PlatformError::Unsupported("No USB UVC webcam detected"));
        }
        for cam in &webcams {
            let auth_path = cam.device_path.join("authorized");
            if let Ok(raw) = self.sysfs.read(&auth_path) {
                if raw.trim() == "1" {
                    return Ok(true);
                }
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
            self.sysfs
                .write(&auth_path, val)
                .map_err(|_| PlatformError::Unavailable)?;
        }
        Ok(())
    }

    /// Returns the configured USB sysfs root path.
    pub fn usb_root(&self) -> &Path {
        &self.usb_root
    }
}
