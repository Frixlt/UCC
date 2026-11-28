// SPDX-License-Identifier: GPL-2.0-only
//! Strict `no_std` EC/SMRW/PWUP address, bounds, and permission validation.
//!
//! Shared between the `uwcc-rs` kernel module and host-side unit tests.

/// Maximum payload length for `UWCC_IOC_SMRW` (matches `UWCC_SMRW_MAX_LEN`).
pub(crate) const UWCC_SMRW_MAX_LEN: u16 = 64;

/// Maximum addressable byte size of the Uniwill SMRW factory limits table (8-bit REOF offset).
pub(crate) const UWCC_SMRW_TABLE_LIMIT: u16 = 0x0100;

/// Maximum allowed `PWUP` boost level.
pub(crate) const UWCC_PWUP_MAX_LEVEL: u32 = 3;

/// `UWCC_IOC_GPU_POWER` op: NVIDIA NPCF Base TGP (`PWBT`).
pub(crate) const UWCC_GPU_POWER_OP_BASE_TGP: u32 = 1;

/// `UWCC_IOC_GPU_POWER` op: NVIDIA NPCF Dynamic Boost (`PWDB`).
pub(crate) const UWCC_GPU_POWER_OP_DYNAMIC_BOOST: u32 = 2;

/// Maximum permitted Base TGP watt value for `PWBT`.
pub(crate) const UWCC_PWBT_MAX_WATTS: u32 = 300;

/// Maximum permitted Dynamic Boost watt value for `PWDB`.
pub(crate) const UWCC_PWDB_MAX_WATTS: u32 = 100;

/// Minimum valid BIOS MUX boot mode (`MXDM`).
pub(crate) const UWCC_MUX_MODE_MIN: u32 = 1;

/// Maximum valid BIOS MUX boot mode (`MXDM`).
pub(crate) const UWCC_MUX_MODE_MAX: u32 = 4;

/// Minimum readable Uniwill EC register address.
pub(crate) const EC_READ_ADDR_MIN: u16 = 0x0400;

/// Maximum readable Uniwill EC register address.
pub(crate) const EC_READ_ADDR_MAX: u16 = 0x18FF;

/// Validation error returned before any ACPI/EC call is dispatched.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ValidationError {
    /// Register address or table offset is outside permitted bounds.
    OutOfBounds,
    /// Target EC register is read-only or not on the explicit write allowlist.
    ReadOnlyOrForbidden,
    /// Invalid buffer length (e.g. `0` or `> 64` for SMRW).
    InvalidLength,
    /// Invalid parameter value (e.g. `PWUP` enable > 1 or level > 3).
    InvalidArgument,
    /// Caller lacks write file descriptor mode or `CAP_SYS_ADMIN`.
    PermissionDenied,
}

impl ValidationError {
    /// Converts a validation error into the corresponding negative Linux errno.
    #[inline]
    pub(crate) const fn to_errno(self) -> i32 {
        match self {
            Self::OutOfBounds => -34,          // -ERANGE
            Self::ReadOnlyOrForbidden => -13,  // -EACCES
            Self::InvalidLength => -22,        // -EINVAL
            Self::InvalidArgument => -22,      // -EINVAL
            Self::PermissionDenied => -1,      // -EPERM
        }
    }
}

/// Returns `true` if `addr` is within the readable Uniwill EC register space.
#[inline]
pub(crate) const fn is_valid_ec_read_addr(addr: u16) -> bool {
    addr >= EC_READ_ADDR_MIN && addr <= EC_READ_ADDR_MAX
}

