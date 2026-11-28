// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux `/dev/uwcc` UAPI ABI definitions, [`UwccTransport`] trait, real Unix device
//! wrapper [`UwccDevice`], and deterministic in-memory [`MockUwccTransport`].
//!
//! Struct layouts and ioctl request codes match `kernel/uwcc-rs/include/uapi/uwcc.h`
//! byte-for-byte.

use crate::platform::PlatformError;
#[cfg(unix)]
use nix::libc;
use std::collections::{BTreeMap, VecDeque};
#[cfg(unix)]
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Canonical `/dev/uwcc` character device path.
pub const UWCC_DEVICE_PATH: &str = "/dev/uwcc";

/// Ioctl type magic byte (`'U'`, `0x55`) matching `UWCC_IOCTL_MAGIC` in `uapi/uwcc.h`.
pub const UWCC_MAGIC: u8 = b'U';

/// Current UAPI protocol version (`UWCC_API_VERSION` in `uapi/uwcc.h`).
pub const UWCC_API_VERSION: u32 = 1;

/// Maximum SMRW byte length per ioctl (`UWCC_SMRW_MAX_LEN` in `uapi/uwcc.h`).
pub const UWCC_SMRW_MAX_LEN: usize = 64;

/// Kernel WMI ring buffer capacity (`UWCC_WMI_RING_CAPACITY` in `uapi/uwcc.h`).
pub const UWCC_WMI_RING_CAPACITY: usize = 64;

/// Maximum valid `PWUP` boost level (`UWCC_PWUP_MAX_LEVEL` in `uapi/uwcc.h`).
pub const UWCC_PWUP_MAX_LEVEL: u32 = 7;

/// Capability bitmask flags returned in [`UwccInfo::flags`].
pub const UWCC_FLAG_HAS_EC: u32 = 1 << 0;
pub const UWCC_FLAG_HAS_SMRW: u32 = 1 << 1;
pub const UWCC_FLAG_HAS_PWUP: u32 = 1 << 2;
pub const UWCC_FLAG_HAS_WMI_EVENTS: u32 = 1 << 3;
pub const UWCC_FLAG_HAS_HWMON: u32 = 1 << 4;
pub const UWCC_FLAG_HAS_PWBT: u32 = 1 << 5;
pub const UWCC_FLAG_HAS_PWDB: u32 = 1 << 6;
pub const UWCC_FLAG_HAS_MUX: u32 = 1 << 7;

/// Operations for `UWCC_IOC_GPU_POWER` (NVIDIA NPCF `PWBT` / `PWDB`).
pub const UWCC_GPU_POWER_OP_BASE_TGP: u32 = 1;
pub const UWCC_GPU_POWER_OP_DYNAMIC_BOOST: u32 = 2;

/// Display MUX state constants matching `kernel/uwcc-rs/include/uapi/uwcc.h`.
pub const UWCC_MUX_ACTIVE_UNKNOWN: u32 = 0;
pub const UWCC_MUX_ACTIVE_IGPU: u32 = 1;
pub const UWCC_MUX_ACTIVE_DGPU: u32 = 2;

pub const UWCC_MUX_MODE_UNKNOWN: u32 = 0;
pub const UWCC_MUX_MODE_DYNAMIC: u32 = 1;
pub const UWCC_MUX_MODE_DGPU_ONLY: u32 = 2;
pub const UWCC_MUX_MODE_IGPU_ONLY: u32 = 3;
pub const UWCC_MUX_MODE_MSHYBRID: u32 = 4;

/// Ioctl sequence numbers matching `kernel/uwcc-rs/include/uapi/uwcc.h`.
pub const UWCC_NR_GET_INFO: u8 = 0x01;
pub const UWCC_NR_EC_READ: u8 = 0x02;
pub const UWCC_NR_EC_WRITE: u8 = 0x03;
pub const UWCC_NR_SMRW: u8 = 0x04;
pub const UWCC_NR_PWUP: u8 = 0x05;
pub const UWCC_NR_GPU_POWER: u8 = 0x06;
pub const UWCC_NR_MUX_GET: u8 = 0x07;
pub const UWCC_NR_MUX_SET: u8 = 0x08;

/// Single EC register read/write payload (`struct uwcc_ec_reg`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UwccEcReg {
    pub addr: u16,
    pub val: u8,
    pub _pad: u8,
}

