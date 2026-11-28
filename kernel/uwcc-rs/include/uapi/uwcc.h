/* SPDX-License-Identifier: GPL-2.0-only WITH Linux-syscall-note */
/*
 * Uniwill Control Center (uwcc-rs) userspace ABI header.
 *
 * Shared contract between the uwcc_rs Linux kernel module (/dev/uwcc)
 * and userspace clients (e.g. ucc-hardware UwccBackend).
 */

#ifndef _UAPI_UWCC_H
#define _UAPI_UWCC_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define UWCC_IOCTL_MAGIC        'U'
#define UWCC_DEVICE_NAME        "uwcc"
#define UWCC_DEVICE_PATH        "/dev/uwcc"

#define UWCC_API_VERSION        1U
#define UWCC_SMRW_MAX_LEN       64U
#define UWCC_WMI_RING_CAPACITY  64U

/* Capability flags reported in struct uwcc_info.flags */
#define UWCC_FLAG_HAS_EC         (1U << 0)
#define UWCC_FLAG_HAS_SMRW       (1U << 1)
#define UWCC_FLAG_HAS_PWUP       (1U << 2)
#define UWCC_FLAG_HAS_WMI_EVENTS (1U << 3)
#define UWCC_FLAG_HAS_HWMON      (1U << 4)

/**
 * struct uwcc_ec_reg - Single EC register read/write payload.
 * @addr: 16-bit Uniwill EC register address.
 * @val:  8-bit register value (output for EC_READ, input for EC_WRITE).
 * @_pad: Reserved padding byte; must be zero.
 */
struct uwcc_ec_reg {
	__u16 addr;
	__u8  val;
	__u8  _pad;
};

/**
 * struct uwcc_smrw - Factory power/thermal limits table (ACPI SMRW) payload.
 * @offset: Byte offset into the SMRW table.
 * @len:    Number of bytes to read (1..=UWCC_SMRW_MAX_LEN).
 * @data:   Output buffer receiving up to 64 bytes from ACPI SMRW.
 */
struct uwcc_smrw {
	__u16 offset;
	__u16 len;
	__u8  data[64];
};

/**
 * struct uwcc_pwup - Liquid-cooler power boost (ACPI PWUP) payload.
 * @enable: 0 to disable power boost, 1 to enable.
 * @level:  Boost level (0..=3; must be 0 when enable == 0).
 */
struct uwcc_pwup {
	__u32 enable;
	__u32 level;
};

/**
 * struct uwcc_info - Driver and platform capability descriptor.
 * @api_version: UAPI protocol version (UWCC_API_VERSION).
 * @project_id:  Uniwill chassis project identifier (e.g. 0x1A).
 * @ec_version:  Encoded EC firmware version (0 if unavailable).
 * @flags:       Bitmask of UWCC_FLAG_* capability bits.
 */
struct uwcc_info {
	__u32 api_version;
	__u32 project_id;
	__u32 ec_version;
	__u32 flags;
};

/**
 * struct uwcc_wmi_event - Event record returned by read() on /dev/uwcc.
 * @timestamp_ns: Monotonic kernel timestamp (CLOCK_MONOTONIC, nanoseconds).
 * @code:         WMI event code reported by firmware.
 * @payload:      Auxiliary event payload (0 if unused).
 */
struct uwcc_wmi_event {
	__u64 timestamp_ns;
	__u32 code;
	__u32 payload;
};

#define UWCC_IOC_GET_INFO  _IOR(UWCC_IOCTL_MAGIC, 0x01, struct uwcc_info)
#define UWCC_IOC_EC_READ   _IOWR(UWCC_IOCTL_MAGIC, 0x02, struct uwcc_ec_reg)
#define UWCC_IOC_EC_WRITE  _IOW(UWCC_IOCTL_MAGIC, 0x03, struct uwcc_ec_reg)
#define UWCC_IOC_SMRW      _IOWR(UWCC_IOCTL_MAGIC, 0x04, struct uwcc_smrw)
#define UWCC_IOC_PWUP      _IOW(UWCC_IOCTL_MAGIC, 0x05, struct uwcc_pwup)

#endif /* _UAPI_UWCC_H */