/// Returns `true` if `addr` is on the explicit allowlist of safe writable EC registers.
///
/// Read-only sensor registers (`0x043E` CPU temp, `0x044F` GPU temp,
/// `0x0464`/`0x0465` and `0x046C`/`0x046D` tachometer RPM), read-only chassis
/// identity (`0x0740` `EC_ADDR_PROJECT_ID`), read-only PWM readback (`0x075B`/`0x075C`),
/// read-only capability masks (`0x0765`/`0x0766`), read-only ROMID (`0x0770..=0x077D`),
/// low firmware/BIOS registers (`< 0x0400`), and unmapped regions are strictly rejected.
#[inline]
pub(crate) const fn is_valid_ec_write_addr(addr: u16) -> bool {
    match addr {
        // Uniwill 0x1A Custom power mode control (0x0726 BIT(7)) & CUME enable (0x0727 BIT(6);
        // callers MUST RMW and preserve BIT(7) PL4D on 0x0727)
        0x0726 | 0x0727 => true,
        // AP_OEM manual control latch (0x0741) & cTGP/DB offset controls (0x0743..=0x0745)
        0x0741 | 0x0743..=0x0745 => true,
        // Lightbar AC control & RGB registers (0x0748 EC_ADDR_LIGHTBAR_AC_CTRL, 0x0749..=0x074B)
        0x0748..=0x074B => true,
        // Manual fan mode / perf controls (0x0751..=0x0753); excludes read-only PWM 0x075B/0x075C
        0x0751..=0x0753 => true,
        // USB sleep charging (0x0762), trigger clear (0x0767), and lid logo RGB (0x0769..=0x076B);
        // excludes read-only 0x0765/0x0766 (SUPPORT_1/2) and 0x0768 (SWITCH_STATUS)
        0x0762 | 0x0767 | 0x0769..=0x076B => true,
        // Upstream Uniwill TDP limits (0x0783 PL1, 0x0784 PL2, 0x0785 PL4), fan default (0x0786),
        // and lid logo EC commit trigger (0x078C); excludes read-only ROMID (0x0770..=0x077D)
        0x0783..=0x0786 | 0x078C => true,
        // OEM switches (0x07A4 FN_LOCK, 0x07A5..=0x07A6 battery mode), lid logo brightness (0x07AB),
        // OEM byte 5 (0x07C5 logo/fan-curve flag RMW), and AP_OEM_6 (0x07C6 ENABLE_UNIVERSAL_FAN_CTRL)
        0x07A4..=0x07A6 | 0x07AB | 0x07C5 | 0x07C6 => true,
        // Battery charge threshold (0x07B9 EC_ADDR_CHARGE_CTRL) & priority (0x07CC EC_ADDR_CHARGE_PRIO)
        0x07B9 | 0x07CC => true,
        // RamFan 1.5 16-point curve tables + CUME/UFME flags
        0x0F00..=0x0F5F => true,
        // Legacy Uniwill CPU/GPU fan duty registers
        0x1804 | 0x1809 => true,
        _ => false,
    }
}

/// Validates an `UWCC_IOC_EC_READ` request.
pub(crate) const fn validate_ec_read(addr: u16, pad: u8) -> Result<(), ValidationError> {
    if pad != 0 {
        return Err(ValidationError::InvalidArgument);
    }
    if !is_valid_ec_read_addr(addr) {
        return Err(ValidationError::OutOfBounds);
    }
    Ok(())
}

/// Validates an `UWCC_IOC_EC_WRITE` request.
///
/// Enforces both file descriptor write permission (`FMODE_WRITE`) and
/// `CAP_SYS_ADMIN` before checking the explicit EC write allowlist.
pub(crate) const fn validate_ec_write(
    addr: u16,
    pad: u8,
    file_writable: bool,
    has_sys_admin: bool,
) -> Result<(), ValidationError> {
    if !file_writable || !has_sys_admin {
        return Err(ValidationError::PermissionDenied);
    }
    if pad != 0 {
        return Err(ValidationError::InvalidArgument);
    }
    if !is_valid_ec_read_addr(addr) {
        return Err(ValidationError::OutOfBounds);
    }
    if !is_valid_ec_write_addr(addr) {
        return Err(ValidationError::ReadOnlyOrForbidden);
    }
    Ok(())
}

/// Validates an `UWCC_IOC_SMRW` table read request.
pub(crate) const fn validate_smrw(offset: u16, len: u16) -> Result<(), ValidationError> {
    if len == 0 || len > UWCC_SMRW_MAX_LEN {
        return Err(ValidationError::InvalidLength);
    }
    match offset.checked_add(len) {
        Some(end) if end <= UWCC_SMRW_TABLE_LIMIT => Ok(()),
        _ => Err(ValidationError::OutOfBounds),
    }
}

/// Validates an `UWCC_IOC_PWUP` liquid-cooler power boost request.
pub(crate) const fn validate_pwup(
    enable: u32,
    level: u32,
    file_writable: bool,
    has_sys_admin: bool,
) -> Result<(), ValidationError> {
    if !file_writable || !has_sys_admin {
        return Err(ValidationError::PermissionDenied);
    }
    if enable > 1 || level > UWCC_PWUP_MAX_LEVEL {
        return Err(ValidationError::InvalidArgument);
    }
    if enable == 0 && level != 0 {
        return Err(ValidationError::InvalidArgument);
    }
    Ok(())
}

