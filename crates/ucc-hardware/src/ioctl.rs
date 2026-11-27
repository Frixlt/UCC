//! Linux `/dev/tuxedo_io` ioctl request definitions and safe abstraction.
//!
//! Note on ABI: The Tuxedo IO driver historically defines `_IOR` / `_IOW` using pointer types
//! (e.g. `int32_t*`, `char*`), so the size encoded in the ioctl request number on 64-bit systems
//! is `std::mem::size_of::<*const ()>()` (8 bytes), NOT `4`. This module strictly preserves this
//! kernel ABI.

#[cfg(unix)]
use nix::libc;
use std::ffi::CStr;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
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

/// Validate that an ioctl request belongs to the canonical Tuxedo IO read_i32 requests.
fn is_valid_tuxedo_read_i32(request: u64) -> bool {
    if request > u32::MAX as u64 {
        return false;
    }
    let dir = (request >> 30) & 3;
    let size = (request >> 16) & 0x3FFF;
    if dir != IOC_READ as u64 || size != 8 {
        return false;
    }
    matches!(
        request,
        requests::R_HWCHECK_CL
            | requests::R_HWCHECK_UW
            | requests::R_CL_FANINFO1
            | requests::R_CL_FANINFO2
            | requests::R_CL_FANINFO3
            | requests::R_CL_WEBCAM_SW
            | requests::R_CL_FLIGHTMODE_SW
            | requests::R_CL_TOUCHPAD_SW
            | requests::R_UW_MODEL_ID
            | requests::R_UW_FANSPEED
            | requests::R_UW_FANSPEED2
            | requests::R_UW_FAN_TEMP
            | requests::R_UW_FAN_TEMP2
            | requests::R_UW_MODE
            | requests::R_UW_MODE_ENABLE
            | requests::R_UW_FANS_OFF_AVAILABLE
            | requests::R_UW_FANS_MIN_SPEED
            | requests::R_UW_TDP0
            | requests::R_UW_TDP1
            | requests::R_UW_TDP2
            | requests::R_UW_TDP0_MIN
            | requests::R_UW_TDP1_MIN
            | requests::R_UW_TDP2_MIN
            | requests::R_UW_TDP0_MAX
            | requests::R_UW_TDP1_MAX
            | requests::R_UW_TDP2_MAX
            | requests::R_UW_PROFS_AVAILABLE
    )
}

/// Validate that an ioctl request belongs to the canonical Tuxedo IO write_i32 requests.
fn is_valid_tuxedo_write_i32(request: u64) -> bool {
    if request > u32::MAX as u64 {
        return false;
    }
    let dir = (request >> 30) & 3;
    let size = (request >> 16) & 0x3FFF;
    if dir != IOC_WRITE as u64 || size != 8 {
        return false;
    }
    matches!(
        request,
        requests::W_CL_FANSPEED
            | requests::W_CL_FANAUTO
            | requests::W_CL_WEBCAM_SW
            | requests::W_CL_FLIGHTMODE_SW
            | requests::W_CL_TOUCHPAD_SW
            | requests::W_CL_PERF_PROFILE
            | requests::W_UW_FANSPEED
            | requests::W_UW_FANSPEED2
            | requests::W_UW_MODE
            | requests::W_UW_MODE_ENABLE
            | requests::W_UW_TDP0
            | requests::W_UW_TDP1
            | requests::W_UW_TDP2
            | requests::W_UW_PERF_PROF
    )
}

/// Validate that an ioctl request belongs to the canonical Tuxedo IO call_none requests.
fn is_valid_tuxedo_call_none(request: u64) -> bool {
    if request > u32::MAX as u64 {
        return false;
    }
    let dir = (request >> 30) & 3;
    let size = (request >> 16) & 0x3FFF;
    if dir != IOC_NONE as u64 || size != 0 {
        return false;
    }
    matches!(request, requests::W_UW_FANAUTO)
}

/// Validate that an ioctl request belongs to the canonical Tuxedo IO read_str requests.
fn is_valid_tuxedo_read_str(request: u64) -> bool {
    if request > u32::MAX as u64 {
        return false;
    }
    let dir = (request >> 30) & 3;
    let size = (request >> 16) & 0x3FFF;
    if dir != IOC_READ as u64 || size != 8 {
        return false;
    }
    matches!(
        request,
        requests::R_MOD_VERSION | requests::R_CL_HW_IF_STR | requests::R_UW_HW_IF_STR
    )
}

/// Typed wrapper around a character device like `/dev/tuxedo_io`.
pub struct TuxedoIoDevice {
    file: File,
    path: PathBuf,
}

