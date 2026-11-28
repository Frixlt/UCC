# Windows Port Engineering Notes (`M15.1`–`M15.5`)

Reference architecture: [`ADR-06: Cross-Platform Linux-First & Windows Architecture`](./execution/decisions/ADR-06-windows-port-architecture.md).

## 1. Architecture Overview

`ucc-rusted` keeps Linux (`#[cfg(unix)]`) as a first-class target while isolating OS-specific boundaries behind portable traits so Windows 10/11 builds reuse the single-source domain logic in `ucc-core`, `ucc-config`, fan/cooler controllers, and JSON-RPC dispatch semantics.

- **Linux-only subsystems gated under `#[cfg(unix)]`:** `tuxedo_io` ioctls, Polkit authorization, `systemd` unit/sleep hooks, Linux `sysfs` ASPM/cpufreq/governor writes, BlueZ D-Bus GATT, `ksni` SNI tray, and XDG autostart/session notification snoopers.
- **Cross-platform path resolution (`ucc-config::platform_paths`):**
  - Unix: `/etc/ucc/settings.json`, `$XDG_CONFIG_HOME/ucc` (fallback `$HOME/.config/ucc`, then `/var/lib/ucc/user-config`), `/run/ucc/sleep-state.json`.
  - Windows: `%ProgramData%\ucc-rusted\settings.json`, `%APPDATA%\ucc-rusted` (fallback `%USERPROFILE%\AppData\Roaming\ucc-rusted`, then `%ProgramData%\ucc-rusted`), `%ProgramData%\ucc-rusted\run\sleep-state.json`.

| Milestone | Scope | Primary Crates / Surfaces |
|---|---|---|
| `M15.1` | Named Pipe IPC, ACL/SID security boundary, Windows SCM service wrapper, platform paths | `ucc-dbus`, `ucc-config`, `uccd` |
| `M15.2` | Driverless ACPI WMI EC backend (`root\WMI` `AcpiTest_MULong`) & EV/WHQL boundary | `ucc-hardware` |
| `M15.3` | `nvml.dll` resolution, Windows Power Management GUIDs, display refresh/brightness, lid events | `ucc-hardware`, `uccd` |
| `M15.4` | WinRT BLE GATT cooler transport & Windows USB HID lighting transport | `ucc-hardware`, `ucc-lighting` |
| `M15.5` | Windows tray (`Shell_NotifyIconW`), standalone `Qt6::Widgets` GUI build, CI & contract tests | `ucc-desktop`, `ucc-gui`, `.github/workflows/windows-check.yml` |

## 2. `M15.1` — Named Pipe IPC & Windows Service Wrapper

### Transport & Framing (`ucc-dbus::ipc`)

- **Endpoint:** `\\.\pipe\ucc-rusted` (`WINDOWS_NAMED_PIPE_PATH`), mirroring `com.uniwill.uccd` on Linux D-Bus (`IpcTransportKind::NamedPipe` vs `IpcTransportKind::Dbus`).
- **Wire format:** Newline-delimited UTF-8 JSON-RPC frames (`IpcRequest { id, method, params }` / `IpcResponse { id, ok, result, error }`) encoded/decoded via `encode_frame` / `decode_frame` and routed through `dispatch_json_rpc(&dyn UccdBackend, &IpcRequest)`.
- **Payload cap:** Enforces the 256 KiB frame limit (`MAX_IPC_FRAME_BYTES`, `262_144` bytes, mirroring `MAX_DBUS_JSON_PAYLOAD_BYTES`) before JSON parsing to match Linux D-Bus DoS bounds.

### Security Boundary (SDDL ACL & Remote Rejection)

- **Pipe creation flags:** Must set `PIPE_REJECT_REMOTE_CLIENTS` so SMB/network clients cannot connect to `\\.\pipe\ucc-rusted`.
- **SDDL ACL:** Grants access exclusively to:
  - `SY` (`LocalSystem`) — full control for the `uccd` service process;
  - `BA` (`Built-in Administrators`) — administrative control;
  - `IU` (`Interactive Users`) — local desktop CLI/GUI/tray sessions.
