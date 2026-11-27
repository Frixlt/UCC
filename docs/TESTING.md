# Testing and Verification (`ucc-rusted`)

[Building and Packaging](BUILDING.md) · [Migration and Recovery](MIGRATION_AND_RECOVERY.md)

This document covers the Rust workspace checks, C++/Qt6 CTest suites (normal and read-only preview), automated `ucc-gui` `ldd` linkage verification (`M12.7` / `ADR-01`), isolated test harnesses (`dbus-run-session`, `LD_PRELOAD` fixtures, mock BlueZ, Wayland/Hyprland), and the hardware acceptance checklist.

---

## 1. Rust Workspace Verification

Run all commands from the repository root. Use an out-of-tree `CARGO_TARGET_DIR` (for example `/tmp/ucc-cargo-target`) to keep the working tree clean, and append `--locked --offline` in offline environments.

### Full Lint, Formatting, Version Parity, and Test Gate

```bash
export CARGO_TARGET_DIR=/tmp/ucc-cargo-target

# 1. Version parity across all 6 manifests, JSON metadata validity, CycloneDX SBOM, and importer invariants
cargo run -p xtask -- check-versions
cargo run -p xtask -- parser-check

# 2. Formatting check
cargo fmt --all -- --check

# 3. Static analysis with Clippy (all targets, warnings denied)
cargo clippy --workspace --all-targets -- -D warnings

# 4. Full workspace unit and integration test suite on an isolated D-Bus session bus
dbus-run-session -- cargo test --workspace --locked --offline --no-fail-fast
```

Targeted run for the core daemon, client, and tooling crates (useful during focused iteration; add `-p ucc-hardware -p ucc-lighting` for all 10 crates):

```bash
CARGO_TARGET_DIR=/tmp/ucc-cargo-target \
  dbus-run-session -- cargo test \
  -p uccd -p ucc-core -p ucc-config -p ucc-dbus \
  -p ucc-cli -p ucc-desktop -p ucc-session-agent -p xtask
```

In CI (`.github/workflows/build.yml`), Rust checks run on a toolchain matrix of MSRV `1.89.0` and pinned `1.99.1` with `--locked --no-fail-fast`.

### Rust Crate Test Inventory (10 Crates, 92 Integration Suites, 661 Tests)

Counts reflect `#[test]` / `#[tokio::test]` functions (`34` unit + `627` integration = `661` total) and Cargo integration test targets under `crates/*/tests/`:

| Crate | Unit | Integration | Total | Integration Suites (`crates/<crate>/tests/`) |
| --- | ---: | ---: | ---: | --- |
| `ucc-core` | 0 | 50 | 50 | 8: `core_charging_and_fans`, `core_display_and_thresholds`, `core_profile_invariants`, `core_serialization`, `core_validation_rules`, `domain_validation`, `dto_mapping`, `property_invariants` |
| `ucc-config` | 12 | 82 | 94 | 13: `config_atomic_writes`, `config_backup_and_defaults`, `config_edge_cases`, `config_file_permissions`, `config_persistence`, `config_policy_parsing`, `config_schema_migration`, `config_security`, `config_thresholds_and_restore`, `config_validation`, `corruption_recovery`, `offline_import`, `settings_manager_tests` |
| `ucc-hardware` | 0 | 171 | 171 | 30: `cooler_protocol`, `cooler_stress_tests`, `cooling_engine`, `cpu_topology_and_control`, `fake_bluez_cooler`, `hardware_backends`, `hardware_device_discovery`, `hardware_driver_contracts`, `hardware_fan_and_cooler`, `hardware_ioctl_bounds`, `hardware_platform_profile`, `hardware_probe_matrix`, `hardware_safety`, `hardware_sysfs_guards`, `hardware_traits`, `ioctl_abi`, `ioctl_preconditions`, `nvml_device_lifecycle`, `nvml_edge_cases`, `nvml_error_handling`, `nvml_guard`, `nvml_memory_and_bdf`, `nvml_telemetry_abi`, `platform_backend`, `power_devices_and_tunables`, `sysfs_failure_matrix`, `sysfs_path_containment`, `telemetry_suite`, `vendor_detection`, `windows_wmi_backend` |
| `ucc-dbus` | 2 | 35 | 37 | 8: `auth_tests`, `contract_tests`, `dbus_auth_and_payloads`, `dbus_introspection_parity`, `dbus_method_dispatch`, `dbus_security_limits`, `dbus_skeleton`, `ipc_framing` |
| `uccd` | 0 | 171 | 171 | 6: `daemon_integration`, `daemon_lid_and_reconcile`, `daemon_policies`, `daemon_security`, `fault_matrix`, `import_check_corpus` |
| `ucc-cli` | 2 | 38 | 40 | 7: `cli_daemon_integration`, `cli_error_handling`, `cli_fan_and_dbus`, `cli_fan_payload_validation`, `cli_options_and_flags`, `cli_security`, `cli_tests` |
| `ucc-desktop` | 7 | 18 | 25 | 7: `tray_actions`, `tray_lifecycle_and_timeouts`, `tray_menu`, `tray_reconnect`, `tray_security`, `tray_state_sync`, `window_activation` |
| `ucc-session-agent` | 6 | 12 | 18 | 3: `session_agent_integration`, `session_lid_and_display`, `session_refresh_and_timeouts` |
| `ucc-lighting` | 0 | 25 | 25 | 6: `hid_transport`, `lighting_device_controls`, `lighting_hidraw_interface`, `lighting_protocol`, `lighting_safety`, `lighting_tests` |
| `xtask` | 5 | 25 | 30 | 4: `build_verification`, `deployment_security`, `maintenance_workflows`, `packaging_contracts` |
| **Total** | **34** | **627** | **661** | **92 integration test targets** |

