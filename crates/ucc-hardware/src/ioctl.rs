//! Linux `/dev/tuxedo_io` ioctl request definitions and safe abstraction.
//!
//! Note on ABI: The Tuxedo IO driver historically defines `_IOR` / `_IOW` using pointer types
//! (e.g. `int32_t*`, `char*`), so the size encoded in the ioctl request number on 64-bit systems
//! is `std::mem::size_of::<*const ()>()` (8 bytes), NOT `4`. This module strictly preserves this
//! kernel ABI.

use nix::libc;
use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Direction bitfields for Linux ioctl macros
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

/// Helper to encode Linux ioctl numbers matching Linux `<linux/ioctl.h>`
const fn _ioc(dir: u32, type_: u32, nr: u32, size: u32) -> u64 {
    ((dir as u64) << 30) | ((type_ as u64) << 8) | (nr as u64) | ((size as u64) << 16)
}

/// Helper for `_IO(type, nr)`
const fn _io(type_: u32, nr: u32) -> u64 {
    _ioc(IOC_NONE, type_, nr, 0)
}

/// Helper for `_IOR(type, nr, pointer_type)` on 64-bit Linux (size = 8)
const fn _ior_ptr(type_: u32, nr: u32) -> u64 {
    _ioc(IOC_READ, type_, nr, 8)
}

/// Helper for `_IOW(type, nr, pointer_type)` on 64-bit Linux (size = 8)
const fn _iow_ptr(type_: u32, nr: u32) -> u64 {
    _ioc(IOC_WRITE, type_, nr, 8)
}

pub mod magic {
    pub const IOCTL_MAGIC: u32 = 0xEC;
    pub const MAGIC_READ_CL: u32 = IOCTL_MAGIC + 1; // 0xED
    pub const MAGIC_WRITE_CL: u32 = IOCTL_MAGIC + 2; // 0xEE
    pub const MAGIC_READ_UW: u32 = IOCTL_MAGIC + 3; // 0xEF
    pub const MAGIC_WRITE_UW: u32 = IOCTL_MAGIC + 4; // 0xF0
}

pub mod requests {
    use super::magic::*;
    use super::*;

    // General
    pub const R_MOD_VERSION: u64 = _ior_ptr(IOCTL_MAGIC, 0x00);
    pub const R_HWCHECK_CL: u64 = _ior_ptr(IOCTL_MAGIC, 0x05);
    pub const R_HWCHECK_UW: u64 = _ior_ptr(IOCTL_MAGIC, 0x06);

    // Clevo interface - Read
    pub const R_CL_HW_IF_STR: u64 = _ior_ptr(MAGIC_READ_CL, 0x00);
    pub const R_CL_FANINFO1: u64 = _ior_ptr(MAGIC_READ_CL, 0x10);
    pub const R_CL_FANINFO2: u64 = _ior_ptr(MAGIC_READ_CL, 0x11);
    pub const R_CL_FANINFO3: u64 = _ior_ptr(MAGIC_READ_CL, 0x12);
    pub const R_CL_WEBCAM_SW: u64 = _ior_ptr(MAGIC_READ_CL, 0x13);
    pub const R_CL_FLIGHTMODE_SW: u64 = _ior_ptr(MAGIC_READ_CL, 0x14);
    pub const R_CL_TOUCHPAD_SW: u64 = _ior_ptr(MAGIC_READ_CL, 0x15);

    // Clevo interface - Write
    pub const W_CL_FANSPEED: u64 = _iow_ptr(MAGIC_WRITE_CL, 0x10);
    pub const W_CL_FANAUTO: u64 = _iow_ptr(MAGIC_WRITE_CL, 0x11);
    pub const W_CL_WEBCAM_SW: u64 = _iow_ptr(MAGIC_WRITE_CL, 0x12);
    pub const W_CL_FLIGHTMODE_SW: u64 = _iow_ptr(MAGIC_WRITE_CL, 0x13);
    pub const W_CL_TOUCHPAD_SW: u64 = _iow_ptr(MAGIC_WRITE_CL, 0x14);
    pub const W_CL_PERF_PROFILE: u64 = _iow_ptr(MAGIC_WRITE_CL, 0x15);

