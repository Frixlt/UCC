# ADR-06: Cross-Platform Linux-First & Windows Architecture (`M15`)

- **Status:** Accepted
- **Date:** 2026-11-27
- **Scope:** `ucc-rusted` cross-platform daemon, hardware backends, peripheral transports, IPC, configuration paths, and standalone Qt GUI (`M15.1`–`M15.6`).

## Context

`ucc-rusted` was built primarily for Linux (`tuxedo-drivers` `/dev/tuxedo_io`, `sysfs`, D-Bus, Polkit, BlueZ, `systemd`, Hyprland/KDE Wayland). At the same time, target Uniwill laptops (`INOU0000` / `MECHREVO` / `XMG` / `TUXEDO`) and companion peripherals (LCT21001/LCT22002 water coolers, ITE `048D:600B` keyboards, ITE `048D:7001`/`048D:7000` lightbar & lid logo LEDs, NVIDIA dGPUs) also run on Windows 10/11.

Inspection of comparable projects (such as `open-uniwill-cc`) confirms that **driverless ACPI WMI access on Windows is feasible** without signing a custom KMDF kernel driver: Windows ships `wmiacpi.sys` built in, exposing the firmware ACPI WMI interface under `root\WMI` (`AcpiTest_MULong`) for EC register reads/writes (`Fan Boost`, performance modes, fan curves, battery thresholds, TDP/cTGP/Dynamic Boost, hardware switches, and lightbar power), while proprietary OEM drivers (`UWACPIDriver.sys`) are only needed for extended OEM methods (`PWUP`, `SMRW`).

## Decision

1. **Linux remains first-class and feature-complete:**
   - Every Linux subsystem (`com.uniwill.uccd` / `de.uniwill.uccd` D-Bus, Polkit authorization, `systemd` sleep/service hooks, `/dev/tuxedo_io` and future `M14` `uwcc-rs`, `sysfs`, BlueZ GATT, `ksni` SNI tray, Hyprland/KDE refresh & DPMS) stays untouched and mandatory on `#[cfg(unix)]`.

2. **Platform-specific OS modules are gated, never hacked onto Windows:**
   - Modules meaningless or unavailable on Windows (`tuxedo_io` ioctls, `Polkit`, `systemd` unit management, Linux sysfs ASPM/governor/cpufreq, XDG autostart/session notification snoopers) are scoped under `#[cfg(unix)]` or omitted in Windows builds, rather than introducing stub failures or brittle workarounds.

3. **Unified cross-platform abstractions across workspace crates:**
   - **Paths (`ucc-config::platform_paths`):** `/etc/ucc` / `~/.config/ucc` / `/run/ucc` on Unix vs `%ProgramData%\ucc-rusted` / `%APPDATA%\ucc-rusted` / `%ProgramData%\ucc-rusted\run` on Windows.
   - **EC / Platform (`PlatformBackendKind::WindowsWmi`, `ucc-hardware::windows_wmi::WindowsWmiEcBackend`):** Trait-abstracted ACPI WMI EC register transport (`WmiEcTransport`) over `root\WMI` `AcpiTest_MULong` without requiring a custom signed KMDF driver (`EV`/`WHQL` boundary).
   - **NVML (`ucc-hardware::nvml`):** Dynamic loading resolves `libnvidia-ml.so.1` on Unix and `nvml.dll` on Windows.
   - **Lighting (`ucc-lighting::hid_transport::UniwillHidTransport`):** Shared direct USB HID transport seam (`HidDeviceTransport`) serving both Linux `M14.6` (`hidraw`) and Windows `M15.4` (`HidD_SetFeature` / `hidapi`) for `048D:600B` (calibrated per-key RGB keyboard) and `048D:7001`/`048D:7000` (Lightbar + lid logo LED zone).
   - **IPC (`ucc-dbus::ipc`):** Transport-agnostic length/newline-framed JSON-RPC dispatcher (`dispatch_json_rpc(&dyn UccdBackend, &IpcRequest)`) targeting D-Bus (`com.uniwill.uccd`) on Linux and Windows Named Pipes (`\\.\pipe\ucc-rusted`) on Windows.

## Consequences

- Shared business logic (`ucc-core` profiles/curves/policy, `ucc-config` validation/atomic persistence, fan & cooler controllers, and JSON IPC semantics) stays single-source across Linux and Windows.
- Linux CI and tests continue running every D-Bus, ioctl, sysfs, BlueZ, and Qt GUI test with zero regressions.