/// Factory power/thermal limits table (`struct uwcc_smrw`, 68 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UwccSmrw {
    pub offset: u16,
    pub len: u16,
    pub data: [u8; UWCC_SMRW_MAX_LEN],
}

impl Default for UwccSmrw {
    fn default() -> Self {
        Self {
            offset: 0,
            len: 0,
            data: [0u8; UWCC_SMRW_MAX_LEN],
        }
    }
}

/// Liquid-cooler power boost payload (`struct uwcc_pwup`, 8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UwccPwup {
    pub enable: u32,
    pub level: u32,
}

/// Driver and platform capability descriptor (`struct uwcc_info`, 16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UwccInfo {
    pub api_version: u32,
    pub project_id: u32,
    pub ec_version: u32,
    pub flags: u32,
}

/// Hardware WMI event record returned by `read()` on `/dev/uwcc` (`struct uwcc_wmi_event`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UwccWmiEvent {
    pub timestamp_ns: u64,
    pub code: u32,
    pub payload: u32,
}

/// NVIDIA NPCF GPU power limit payload (`struct uwcc_gpu_power`, 8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UwccGpuPower {
    pub op: u32,
    pub value: u32,
}

/// Hardware display MUX state/mode payload (`struct uwcc_mux_state`, 8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UwccMuxState {
    pub active_gpu: u32,
    pub boot_mode: u32,
}

const _: () = assert!(std::mem::size_of::<UwccEcReg>() == 4);
const _: () = assert!(std::mem::size_of::<UwccSmrw>() == 68);
const _: () = assert!(std::mem::size_of::<UwccPwup>() == 8);
const _: () = assert!(std::mem::size_of::<UwccInfo>() == 16);
const _: () = assert!(std::mem::size_of::<UwccWmiEvent>() == 16);
const _: () = assert!(std::mem::size_of::<UwccGpuPower>() == 8);
const _: () = assert!(std::mem::size_of::<UwccMuxState>() == 8);

#[cfg(unix)]
pub const UWCC_IOC_GET_INFO: u64 = nix::request_code_read!(
    UWCC_MAGIC,
    UWCC_NR_GET_INFO,
    std::mem::size_of::<UwccInfo>()
) as u64;

#[cfg(unix)]
pub const UWCC_IOC_EC_READ: u64 = nix::request_code_readwrite!(
    UWCC_MAGIC,
    UWCC_NR_EC_READ,
    std::mem::size_of::<UwccEcReg>()
) as u64;

#[cfg(unix)]
pub const UWCC_IOC_EC_WRITE: u64 = nix::request_code_write!(
    UWCC_MAGIC,
    UWCC_NR_EC_WRITE,
    std::mem::size_of::<UwccEcReg>()
) as u64;

#[cfg(unix)]
pub const UWCC_IOC_SMRW: u64 =
    nix::request_code_readwrite!(UWCC_MAGIC, UWCC_NR_SMRW, std::mem::size_of::<UwccSmrw>()) as u64;

#[cfg(unix)]
pub const UWCC_IOC_PWUP: u64 =
    nix::request_code_write!(UWCC_MAGIC, UWCC_NR_PWUP, std::mem::size_of::<UwccPwup>()) as u64;

#[cfg(unix)]
pub const UWCC_IOC_GPU_POWER: u64 = nix::request_code_write!(
    UWCC_MAGIC,
    UWCC_NR_GPU_POWER,
    std::mem::size_of::<UwccGpuPower>()
) as u64;

#[cfg(unix)]
pub const UWCC_IOC_MUX_GET: u64 = nix::request_code_read!(
    UWCC_MAGIC,
    UWCC_NR_MUX_GET,
    std::mem::size_of::<UwccMuxState>()
) as u64;

#[cfg(unix)]
pub const UWCC_IOC_MUX_SET: u64 = nix::request_code_write!(
    UWCC_MAGIC,
    UWCC_NR_MUX_SET,
    std::mem::size_of::<UwccMuxState>()
) as u64;