---

## 2. C++ / Qt6 CTest, Python, and GNOME Suites

All CMake test builds compile with `-DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS"` to catch STL bounds violations, and use out-of-tree build directories under `/tmp/`.

### Pre-Building `interop_daemon`

Cross-language tests `test_rust_uccd_interop` and `test_gui_rust_e2e` launch the Rust daemon example binary `interop_daemon`:

```bash
CARGO_TARGET_DIR=/tmp/ucc-cargo-target \
  cargo build -p uccd --example interop_daemon --locked --offline
```

Pass the compiled binary path through `UCC_INTEROP_DAEMON_BIN` (defaults to `target/debug/examples/interop_daemon` when unset).

### Normal Suite (`-DUCC_READ_ONLY_PREVIEW=OFF`)

Standalone Rust-runtime configuration (`-DBUILD_TRAY=OFF`):

```bash
cmake -S . -B /tmp/ucc-build-normal -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS" \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DUCC_READ_ONLY_PREVIEW=OFF \
  -DBUILD_TRAY=OFF \
  -DBUILD_TESTS=ON
cmake --build /tmp/ucc-build-normal --parallel 2

# Automated ldd linkage invariant check for ucc-gui (M12.7 / ADR-01)
bash scripts/check_gui_ldd.sh /tmp/ucc-build-normal/bin/ucc-gui

# Full CTest run
UCC_INTEROP_DAEMON_BIN=/tmp/ucc-cargo-target/debug/examples/interop_daemon \
  ctest --test-dir /tmp/ucc-build-normal --output-on-failure --parallel 2
```

#### Normal Suite Breakdown (`tests/CMakeLists.txt`)

`tests/CMakeLists.txt` registers **49** base CTest entries without optional JS runtimes (including `test_gui_ldd_invariants`), up to **53** when both `gjs` and `node` are installed (`50` with `gjs` only, `52` with `node` only):

- **19 C++ unit tests**: `test_session_telemetry`, `test_process_terminator`, `test_state_utils`, `test_sleep_state`, `test_lightbar`, `test_driver_health`, `test_notification_monitor`, `test_power_policy`, `test_cpu_selection`, `test_power_devices`, `test_lid_wakeup`, `test_gpu_sysfs`, `test_display_backlight`, `test_keyboard_controller`, `test_fan_profile`, `test_profile_manager`, `test_settings_manager`, `test_metrics_history`, `test_aquaris_selection`.
- **14 Python CTest suites**: `test_deployment`, `test_prepare_deployment`, `test_desktop_transition`, `test_import_check`, `test_sleep_install`, `test_tcc_import`, `test_controlcenter_migration`, `test_control_authorization`, `test_watercooler_override`, `test_device_identification`, `test_bluetooth_retry`, `test_bluetooth_startup`, `test_display_identity`, `test_contrib_packaging`.
  - Note: `tests/` contains **15** `test_*.py` files in total. The 15th file, `tests/test_ite8291_brightness.py`, is a standalone kernel-driver callback harness invoked manually against a patched `tuxedo-drivers` source tree (`python3 tests/test_ite8291_brightness.py /path/to/tuxedo-drivers`).
  - Additionally, `PKGBUILD` executes the Mechrevo migration test:
    ```bash
    python3 contrib/mechrevo/tests/test_migration.py
    ```
