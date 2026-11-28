// SPDX-License-Identifier: GPL-2.0-only
//! Rust-for-Linux kernel module for Uniwill `INOU0000` EC/WMI/hwmon (`/dev/uwcc`).
//!
//! Target kernel: Linux 7.3+ with `CONFIG_RUST=y`.
//!
//! Architecture:
//! - Driver state, strict EC/SMRW/PWUP validation (`validation.rs`),
//!   64-entry monotonic WMI event ring buffer (`ring_buffer.rs`), and
//!   ioctl/WMI dispatch entry points live in Rust.
//! - Minimal C FFI (`acpi_shim.c`) wraps Linux 7.3 ACPI (`INOU0000`),
//!   `struct wmi_driver`, `hwmon_device_register_with_info`, IRQ-safe
//!   `spinlock_t`, and `/dev/uwcc` `miscdevice` (`file_operations`).

mod ring_buffer;
mod validation;

use core::cell::UnsafeCell;
use core::ffi::c_ulong;
use core::mem::size_of;

use kernel::prelude::*;

pub use ring_buffer::WmiEvent;
use ring_buffer::WmiRingBuffer;
use validation::{
    validate_ec_read, validate_ec_write, validate_gpu_power, validate_mux_set, validate_pwup,
    validate_smrw,
};

// ---------------------------------------------------------------------------
// UAPI ABI mirrors (must match include/uapi/uwcc.h byte-for-byte)
// ---------------------------------------------------------------------------

const UWCC_API_VERSION: u32 = 1;
const UWCC_FLAG_HAS_HWMON: u32 = 1 << 4;

const ERRNO_EINVAL: i32 = -22;
const ERRNO_ENODEV: i32 = -19;

/// UAPI payload for `UWCC_IOC_EC_READ` and `UWCC_IOC_EC_WRITE` (`struct uwcc_ec_reg`).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct UwccEcReg {
    addr: u16,
    val: u8,
    _pad: u8,
}

/// UAPI payload for `UWCC_IOC_SMRW` (`struct uwcc_smrw`).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct UwccSmrw {
    offset: u16,
    len: u16,
    data: [u8; 64],
}

/// UAPI payload for `UWCC_IOC_PWUP` (`struct uwcc_pwup`).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct UwccPwup {
    enable: u32,
    level: u32,
}

/// UAPI payload for `UWCC_IOC_GET_INFO` (`struct uwcc_info`).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct UwccInfo {
    /// UAPI ABI version (`UWCC_API_VERSION`).
    pub api_version: u32,
    /// Uniwill chassis project ID read from EC (`0x0740`).
    pub project_id: u32,
    /// Encoded EC firmware version.
    pub ec_version: u32,
    /// Capability bitmask (`UWCC_FLAG_*`).
    pub flags: u32,
}

const _: () = assert!(size_of::<UwccEcReg>() == 4);
const _: () = assert!(size_of::<UwccSmrw>() == 68);
const _: () = assert!(size_of::<UwccPwup>() == 8);
/// UAPI payload for `UWCC_IOC_GPU_POWER` (`struct uwcc_gpu_power`).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct UwccGpuPower {
    op: u32,
    value: u32,
}

/// UAPI payload for `UWCC_IOC_MUX_GET` and `UWCC_IOC_MUX_SET` (`struct uwcc_mux_state`).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct UwccMuxState {
    active_gpu: u32,
    boot_mode: u32,
}

const _: () = assert!(size_of::<UwccInfo>() == 16);
const _: () = assert!(size_of::<WmiEvent>() == 16);
const _: () = assert!(size_of::<UwccGpuPower>() == 8);
const _: () = assert!(size_of::<UwccMuxState>() == 8);

// ---------------------------------------------------------------------------
// FFI declarations for acpi_shim.c
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct UwccAcpiCaps {
    project_id: u32,
    ec_version: u32,
    flags: u32,
}

extern "C" {
    fn uwcc_acpi_init(out_caps: *mut UwccAcpiCaps) -> i32;
    fn uwcc_acpi_exit();
    fn uwcc_misc_register() -> i32;
    fn uwcc_misc_deregister();
    fn uwcc_acpi_ec_read(addr: u16, out_val: *mut u8) -> i32;
    fn uwcc_acpi_ec_write(addr: u16, val: u8) -> i32;
    fn uwcc_acpi_smrw(offset: u16, len: u16, out_buf: *mut u8) -> i32;
    fn uwcc_acpi_pwup(enable: u32, level: u32) -> i32;
    fn uwcc_acpi_gpu_power(op: u32, value: u32) -> i32;
    fn uwcc_acpi_mux_get(out_active_gpu: *mut u32, out_boot_mode: *mut u32) -> i32;
    fn uwcc_acpi_mux_set(boot_mode: u32) -> i32;
    fn uwcc_hwmon_register() -> i32;
    fn uwcc_hwmon_unregister();
    fn uwcc_state_lock_irqsave() -> c_ulong;
    fn uwcc_state_unlock_irqrestore(flags: c_ulong);
    fn uwcc_ktime_get_ns() -> u64;
    fn uwcc_wmi_wake_readers();
    fn uwcc_wmi_wake_all();
}