#[cfg(not(unix))]
const fn linux_ioc(dir: u32, ty: u8, nr: u8, size: usize) -> u64 {
    ((dir as u64) << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | (nr as u64)
}

#[cfg(not(unix))]
pub const UWCC_IOC_GET_INFO: u64 = linux_ioc(
    2,
    UWCC_MAGIC,
    UWCC_NR_GET_INFO,
    std::mem::size_of::<UwccInfo>(),
);

#[cfg(not(unix))]
pub const UWCC_IOC_EC_READ: u64 = linux_ioc(
    3,
    UWCC_MAGIC,
    UWCC_NR_EC_READ,
    std::mem::size_of::<UwccEcReg>(),
);

#[cfg(not(unix))]
pub const UWCC_IOC_EC_WRITE: u64 = linux_ioc(
    1,
    UWCC_MAGIC,
    UWCC_NR_EC_WRITE,
    std::mem::size_of::<UwccEcReg>(),
);

#[cfg(not(unix))]
pub const UWCC_IOC_SMRW: u64 =
    linux_ioc(3, UWCC_MAGIC, UWCC_NR_SMRW, std::mem::size_of::<UwccSmrw>());

#[cfg(not(unix))]
pub const UWCC_IOC_PWUP: u64 =
    linux_ioc(1, UWCC_MAGIC, UWCC_NR_PWUP, std::mem::size_of::<UwccPwup>());

#[cfg(not(unix))]
pub const UWCC_IOC_GPU_POWER: u64 = linux_ioc(
    1,
    UWCC_MAGIC,
    UWCC_NR_GPU_POWER,
    std::mem::size_of::<UwccGpuPower>(),
);

#[cfg(not(unix))]
pub const UWCC_IOC_MUX_GET: u64 = linux_ioc(
    2,
    UWCC_MAGIC,
    UWCC_NR_MUX_GET,
    std::mem::size_of::<UwccMuxState>(),
);

#[cfg(not(unix))]
pub const UWCC_IOC_MUX_SET: u64 = linux_ioc(
    1,
    UWCC_MAGIC,
    UWCC_NR_MUX_SET,
    std::mem::size_of::<UwccMuxState>(),
);

/// Transport abstraction over `/dev/uwcc` ioctls and WMI event reads.
pub trait UwccTransport: Send + Sync {
    /// Returns `true` when the underlying `/dev/uwcc` handle is open and usable.
    fn is_available(&self) -> bool;

    /// Fetches driver ABI, chassis project ID, EC version, and capability flags.
    fn get_info(&self) -> Result<UwccInfo, PlatformError>;

    /// Reads a single EC register byte (`ECRR`).
    fn ec_read(&self, addr: u16) -> Result<u8, PlatformError>;

    /// Writes a single EC register byte (`ECRW`).
    fn ec_write(&self, addr: u16, val: u8) -> Result<(), PlatformError>;

    /// Executes an ACPI `SMRW` table read (`offset`, `len` in `1..=64`).
    fn smrw(&self, req: UwccSmrw) -> Result<UwccSmrw, PlatformError>;

    /// Configures water-cooler power-up boost (`PWUP`).
    fn pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError>;

    /// Dispatches NVIDIA NPCF GPU power limit updates (`PWBT` / `PWDB`).
    fn set_gpu_power(&self, _op: u32, _value: u32) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "GPU NPCF power ioctl unsupported",
        ))
    }

    /// Reads hardware display MUX state (`MXDS(0)`) and BIOS boot mode (`MXDM(0)`).
    fn mux_get(&self) -> Result<UwccMuxState, PlatformError> {
        Err(PlatformError::Unsupported("Display MUX unsupported"))
    }

    /// Sets persistent BIOS display MUX boot mode (`MXDM(1..=4)`, applies on next reboot).
    fn mux_set(&self, _boot_mode: u32) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("Display MUX unsupported"))
    }

    /// Non-blocking poll for the next queued WMI event.
    fn poll_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError>;
}

/// Real `/dev/uwcc` character device handle opened with `O_NOFOLLOW | O_NONBLOCK`.
pub struct UwccDevice {
    #[cfg(unix)]
    file: File,
    path: PathBuf,
}

impl UwccDevice {
    /// Opens the default `/dev/uwcc` character device.
    pub fn open_default() -> Result<Self, PlatformError> {
        Self::open(UWCC_DEVICE_PATH)
    }

    /// Opens a `/dev/uwcc`-compatible character device at `path`.
    #[cfg(unix)]
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PlatformError> {
        let path_buf = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&path_buf)
            .map_err(|source| {
                PlatformError::Ioctl(crate::ioctl::IoctlError::OpenDevice {
                    path: path_buf.clone(),
                    source,
                })
            })?;
        Ok(Self {
            file,
            path: path_buf,
        })
    }

    #[cfg(not(unix))]
    pub fn open(_path: impl AsRef<Path>) -> Result<Self, PlatformError> {
        Err(PlatformError::Unavailable)
    }

    /// Returns the opened device path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Checks whether `/dev/uwcc` exists on the filesystem.
    pub fn default_device_exists() -> bool {
        Path::new(UWCC_DEVICE_PATH).exists()
    }
}