- **3 session-agent D-Bus tests**: `test_driver_alert`, `test_refresh_advisor_modes`, `test_refresh_advisor_lifecycle`.
- **4 GNOME Shell tests**:
  - 1 GJS test (when `gjs` is installed): `test_gnome_async` (`tests/test_gnome_async.js` via `tests/run_gnome_async_test.sh`);
  - 3 Node.js tests (when `node` is installed): `test_gnome_launch` (`tests/test_gnome_launch.cjs`), `test_gnome_refresh` (`tests/test_gnome_refresh.cjs`), `test_gnome_extension_contracts` (`tests/test_gnome_extension_contracts.cjs`).
- **11 GUI tests**: `test_fan_curve_editor`, 5 shards of `test_gui_audit` (52 slots total: `test_gui_audit_profiles_cpu` [16], `test_gui_audit_dashboard_cooling` [7], `test_gui_audit_lighting_devices` [8], `test_gui_audit_monitor_theme` [12], `test_gui_audit_preview_gallery` [9]), `test_gui_regressions` (14 regression slots), `test_gui_instance`, `test_gui_rust_e2e`, `test_system_monitor`, `test_app_logging`.
- **1 C++/Rust D-Bus client interop test**: `test_rust_uccd_interop`.
- **1 GUI `ldd` linkage invariant test**: `test_gui_ldd_invariants` (invokes `scripts/check_gui_ldd.sh`).

### Automated `ucc-gui` `ldd` Invariant Check (`M12.7` / `ADR-01`)

Per `M12.7` and `ADR-01`, the standalone `ucc-gui` binary built with `-DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF` must link only against `Qt6::{Core,Gui,Widgets,DBus,Charts}` and statically linked `libucc-dbus`. Automated verification runs through `scripts/check_gui_ldd.sh` and CTest target `test_gui_ldd_invariants`:

```bash
# Direct script invocation
bash scripts/check_gui_ldd.sh /tmp/ucc-build-normal/bin/ucc-gui

# Via CTest
ctest --test-dir /tmp/ucc-build-normal -R "^test_gui_ldd_invariants$" --output-on-failure
```

The check fails if `ldd` reports any missing (`not found`) shared object or any link to forbidden libraries:

- `KF6` (KDE Frameworks / Plasma)
- `QML` / `Qt6Qml` / `Qt6Quick`
- `Qt6Bluetooth`
- `libucc-dbus.so` (must be linked statically, never as a public shared library)

Equivalent manual shell check:

```bash
! ldd /tmp/ucc-build-normal/bin/ucc-gui | grep -qE 'not found|KF6|Qt6Qml|Qt6Quick|Qt6Bluetooth|libucc-dbus'
```

### Read-Only Preview Suite (`-DUCC_READ_ONLY_PREVIEW=ON`)

```bash
cmake -S . -B /tmp/ucc-build-preview -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS" \
  -DUCC_READ_ONLY_PREVIEW=ON \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_TESTS=ON
cmake --build /tmp/ucc-build-preview --parallel 2 \
  --target test_gui_audit test_gui_regressions test_gui_instance test_app_logging ucc-gui

# Targeted 3-test preview gate (matches CI)
ctest --test-dir /tmp/ucc-build-preview \
  -R "^(test_gui_audit_preview_gallery|test_gui_instance|test_app_logging)$" \
  --output-on-failure

# Optional: preview-safe 8-slot regression subset
ctest --test-dir /tmp/ucc-build-preview \
  -R "^test_gui_regressions$" \
  --output-on-failure
```

Under `-DUCC_READ_ONLY_PREVIEW=ON`:
- `test_gui_audit_preview_gallery` verifies write blocking (`previewMustBlockEveryWritePath`, `previewMustNotSaveLocalMonitorPreferences`), `test_gui_instance` verifies single-instance isolation, and `test_app_logging` verifies preview logging.
- `test_gui_regressions` runs a preview-safe 8-slot subset (`n12_mainwindow_ctor_avoids_sync_dbus_capability_calls`, `n13_keyboard_visualizer_avoids_blocking_detect_layout_in_ctor`, `n26_preview_mode_allows_readonly_webcam_sw_available`, `n31_fan_control_tab_disables_remove_for_builtin_profile`, `n28_sysfs_node_is_available_does_not_terminate_on_eacces`, `p0_14_monitor_tab_sticky_mark_click_no_sync_self_delete_uaf`, `p0_15_hardware_tab_accepts_null_parent_without_crash`, `m_gui_05_read_json_int_preserves_negative_gpu_clock_offsets`).
- `test_gui_rust_e2e`, `test_system_monitor`, and `test_rust_uccd_interop` are excluded from CTest in preview builds.

