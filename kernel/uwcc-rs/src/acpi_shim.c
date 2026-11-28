// SPDX-License-Identifier: GPL-2.0-only
/*
 * Minimal C ACPI/WMI/hwmon/miscdevice FFI shim for the uwcc-rs Rust-for-Linux driver.
 *
 * Wraps Linux 7.3+ kernel C APIs not yet exposed through stable Rust bindings:
 *   - ACPI INOU0000 method evaluation (ECRR, ECRW, SMRW, PWUP)
 *   - Modern struct wmi_driver event notification registration
 *   - Native hwmon_device_register_with_info() telemetry
 *   - /dev/uwcc miscdevice with full file_operations (.open, .unlocked_ioctl,
 *     .compat_ioctl, .read, .poll) delegating validation/dispatch and ring buffer
 *     management to Rust (uwcc_rs.rs)
 *
 * D3cold invariant (ADR-04 / ADR-05):
 *   All hwmon temperature, fan RPM, and PWM readings are fetched strictly
 *   from EC registers via ACPI ECRR. PCI GPU config space and NVIDIA power
 *   interfaces are never touched, preserving RTX dGPU D3cold residency.
 */

#ifdef __KERNEL__

#include <asm/byteorder.h>
#include <linux/acpi.h>
#include <linux/capability.h>
#include <linux/device.h>
#include <linux/err.h>
#include <linux/errno.h>
#include <linux/minmax.h>
#include <linux/fs.h>
#include <linux/hwmon.h>
#include <linux/ktime.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/poll.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <linux/wait.h>
#if IS_ENABLED(CONFIG_ACPI_WMI)
#include <linux/wmi.h>
#endif

#include "../include/uapi/uwcc.h"
#include "acpi_shim.h"

/* ACPI hardware ID for Uniwill EC control interface */
#define UWCC_ACPI_HID "INOU0000"

/* Standard Uniwill WMI event GUID */
#define UWCC_WMI_EVENT_GUID "ABBC0F72-8EA1-11D1-00A0-C90629100000"

/* Uniwill EC register addresses used by native hwmon and identity probe */
#define UWCC_EC_REG_PROJECT_ID   0x0740U
#define UWCC_EC_REG_EC_VER_HI    0x0770U
#define UWCC_EC_REG_EC_VER_LO    0x0771U

/* D3cold-safe EC temperature registers (degrees Celsius) */
#define UWCC_EC_REG_CPU_TEMP     0x043EU
#define UWCC_EC_REG_GPU_TEMP     0x044FU
#define UWCC_EC_TEMP_MAX_VALID_C 150U

/* Real tachometer RPM registers (16-bit: MSB/LSB) */
#define UWCC_EC_REG_FAN1_RPM_HI  0x0464U
#define UWCC_EC_REG_FAN1_RPM_LO  0x0465U
#define UWCC_EC_REG_FAN2_RPM_HI  0x046CU
#define UWCC_EC_REG_FAN2_RPM_LO  0x046DU

/* Fan PWM duty registers (Uniwill 0..=200 scale) */
#define UWCC_EC_REG_PWM1         0x075BU
#define UWCC_EC_REG_PWM2         0x075CU
#define UWCC_EC_PWM_MAX_RAW      200U

static DEFINE_MUTEX(uwcc_ec_lock);
static DEFINE_SPINLOCK(uwcc_state_lock);
static DECLARE_WAIT_QUEUE_HEAD(uwcc_wmi_wq);

static acpi_handle uwcc_acpi_handle;
static bool uwcc_has_ecrr;
static bool uwcc_has_ecrw;
static bool uwcc_has_smrw;
static bool uwcc_has_pwup;
static bool uwcc_has_pwbt;
static bool uwcc_has_pwdb;
static acpi_handle uwcc_mux_handle;
static bool uwcc_has_mux;
#if IS_ENABLED(CONFIG_ACPI_WMI)
static bool uwcc_wmi_registered;
#endif
static bool uwcc_misc_registered;
static struct device *uwcc_hwmon_dev;

/* --------------------------------------------------------------------------
 * Internal ACPI helpers (caller must hold uwcc_ec_lock)
 * -------------------------------------------------------------------------- */

