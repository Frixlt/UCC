// SPDX-License-Identifier: GPL-3.0-or-later
//! WMI event translation (`/dev/uwcc` [`UwccWmiEvent`] -> [`UwccHotkeyEvent`]) and
//! virtual keyboard emission over Linux `/dev/uinput` (`O_NOFOLLOW`) or [`MockUinputEmitter`].

use crate::platform::PlatformError;
use crate::uwcc::uapi::UwccWmiEvent;
#[cfg(unix)]
use nix::libc;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Canonical Linux uinput device path.
pub const UINPUT_DEVICE_PATH: &str = "/dev/uinput";

/// Uniwill WMI hotkey event codes emitted by `/dev/uwcc`.
pub mod wmi_code {
    pub const TOUCHPAD_TOGGLE: u32 = 0x00B0;
    pub const PERF_MODE_CYCLE: u32 = 0x00B1;
    pub const KBD_BRIGHTNESS_UP: u32 = 0x00B2;
    pub const KBD_BRIGHTNESS_DOWN: u32 = 0x00B3;
    pub const WEBCAM_TOGGLE: u32 = 0x00B4;
    pub const FAN_BOOST_TOGGLE: u32 = 0x00B5;
    pub const FN_LOCK_TOGGLE: u32 = 0x00B8;
    pub const WIN_LOCK_TOGGLE: u32 = 0x00B9;
}

/// Standard Linux `<linux/input-event-codes.h>` keycodes emitted via `/dev/uinput`.
pub mod linux_key {
    pub const KEY_PROG1: u16 = 148;
    pub const KEY_PROG2: u16 = 149;
    pub const KEY_PROG3: u16 = 202;
    pub const KEY_CAMERA: u16 = 212;
    pub const KEY_KBDILLUMDOWN: u16 = 229;
    pub const KEY_KBDILLUMUP: u16 = 230;
    pub const KEY_FN_ESC: u16 = 465;
    pub const KEY_TOUCHPAD_TOGGLE: u16 = 530;
}

/// Semantic hotkey events decoded from raw `/dev/uwcc` WMI notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UwccHotkeyEvent {
    FnLockToggle,
    WinLockToggle,
    TouchpadToggle,
    PerformanceModeCycle,
    KeyboardBrightnessUp,
    KeyboardBrightnessDown,
    WebcamToggle,
    FanBoostToggle,
    Unknown(u32),
}

/// Translates a raw [`UwccWmiEvent`] into a semantic [`UwccHotkeyEvent`].
pub fn translate_wmi_event(event: UwccWmiEvent) -> UwccHotkeyEvent {
    match event.code {
        wmi_code::FN_LOCK_TOGGLE => UwccHotkeyEvent::FnLockToggle,
        wmi_code::WIN_LOCK_TOGGLE => UwccHotkeyEvent::WinLockToggle,
        wmi_code::TOUCHPAD_TOGGLE => UwccHotkeyEvent::TouchpadToggle,
        wmi_code::PERF_MODE_CYCLE => UwccHotkeyEvent::PerformanceModeCycle,
        wmi_code::KBD_BRIGHTNESS_UP => UwccHotkeyEvent::KeyboardBrightnessUp,
        wmi_code::KBD_BRIGHTNESS_DOWN => UwccHotkeyEvent::KeyboardBrightnessDown,
        wmi_code::WEBCAM_TOGGLE => UwccHotkeyEvent::WebcamToggle,
        wmi_code::FAN_BOOST_TOGGLE => UwccHotkeyEvent::FanBoostToggle,
        other => UwccHotkeyEvent::Unknown(other),
    }
}

/// Maps a semantic [`UwccHotkeyEvent`] to a Linux input keycode for `/dev/uinput` injection.
pub fn hotkey_to_linux_keycode(event: UwccHotkeyEvent) -> Option<u16> {
    match event {
        UwccHotkeyEvent::FnLockToggle => Some(linux_key::KEY_FN_ESC),
        UwccHotkeyEvent::WinLockToggle => Some(linux_key::KEY_PROG3),
        UwccHotkeyEvent::TouchpadToggle => Some(linux_key::KEY_TOUCHPAD_TOGGLE),
        UwccHotkeyEvent::PerformanceModeCycle => Some(linux_key::KEY_PROG1),
        UwccHotkeyEvent::KeyboardBrightnessUp => Some(linux_key::KEY_KBDILLUMUP),
        UwccHotkeyEvent::KeyboardBrightnessDown => Some(linux_key::KEY_KBDILLUMDOWN),
        UwccHotkeyEvent::WebcamToggle => Some(linux_key::KEY_CAMERA),
        UwccHotkeyEvent::FanBoostToggle => Some(linux_key::KEY_PROG2),
        UwccHotkeyEvent::Unknown(_) => None,
    }
}

/// Virtual input key emitter trait decoupled from Linux `/dev/uinput`.
pub trait UinputEmitter: Send + Sync {
    /// Emits a single key press + release + sync sequence for `keycode`.
    fn emit_key_click(&self, keycode: u16) -> Result<(), PlatformError>;