- Non-interactive network/anonymous/guest SIDs (`AN`, `WD`, `NU`) are denied. Privileged hardware operations mirror the Linux Polkit `ManageHardware` boundary using client token/elevation checks.

### `uccd` Windows Service Control Manager (SCM) Wrapper

- `uccd` runs as a Windows service via `uccd::windows_service` (direct `advapi32` SCM FFI) on `#[cfg(windows)]` in place of `systemd` (`sd_notify` / `Type=notify`).
- Service lifecycle transitions (`StartPending` → `Running` → `StopPending` → `Stopped`, plus `Shutdown` / `PowerEvent` suspend-resume hooks) coordinate clean backend shutdown and return EC fans to firmware automatic control (`set_fans_auto()`) on service stop or fault recovery.

## 3. `M15.2` — Driverless ACPI WMI EC Backend & EV/WHQL Boundary

### `WindowsWmiEcBackend` (`ucc-hardware::windows_wmi`)

`WindowsWmiEcBackend` implements `PlatformBackend` (`PlatformBackendKind::WindowsWmi`) over the abstract `WmiEcTransport` trait targeting Windows' built-in `wmiacpi.sys` WMI class `AcpiTest_MULong` under namespace `root\WMI` (ACPI device `INOU0000`).

Mapped Uniwill EC registers (`ec_reg`):

| Register | Purpose | Notes |
|---|---|---|
| `0x0751` | `FAN_MANUAL_MODE` | `1` enables manual fan duty; `0` restores firmware auto |
| `0x1804` / `0x1809` | `CPU_FAN_DUTY` / `GPU_FAN_DUTY` | Scaled `0..=200` for `0..=100%` |
| `0x043E` / `0x044F` | `CPU_TEMP_C` / `GPU_TEMP_C` | `0` treated as `SensorUnavailable` |
| `0x0752` | `PERF_MODE` | Platform performance profile index |
| `0x0740` / `0x0741` / `0x0742` | `PL1_WATTS` / `PL2_WATTS` / `PL4_WATTS` | Bounded against `WmiPlatformLimits` before write |

### Capability Matrix: Driverless (`wmiacpi.sys`) vs EV/WHQL KMDF (`UWACPIDriver.sys`)

| Capability | Driverless (`root\WMI` `AcpiTest_MULong` / `wmiacpi.sys`) | Signed KMDF (`UWACPIDriver.sys`, EV/WHQL) |
|---|---|---|
| Performance modes (`0x0752`) | Supported | Supported |
| Manual fan duty & Fan Boost (`0x0751`, `0x1804`, `0x1809`, `0x0753`) | Supported | Supported |
| EC CPU/GPU temperatures (`0x043E`, `0x044F`) | Supported | Supported |
| TDP limits `PL1`/`PL2`/`PL4` (`0x0740`–`0x0742`) | Supported (within EC register bounds) | Supported |
| Battery charge thresholds, Fn/Win Lock, Lightbar EC power (`0x0780`, `ec_reg::LIGHTBAR_POWER`) | Supported where firmware exposes EC R/W via `AcpiTest_MULong` | Supported |
| Extended OEM ACPI `SMRW` (firmware limit table / TCC / OEM reads) | Not exposed via `AcpiTest_MULong` | Required (`SMRW` ioctl/ACPI method) |
| Extended OEM ACPI `PWUP` (water-cooler / OEM power-up control) | Not exposed via `AcpiTest_MULong` | Required (`PWUP` ioctl/ACPI method) |
| Direct raw EC I/O port access (`0x62`/`0x66`) outside WMI | Unavailable (blocked by Windows kernel) | Required (EV code-signing + WHQL attestation) |

Driverless WMI is the default Windows path; any feature requiring `SMRW`, `PWUP`, or raw port I/O explicitly degrades to unavailable unless an EV/WHQL-signed KMDF driver is present.

## 4. `M15.3` — Telemetry, Power Management, Display, and Lid Events