static int uwcc_ec_read_locked(u16 addr, u8 *out_val)
{
	union acpi_object arg;
	struct acpi_object_list params;
	unsigned long long result = 0;
	acpi_status status;

	if (!uwcc_acpi_handle || !uwcc_has_ecrr)
		return -ENODEV;

	arg.type = ACPI_TYPE_INTEGER;
	arg.integer.value = addr;

	params.count = 1;
	params.pointer = &arg;

	status = acpi_evaluate_integer(uwcc_acpi_handle, "ECRR", &params, &result);
	if (ACPI_FAILURE(status))
		return -EIO;

	*out_val = (u8)(result & 0xFFU);
	return 0;
}

static int uwcc_ec_write_locked(u16 addr, u8 val)
{
	union acpi_object args[2];
	struct acpi_object_list params;
	unsigned long long result = 0;
	acpi_status status;

	if (!uwcc_acpi_handle || !uwcc_has_ecrw)
		return -ENODEV;

	args[0].type = ACPI_TYPE_INTEGER;
	args[0].integer.value = addr;
	args[1].type = ACPI_TYPE_INTEGER;
	args[1].integer.value = val;

	params.count = 2;
	params.pointer = args;

	status = acpi_evaluate_integer(uwcc_acpi_handle, "ECRW", &params, &result);
	if (ACPI_FAILURE(status))
		return -EIO;

	return 0;
}

/* --------------------------------------------------------------------------
 * Public serialized ACPI FFI
 * -------------------------------------------------------------------------- */

int uwcc_acpi_ec_read(u16 addr, u8 *out_val)
{
	int ret;

	if (!out_val)
		return -EINVAL;

	mutex_lock(&uwcc_ec_lock);
	ret = uwcc_ec_read_locked(addr, out_val);
	mutex_unlock(&uwcc_ec_lock);

	return ret;
}

int uwcc_acpi_ec_write(u16 addr, u8 val)
{
	int ret;

	mutex_lock(&uwcc_ec_lock);
	ret = uwcc_ec_write_locked(addr, val);
	mutex_unlock(&uwcc_ec_lock);

	return ret;
}

int uwcc_acpi_smrw(u16 offset, u16 len, u8 *out_buf)
{
	u16 pos;
	int ret = 0;

	if (!out_buf || len == 0 || len > UWCC_SMRW_MAX_LEN)
		return -EINVAL;
	if ((u32)offset + (u32)len > UWCC_SMRW_TABLE_LIMIT)
		return -ERANGE;

	mutex_lock(&uwcc_ec_lock);

	if (!uwcc_acpi_handle || !uwcc_has_smrw) {
		mutex_unlock(&uwcc_ec_lock);
		return -ENODEV;
	}

	memset(out_buf, 0, len);

	/*
	 * Uniwill INOU0000 Method(SMRW, 1) accepts a single ACPI_TYPE_BUFFER:
	 *   buf[0] = 0xBB (read DWord from active GFID power table)
	 *   buf[1] = 8-bit byte offset (REOF)
	 *   buf[2..63] = 0
	 * and returns a 32-bit integer (or buffer on alternate firmware).
	 */
	for (pos = 0; pos < len; pos += 4U) {
		/* 64-byte zero-padded input buffer prevents AE_AML_BUFFER_LIMIT in DSDT CreateField */
		u8 req_buf[64];
		union acpi_object arg;
		struct acpi_object_list params;
		struct acpi_buffer output = { ACPI_ALLOCATE_BUFFER, NULL };
		union acpi_object *obj;
		acpi_status status;
		u16 cur_off = offset + pos;

		memset(req_buf, 0, sizeof(req_buf));
		req_buf[0] = 0xBB;
		req_buf[1] = (u8)cur_off;

		arg.type = ACPI_TYPE_BUFFER;
		arg.buffer.length = sizeof(req_buf);
		arg.buffer.pointer = req_buf;

		params.count = 1;
		params.pointer = &arg;

		status = acpi_evaluate_object(uwcc_acpi_handle, "SMRW", &params, &output);
		if (ACPI_FAILURE(status) || !output.pointer) {
			kfree(output.pointer);
			if (status == AE_AML_BUFFER_LIMIT && pos > 0)
				ret = 0;
			else
				ret = -EIO;
			break;
		}

		obj = (union acpi_object *)output.pointer;
		if (obj->type == ACPI_TYPE_INTEGER) {
			__le32 dword = cpu_to_le32((u32)(obj->integer.value & 0xFFFFFFFFULL));
			size_t chunk = min_t(size_t, 4U, (size_t)(len - pos));

			memcpy(out_buf + pos, &dword, chunk);
			kfree(output.pointer);
		} else if (obj->type == ACPI_TYPE_BUFFER && obj->buffer.pointer) {
			size_t copy_len = min_t(size_t, (size_t)(len - pos),
						(size_t)obj->buffer.length);

			memcpy(out_buf + pos, obj->buffer.pointer, copy_len);
			kfree(output.pointer);
			break;
		} else {
			kfree(output.pointer);
			ret = -EPROTO;
			break;
		}
	}

	mutex_unlock(&uwcc_ec_lock);
	return ret;
}

