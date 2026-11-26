//! NVIDIA NVML safe dynamic wrapper and runtime power management guard.
//!
//! Avoids static build-time dependencies on the proprietary CUDA/NVML SDK.
//! Enforces a runtime PM check (`runtime_status` == "active") to ensure the discrete GPU
//! is never inadvertently woken from D3cold sleep by background telemetry queries.

use libloading::Library;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_uint, c_ulonglong, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;

pub const NVML_SUCCESS: c_uint = 0;
pub const NVML_ERROR_NOT_SUPPORTED: c_uint = 3;
pub const NVML_TEMPERATURE_GPU: c_uint = 0;
pub const NVML_CLOCK_GRAPHICS: c_uint = 0;
pub const NVML_CLOCK_MEM: c_uint = 2;
pub const NVML_PSTATE_UNKNOWN: c_uint = 32;

const NVML_CLOCKS_THROTTLE_REASON_GPU_IDLE: u64 = 0x0000_0000_0000_0001;
const NVML_CLOCKS_THROTTLE_REASON_APPLICATIONS_CLOCKS_SETTING: u64 = 0x0000_0000_0000_0002;
const NVML_CLOCKS_THROTTLE_REASON_SW_POWER_CAP: u64 = 0x0000_0000_0000_0004;
const NVML_CLOCKS_THROTTLE_REASON_HW_SLOWDOWN: u64 = 0x0000_0000_0000_0008;
const NVML_CLOCKS_THROTTLE_REASON_SYNC_BOOST: u64 = 0x0000_0000_0000_0010;
const NVML_CLOCKS_THROTTLE_REASON_SW_THERMAL_SLOWDOWN: u64 = 0x0000_0000_0000_0020;
const NVML_CLOCKS_THROTTLE_REASON_HW_THERMAL_SLOWDOWN: u64 = 0x0000_0000_0000_0040;
const NVML_CLOCKS_THROTTLE_REASON_HW_POWER_BRAKE_SLOWDOWN: u64 = 0x0000_0000_0000_0080;
const NVML_CLOCKS_THROTTLE_REASON_DISPLAY_CLOCK_SETTING: u64 = 0x0000_0000_0000_0100;

const NVAPI_INITIALIZE_ID: u32 = 0x0150_E828;
const NVAPI_UNLOAD_ID: u32 = 0xD22B_DD7E;
const NVAPI_ENUM_PHYSICAL_GPUS_ID: u32 = 0xE5AC_921F;
const NVAPI_VOLTAGE_ID: u32 = 0x465F_9BCF;
const NVAPI_GPU_GET_BUS_ID: u32 = 0x1BE0_B8E5;
const NVAPI_GPU_GET_BUS_SLOT_ID: u32 = 0x2A0A_350F;

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

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct NvmlMemoryV2 {
    version: c_uint,
    total: c_ulonglong,
    reserved: c_ulonglong,
    free: c_ulonglong,
    used: c_ulonglong,
}