impl TuxedoIoDevice {
    pub const DEFAULT_PATH: &'static str = "/dev/tuxedo_io";

    /// Opens the device at the given path.
    #[cfg(unix)]
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

    #[cfg(not(unix))]
    pub fn open(_path: impl AsRef<Path>) -> Result<Self, IoctlError> {
        Err(IoctlError::DeviceUnavailable)
    }

    /// Returns the device path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read an `i32` value via ioctl.
    ///
    /// # Safety
    /// Caller must ensure that `request` is a valid ioctl request code whose ABI expects
    /// a pointer to an `int32_t`.
    #[cfg(unix)]
    pub unsafe fn read_i32(&self, request: u64) -> Result<i32, IoctlError> {
        if !is_valid_tuxedo_read_i32(request) {
            return Err(IoctlError::PreconditionFailed(
                "Cannot invoke read_i32 with non-read or unsupported ioctl request",
            ));
        }

        let mut val: i32 = 0;
        let ret = libc::ioctl(self.file.as_raw_fd(), request, &mut val);
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(val)
    }

    #[cfg(not(unix))]
    pub unsafe fn read_i32(&self, _request: u64) -> Result<i32, IoctlError> {
        Err(IoctlError::DeviceUnavailable)
    }

    /// Write an `i32` value via ioctl.
    ///
    /// # Safety
    /// Caller must ensure that `request` is a valid ioctl request code whose ABI expects
    /// a pointer to an `int32_t`.
    #[cfg(unix)]
    pub unsafe fn write_i32(&self, request: u64, mut val: i32) -> Result<(), IoctlError> {
        if !is_valid_tuxedo_write_i32(request) {
            return Err(IoctlError::PreconditionFailed(
                "Cannot invoke write_i32 with non-write or unsupported ioctl request",
            ));
        }
        let ret = libc::ioctl(self.file.as_raw_fd(), request, &mut val);
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub unsafe fn write_i32(&self, _request: u64, _val: i32) -> Result<(), IoctlError> {
        Err(IoctlError::DeviceUnavailable)
    }

    /// Execute an ioctl without arguments (such as `W_UW_FANAUTO`).
    ///
    /// # Safety
    /// Caller must ensure that `request` is an `_IO` request code taking no arguments.
    #[cfg(unix)]
    pub unsafe fn call_none(&self, request: u64) -> Result<(), IoctlError> {
        if !is_valid_tuxedo_call_none(request) {
            return Err(IoctlError::PreconditionFailed(
                "Cannot invoke call_none with argument-bearing or unsupported ioctl request",
            ));
        }
        let ret = libc::ioctl(self.file.as_raw_fd(), request);
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub unsafe fn call_none(&self, _request: u64) -> Result<(), IoctlError> {
        Err(IoctlError::DeviceUnavailable)
    }

    /// Maximum buffer size for module and interface version strings per kernel ABI (20 bytes).
    pub const VERSION_BUFFER_LEN: usize = 20;

    /// Clevo kernel driver interface string buffer length (50 bytes).
    pub const CLEVO_IF_STR_BUFFER_LEN: usize = 50;

    /// Reads driver module version safely using the kernel ABI fixed 20-byte buffer.
    pub fn read_module_version(&self) -> Result<String, IoctlError> {
        unsafe { self.read_str(requests::R_MOD_VERSION, Self::VERSION_BUFFER_LEN) }
    }

    /// Reads Uniwill hardware interface string safely using the kernel ABI fixed 20-byte buffer.
    pub fn read_uniwill_interface_string(&self) -> Result<String, IoctlError> {
        unsafe { self.read_str(requests::R_UW_HW_IF_STR, Self::VERSION_BUFFER_LEN) }
    }

    /// Reads Clevo hardware interface string safely using the kernel ABI fixed 50-byte buffer.
    pub fn read_clevo_interface_string(&self) -> Result<String, IoctlError> {
        unsafe { self.read_str(requests::R_CL_HW_IF_STR, Self::CLEVO_IF_STR_BUFFER_LEN) }
    }

    /// Read a fixed-length C string (e.g., driver version or interface ID).
    ///
    /// # Safety
    /// Caller must ensure `request` expects a char buffer of at least `max_len` bytes.
    #[cfg(unix)]
    pub unsafe fn read_str(&self, request: u64, max_len: usize) -> Result<String, IoctlError> {
        if !is_valid_tuxedo_read_str(request) {
            return Err(IoctlError::PreconditionFailed(
                "Cannot invoke read_str with non-string or unsupported ioctl request",
            ));
        }
        let required_len = if request == requests::R_CL_HW_IF_STR {
            Self::CLEVO_IF_STR_BUFFER_LEN
        } else {
            Self::VERSION_BUFFER_LEN
        };
        if max_len < required_len {
            return Err(IoctlError::PreconditionFailed(
                "Buffer length is smaller than kernel ABI requires",
            ));
        }

        let mut buffer = vec![0u8; max_len.max(1)];
        let ret = libc::ioctl(self.file.as_raw_fd(), request, buffer.as_mut_ptr());
        if ret < 0 {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::last(),
            });
        }
        let c_str = CStr::from_bytes_until_nul(&buffer).map_err(|_| {
            IoctlError::PreconditionFailed("String from ioctl is missing NUL terminator")
        })?;
        Ok(c_str.to_str()?.to_string())
    }