- **NVIDIA Telemetry (`ucc-hardware::nvml`):** Dynamic `libloading` resolution loads `nvml.dll` on Windows (mirroring `libnvidia-ml.so.1` on Unix) with no compile-time CUDA/NVML SDK dependency, using a trusted-first search order (`<SystemRoot>\System32\nvml.dll`, `<ProgramFiles>\NVIDIA Corporation\NVSMI\nvml.dll`, then `nvml.dll` fallback). Telemetry polling respects GPU idle/sleep states to avoid unnecessary dGPU wakeups.
- **CPU Power Management (Windows Power Management API / `powrprof.dll`):** Maps profile CPU tuning onto `GUID_PROCESSOR_SETTINGS_SUBGROUP` (`54533251-82be-4824-96c1-47b60b740d00`) settings:
  - Energy Performance Preference (EPP): `GUID_PROCESSOR_PERF_ENERGY_PREFERENCE` (`36687f9e-e3a5-4dbf-b1dc-15eb381c6863`, `0..=100`);
  - Minimum / Maximum processor state (throttle): `GUID_PROCESSOR_THROTTLE_MINIMUM` (`893dee8e-2bef-41e0-89c6-b55d0929964c`) and `GUID_PROCESSOR_THROTTLE_MAXIMUM` (`bc5038f7-23e0-4960-96da-33abaf5935ec`);
  - Maximum frequency cap (MHz): `GUID_PROCESSOR_FREQUENCY_MAXIMUM` (`75b0ae3f-bce0-45a7-8c89-c9611c25e100`);
  - Turbo / Boost mode: `GUID_PROCESSOR_PERF_BOOST_MODE` (`be337238-0d82-4146-a960-4f3749d470c7`).
- **Display Refresh Rate & Brightness:**
  - Refresh rate enumeration and mode switches use `EnumDisplaySettingsExW` / `ChangeDisplaySettingsExW` (`DEVMODEW::dmDisplayFrequency`) with test-first (`CDS_TEST`) validation before applying.
  - Internal panel brightness control uses WMI `root\WMI` `WmiMonitorBrightness` (read) and `WmiMonitorBrightnessMethods::WmiSetBrightness` (write).
- **Lid Switch Notifications:** Service and session power callbacks register via `RegisterPowerSettingNotification` for `GUID_LIDSWITCH_STATE_CHANGE` (`ba3e0f4d-b817-4094-a2d1-d56379e6a0f3`), translating `0` (closed) / `1` (open) into the same lid-policy state machine used on Linux.

## 5. `M15.4` — Peripheral Transports (WinRT BLE & Windows HID)

### Water Cooler BLE GATT (WinRT)

- Replaces Linux BlueZ (`ucc-hardware::bluez_cooler`) with WinRT `Windows.Devices.Bluetooth.GenericAttributeProfile` over the shared LCT protocol state machine (`ucc-hardware::cooler`).
- **Nordic UART Service (NUS) UUIDs:**
  - Service: `6e400001-b5a3-f393-e0a9-e50e24dcca9e`
  - TX (write): `6e400002-b5a3-f393-e0a9-e50e24dcca9e`
  - RX (notify): `6e400003-b5a3-f393-e0a9-e50e24dcca9e`
  - CCCD: `00002902-0000-1000-8000-00805f9b34fb`
- **Protocol invariants:** 8-byte framed packets (`0xFE ... 0xEF`), `80 ms` minimum inter-write gap (`BLE_WRITE_GAP`), `10 s` keepalive (`KEEPALIVE_INTERVAL`).
- **Pump code `1` (`V12`) prohibition:** Pump voltage code `1` is rejected across all layers (API, protocol encoder, and WinRT transport). Valid manual pump codes are `0` (`11V`), `2` (`7V`), `3` (`8V`), and `4` (`Off`).

### Direct USB HID Lighting (`ucc-lighting::hid_transport`, `ucc-lighting::windows_hid`)