int uwcc_acpi_pwup(u32 enable, u32 level)
{
	union acpi_object arg;
	struct acpi_object_list params;
	acpi_status status;
	int ret = 0;

	if (enable > 1U || level > 3U)
		return -EINVAL;
	if (enable == 0U && level != 0U)
		return -EINVAL;

	mutex_lock(&uwcc_ec_lock);

	if (!uwcc_acpi_handle || !uwcc_has_pwup) {
		mutex_unlock(&uwcc_ec_lock);
		return -ENODEV;
	}

	/* Uniwill INOU0000 Method(PWUP, 1) takes 1 integer and notifies NPCF & dGPU. */
	arg.type = ACPI_TYPE_INTEGER;
	arg.integer.value = enable ? (u64)level : 0ULL;

	params.count = 1;
	params.pointer = &arg;

	status = acpi_evaluate_object(uwcc_acpi_handle, "PWUP", &params, NULL);
	if (ACPI_FAILURE(status))
		ret = -EIO;

	mutex_unlock(&uwcc_ec_lock);
	return ret;
}

int uwcc_acpi_gpu_power(u32 op, u32 value)
{
	union acpi_object arg;
	struct acpi_object_list params;
	const char *method;
	acpi_status status;
	int ret = 0;

	mutex_lock(&uwcc_ec_lock);

	if (!uwcc_acpi_handle) {
		mutex_unlock(&uwcc_ec_lock);
		return -ENODEV;
	}

	if (op == UWCC_GPU_POWER_OP_BASE_TGP) {
		if (!uwcc_has_pwbt) {
			mutex_unlock(&uwcc_ec_lock);
			return -ENODEV;
		}
		method = "PWBT";
	} else if (op == UWCC_GPU_POWER_OP_DYNAMIC_BOOST) {
		if (!uwcc_has_pwdb) {
			mutex_unlock(&uwcc_ec_lock);
			return -ENODEV;
		}
		method = "PWDB";
	} else {
		mutex_unlock(&uwcc_ec_lock);
		return -EINVAL;
	}

	arg.type = ACPI_TYPE_INTEGER;
	arg.integer.value = (u64)value;
	params.count = 1;
	params.pointer = &arg;

	status = acpi_evaluate_object(uwcc_acpi_handle, (char *)method, &params, NULL);
	if (ACPI_FAILURE(status))
		ret = -EIO;

	mutex_unlock(&uwcc_ec_lock);
	return ret;
}

int uwcc_acpi_mux_get(u32 *out_active_gpu, u32 *out_boot_mode)
{
	union acpi_object arg;
	struct acpi_object_list params;
	unsigned long long active_val = 0;
	unsigned long long mode_val = 0;
	acpi_status status;

	if (!out_active_gpu || !out_boot_mode)
		return -EINVAL;

	mutex_lock(&uwcc_ec_lock);

	if (!uwcc_mux_handle || !uwcc_has_mux) {
		mutex_unlock(&uwcc_ec_lock);
		return -ENODEV;
	}

	arg.type = ACPI_TYPE_INTEGER;
	arg.integer.value = 0ULL;
	params.count = 1;
	params.pointer = &arg;

	status = acpi_evaluate_integer(uwcc_mux_handle, "MXDS", &params, &active_val);
	if (ACPI_FAILURE(status)) {
		mutex_unlock(&uwcc_ec_lock);
		return -EIO;
	}

	status = acpi_evaluate_integer(uwcc_mux_handle, "MXDM", &params, &mode_val);
	if (ACPI_FAILURE(status)) {
		mutex_unlock(&uwcc_ec_lock);
		return -EIO;
	}

	*out_active_gpu = (u32)(active_val & 0x0FU);
	*out_boot_mode = (u32)(mode_val & 0x07U);

	mutex_unlock(&uwcc_ec_lock);
	return 0;
}

