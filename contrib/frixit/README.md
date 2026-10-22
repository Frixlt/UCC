# Frixit Control Center migration

## Current local launch (22 October 2026)

At the user's explicit request, the fork was installed through pacman as
`frixit-control-center 0.2.0.r153.g46cd81f4-1`. The running system daemon is UCC;
TCC and the Python power daemon are stopped and disabled, with TCC temporarily
masked against activation. Original source files remain intact. The old desktop
launcher is overridden with a Hidden entry; the new GUI, tray and notification
agent run as the desktop user and have login integration.

The selected import contains all 57 device policies, 126 physical keyboard zones
and three resolved profiles. Live verification found matching RGB/brightness,
device policy and lightbar settings. The daemon recognized the actual laptop,
applied the source CPU/TDP/cTGP values and completed a real prepare/resume daemon
lifecycle without suspending the machine. An actual hardware sleep cycle remains
unverified. A separate pre-existing NVIDIA Dynamic Boost service start-limit error
was cleared during launch; the service is running, and reapplying the unchanged
device policy reports zero failures across all 57 entries.

The final complete GUI audit timed out. It remains an explicitly recorded known
failure; installation was requested after that result was disclosed. The earlier
successful full runs and the later interrupted/timed-out runs are retained as
separate reports. The deployment manifest records the launch exception rather
than claiming a clean final audit. See [DEPLOYMENT.md](DEPLOYMENT.md) for recovery.

Earlier sections below describe implementation and offline-development checks;
their statements that no files had yet been installed refer to those earlier
stages.

This branch uses the user's committed ControlCenter working files as the source
of behavior and configuration. The old CPU temperature-limit/MSR implementation
is intentionally excluded. The user also explicitly excluded `gpu-run` from the
current migration. Original repositories remain separate and unchanged
apart from their requested baseline commits:

- ControlCenter root: `55a1d5c` (includes SOURCE_SNAPSHOT.json).
- Python daemon: `4cedca2`.
- Python GUI and calibrated keyboard: `53a493b`.
- UCC base: `eb98bfa`, branch `codex/frixit-control-center`.

## Implemented

- Automatic AC / USB-C / Battery detection, including simultaneous online supplies.
- Manual Auto / AC / Type-C / Battery selection in the GUI header and CLI.
- Separate physical source, effective power mode and selected profile state.
  Automatic water-cooler profile switching remains available. Manual mode wins;
  application requests can raise the effective mode and have order-independent
  priority. An informational request does not change the power policy. Requests
  disappear when their owning D-Bus connection closes.
- Type-C profile assignment. Older settings without that assignment use the AC
  assignment as a fallback, including at daemon startup.
- **Power & Devices**: independent AC / Type-C / Battery values for every configured
  PCI/USB runtime-PM control, Wi-Fi power saving, nvidia-powerd, audio power saving,
  NMI watchdog, VM writeback and CPU Turbo Boost. CPU governor, EPP and PCIe ASPM
  are editable per mode. Imported settings override profile CPU settings while
  per-mode policy control is enabled.
- Re-scanning preserves old values and temporarily absent devices; the CPU Turbo
  control is included. Hardware errors are reported instead of being presented
  as successful writes. Power-policy changes apply the chosen device column even
  if a water-cooler profile remains selected.
- Optional NVIDIA D0/persistence workaround takes precedence over the GPU's
  runtime-PM `auto` value. The displayed last application reports that override.
  NVIDIA is awakened before starting Dynamic Boost. No TCC-specific rescue or
  tccd restart is performed by UCC.
- Calibrated physical Mechrevo keyboard, including Enter 56, LShift 21, LCtrl 0,
  SC/PrtSc and the navigation cluster. `keyboard-layout.json` has the 101 physical
  keys, geometry, tooltips and CLI aliases copied from the user's GUI baseline.
  It is embedded as a Qt resource and enabled for the matching Mechrevo model or
  explicit `keyboardLayout: "frixit-mechrevo"` setting. Other models keep their
  existing layouts. Physical `ledIds` are reported by the daemon so missing LEDs
  do not silently shift per-key assignments.
- Shift-click/drag selection, Ctrl selection, brightness preview while dragging
  and a single final hardware update on release. The controller updates brightness
  on every physical LED and propagates failed writes to the caller. Imported
  keyboard state is restored at daemon startup; live edits of the personal layout
  persist the current colors/brightness and selected keyboard profile.

