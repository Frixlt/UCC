# AGENTS.md (`ucc-rusted`)

Hybrid Linux laptop control center: 10 Rust crates (`crates/`, MSRV `1.89`, Edition `2021`) + standalone C++20/Qt6 GUI (`ucc-gui/`). License: `GPL-3.0-or-later`.

## 1. Task → File Map (jump directly, avoid repo-wide scans)

| Task | Primary files to open |
|---|---|
| Domain types / validation / DTOs | `crates/ucc-core/src/{profile,fan,cooling,cpu,power,charging,dto}.rs` |
| Config persistence / offline import | `crates/ucc-config/src/{manager,persistence,settings,import,platform_paths}.rs` |
| EC ioctl / fans / CPU / BLE / NVML | `crates/ucc-hardware/src/{platform,ioctl,sysfs,fan_controller,cooler,bluez_cooler,nvml}.rs`, `cpu/`, `devices/`, `telemetry/` |
| Keyboard RGB & Lightbar | `crates/ucc-lighting/src/{keyboard,lightbar,transport,hid_transport}.rs` |
| D-Bus API (123 methods, 7 signals) | `crates/ucc-dbus/src/{interface,backend,auth,proxy,ipc}.rs`, `uccd/com.uniwill.uccd.{conf,policy}` |
| Root daemon & profile transactions | `crates/uccd/src/{main,orchestrator,lid,sleep_state}.rs`, `backend/{dbus_impl,profile_tx,cooling,cpu,display_gpu,lighting_power}.rs` |
| Oneshot helpers | `crates/uccd/src/bin/{ucc-hardware,ucc-sleep,ucc-import-check}.rs` |
| CLI / Tray / Session Agent | `crates/ucc-cli/src/`, `crates/ucc-desktop/src/`, `crates/ucc-session-agent/src/` |
| Active C++20 Qt6 GUI & D-Bus client | `ucc-gui/src/`, `libucc-dbus/` |
| Packaging / versions / CI / xtask | `crates/xtask/src/{main,maintenance}.rs`, `PKGBUILD`, `CMakeLists.txt`, `debian/`, `ucc.spec`, `package.nix`, `.github/workflows/` |

## 2. Do Not Read / Do Not Edit (saves context tokens)

- **Skip large generated/binary assets:** `Cargo.lock`, `flake.lock`, `screenshots/`, `icons/`, `nvidia_findings/*.txt`, `target/`, `artifacts/`.
- **Legacy C++ (scheduled for deletion in M13.7+ — do NOT add features):** `uccd/src/*`, `uccd/inc/*`, `ucc-cli/*`, `ucc-tray/*` (retired per ADR-03), `ucc-session/*`, `ucc-gnome/*` (optional external GJS). Keep only `uccd/com.uniwill.uccd.{conf,policy,service}` and `uccd/uccd*.service`.

## 3. Hard Invariants (non-negotiable)

1. **Zero raw host hardware I/O in tests:** never touch host `/sys`, `/proc`, `/dev`; use `SysfsRoot` + mock `PlatformBackend`/`LightingTransport`/`Authorizer`.
2. **D-Bus wire parity:** all 123 methods and 7 signals on both `com.uniwill.uccd` and `de.uniwill.uccd` (`define_uccd_interface!`); enforce `MAX_DBUS_JSON_PAYLOAD_BYTES` (256 KiB).
3. **Polkit boundaries:** identify callers via `system-bus-name` only (never `unix-process` PID). `ManageHardware` is required for TDP, `fan.useControl == false`, charge thresholds/type, cTGP, and manual pump voltage.
4. **Pump code `1` (`V12`) is forbidden:** valid manual codes are `0` (`11V`), `2` (`7V`), `3` (`8V`), `4` (`Off`). Reject `1` everywhere.
5. **Zero D3cold GPU wakeups (`ADR-04`):** load `libnvidia-ml.so.1` via `libloading` only when PCI `power/runtime_status == "active"`.
6. **Zero panics in runtime:** no `unwrap()`, `expect()`, `panic!()`, `todo!()` in production crates; recover poisoned locks with `unwrap_or_else(|e| e.into_inner())`.
7. **Standalone Qt6 GUI (`ADR-01`, `ADR-03`):** `ucc-gui` links only `Qt6::{Core,Gui,Widgets,DBus,Charts}` + static `libucc-dbus`. Never add `KF6`, `Plasma`, `QML`, or `Qt6Bluetooth`.
8. **Root file safety & clean licensing:** use `libc::O_NOFOLLOW` (under `#[cfg(unix)]`), never hardcode `0o400000`; never import proprietary/OEM modules (`proprietary/`, `uwccd/lighting/oem/`).

## 4. Exact Commands (prefer targeted runs first to save output tokens)

```bash
export PATH="$HOME/.cargo/bin:$PATH"

# Fast targeted iteration (use during development):
cargo test -q -p <crate> <test_name> --locked --offline

# Mandatory Rust pre-commit gates:
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
dbus-run-session -- cargo test -q --workspace --locked --offline --no-fail-fast
cargo run -q -p xtask --locked --offline -- check-versions
```

- **Version bumps:** exclusively via `cargo run -p xtask --locked --offline -- release <X.Y.Z> [--dry-run]` (syncs all 6 manifests).
- **Root commands:** use `pkexec` in agent tool calls; write `sudo` in user-facing docs.
- **Commits:** Conventional Commits (`<type>(<scope>): <summary>`). Never run `git push` or `git reset --hard` without explicit user approval.
- **Docs style:** strict engineering prose only; zero marketing fluff or first-person "наш"/"our".

## 5. Progressive Disclosure (read ONLY when your task enters that area)

- Daemon / transactions / D-Bus / Polkit: `crates/uccd/AGENTS.md`, `docs/DBUS_API.md`
- Hardware / sysfs / NVML / BLE / ioctl: `crates/ucc-hardware/AGENTS.md`, `docs/HARDWARE_MATRIX.md`
- C++20 Qt6 GUI & `interop_daemon` CTest: `ucc-gui/AGENTS.md`
- Deep onboarding & adding D-Bus methods: `docs/AI_AGENT_GUIDE.md`, `docs/ARCHITECTURE.md`
- Architectural decisions: `docs/rust/execution/decisions/ADR-01..ADR-06`