const NVML_MEMORY_V2: c_uint = (std::mem::size_of::<NvmlMemoryV2>() as c_uint) | (2u32 << 24);

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
        devices.sort();
        Ok(devices)
    }

    /// Checks whether the NVIDIA device at given index (sorted by BDF) is awake (active)
    pub fn is_device_awake(&self, index: u32) -> bool {
        match self.find_nvidia_devices() {
            Ok(devices) => {
                if let Some(dev_path) = devices.get(index as usize) {
                    self.get_runtime_status(dev_path) == GpuRuntimeStatus::Active
                } else {
                    false
                }
            }
            Err(_) => false,
        }
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct NvmlClockOffset {
    version: c_uint,
    clock_type: c_uint,
    pstate: c_uint,
    clock_offset_mhz: i32,
    min_clock_offset_mhz: i32,
    max_clock_offset_mhz: i32,
}

const NVML_CLOCK_OFFSET_VER1: c_uint =
    (std::mem::size_of::<NvmlClockOffset>() as c_uint) | (1u32 << 24);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NvApiVoltage {
    version: u32,
    flags: u32,
    padding: [u32; 8],
    value_uv: u32,
    padding2: [u32; 8],
}

impl Default for NvApiVoltage {
    fn default() -> Self {
        Self {
            version: (std::mem::size_of::<Self>() as u32 & 0xFFFF) | (1u32 << 16),
            flags: 0,
            padding: [0; 8],
            value_uv: 0,
            padding2: [0; 8],
        }
    }
}

struct NvApiState {
    library_path: Option<PathBuf>,
    attempted: bool,
    _lib: Option<Library>,
    fn_unload: Option<unsafe extern "C" fn() -> i32>,
    fn_get_voltage: Option<unsafe extern "C" fn(*mut c_void, *mut NvApiVoltage) -> i32>,
    fn_get_bus_id: Option<unsafe extern "C" fn(*mut c_void, *mut u32) -> i32>,
    fn_get_bus_slot_id: Option<unsafe extern "C" fn(*mut c_void, *mut u32) -> i32>,
    gpu_handles: Vec<usize>,
    unverified_bound_bdf: Option<String>,
    unverified_fallback_disabled: bool,
    verified_bus_slot_owner: HashMap<(u32, u32), String>,
    conflicted_bus_slots: HashSet<(u32, u32)>,
}

fn parse_bdf_bus_slot(bdf: &str) -> Option<(u32, u32)> {
    let mut parts = bdf.trim().split(':');
    let _domain = parts.next()?;
    let bus_str = parts.next()?;
    let dev_fn = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (slot_str, _func) = dev_fn.split_once('.')?;
    let bus = u32::from_str_radix(bus_str, 16).ok()?;
    let slot = u32::from_str_radix(slot_str, 16).ok()?;
    Some((bus, slot))
}

impl NvApiState {
    fn new(library_path: Option<PathBuf>, initial_bdfs: &[String]) -> Self {
        let mut verified_bus_slot_owner = HashMap::new();
        let mut conflicted_bus_slots = HashSet::new();
        for bdf in initial_bdfs {
            if let Some(key) = parse_bdf_bus_slot(bdf) {
                if conflicted_bus_slots.contains(&key) {
                    continue;
                }
                if let Some(prev) = verified_bus_slot_owner.get(&key) {
                    if prev != bdf {
                        verified_bus_slot_owner.remove(&key);
                        conflicted_bus_slots.insert(key);
                    }
                } else {
                    verified_bus_slot_owner.insert(key, bdf.clone());
                }
            }
        }
        let unverified_bound_bdf = if initial_bdfs.len() == 1 {
            Some(initial_bdfs[0].clone())
        } else {
            None
        };
        Self {
            library_path,
            attempted: false,
            _lib: None,
            fn_unload: None,
            fn_get_voltage: None,
            fn_get_bus_id: None,
            fn_get_bus_slot_id: None,
            gpu_handles: Vec::new(),
            unverified_bound_bdf,
            unverified_fallback_disabled: initial_bdfs.len() > 1,
            verified_bus_slot_owner,
            conflicted_bus_slots,
        }
    }

    fn ensure_initialized(&mut self) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        let Some(ref path) = self.library_path else {
            return;
        };
        let Ok(lib) = (unsafe { Library::new(path) }) else {
            return;
        };
        let Ok(query_sym) = (unsafe {
            lib.get::<unsafe extern "C" fn(u32) -> *mut c_void>(b"nvapi_QueryInterface")
        }) else {
            return;
        };
        let query = *query_sym;
        unsafe {
            let init_ptr = query(NVAPI_INITIALIZE_ID);
            let unload_ptr = query(NVAPI_UNLOAD_ID);
            let enum_ptr = query(NVAPI_ENUM_PHYSICAL_GPUS_ID);
            let volt_ptr = query(NVAPI_VOLTAGE_ID);
            let bus_ptr = query(NVAPI_GPU_GET_BUS_ID);
            let slot_ptr = query(NVAPI_GPU_GET_BUS_SLOT_ID);
            if init_ptr.is_null() || enum_ptr.is_null() || volt_ptr.is_null() {
                return;
            }
            let fn_init: unsafe extern "C" fn() -> i32 = std::mem::transmute(init_ptr);
            let fn_enum: unsafe extern "C" fn(*mut *mut c_void, *mut u32) -> i32 =
                std::mem::transmute(enum_ptr);
            let fn_voltage: unsafe extern "C" fn(*mut c_void, *mut NvApiVoltage) -> i32 =
                std::mem::transmute(volt_ptr);
            let fn_unload = (!unload_ptr.is_null()).then(|| {
                std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> i32>(unload_ptr)
            });
            let fn_get_bus_id = (!bus_ptr.is_null()).then(|| {
                std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, *mut u32) -> i32>(
                    bus_ptr,
                )
            });
            let fn_get_bus_slot_id = (!slot_ptr.is_null()).then(|| {
                std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, *mut u32) -> i32>(
                    slot_ptr,
                )
            });
            if fn_init() != 0 {
                return;
            }
            let mut raw_handles = [std::ptr::null_mut::<c_void>(); 64];
            let mut count: u32 = 0;
            if fn_enum(raw_handles.as_mut_ptr(), &mut count) != 0 || count == 0 {
                if let Some(unload) = fn_unload {
                    let _ = unload();
                }
                return;
            }
            self.gpu_handles = raw_handles[..usize::min(count as usize, raw_handles.len())]
                .iter()
                .map(|p| *p as usize)
                .collect();
            self.fn_unload = fn_unload;
            self.fn_get_voltage = Some(fn_voltage);
            self.fn_get_bus_id = fn_get_bus_id;
            self.fn_get_bus_slot_id = fn_get_bus_slot_id;
            self._lib = Some(lib);
        }
    }

    fn find_handle_by_bdf(
        &mut self,
        target_bdf: &str,
        current_bdfs: &[String],
    ) -> Option<*mut c_void> {
        let fn_bus = self.fn_get_bus_id?;
        let fn_slot = self.fn_get_bus_slot_id?;
        let want_key = parse_bdf_bus_slot(target_bdf)?;

        let mut matching_sysfs_count = 0usize;
        for bdf in current_bdfs {
            let key = parse_bdf_bus_slot(bdf)?;
            if key == want_key {
                matching_sysfs_count += 1;
            }
        }
        if matching_sysfs_count != 1 {
            self.verified_bus_slot_owner.remove(&want_key);
            self.conflicted_bus_slots.insert(want_key);
            return None;
        }
        if self.conflicted_bus_slots.contains(&want_key) {
            return None;
        }
        if let Some(existing) = self.verified_bus_slot_owner.get(&want_key) {
            if existing != target_bdf {
                self.verified_bus_slot_owner.remove(&want_key);
                self.conflicted_bus_slots.insert(want_key);
                return None;
            }
        } else {
            self.verified_bus_slot_owner
                .insert(want_key, target_bdf.to_string());
        }

        let mut matched: Option<*mut c_void> = None;
        for &raw in &self.gpu_handles {
            let h = raw as *mut c_void;
            let mut bus: u32 = u32::MAX;
            let mut slot: u32 = u32::MAX;
            let ok = unsafe { fn_bus(h, &mut bus) == 0 && fn_slot(h, &mut slot) == 0 };
            if ok && (bus, slot) == want_key {
                if matched.is_some() {
                    return None;
                }
                matched = Some(h);
            }
        }
        matched
    }

    fn read_voltage_mv_for_bdf(&mut self, target_bdf: &str, current_bdfs: &[String]) -> Option<u32> {
        if current_bdfs.len() != 1 || current_bdfs[0] != target_bdf {
            self.unverified_fallback_disabled = true;
            self.unverified_bound_bdf = None;
        } else if let Some(ref bound) = self.unverified_bound_bdf {
            if bound != target_bdf {
                self.unverified_fallback_disabled = true;
                self.unverified_bound_bdf = None;
            }
        }

        self.ensure_initialized();
        let fn_voltage = self.fn_get_voltage?;

        let handle = if self.fn_get_bus_id.is_some() && self.fn_get_bus_slot_id.is_some() {
            self.find_handle_by_bdf(target_bdf, current_bdfs)?
        } else {
            if self.unverified_fallback_disabled
                || current_bdfs.len() != 1
                || current_bdfs[0] != target_bdf
                || self.gpu_handles.len() != 1
            {
                self.unverified_fallback_disabled = true;
                self.unverified_bound_bdf = None;
                return None;
            }
            if self.unverified_bound_bdf.is_none() {
                self.unverified_bound_bdf = Some(target_bdf.to_string());
            }
            self.gpu_handles[0] as *mut c_void
        };

        let mut voltage = NvApiVoltage::default();
        let ret = unsafe { fn_voltage(handle, &mut voltage) };
        if ret == 0 && voltage.value_uv > 0 {
            Some(voltage.value_uv / 1000)
        } else {
            None
        }
    }
}