int uwcc_acpi_mux_set(u32 boot_mode)
{
	union acpi_object arg;
	struct acpi_object_list params;
	unsigned long long result = 0;
	acpi_status status;

	if (boot_mode < UWCC_MUX_MODE_DYNAMIC || boot_mode > UWCC_MUX_MODE_MSHYBRID)
		return -EINVAL;

	mutex_lock(&uwcc_ec_lock);

	if (!uwcc_mux_handle || !uwcc_has_mux) {
		mutex_unlock(&uwcc_ec_lock);
		return -ENODEV;
	}

	/*
	 * Call MXDM(boot_mode) to persist the BIOS MUX mode via SMI 0xB8 for next reboot.
	 * Never invokes live MXDS GPIO switching to avoid blank-screen/display-link races.
	 */
	arg.type = ACPI_TYPE_INTEGER;
	arg.integer.value = (u64)(boot_mode & 0x07U);
	params.count = 1;
	params.pointer = &arg;

	status = acpi_evaluate_integer(uwcc_mux_handle, "MXDM", &params, &result);
	if (ACPI_FAILURE(status) || result == 0ULL) {
		mutex_unlock(&uwcc_ec_lock);
		return -EIO;
	}

	mutex_unlock(&uwcc_ec_lock);
	return 0;
}

/* --------------------------------------------------------------------------
 * Modern struct wmi_driver bridge -> Rust ring buffer
 * -------------------------------------------------------------------------- */

#if IS_ENABLED(CONFIG_ACPI_WMI)
static void uwcc_wmi_notify(struct wmi_device *wdev, union acpi_object *obj)
{
	u32 code = 0;
	u32 payload = 0;

	(void)wdev;

	if (!obj)
		return;

	if (obj->type == ACPI_TYPE_INTEGER) {
		code = (u32)(obj->integer.value & 0xFFFFFFFFULL);
		payload = (u32)((obj->integer.value >> 32) & 0xFFFFFFFFULL);
	} else if (obj->type == ACPI_TYPE_BUFFER &&
		   obj->buffer.pointer && obj->buffer.length >= 4) {
		memcpy(&code, obj->buffer.pointer, sizeof(code));
		if (obj->buffer.length >= 8)
			memcpy(&payload, obj->buffer.pointer + 4, sizeof(payload));
	} else {
		return;
	}

	uwcc_rs_on_wmi_event(code, payload);
}

static const struct wmi_device_id uwcc_wmi_id_table[] = {
	{ UWCC_WMI_EVENT_GUID, NULL },
	{ }
};

static struct wmi_driver uwcc_wmi_driver = {
	.driver = {
		.name = "uwcc_wmi",
	},
	.id_table = uwcc_wmi_id_table,
	.notify = uwcc_wmi_notify,
	.no_singleton = true,
};
#endif

/* --------------------------------------------------------------------------
 * /dev/uwcc miscdevice with full file_operations delegating to Rust
 * -------------------------------------------------------------------------- */

static int uwcc_dev_open(struct inode *inode, struct file *filp)
{
	(void)inode;
	(void)filp;

	return uwcc_rs_is_ready() ? 0 : -ENODEV;
}

