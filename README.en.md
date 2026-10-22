# Frixit Control Center

**Frixit's Python ControlCenter behavior integrated into UCC Revorked's Qt 6 / C++20 suite.**
Personal power/device policies and physical LED calibration take priority.

[Русский](README.md) · [Releases](https://github.com/Frixlt/UCC/releases) ·
[Comparison](docs/COMPARISON.md) · [Build and install](docs/BUILDING.md) ·
[Verification](docs/TESTING.md) · [Origins](PROJECT_ORIGINS.md)

![Frixit Control Center — Dark overview](screenshots/current/dark/overview.png)

## Compared with UCC Revorked

The reference is upstream commit `eb98bfa`, checked on 22 October 2026.
The Qt interface, monitoring, system profiles, fan curves, Bluetooth water-cooler
control, CLI and optional GNOME/Plasma integrations are inherited.
This fork adds:

- Separate AC / USB-C / Battery CPU and device policies: governor, EPP, turbo,
  ASPM, PCI/USB runtime PM, Wi-Fi, system parameters and service control.
- Manual Auto / AC / Type-C / Battery modes, distinct physical/effective power
  states, Type-C profile assignment and connection-scoped application requests.
- The calibrated Mechrevo physical keyboard: Enter LED 56, left Shift 21,
  left Ctrl 0; sparse LED identities and saved user settings are preserved.
- Native Lightbar settings and five notification animations, silent/welcome
  behavior, saved-setting tests and idle-off Flash behavior.
- A standalone Qt tray, battery watts and confirmed termination of the current
  user's NVIDIA clients. No privileged process-kill API is added.
- KDE refresh-rate offers that apply only after the notification action.
- Driver-health checks, a red Overview banner and a login notification.
- Wayland activation-token forwarding for immediate tray focus/workspace changes.
- Offline migration of original ControlCenter/TCC settings and rollback tooling.

The [full comparison](docs/COMPARISON.md) also covers the original Python project,
architecture and verification limits. The Python source is preserved separately
at [Frixlt/ControlCenter-Python](https://github.com/Frixlt/ControlCenter-Python).
The original CPU thermal-limit implementation and `gpu-run` are excluded.
The optional [85 °C fan preset](contrib/frixit/fan-max-performance-85.json) reaches
100% fan duty at 85 °C; it is not a CPU temperature limiter.

## Interface

| Page | Controls |
| --- | --- |
| Overview | CPU/GPU metrics, profile, cooler controls and driver status |
| Profiles | CPU, supported power/charge/display options and profile assignments |
| Cooler Settings | CPU/GPU curves and shared fan-speed behavior |
| Watercool Settings | Bluetooth cooler, fan, pump, LED and automatic curves |
| Monitor | Metric history, charts, zoom and inspection markers |
| Keyboard & Hardware | Per-key RGB, brightness and supported hardware toggles |
| About | Fork source/build links, version, upstream credits, licenses and logs |
| Power & Devices | Independent power-mode CPU/device policy columns |
| Lightbar | Per-mode brightness/color, notifications and saved-setting tests |
| Battery & Processes | Battery power, NVIDIA clients and desktop preferences |

Light/Dark/Auto themes, notification history and single-instance behavior are
retained. Explicit tray clicks open the GUI even if automatic startup is configured
to remain hidden. On KDE Wayland, native tray and KWindowSystem integration forward
the compositor's activation token to the GUI.

**Flash (stays off)** remains dark between notifications, including after Save,
startup and power-mode changes. Test uses the saved settings of the effective
power mode and bypasses silent mode. Notification content is never forwarded.
Manual cooler Fan/Pump values apply immediately and can be persisted in a system
profile. Auto resumes both curves; `sameSpeed` uses the greater CPU/GPU fan duty.
A third fan is not exposed as an independent editable curve by the current backend.

## Current screenshots

Captured from the current application on 22 October 2026 using isolated fake D-Bus
telemetry. No personal configurations or real process names are shown.
See the [complete gallery](README.md#скриншоты-версии-030) and
[reproduction instructions](screenshots/README.md).

| Power policies | Lightbar | Driver warning |
| --- | --- | --- |
| ![Power & Devices](screenshots/current/dark/power-devices.png) | ![Lightbar](screenshots/current/dark/lightbar.png) | ![Driver warning](screenshots/current/dark/driver-warning.png) |

## Build

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_TRAY=OFF -DBUILD_TESTS=ON
cmake --build build -j2
```

Dependencies, installation and preview mode: [BUILDING.md](docs/BUILDING.md).
The root `PKGBUILD` produces `frixit-control-center` from the release source.
The standalone tray and session agent are enabled; the separate Plasma applet
is disabled. Retained DEB/RPM/Nix recipes are not validated binary releases.

The primary verified system is MECHREVO YAOSHI Series-X6AR55xY, Core Ultra 9 275HX /
RTX 5080 Laptop, CachyOS, Qt 6.12, KDE Wayland. Other upstream devices have not
received a complete hardware retest. A compatible TUXEDO kernel driver and matching
headers are required separately. See [Linux 7.3 recovery](contrib/frixit/drivers/README.md).

`uccd` runs as root; desktop clients run as the logged-in user. Polkit authorizes
ordinary control for the active local session. Run one hardware controller.
[Migration](contrib/frixit/README.md), [first deployment/rollback](contrib/frixit/DEPLOYMENT.md)
and [test evidence/limits](docs/TESTING.md) have separate guides. A complete old GUI
audit previously timed out; focused passing checks do not erase that result.

## Credits and license

Frixit's original ControlCenter and the integrated fork retain the contributions
of [noteMASTER11/UCC-Revorked](https://github.com/noteMASTER11/UCC-Revorked),
[nanomatters/ucc](https://github.com/nanomatters/ucc) and TUXEDO IO.
This native suite remains **GPL-3.0-or-later**. See [COPYING](COPYING),
[THIRD_PARTY.md](THIRD_PARTY.md) and [PROJECT_ORIGINS.md](PROJECT_ORIGINS.md).
The separate Python archive has its own Apache-2.0 license and attribution NOTICE;
that license does not relicense the UCC codebase.