// ---------------------------------------------------------------------------
// Shared driver state (guarded by IRQ-safe C spinlock `uwcc_state_lock`)
// ---------------------------------------------------------------------------

struct DriverState {
    ready: bool,
    info: UwccInfo,
    ring: WmiRingBuffer,
}

impl DriverState {
    const fn new() -> Self {
        Self {
            ready: false,
            info: UwccInfo {
                api_version: UWCC_API_VERSION,
                project_id: 0,
                ec_version: 0,
                flags: 0,
            },
            ring: WmiRingBuffer::new(),
        }
    }
}

struct StateCell(UnsafeCell<DriverState>);

// SAFETY: Every access to `StateCell` is serialized through `with_state()`,
// which acquires and releases the IRQ-safe C spinlock `uwcc_state_lock`.
unsafe impl Sync for StateCell {}

static DRIVER_STATE: StateCell = StateCell(UnsafeCell::new(DriverState::new()));

#[inline]
fn with_state<R>(f: impl FnOnce(&mut DriverState) -> R) -> R {
    // SAFETY: Acquires `uwcc_state_lock` with IRQs saved to serialize access to `DRIVER_STATE`.
    let flags = unsafe { uwcc_state_lock_irqsave() };
    let res = {
        // SAFETY: `uwcc_state_lock` is held, granting exclusive access to `DRIVER_STATE`.
        let state = unsafe { &mut *DRIVER_STATE.0.get() };
        f(state)
    };
    // SAFETY: Releases `uwcc_state_lock` and restores IRQ state matching `uwcc_state_lock_irqsave`.
    unsafe { uwcc_state_unlock_irqrestore(flags) };
    res
}

fn c_ret_to_result(ret: i32) -> Result {
    if ret == 0 {
        Ok(())
    } else {
        Err(Error::from_errno(ret))
    }
}

// ---------------------------------------------------------------------------
// Exported C FFI entry points called by acpi_shim.c (/dev/uwcc & WMI driver)
// ---------------------------------------------------------------------------

/// Reports whether the driver is initialized and ready to serve `/dev/uwcc`.
#[no_mangle]
pub extern "C" fn uwcc_rs_is_ready() -> bool {
    with_state(|s| s.ready)
}

/// Callback invoked from `acpi_shim.c` upon receiving a Uniwill WMI event.
#[no_mangle]
pub extern "C" fn uwcc_rs_on_wmi_event(code: u32, payload: u32) {
    // SAFETY: `uwcc_ktime_get_ns` is a pure read of the kernel monotonic clock.
    let ts = unsafe { uwcc_ktime_get_ns() };
    let event = WmiEvent {
        timestamp_ns: ts,
        code,
        payload,
    };

    let pushed = with_state(|s| {
        if !s.ready {
            return false;
        }
        let _ = s.ring.push(event);
        true
    });

    if pushed {
        // SAFETY: Wakes sleepers on the static waitqueue in acpi_shim.c.
        unsafe { uwcc_wmi_wake_readers() };
    }
}

/// Pops the oldest queued WMI event into `out_event`.
///
/// # Safety
///
/// `out_event` must be either null or a valid, writable, properly aligned pointer to `WmiEvent`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_wmi_pop(out_event: *mut WmiEvent) -> bool {
    if out_event.is_null() {
        return false;
    }

    match with_state(|s| s.ring.pop()) {
        Some(ev) => {
            // SAFETY: `out_event` is non-null and points to a valid `struct uwcc_wmi_event`.
            unsafe { *out_event = ev };
            true
        }
        None => false,
    }
}

/// Returns `true` if at least one unread WMI event is queued.
#[no_mangle]
pub extern "C" fn uwcc_rs_wmi_has_data() -> bool {
    with_state(|s| !s.ring.is_empty())
}

/// Waitqueue predicate for blocking `read()` on `/dev/uwcc`.
#[no_mangle]
pub extern "C" fn uwcc_rs_wmi_has_data_or_shutdown() -> bool {
    with_state(|s| !s.ready || !s.ring.is_empty())
}