static long uwcc_dev_ioctl(struct file *filp, unsigned int cmd, unsigned long arg)
{
	void __user *argp = (void __user *)arg;
	bool writable = (filp->f_mode & FMODE_WRITE) != 0;
	int ret;

	switch (cmd) {
	case UWCC_IOC_GET_INFO: {
		struct uwcc_info info;

		memset(&info, 0, sizeof(info));
		ret = uwcc_rs_ioctl_get_info(&info);
		if (ret)
			return ret;
		if (copy_to_user(argp, &info, sizeof(info)))
			return -EFAULT;
		return 0;
	}

	case UWCC_IOC_EC_READ: {
		struct uwcc_ec_reg reg;

		if (copy_from_user(&reg, argp, sizeof(reg)))
			return -EFAULT;
		ret = uwcc_rs_ioctl_ec_read(&reg);
		if (ret)
			return ret;
		if (copy_to_user(argp, &reg, sizeof(reg)))
			return -EFAULT;
		return 0;
	}

	case UWCC_IOC_EC_WRITE: {
		struct uwcc_ec_reg reg;

		if (copy_from_user(&reg, argp, sizeof(reg)))
			return -EFAULT;
		return uwcc_rs_ioctl_ec_write(&reg, writable, capable(CAP_SYS_ADMIN));
	}

	case UWCC_IOC_SMRW: {
		struct uwcc_smrw smrw;

		if (copy_from_user(&smrw, argp, sizeof(smrw)))
			return -EFAULT;
		ret = uwcc_rs_ioctl_smrw(&smrw);
		if (ret)
			return ret;
		if (copy_to_user(argp, &smrw, sizeof(smrw)))
			return -EFAULT;
		return 0;
	}

	case UWCC_IOC_PWUP: {
		struct uwcc_pwup pwup;

		if (copy_from_user(&pwup, argp, sizeof(pwup)))
			return -EFAULT;
		return uwcc_rs_ioctl_pwup(&pwup, writable, capable(CAP_SYS_ADMIN));
	}

	case UWCC_IOC_GPU_POWER: {
		struct uwcc_gpu_power req;

		if (copy_from_user(&req, argp, sizeof(req)))
			return -EFAULT;
		return uwcc_rs_ioctl_gpu_power(&req, writable, capable(CAP_SYS_ADMIN));
	}

	case UWCC_IOC_MUX_GET: {
		struct uwcc_mux_state st;

		memset(&st, 0, sizeof(st));
		ret = uwcc_rs_ioctl_mux_get(&st);
		if (ret)
			return ret;
		if (copy_to_user(argp, &st, sizeof(st)))
			return -EFAULT;
		return 0;
	}

	case UWCC_IOC_MUX_SET: {
		struct uwcc_mux_state req;

		if (copy_from_user(&req, argp, sizeof(req)))
			return -EFAULT;
		return uwcc_rs_ioctl_mux_set(&req, writable, capable(CAP_SYS_ADMIN));
	}

	default:
		return -ENOTTY;
	}
}

static ssize_t uwcc_dev_read(struct file *filp, char __user *buf,
			     size_t count, loff_t *ppos)
{
	struct uwcc_wmi_event ev;
	int ret;

	(void)ppos;

	if (count < sizeof(ev))
		return -EINVAL;

	for (;;) {
		if (uwcc_rs_wmi_pop(&ev)) {
			if (copy_to_user(buf, &ev, sizeof(ev)))
				return -EFAULT;
			return (ssize_t)sizeof(ev);
		}

		if (!uwcc_rs_is_ready())
			return 0;

		if (filp->f_flags & O_NONBLOCK)
			return -EAGAIN;

		ret = wait_event_interruptible(uwcc_wmi_wq,
					       uwcc_rs_wmi_has_data_or_shutdown());
		if (ret)
			return -ERESTARTSYS;
	}
}

static __poll_t uwcc_dev_poll(struct file *filp, struct poll_table_struct *wait)
{
	__poll_t mask = 0;

	poll_wait(filp, &uwcc_wmi_wq, wait);

	if (uwcc_rs_wmi_has_data())
		mask |= EPOLLIN | EPOLLRDNORM;
	else if (!uwcc_rs_is_ready())
		mask |= EPOLLHUP;

	return mask;
}

static const struct file_operations uwcc_fops = {
	.owner = THIS_MODULE,
	.open = uwcc_dev_open,
	.unlocked_ioctl = uwcc_dev_ioctl,
	.compat_ioctl = compat_ptr_ioctl,
	.read = uwcc_dev_read,
	.poll = uwcc_dev_poll,
};

static struct miscdevice uwcc_miscdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = UWCC_DEVICE_NAME,
	.fops = &uwcc_fops,
};

int uwcc_misc_register(void)
{
	int ret;

	if (uwcc_misc_registered)
		return 0;

	ret = misc_register(&uwcc_miscdev);
	if (ret == 0)
		uwcc_misc_registered = true;

	return ret;
}

