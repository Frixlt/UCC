# Runbook Агента 2 — `ucc-core`, `ucc-config`, `ucc-dbus`, `uccd` и systemd-сервисы

- **Ветка:** `release/a2-core-config-ipc-daemon` (от `rust/migration`)
- **Эксклюзивная зона владения:** `crates/ucc-core/**`, `crates/ucc-config/**`, `crates/ucc-dbus/**`, `crates/uccd/**`, `uccd/**`
- **Обязательное чтение перед стартом:** `docs/release-v1/CONTRACT.md`, `docs/release-v1/SYNC_PROTOCOL.md`, `docs/audit/TOTAL_AUDIT_2026_11_28.md`.

## Фаза 1 — 5 параллельных субагентов-исполнителей

### Субагент 2.1: `crates/ucc-core/**`
- Включить `#![forbid(unsafe_code)]` в `crates/ucc-core/src/lib.rs`.
- `profile.rs`, `cpu.rs`, `dto.rs`, `fan.rs`, `settings`:
  - Добавить валидацию `nvidia_ctgp_offset` (`0..=1000`) в `UccProfile::validate()`.
  - Добавить поля контракта: `ProfileCpu::max_temp_c: Option<u8>` (`80..=95`), `UccProfile::nvidia_dynamic_boost_w: Option<u8>` (`0..=25`), `ExperimentalSettings { mode_button_0x00b0_action: String }` (`"firmware" | "cycle_profiles" | "toggle_custom_max" | "emit_key_f14"`).
  - В `manual_fan_speed_serde`: считать sentinel только `-1`, значения вне `-1..=100` отклонять с `Err`.
  - В `dto.rs`: убрать тихие `.clamp()` и `.and_then()` до `validate()`, заменить `u32 as i32` на `i32::try_from`.
  - В `fan.rs`/`profile.rs`: проверять монотонность `table_pump` и запретить `speed > 3` (`V12`).
  - В `cpu.rs` & `profile.rs`: добавить валидацию `online_cores <= 4096`, charset для `governor`/`epp`, непустые `id`/`name`, `ProfileDisplay` при включённых флагах.
- `cooling.rs`: защитить `TemperatureFilter::new` / `filtered_value()` от `NaN`/`Inf` и `alpha` вне `(0.0..=1.0]` (иначе `NaN as i32 == 0` отключает перегрев!).
- `power.rs:377`: заменить `unreachable!()` на явный `match`.

### Субагент 2.2: `crates/ucc-config/**`
- Убрать все 4 `unsafe`-блока (`persistence.rs:77-85, 181-184`, `manager.rs:143-146, 193-196`) через `nix::unistd::geteuid()`, включить `#![forbid(unsafe_code)]` в `crates/ucc-config/src/lib.rs`.
- `persistence.rs` & `manager.rs`:
  - Перевести `write_atomic()` и `restore_from_backup()` на `openat`/`renameat` с `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`, ограничением размера `MAX_CONFIG_FILE_BYTES` (8 MiB), единым `TempFileGuard` + `next_temp_id()`.
  - Ограничить права файлов сверху: `mode & 0o644` (для root `/etc/ucc`) / `0o600` (для user), никогда не сохранять `0o666`/`0o777`.
  - Починить `cleanup_temp_files` (пропускать симлинки и живые PID), построение `.bak` через `OsString` (не `.display()`), логировать ошибки `dir_f.sync_all()`.
  - Расширить `validate_global_settings()` (проверка `charge_type`, `platform_backend`, `experimental.mode_button_0x00b0_action`, `state_map`).
- `import.rs` & `legacy.rs`: лимит размера в `read_json_file`, `read_exact` вместо `read` в `read_tcc_fan_presets`, `rate.is_finite()`, уникальные ключи `tcc-imported-{id}` в `convert_mechrevo_tcc`, `i32::try_from` в `legacy.rs`, убрать `.unwrap()`/`.expect()`/`points[high - 1]`.

### Субагент 2.3: `crates/ucc-dbus/**` + `uccd/com.uniwill.uccd.{policy,conf}`
- **`named_pipe.rs` (P0):**
  - `inspect_pipe_client_identity` -> `Result<CallerIdentity, NamedPipeError>`: при сбое `ImpersonateNamedPipeClient` возвращать `Err` и разрывать соединение (fail-closed!). Проверять `TokenImpersonationLevel` и членство в `WinInteractiveSid`/`SY`/`BA`.
  - Убрать `GW` (`FILE_CREATE_PIPE_INSTANCE`) из `WINDOWS_PIPE_SDDL` для `IU` (заменить на `0x0012019b`), добавить `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION` в клиент.
  - Выносить соединения `NamedPipeServer` в ограниченный пул воркеров с таймаутом и вызывать `on_client_disconnected(&caller.principal)` при EOF (устранить утечку `RegisterPowerRequest`).
- **`ipc.rs` (P0/P1):**
  - `required_access_level` -> `Option<IpcAccessLevel>`: явно перечислить все `Read`-методы, `_ => None` (fail-closed!).
  - Убрать публичный `dispatch_json_rpc` (закрыть `#[cfg(test)]`).
  - Перевести `TurnOffWaterCoolerPump` / `TurnOffWaterCoolerFan` на `ManageHardware`.
  - Валидировать JSON профиля (десериализация `UccProfile`) и целочисленные диапазоны на границе IPC/D-Bus до Polkit и worker pool.
- **`interface.rs` & `auth.rs` (P0/P1):**
  - Перевести блокирующие D-Bus `Read` RPC (sysfs/ioctl/hidraw/procfs) на `self.offload_or(OffloadLane::Read, ...)` или чтение из кэшированного снапшота без блокировки Tokio.
  - Добавить D-Bus методы `GetModeButtonAction() -> s` и `SetModeButtonAction(s) -> b` (`ManageHardware`).
  - Сузить `should_try_legacy_action` в `auth.rs` (не матчить произвольное `"Action"`).
  - Включить `#![cfg_attr(not(windows), forbid(unsafe_code))]` в `crates/ucc-dbus/src/lib.rs`.