    /// Translates and emits a semantic [`UwccHotkeyEvent`]. Returns `Ok(true)` if a key
    /// was emitted, or `Ok(false)` for unmapped [`UwccHotkeyEvent::Unknown`] events.
    fn emit_hotkey(&self, event: UwccHotkeyEvent) -> Result<bool, PlatformError> {
        match hotkey_to_linux_keycode(event) {
            Some(code) => {
                self.emit_key_click(code)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Convenience pipeline: decodes a raw [`UwccWmiEvent`] and emits its keycode if mapped.
    fn handle_wmi_event(&self, event: UwccWmiEvent) -> Result<UwccHotkeyEvent, PlatformError> {
        let hotkey = translate_wmi_event(event);
        let _ = self.emit_hotkey(hotkey)?;
        Ok(hotkey)
    }
}

/// Deterministic in-memory mock [`UinputEmitter`] recording emitted Linux keycodes.
#[derive(Debug, Default)]
pub struct MockUinputEmitter {
    fail_emit: AtomicBool,
    emitted: Mutex<Vec<u16>>,
}

impl MockUinputEmitter {
    pub fn new() -> Self {
        Self {
            fail_emit: AtomicBool::new(false),
            emitted: Mutex::new(Vec::new()),
        }
    }

    pub fn set_fail(&self, fail: bool) {
        self.fail_emit.store(fail, Ordering::SeqCst);
    }

    pub fn emitted_keys(&self) -> Vec<u16> {
        self.emitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear(&self) {
        self.emitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl UinputEmitter for MockUinputEmitter {
    fn emit_key_click(&self, keycode: u16) -> Result<(), PlatformError> {
        if self.fail_emit.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        self.emitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(keycode);
        Ok(())
    }
}

/// Linux `/dev/uinput` virtual keyboard emitter opened with `O_NOFOLLOW | O_NONBLOCK`.
#[cfg(unix)]
pub struct LinuxUinputEmitter {
    file: Mutex<File>,
}

#[cfg(unix)]
mod linux_uinput_abi {
    use nix::libc;

    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const SYN_REPORT: u16 = 0x00;
    pub const BUS_VIRTUAL: u16 = 0x06;

    pub const UINPUT_MAX_NAME_SIZE: usize = 80;

    #[repr(C)]
    pub struct InputId {
        pub bustype: u16,
        pub vendor: u16,
        pub product: u16,
        pub version: u16,
    }

    #[repr(C)]
    pub struct UinputSetup {
        pub id: InputId,
        pub name: [libc::c_char; UINPUT_MAX_NAME_SIZE],
        pub ff_effects_max: u32,
    }

    #[repr(C)]
    pub struct InputEvent {
        pub tv_sec: libc::time_t,
        pub tv_usec: libc::suseconds_t,
        pub type_: u16,
        pub code: u16,
        pub value: i32,
    }

    const UINPUT_MAGIC: u8 = b'U';
    pub const UI_DEV_CREATE: u64 = nix::request_code_none!(UINPUT_MAGIC, 1);
    pub const UI_DEV_DESTROY: u64 = nix::request_code_none!(UINPUT_MAGIC, 2);
    pub const UI_DEV_SETUP: u64 =
        nix::request_code_write!(UINPUT_MAGIC, 3, std::mem::size_of::<UinputSetup>()) as u64;
    pub const UI_SET_EVBIT: u64 =
        nix::request_code_write!(UINPUT_MAGIC, 100, std::mem::size_of::<libc::c_int>()) as u64;
    pub const UI_SET_KEYBIT: u64 =
        nix::request_code_write!(UINPUT_MAGIC, 101, std::mem::size_of::<libc::c_int>()) as u64;
}

#[cfg(unix)]
impl LinuxUinputEmitter {
    /// Opens `/dev/uinput` with `O_NOFOLLOW | O_NONBLOCK` and registers supported hotkey codes.
    pub fn open_default() -> Result<Self, PlatformError> {
        Self::open(UINPUT_DEVICE_PATH)
    }

    /// Opens a uinput character device at `path` with `O_NOFOLLOW | O_NONBLOCK`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PlatformError> {
        let path_buf = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&path_buf)
            .map_err(|source| {
                PlatformError::Ioctl(crate::ioctl::IoctlError::OpenDevice {
                    path: path_buf,
                    source,
                })
            })?;

        let fd = file.as_raw_fd();
        let ioctl_int = |req: u64, val: libc::c_int| -> Result<(), PlatformError> {
            // SAFETY: Standard Linux uinput `_IOW('U', nr, int)` ioctl taking an integer argument.
            crate::ioctl::retry_eintr(|| unsafe { libc::ioctl(fd, req as _, val) }).map_err(
                |source| {
                    PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                        request: req,
                        source,
                    })
                },
            )?;
            Ok(())
        };

        ioctl_int(
            linux_uinput_abi::UI_SET_EVBIT,
            libc::c_int::from(linux_uinput_abi::EV_KEY),
        )?;
        ioctl_int(
            linux_uinput_abi::UI_SET_EVBIT,
            libc::c_int::from(linux_uinput_abi::EV_SYN),
        )?;

        let supported_keys = [
            linux_key::KEY_PROG1,
            linux_key::KEY_PROG2,
            linux_key::KEY_PROG3,
            linux_key::KEY_CAMERA,
            linux_key::KEY_KBDILLUMDOWN,
            linux_key::KEY_KBDILLUMUP,
            linux_key::KEY_FN_ESC,
            linux_key::KEY_TOUCHPAD_TOGGLE,
        ];
        for key in supported_keys {
            ioctl_int(linux_uinput_abi::UI_SET_KEYBIT, libc::c_int::from(key))?;
        }

        let mut name_buf = [0 as libc::c_char; linux_uinput_abi::UINPUT_MAX_NAME_SIZE];
        let device_name = b"UCC WMI Hotkeys";
        for (dst, &src) in name_buf.iter_mut().zip(device_name.iter()) {
            *dst = src as libc::c_char;
        }
        let setup = linux_uinput_abi::UinputSetup {
            id: linux_uinput_abi::InputId {
                bustype: linux_uinput_abi::BUS_VIRTUAL,
                vendor: 0x1D6B,
                product: 0x001A,
                version: 1,
            },
            name: name_buf,
            ff_effects_max: 0,
        };

        // SAFETY: `UI_DEV_SETUP` expects a valid pointer to `UinputSetup`.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(
                fd,
                linux_uinput_abi::UI_DEV_SETUP as _,
                &setup as *const linux_uinput_abi::UinputSetup,
            )
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: linux_uinput_abi::UI_DEV_SETUP,
                source,
            })
        })?;

        // SAFETY: `UI_DEV_CREATE` takes no arguments.
        crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(fd, linux_uinput_abi::UI_DEV_CREATE as _)
        })
        .map_err(|source| {
            PlatformError::Ioctl(crate::ioctl::IoctlError::IoctlFailed {
                request: linux_uinput_abi::UI_DEV_CREATE,
                source,
            })
        })?;

        Ok(Self {
            file: Mutex::new(file),
        })
    }

    fn build_key_click_batch(keycode: u16) -> [linux_uinput_abi::InputEvent; 4] {
        [
            linux_uinput_abi::InputEvent {
                tv_sec: 0,
                tv_usec: 0,
                type_: linux_uinput_abi::EV_KEY,
                code: keycode,
                value: 1,
            },
            linux_uinput_abi::InputEvent {
                tv_sec: 0,
                tv_usec: 0,
                type_: linux_uinput_abi::EV_SYN,
                code: linux_uinput_abi::SYN_REPORT,
                value: 0,
            },
            linux_uinput_abi::InputEvent {
                tv_sec: 0,
                tv_usec: 0,
                type_: linux_uinput_abi::EV_KEY,
                code: keycode,
                value: 0,
            },
            linux_uinput_abi::InputEvent {
                tv_sec: 0,
                tv_usec: 0,
                type_: linux_uinput_abi::EV_SYN,
                code: linux_uinput_abi::SYN_REPORT,
                value: 0,
            },
        ]
    }

    fn write_batch_all(writer: &mut impl Write, mut buf: &[u8]) -> Result<(), PlatformError> {
        const MAX_EAGAIN_RETRIES: u8 = 5;
        const MAX_EINTR_RETRIES: u8 = 16;
        let event_size = std::mem::size_of::<linux_uinput_abi::InputEvent>();
        let mut eagain_retries = 0u8;
        let mut eintr_retries = 0u8;
        while !buf.is_empty() {
            match writer.write(buf) {
                Ok(0) => return Err(PlatformError::Unavailable),
                Ok(n) => {
                    if n % event_size != 0 {
                        return Err(PlatformError::Unavailable);
                    }
                    buf = &buf[n..];
                    eagain_retries = 0;
                    eintr_retries = 0;
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
                    eintr_retries += 1;
                    if eintr_retries > MAX_EINTR_RETRIES {
                        return Err(PlatformError::Unavailable);
                    }
                    continue;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    eagain_retries += 1;
                    if eagain_retries > MAX_EAGAIN_RETRIES {
                        return Err(PlatformError::Unavailable);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(_) => return Err(PlatformError::Unavailable),
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
impl UinputEmitter for LinuxUinputEmitter {
    fn emit_key_click(&self, keycode: u16) -> Result<(), PlatformError> {
        let events = Self::build_key_click_batch(keycode);
        // SAFETY: `[InputEvent; 4]` is a contiguous array of plain `#[repr(C)]` POD structs.
        let bytes = unsafe {
            std::slice::from_raw_parts(events.as_ptr().cast::<u8>(), std::mem::size_of_val(&events))
        };
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        Self::write_batch_all(&mut *guard, bytes)
    }
}

#[cfg(unix)]
impl Drop for LinuxUinputEmitter {
    fn drop(&mut self) {
        let guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `UI_DEV_DESTROY` takes no arguments on an open uinput fd.
        let _ = crate::ioctl::retry_eintr(|| unsafe {
            libc::ioctl(guard.as_raw_fd(), linux_uinput_abi::UI_DEV_DESTROY as _)
        });
    }
}