fn format_perf_limit_reason(reasons: u64) -> String {
    if reasons & NVML_CLOCKS_THROTTLE_REASON_HW_POWER_BRAKE_SLOWDOWN != 0 {
        "HW Power Brake".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_SW_POWER_CAP != 0 {
        "Power Limit".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_HW_THERMAL_SLOWDOWN != 0 {
        "HW Thermal".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_SW_THERMAL_SLOWDOWN != 0 {
        "SW Thermal".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_HW_SLOWDOWN != 0 {
        "HW Slowdown".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_APPLICATIONS_CLOCKS_SETTING != 0 {
        "App Clocks".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_DISPLAY_CLOCK_SETTING != 0 {
        "Display Limit".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_SYNC_BOOST != 0 {
        "Sync Boost".to_string()
    } else if reasons & NVML_CLOCKS_THROTTLE_REASON_GPU_IDLE != 0 {
        "Idle".to_string()
    } else {
        "None".to_string()
    }
}

/// Option-typed NVML telemetry snapshot preserving partial-sensor availability without
/// substituting `0` on failing getters (`HW-08` / `M7.3`–`M7.4`).
#[derive(Debug, Clone)]
pub struct NvmlTelemetrySnapshot {
    pub name: Option<String>,
    pub temperature_c: Option<u32>,
    pub power_usage_mw: Option<u32>,
    pub enforced_limit_mw: Option<u32>,
    pub default_power_limit_mw: Option<u32>,
    pub max_power_limit_mw: Option<u32>,
    pub utilization: Option<NvmlUtilization>,
    pub memory: Option<NvmlMemory>,
    pub core_clock_mhz: Option<u32>,
    pub max_core_clock_mhz: Option<u32>,
    pub vram_clock_mhz: Option<u32>,
    pub perf_limit_reason: Option<String>,
    pub encoder_util_pct: Option<u32>,
    pub decoder_util_pct: Option<u32>,
    pub current_pstate: Option<u32>,
    pub gr_clock_offset_mhz: Option<i32>,
    pub mem_clock_offset_mhz: Option<i32>,
    pub core_voltage_mv: Option<u32>,
    pub collected_at: Instant,
}

/// Dynamically loaded NVML client
#[allow(clippy::type_complexity)]
pub struct NvmlWrapper {
    _lib: Library,
    init_ok: Arc<AtomicBool>,
    init_lock: Mutex<()>,
    device_count: Arc<AtomicU32>,
    power_guard: NvidiaPowerGuard,
    nvapi: Mutex<NvApiState>,

    // Required function pointers
    fn_init: unsafe extern "C" fn() -> c_uint,
    fn_shutdown: unsafe extern "C" fn() -> c_uint,
    fn_device_get_count: unsafe extern "C" fn(*mut c_uint) -> c_uint,
    fn_device_get_handle_by_index: unsafe extern "C" fn(c_uint, *mut *mut c_void) -> c_uint,
    fn_device_get_handle_by_pci_bus_id:
        Option<unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_uint>,
    fn_device_get_name: unsafe extern "C" fn(*mut c_void, *mut c_char, c_uint) -> c_uint,
    fn_device_get_temperature: unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_uint,
    fn_device_get_power_usage: unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint,
    fn_device_get_enforced_power_limit: unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint,
    fn_device_get_power_management_limit_constraints:
        unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_uint,
    fn_device_get_power_management_default_limit:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint>,
    fn_device_set_power_management_limit: unsafe extern "C" fn(*mut c_void, c_uint) -> c_uint,
    fn_device_get_utilization_rates:
        unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> c_uint,
    fn_device_get_memory_info_v1:
        Option<unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> c_uint>,
    fn_device_get_memory_info_v2:
        Option<unsafe extern "C" fn(*mut c_void, *mut NvmlMemoryV2) -> c_uint>,

    // Optional telemetry / OC function pointers
    fn_device_get_clock_info: Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_uint>,
    fn_device_get_max_clock_info:
        Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_uint>,
    fn_device_get_performance_state:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint>,
    fn_device_get_current_clocks_throttle_reasons:
        Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> c_uint>,
    fn_device_get_encoder_utilization:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_uint>,
    fn_device_get_decoder_utilization:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_uint>,
    fn_device_get_clock_offsets:
        Option<unsafe extern "C" fn(*mut c_void, *mut NvmlClockOffset) -> c_uint>,
}

impl NvmlWrapper {
    /// Attempts to load `libnvidia-ml.so.1` or `libnvidia-ml.so` (and lazily `libnvidia-api.so.1`)
    pub fn new(power_guard: NvidiaPowerGuard) -> Result<Self, NvmlError> {
        let lib = unsafe {
            Library::new("libnvidia-ml.so.1").or_else(|_| Library::new("libnvidia-ml.so"))?
        };
        Self::from_library(power_guard, lib, Some(PathBuf::from("libnvidia-api.so.1")))
    }

    /// Loads NVML symbols from an explicit shared-library path without touching host `libnvidia-api.so.1`.
    pub fn with_library_path(
        power_guard: NvidiaPowerGuard,
        library_path: impl AsRef<Path>,
    ) -> Result<Self, NvmlError> {
        let lib = unsafe { Library::new(library_path.as_ref())? };
        Self::from_library(power_guard, lib, None)
    }

    /// Loads NVML and an optional NvAPI shared library from explicit paths (useful for isolated tests).
    pub fn with_library_paths(
        power_guard: NvidiaPowerGuard,
        nvml_library_path: impl AsRef<Path>,
        nvapi_library_path: Option<&Path>,
    ) -> Result<Self, NvmlError> {
        let lib = unsafe { Library::new(nvml_library_path.as_ref())? };
        Self::from_library(power_guard, lib, nvapi_library_path.map(Path::to_path_buf))
    }

    fn from_library(
        power_guard: NvidiaPowerGuard,
        lib: Library,
        nvapi_path: Option<PathBuf>,
    ) -> Result<Self, NvmlError> {
        unsafe {
            let fn_init: unsafe extern "C" fn() -> c_uint = *lib.get(b"nvmlInit_v2")?;
            let fn_shutdown: unsafe extern "C" fn() -> c_uint = *lib.get(b"nvmlShutdown")?;
            let fn_device_get_count: unsafe extern "C" fn(*mut c_uint) -> c_uint =
                *lib.get(b"nvmlDeviceGetCount_v2")?;
            let fn_device_get_handle_by_index: unsafe extern "C" fn(
                c_uint,
                *mut *mut c_void,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetHandleByIndex_v2")?;
            let fn_device_get_handle_by_pci_bus_id: Option<
                unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_uint,
            > = lib
                .get(b"nvmlDeviceGetHandleByPciBusId_v2")
                .or_else(|_| lib.get(b"nvmlDeviceGetHandleByPciBusId"))
                .ok()
                .map(|s| *s);
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
            let fn_device_get_power_management_default_limit = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint>(
                    b"nvmlDeviceGetPowerManagementDefaultLimit",
                )
                .ok()
                .map(|s| *s);
            let fn_device_set_power_management_limit: unsafe extern "C" fn(
                *mut c_void,
                c_uint,
            ) -> c_uint = *lib.get(b"nvmlDeviceSetPowerManagementLimit")?;
            let fn_device_get_utilization_rates: unsafe extern "C" fn(
                *mut c_void,
                *mut NvmlUtilization,
            ) -> c_uint = *lib.get(b"nvmlDeviceGetUtilizationRates")?;
            let fn_device_get_memory_info_v1 = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> c_uint>(
                    b"nvmlDeviceGetMemoryInfo",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_memory_info_v2 = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut NvmlMemoryV2) -> c_uint>(
                    b"nvmlDeviceGetMemoryInfo_v2",
                )
                .ok()
                .map(|s| *s);
            if fn_device_get_memory_info_v1.is_none() && fn_device_get_memory_info_v2.is_none() {
                let _: unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> c_uint =
                    *lib.get(b"nvmlDeviceGetMemoryInfo")?;
            }

            let initial_bdfs: Vec<String> = power_guard
                .find_nvidia_devices()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
                .collect();

            let fn_device_get_clock_info = lib
                .get::<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_uint>(
                    b"nvmlDeviceGetClockInfo",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_max_clock_info = lib
                .get::<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_uint>(
                    b"nvmlDeviceGetMaxClockInfo",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_performance_state = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_uint) -> c_uint>(
                    b"nvmlDeviceGetPerformanceState",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_current_clocks_throttle_reasons = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut u64) -> c_uint>(
                    b"nvmlDeviceGetCurrentClocksThrottleReasons",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_encoder_utilization = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_uint>(
                    b"nvmlDeviceGetEncoderUtilization",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_decoder_utilization = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut c_uint) -> c_uint>(
                    b"nvmlDeviceGetDecoderUtilization",
                )
                .ok()
                .map(|s| *s);
            let fn_device_get_clock_offsets = lib
                .get::<unsafe extern "C" fn(*mut c_void, *mut NvmlClockOffset) -> c_uint>(
                    b"nvmlDeviceGetClockOffsets",
                )
                .ok()
                .map(|s| *s);

            let init_ok = Arc::new(AtomicBool::new(false));
            let device_count = Arc::new(AtomicU32::new(0));

            // Strict D3cold guard: do NOT invoke native nvmlInit during construction
            // if the discrete GPU is suspended, avoiding waking the hardware.
            if power_guard.is_dgpu_awake() {
                let ret = fn_init();
                if ret == NVML_SUCCESS {
                    init_ok.store(true, Ordering::SeqCst);
                    let mut count: c_uint = 0;
                    if fn_device_get_count(&mut count) == NVML_SUCCESS && count > 0 {
                        device_count.store(count, Ordering::SeqCst);
                    }
                }
            }

            Ok(Self {
                _lib: lib,
                init_ok,
                init_lock: Mutex::new(()),
                device_count,
                power_guard,
                nvapi: Mutex::new(NvApiState::new(nvapi_path, &initial_bdfs)),
                fn_init,
                fn_shutdown,
                fn_device_get_count,
                fn_device_get_handle_by_index,
                fn_device_get_handle_by_pci_bus_id,
                fn_device_get_name,
                fn_device_get_temperature,
                fn_device_get_power_usage,
                fn_device_get_enforced_power_limit,
                fn_device_get_power_management_limit_constraints,
                fn_device_get_power_management_default_limit,
                fn_device_set_power_management_limit,
                fn_device_get_utilization_rates,
                fn_device_get_memory_info_v1,
                fn_device_get_memory_info_v2,
                fn_device_get_clock_info,
                fn_device_get_max_clock_info,
                fn_device_get_performance_state,
                fn_device_get_current_clocks_throttle_reasons,
                fn_device_get_encoder_utilization,
                fn_device_get_decoder_utilization,
                fn_device_get_clock_offsets,
            })
        }
    }

    pub fn device_count(&self) -> u32 {
        self.device_count.load(Ordering::SeqCst)
    }

    fn resolve_active_device_path(&self, sysfs_idx: u32) -> Result<PathBuf, NvmlError> {
        let devices = self.power_guard.find_nvidia_devices()?;
        let dev_path = devices
            .get(sysfs_idx as usize)
            .ok_or(NvmlError::NoGpuFound)?;
        if self.power_guard.get_runtime_status(dev_path) != GpuRuntimeStatus::Active {
            return Err(NvmlError::GpuSuspended);
        }
        Ok(dev_path.clone())
    }

    fn resolve_active_device_by_bdf(&self, bdf: &str) -> Result<PathBuf, NvmlError> {
        let devices = self.power_guard.find_nvidia_devices()?;
        let dev_path = devices
            .into_iter()
            .find(|d| d.file_name().and_then(|n| n.to_str()) == Some(bdf))
            .ok_or(NvmlError::NoGpuFound)?;
        if self.power_guard.get_runtime_status(&dev_path) != GpuRuntimeStatus::Active {
            return Err(NvmlError::GpuSuspended);
        }
        Ok(dev_path)
    }

    fn ensure_active_and_initialized_for_path(&self, dev_path: &Path) -> Result<(), NvmlError> {
        if self.power_guard.get_runtime_status(dev_path) != GpuRuntimeStatus::Active {
            return Err(NvmlError::GpuSuspended);
        }

        if self.init_ok.load(Ordering::SeqCst) && self.device_count.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }

        let _guard = self.init_lock.lock().unwrap();

        if self.power_guard.get_runtime_status(dev_path) != GpuRuntimeStatus::Active {
            return Err(NvmlError::GpuSuspended);
        }

        if !self.init_ok.load(Ordering::SeqCst) {
            unsafe {
                let ret = (self.fn_init)();
                if ret != NVML_SUCCESS {
                    return Err(NvmlError::NvmlCall(ret));
                }
                self.init_ok.store(true, Ordering::SeqCst);
            }
        }

        if self.device_count.load(Ordering::SeqCst) == 0 {
            unsafe {
                let mut count: c_uint = 0;
                let count_ret = (self.fn_device_get_count)(&mut count);
                if count_ret == NVML_SUCCESS && count > 0 {
                    self.device_count.store(count, Ordering::SeqCst);
                } else if count_ret != NVML_SUCCESS {
                    return Err(NvmlError::NvmlCall(count_ret));
                } else {
                    return Err(NvmlError::NoGpuFound);
                }
            }
        }
        Ok(())
    }

    /// Resolves an NVML handle for the already-pinned sysfs PCI device path (`W1-08`).
    ///
    /// - BDF-capable drivers bind directly to `dev_path`'s BDF without re-indexing sysfs,
    ///   preventing concurrent hotplug of a lower-BDF GPU from hijacking the selected card.
    /// - Index-only drivers fail closed whenever >1 NVIDIA display device is present (even if
    ///   all appear active) or the sole device does not match `dev_path`.
    fn get_device_handle_for_path(&self, dev_path: &Path) -> Result<*mut c_void, NvmlError> {
        if self.power_guard.get_runtime_status(dev_path) != GpuRuntimeStatus::Active {
            return Err(NvmlError::GpuSuspended);
        }

        if let Some(fn_by_pci) = self.fn_device_get_handle_by_pci_bus_id {
            let bdf_os = dev_path.file_name().ok_or(NvmlError::NoGpuFound)?;
            let bdf_str = bdf_os.to_str().ok_or(NvmlError::NoGpuFound)?;
            let c_bdf = CString::new(bdf_str).map_err(|_| NvmlError::NoGpuFound)?;
            let mut handle: *mut c_void = std::ptr::null_mut();
            let ret = unsafe { fn_by_pci(c_bdf.as_ptr(), &mut handle) };
            if ret != NVML_SUCCESS || handle.is_null() {
                return Err(NvmlError::NvmlCall(ret));
            }
            return Ok(handle);
        }

        let devices = self.power_guard.find_nvidia_devices()?;
        let mut live_count: c_uint = 0;
        let count_ret = unsafe { (self.fn_device_get_count)(&mut live_count) };
        if count_ret != NVML_SUCCESS {
            return Err(NvmlError::NvmlCall(count_ret));
        }
        self.device_count.store(live_count, Ordering::SeqCst);
        if devices.len() > 1 || live_count != 1 {
            let any_suspended = devices
                .iter()
                .any(|d| self.power_guard.get_runtime_status(d) != GpuRuntimeStatus::Active);
            return Err(if any_suspended {
                NvmlError::GpuSuspended
            } else {
                NvmlError::NvmlCall(NVML_ERROR_NOT_SUPPORTED)
            });
        }
        if devices.len() != 1 || devices[0] != dev_path {
            return Err(NvmlError::NoGpuFound);
        }

        unsafe {
            let mut handle: *mut c_void = std::ptr::null_mut();
            let ret = (self.fn_device_get_handle_by_index)(0, &mut handle);
            if ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(ret));
            }
            Ok(handle)
        }
    }

    fn resolve_pinned_handle_and_bdf_by_index(
        &self,
        index: u32,
    ) -> Result<(*mut c_void, String), NvmlError> {
        let dev_path = self.resolve_active_device_path(index)?;
        let bdf = dev_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(NvmlError::NoGpuFound)?
            .to_string();
        self.ensure_active_and_initialized_for_path(&dev_path)?;
        let handle = self.get_device_handle_for_path(&dev_path)?;
        Ok((handle, bdf))
    }

    fn resolve_pinned_handle_by_index(&self, index: u32) -> Result<*mut c_void, NvmlError> {
        self.resolve_pinned_handle_and_bdf_by_index(index)
            .map(|(handle, _)| handle)
    }

    fn resolve_pinned_handle_by_bdf(&self, bdf: &str) -> Result<*mut c_void, NvmlError> {
        let dev_path = self.resolve_active_device_by_bdf(bdf)?;
        self.ensure_active_and_initialized_for_path(&dev_path)?;
        self.get_device_handle_for_path(&dev_path)
    }

    /// Query minimum and maximum power limits in milliwatts
    pub fn get_power_limit_constraints(&self, index: u32) -> Result<(u32, u32), NvmlError> {
        let handle = self.resolve_pinned_handle_by_index(index)?;

        unsafe {
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

    /// Read telemetry for a GPU index, strictly respecting D3cold guard and pinned BDF identity.
    pub fn get_telemetry(&self, index: u32) -> Result<NvmlTelemetry, NvmlError> {
        let handle = self.resolve_pinned_handle_by_index(index)?;
        self.read_telemetry_for_handle(handle)
    }

    /// Read telemetry for a specific PCI BDF (e.g. `"0000:02:00.0"`), strictly guarded.
    pub fn get_telemetry_by_bdf(&self, bdf: &str) -> Result<NvmlTelemetry, NvmlError> {
        let handle = self.resolve_pinned_handle_by_bdf(bdf)?;
        self.read_telemetry_for_handle(handle)
    }

    /// Read Option-typed telemetry for GPU `index` without turning getter errors into zero (`HW-08`).
    pub fn get_telemetry_snapshot(&self, index: u32) -> Result<NvmlTelemetrySnapshot, NvmlError> {
        let (handle, bdf) = self.resolve_pinned_handle_and_bdf_by_index(index)?;
        self.read_telemetry_snapshot_for_handle(handle, &bdf)
    }

    /// Read Option-typed telemetry for an explicit PCI BDF without turning getter errors into zero.
    pub fn get_telemetry_snapshot_by_bdf(
        &self,
        bdf: &str,
    ) -> Result<NvmlTelemetrySnapshot, NvmlError> {
        let handle = self.resolve_pinned_handle_by_bdf(bdf)?;
        self.read_telemetry_snapshot_for_handle(handle, bdf)
    }

    unsafe fn read_memory_info(&self, handle: *mut c_void) -> Result<NvmlMemory, c_uint> {
        if let Some(f2) = self.fn_device_get_memory_info_v2 {
            let mut mem_v2 = NvmlMemoryV2 {
                version: NVML_MEMORY_V2,
                ..Default::default()
            };
            let ret2 = unsafe { f2(handle, &mut mem_v2) };
            if ret2 == NVML_SUCCESS {
                return Ok(NvmlMemory {
                    total: mem_v2.total,
                    free: mem_v2.free,
                    used: mem_v2.used,
                });
            }
            if let Some(f1) = self.fn_device_get_memory_info_v1 {
                let mut mem_v1 = NvmlMemory::default();
                let ret1 = unsafe { f1(handle, &mut mem_v1) };
                if ret1 == NVML_SUCCESS {
                    return Ok(mem_v1);
                }
                return Err(ret1);
            }
            return Err(ret2);
        }

        if let Some(f1) = self.fn_device_get_memory_info_v1 {
            let mut mem_v1 = NvmlMemory::default();
            let ret1 = unsafe { f1(handle, &mut mem_v1) };
            if ret1 == NVML_SUCCESS {
                return Ok(mem_v1);
            }
            return Err(ret1);
        }

        Err(NVML_ERROR_NOT_SUPPORTED)
    }

    fn read_telemetry_snapshot_for_handle(
        &self,
        handle: *mut c_void,
        target_bdf: &str,
    ) -> Result<NvmlTelemetrySnapshot, NvmlError> {
        unsafe {
            let mut last_err: Option<c_uint> = None;

            let mut name_buf = [0 as c_char; 256];
            let name_ret = (self.fn_device_get_name)(handle, name_buf.as_mut_ptr(), 256);
            name_buf[255] = 0;
            let name = if name_ret == NVML_SUCCESS {
                CStr::from_ptr(name_buf.as_ptr())
                    .to_str()
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            } else {
                last_err = Some(name_ret);
                None
            };

            let mut temp: c_uint = 0;
            let temp_ret = (self.fn_device_get_temperature)(handle, NVML_TEMPERATURE_GPU, &mut temp);
            let temperature_c = if temp_ret == NVML_SUCCESS {
                Some(temp)
            } else {
                last_err = Some(temp_ret);
                None
            };

            let mut power_mw: c_uint = 0;
            let power_ret = (self.fn_device_get_power_usage)(handle, &mut power_mw);
            let power_usage_mw = if power_ret == NVML_SUCCESS {
                Some(power_mw)
            } else {
                last_err = Some(power_ret);
                None
            };

            let mut limit_mw: c_uint = 0;
            let limit_ret = (self.fn_device_get_enforced_power_limit)(handle, &mut limit_mw);
            let enforced_limit_mw = if limit_ret == NVML_SUCCESS {
                Some(limit_mw)
            } else {
                last_err = Some(limit_ret);
                None
            };

            let mut min_mw: c_uint = 0;
            let mut max_mw: c_uint = 0;
            let constr_ret = (self.fn_device_get_power_management_limit_constraints)(
                handle,
                &mut min_mw,
                &mut max_mw,
            );
            let max_power_limit_mw = if constr_ret == NVML_SUCCESS && max_mw > 0 {
                Some(max_mw)
            } else {
                if constr_ret != NVML_SUCCESS {
                    last_err = Some(constr_ret);
                }
                None
            };

            let default_power_limit_mw =
                if let Some(get_def) = self.fn_device_get_power_management_default_limit {
                    let mut def_mw: c_uint = 0;
                    if (get_def)(handle, &mut def_mw) == NVML_SUCCESS && def_mw > 0 {
                        Some(def_mw)
                    } else {
                        None
                    }
                } else {
                    None
                };

            let mut util = NvmlUtilization::default();
            let util_ret = (self.fn_device_get_utilization_rates)(handle, &mut util);
            let utilization = if util_ret == NVML_SUCCESS {
                Some(util)
            } else {
                last_err = Some(util_ret);
                None
            };

            let memory = match self.read_memory_info(handle) {
                Ok(mem) => Some(mem),
                Err(err) => {
                    last_err = Some(err);
                    None
                }
            };

            let core_clock_mhz = self.fn_device_get_clock_info.and_then(|f| {
                let mut mhz: c_uint = 0;
                (f(handle, NVML_CLOCK_GRAPHICS, &mut mhz) == NVML_SUCCESS).then_some(mhz)
            });

            let max_core_clock_mhz = self.fn_device_get_max_clock_info.and_then(|f| {
                let mut mhz: c_uint = 0;
                (f(handle, NVML_CLOCK_GRAPHICS, &mut mhz) == NVML_SUCCESS).then_some(mhz)
            });

            let vram_clock_mhz = self.fn_device_get_clock_info.and_then(|f| {
                let mut mhz: c_uint = 0;
                (f(handle, NVML_CLOCK_MEM, &mut mhz) == NVML_SUCCESS).then_some(mhz)
            });

            let perf_limit_reason = self
                .fn_device_get_current_clocks_throttle_reasons
                .and_then(|f| {
                    let mut reasons: u64 = 0;
                    (f(handle, &mut reasons) == NVML_SUCCESS)
                        .then(|| format_perf_limit_reason(reasons))
                });

            let encoder_util_pct = self.fn_device_get_encoder_utilization.and_then(|f| {
                let mut util: c_uint = 0;
                let mut sample_us: c_uint = 0;
                (f(handle, &mut util, &mut sample_us) == NVML_SUCCESS).then_some(util)
            });

            let decoder_util_pct = self.fn_device_get_decoder_utilization.and_then(|f| {
                let mut util: c_uint = 0;
                let mut sample_us: c_uint = 0;
                (f(handle, &mut util, &mut sample_us) == NVML_SUCCESS).then_some(util)
            });

            let current_pstate = self.fn_device_get_performance_state.and_then(|f| {
                let mut pstate: c_uint = NVML_PSTATE_UNKNOWN;
                if f(handle, &mut pstate) == NVML_SUCCESS && pstate != NVML_PSTATE_UNKNOWN {
                    Some(pstate)
                } else {
                    None
                }
            });

            let (gr_clock_offset_mhz, mem_clock_offset_mhz) =
                if let (Some(ps), Some(f)) = (current_pstate, self.fn_device_get_clock_offsets) {
                    let mut gr_info = NvmlClockOffset {
                        version: NVML_CLOCK_OFFSET_VER1,
                        clock_type: NVML_CLOCK_GRAPHICS,
                        pstate: ps,
                        ..Default::default()
                    };
                    let gr = (f(handle, &mut gr_info) == NVML_SUCCESS)
                        .then_some(gr_info.clock_offset_mhz);

                    let mut mem_info = NvmlClockOffset {
                        version: NVML_CLOCK_OFFSET_VER1,
                        clock_type: NVML_CLOCK_MEM,
                        pstate: ps,
                        ..Default::default()
                    };
                    let mem = (f(handle, &mut mem_info) == NVML_SUCCESS)
                        .then_some(mem_info.clock_offset_mhz);
                    (gr, mem)
                } else {
                    (None, None)
                };

            let current_bdfs: Vec<String> = self
                .power_guard
                .find_nvidia_devices()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
                .collect();
            let core_voltage_mv = self
                .nvapi
                .lock()
                .unwrap()
                .read_voltage_mv_for_bdf(target_bdf, &current_bdfs);

            if temperature_c.is_none()
                && power_usage_mw.is_none()
                && enforced_limit_mw.is_none()
                && default_power_limit_mw.is_none()
                && max_power_limit_mw.is_none()
                && utilization.is_none()
                && memory.is_none()
                && core_clock_mhz.is_none()
                && max_core_clock_mhz.is_none()
                && vram_clock_mhz.is_none()
                && perf_limit_reason.is_none()
                && encoder_util_pct.is_none()
                && decoder_util_pct.is_none()
                && current_pstate.is_none()
                && core_voltage_mv.is_none()
            {
                return Err(NvmlError::NvmlCall(last_err.unwrap_or(NVML_ERROR_NOT_SUPPORTED)));
            }

            Ok(NvmlTelemetrySnapshot {
                name,
                temperature_c,
                power_usage_mw,
                enforced_limit_mw,
                default_power_limit_mw,
                max_power_limit_mw,
                utilization,
                memory,
                core_clock_mhz,
                max_core_clock_mhz,
                vram_clock_mhz,
                perf_limit_reason,
                encoder_util_pct,
                decoder_util_pct,
                current_pstate,
                gr_clock_offset_mhz,
                mem_clock_offset_mhz,
                core_voltage_mv,
                collected_at: Instant::now(),
            })
        }
    }

    fn read_telemetry_for_handle(&self, handle: *mut c_void) -> Result<NvmlTelemetry, NvmlError> {
        unsafe {
            let mut name_buf = [0 as c_char; 256];
            let _ = (self.fn_device_get_name)(handle, name_buf.as_mut_ptr(), 256);
            name_buf[255] = 0;
            let name = CStr::from_ptr(name_buf.as_ptr())
                .to_str()
                .unwrap_or("NVIDIA GPU")
                .to_string();

            let mut temp: c_uint = 0;
            let temp_ret = (self.fn_device_get_temperature)(handle, NVML_TEMPERATURE_GPU, &mut temp);
            if temp_ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(temp_ret));
            }

            let mut power_mw: c_uint = 0;
            let power_ret = (self.fn_device_get_power_usage)(handle, &mut power_mw);
            if power_ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(power_ret));
            }

            let mut limit_mw: c_uint = 0;
            let limit_ret = (self.fn_device_get_enforced_power_limit)(handle, &mut limit_mw);
            if limit_ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(limit_ret));
            }

            let mut util = NvmlUtilization::default();
            let util_ret = (self.fn_device_get_utilization_rates)(handle, &mut util);
            if util_ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(util_ret));
            }

            let mem = self
                .read_memory_info(handle)
                .map_err(NvmlError::NvmlCall)?;

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

    /// Set power limit in milliwatts for GPU `index`.
    pub fn set_power_limit(&self, index: u32, power_mw: u32) -> Result<(), NvmlError> {
        let handle = self.resolve_pinned_handle_by_index(index)?;
        self.apply_power_limit_for_handle(handle, power_mw)
    }

    /// Set power limit in milliwatts for an explicit PCI BDF (`"0000:02:00.0"`).
    pub fn set_power_limit_by_bdf(&self, bdf: &str, power_mw: u32) -> Result<(), NvmlError> {
        let handle = self.resolve_pinned_handle_by_bdf(bdf)?;
        self.apply_power_limit_for_handle(handle, power_mw)
    }

    fn apply_power_limit_for_handle(
        &self,
        handle: *mut c_void,
        power_mw: u32,
    ) -> Result<(), NvmlError> {
        unsafe {
            let mut min_mw: c_uint = 0;
            let mut max_mw: c_uint = 0;
            let constr_ret = (self.fn_device_get_power_management_limit_constraints)(
                handle,
                &mut min_mw,
                &mut max_mw,
            );
            if constr_ret != NVML_SUCCESS {
                return Err(NvmlError::NvmlCall(constr_ret));
            }
            if min_mw == 0 || max_mw < min_mw || power_mw < min_mw || power_mw > max_mw {
                return Err(NvmlError::NvmlCall(NVML_ERROR_NOT_SUPPORTED));
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
        if let Ok(mut nvapi) = self.nvapi.lock() {
            if let Some(unload) = nvapi.fn_unload.take() {
                unsafe {
                    let _ = unload();
                }
            }
            nvapi.fn_get_voltage = None;
            nvapi.gpu_handles.clear();
            nvapi._lib = None;
        }
        if self.init_ok.swap(false, Ordering::SeqCst) {
            unsafe {
                (self.fn_shutdown)();
            }
        }
    }
}