### Субагент 2.4: `crates/uccd/src/backend/{profile_tx.rs, cooling.rs, lighting_power.rs, display_gpu.rs, dbus_impl.rs}`
- **`profile_tx.rs` (P0):**
  - Починить `temp_profile_name`: обновлять под `hw_reconcile_lock` в Phase 3; пропускать 200ms перезапись из `state_map` пока `temp_profile_name.is_some()` и `!state_changed && !force_apply`; в `set_temp_profile("")` сбрасывать `last_applied_profile_state = None` и вызывать `reconcile_power_state(true)`; в `apply_profile()` очищать `temp_profile_name`.
  - Свести откат `apply_profile_txn` в единую структуру `TxUndo` (ODM, TDP, CPU, display/webcam, charging, cTGP, keyboard), передавать её целиком в rollback/reconcile, не сбрасывать `hw_reconcile_lock` до отката, не глотать ошибки Phase 3 (`2142-2155`) и `reconcile_to_latest_committed`.
  - В `delete_custom_profile_impl` обновлять `latest_committed_profile.hw_tdps` атомарно.
  - Поддержать применение `profile.cpu.max_temp_c` (`set_tcc_offset(105 - max_temp_c)`) и `profile.nvidia_dynamic_boost_w` (через `PlatformBackend`, graceful skip на `Unsupported`).
- **`cooling.rs`, `lighting_power.rs`, `display_gpu.rs`, `dbus_impl.rs` (P1):**
  - Ввести `MAX_CUSTOM_FAN_PROFILES = 64` и `MAX_CUSTOM_KEYBOARD_PROFILES = 64` со строгой валидацией JSON (не хранить сырой `Value` и лишние ключи).
  - Перенести `load_saved_cooler_identity` и `sync_water_cooler_pwup` в `spawn_blocking`, обернуть `discover_coolers()` в `tokio::time::timeout(5s)`.
  - Заменить `"prime-select"` по `PATH` на абсолютные пути `/usr/bin/prime-select` / `/sbin/prime-select`.
  - Расширить `GetCpuPowerValuesJSON`, `GetDGpuInfoValuesJSON`, `GetSystemInfoJSON` опциональными live EC-полями из кэша телеметрии (`pl1W`, `pl2W`, `pl4W`, `tccLimitC`, `pmscMode`, `ctgpW`, `dynamicBoostW`, `tccSupported`, `tccMinC`, `tccMaxC`) строго по `CONTRACT.md`.

### Субагент 2.5: `crates/uccd/src/{main.rs, orchestrator.rs, backend.rs, sleep_state.rs, bin/*}` + `uccd/*.service`
- **Чистый EC handoff (P0):**
  - Явно вызывать `self.platform.set_mode_enable(false)` и `self.platform.set_fans_auto()` в `LiveUccdBackend::shutdown()`, `on_system_suspend()`, `ucc-sleep prepare` и `ucc-hardware --fan-auto` (не полагаясь на `Drop`).
  - Сохранять и джойнить с таймаутом `JoinHandle` всех OS-воркеров (`tel`, `fan`, `pow`, `cpu`, `lightbar`, `wmi`, `nvml-init`), проверять `shutting_down` в `uccd-nvml-init`.
- **Экспериментальное переназначение `0x00B0`:**
  - Отделить WMI-поллер в `orchestrator.rs` от `Option<LinuxUinputEmitter>` (поллить `/dev/uwcc` даже без `/dev/uinput`), логировать ошибки с rate-limit, завершать поток на `Unsupported`.
  - При `mode_button_0x00b0_action != "firmware"` управлять `0x0741 BIT(0)` и обрабатывать `PerformanceModeCycle` в отдельном bounded-канале (не блокируя 50ms WMI-поллер).
- **Безопасность и hardening:**
  - Убрать Unix `unsafe` из `main.rs`, `backend.rs`, `ucc-sleep.rs` (через `nix`) и включить `#![cfg_attr(not(windows), forbid(unsafe_code))]`.
  - Починить `PidLock::drop` (`(dev, ino)` проверка) и `stop_daemon` (проверка UID + `pidfd`), `DirBuilder::mode(0o700)` в `sleep_state.rs`, `ucc-sleep` (запретить `UCC_SYSTEMCTL` вне `#[cfg(test)]`, не трогать EC при провале `stop_daemon`, убрать pipe deadlock).
  - Усилить `uccd/uccd.service` (сузить `ReadWritePaths` с `/run` до `/run/ucc`, добавить `ProtectKernelLogs`, `ProtectClock`, `ProtectHostname`, `RestrictSUIDSGID`, `RestrictRealtime`, `SystemCallArchitectures=native`) и `uccd/uccd-sleep.service` (добавить песочницу).

## Фаза 2 — 5 независимых adversarial-ревьюеров + Финальный Gate
Запусти 5 независимых ревьюеров (при необходимости запросив у пользователя «продолжай» для сброса лимита субагентов), исправь все замечания и прогони:
```bash
export CARGO_TARGET_DIR="$HOME/ucc-target-slim" TMPDIR="$HOME/ucc-tmp" RUSTFLAGS="-D warnings -C debuginfo=0"
cargo fmt --all -- --check
dbus-run-session -- cargo test -j 2 -p ucc-core -p ucc-config -p ucc-dbus -p uccd
```
Запушь в `release/a2-core-config-ipc-daemon` и обнови `sync/agent2.md` до `READY_FOR_MERGE`.
