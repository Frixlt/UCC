# Frixit Control Center: project origins

This project combines two codebases and their behavior:

1. **Frixit's original ControlCenter** — the personal power/device policy model,
   AC / Type-C / Battery behavior, calibrated physical keyboard, front lightbar,
   notification animations, desktop tray, battery/GPU process tools and KDE
   refresh suggestions. Personal settings and LED calibration take priority.
2. **UCC Revorked by noteMASTER11** — the Qt/C++ interface, system daemon and
   D-Bus architecture, profile editor, monitoring, fan and water-cooler controls,
   Mechrevo hardware integration and Fluent-style presentation.

The original Python behavior has been ported into the native suite. The combined
project uses `uccd`, `ucc-gui`, `ucc-desktop-tray`, `ucc-session-agent` and `ucc-cli`.
Executable names, D-Bus identifiers and preference paths remain compatible with
UCC so existing settings and integrations keep working.

## Source snapshots

| Source | Baseline commit | Role |
| --- | --- | --- |
| Original ControlCenter root | `55a1d5c` | Source/configuration snapshot and `SOURCE_SNAPSHOT.json` |
| Original Python daemon | `4cedca2` | Power policies, devices, lightbar and desktop behavior |
| Original Python GUI | `53a493b` | Personal interface behavior and physical LED calibration |
| UCC Revorked | `eb98bfa` | Native application/backend foundation before the port |

The original repositories and their committed snapshots are retained separately.
The integration commits live on `codex/frixit-control-center`. Imported personal
data and machine-specific deployment bundles are local ignored artifacts.

## Upstream projects and attribution

- [UCC Revorked](https://github.com/noteMASTER11/UCC-Revorked), by noteMASTER11.
- [Uniwill Control Center](https://github.com/nanomatters/ucc), by nanomatters,
  is the earlier UCC foundation.
- [TUXEDO Control Center](https://github.com/tuxedocomputers/tuxedo-control-center)
  supplies the original TUXEDO IO layer; the separate kernel drivers retain their
  own licensing and notices.

Existing upstream history, copyright headers and license notices are preserved.
The combined native suite remains GPL-3.0-or-later. See
[open-source notices](THIRD_PARTY.md).

## Scope and publication

The original CPU temperature-limit implementation and `gpu-run` are excluded at
the owner's request. Imported profiles and settings override conflicting upstream
defaults; Enter uses physical LED 56, left Shift 21 and left Ctrl 0.

The combined fork is hosted at [Frixlt/UCC](https://github.com/Frixlt/UCC),
configured as `origin`. [UCC Revorked](https://github.com/noteMASTER11/UCC-Revorked)
is retained as `upstream`. Integration build links point to Frixlt/UCC.
The original ControlCenter repositories remain separate local source snapshots;
their Git histories have not been merged into the UCC history.