    // Uniwill interface - Read
    pub const R_UW_HW_IF_STR: u64 = _ior_ptr(MAGIC_READ_UW, 0x00);
    pub const R_UW_MODEL_ID: u64 = _ior_ptr(MAGIC_READ_UW, 0x01);
    pub const R_UW_FANSPEED: u64 = _ior_ptr(MAGIC_READ_UW, 0x10);
    pub const R_UW_FANSPEED2: u64 = _ior_ptr(MAGIC_READ_UW, 0x11);
    pub const R_UW_FAN_TEMP: u64 = _ior_ptr(MAGIC_READ_UW, 0x12);
    pub const R_UW_FAN_TEMP2: u64 = _ior_ptr(MAGIC_READ_UW, 0x13);
    pub const R_UW_MODE: u64 = _ior_ptr(MAGIC_READ_UW, 0x14);
    pub const R_UW_MODE_ENABLE: u64 = _ior_ptr(MAGIC_READ_UW, 0x15);
    pub const R_UW_FANS_OFF_AVAILABLE: u64 = _ior_ptr(MAGIC_READ_UW, 0x16);
    pub const R_UW_FANS_MIN_SPEED: u64 = _ior_ptr(MAGIC_READ_UW, 0x17);
    pub const R_UW_TDP0: u64 = _ior_ptr(MAGIC_READ_UW, 0x18);
    pub const R_UW_TDP1: u64 = _ior_ptr(MAGIC_READ_UW, 0x19);
    pub const R_UW_TDP2: u64 = _ior_ptr(MAGIC_READ_UW, 0x1a);
    pub const R_UW_TDP0_MIN: u64 = _ior_ptr(MAGIC_READ_UW, 0x1b);
    pub const R_UW_TDP1_MIN: u64 = _ior_ptr(MAGIC_READ_UW, 0x1c);
    pub const R_UW_TDP2_MIN: u64 = _ior_ptr(MAGIC_READ_UW, 0x1d);
    pub const R_UW_TDP0_MAX: u64 = _ior_ptr(MAGIC_READ_UW, 0x1e);
    pub const R_UW_TDP1_MAX: u64 = _ior_ptr(MAGIC_READ_UW, 0x1f);
    pub const R_UW_TDP2_MAX: u64 = _ior_ptr(MAGIC_READ_UW, 0x20);
    pub const R_UW_PROFS_AVAILABLE: u64 = _ior_ptr(MAGIC_READ_UW, 0x21);

    // Uniwill interface - Write
    pub const W_UW_FANSPEED: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x10);
    pub const W_UW_FANSPEED2: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x11);
    pub const W_UW_MODE: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x12);
    pub const W_UW_MODE_ENABLE: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x13);
    pub const W_UW_FANAUTO: u64 = _io(MAGIC_WRITE_UW, 0x14);
    pub const W_UW_TDP0: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x15);
    pub const W_UW_TDP1: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x16);
    pub const W_UW_TDP2: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x17);
    pub const W_UW_PERF_PROF: u64 = _iow_ptr(MAGIC_WRITE_UW, 0x18);
}

#[derive(Debug, Error)]
pub enum IoctlError {
    #[error("Failed to open device at {path}: {source}")]
    OpenDevice {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Ioctl request 0x{request:08x} failed: {source}")]
    IoctlFailed {
        request: u64,
        #[source]
        source: nix::Error,
    },
    #[error("String returned from ioctl is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("Hardware precondition failed: {0}")]
    PreconditionFailed(&'static str),
    #[error("Hardware device is unavailable or closed")]
    DeviceUnavailable,
}

/// Typed wrapper around a character device like `/dev/tuxedo_io`.
pub struct TuxedoIoDevice {
    file: File,
    path: PathBuf,
}

impl TuxedoIoDevice {
    pub const DEFAULT_PATH: &'static str = "/dev/tuxedo_io";

    /// Opens the device at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IoctlError> {
        let path_buf = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_buf)
            .map_err(|e| IoctlError::OpenDevice {
                path: path_buf.clone(),
                source: e,
            })?;
        Ok(Self {
            file,
            path: path_buf,
        })
    }