/// Handles `UWCC_IOC_GET_INFO`.
///
/// # Safety
///
/// `out_info` must be either null or a valid, writable, properly aligned pointer to `UwccInfo`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_get_info(out_info: *mut UwccInfo) -> i32 {
    if out_info.is_null() {
        return ERRNO_EINVAL;
    }

    let info = with_state(|s| {
        if !s.ready {
            return None;
        }
        Some(s.info)
    });

    match info {
        Some(val) => {
            // SAFETY: `out_info` is non-null and points to a valid `struct uwcc_info`.
            unsafe { *out_info = val };
            0
        }
        None => ERRNO_ENODEV,
    }
}

/// Validates and dispatches `UWCC_IOC_EC_READ`.
///
/// # Safety
///
/// `reg` must be either null or a valid, readable and writable pointer to `UwccEcReg`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_ec_read(reg: *mut UwccEcReg) -> i32 {
    if reg.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    // SAFETY: `reg` is a valid kernel stack pointer copied from userspace by acpi_shim.c.
    let req = unsafe { &mut *reg };
    if let Err(err) = validate_ec_read(req.addr, req._pad) {
        return err.to_errno();
    }

    let mut val: u8 = 0;
    // SAFETY: `val` is a valid stack pointer; acpi_shim serializes EC access.
    let ret = unsafe { uwcc_acpi_ec_read(req.addr, &mut val) };
    if ret != 0 {
        return ret;
    }

    req.val = val;
    0
}

/// Validates permissions and EC allowlist, then dispatches `UWCC_IOC_EC_WRITE`.
///
/// # Safety
///
/// `reg` must be either null or a valid, readable pointer to `UwccEcReg`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_ec_write(
    reg: *const UwccEcReg,
    writable: bool,
    sys_admin: bool,
) -> i32 {
    if reg.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    // SAFETY: `reg` is a valid kernel stack pointer copied from userspace by acpi_shim.c.
    let req = unsafe { &*reg };
    if let Err(err) = validate_ec_write(req.addr, req._pad, writable, sys_admin) {
        return err.to_errno();
    }

    // SAFETY: Address and permissions were validated above.
    unsafe { uwcc_acpi_ec_write(req.addr, req.val) }
}

/// Validates bounds and dispatches `UWCC_IOC_SMRW`.
///
/// # Safety
///
/// `smrw` must be either null or a valid, readable and writable pointer to `UwccSmrw`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_smrw(smrw: *mut UwccSmrw) -> i32 {
    if smrw.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    // SAFETY: `smrw` is a valid kernel stack pointer copied from userspace by acpi_shim.c.
    let req = unsafe { &mut *smrw };
    if let Err(err) = validate_smrw(req.offset, req.len) {
        return err.to_errno();
    }

    // SAFETY: `req.len <= 64` fits within `req.data[64]`.
    unsafe { uwcc_acpi_smrw(req.offset, req.len, req.data.as_mut_ptr()) }
}

/// Validates permissions and parameters, then dispatches `UWCC_IOC_PWUP`.
///
/// # Safety
///
/// `pwup` must be either null or a valid, readable pointer to `UwccPwup`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_pwup(
    pwup: *const UwccPwup,
    writable: bool,
    sys_admin: bool,
) -> i32 {
    if pwup.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    // SAFETY: `pwup` is a valid kernel stack pointer copied from userspace by acpi_shim.c.
    let req = unsafe { &*pwup };
    if let Err(err) = validate_pwup(req.enable, req.level, writable, sys_admin) {
        return err.to_errno();
    }

    // SAFETY: Bounds and permissions were validated above.
    unsafe { uwcc_acpi_pwup(req.enable, req.level) }
}

/// Validates permissions and parameters, then dispatches `UWCC_IOC_GPU_POWER` (`PWBT`/`PWDB`).
///
/// # Safety
///
/// `req` must be either null or a valid, readable pointer to `UwccGpuPower`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_gpu_power(
    req: *const UwccGpuPower,
    writable: bool,
    sys_admin: bool,
) -> i32 {
    if req.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    // SAFETY: `req` is a valid kernel stack pointer copied from userspace by acpi_shim.c.
    let payload = unsafe { &*req };
    if let Err(err) = validate_gpu_power(payload.op, payload.value, writable, sys_admin) {
        return err.to_errno();
    }

    // SAFETY: Operation and bounds were validated above.
    unsafe { uwcc_acpi_gpu_power(payload.op, payload.value) }
}