### Cross-Language Interop Tests (`test_rust_uccd_interop` and `test_gui_rust_e2e`)

Both suites execute via `dbus-run-session -- bash tests/run_rust_interop_test.sh <test-binary> <source-dir> [args...]`:

- `test_rust_uccd_interop`: verifies C++ `UccdClient` (`libucc-dbus`) compatibility against a live Rust `interop_daemon` (JSON contracts, signals, profile and policy mutations, isolated `UCC_INTEROP_CONFIG_DIR`).
- `test_gui_rust_e2e`: boots the full Qt6 `MainWindow` against the Rust `LiveUccdBackend` (`interop_daemon`), exercises all 10 sidebar sections (9 navigation tabs + About), Light/Dark theme switching, profile and lid-policy writes (`F45 Lid`), rejection of invalid commands without false success reporting, and write blocking under `UCC_READ_ONLY_PREVIEW=1`.

---

## 3. Isolated Test Harnesses

### Private System D-Bus

`tests/run_private_system_bus.sh` redirects the system bus address to an ephemeral session bus inside `dbus-run-session`:

```bash
dbus-run-session -- bash tests/run_private_system_bus.sh <command>
```

Environment variables set: `DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"` and `GIO_USE_VFS=local`. The wrapper `tests/run_rust_interop_test.sh` additionally creates isolated temporary `HOME` and `XDG_{CONFIG,DATA,STATE,CACHE}_HOME` directories, invokes `<test-binary>` as a child process (without `exec`) propagating extra arguments and exit code, and guarantees cleanup via `trap ... EXIT`.

### `LD_PRELOAD` C Fixtures (`stand-fixtures/`, `extra-fixtures/`)

Compile all 6 shared fixture libraries with:

```bash
UCC_FIXTURES_DIR=/tmp/ucc-fixtures cargo run -p xtask -- build-fixtures
```

| Source | Artifact | Purpose & Control Environment Variables |
| --- | --- | --- |
| `stand-fixtures/open-ancestor.c`, `extra-fixtures/open-ancestor.c` | `stand/open-ancestor.so`, `extra/open-ancestor.so` | Intercepts `open64` (`O_WRONLY`) on `UCC_E1_SWAP_OPEN_PATH`: renames parent directory `UCC_E1_SWAP_ANCESTOR` and replaces it with a symlink to `UCC_E1_SWAP_DESTINATION` (ancestor TOCTOU defense test) |
| `stand-fixtures/open-swap.c` | `stand/open-swap.so` | Intercepts `open64` (`O_WRONLY`) on `UCC_AUDIT_SWAP_TARGET`: performs `unlink` + `symlink` to `UCC_AUDIT_SWAP_DESTINATION` (target-file TOCTOU defense test) |
| `stand-fixtures/nvml.c` | `stand/libnvidia-ml.so.1` | Mock NVML tracking refcounts (`nvmlAuditRefCount`) and sleeping-GPU probes (`nvmlAuditSleepingProbes`). Controls: `UCC_E1_FIXTURE_SLOW_INIT`, `UCC_AUDIT_FAIL_FIRST_NVML_COUNT`, `UCC_E1_FIXTURE_REVERSED_ORDER`, `UCC_AUDIT_REVERSED_NVML_INDEX` |
| `extra-fixtures/nvml.c` | `extra/index/libnvidia-ml.so.1` | Extended mock NVML without `nvmlDeviceGetHandleByPciBusId_v2` (index fallback, hotplug swap via `UCC_E16_FIXTURE_HOTPLUG_FROM`/`TO`, `UCC_E16_FIXTURE_FORWARD_ORDER`) |
| `extra-fixtures/nvml.c` (`-DAUDIT_WITH_BDF`) | `extra/bdf/libnvidia-ml.so.1` | Mock NVML with `nvmlDeviceGetHandleByPciBusId_v2` and `nvmlAuditBdfCalls` counter (`UCC_E16_FIXTURE_REFUSE_BDF`, BDF routing fallback test) |

### Isolated Mock BlueZ Water Cooler