    /// Returns the device path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read an `i32` value via ioctl.
    pub fn read_i32(&self, request: u64) -> Result<i32, IoctlError> {
        let mut val: i32 = 0;
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), request, &mut val) };
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(val)
    }

    /// Write an `i32` value via ioctl.
    pub fn write_i32(&self, request: u64, mut val: i32) -> Result<(), IoctlError> {
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), request, &mut val) };
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(())
    }

    /// Execute an ioctl without arguments (such as `W_UW_FANAUTO`).
    pub fn call_none(&self, request: u64) -> Result<(), IoctlError> {
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), request) };
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(())
    }

    /// Maximum buffer size for module and interface version strings per kernel ABI (20 bytes).
    pub const VERSION_BUFFER_LEN: usize = 20;

    /// Reads driver module version safely using the kernel ABI fixed 20-byte buffer.
    pub fn read_module_version(&self) -> Result<String, IoctlError> {
        self.read_str(requests::R_MOD_VERSION, Self::VERSION_BUFFER_LEN)
    }

    /// Reads Uniwill hardware interface string safely using the kernel ABI fixed 20-byte buffer.
    pub fn read_uniwill_interface_string(&self) -> Result<String, IoctlError> {
        self.read_str(requests::R_UW_HW_IF_STR, Self::VERSION_BUFFER_LEN)
    }

    /// Reads Clevo hardware interface string safely using the kernel ABI fixed 20-byte buffer.
    pub fn read_clevo_interface_string(&self) -> Result<String, IoctlError> {
        self.read_str(requests::R_CL_HW_IF_STR, Self::VERSION_BUFFER_LEN)
    }

    /// Read a fixed-length C string (e.g., driver version or interface ID).
    pub fn read_str(&self, request: u64, max_len: usize) -> Result<String, IoctlError> {
        let mut buffer = vec![0u8; max_len.max(1)];
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), request, buffer.as_mut_ptr()) };
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        // Safely extract null-terminated string or convert full valid UTF-8 slice without panicking
        let s = match CStr::from_bytes_until_nul(&buffer) {
            Ok(c) => c.to_str()?.to_string(),
            Err(_) => std::str::from_utf8(&buffer)?.trim_end_matches('\0').to_string(),
        };
        Ok(s)
    }

    /// Low-level raw ioctl invocation with caller-managed buffer pointers and invariants.
    ///
    /// # Safety
    /// Caller must guarantee `arg` points to valid memory corresponding to `request` ABI requirements.
    pub unsafe fn raw_ioctl(&self, request: u64, arg: *mut libc::c_void) -> Result<i32, IoctlError> {
        let ret = libc::ioctl(self.file.as_raw_fd(), request, arg);
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(ret)
    }

    /// Checks if this device instance is open and valid.
    pub fn is_available(&self) -> bool {
        self.file.as_raw_fd() >= 0
    }

    /// Checks whether the character device file exists on the filesystem.
    pub fn is_device_available(path: impl AsRef<Path>) -> bool {
        path.as_ref().exists()
    }

    // ========================================================================
    // Safe High-Level Wrappers with Strict Preconditions (Fail-Closed)
    // ========================================================================

    /// Sets Clevo fan speed with fail-closed bounds checking.
    pub fn set_clevo_fan_speed(&self, fan_idx: usize, percent: u8) -> Result<(), IoctlError> {
        set_clevo_fan_speed(self, fan_idx, percent)
    }

    /// Sets Clevo fans to automatic control.
    pub fn set_clevo_fans_auto(&self) -> Result<(), IoctlError> {
        set_clevo_fans_auto(self)
    }

    /// Sets Uniwill fan speed with fail-closed bounds checking.
    pub fn set_uniwill_fan_speed(&self, fan_idx: usize, percent: u8) -> Result<(), IoctlError> {
        set_uniwill_fan_speed(self, fan_idx, percent)
    }

    /// Sets Uniwill fans to automatic control mode.
    pub fn set_uniwill_fans_auto(&self) -> Result<(), IoctlError> {
        set_uniwill_fans_auto(self)
    }

    /// Sets Uniwill TDP limit with hardware boundary validation.
    pub fn set_uniwill_tdp(&self, index: usize, watts: i32, limits: Option<(i32, i32)>) -> Result<(), IoctlError> {
        set_uniwill_tdp(self, index, watts, limits)
    }

    /// Enables or disables Uniwill performance mode control.
    pub fn set_uniwill_mode_enable(&self, enable: bool) -> Result<(), IoctlError> {
        set_uniwill_mode_enable(self, enable)
    }

    /// Sets Uniwill mode index.
    pub fn set_uniwill_mode(&self, mode: i32) -> Result<(), IoctlError> {
        set_uniwill_mode(self, mode)
    }

    /// Sets performance profile code depending on hardware vendor.
    pub fn set_perf_profile(&self, vendor: crate::detection::DeviceVendor, profile_code: i32) -> Result<(), IoctlError> {
        set_perf_profile(self, vendor, profile_code)
    }
}

