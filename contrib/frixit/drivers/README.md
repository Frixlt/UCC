# Mechrevo driver recovery after Linux 7.3 upgrades

On the tested YAOSHI X6AR55xY, Linux 7.3 removed the legacy `struct acpi_driver`
API. Three ACPI-only drivers then failed compilation and prevented the entire
`tuxedo-mechrevo/custom` DKMS bundle from installing, including Uniwill, keyboard
and lightbar modules. A working build for another installed kernel does not
provide modules for the currently booted kernel.

`linux-7.3-acpi-and-lightbar.patch` was compiled against the existing custom
TUXEDO driver snapshot based on upstream commit
`0b2f8c6ab7ef709343ae74b3de51dcae45357641`, using CachyOS 7.3.0-2 headers.
It preserves all 27 modules: Clevo ACPI, TUXI and GXTP use platform drivers with
explicit ACPI notification registration on Linux 7.3+, retaining the old paths
on older kernels. It also initializes all three RGB channel maxima in the
separate ITE 8291 lightbar driver for the Linux 7.2+ LED API.

The patch supplements the existing exact Mechrevo DMI/profile and ITE keyboard
RGB fixes; it does not replace them. Existing device calibration is retained.
Apply it to the driver source tree before `dkms build`, then install for the
specific kernel shown by `uname -r`. Back up the source files before patching.
The original installed modules for other kernels can remain installed.

`update-hook.patch` records the matching changes to Frixit's existing driver
update scripts. Copy the driver patch next to those scripts as
`0003-linux-7.3-acpi-lightbar.patch` and apply the hook patch in that directory.
The worker applies the compatibility fix before replacing the custom DKMS source,
builds with one job, and reports failure if any target kernel failed. The launcher
limits build CPU/memory and collects transient units so a failed update does not
block the next run. This integration is specific to those existing local hooks;
it is not automatically installed on other machines.

The recovered laptop explicitly loads `uniwill_wmi`, `tuxedo_keyboard`,
`tuxedo_io`, `ite_8291`, `ite_8291_lb` and `tuxedo_nb02_nvidia_power_ctrl` through
`/etc/modules-load.d/frixit-control-center.conf`. The UCC daemon starts after
`systemd-modules-load.service`. Kernel upgrades still require matching headers
and a successful DKMS build before rebooting into the new kernel.

Control Center exposes `GetDriverHealthJSON` separately from its D-Bus connection
status. The dashboard displays a red warning for a missing hardware module/device,
a daemon still using its pre-driver fallback, or missing keyboard/lightbar
interfaces on the exact Mechrevo models. The session agent checks after a 15-second
login grace period, retries unavailable notification services, coalesces repeated
identical failures and closes its warning after recovery. Checks are read-only;
they do not load drivers, restart services or change hardware settings.

## Verified state for release 0.3.0

On 22 October 2026, all 27 modules built and installed for 7.3.0-2-cachyos;
`/dev/tuxedo_io`, 126 keyboard zones and the Lightbar interface returned. A real
Flash test ended with RGB 0/0/0, and driver health returned `healthy: true`.
The update scripts and modules-load configuration were installed locally.
A subsequent reboot after the final repair is not claimed as separately tested.

This driver patch is distributed as source with the native application's release;
no private DKMS tree, machine configuration or prebuilt kernel modules are shipped.
For application installation see [BUILDING.md](../../../docs/BUILDING.md).

## Keyboard brightness with KDE / UPower

`ite8291-global-brightness.patch` fixes redundant RGB transfers when UPower
sets brightness on all 126 per-key LED devices. These devices share a single
hardware brightness. The driver now sends a frame only when brightness or the
scaled RGB row changes. Color changes at unchanged brightness still work;
buffered edits still flush together, and a failed USB flush remains dirty.
This patch changes the GPL-2.0 TUXEDO driver; it retains the driver's license.

Copy it next to the existing updater as
`0004-ite8291-global-brightness.patch`. The updated `update-hook.patch` requires
both patches 0003 and 0004 before replacing the installed DKMS source. Rebuild
and install for every installed kernel with matching headers. Reloading the
keyboard LED module resets live colors, so save and restore the current RGB
values and brightness or reboot after installing.

To exercise the actual patched callbacks with a mocked USB/LED device:

```sh
python3 tests/test_ite8291_brightness.py /path/to/patched/tuxedo-drivers
```

The test checks a 126-zone brightness update, unchanged scaled colors, color-only
writes, failed transfer retries, buffering and zero/maximum brightness. A full
kernel-module build and a live brightness timing check provide separate hardware
validation.

On 23 October 2026, the patched 27-module bundle built and installed for both
7.3.0-2-cachyos and 6.18.55-2-cachyos-lts. Reloading only `ite_8291` on the running
7.3 kernel preserved all 126 RGB values and restored the original brightness.
A UPower brightness call measured about 966 ms before the patch; the first two
calls after reloading measured 62 ms and 9 ms. These are spot measurements on
this laptop, not latency guarantees for other keyboards or desktop sessions.
