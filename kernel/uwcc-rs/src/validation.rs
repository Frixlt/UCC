// SPDX-License-Identifier: GPL-2.0-only
//! Strict `no_std` EC/SMRW/PWUP address, bounds, and permission validation.
//!
//! Shared between the `uwcc-rs` kernel module and host-side unit tests.

/// Maximum payload length for `UWCC_IOC_SMRW` (matches `UWCC_SMRW_MAX_LEN`).
pub(crate) const UWCC_SMRW_MAX_LEN: u16 = 64;

/// Maximum addressable byte size of the Uniwill SMRW factory limits table.
pub(crate) const UWCC_SMRW_TABLE_LIMIT: u16 = 0x0400;

/// Maximum allowed `PWUP` boost level.
pub(crate) const UWCC_PWUP_MAX_LEVEL: u32 = 3;

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
/// `0x0464`/`0x0465` and `0x046C`/`0x046B` tachometer RPM), read-only chassis
/// identity (`0x0748` `UWCC_EC_REG_PROJECT_ID`), low firmware/BIOS registers
/// (`< 0x0400`), and unmapped regions are strictly rejected.
#[inline]
pub(crate) const fn is_valid_ec_write_addr(addr: u16) -> bool {
    match addr {
        // TDP limits (PL1=0x0740, PL2=0x0741, PL4=0x0742) and thermal/power controls
        // (explicitly excludes 0x0748 UWCC_EC_REG_PROJECT_ID)
        0x0740..=0x0745 => true,
        // Fan manual mode (0x0751), perf mode (0x0752), charge control & fan PWM (0x075B/0x075C)
        0x0751..=0x075C => true,
        // Hardware switches (0x0760 FN_LOCK, 0x0761 WIN_LOCK, 0x0762 USB_SLEEP_CHARGING),
        // battery charge thresholds (0x0765 CHARGE_START_PCT, 0x0766 CHARGE_END_PCT),
        // and Lightbar / chassis LED EC power switches (0x0767/0x0768 LIGHTBAR_POWER)
        0x0760..=0x0768 => true,
        // Additional Uniwill EC hardware switch registers
        0x0770..=0x078F => true,
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
            validate_ec_write(0x0740, 0, false, true),
            Err(ValidationError::PermissionDenied)
        );
        assert_eq!(
            validate_ec_write(0x0740, 0, true, false),
            Err(ValidationError::PermissionDenied)
        );

        // Allowed writable registers (including 0x0740..=0x0745 and 0x0760..=0x0768).
        for addr in [
            0x0740, 0x0741, 0x0742, 0x0745, 0x0751, 0x0752, 0x075B, 0x075C, 0x0760, 0x0761,
            0x0762, 0x0765, 0x0766, 0x0767, 0x0768, 0x0F00, 0x0F5C, 0x1804, 0x1809,
        ] {
            assert_eq!(validate_ec_write(addr, 0, true, true), Ok(()));
        }

        // Full 0x0760..=0x0768 range must be writable.
        for addr in 0x0760..=0x0768 {
            assert_eq!(validate_ec_write(addr, 0, true, true), Ok(()));
        }

        // Read-only telemetry registers and 0x0748 (PROJECT_ID) must be rejected.
        for addr in [0x043E, 0x044F, 0x0464, 0x0465, 0x046B, 0x046C, 0x0746, 0x0748, 0x074E] {
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
        assert_eq!(validate_smrw(0x03C0, 64), Ok(()));
        assert_eq!(validate_smrw(0, 0), Err(ValidationError::InvalidLength));
        assert_eq!(validate_smrw(0, 65), Err(ValidationError::InvalidLength));
        assert_eq!(
            validate_smrw(0x03C1, 64),
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
}