impl crate::traits::HardwareIo for TuxedoIoDevice {
    fn read_i32(&self, request: u64) -> Result<i32, IoctlError> {
        self.read_i32(request)
    }

    fn write_i32(&self, request: u64, val: i32) -> Result<(), IoctlError> {
        self.write_i32(request, val)
    }

    fn call_none(&self, request: u64) -> Result<(), IoctlError> {
        self.call_none(request)
    }

    fn read_str(&self, request: u64, max_len: usize) -> Result<String, IoctlError> {
        self.read_str(request, max_len)
    }

    fn is_available(&self) -> bool {
        self.is_available()
    }
}

// ========================================================================
// Generic HardwareIo Safe Fail-Closed Wrappers
// ========================================================================

use crate::traits::HardwareIo;

/// Sets Clevo fan speed with fail-closed bounds checking.
pub fn set_clevo_fan_speed(hw: &dyn HardwareIo, fan_idx: usize, percent: u8) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if fan_idx >= 3 {
        return Err(IoctlError::PreconditionFailed("Clevo fan index must be 0..=2"));
    }
    if percent > 100 {
        return Err(IoctlError::PreconditionFailed("Fan speed percentage cannot exceed 100"));
    }
    // Argument encoding: (fan_idx << 8) | percent
    let arg = ((fan_idx as i32) << 8) | (percent as i32);
    hw.write_i32(requests::W_CL_FANSPEED, arg)
}

/// Sets Clevo fans to automatic control.
pub fn set_clevo_fans_auto(hw: &dyn HardwareIo) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    let mut argument = 0i32;
    argument |= 1;
    argument |= 1 << 0x01;
    argument |= 1 << 0x02;
    argument |= 1 << 0x03;
    hw.write_i32(requests::W_CL_FANAUTO, argument)
}

/// Sets Uniwill fan speed with fail-closed bounds checking.
pub fn set_uniwill_fan_speed(hw: &dyn HardwareIo, fan_idx: usize, percent: u8) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if fan_idx >= 2 {
        return Err(IoctlError::PreconditionFailed("Uniwill fan index must be 0 (CPU) or 1 (GPU)"));
    }
    if percent > 100 {
        return Err(IoctlError::PreconditionFailed("Fan speed percentage cannot exceed 100"));
    }
    let req = if fan_idx == 0 {
        requests::W_UW_FANSPEED
    } else {
        requests::W_UW_FANSPEED2
    };
    hw.write_i32(req, percent as i32)
}

/// Sets Uniwill fans to automatic control mode.
pub fn set_uniwill_fans_auto(hw: &dyn HardwareIo) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    hw.call_none(requests::W_UW_FANAUTO)
}