void uwcc_misc_deregister(void)
{
	if (uwcc_misc_registered) {
		misc_deregister(&uwcc_miscdev);
		uwcc_misc_registered = false;
	}
}

/* --------------------------------------------------------------------------
 * Native hwmon (/sys/class/hwmon/hwmonX) — strictly EC-backed, D3cold-safe
 * -------------------------------------------------------------------------- */

static const char *const uwcc_temp_labels[] = {
	"CPU (EC)",
	"GPU (EC)",
};

static const char *const uwcc_fan_labels[] = {
	"CPU Fan",
	"GPU Fan",
};

static umode_t uwcc_hwmon_is_visible(const void *drvdata,
				     enum hwmon_sensor_types type,
				     u32 attr, int channel)
{
	(void)drvdata;

	switch (type) {
	case hwmon_temp:
		if (channel >= 0 && channel < 2 &&
		    (attr == hwmon_temp_input || attr == hwmon_temp_label))
			return 0444;
		return 0;
	case hwmon_fan:
		if (channel >= 0 && channel < 2 &&
		    (attr == hwmon_fan_input || attr == hwmon_fan_label))
			return 0444;
		return 0;
	case hwmon_pwm:
		if (channel >= 0 && channel < 2 && attr == hwmon_pwm_input)
			return 0444;
		return 0;
	default:
		return 0;
	}
}

static int uwcc_hwmon_read(struct device *dev, enum hwmon_sensor_types type,
			   u32 attr, int channel, long *val)
{
	u8 hi = 0, lo = 0, raw = 0;
	u16 rpm;
	int ret;

	(void)dev;

	if (!val)
		return -EINVAL;

	switch (type) {
	case hwmon_temp:
		if (attr != hwmon_temp_input)
			return -EOPNOTSUPP;
		if (channel == 0)
			ret = uwcc_acpi_ec_read(UWCC_EC_REG_CPU_TEMP, &raw);
		else if (channel == 1)
			ret = uwcc_acpi_ec_read(UWCC_EC_REG_GPU_TEMP, &raw);
		else
			return -EINVAL;

		if (ret)
			return ret;
		if (raw == 0 || raw > UWCC_EC_TEMP_MAX_VALID_C)
			return -ENODATA;
		*val = (long)raw * 1000L;
		return 0;

	case hwmon_fan:
		if (attr != hwmon_fan_input)
			return -EOPNOTSUPP;

		mutex_lock(&uwcc_ec_lock);
		if (channel == 0) {
			ret = uwcc_ec_read_locked(UWCC_EC_REG_FAN1_RPM_HI, &hi);
			if (!ret)
				ret = uwcc_ec_read_locked(UWCC_EC_REG_FAN1_RPM_LO, &lo);
		} else if (channel == 1) {
			ret = uwcc_ec_read_locked(UWCC_EC_REG_FAN2_RPM_HI, &hi);
			if (!ret)
				ret = uwcc_ec_read_locked(UWCC_EC_REG_FAN2_RPM_LO, &lo);
		} else {
			ret = -EINVAL;
		}
		mutex_unlock(&uwcc_ec_lock);

		if (ret)
			return ret;

		rpm = ((u16)hi << 8) | (u16)lo;
		if (rpm == 0xFFFFU)
			return -ENODATA;
		*val = (long)rpm;
		return 0;

	case hwmon_pwm:
		if (attr != hwmon_pwm_input)
			return -EOPNOTSUPP;
		if (channel == 0)
			ret = uwcc_acpi_ec_read(UWCC_EC_REG_PWM1, &raw);
		else if (channel == 1)
			ret = uwcc_acpi_ec_read(UWCC_EC_REG_PWM2, &raw);
		else
			return -EINVAL;

		if (ret)
			return ret;
		if (raw > UWCC_EC_PWM_MAX_RAW)
			raw = UWCC_EC_PWM_MAX_RAW;
		/* Scale Uniwill 0..=200 duty cycle to standard Linux hwmon 0..=255 */
		*val = ((long)raw * 255L + 100L) / (long)UWCC_EC_PWM_MAX_RAW;
		return 0;

	default:
		return -EOPNOTSUPP;
	}
}