- `UniwillHidTransport` sits atop the portable `HidDeviceTransport` trait, backed by `/dev/hidraw*` on Linux and `ucc-lighting::windows_hid` (`WindowsHidTransport` / `WinHidSysApi` / `LiveWinHidSysApi`) using `HidD_SetFeature` / `HidD_GetFeature` over direct `hid.dll` / `setupapi.dll` / `kernel32.dll` FFI on Windows.
- **Supported ITE controllers (`VID 0x048D`):**
  - `048D:600B` — Per-key RGB keyboard (126 zones, calibrated layout mapping, hardware brightness scaled `0..=50` via report `0x08 0x02 0x01 0x05 <level>`, zone updates chunked into `<= 64`-byte feature reports with up to 16 RGB zones per `0x14` frame).
  - `048D:7001` (4-zone front Lightbar) and `048D:7000` (5-zone Lightbar + lid logo LED) — brightness via `0x08 0x01 <level>` and per-zone color via `0x14 <zone> <r> <g> <b>`.
- **Interface disambiguation:** Composite ITE HID devices expose multiple top-level collections (keyboard input vs vendor lighting). `UniwillHidTransport` selects the vendor lighting collection by requiring HID `usage_page >= 0xFF00` (falling back to USB `bInterfaceNumber == 1` when usage page metadata is unavailable).

## 6. `M15.5` — Windows Tray, Standalone Qt6 GUI, and Verification Commands

### Tray & Standalone `Qt6::Widgets` GUI

- **System tray (`ucc-desktop`):** On Windows, native tray integration uses `Shell_NotifyIconW` (or `tray-icon` wrapping `Shell_NotifyIconW`) with left-click window activation and right-click context menu for profile switching, power mode, and quit, replacing Linux `ksni` (StatusNotifierItem).
- **Standalone `Qt6::Widgets` GUI (`ucc-gui`):** Per `ADR-01` and `ADR-03`, `ucc-gui` has zero `KF6`, `Plasma`, `QML`, or `Qt6Bluetooth` dependencies. Windows builds target `Qt6::{Core,Gui,Widgets,Charts}` with Named Pipe JSON-RPC transport replacing Linux `Qt6::DBus`:
  - **MSVC (recommended for native Qt6 binaries):**
    ```powershell
    cmake -S . -B build-win-msvc -G Ninja `
      -DCMAKE_BUILD_TYPE=RelWithDebInfo `
      -DCMAKE_PREFIX_PATH="C:\Qt\6.8\msvc2022_64" `
      -DBUILD_GUI=ON -DBUILD_TRAY=OFF -DBUILD_GNOME=OFF -DBUILD_TESTS=OFF
    cmake --build build-win-msvc --parallel
    ```
  - **MinGW-w64 (MSYS2 / Qt6 MinGW kit):**
    ```bash
    cmake -S . -B build-win-mingw -G Ninja \
      -DCMAKE_BUILD_TYPE=RelWithDebInfo \
      -DCMAKE_PREFIX_PATH="/c/Qt/6.8/mingw_64" \
      -DBUILD_GUI=ON -DBUILD_TRAY=OFF -DBUILD_GNOME=OFF -DBUILD_TESTS=OFF
    cmake --build build-win-mingw --parallel
    ```

### Exact CI & Local Verification Commands

Workflow definition: [`.github/workflows/windows-check.yml`](../../.github/workflows/windows-check.yml).

1. **Linux contract tests for Windows protocol seams:**
   ```bash
   cargo test -p ucc-dbus --test named_pipe_ipc --test ipc_framing
   cargo test -p ucc-hardware --test windows_wmi_backend --test windows_power_display_lid --test winrt_ble_cooler
   cargo test -p ucc-lighting --test windows_hid_transport --test hid_transport
   cargo test -p ucc-config --test windows_platform_paths
   cargo test -p uccd --test windows_service_lifecycle
   ```

2. **Linux-hosted Windows GNU cross-compilation check:**
   ```bash
   rustup target add x86_64-pc-windows-gnu
   cargo check -p ucc-core -p ucc-config -p ucc-dbus --target x86_64-pc-windows-gnu
   ```

3. **Native Windows MSVC check:**
   ```powershell
   cargo check -p ucc-core -p ucc-config -p ucc-dbus --target x86_64-pc-windows-msvc
   ```

Note: `ucc-hardware` and `ucc-lighting` Windows target checks await Unix-gating of the `nix` dependency in `crates/ucc-hardware/Cargo.toml` (owned by the parallel Linux M14 track).
