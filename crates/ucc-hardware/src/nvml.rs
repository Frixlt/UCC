//! NVIDIA NVML safe dynamic wrapper and runtime power management guard.
//!
//! Avoids static build-time dependencies on the proprietary CUDA/NVML SDK.
//! Enforces a runtime PM check (`runtime_status` == "active") to ensure the discrete GPU
//! is never inadvertently woken from D3cold sleep by background telemetry queries.

use libloading::Library;
use std::ffi::{c_char, c_uint, c_ulonglong, c_void, CStr};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use thiserror::Error;

pub const NVML_SUCCESS: c_uint = 0;
pub const NVML_ERROR_NOT_SUPPORTED: c_uint = 3;
pub const NVML_TEMPERATURE_GPU: c_uint = 0;

pub const NVIDIA_PCI_VENDOR_ID: &str = "0x10de";

#[derive(Debug, Error)]
pub enum NvmlError {
    #[error("Failed to load libnvidia-ml.so: {0}")]
    LibraryLoad(#[from] libloading::Error),
    #[error("NVML returned error code {0}")]
    NvmlCall(c_uint),
    #[error("GPU is suspended in D3cold; queries are guarded to save power")]
    GpuSuspended,
    #[error("No NVIDIA GPU found")]
    NoGpuFound,
    #[error("IO error reading sysfs: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid UTF-8 in GPU string: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmlUtilization {
    pub gpu: c_uint,
    pub memory: c_uint,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmlMemory {
    pub total: c_ulonglong,
    pub free: c_ulonglong,
    pub used: c_ulonglong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuRuntimeStatus {
    Active,
    Suspended,
    Unsupported,
    Unknown,
}

/// Guard to inspect sysfs runtime PM state before calling NVML
pub struct NvidiaPowerGuard {
    sysfs_pci_root: PathBuf,
}

impl Default for NvidiaPowerGuard {
    fn default() -> Self {
        Self::new("/sys/bus/pci/devices")
    }
}

impl NvidiaPowerGuard {
    pub fn new(sysfs_pci_root: impl AsRef<Path>) -> Self {
        Self {
            sysfs_pci_root: sysfs_pci_root.as_ref().to_path_buf(),
        }
    }

    /// Finds all NVIDIA PCI devices (vendor 0x10de) in the specified sysfs path
    pub fn find_nvidia_devices(&self) -> Result<Vec<PathBuf>, NvmlError> {
        let mut devices = Vec::new();
        if !self.sysfs_pci_root.exists() {
            return Ok(devices);
        }

        for entry in fs::read_dir(&self.sysfs_pci_root)? {
            let entry = entry?;
            let path = entry.path();
            let vendor_path = path.join("vendor");
            let class_path = path.join("class");
            if vendor_path.exists() {
                if let Ok(content) = fs::read_to_string(&vendor_path) {
                    if content.trim().eq_ignore_ascii_case(NVIDIA_PCI_VENDOR_ID) {
                        // Check PCI class to ensure this is a display/3D graphics device, not audio (0x0403xx)
                        let Ok(class_str) = fs::read_to_string(&class_path) else {
                            continue;
                        };
                        let c = class_str.trim().to_ascii_lowercase();
                        // Base class 0x03 indicates Display controller (VGA 0x0300, 3D 0x0302, etc.)
                        if !c.starts_with("0x03") && !c.starts_with("03") {
                            continue;
                        }
                        devices.push(path);
                    }
                }
            }
        }
        Ok(devices)
    }

    /// Checks the runtime PM status of an NVIDIA device
    pub fn get_runtime_status(&self, device_path: &Path) -> GpuRuntimeStatus {
        let status_path = device_path.join("power/runtime_status");
        if !status_path.exists() {
            return GpuRuntimeStatus::Unsupported;
        }

        match fs::read_to_string(&status_path) {
            Ok(content) => match content.trim() {
                "active" => GpuRuntimeStatus::Active,
                "suspended" => GpuRuntimeStatus::Suspended,
                _ => GpuRuntimeStatus::Unknown,
            },
            Err(_) => GpuRuntimeStatus::Unknown,
        }
    }

    /// Returns true only if at least one NVIDIA display/graphics device is active (awake).
    /// If all NVIDIA graphics devices are suspended or none exist, returns false.
    pub fn is_dgpu_awake(&self) -> bool {
        match self.find_nvidia_devices() {
            Ok(devices) => {
                if devices.is_empty() {
                    return false;
                }
                devices
                    .iter()
                    .any(|d| self.get_runtime_status(d) == GpuRuntimeStatus::Active)
            }
            Err(_) => false,
        }
    }
}

pub struct NvmlTelemetry {
    pub name: String,
    pub temperature_c: u32,
    pub power_usage_mw: u32,
    pub enforced_limit_mw: u32,
    pub utilization: NvmlUtilization,
    pub memory: NvmlMemory,
}

/// Dynamically loaded NVML client
pub struct NvmlWrapper {
    _lib: Library,
    init_ok: Arc<AtomicBool>,
    device_count: Arc<AtomicU32>,
    power_guard: NvidiaPowerGuard,

    // Function pointers
    fn_init: unsafe extern "C" fn() -> c_uint,
    fn_shutdown: unsafe extern "C" fn() -> c_uint,
    fn_device_get_count: unsafe extern "C" fn(*mut c_uint) -> c_uint,
    fn_device_get_handle_by_index: unsafe extern "C" fn(c_uint, *mut *mut c_void) -> c_uint,
    fn_device_get_name: unsafe extern "C" fn(*mut c_void, *mut c_char, c_uint) -> c_uint,
    fn_device_get_temperature: unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_uint,
    fn_device_get_power_usage: unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint,
    fn_device_get_enforced_power_limit: unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint,
    fn_device_get_power_management_limit_constraints:
        unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_uint,
    fn_device_set_power_management_limit: unsafe extern "C" fn(*mut c_void, c_uint) -> c_uint,
    fn_device_get_utilization_rates:
        unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> c_uint,
    fn_device_get_memory_info: unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> c_uint,
}

impl NvmlWrapper {
    /// Attempts to load `libnvidia-ml.so.1` or `libnvidia-ml.so`
    pub fn new(power_guard: NvidiaPowerGuard) -> Result<Self, NvmlError> {
        let lib = unsafe {
            Library::new("libnvidia-ml.so.1").or_else(|_| Library::new("libnvidia-ml.so"))?
        };

        unsafe {
            let fn_init: unsafe extern "C" fn() -> c_uint = *lib.get(b"nvmlInit_v2")?;
            let fn_shutdown: unsafe extern "C" fn() -> c_uint = *lib.get(b"nvmlShutdown")?;
            let fn_device_get_count: unsafe extern "C" fn(*mut c_uint) -> c_uint =
                *lib.get(b"nvmlDeviceGetCount_v2")?;
            let fn_device_get_handle_by_index: unsafe extern "C" fn(
                c_uint,
                *mut *mut c_void,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetHandleByIndex_v2")?;
            let fn_device_get_name: unsafe extern "C" fn(
                *mut c_void,
                *mut c_char,
                c_uint,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetName")?;
            let fn_device_get_temperature: unsafe extern "C" fn(
                *mut c_void,
                c_uint,
                *mut c_uint,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetTemperature")?;
            let fn_device_get_power_usage: unsafe extern "C" fn(
                *mut c_void,
                *mut c_uint,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetPowerUsage")?;
            let fn_device_get_enforced_power_limit: unsafe extern "C" fn(
                *mut c_void,
                *mut c_uint,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetEnforcedPowerLimit")?;
            let fn_device_get_power_management_limit_constraints: unsafe extern "C" fn(
                *mut c_void,
                *mut c_uint,
                *mut c_uint,
            )
                -> c_uint = *lib.get(b"nvmlDeviceGetPowerManagementLimitConstraints")?;
            let fn_device_set_power_management_limit: unsafe extern "C" fn(
                *mut c_void,
                c_uint,
            ) -> c_uint = *lib.get(b"nvmlDeviceSetPowerManagementLimit")?;
            let fn_device_get_utilization_rates: unsafe extern "C" fn(
                *mut c_void,
                *mut NvmlUtilization,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetUtilizationRates")?;
            let fn_device_get_memory_info: unsafe extern "C" fn(
                *mut c_void,
                *mut NvmlMemory,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetMemoryInfo")?;

            let init_ok = Arc::new(AtomicBool::new(false));
            let device_count = Arc::new(AtomicU32::new(0));

            // Strict D3cold guard: do NOT invoke native nvmlInit during construction
            // if the discrete GPU is suspended, avoiding waking the hardware.
            if power_guard.is_dgpu_awake() {
                let ret = fn_init();
                if ret == NVML_SUCCESS {
                    init_ok.store(true, Ordering::SeqCst);
                    let mut count: c_uint = 0;
                    if fn_device_get_count(&mut count) == NVML_SUCCESS {
                        device_count.store(count, Ordering::SeqCst);
                    }
                }
            }

            Ok(Self {
                _lib: lib,
                init_ok,
                device_count,
                power_guard,
                fn_init,
                fn_shutdown,
                fn_device_get_count,
                fn_device_get_handle_by_index,
                fn_device_get_name,
                fn_device_get_temperature,
                fn_device_get_power_usage,
                fn_device_get_enforced_power_limit,
                fn_device_get_power_management_limit_constraints,
                fn_device_set_power_management_limit,
                fn_device_get_utilization_rates,
                fn_device_get_memory_info,
            })
        }
    }

    pub fn device_count(&self) -> u32 {
        self.device_count.load(Ordering::SeqCst)
    }

    fn ensure_active_and_initialized(&self) -> Result<(), NvmlError> {
        if !self.power_guard.is_dgpu_awake() {
            return Err(NvmlError::GpuSuspended);
        }
        if !self.init_ok.load(Ordering::SeqCst) {
            unsafe {
                let ret = (self.fn_init)();
                if ret != NVML_SUCCESS {
                    return Err(NvmlError::NvmlCall(ret));
                }
                self.init_ok.store(true, Ordering::SeqCst);
                let mut count: c_uint = 0;
                if (self.fn_device_get_count)(&mut count) == NVML_SUCCESS {
                    self.device_count.store(count, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }

    /// Query minimum and maximum power limits in milliwatts
    pub fn get_power_limit_constraints(&self, index: u32) -> Result<(u32, u32), NvmlError> {
        self.ensure_active_and_initialized()?;
        if index >= self.device_count.load(Ordering::SeqCst) {
            return Err(NvmlError::NoGpuFound);
        }

        unsafe {
            let mut handle: *mut c_void = std::ptr::null_mut();
            let ret = (self.fn_device_get_handle_by_index)(index, &mut handle);
            if ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(ret));
            }

            let mut min_mw: c_uint = 0;
            let mut max_mw: c_uint = 0;
            let ret = (self.fn_device_get_power_management_limit_constraints)(
                handle,
                &mut min_mw,
                &mut max_mw,
            );
            if ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(ret));
            }
            Ok((min_mw, max_mw))
        }
    }

    /// Read telemetry for a GPU index, strictly respecting the power guard
    pub fn get_telemetry(&self, index: u32) -> Result<NvmlTelemetry, NvmlError> {
        // GUARD: check if dGPU is suspended in D3cold before waking it!
        self.ensure_active_and_initialized()?;
        if index >= self.device_count.load(Ordering::SeqCst) {
            return Err(NvmlError::NoGpuFound);
        }

        unsafe {
            let mut handle: *mut c_void = std::ptr::null_mut();
            let ret = (self.fn_device_get_handle_by_index)(index, &mut handle);
            if ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(ret));
            }

            // Name
            let mut name_buf = [0 as c_char; 64];
            let _ = (self.fn_device_get_name)(handle, name_buf.as_mut_ptr(), 64);
            let name = CStr::from_ptr(name_buf.as_ptr())
                .to_str()
                .unwrap_or("NVIDIA GPU")
                .to_string();

            // Temperature
            let mut temp: c_uint = 0;
            let _ = (self.fn_device_get_temperature)(handle, NVML_TEMPERATURE_GPU, &mut temp);

            // Power
            let mut power_mw: c_uint = 0;
            let _ = (self.fn_device_get_power_usage)(handle, &mut power_mw);

            // Power limit
            let mut limit_mw: c_uint = 0;
            let _ = (self.fn_device_get_enforced_power_limit)(handle, &mut limit_mw);

            // Utilization
            let mut util = NvmlUtilization::default();
            let _ = (self.fn_device_get_utilization_rates)(handle, &mut util);

            // Memory
            let mut mem = NvmlMemory::default();
            let _ = (self.fn_device_get_memory_info)(handle, &mut mem);

            Ok(NvmlTelemetry {
                name,
                temperature_c: temp,
                power_usage_mw: power_mw,
                enforced_limit_mw: limit_mw,
                utilization: util,
                memory: mem,
            })
        }
    }

    /// Set power limit in milliwatts
    pub fn set_power_limit(&self, index: u32, power_mw: u32) -> Result<(), NvmlError> {
        self.ensure_active_and_initialized()?;
        if index >= self.device_count.load(Ordering::SeqCst) {
            return Err(NvmlError::NoGpuFound);
        }

        unsafe {
            let mut handle: *mut c_void = std::ptr::null_mut();
            let ret = (self.fn_device_get_handle_by_index)(index, &mut handle);
            if ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(ret));
            }

            let ret = (self.fn_device_set_power_management_limit)(handle, power_mw);
            if ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(ret));
            }
            Ok(())
        }
    }
}

impl Drop for NvmlWrapper {
    fn drop(&mut self) {
        if self.init_ok.swap(false, Ordering::SeqCst) {
            unsafe {
                (self.fn_shutdown)();
            }
        }
    }
}
