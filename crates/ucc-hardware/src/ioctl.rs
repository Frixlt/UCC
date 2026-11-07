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
}
