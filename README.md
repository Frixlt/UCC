# ucc-rusted

[English](README.md) | [Русский](README.ru.md)

[![CI Build](https://img.shields.io/github/actions/workflow/status/Frixlt/UCC/build.yml?label=CI%20Build)](https://github.com/Frixlt/UCC/actions/workflows/build.yml)
[![Nix Check](https://img.shields.io/github/actions/workflow/status/Frixlt/UCC/nix.yml?label=Nix%20Check)](https://github.com/Frixlt/UCC/actions/workflows/nix.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](COPYING)
[![Rust MSRV 1.89+](https://img.shields.io/badge/Rust-1.89%2B-orange.svg)](Cargo.toml)
[![GUI: C++20 / Qt6](https://img.shields.io/badge/GUI-C%2B%2B20%20%2F%20Qt6-41CD52.svg)](CMakeLists.txt)
[![Platform: Linux x86_64](https://img.shields.io/badge/platform-Linux%20x86__64-lightgrey.svg)](PKGBUILD)
[![D-Bus API: 123 methods | 7 signals](https://img.shields.io/badge/D--Bus%20API-123%20methods%20%7C%207%20signals-blueviolet.svg)](docs/DBUS_API.md)

**ucc-rusted** is the Rust system control suite for Uniwill, Clevo, Mechrevo, and TUXEDO laptops on Linux (`x86_64`), migrated from Uniwill Control Center (`UCC`). The background daemon, hardware/sysfs/ioctl/BlueZ/NVML backends, lighting engine, CLI, system tray, session agent, and systemd sleep/recovery helpers run as compiled Rust binaries (MSRV `1.89`, edition `2021`). The configuration window (`ucc-gui`) is a standalone on-demand `C++20 / Qt6` D-Bus client with zero `KF6` or `QML` dependencies per `ADR-01`.

<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/overview.png -->
> **Hero screenshot slot (`Overview`, dark theme):** `screenshots/current/dark/overview.png`

---

## Project Scope & Process Architecture

All privileged hardware operations execute inside a single root daemon (`uccd`) serialized through an internal command path. Desktop clients run unprivileged in the user session and communicate exclusively over the system D-Bus (`com.uniwill.uccd` at `/com/uniwill/uccd`, with a transparent legacy alias `de.uniwill.uccd` at `/de/uniwill/uccd` exposing 123 methods and 7 signals).

### Runtime Binaries (7 Rust + 1 Standalone Qt6 GUI)

| Binary | Installed Path | Language / Stack | Privilege | Responsibility |
| --- | --- | --- | --- | --- |
| `uccd` | `/usr/bin/uccd` | Rust (`crates/uccd`) | `root` (systemd) | System daemon: hardware IO, fan curves, power/device policies, lighting, Bluetooth LCT cooler, telemetry, D-Bus server |
| `ucc-cli` | `/usr/bin/ucc-cli` | Rust (`crates/ucc-cli`) | User | Scriptable CLI (`clap` 4.5): status, live monitor, profiles, state map, fans, keyboard, water cooler, charging |
| `ucc-desktop-tray` | `/usr/bin/ucc-desktop-tray` | Rust (`crates/ucc-desktop`) | User | Standalone StatusNotifierItem (SNI) tray: power modes, battery watts, user NVIDIA process list (`pidfd` termination), Wayland activation-token forwarding |
| `ucc-session-agent` | `/usr/bin/ucc-session-agent` | Rust (`crates/ucc-session-agent`) | User | Session D-Bus observer: Lightbar notification triggers, KDE (`kscreen`) / Hyprland (`hyprctl`) refresh-rate offers, driver-health login alert |
| `ucc-hardware` | `/usr/lib/ucc/ucc-hardware` | Rust (`crates/uccd`) | `root` (helper) | Direct hardware helper invoked by `uccd.service` `ExecStopPost` (`--fan-auto`) to restore firmware fan control on exit or crash |
| `ucc-sleep` | `/usr/lib/ucc/ucc-sleep` | Rust (`crates/uccd`) | `root` (helper) | Suspend/resume hook invoked by `uccd-pre-sleep.service` and `uccd-sleep.service` |
| `ucc-import-check` | `/usr/lib/ucc/ucc-import-check` | Rust (`crates/uccd`) | User / `root` | Offline configuration importer and schema validator |
| `ucc-gui` | `/usr/bin/ucc-gui` | C++20 / `Qt6::Widgets` + `Qt6::Charts` | User (on-demand) | Standalone settings window (`ADR-01`): zero `KF6`, `QML`, `Qt6::Bluetooth`, or `libucc-dbus.so`; 0 MB background footprint when closed |

The deprecated Plasma C++/QML applet (`ucc-tray`) is excluded from packaged builds (`-DBUILD_TRAY=OFF`, `ADR-03`); `ucc-desktop-tray` replaces it across KDE Plasma, Hyprland, Sway/Waybar, and GNOME (with an SNI/AppIndicator host). `ucc-gnome` remains an optional external GJS D-Bus adapter.

### Cargo Workspace Layout (10 Crates)

| Crate | Type | Role |
| --- | --- | --- |
| `ucc-core` | Library | Domain types, profiles, fan curves, telemetry structs, error definitions |
| `ucc-config` | Library | `/etc/ucc/settings` (with `/etc/ucc/settings.json` fallback) and `~/.config/ucc/settings.json` persistence, atomic saves with `.bak` recovery, offline importer |
| `ucc-hardware` | Library | DMI/platform detection, `tuxedo-drivers` ioctl, sysfs/procfs CPU/PCI/USB/ASPM/charging control, NVML (`libloading`), BlueZ GATT cooler |
| `ucc-lighting` | Library | Per-key RGB keyboard state, Mechrevo physical LED mapping, front Lightbar state machine and 5 notification animations |
| `ucc-dbus` | Library | `zbus` 5.5 D-Bus interface (`com.uniwill.uccd` + `de.uniwill.uccd`), typed client proxies, Polkit authorizer |
| `uccd` | Binaries | `uccd`, `ucc-hardware`, `ucc-sleep`, `ucc-import-check` |
| `ucc-cli` | Lib + Bin | `ucc-cli`, shell completion and man-page generation |
| `ucc-desktop` | Lib + Bin | Desktop utilities + `ucc-desktop-tray` |
| `ucc-session-agent` | Lib + Bin | Session integration + `ucc-session-agent` |
| `xtask` | Tooling | Workspace build/staging/release automation (`install-runtime`, `check-versions`, `dist`, `dist-cli`, `capture-gallery`) |

---

## GUI Sections & Screenshot Placeholders

`ucc-gui` exposes 10 functional tabs plus an inline driver-health warning banner on `Overview`. Screenshot slots below map to the exact paths tracked in [`screenshots/current/manifest.json`](screenshots/current/manifest.json) (viewport `1440x1000`, offscreen isolated D-Bus fixture, no live hardware writes).

<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/overview.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/overview.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/profiles.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/profiles.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/cooler-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/cooler-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/watercool-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/watercool-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/keyboard-hardware.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/keyboard-hardware.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/lightbar.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/lightbar.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/power-devices.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/power-devices.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/battery-processes.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/battery-processes.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/monitor.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/monitor.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/about.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/about.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/driver-warning.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/driver-warning.png -->

| GUI Tab / State | Scope | Dark Theme Slot | Light Theme Slot |
| --- | --- | --- | --- |
| Overview | CPU/GPU telemetry, active profile, water-cooler quick controls, driver status | `screenshots/current/dark/overview.png` | `screenshots/current/light/overview.png` |
| Profiles | CPU cores/frequencies, power limits, charging, display refresh, keyboard, state assignments | `screenshots/current/dark/profiles.png` | `screenshots/current/light/profiles.png` |
| Cooler Settings | CPU/GPU fan curves, copy/save presets, `sameSpeed` shared fan duty | `screenshots/current/dark/cooler-settings.png` | `screenshots/current/light/cooler-settings.png` |
| Watercool Settings | Bluetooth LCT connection, fan/pump manual overrides, LED, temperature curves | `screenshots/current/dark/watercool-settings.png` | `screenshots/current/light/watercool-settings.png` |
| Keyboard & Hardware | Per-key RGB editor, brightness, hardware toggles (webcam, Fn Lock, display) | `screenshots/current/dark/keyboard-hardware.png` | `screenshots/current/light/keyboard-hardware.png` |
| Lightbar | Per-power-mode color/brightness, 5 notification animations, Silent, Welcome, Test | `screenshots/current/dark/lightbar.png` | `screenshots/current/light/lightbar.png` |
| Power & Devices | Independent AC / Type-C / Battery CPU & device policies (governor, EPP, turbo, ASPM, PCI/USB PM, Wi-Fi) | `screenshots/current/dark/power-devices.png` | `screenshots/current/light/power-devices.png` |
| Battery & Processes | Battery discharge watts, current user's NVIDIA processes, startup & KDE refresh preferences | `screenshots/current/dark/battery-processes.png` | `screenshots/current/light/battery-processes.png` |
| Monitor | Time-series history (temperature, power, clocks, fans, voltage) with zoom and markers | `screenshots/current/dark/monitor.png` | `screenshots/current/light/monitor.png` |
| About | Build version/commit, Qt version, repository links, credits, licenses, diagnostic logs | `screenshots/current/dark/about.png` | `screenshots/current/light/about.png` |
| Driver Warning | Red `Overview` banner displayed when `tuxedo-drivers` / hardware interface checks fail | `screenshots/current/dark/driver-warning.png` | `screenshots/current/light/driver-warning.png` |

To regenerate all screenshots against the isolated offscreen D-Bus fixture:

```bash
python3 contrib/frixit/capture-gallery.py --build build --output screenshots/current
```

---

## Supported Hardware Summary

- **Platform families**: Uniwill, Clevo, Mechrevo, and TUXEDO laptops exposed through compatible `tuxedo-drivers` kernel interfaces. Feature availability (TDP/cTGP limits, per-key RGB, Lightbar, charging profiles, webcam/Fn-Lock toggles, Bluetooth liquid cooler) is probed dynamically from driver and firmware capabilities.
- **Primary verified target**: **MECHREVO YAOSHI Series-X6AR55xY** (Intel Core Ultra 9 275HX, NVIDIA RTX 5080 Laptop, CachyOS, Qt 6.12, KDE Wayland). Upstream Uniwill/Clevo/TUXEDO device tables are preserved, though full hardware re-testing on other models has not been executed.
- **Mechrevo physical keyboard calibration**: Sparse physical LED indices are preserved: Enter = `56`, Left Shift = `21`, Left Ctrl = `0`.
- **Bluetooth water cooler (LCT)**: Supported for Uniwill liquid-cooled chassis (`STELLARIS1XI04`, `STEPOL1XA04`, `STELLARIS1XI05`, `STELLARIS16I06`, `STELLARIS17I06`, `STELLARIS16A07`, `STELLARIS16I07`, `PCS_X6FR559Y`, `XNE16A25`, `XNE16E25`) via BlueZ GATT.
- **Kernel driver prerequisite**: `ucc-rusted` does not bundle kernel drivers. Install compatible `tuxedo-drivers` and matching kernel headers separately. See [`contrib/frixit/drivers/README.md`](contrib/frixit/drivers/README.md) for Linux 7.3 recovery notes and [`docs/HARDWARE_MATRIX.md`](docs/HARDWARE_MATRIX.md) for the full device matrix.
- **Explicitly excluded features**: CPU thermal-limit overrides and `gpu-run` are omitted by design. The optional 85 °C fan preset ([`contrib/frixit/fan-max-performance-85.json`](contrib/frixit/fan-max-performance-85.json)) ramps fans to 100% duty at 85 °C; it does not cap CPU temperature.

---

## Installation

Repository and source releases: `https://github.com/Frixlt/UCC` (branch `main`).

### Arch Linux / CachyOS (`PKGBUILD`)

The root [`PKGBUILD`](PKGBUILD) builds package `ucc-rusted` (provides `uniwill-control-center`, replaces `frixit-control-center`, conflicts with `uniwill-control-center` and `uniwill-control-center-mechrevo`):

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC
UCC_SOURCE_DIR="$PWD" makepkg -si
```

### Debian / Ubuntu (DEB)

Packaging metadata resides in `debian/` (requires `rustc >= 1.89`, `cargo`, `cmake >= 3.20`, `qt6-base-dev`, `qt6-charts-dev`, `nlohmann-json3-dev`, `libsystemd-dev`, `libxrandr-dev`, `libudev-dev`):

```bash
make deb
```

### Fedora / RHEL (RPM)

Packaging metadata resides in [`ucc.spec`](ucc.spec):

```bash
make rpm
```

### Nix Flake

[`flake.nix`](flake.nix) exports `packages.ucc`, `overlays.default`, and `nixosModules.uccd`:

```bash
nix build .#ucc
nix flake check
```

> **Note**: DEB, RPM, and Nix recipes are maintained in-tree but are not published as prebuilt binary releases.

### Building from Source

Prerequisites: Rust toolchain `>= 1.89`, CMake `>= 3.20`, Ninja, C++20 compiler, `qt6-base`, `qt6-charts`, `nlohmann-json`, `systemd` / `dbus` / `polkit` headers.

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC

# 1. Build all 7 Rust runtime binaries + xtask
cargo build --workspace --bins --release --locked --target-dir target

# 2. Build standalone Qt6 GUI (KF6-free, Plasma applet disabled)
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_GUI=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_CLI=OFF \
  -DUCC_PACKAGE_RUST_RUNTIME=ON
cmake --build build --parallel 2

# 3. Install GUI assets + Rust runtime binaries, completions, and man page
sudo cmake --install build
sudo ./target/release/xtask install-runtime /usr ./target/release
```

---

## Quick Start

### Systemd Services

Enable and start the system daemon and suspend/resume hooks:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now uccd.service
sudo systemctl enable uccd-pre-sleep.service uccd-sleep.service
systemctl status uccd.service
```

### CLI (`ucc-cli`)

```bash
ucc-cli status                  # System dashboard (CPU, GPU, fans, cooler, charging)
ucc-cli monitor                 # Live telemetry stream (alias: mon)
ucc-cli profile list            # List available profiles (alias: prof)
ucc-cli statemap                # View AC / Battery / Type-C / Water Cooler mapping
ucc-cli fan                     # Fan profile and curve status
ucc-cli keyboard                # Keyboard backlight state (alias: kb)
ucc-cli cooler                  # Bluetooth water cooler status (aliases: watercooler, wc)
ucc-cli charging                # Battery charging thresholds (alias: charge)
```

### GUI, Tray & Session Agent

```bash
ucc-gui                         # Launch on-demand Qt6 configuration window
ucc-desktop-tray                # Start standalone SNI tray (autostarted via XDG)
ucc-session-agent               # Start session agent (autostarted via XDG)
```

### Read-Only Preview Mode (No Hardware Writes)

Inspect `ucc-gui` against an isolated fixture without touching `/etc/ucc` or live hardware:

```bash
./run-preview.sh
```

---

## Configuration & Safety Invariants

### Configuration Paths & Persistence

- **System configuration**: `/etc/ucc/settings` (primary path, with `/etc/ucc/settings.json` fallback; owned by `root`, created/imported locally; private settings and Bluetooth addresses are never shipped in packages).
- **User preferences**: `~/.config/ucc/settings.json`.
- **Atomic writes**: Configuration updates write to a temporary file, sync, and atomically rename into place while maintaining a `.bak` backup for automatic corruption recovery.

### Privilege Boundary & Systemd Hardening

- **Privilege separation**: Only `uccd` (and root helper hooks `ucc-hardware`, `ucc-sleep`) executes privileged hardware operations. `ucc-gui`, `ucc-cli`, `ucc-desktop-tray`, and `ucc-session-agent` run unprivileged.
- **Polkit authorization**: D-Bus access is gated by `com.uniwill.uccd.read`, `com.uniwill.uccd.control`, and `com.uniwill.uccd.manage-hardware` (with legacy `de.uniwill.uccd.*` fallback). Active local sessions are authorized for standard control.
- **Process termination boundary**: `ucc-desktop-tray` / `ucc-gui` terminate only the current unprivileged user's NVIDIA client processes via `pidfd` after confirmation. No privileged process-kill D-Bus API exists in `uccd`.
- **Systemd sandboxing**: `uccd.service` enforces `ProtectSystem=strict`, `ProtectHome=yes`, `NoNewPrivileges=true`, `ProtectKernelModules=yes`, `ProtectControlGroups=yes`, `RestrictNamespaces=yes`, `LockPersonality=yes`, `MemoryDenyWriteExecute=yes`, and `CapabilityBoundingSet=CAP_SYS_RAWIO CAP_NET_ADMIN CAP_SYS_ADMIN`.

### Hardware & Behavioral Invariants

- **Single hardware controller**: `uccd.service` declares `Conflicts=tccd.service tuxedo-power.service`. Only one hardware controller may run at a time.
- **Crash-safe fan recovery**: `uccd.service` runs `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto`, returning fan duty control to firmware even after an abnormal daemon exit.
- **Fan curve semantics**: With `sameSpeed` enabled, both internal fans follow `max(cpu_fan_duty, gpu_fan_duty)`. A third fan is not exposed as a separate editable curve by the current backend. Manual water-cooler Fan/Pump values apply immediately and persist when saved into a system profile; `Auto` restores temperature curves.
- **Power policy precedence**: Manual power mode (`AC` / `Type-C` / `Battery`) overrides automatic physical power-source detection. Connection-scoped D-Bus application requests can raise the effective mode and are automatically cleared when the requesting D-Bus client disconnects. When device/CPU policy management is enabled, its CPU settings take precedence over the active profile.
- **Lightbar privacy & state**: `Flash (stays off)` keeps the Lightbar dark between notifications across Save, power transitions, and daemon startup. `Silent` suppresses regular animations while `Test` bypasses `Silent` using the saved settings of the effective power mode. Notification text/content is never forwarded to `uccd`.

---

## Documentation Index

| Document | Contents |
| --- | --- |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | System architecture, crate boundaries, actor/serialization model, `ADR-01` / `ADR-03` |
| [`docs/HARDWARE_MATRIX.md`](docs/HARDWARE_MATRIX.md) | Supported Uniwill/Clevo/Mechrevo/TUXEDO chassis, capability probes, LED calibration |
| [`docs/DBUS_API.md`](docs/DBUS_API.md) | Complete reference for `com.uniwill.uccd` / `de.uniwill.uccd` (123 methods, 7 signals, Polkit actions) |
| [`docs/CLI_REFERENCE.md`](docs/CLI_REFERENCE.md) | `ucc-cli` subcommands, flags, JSON output schemas, exit codes |
| [`docs/BUILDING.md`](docs/BUILDING.md) | Build dependencies, CMake/Cargo flags, packaging (Arch/DEB/RPM/Nix), preview mode |
| [`docs/TESTING.md`](docs/TESTING.md) | Workspace tests, D-Bus interop suite, GUI audit fixtures, verification scope and limits |
| [`docs/MIGRATION_AND_RECOVERY.md`](docs/MIGRATION_AND_RECOVERY.md) | Offline config migration, backup/rollback procedures, kernel driver recovery |
| [`docs/COMPARISON.md`](docs/COMPARISON.md) | Feature and architectural comparison against upstream UCC Revorked (`eb98bfa`) |
| [`docs/AI_AGENT_GUIDE.md`](docs/AI_AGENT_GUIDE.md) | Codebase navigation, test harnesses, and modification constraints for automated agents |
| [`AGENTS.md`](AGENTS.md) | Repository commit discipline, pre-commit Cargo checks, fixture rules, privilege rules |

---

## Origins & License

`ucc-rusted` preserves upstream copyright headers, Git history, and open-source notices from:

- [`noteMASTER11/UCC-Revorked`](https://github.com/noteMASTER11/UCC-Revorked) (upstream baseline commit `eb98bfa`)
- [`nanomatters/ucc`](https://github.com/nanomatters/ucc) (original Uniwill Control Center foundation)
- [`tuxedocomputers/tuxedo-control-center`](https://github.com/tuxedocomputers/tuxedo-control-center) (TUXEDO IO layer; external kernel drivers retain their own licenses)

Detailed provenance and third-party attributions are documented in [`PROJECT_ORIGINS.md`](PROJECT_ORIGINS.md) and [`THIRD_PARTY.md`](THIRD_PARTY.md).

Licensed under **GPL-3.0-or-later**. Full license text: [`COPYING`](COPYING).
