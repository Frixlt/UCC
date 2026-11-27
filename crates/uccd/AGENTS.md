# crates/uccd/AGENTS.md

Scoped rules loaded when editing `crates/uccd/` or `crates/ucc-dbus/`.

## Module Map
- `src/orchestrator.rs`: Tokio runtime startup, `Type=dbus` bus acquisition, signal dispatch, bounded shutdown deadline (`run_detached_blocking`).
- `src/backend/profile_tx.rs`: 3-phase profile transaction engine (seqlock + reconcile + rollback).
- `src/backend/{cooling,cpu,display_gpu,lighting_power,dbus_impl}.rs`: domain handlers.
- `src/sleep_state.rs` + `src/bin/ucc-sleep.rs`: `/run/ucc/sleep-state.json` (`0600`, `O_NOFOLLOW`).
- `src/bin/{ucc-hardware,ucc-import-check}.rs`: oneshot emergency fan reset & offline config validator.

## Concurrency & Security Rules
- **Rust 2021 `if let` drop scope trap:** NEVER write `if let Some(x) = self.lock_mutex(&m).take() { self.lock_mutex(&other); }` — the first `MutexGuard` stays locked until the end of the `if let` block (causes AB-BA deadlocks). Bind `let x = self.lock_mutex(&m).take();` on a separate line first.
- **Never block Tokio workers:** route all blocking sysfs/ioctl/subprocess work through `OffloadLanes` (`crates/ucc-dbus/src/interface.rs`), including slow `Get*` methods like `GetPrimeState`.
- **Single shared `Arc<OffloadLanes>`:** share one `OffloadLanes` across all 4 registered D-Bus interface/path pairs.
- **Shutdown safety:** never spawn hardware I/O in `tokio::task::spawn_blocking` inside background loops; use `run_detached_blocking` so `Runtime::drop` obeys the shutdown deadline.

## Fast Targeted Tests
```bash
dbus-run-session -- cargo test -q -p ucc-dbus --locked --offline
dbus-run-session -- cargo test -q -p uccd --test daemon_integration --locked --offline
```