#[cfg(unix)]
impl UwccTransport for UwccDevice {
    fn is_available(&self) -> bool {
        self.file.as_raw_fd() >= 0
    }

    fn get_info(&self) -> Result<UwccInfo, PlatformError> {
        let mut info = UwccInfo::default();
        // SAFETY: `UWCC_IOC_GET_INFO` expects a valid `*mut UwccInfo` per `uapi/uwcc.h`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_GET_INFO as _,
                &mut info as *mut UwccInfo,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_GET_INFO,
                source,
            })
        })?;
        Ok(info)
    }

    fn ec_read(&self, addr: u16) -> Result<u8, PlatformError> {
        let mut reg = UwccEcReg {
            addr,
            val: 0,
            _pad: 0,
        };
        // SAFETY: `UWCC_IOC_EC_READ` expects a valid `*mut UwccEcReg`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_EC_READ as _,
                &mut reg as *mut UwccEcReg,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_EC_READ,
                source,
            })
        })?;
        Ok(reg.val)
    }

    fn ec_write(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
        let mut reg = UwccEcReg { addr, val, _pad: 0 };
        // SAFETY: `UWCC_IOC_EC_WRITE` expects a valid `*mut UwccEcReg`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_EC_WRITE as _,
                &mut reg as *mut UwccEcReg,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_EC_WRITE,
                source,
            })
        })?;
        Ok(())
    }

    fn smrw(&self, mut req: UwccSmrw) -> Result<UwccSmrw, PlatformError> {
        if req.len == 0 || usize::from(req.len) > UWCC_SMRW_MAX_LEN {
            return Err(PlatformError::InvalidArgument(
                "SMRW length must be 1..=64 bytes",
            ));
        }
        // SAFETY: `UWCC_IOC_SMRW` expects a valid `*mut UwccSmrw`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_SMRW as _,
                &mut req as *mut UwccSmrw,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_SMRW,
                source,
            })
        })?;
        Ok(req)
    }

    fn pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
        if level > UWCC_PWUP_MAX_LEVEL || (!enable && level != 0) {
            return Err(PlatformError::InvalidArgument(
                "PWUP level must be 0..=7 (and 0 when disabled)",
            ));
        }
        let mut payload = UwccPwup {
            enable: u32::from(enable),
            level,
        };
        // SAFETY: `UWCC_IOC_PWUP` expects a valid `*mut UwccPwup`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_PWUP as _,
                &mut payload as *mut UwccPwup,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_PWUP,
                source,
            })
        })?;
        Ok(())
    }

    fn set_gpu_power(&self, op: u32, value: u32) -> Result<(), PlatformError> {
        match op {
            UWCC_GPU_POWER_OP_BASE_TGP if value <= 300 => {}
            UWCC_GPU_POWER_OP_DYNAMIC_BOOST if value <= 100 => {}
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Invalid NVIDIA NPCF GPU power operation or watt bound",
                ));
            }
        }
        let mut req = UwccGpuPower { op, value };
        // SAFETY: `UWCC_IOC_GPU_POWER` expects a valid `*mut UwccGpuPower`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_GPU_POWER as _,
                &mut req as *mut UwccGpuPower,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_GPU_POWER,
                source,
            })
        })?;
        Ok(())
    }

    fn mux_get(&self) -> Result<UwccMuxState, PlatformError> {
        let mut state = UwccMuxState::default();
        // SAFETY: `UWCC_IOC_MUX_GET` expects a valid `*mut UwccMuxState`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_MUX_GET as _,
                &mut state as *mut UwccMuxState,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_MUX_GET,
                source,
            })
        })?;
        Ok(state)
    }

    fn mux_set(&self, boot_mode: u32) -> Result<(), PlatformError> {
        if !(UWCC_MUX_MODE_DYNAMIC..=UWCC_MUX_MODE_MSHYBRID).contains(&boot_mode) {
            return Err(PlatformError::InvalidArgument(
                "MUX boot mode must be 1..=4",
            ));
        }
        let mut req = UwccMuxState {
            active_gpu: 0,
            boot_mode,
        };
        // SAFETY: `UWCC_IOC_MUX_SET` expects a valid `*mut UwccMuxState`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                UWCC_IOC_MUX_SET as _,
                &mut req as *mut UwccMuxState,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: UWCC_IOC_MUX_SET,
                source,
            })
        })?;
        Ok(())
    }

    fn poll_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
        let mut evt = UwccWmiEvent::default();
        let size = std::mem::size_of::<UwccWmiEvent>();
        // SAFETY: `evt` is a valid writable `#[repr(C)]` struct of `size` bytes.
        let n = match crate::ioctl::retry_eintr_ssize(|| unsafe {
            libc::read(
                self.file.as_raw_fd(),
                (&mut evt as *mut UwccWmiEvent).cast::<libc::c_void>(),
                size,
            )
        }) {
            Ok(n) => n,
            Err(err)
                if err == nix::errno::Errno::EAGAIN || err == nix::errno::Errno::EWOULDBLOCK =>
            {
                return Ok(None);
            }
            Err(source) => {
                return Err(PlatformError::Ioctl(
                    crate::ioctl::IoctlError::IoctlFailed { request: 0, source },
                ));
            }
        };
        if n == 0 {
            return Ok(None);
        }
        if (n as usize) < size {
            return Err(PlatformError::Io {
                context: "Short read from /dev/uwcc WMI event stream",
                source: std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("partial WMI event read: expected {} bytes, got {}", size, n),
                ),
            });
        }
        Ok(Some(evt))
    }
}