/// Dispatches read-only `UWCC_IOC_MUX_GET` (`MXDS(0)` and `MXDM(0)`).
///
/// # Safety
///
/// `out_state` must be either null or a valid, writable pointer to `UwccMuxState`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_mux_get(out_state: *mut UwccMuxState) -> i32 {
    if out_state.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    let mut active_gpu: u32 = 0;
    let mut boot_mode: u32 = 0;
    // SAFETY: Output pointers are valid stack locals; acpi_shim serializes ACPI access.
    let ret = unsafe { uwcc_acpi_mux_get(&mut active_gpu, &mut boot_mode) };
    if ret != 0 {
        return ret;
    }

    // SAFETY: `out_state` is non-null and points to a valid `struct uwcc_mux_state`.
    unsafe {
        (*out_state).active_gpu = active_gpu;
        (*out_state).boot_mode = boot_mode;
    }
    0
}

/// Validates permissions and parameters, then dispatches `UWCC_IOC_MUX_SET` (`MXDM(1..=4)`).
///
/// # Safety
///
/// `req` must be either null or a valid, readable pointer to `UwccMuxState`.
#[no_mangle]
pub unsafe extern "C" fn uwcc_rs_ioctl_mux_set(
    req: *const UwccMuxState,
    writable: bool,
    sys_admin: bool,
) -> i32 {
    if req.is_null() {
        return ERRNO_EINVAL;
    }
    if !uwcc_rs_is_ready() {
        return ERRNO_ENODEV;
    }

    // SAFETY: `req` is a valid kernel stack pointer copied from userspace by acpi_shim.c.
    let payload = unsafe { &*req };
    if let Err(err) = validate_mux_set(payload.active_gpu, payload.boot_mode, writable, sys_admin) {
        return err.to_errno();
    }

    // SAFETY: Mode bounds and permissions were validated above.
    unsafe { uwcc_acpi_mux_set(payload.boot_mode) }
}

// ---------------------------------------------------------------------------
// Kernel module entry & teardown
// ---------------------------------------------------------------------------

struct UwccModule;

impl kernel::Module for UwccModule {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        let mut caps = UwccAcpiCaps::default();
        // SAFETY: `caps` is a valid stack struct; initializes ACPI INOU0000 and WMI driver.
        let ret = unsafe { uwcc_acpi_init(&mut caps) };
        c_ret_to_result(ret)?;

        with_state(|s| {
            s.info = UwccInfo {
                api_version: UWCC_API_VERSION,
                project_id: caps.project_id,
                ec_version: caps.ec_version,
                flags: caps.flags,
            };
            s.ring.clear();
            s.ready = true;
        });

        // Register /dev/uwcc miscdevice first so `uwcc_miscdev.this_device` provides a valid
        // non-NULL parent `struct device *` for `hwmon_device_register_with_info`.
        // SAFETY: State is initialized and ready before exposing /dev/uwcc.
        let misc_ret = unsafe { uwcc_misc_register() };
        if let Err(err) = c_ret_to_result(misc_ret) {
            with_state(|s| {
                s.ready = false;
            });
            // SAFETY: Roll back WMI driver and ACPI state on failure.
            unsafe { uwcc_acpi_exit() };
            return Err(err);
        }

        // Register native /sys/class/hwmon/hwmonX (D3cold-safe EC telemetry).
        let mut flags = caps.flags;
        // SAFETY: `uwcc_hwmon_register()` binds to `uwcc_miscdev.this_device` registered above.
        if unsafe { uwcc_hwmon_register() } == 0 {
            flags |= UWCC_FLAG_HAS_HWMON;
            with_state(|s| {
                s.info.flags = flags;
            });
        } else {
            pr_warn!("uwcc_rs: hwmon registration failed; continuing with /dev/uwcc only\n");
        }

        pr_info!(
            "uwcc_rs: initialized /dev/uwcc (project_id=0x{:02x}, ec_ver=0x{:04x}, flags=0x{:x})\n",
            caps.project_id,
            caps.ec_version,
            flags
        );

        Ok(Self)
    }
}

impl Drop for UwccModule {
    fn drop(&mut self) {
        with_state(|s| {
            s.ready = false;
        });
        // SAFETY: Wakes any blocked /dev/uwcc readers, deregisters /dev/uwcc,
        // and tears down hwmon, WMI driver, and ACPI handle (via uwcc_acpi_exit).
        unsafe {
            uwcc_wmi_wake_all();
            uwcc_hwmon_unregister();
            uwcc_misc_deregister();
            uwcc_acpi_exit();
        }
        pr_info!("uwcc_rs: unloaded\n");
    }
}

module! {
    type: UwccModule,
    name: "uwcc_rs",
    authors: ["ucc-rusted contributors"],
    description: "Uniwill INOU0000 EC/WMI/hwmon driver (Rust-for-Linux)",
    license: "GPL v2",
}