- Native front lightbar control: independent AC / USB-C / Battery enable,
  brightness, custom/preset/majority-keyboard color and all five original
  notification animations. Silent mode suppresses normal notifications; manual
  Test and the optional one-time welcome animation bypass it. Disabled power
  modes remain off. Flash finishes off, other animations restore the current
  color. Changing power mode or settings cancels an old animation immediately.
- `ucc-session-agent` observes notification method headers in the desktop session
  using `dbus-monitor` and forwards only an event to the daemon. It never forwards
  or logs notification contents. It reconnects after monitor/service restarts,
  coalesces storms, keeps one instance per user session and cleans up its child
  process on SIGTERM/SIGINT. A desktop autostart entry is installed by packaging.
  The agent exits without monitoring or commands in read-only builds.

- Standalone `ucc-desktop-tray`, independent of the Plasma applet: left-click opens
  or activates the single GUI instance; the menu shows battery percentage/status,
  charge/discharge watts, GPU runtime state, active profile and manual power modes.
  The colored icon retains the source tray's profile symbol, charging badge and
  dashed/manual-mode indicator. Packaging supplies a desktop autostart entry.
- Battery and NVIDIA process management in both the tray and **Battery &
  Processes** GUI page. NVIDIA display controllers are discovered by vendor/class,
  without fixed PCI addresses. Battery watts use `power_now` or the original
  `current_now × voltage_now` fallback. The collector reads sysfs/procfs on a
  worker thread and never calls NVML or `nvidia-smi`.
- Only the desktop user's visible GPU clients are listed. A confirmation dialog
  defaults to No. Accepted termination sends SIGTERM, waits three seconds, then
  sends SIGKILL to survivors. Numeric PID plus start-time checks and Linux pidfds
  prevent signaling a recycled PID. Termination stays unprivileged; no root D-Bus
  kill endpoint is added. Results report observed exits and failures separately.
- The tray samples battery/runtime state every 30 seconds while closed, scans
  processes on opening, and can refresh its open menu every two seconds. The
  process dialog/page refreshes every two seconds while visible. Hidden or
  minimized GUI pages stop their collector. Sleeping GPUs skip the process scan.
- Source GUI preferences now export to `desktop-preferences.json`: theme
  system/light/dark maps to Auto/Light/Dark and `start_minimized` is preserved.
  Explicit GUI preference import runs before any hardware client and can exit
  immediately. The page also exposes the start-minimized setting. With the tray
  running, minimized startup hides the GUI; otherwise it minimizes to the taskbar.

Device settings are stored in `/etc/ucc/settings` under `powerDevices`. Their
paths, services and value ranges are validated in the daemon. The systemd unit
permits writes to the four explicit module/sysctl files required by the original
behavior. It does not make the entire `/proc/sys` tree writable.

Disabling per-mode policy control stops further device-policy writes and returns
CPU settings to the active profile. It does not promise to restore the previous
value of every device/module that was already changed; use the desired column or
edit the policy before disabling it.

## Offline import with user configuration priority

`migrate-controlcenter.py` reads the user's Data directory and creates a **new**
private directory. It never installs settings, changes a service or contacts
hardware. It produces:

- `settings`: UCC settings with the user's colors, physical layout, complete device
  table, per-mode CPU/ASPM policies and the source lightbar settings.
- `report.json`: imported key/device counts, excluded temperature limit and any
  unresolved TCC profile IDs.
- `desktop-preferences.json`: source theme and minimized-start preference, applied
  separately as the desktop user.
- `pending.json`: original GUI values retained for reference. `gpu-run` is listed
  separately as excluded in the report.

An exported/converted UCC settings file is optional. A local TCC directory and
installed `app.asar` can also supply the original profiles (see below). When a
referenced TCC profile exists in the baseline, the importer creates a separate
profile for each mode, preserving
fan curves, cooling and power-limit data but replacing CPU/key assignments with
user values. A shared source profile is copied rather than overwritten in place.
Missing TCC profiles are reported; guessed fan curves or TDP values are never
created. Device and CPU policies work independently of those missing profiles.

With an updated daemon already available, read-only exports are:

```bash
ucc-cli --json keyboard info > capabilities.json
ucc-cli --json statemap get > baseline-settings.json
python3 contrib/frixit/migrate-controlcenter.py \
  /path/to/ControlCenter/Data ./new-import \
  --capabilities capabilities.json --settings baseline-settings.json
```

Brightness uses hardware units and is preserved from exported keyboard state.
`--brightness N` can select an explicit hardware value. Without exported state,
brightness defaults to the reported hardware maximum. LED 0 is a real saved key;
unknown/missing LED IDs or invalid colors are rejected. The old `rgb-config.json`
is not used in place of the current `keyboard_colors.json` and `lightbar.json`.