#[cfg(not(unix))]
impl UwccTransport for UwccDevice {
    fn is_available(&self) -> bool {
        false
    }

    fn get_info(&self) -> Result<UwccInfo, PlatformError> {
        Err(PlatformError::Unavailable)
    }

    fn ec_read(&self, _addr: u16) -> Result<u8, PlatformError> {
        Err(PlatformError::Unavailable)
    }

    fn ec_write(&self, _addr: u16, _val: u8) -> Result<(), PlatformError> {
        Err(PlatformError::Unavailable)
    }

    fn smrw(&self, _req: UwccSmrw) -> Result<UwccSmrw, PlatformError> {
        Err(PlatformError::Unavailable)
    }

    fn pwup(&self, _enable: bool, _level: u32) -> Result<(), PlatformError> {
        Err(PlatformError::Unavailable)
    }

    fn poll_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
        Err(PlatformError::Unavailable)
    }
}

/// Recorded mutation operation on [`MockUwccTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UwccTransportWrite {
    EcWrite { addr: u16, val: u8 },
    Smrw { offset: u16, len: u16 },
    Pwup { enable: bool, level: u32 },
    GpuPower { op: u32, value: u32 },
    MuxSet { boot_mode: u32 },
}

/// Deterministic in-memory `/dev/uwcc` transport for unit & integration tests.
#[derive(Debug)]
pub struct MockUwccTransport {
    available: AtomicBool,
    fail_writes: AtomicBool,
    info: Mutex<UwccInfo>,
    regs: Mutex<BTreeMap<u16, u8>>,
    smrw_table: Mutex<BTreeMap<u16, [u8; UWCC_SMRW_MAX_LEN]>>,
    mux_state: Mutex<UwccMuxState>,
    events: Mutex<VecDeque<UwccWmiEvent>>,
    journal: Mutex<Vec<UwccTransportWrite>>,
}

impl Default for MockUwccTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockUwccTransport {
    /// Creates an empty available mock transport.
    pub fn new() -> Self {
        Self {
            available: AtomicBool::new(true),
            fail_writes: AtomicBool::new(false),
            info: Mutex::new(UwccInfo {
                api_version: UWCC_API_VERSION,
                project_id: super::CHASSIS_PROJECT_0X1A,
                ec_version: 0x0105,
                flags: UWCC_FLAG_HAS_EC
                    | UWCC_FLAG_HAS_SMRW
                    | UWCC_FLAG_HAS_PWUP
                    | UWCC_FLAG_HAS_WMI_EVENTS
                    | UWCC_FLAG_HAS_HWMON
                    | UWCC_FLAG_HAS_PWBT
                    | UWCC_FLAG_HAS_PWDB
                    | UWCC_FLAG_HAS_MUX,
            }),
            regs: Mutex::new(BTreeMap::new()),
            smrw_table: Mutex::new(BTreeMap::new()),
            mux_state: Mutex::new(UwccMuxState {
                active_gpu: UWCC_MUX_ACTIVE_IGPU,
                boot_mode: UWCC_MUX_MODE_DYNAMIC,
            }),
            events: Mutex::new(VecDeque::new()),
            journal: Mutex::new(Vec::new()),
        }
    }