`crates/ucc-hardware/tests/fake_bluez_cooler.rs` registers mock `org.bluez.Device1` (`name = "LCT21001-TEST"`, Nordic UART Service UUID) and `org.bluez.GattCharacteristic1` (TX/RX) objects on a private D-Bus session bus, testing the full `BluezCoolerClient` connect, LCT protocol exchange, and retry lifecycle without a physical Bluetooth adapter:

```bash
CARGO_TARGET_DIR=/tmp/ucc-cargo-target \
  dbus-run-session -- cargo test -p ucc-hardware --test fake_bluez_cooler --locked --offline
```

### Non-Root Hyprland / Wayland Verification

User-session clients (`ucc-gui`, `ucc-desktop-tray`, `ucc-session-agent`) run strictly as an unprivileged user:

- In headless environments (CI, SSH), `QT_QPA_PLATFORM=offscreen` is set automatically for `test_fan_curve_editor`, `test_gui_instance`, and inside `tests/run_gui_audit_test.sh`.
- Hyprland 0.55+ refresh-rate logic (`hyprctl` mode updates preserving resolution, scale, position, and color profile) and `org.freedesktop.Notifications` integration are verified without root or a live compositor via:
  ```bash
  ctest --test-dir /tmp/ucc-build-normal -R "^test_refresh_advisor_(modes|lifecycle)$" --output-on-failure
  ```
- On a live Wayland session, verify: window focus activation from tray click (`XDG_ACTIVATION_TOKEN`), single-instance lock for `ucc-session-agent`, power-source transition notifications (AC / Battery / USB-C), and display refresh-rate switching only after explicit confirmation or profile change.

---

## 4. Hardware Acceptance Checklist

### A. Non-Destructive Checks

Run before any writes to hardware registers:

1. Kernel driver node and daemon status:
   ```bash
   ls -l /dev/tuxedo_io
   systemctl status uccd.service --no-pager
   ```
2. Read-only EC fan sensor audit:
   ```bash
   sudo /usr/lib/ucc/ucc-hardware --audit
   ```
3. Daemon telemetry and policy status via CLI:
   ```bash
   ucc-cli status
   ucc-cli power status
   ucc-cli devices status
   ucc-cli lightbar status
   ```
4. Read-Only Preview verification: run `./run-preview.sh`, navigate across all tabs, and confirm `/etc/ucc/settings` and hardware registers remain unmodified.

### B. Live Hardware Checks

1. **Discrete NVIDIA GPU D3cold Sleep (`ADR-04`)**:
   - In `Power & Devices`, disable **Keep NVIDIA awake (Blackwell workaround)** and apply the policy.
   - Confirm PCI runtime power control is `auto`, persistence mode is off, and with no active GPU workloads or external displays on NVIDIA the status transitions to `suspended`:
     ```bash
     cat /sys/bus/pci/devices/<NVIDIA_BDF>/power/control
     cat /sys/bus/pci/devices/<NVIDIA_BDF>/power/runtime_status
     ```
   - Verify background `uccd` telemetry does not wake a suspended GPU (NVML polling interval under `auto` is at least 16 s, and probes are skipped while `runtime_status` is `suspended`).

2. **BLE Water Cooler (`LCT22002` / `LCT21001`)**:
   - Confirm device visibility in `bluetoothctl` and connection through `uccd`.
   - Verify mode switching (`Auto` / `Manual`), fan duty and pump voltage updates, and that opening manual controls in `ucc-gui` does not stop active cooling.
   - Verify automatic recovery after a transient Bluetooth disconnect.

3. **Suspend / Resume Cycle**:
   - Confirm `uccd-sleep.service` is enabled (`systemctl is-enabled uccd-sleep.service`).
   - Switch to a manual mode (or enable the water cooler), run `systemctl suspend`, and resume.
   - Inspect `journalctl -u uccd-sleep.service -u uccd.service -b`:
     - `ucc-sleep prepare` applied `WakeupPolicy`, wrote `/run/ucc/sleep-state.json`, and stopped `uccd.service`;
     - `ucc-sleep resume` started `uccd.service`, `uccd` restored saved state before claiming the bus name, and `/run/ucc/sleep-state.json` was removed.

4. **CPU, Fans, and Lighting**:
   - Verify `cpu.enabledCores` (individual P/E core toggles, kernel-locked `cpu0` preservation, restoration of all cores on daemon stop).
   - Verify emergency firmware fan fallback: `sudo /usr/lib/ucc/ucc-hardware --fan-auto` (exit code `0`).
   - Verify per-key keyboard backlight (physical LED IDs) and Lightbar (color changes and clean animation termination).
