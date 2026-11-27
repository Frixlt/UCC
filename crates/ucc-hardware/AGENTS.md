# crates/ucc-hardware/AGENTS.md

Scoped rules loaded when editing `crates/ucc-hardware/` or `crates/ucc-lighting/`.

## Key Entrypoints
- `src/sysfs.rs`: `SysfsRoot` + `SysfsWriteJournal` — ALL sysfs/procfs reads/writes must route through this (no raw `std::fs::write`).
- `src/platform.rs` & `src/ioctl.rs`: `PlatformBackend` trait & `TuxedoIoBackend` (`/dev/tuxedo_io` ABI verified against `tuxedo_io_ioctl.h`).
- `src/fan_controller.rs`: fan curve tick loop + mandatory `set_fans_auto()` failsafe when any required temp sensor is unavailable.
- `src/cooler.rs` & `src/bluez_cooler.rs`: LCT21001/LCT22002 BLE GATT state machine (`BLE_WRITE_GAP >= 80ms`, `PumpVoltage::V12` code `1` forbidden).
- `src/nvml.rs`: dynamic `libloading` NVML/NvAPI with PCI `power/runtime_status == "active"` D3cold guard (fail-closed on read error).
- `src/cpu/{controller,topology,worker}.rs`: `intel_pstate`, `amd-pstate`, `acpi-cpufreq` (`cpufreq/boost`), P/E topology.

## Safety & Test Rules
- **Subprocess execution:** use strict absolute path allowlists + `.env_clear()` (never fall back to `PATH`).
- **Env mutation in tests:** guard every `std::env::set_var` / `remove_var` with a static `ENV_LOCK` + RAII `Drop` guard.
- **Mock fixtures:** `stand-fixtures/*.c` and `extra-fixtures/*.c` are compiled on the fly for `LD_PRELOAD` tests; never skip a security test silently with `return;`.

## Fast Targeted Tests
```bash
cargo test -q -p ucc-hardware --locked --offline
cargo test -q -p ucc-lighting --locked --offline
```