    #[cfg(not(unix))]
    pub unsafe fn read_str(&self, _request: u64, _max_len: usize) -> Result<String, IoctlError> {
        let _ = CStr::from_bytes_until_nul(b"\0");
        Err(IoctlError::DeviceUnavailable)
    }

    /// Low-level raw ioctl invocation with caller-managed buffer pointers and invariants.
    ///
    /// # Safety
    /// Caller must guarantee `arg` points to valid memory corresponding to `request` ABI requirements.
    #[cfg(unix)]
    pub unsafe fn raw_ioctl(
        &self,
        request: u64,
        arg: *mut libc::c_void,
    ) -> Result<i32, IoctlError> {
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
    #[cfg(unix)]
    pub fn is_available(&self) -> bool {
        let _ = &self.file;
        self.file.as_raw_fd() >= 0
    }

    #[cfg(not(unix))]
    pub fn is_available(&self) -> bool {
        let _ = &self.file;
        false
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
    pub fn set_uniwill_tdp(
        &self,
        index: usize,
        watts: i32,
        limits: Option<(i32, i32)>,
    ) -> Result<(), IoctlError> {
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
    pub fn set_perf_profile(
        &self,
        vendor: crate::detection::DeviceVendor,
        profile_code: i32,
    ) -> Result<(), IoctlError> {
        set_perf_profile(self, vendor, profile_code)
    }
}

impl crate::traits::HardwareIo for TuxedoIoDevice {
    fn read_i32(&self, request: u64) -> Result<i32, IoctlError> {
        unsafe { self.read_i32(request) }
    }

    fn write_i32(&self, request: u64, val: i32) -> Result<(), IoctlError> {
        unsafe { self.write_i32(request, val) }
    }

    fn call_none(&self, request: u64) -> Result<(), IoctlError> {
        unsafe { self.call_none(request) }
    }

    fn read_str(&self, request: u64, max_len: usize) -> Result<String, IoctlError> {
        unsafe { self.read_str(request, max_len) }
    }

    fn is_available(&self) -> bool {
        self.is_available()
    }
}

// ========================================================================
// Generic HardwareIo Safe Fail-Closed Wrappers
// ========================================================================

use crate::traits::HardwareIo;

/// Sets Clevo fan speed with fail-closed bounds checking and 3-fan payload packing per C++ clevo.h:72-101.
pub fn set_clevo_fan_speed(
    hw: &dyn HardwareIo,
    fan_idx: usize,
    percent: u8,
) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if fan_idx >= 3 {
        return Err(IoctlError::PreconditionFailed(
            "Clevo fan index must be 0..=2",
        ));
    }
    if percent > 100 {
        return Err(IoctlError::PreconditionFailed(
            "Fan speed percentage cannot exceed 100",
        ));
    }

    let mut fan_speeds_raw = [0i32; 3];
    for (i, slot) in fan_speeds_raw.iter_mut().enumerate() {
        if i == fan_idx {
            *slot = (percent as f64 * 255.0 / 100.0).round() as i32;
        } else {
            let req = match i {
                0 => requests::R_CL_FANINFO1,
                1 => requests::R_CL_FANINFO2,
                2 => requests::R_CL_FANINFO3,
                _ => unreachable!(),
            };
            let info = hw.read_i32(req)?;
            *slot = info & 0xff;
        }
    }

    let val = (fan_speeds_raw[0] & 0xff)
        | ((fan_speeds_raw[1] & 0xff) << 8)
        | ((fan_speeds_raw[2] & 0xff) << 16);

    hw.write_i32(requests::W_CL_FANSPEED, val)
}

/// Reads Clevo fan speed as a percentage (0..=100) per C++ clevo.h:103-112.
pub fn read_clevo_fan_speed_percent(hw: &dyn HardwareIo, fan_idx: usize) -> Result<u8, IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if fan_idx >= 3 {
        return Err(IoctlError::PreconditionFailed(
            "Clevo fan index must be 0..=2",
        ));
    }
    let req = match fan_idx {
        0 => requests::R_CL_FANINFO1,
        1 => requests::R_CL_FANINFO2,
        2 => requests::R_CL_FANINFO3,
        _ => unreachable!(),
    };
    let info = hw.read_i32(req)?;
    let raw = info & 0xff;
    let pct = (raw as f64 * 100.0 / 255.0).round() as u8;
    Ok(pct.min(100))
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

/// Sets Uniwill fan speed with fail-closed bounds checking and 200 raw scaling per C++ uniwill.h:95-117.
pub fn set_uniwill_fan_speed(
    hw: &dyn HardwareIo,
    fan_idx: usize,
    percent: u8,
) -> Result<(), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    if fan_idx >= 2 {
        return Err(IoctlError::PreconditionFailed(
            "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
        ));
    }
    if percent > 100 {
        return Err(IoctlError::PreconditionFailed(
            "Fan speed percentage cannot exceed 100",
        ));
    }
    let raw = (200.0 * percent as f64 / 100.0).round() as i32;
    let req = if fan_idx == 0 {
        requests::W_UW_FANSPEED
    } else {
        requests::W_UW_FANSPEED2
    };
    hw.write_i32(req, raw)
}

/// Reads Uniwill fan speed as a percentage (0..=100) per C++ uniwill.h:119-149.
pub fn read_uniwill_fan_speed_percent(
    hw: &dyn HardwareIo,
    fan_idx: usize,
) -> Result<u8, IoctlError> {
    let raw = read_uniwill_fan_speed(hw, fan_idx)?;
    if raw <= 0 {
        Ok(0)
    } else {
        let pct = ((raw as f64) * 100.0 / 200.0).ceil() as u8;
        Ok(pct.min(100))
    }
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
        return Err(IoctlError::PreconditionFailed(
            "Uniwill TDP index must be 0..=2 (PL1, PL2, PL4)",
        ));
    }
    let d_lim = read_uniwill_tdp_limits(hw, index)?;
    if d_lim.0 <= 0 || d_lim.1 <= 0 || d_lim.0 > d_lim.1 {
        return Err(IoctlError::PreconditionFailed(
            "Hardware device reported invalid TDP limits",
        ));
    }
    let (min, max) = match limits {
        Some(lim) => (lim.0.max(d_lim.0), lim.1.min(d_lim.1)),
        None => d_lim,
    };
    if min <= 0 || max <= 0 || min > max || !(min..=max).contains(&watts) {
        return Err(IoctlError::PreconditionFailed(
            "TDP wattage is outside validated device hardware limits",
        ));
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
        return Err(IoctlError::PreconditionFailed(
            "Uniwill mode index cannot be negative",
        ));
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
        crate::detection::DeviceVendor::Unsupported => Err(IoctlError::PreconditionFailed(
            "Cannot set performance profile on unsupported hardware",
        )),
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
        _ => Err(IoctlError::PreconditionFailed(
            "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
        )),
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
        _ => Err(IoctlError::PreconditionFailed(
            "Uniwill fan index must be 0 (CPU) or 1 (GPU)",
        )),
    }
}

/// Reads Uniwill TDP limits (min, max) for given index.
pub fn read_uniwill_tdp_limits(
    hw: &dyn HardwareIo,
    index: usize,
) -> Result<(i32, i32), IoctlError> {
    if !hw.is_available() {
        return Err(IoctlError::DeviceUnavailable);
    }
    let (req_min, req_max) = match index {
        0 => (requests::R_UW_TDP0_MIN, requests::R_UW_TDP0_MAX),
        1 => (requests::R_UW_TDP1_MIN, requests::R_UW_TDP1_MAX),
        2 => (requests::R_UW_TDP2_MIN, requests::R_UW_TDP2_MAX),
        _ => {
            return Err(IoctlError::PreconditionFailed(
                "Uniwill TDP index must be 0..=2",
            ))
        }
    };
    let min = hw.read_i32(req_min)?;
    let max = hw.read_i32(req_max)?;
    if min <= 0 || max <= 0 || min > max {
        return Err(IoctlError::PreconditionFailed(
            "Invalid or missing TDP hardware boundaries",
        ));
    }
    Ok((min, max))
}