This session's offline preview uses the actual read-only detected capabilities:
126 zones, max brightness 50, current brightness 0. The input capabilities and
prepared output are local, ignored artifacts under `artifacts/frixit-import-*`.
No prepared file has been copied into `/etc/ucc`.

### Direct import from the installed TCC

```bash
python3 contrib/frixit/migrate-controlcenter.py \
  /path/to/ControlCenter/Data ./new-import \
  --capabilities capabilities.json \
  --tcc-dir /etc/tcc \
  --tcc-archive /opt/tuxedo-control-center/resources/app.asar
```

The archive is read only. A bounded parser reads the static fan-preset literal;
no JavaScript from the archive is executed or extracted to the filesystem. The
converter distinguishes **named** active presets from inactive `customFanCurve`
values. For named presets, it bakes the source minimum/maximum/offset into full
one-degree tables. Custom curves use TCC's rounding and ignore those modifiers,
as TCC does. Missing presets fail rather than falling back to guessed curves.
UCC's runtime filtering, hysteresis and critical-temperature overrides remain in
use; importing static tables does not claim identical runtime fan dynamics.

CPU core/frequency limits, ODM power limits, cTGP offsets and charging settings
are preserved. The old `nvidiaPowerCTRLProfile.cTGPOffset` field is converted to
`nvidiaCTGPOffset`. Each mode gets its own main profile and the personal keyboard
reference. Explicit `--settings` UCC profiles take precedence over profiles of the
same ID from TCC, preserving existing cooler settings.

The actual source installation supplied all three requested IDs. Local output is
`artifacts/frixit-import-with-tcc-final`. Its report explicitly excludes
`gpu-run`. AC retains 24 cores, TDP values 210/210/420 and
cTGP offset 95; USB-C retains 24 cores and 40/40/72; battery retains 8 cores and
11/20/32. These are the user's source values, not new recommended limits. All
three have 101 CPU and 101 GPU table points. The actual output was validated by
UCC's settings and profile parsers, including profile serialization round-trip;
all CPU/TDP/cTGP/keyboard references and every table point survived. Nothing was
installed or applied to hardware.

## Lightbar session integration

Build option `BUILD_SESSION_AGENT` defaults to ON. The daemon owns LED access;
`ucc-session-agent` runs as the desktop user and requires `dbus-monitor` in PATH.
Session monitoring depends on the session bus allowing monitoring. Run the agent
manually after a first installation, or log in again to use its autostart entry.
The GUI does not have to remain open. No agent or autostart entry was activated
on this machine during development.

Lightbar control defaults to unmanaged for existing UCC installations; the offline
import explicitly enables it using the user's values. Brightness remains in
0–255 UI units and is scaled to the detected hardware maximum. The actual local
lightbar was read only: one RGB zone, maximum 100, current brightness 0. Tests use
fake LED nodes and the GUI screenshot uses demonstration data.

## Standalone tray and desktop preferences

`BUILD_DESKTOP_TRAY=ON` is the default. `BUILD_TRAY` still controls the separate
Plasma applet; a Qt-only build can use `BUILD_TRAY=OFF` with the standalone tray.
The tray, GUI and notification agent are separate processes, so closing the GUI
keeps the tray and notification lightbar available. Each uses its own session
identity; read-only builds use separate preview identities.

After installation, start these as the normal desktop user (or use the packaged
login entries). None of these entries were activated on the development desktop:

```bash
ucc-desktop-tray
ucc-session-agent
ucc-gui --import-preferences ./new-import/desktop-preferences.json --preferences-only
ucc-gui
```

`ucc-gui --minimized` requests minimized startup for one launch. Preference import
and all process/power controls are blocked in preview builds. Process management
requires kernel pidfd support; unsupported operations report an error. It cannot
terminate other users' processes or run as root. No external `gpu-run` command or
mode selector is included.

The current complete offline output is `artifacts/frixit-import-desktop`, including
the source preference `theme: auto, startMinimized: false`. Its daemon settings
are identical to the previously validated TCC import. Files are private (0600)
and have not been installed.

## API and CLI

```bash
ucc-cli lightbar get
ucc-cli lightbar status
ucc-cli lightbar set '<validated JSON object>'
ucc-cli lightbar test
ucc-cli power status
ucc-cli power mode auto
ucc-cli power mode type_c
ucc-cli devices get
ucc-cli devices status
ucc-cli devices scan
ucc-cli devices set '<validated JSON object>'
```