static int uwcc_hwmon_read_string(struct device *dev,
				  enum hwmon_sensor_types type,
				  u32 attr, int channel, const char **str)
{
	(void)dev;

	if (!str)
		return -EINVAL;

	if (type == hwmon_temp && attr == hwmon_temp_label &&
	    channel >= 0 && channel < 2) {
		*str = uwcc_temp_labels[channel];
		return 0;
	}

	if (type == hwmon_fan && attr == hwmon_fan_label &&
	    channel >= 0 && channel < 2) {
		*str = uwcc_fan_labels[channel];
		return 0;
	}

	return -EOPNOTSUPP;
}

static const struct hwmon_channel_info *const uwcc_hwmon_info[] = {
	HWMON_CHANNEL_INFO(temp,
			   HWMON_T_INPUT | HWMON_T_LABEL,
			   HWMON_T_INPUT | HWMON_T_LABEL),
	HWMON_CHANNEL_INFO(fan,
			   HWMON_F_INPUT | HWMON_F_LABEL,
			   HWMON_F_INPUT | HWMON_F_LABEL),
	HWMON_CHANNEL_INFO(pwm,
			   HWMON_PWM_INPUT,
			   HWMON_PWM_INPUT),
	NULL
};

static const struct hwmon_ops uwcc_hwmon_ops = {
	.is_visible = uwcc_hwmon_is_visible,
	.read = uwcc_hwmon_read,
	.read_string = uwcc_hwmon_read_string,
};

static const struct hwmon_chip_info uwcc_hwmon_chip_info = {
	.ops = &uwcc_hwmon_ops,
	.info = uwcc_hwmon_info,
};

int uwcc_hwmon_register(void)
{
	struct device *dev = uwcc_miscdev.this_device;
	struct device *hdev;

	if (uwcc_hwmon_dev)
		return 0;

	if (!dev)
		return -ENODEV;

	hdev = hwmon_device_register_with_info(dev, "uwcc", NULL,
					       &uwcc_hwmon_chip_info, NULL);
	if (IS_ERR(hdev))
		return PTR_ERR(hdev);

	uwcc_hwmon_dev = hdev;
	return 0;
}

void uwcc_hwmon_unregister(void)
{
	if (uwcc_hwmon_dev) {
		hwmon_device_unregister(uwcc_hwmon_dev);
		uwcc_hwmon_dev = NULL;
	}
}

/* --------------------------------------------------------------------------
 * ACPI INOU0000 discovery and lifecycle
 * -------------------------------------------------------------------------- */

static acpi_status uwcc_find_acpi_cb(acpi_handle handle, u32 level,
				     void *context, void **return_value)
{
	(void)level;
	(void)return_value;

	*(acpi_handle *)context = handle;
	return AE_CTRL_TERMINATE;
}

static acpi_status uwcc_find_mux_cb(acpi_handle handle, u32 level,
				    void *context, void **return_value)
{
	(void)level;
	(void)return_value;

	if (acpi_has_method(handle, "MXDS") && acpi_has_method(handle, "MXDM")) {
		*(acpi_handle *)context = handle;
		return AE_CTRL_TERMINATE;
	}
	return AE_OK;
}