/// Sets Uniwill TDP limit with hardware boundary validation.
pub fn set_uniwill_tdp(
    hw: &dyn HardwareIo,
    index: usize,
    watts: i32,
    limits: Option<(i32, i32)>,
) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if index > 2 {
        return Err(IoctlError::PreconditionFailed("Uniwill TDP index must be 0..=2 (PL1, PL2, PL4)"));
    }
    if let Some((min, max)) = limits {
        if min > max || !(min..=max).contains(&watts) {
            return Err(IoctlError::PreconditionFailed("TDP wattage is outside validated hardware boundary limits"));
        }
    } else if !(0..=500).contains(&watts) {
        return Err(IoctlError::PreconditionFailed("TDP wattage is outside safe physical envelope"));
    }

    let req = match index {
        0 => requests::W_UW_TDP0,
        1 => requests::W_UW_TDP1,
        2 => requests::W_UW_TDP2,
        _ => unreachable!(),
    };
    hw.write_i32(req, watts)
}

/// Enables or disables Uniwill performance mode control.
pub fn set_uniwill_mode_enable(hw: &dyn HardwareIo, enable: bool) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    hw.write_i32(requests::W_UW_MODE_ENABLE, if enable { 1 } else { 0 })
}

/// Sets Uniwill mode index.
pub fn set_uniwill_mode(hw: &dyn HardwareIo, mode: i32) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if mode < 0 {
        return Err(IoctlError::PreconditionFailed("Uniwill mode index cannot be negative"));
    }
    hw.write_i32(requests::W_UW_MODE, mode)
}

/// Sets performance profile code depending on hardware vendor.
pub fn set_perf_profile(
    hw: &dyn HardwareIo,
    vendor: crate::detection::DeviceVendor,
    profile_code: i32,
) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    match vendor {
        crate::detection::DeviceVendor::Clevo => {
            hw.write_i32(requests::W_CL_PERF_PROFILE, profile_code)
        }
        crate::detection::DeviceVendor::Uniwill => {
            hw.write_i32(requests::W_UW_PERF_PROF, profile_code)
        }
        crate::detection::DeviceVendor::Unsupported => {
            Err(IoctlError::PreconditionFailed("Cannot set performance profile on unsupported hardware"))
        }
    }
}

/// Reads Uniwill fan speed (RPM or percentage).
pub fn read_uniwill_fan_speed(hw: &dyn HardwareIo, fan_idx: usize) -> Result<i32, IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    match fan_idx {
        0 => hw.read_i32(requests::R_UW_FANSPEED),
        1 => hw.read_i32(requests::R_UW_FANSPEED2),
        _ => Err(IoctlError::PreconditionFailed("Uniwill fan index must be 0 (CPU) or 1 (GPU)")),
    }
}

/// Reads Uniwill fan temperature in degrees C.
pub fn read_uniwill_fan_temp(hw: &dyn HardwareIo, fan_idx: usize) -> Result<i32, IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    match fan_idx {
        0 => hw.read_i32(requests::R_UW_FAN_TEMP),
        1 => hw.read_i32(requests::R_UW_FAN_TEMP2),
        _ => Err(IoctlError::PreconditionFailed("Uniwill fan index must be 0 (CPU) or 1 (GPU)")),
    }
}

/// Reads Uniwill TDP limits (min, max) for given index.
pub fn read_uniwill_tdp_limits(hw: &dyn HardwareIo, index: usize) -> Result<(i32, i32), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    let (req_min, req_max) = match index {
        0 => (requests::R_UW_TDP0_MIN, requests::R_UW_TDP0_MAX),
        1 => (requests::R_UW_TDP1_MIN, requests::R_UW_TDP1_MAX),
        2 => (requests::R_UW_TDP2_MIN, requests::R_UW_TDP2_MAX),
        _ => return Err(IoctlError::PreconditionFailed("Uniwill TDP index must be 0..=2")),
    };
    let min = hw.read_i32(req_min)?;
    let max = hw.read_i32(req_max)?;
    Ok((min, max))
}
