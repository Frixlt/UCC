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