int uwcc_acpi_init(struct uwcc_acpi_caps *out_caps)
{
	acpi_handle found = NULL;
	acpi_status status;
	u8 proj = 0, ver_hi = 0, ver_lo = 0;
	u32 flags = 0;

	if (!out_caps)
		return -EINVAL;

	memset(out_caps, 0, sizeof(*out_caps));

	status = acpi_get_devices(UWCC_ACPI_HID, uwcc_find_acpi_cb, &found, NULL);
	if (ACPI_FAILURE(status) || !found)
		return -ENODEV;

	mutex_lock(&uwcc_ec_lock);
	uwcc_acpi_handle = found;
	uwcc_has_ecrr = acpi_has_method(found, "ECRR");
	uwcc_has_ecrw = acpi_has_method(found, "ECRW");
	uwcc_has_smrw = acpi_has_method(found, "SMRW");
	uwcc_has_pwup = acpi_has_method(found, "PWUP");
	uwcc_has_pwbt = acpi_has_method(found, "PWBT");
	uwcc_has_pwdb = acpi_has_method(found, "PWDB");

	if (!uwcc_has_ecrr) {
		uwcc_acpi_handle = NULL;
		mutex_unlock(&uwcc_ec_lock);
		return -ENODEV;
	}

	flags |= UWCC_FLAG_HAS_EC;
	if (uwcc_has_smrw)
		flags |= UWCC_FLAG_HAS_SMRW;
	if (uwcc_has_pwup)
		flags |= UWCC_FLAG_HAS_PWUP;
	if (uwcc_has_pwbt)
		flags |= UWCC_FLAG_HAS_PWBT;
	if (uwcc_has_pwdb)
		flags |= UWCC_FLAG_HAS_PWDB;

	/* Discover NVIDIA DDS / Parade PS8461E MUX device (MXDS + MXDM) */
	uwcc_mux_handle = NULL;
	uwcc_has_mux = false;
	if (ACPI_SUCCESS(acpi_get_handle(NULL, "\\_SB.PC00.RP09.PXSX.LCD0", &uwcc_mux_handle)) &&
	    acpi_has_method(uwcc_mux_handle, "MXDS") &&
	    acpi_has_method(uwcc_mux_handle, "MXDM")) {
		uwcc_has_mux = true;
	} else {
		uwcc_mux_handle = NULL;
		acpi_walk_namespace(ACPI_TYPE_DEVICE, ACPI_ROOT_OBJECT, 8,
				    uwcc_find_mux_cb, NULL, &uwcc_mux_handle, NULL);
		if (uwcc_mux_handle)
			uwcc_has_mux = true;
	}
	if (uwcc_has_mux)
		flags |= UWCC_FLAG_HAS_MUX;

	if (uwcc_ec_read_locked(UWCC_EC_REG_PROJECT_ID, &proj) == 0)
		out_caps->project_id = proj;
	if (uwcc_ec_read_locked(UWCC_EC_REG_EC_VER_HI, &ver_hi) == 0 &&
	    uwcc_ec_read_locked(UWCC_EC_REG_EC_VER_LO, &ver_lo) == 0)
		out_caps->ec_version = ((u32)ver_hi << 8) | (u32)ver_lo;

	mutex_unlock(&uwcc_ec_lock);

#if IS_ENABLED(CONFIG_ACPI_WMI)
	if (wmi_has_guid(UWCC_WMI_EVENT_GUID)) {
		if (wmi_driver_register(&uwcc_wmi_driver) == 0) {
			uwcc_wmi_registered = true;
			flags |= UWCC_FLAG_HAS_WMI_EVENTS;
		}
	}
#endif

	out_caps->flags = flags;
	return 0;
}

void uwcc_acpi_exit(void)
{
	uwcc_hwmon_unregister();

#if IS_ENABLED(CONFIG_ACPI_WMI)
	if (uwcc_wmi_registered) {
		wmi_driver_unregister(&uwcc_wmi_driver);
		uwcc_wmi_registered = false;
	}
#endif

	mutex_lock(&uwcc_ec_lock);
	uwcc_acpi_handle = NULL;
	uwcc_has_ecrr = false;
	uwcc_has_ecrw = false;
	uwcc_has_smrw = false;
	uwcc_has_pwup = false;
	uwcc_has_pwbt = false;
	uwcc_has_pwdb = false;
	uwcc_mux_handle = NULL;
	uwcc_has_mux = false;
	mutex_unlock(&uwcc_ec_lock);

	wake_up_interruptible_all(&uwcc_wmi_wq);
}

/* --------------------------------------------------------------------------
 * Thin kernel helpers for Rust core (IRQ-safe spinlock, time, waitqueue)
 * -------------------------------------------------------------------------- */

unsigned long uwcc_state_lock_irqsave(void)
{
	unsigned long flags;

	spin_lock_irqsave(&uwcc_state_lock, flags);
	return flags;
}

void uwcc_state_unlock_irqrestore(unsigned long flags)
{
	spin_unlock_irqrestore(&uwcc_state_lock, flags);
}

u64 uwcc_ktime_get_ns(void)
{
	return ktime_get_ns();
}

void uwcc_wmi_wake_readers(void)
{
	wake_up_interruptible(&uwcc_wmi_wq);
}

void uwcc_wmi_wake_all(void)
{
	wake_up_interruptible_all(&uwcc_wmi_wq);
}

#endif /* __KERNEL__ */
