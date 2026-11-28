/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Minimal C FFI shim header for uwcc-rs (ACPI INOU0000, WMI driver, hwmon, /dev/uwcc).
 *
 * Isolates kernel C APIs that do not yet have stable Rust-for-Linux wrappers
 * in Linux 7.3 (acpi_evaluate_*, wmi_driver_register, hwmon, miscdevice fops).
 */

#ifndef _UWCC_ACPI_SHIM_H
#define _UWCC_ACPI_SHIM_H

#ifdef __KERNEL__

#include <linux/types.h>

#include "../include/uapi/uwcc.h"

struct device;

/**
 * struct uwcc_acpi_caps - Capabilities discovered during ACPI/WMI probe.
 * @project_id: Uniwill project ID read from EC (0 if unavailable).
 * @ec_version: Encoded EC firmware version (0 if unavailable).
 * @flags:      Bitmask of UWCC_FLAG_* bits.
 */
struct uwcc_acpi_caps {
	u32 project_id;
	u32 ec_version;
	u32 flags;
};

/*
 * Entry points implemented in Rust (uwcc_rs.rs) and invoked by acpi_shim.c
 * for WMI event queuing, ioctl validation/dispatch, and /dev/uwcc read/poll.
 */
extern bool uwcc_rs_is_ready(void);
extern void uwcc_rs_on_wmi_event(u32 code, u32 payload);
extern bool uwcc_rs_wmi_pop(struct uwcc_wmi_event *out_event);
extern bool uwcc_rs_wmi_has_data(void);
extern bool uwcc_rs_wmi_has_data_or_shutdown(void);
extern int uwcc_rs_ioctl_get_info(struct uwcc_info *out_info);
extern int uwcc_rs_ioctl_ec_read(struct uwcc_ec_reg *reg);
extern int uwcc_rs_ioctl_ec_write(const struct uwcc_ec_reg *reg,
				  bool writable, bool sys_admin);
extern int uwcc_rs_ioctl_smrw(struct uwcc_smrw *smrw);
extern int uwcc_rs_ioctl_pwup(const struct uwcc_pwup *pwup,
			      bool writable, bool sys_admin);
extern int uwcc_rs_ioctl_gpu_power(const struct uwcc_gpu_power *req,
				   bool writable, bool sys_admin);
extern int uwcc_rs_ioctl_mux_get(struct uwcc_mux_state *out_state);
extern int uwcc_rs_ioctl_mux_set(const struct uwcc_mux_state *req,
				 bool writable, bool sys_admin);

/* Lifecycle */
int uwcc_acpi_init(struct uwcc_acpi_caps *out_caps);
void uwcc_acpi_exit(void);
int uwcc_misc_register(void);
void uwcc_misc_deregister(void);

/* Serialized ACPI INOU0000 calls (protected by internal EC mutex) */
int uwcc_acpi_ec_read(u16 addr, u8 *out_val);
int uwcc_acpi_ec_write(u16 addr, u8 val);
int uwcc_acpi_ec_rmw(u16 addr, u8 mask, u8 val);
int uwcc_acpi_smrw(u16 offset, u16 len, u8 *out_buf);
int uwcc_acpi_pwup(u32 enable, u32 level);
int uwcc_acpi_gpu_power(u32 op, u32 value);
int uwcc_acpi_mux_get(u32 *out_active_gpu, u32 *out_boot_mode);
int uwcc_acpi_mux_set(u32 boot_mode);

/* Native /sys/class/hwmon registration (D3cold-safe EC-only telemetry) */
int uwcc_hwmon_register(void);
void uwcc_hwmon_unregister(void);

/* Thin kernel helpers for Rust core (IRQ-safe spinlock, waitqueue, monotonic time) */
unsigned long uwcc_state_lock_irqsave(void);
void uwcc_state_unlock_irqrestore(unsigned long flags);
u64 uwcc_ktime_get_ns(void);
void uwcc_wmi_wake_readers(void);
void uwcc_wmi_wake_all(void);

#endif /* __KERNEL__ */

#endif /* _UWCC_ACPI_SHIM_H */
