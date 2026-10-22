# Frixit Control Center migration

This branch uses the user's committed ControlCenter working files as the source
of behavior and configuration. The old CPU temperature-limit/MSR implementation
is intentionally excluded. Original repositories remain separate and unchanged
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
  table and per-mode CPU/ASPM policies.
- `report.json`: imported key/device counts, excluded temperature limit and any
  unresolved TCC profile IDs.
- `pending.json`: preserved lightbar, GUI and gpu-run values for remaining work.

An exported/converted UCC settings file is optional. When a referenced TCC profile
exists in it, the importer creates a separate profile for each mode, preserving
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

## API and CLI

```bash
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
Signals: `PowerPolicyChanged`, `PowerDeviceStatusChanged`.
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
build passed all 22 CTest entries. A separate read-only build passed the focused
keyboard, sparse LED, device matrix, policy signal, rejected command and preview
write-guard checks. The actual offline import validated all 57 device entries
and 101 saved keys. Qt Charts was supplied from an extracted matching package
under `/tmp`; no system packages, services or live hardware settings were changed.

Remaining migration work: notification-driven lightbar and animations, external
`gpu-run` compatibility, GPU process controls, battery/tray integration, KDE
refresh-rate notifications, complete TCC cooling/profile import, autostart and
real suspend/resume verification. These are not claimed as implemented by the
initial power/device and keyboard changes.