/// Validates an `UWCC_IOC_GPU_POWER` (`PWBT` / `PWDB`) request.
pub(crate) const fn validate_gpu_power(
    op: u32,
    value: u32,
    file_writable: bool,
    has_sys_admin: bool,
) -> Result<(), ValidationError> {
    if !file_writable || !has_sys_admin {
        return Err(ValidationError::PermissionDenied);
    }
    match op {
        UWCC_GPU_POWER_OP_BASE_TGP if value <= UWCC_PWBT_MAX_WATTS => Ok(()),
        UWCC_GPU_POWER_OP_DYNAMIC_BOOST if value <= UWCC_PWDB_MAX_WATTS => Ok(()),
        _ => Err(ValidationError::InvalidArgument),
    }
}

/// Validates an `UWCC_IOC_MUX_SET` (`MXDM`) request.
///
/// Requires `active_gpu == 0` so callers only request reboot-safe BIOS mode changes (`1..=4`).
pub(crate) const fn validate_mux_set(
    active_gpu: u32,
    boot_mode: u32,
    file_writable: bool,
    has_sys_admin: bool,
) -> Result<(), ValidationError> {
    if !file_writable || !has_sys_admin {
        return Err(ValidationError::PermissionDenied);
    }
    if active_gpu != 0 || boot_mode < UWCC_MUX_MODE_MIN || boot_mode > UWCC_MUX_MODE_MAX {
        return Err(ValidationError::InvalidArgument);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ec_read_bounds() {
        assert_eq!(validate_ec_read(0x043E, 0), Ok(()));
        assert_eq!(validate_ec_read(0x044F, 0), Ok(()));
        assert_eq!(validate_ec_read(0x0464, 0), Ok(()));
        assert_eq!(validate_ec_read(0x1809, 0), Ok(()));
        assert_eq!(validate_ec_read(0x03FF, 0), Err(ValidationError::OutOfBounds));
        assert_eq!(validate_ec_read(0x1900, 0), Err(ValidationError::OutOfBounds));
        assert_eq!(
            validate_ec_read(0x043E, 1),
            Err(ValidationError::InvalidArgument)
        );
    }

    #[test]
    fn ec_write_allowlist_and_permissions() {
        // Requires both FMODE_WRITE and CAP_SYS_ADMIN.
        assert_eq!(
            validate_ec_write(0x0783, 0, false, true),
            Err(ValidationError::PermissionDenied)
        );
        assert_eq!(
            validate_ec_write(0x0783, 0, true, false),
            Err(ValidationError::PermissionDenied)
        );

        // Allowed writable registers aligned with upstream uniwill-acpi.c.
        for addr in [
            0x0726, 0x0727, 0x0741, 0x0743, 0x0744, 0x0745, 0x0748, 0x0751, 0x0752, 0x0753,
            0x0762, 0x0767, 0x0769, 0x076A, 0x076B, 0x0783, 0x0784, 0x0785, 0x0786, 0x078C,
            0x07A4, 0x07A6, 0x07AB, 0x07B9, 0x07C5, 0x07C6, 0x07CC, 0x0F00, 0x0F5C, 0x1804,
            0x1809,
        ] {
            assert_eq!(validate_ec_write(addr, 0, true, true), Ok(()));
        }

        // Read-only telemetry, 0x0740 (PROJECT_ID), 0x075B/0x075C (PWM RO), 0x0765/0x0766 (SUPPORT),
        // 0x0768 (SWITCH_STATUS), 0x0770 (ROMID), 0x077D (ROMID tail) must be rejected.
        for addr in [
            0x043E, 0x044F, 0x0464, 0x0465, 0x046C, 0x046D, 0x0725, 0x0728, 0x0740, 0x0742,
            0x0746, 0x074E, 0x075B, 0x075C, 0x0760, 0x0765, 0x0766, 0x0768, 0x076C, 0x0770,
            0x077D, 0x078B, 0x078D, 0x07AA, 0x07AC, 0x07C4, 0x07C7,
        ] {
            assert_eq!(
                validate_ec_write(addr, 0, true, true),
                Err(ValidationError::ReadOnlyOrForbidden)
            );
        }

        // Out-of-bounds low/high registers must be rejected.
        assert_eq!(
            validate_ec_write(0x0001, 0, true, true),
            Err(ValidationError::OutOfBounds)
        );
        assert_eq!(
            validate_ec_write(0xFFFF, 0, true, true),
            Err(ValidationError::OutOfBounds)
        );
    }

    #[test]
    fn smrw_bounds() {
        assert_eq!(validate_smrw(0, 1), Ok(()));
        assert_eq!(validate_smrw(0, 64), Ok(()));
        assert_eq!(validate_smrw(0x00C0, 64), Ok(()));
        assert_eq!(validate_smrw(0x00FC, 4), Ok(()));
        assert_eq!(validate_smrw(0, 0), Err(ValidationError::InvalidLength));
        assert_eq!(validate_smrw(0, 65), Err(ValidationError::InvalidLength));
        assert_eq!(
            validate_smrw(0x00C1, 64),
            Err(ValidationError::OutOfBounds)
        );
        assert_eq!(
            validate_smrw(0x00FC, 8),
            Err(ValidationError::OutOfBounds)
        );
        assert_eq!(
            validate_smrw(u16::MAX, 1),
            Err(ValidationError::OutOfBounds)
        );
    }

    #[test]
    fn pwup_bounds_and_permissions() {
        assert_eq!(validate_pwup(0, 0, true, true), Ok(()));
        assert_eq!(validate_pwup(1, 0, true, true), Ok(()));
        assert_eq!(validate_pwup(1, 3, true, true), Ok(()));
        assert_eq!(
            validate_pwup(0, 1, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_pwup(2, 0, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_pwup(1, 4, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_pwup(1, 1, false, true),
            Err(ValidationError::PermissionDenied)
        );
        assert_eq!(
            validate_pwup(1, 1, true, false),
            Err(ValidationError::PermissionDenied)
        );
    }

    #[test]
    fn gpu_power_and_mux_validation() {
        assert_eq!(validate_gpu_power(1, 150, true, true), Ok(()));
        assert_eq!(validate_gpu_power(1, 175, true, true), Ok(()));
        assert_eq!(validate_gpu_power(2, 25, true, true), Ok(()));
        assert_eq!(validate_gpu_power(2, 0, true, true), Ok(()));
        assert_eq!(
            validate_gpu_power(1, 301, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_gpu_power(2, 101, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_gpu_power(3, 25, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_gpu_power(1, 150, false, true),
            Err(ValidationError::PermissionDenied)
        );
        assert_eq!(
            validate_gpu_power(1, 150, true, false),
            Err(ValidationError::PermissionDenied)
        );

        assert_eq!(validate_mux_set(0, 1, true, true), Ok(()));
        assert_eq!(validate_mux_set(0, 2, true, true), Ok(()));
        assert_eq!(validate_mux_set(0, 4, true, true), Ok(()));
        assert_eq!(
            validate_mux_set(0, 0, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_mux_set(0, 5, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_mux_set(2, 2, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_mux_set(0, 2, false, true),
            Err(ValidationError::PermissionDenied)
        );
        assert_eq!(ValidationError::OutOfBounds.to_errno(), -34);
        assert_eq!(ValidationError::ReadOnlyOrForbidden.to_errno(), -13);
        assert_eq!(ValidationError::InvalidLength.to_errno(), -22);
        assert_eq!(ValidationError::InvalidArgument.to_errno(), -22);
        assert_eq!(ValidationError::PermissionDenied.to_errno(), -1);
    }

    #[test]
    fn ec_write_rejects_nonzero_pad_and_mux_rejects_missing_sys_admin() {
        assert_eq!(
            validate_ec_write(0x0783, 1, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_ec_write(0x0783, 0xFF, true, true),
            Err(ValidationError::InvalidArgument)
        );
        assert_eq!(
            validate_ec_write(0x0783, 1, false, true),
            Err(ValidationError::PermissionDenied)
        );
        assert_eq!(
            validate_mux_set(0, 2, true, false),
            Err(ValidationError::PermissionDenied)
        );

        for addr in [0x0741, 0x0751, 0x0752, 0x0753, 0x0783, 0x0786] {
            assert_eq!(validate_ec_write(addr, 0, true, true), Ok(()));
        }
        for addr in [0x07B9, 0x07CC, 0x0F00, 0x0F5F, 0x1804, 0x1809] {
            assert_eq!(validate_ec_write(addr, 0, true, true), Ok(()));
        }

        for addr in [0x0740, 0x0750, 0x0754, 0x0782, 0x0787, 0x07B8, 0x07BA] {
            assert_eq!(
                validate_ec_write(addr, 0, true, true),
                Err(ValidationError::ReadOnlyOrForbidden)
            );
        }
        for addr in [0x07CB, 0x07CD, 0x0EFF, 0x0F60, 0x1803, 0x1808, 0x180A] {
            assert_eq!(
                validate_ec_write(addr, 0, true, true),
                Err(ValidationError::ReadOnlyOrForbidden)
            );
        }
    }
}