    /// Creates a mock transport pre-populated with MECHREVO YAOSHI X6AR55xY (`0x1A`) registers
    /// and a valid `SMRW` power-limits table.
    pub fn chassis_0x1a_default() -> Self {
        let transport = Self::new();
        transport.set_reg(super::uwcc_ec_reg::CPU_TEMP_C, 55);
        transport.set_reg(super::uwcc_ec_reg::GPU_TEMP_C, 48);
        // CPU RPM = 2800 (0x0AF0) -> hi=0x0A, lo=0xF0
        transport.set_reg(super::uwcc_ec_reg::CPU_FAN_RPM_HI, 0x0A);
        transport.set_reg(super::uwcc_ec_reg::CPU_FAN_RPM_LO, 0xF0);
        // GPU RPM = 2400 (0x0960) -> hi=0x09, lo=0x60
        transport.set_reg(super::uwcc_ec_reg::GPU_FAN_RPM_HI, 0x09);
        transport.set_reg(super::uwcc_ec_reg::GPU_FAN_RPM_LO, 0x60);
        // Duty 40% (raw 80/200) and 35% (raw 70/200)
        transport.set_reg(super::uwcc_ec_reg::CPU_FAN_DUTY_READ, 80);
        transport.set_reg(super::uwcc_ec_reg::GPU_FAN_DUTY_READ, 70);
        // TDP defaults
        transport.set_reg(super::uwcc_ec_reg::PL1_WATTS, 55);
        transport.set_reg(super::uwcc_ec_reg::PL2_WATTS, 80);
        transport.set_reg(super::uwcc_ec_reg::PL4_WATTS, 115);
        transport.set_reg(super::uwcc_ec_reg::TCC_OFFSET_C, 0);
        transport.set_reg(super::uwcc_ec_reg::PERF_MODE, 2);
        transport.set_reg(super::uwcc_ec_reg::CHARGE_CTRL, 80);
        transport.set_reg(super::uwcc_ec_reg::LIGHTBAR_AC_CTRL, 0x81);

        let limits = super::SmrwPowerLimits::chassis_0x1a_default();
        transport.set_smrw_table(super::SMRW_POWER_LIMITS_OFFSET, limits.encode());
        transport
    }

    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }

    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    pub fn set_info(&self, info: UwccInfo) {
        *self.info.lock().unwrap_or_else(|e| e.into_inner()) = info;
    }

    pub fn set_reg(&self, addr: u16, val: u8) {
        self.regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(addr, val);
    }

    pub fn get_reg(&self, addr: u16) -> Option<u8> {
        self.regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&addr)
            .copied()
    }

    pub fn set_smrw_table(&self, offset: u16, data: [u8; UWCC_SMRW_MAX_LEN]) {
        self.smrw_table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(offset, data);
    }

    pub fn push_event(&self, event: UwccWmiEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
    }

    pub fn journal(&self) -> Vec<UwccTransportWrite> {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear_journal(&self) {
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn set_mux_state(&self, state: UwccMuxState) {
        *self.mux_state.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }
}

impl UwccTransport for MockUwccTransport {
    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn get_info(&self) -> Result<UwccInfo, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(*self.info.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn ec_read(&self, addr: u16) -> Result<u8, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self
            .regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&addr)
            .copied()
            .unwrap_or(0))
    }

    fn ec_write(&self, addr: u16, val: u8) -> Result<(), PlatformError> {
        if !self.is_available() || self.fail_writes.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        self.regs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(addr, val);
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(UwccTransportWrite::EcWrite { addr, val });
        Ok(())
    }

    fn smrw(&self, mut req: UwccSmrw) -> Result<UwccSmrw, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        if req.len == 0 || usize::from(req.len) > UWCC_SMRW_MAX_LEN {
            return Err(PlatformError::InvalidArgument(
                "SMRW length must be 1..=64 bytes",
            ));
        }
        if let Some(table) = self
            .smrw_table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req.offset)
            .copied()
        {
            let copy_len = usize::from(req.len).min(UWCC_SMRW_MAX_LEN);
            req.data[..copy_len].copy_from_slice(&table[..copy_len]);
        }
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(UwccTransportWrite::Smrw {
                offset: req.offset,
                len: req.len,
            });
        Ok(req)
    }

    fn pwup(&self, enable: bool, level: u32) -> Result<(), PlatformError> {
        if !self.is_available() || self.fail_writes.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        if level > UWCC_PWUP_MAX_LEVEL || (!enable && level != 0) {
            return Err(PlatformError::InvalidArgument(
                "PWUP level must be 0..=7 (and 0 when disabled)",
            ));
        }
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(UwccTransportWrite::Pwup { enable, level });
        Ok(())
    }

    fn set_gpu_power(&self, op: u32, value: u32) -> Result<(), PlatformError> {
        if !self.is_available() || self.fail_writes.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        match op {
            UWCC_GPU_POWER_OP_BASE_TGP if value <= 300 => {}
            UWCC_GPU_POWER_OP_DYNAMIC_BOOST if value <= 100 => {}
            _ => {
                return Err(PlatformError::InvalidArgument(
                    "Invalid NVIDIA NPCF GPU power operation or watt bound",
                ));
            }
        }
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(UwccTransportWrite::GpuPower { op, value });
        Ok(())
    }

    fn mux_get(&self) -> Result<UwccMuxState, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(*self.mux_state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn mux_set(&self, boot_mode: u32) -> Result<(), PlatformError> {
        if !self.is_available() || self.fail_writes.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        if !(UWCC_MUX_MODE_DYNAMIC..=UWCC_MUX_MODE_MSHYBRID).contains(&boot_mode) {
            return Err(PlatformError::InvalidArgument(
                "MUX boot mode must be 1..=4",
            ));
        }
        self.mux_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .boot_mode = boot_mode;
        self.journal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(UwccTransportWrite::MuxSet { boot_mode });
        Ok(())
    }

    fn poll_event(&self) -> Result<Option<UwccWmiEvent>, PlatformError> {
        if !self.is_available() {
            return Err(PlatformError::Unavailable);
        }
        Ok(self
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_uwcc_transport_pwup_validates_0_to_7() {
        let transport = MockUwccTransport::new();
        for level in 0..=UWCC_PWUP_MAX_LEVEL {
            transport
                .pwup(true, level)
                .unwrap_or_else(|e| panic!("level {level} should succeed: {e}"));
        }
        transport.pwup(false, 0).unwrap();
        assert!(matches!(
            transport.pwup(true, UWCC_PWUP_MAX_LEVEL + 1),
            Err(PlatformError::InvalidArgument(_))
        ));
        assert!(matches!(
            transport.pwup(false, 1),
            Err(PlatformError::InvalidArgument(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn poll_event_rejects_partial_read_and_decodes_full_event() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // 1. Partial read (4 bytes < 16 bytes) must return PlatformError::Io, never Ok(None)
        let mut partial_file = NamedTempFile::new().expect("temp file");
        partial_file
            .write_all(&[1, 2, 3, 4])
            .expect("write partial");
        partial_file.flush().expect("flush");
        let dev = UwccDevice::open(partial_file.path()).expect("open partial file");
        let err = dev
            .poll_event()
            .expect_err("partial read must return error instead of Ok(None)");
        assert!(
            matches!(err, PlatformError::Io { ref source, .. } if source.kind() == std::io::ErrorKind::UnexpectedEof),
            "expected PlatformError::Io(UnexpectedEof), got {err:?}"
        );

        // 2. Empty file (EOF, 0 bytes) returns Ok(None)
        let empty_file = NamedTempFile::new().expect("empty file");
        let empty_dev = UwccDevice::open(empty_file.path()).expect("open empty file");
        assert_eq!(empty_dev.poll_event().expect("eof poll"), None);

        // 3. Full 16-byte UwccWmiEvent decodes cleanly
        let expected = UwccWmiEvent {
            timestamp_ns: 0x0102_0304_0506_0708,
            code: 0x00B0,
            payload: 0x0000_0001,
        };
        let mut full_file = NamedTempFile::new().expect("full file");
        let raw_bytes = unsafe {
            std::slice::from_raw_parts(
                (&expected as *const UwccWmiEvent).cast::<u8>(),
                std::mem::size_of::<UwccWmiEvent>(),
            )
        };
        full_file.write_all(raw_bytes).expect("write event");
        full_file.flush().expect("flush");
        let full_dev = UwccDevice::open(full_file.path()).expect("open full file");
        assert_eq!(full_dev.poll_event().expect("full poll"), Some(expected));
    }
}