New D-Bus methods on `com.uniwill.uccd`:
`GetPowerPolicyJSON`, `SetManualPowerMode`,
`RegisterPowerRequest(name, reason, mode)`, `UnregisterPowerRequest(name)`,
`GetPowerDeviceSettingsJSON`, `SetPowerDeviceSettingsJSON`,
`GetPowerDeviceStatusJSON`, `ScanPowerDevices`.
Lightbar methods: `GetLightbarSettingsJSON`, `GetLightbarStatusJSON`,
`SetLightbarSettingsJSON`, `TriggerLightbarAnimation(force)`. The session relay
passes false, manual Test passes true.
Signals: `PowerPolicyChanged`, `PowerDeviceStatusChanged`, `LightbarSettingsChanged`.
Control calls use the active-session Polkit control action; previews block writes.
Keep the registering D-Bus connection open for the lifetime of an application
request. `ac` and `type_c` request a minimum performance mode; an empty mode only
registers an observer. Requests cannot reduce physical AC to battery mode; manual
mode can. Requests that need no elevation preserve the automatic cooler state.

## Verification and remaining work

The build and tests use temporary build directories and isolated D-Bus sessions.
Controller tests use temporary sysfs/procfs trees and command runners. GUI tests
exercise the physical keyboard, sparse LED mapping, D-Bus updates, device matrix,
preview write guards and existing upstream screens. Saved screenshots are local
artifacts, not captures of a running controlling daemon.

Verified locally with Qt 6.12: daemon, CLI and GUI builds succeeded; the normal
build passed all 30 CTest entries. A separate read-only build passed the focused
keyboard, sparse LED, device matrix, policy signal, rejected command and preview
write-guard checks. The lightbar update also passed focused read-only GUI, session
agent and all-write-guard checks; normal tests include an end-to-end fake-monitor
relay, mode changes during animation and original-source lightbar import. Five
additional converter tests cover named/custom fan selection, modifiers, rounding,
missing/malformed presets and read-only archive parsing. The actual offline
import validated all 57 device entries
and 101 saved keys. Qt Charts was supplied from an extracted matching package
under `/tmp`. Desktop tests also cover synthetic battery/GPU trees, real test
children exiting through SIGTERM/SIGKILL, stale identities, tray command failures,
confirmation cancellation, minimized polling and preference-only import. The
read-only build passed both desktop test suites and the focused GUI/guard checks.
A staged installation under `/tmp` verified the tray/agent executables and their
autostart entries. No system packages, services or live hardware settings were
changed.

KDE refresh suggestions are now handled by `ucc-session-agent`, independently of
the tray/GUI. The agent watches physical AC/type-C/battery transitions, skips its
initial observation and ignores manual power-mode changes. It proposes 60 Hz on
battery, up to 240 Hz on external power, or up to 300 Hz for an internal panel
routed through NVIDIA. It reads KScreen's actual mode IDs and preserves resolution
(the original fixed mode IDs would select a different resolution on this machine).
Only the notification action applies a change; a fresh query rejects stale
offers, and a final query verifies the result. External displays are not changed.
Battery & Processes → “Offer display refresh changes on KDE” controls the offers.
The feature pauses on login1 sleep signals and stays disabled in preview builds.

The unified sleep hook saves the manual mode and cooler enable state in a private
root-owned `/run/ucc/sleep-state.json`, stops UCC before sleep/NVIDIA suspend and
restarts it after resume only if previously running. The daemon reads that state
before applying its startup profile, preserving the mode and skipping the welcome
animation. State is removed after a successful restart; failed starts retain it
for recovery. Old pre-sleep unit names resolve to the same hook. Generic CMake and
Mechrevo packages now share this implementation.

New tests use fake KScreen executables and a private notification/login1 bus to
cover action-only mode changes, stale offers, command verification failures,
sleep cancellation, disabled offers and preview guards. Temporary snapshots cover
capture/stop/start ordering, already stopped daemons, retries, invalid owners,
permissions and symlinks. A staged `/usr` installation also verified the unit
alias, migration of old enabled links and `systemd-analyze verify`; no host units
were enabled or started. Upgrade tests cover disabled, masked and legacy-only
sleep hooks using `systemctl --root` in temporary directories.

Remaining deployment work: in-session autostart verification and real hardware
suspend/resume verification. No live hardware sleep cycle or system installation
has been performed. `gpu-run` and the original CPU thermal limit are excluded at
the user's request.
