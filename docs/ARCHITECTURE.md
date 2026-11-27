# Архитектурная спецификация `ucc-rusted` (M13.11)

Документ фиксирует процессную топологию, граф зависимостей workspace, модель конкурентности, авторизацию D-Bus/Polkit, транзакционный движок профилей, конечный автомат сна/пробуждения и архитектурные швы платформенных расширений (`ADR-01`–`ADR-06`).

---

## 1. Многопроцессная топология с разделением привилегий

Система разделена на три домена исполнения: непривилегированная пользовательская сессия (UID >= 1000), шина IPC/авторизации (System D-Bus + Polkit + `systemd-logind`) и привилегированный системный контур (`UID=0`, изолированный директивами systemd hardening). В фоне работают только Rust-процессы; окно настроек `ucc-gui` (C++20 / `Qt6::Widgets` + `Qt6::Charts`) запускается по требованию и полностью выгружается при закрытии (`ADR-01`).

<!-- DIAGRAM PLACEHOLDER: Privilege-separated multi-process topology (User Session <-> System D-Bus/Polkit <-> Root uccd & oneshot helpers) -->

```text
+--------------------------------------------------------------------------------------+
| USER SESSION (UID >= 1000, Wayland / X11, Session D-Bus)                             |
|                                                                                      |
|  +----------------------+   +------------------------+   +------------------------+  |
|  | ucc-desktop-tray     |   | ucc-session-agent      |   | ucc-gui (on-demand)    |  |
|  | (Rust, ksni SNI)     |   | (Rust, flock lock)     |   | (C++20 Qt6::Widgets,   |  |
|  | - Tray menu & status |   | - LidDisplayHandler    |   |  Qt6::Charts, static   |  |
|  | - ProcessTerminator  |   | - RefreshAdvisor       |   |  libucc-dbus)          |  |
|  |   (pidfd_open)       |   |   (hyprctl/kscreen)    |   | - 0 MB Qt in bg        |  |
|  | - XDG_ACTIVATION_    |-->| - org.freedesktop.     |   | - com.uniwill.UccGui   |  |
|  |   TOKEN -> ucc-gui   |   |   Notifications relay  |   |   ActivateWithToken    |  |
|  +----------+-----------+   +-----------+------------+   +-----------+------------+  |
|             |                           |                            |               |
|  +----------+-----------+               |                            |               |
|  | ucc-cli (Rust, clap) |               |                            |               |
|  +----------+-----------+               |                            |               |
+-------------|---------------------------|----------------------------|---------------+
              |                           |                            |
              v                           v                            v
+--------------------------------------------------------------------------------------+
| SYSTEM IPC & AUTHORIZATION LAYER                                                     |
|                                                                                      |
|  System D-Bus: com.uniwill.uccd (object paths: /com/uniwill/uccd, /de/uniwill/uccd)  |
|                interfaces: com.uniwill.uccd, de.uniwill.uccd                         |
|  PolicyKit1:   org.freedesktop.PolicyKit1.Authority (subject: system-bus-name)       |
|                actions: com.uniwill.uccd.{read,control,manage-hardware}              |
|  BlueZ / logind: org.bluez (GATT NUS), org.freedesktop.login1 (LidClosed, Inhibit)   |
+-----------------------------------------+--------------------------------------------+
                                          |
                                          v
+--------------------------------------------------------------------------------------+
| PRIVILEGED SYSTEM LAYER (UID=0, systemd hardening: ProtectSystem=strict,             |
| NoNewPrivileges=true, MemoryDenyWriteExecute=yes, CapabilityBoundingSet=             |
| CAP_SYS_RAWIO CAP_NET_ADMIN CAP_SYS_ADMIN)                                           |
|                                                                                      |
|  +--------------------------------------------------------------------------------+  |
|  | uccd (uccd.service, Type=dbus, BusName=com.uniwill.uccd, /run/uccd.pid flock)  |  |
|  | - DaemonRuntime + LiveUccdBackend + 8 D-Bus OffloadLanes                       |  |
|  | - 5 OS periodic workers (tel 800ms, fan 1s, pow 200ms, cpu 10s, lb 20ms)       |  |
|  | - Async tasks: D-Bus signal dispatcher, NameOwnerChanged, BlueZ BLE, logind    |  |
|  +--------------------------------------------------------------------------------+  |
|                                                                                      |
|  ONESHOT / OFFLINE HELPERS:                                                          |
|  +----------------------------+ +------------------------+ +----------------------+  |
|  | ucc-sleep (root)           | | ucc-hardware (root)    | | ucc-import-check     |  |
|  | uccd-sleep.service         | | ExecStopPost=-...      | | (offline, no hw/dbus)|  |
|  | prepare | resume           | | --audit | --fan-auto   | | settings.json + caps |  |
|  | /run/ucc/sleep-state.json  | | /dev/tuxedo_io failsafe| | schema preflight     |  |
|  +----------------------------+ +------------------------+ +----------------------+  |
+--------------------------------------------------------------------------------------+
```

### Характеристики процессов

| Процесс / бинарный файл | Крейт / стек | UID / контекст | Механизм одиночного экземпляра | Роль |
|---|---|---|---|---|
| `uccd` | `crates/uccd` (Rust) | `root` (`uccd.service`, `Type=dbus`) | `flock(LOCK_EX\|LOCK_NB)` на `/run/uccd.pid` (`O_NOFOLLOW`, `0600`) + D-Bus `RequestNameFlags::DoNotQueue` | Владение аппаратным состоянием, D-Bus API, воркеры телеметрии/охлаждения/питания/подсветки |
| `ucc-sleep` | `crates/uccd/src/bin/ucc-sleep.rs` (Rust) | `root` (`uccd-sleep.service`, `Type=oneshot`, `UMask=0077`) | Однократный запуск systemd (`Before=sleep.target`) | Применение `WakeupPolicy`, сохранение `/run/ucc/sleep-state.json`, останов/запуск `uccd.service` |
| `ucc-hardware` | `crates/uccd/src/bin/ucc-hardware.rs` (Rust) | `root` (`ExecStopPost` в `uccd.service`) | Однократный запуск | Аварийный возврат вентиляторов в авто-режим (`--fan-auto`, `W_UW_FANAUTO`/`W_CL_FANAUTO`) и аудит датчиков (`--audit`) |
| `ucc-import-check` | `crates/uccd/src/bin/ucc-import-check.rs` (Rust) | Непривилегированный / offline | Однократный запуск | Статическая валидация JSON настроек (`<= 16 MiB`) и возможностей клавиатуры без открытия `/dev/*` или D-Bus |
| `ucc-desktop-tray` | `crates/ucc-desktop` (Rust, `ksni`) | Пользователь (Session) | SNI registration `ucc-rusted` | Системный трей (`org.kde.StatusNotifierItem`), запуск/активация `ucc-gui`, завершение GPU-процессов через `pidfd_open` + `pidfd_send_signal` |
| `ucc-session-agent` | `crates/ucc-session-agent` (Rust) | Пользователь (Session) | `flock(LOCK_EX\|LOCK_NB)` на `$XDG_RUNTIME_DIR/ucc-session-agent.lock` | Релей `org.freedesktop.Notifications` в Lightbar, `LidDisplayHandler`, `RefreshAdvisor` (`hyprctl`/`kscreen-doctor`), алерты питания/драйвера |
| `ucc-cli` | `crates/ucc-cli` (Rust, `clap 4.5`) | Пользователь / root | Однократный запуск | CLI-клиент ко всем подсистемам `com.uniwill.uccd`, генерация shell completions и man-страниц |
| `ucc-gui` | `ucc-gui/` (C++20, `Qt6::Widgets`, `Qt6::Charts`) | Пользователь (Session, on-demand) | Session D-Bus `com.uniwill.UccGui` (`Activate` / `ActivateWithToken`) | 10-страничный UI настроек поверх статически слинкованного `libucc-dbus`; без `KF6`, `Plasma`, `QML`, `Qt6::Bluetooth` (`ADR-01`, `ADR-03`) |

---

## 2. Граф зависимостей workspace и `ucc-gui`

Корневой `Cargo.toml` (Rust edition `2021`, `rust-version = "1.89"`, `resolver = "2"`) объединяет 10 крейтов. Прототип `crates/ucc-gui` (`iced`) исключён из workspace (`ADR-01`), а Plasma-апплет `ucc-tray` (`KF6`/`QML`) снят с поставки (`ADR-03`).

<!-- DIAGRAM PLACEHOLDER: Workspace crate dependency DAG (10 Rust crates + standalone C++20 Qt6 ucc-gui) -->

```text
                             +------------+
                             |  ucc-core  |  (serde, serde_json, thiserror)
                             +-----+------+
                                   |
          +------------------------+------------------------+
          |                        |                        |
          v                        v                        v
   +------+-------+        +-------+-------+        +-------+-------+
   |  ucc-config  |        | ucc-hardware  |        |   ucc-dbus    |
   +--+---+-------+        +-------+-------+        +--+--+---+--+--+
      |   |                        |                   |  |   |  |
      |   |                        v                   |  |   |  |
      |   |                +-------+-------+           |  |   |  |
      |   |                | ucc-lighting  |           |  |   |  |
      |   |                +-------+-------+           |  |   |  |
      |   |                        |                   |  |   |  |
      |   +------------+           +-------+   +-------+  |   |  |
      |                |                   |   |          |   |  |
      |                v                   v   v          |   |  |
      |        +-------+-------------------+---+--+       |   |  |
      |        |               uccd               |       |   |  |
      |        | (bins: uccd, ucc-sleep,          |       |   |  |
      |        |  ucc-hardware, ucc-import-check) |       |   |  |
      |        +----------------------------------+       |   |  |
      |                                                   |   |  |
      |   +-----------------------------------------------+   |  |
      |   |                    +------------------------------+  |
      |   |                    |                                 |
      v   v                    v                                 v
+-----+---+-------+   +--------+----------+               +------+------+
|   ucc-desktop   |   | ucc-session-agent |               |   ucc-cli   |
| (ucc-desktop-   |   | (ucc-session-     |               | (ucc-cli)   |
|  tray, ksni)    |   |  agent)           |               +------+------+
+-----------------+   +-------------------+                      |
                                                                 v
+-----------------------------------------+               +------+------+
| Standalone C++20: ucc-gui               |               |    xtask    |
| (Qt6::Widgets, Qt6::Charts, static      |               | (+ ucc-     |
|  libucc-dbus) --[System D-Bus]--> uccd  |               |  config)    |
+-----------------------------------------+               +-------------+
```

### Матрица зависимостей 10 крейтов workspace

| Крейт | Внутренние зависимости workspace | Ключевые внешние зависимости | Поставляемые бинарники / назначение |
|---|---|---|---|
| `ucc-core` | — | `serde`, `serde_json`, `thiserror` | Доменные модели: `UccProfile`, `FanProfile`, `FanControlLogic`, `PowerPolicy`, `CpuSettings`, `ChargingProfile`, `CoolingSettings`, DTO |
| `ucc-config` | `ucc-core` | `serde`, `serde_json`, `thiserror` | `ConfigManager`, атомарное сохранение, восстановление повреждённых файлов, импорт legacy, `platform_paths` |
| `ucc-hardware` | `ucc-core` | `libc`, `nix`, `tokio`, `zbus`, `libloading`, `tracing`, `thiserror` | `PlatformBackend`, `TuxedoIoBackend`, `WindowsWmiEcBackend`, `CpuController`/`CpuTopology`, `FanController`, `CoolerController`/`BluezCoolerClient`, `NvmlWrapper`/`NvidiaPowerGuard`, `PowerDeviceController`, `TelemetryCollector` |
| `ucc-lighting` | `ucc-core`, `ucc-hardware` | `serde`, `serde_json`, `thiserror`, `tracing` | `LightingTransport`, `SysfsLedTransport`, `UniwillHidTransport`, `KeyboardBacklightController`, `LightbarController` |
| `ucc-dbus` | `ucc-core` | `zbus 5.5`, `tokio 1.43`, `serde`, `serde_json`, `thiserror`, `tracing` | `UccdBackend` trait, `UccdInterface` (`com.uniwill.uccd`), `UccdLegacyInterface` (`de.uniwill.uccd`), `PolkitAuthorizer`, `OffloadLane`, `UccdProxy`, `ipc` JSON-RPC |
| `uccd` | `ucc-core`, `ucc-config`, `ucc-hardware`, `ucc-lighting`, `ucc-dbus` | `tokio`, `zbus`, `nix`, `tracing`, `tracing-subscriber`, `thiserror` | Бинарники: `uccd`, `ucc-sleep`, `ucc-hardware`, `ucc-import-check`; модули `DaemonRuntime`, `LiveUccdBackend`, `LidController`/`WakeupPolicy`, `SleepStateStore` |
| `ucc-cli` | `ucc-dbus` | `clap 4.5`, `clap_complete`, `clap_mangen`, `zbus`, `tokio`, `serde_json` | Бинарник `ucc-cli` (16 подкоманд управления и мониторинга) |
| `ucc-desktop` | `ucc-dbus`, `ucc-config` | `ksni 0.3.7`, `zbus`, `nix`, `tokio`, `tracing` | Бинарник `ucc-desktop-tray`: SNI-трей, `launcher` (`XDG_ACTIVATION_TOKEN`), `ProcessTerminator` (`pidfd_open`) |
| `ucc-session-agent` | `ucc-dbus`, `ucc-config` | `zbus`, `tokio`, `nix`, `serde_json`, `tracing` | Бинарник `ucc-session-agent`: `LidDisplayHandler`, `RefreshAdvisor`, `NotificationMonitorParser`, `DriverAlertState`, `PowerSourceAlertState` |
| `xtask` | `ucc-cli`, `ucc-config` | `clap_complete`, `serde`, `serde_json`, `thiserror` | Автоматизация сборки, генерация completions/man, проверка упаковки и maintenance workflows |

---

## 3. Модель конкурентности, воркеры, D-Bus Offload Lanes и Polkit

### 3.1. Акторная и потоковая модель `uccd` (`ADR-02`)

Демон запускает единый многопоточный `tokio` runtime (`#[tokio::main]`), но изолирует весь блокирующий аппаратный I/O (`/dev/tuxedo_io`, `sysfs`, NVML `dlopen`, `ioctl`, внешние утилиты `iw`/`systemctl`) от async-потоков `tokio`:

1. **Async-задачи Tokio** (`crates/uccd/src/orchestrator.rs`):
   - **D-Bus object server**: обслуживает одновременно `/com/uniwill/uccd` и `/de/uniwill/uccd` (оба пути регистрируют интерфейсы `com.uniwill.uccd` и `de.uniwill.uccd`, 7 сигналов `#[zbus(signal)]`).
   - **D-Bus Signal Dispatcher**: читает `broadcast::Receiver<DbusSignalEvent>` и транслирует сигналы на оба пути и оба интерфейса (`ProfileChanged`, `ModeReapplyPendingChanged`, `PowerStateChanged`, `PowerPolicyChanged`, `PowerDeviceStatusChanged`, `LightbarSettingsChanged`, `WaterCoolerStatusChanged`).
   - **Client Eviction (`NameOwnerChanged`)**: отслеживает отключение клиентов (`name.starts_with(':') && name == old_owner && new_owner.is_empty()`), сбрасывает временные power-запросы отключившегося клиента и выносит `reconcile_power_state(false)` в отдельный OS-поток `uccd-disconnect`.
   - **BLE Water Cooler Actor (`run_water_cooler_ble_loop`)**: асинхронный цикл BlueZ GATT с генерационной очередью команд `CoolerCommandQueue`, жёстким интервалом `BLE_WRITE_GAP = 80 ms` и keepalive `10 s`.
   - **Logind Monitor (`run_logind_monitor_loop`)**: мониторинг `org.freedesktop.login1` (`LidClosed`, ингибитор `handle-lid-switch`).

2. **Выделенные периодические OS-воркеры (`spawn_periodic_worker`)**:
   Останавливаются через общую пару `Arc<(Mutex<bool>, Condvar)>` и учитываются счётчиком активных аппаратных операций `enter_worker_hw_op() -> Option<HardwareOpGuard>`, обернуты в `std::panic::catch_unwind`:

| Поток | Период | Операция | Назначение |
|---|---|---|---|
| `uccd-worker-tel` | `800 ms` | `poll_telemetry_once()` | Сбор CPU/iGPU/dGPU/fan телеметрии в snapshot (`Arc<RwLock<...>>`) |
| `uccd-worker-fan` | `1 s` | `tick_fans_once()` | Расчёт кривых `FanControlLogic` и запись скважности в `PlatformBackend` |
| `uccd-worker-pow` | `200 ms` | `reconcile_power_state(false)` | Детект смены источника (`ac`/`type_c`/`bat`) и согласование профиля |
| `uccd-worker-cpu` | `10 s` | `validate_and_reapply_cpu_once()` | Контроль дрейфа governor/EPP/частот (`MAX_REAPPLY_ATTEMPTS = 3`) |
| `uccd-worker-lightbar` | `20 ms` (50 Гц) | `tick_lightbar_animation()` | Кадровая генерация анимаций передней полосы (`LIGHTBAR_FRAME_INTERVAL`) |

### 3.2. D-Bus Offload Lanes (`crates/ucc-dbus/src/interface.rs`)

Чтобы зависание или медленный I/O в одной аппаратной подсистеме (например, sysfs зарядки или NVML) не блокировали Tokio executor и не вызывали голодания независимых подсистем, синхронные вызовы `UccdBackend` исполняются в выделенных OS-потоках `uccd-dbus-io` с `catch_unwind` и изоляцией по семафорам `OffloadLane`:

| Канал (`OffloadLane`) | Категория нагрузки | Ёмкость семафора | Таймаут очереди (`UCC_OFFLOAD_QUEUE_TIMEOUT_MS`) | Защита от отключённых клиентов |
|---|---|---|---|---|
| `Read` | Чтение телеметрии, статусов, настроек | `16` | `500 ms` (мин. `10 ms`) | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `Profile` | Мутации и применение профилей | `4` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `Charging` | Пороги, тип, профиль и приоритет зарядки | `4` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `Lighting` | Клавиатура RGB и Lightbar | `4` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `PowerGpu` | Power policy, PowerDevices, TDP, cTGP, PRIME | `4` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `Cooler` | Управление СЖО LCT (fan/pump/RGB/enable) | `4` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `Display` | Яркость и режимы дисплея | `4` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |
| `Misc` | Прочие управляющие вызовы | `8` | `500 ms` | Проверка `sender_is_alive` если ожидание `> 25 ms` |

Максимальный размер JSON-нагрузки на входе D-Bus ограничен константой `MAX_DBUS_JSON_PAYLOAD_BYTES = 256 * 1024` (256 KiB).

### 3.3. Поток авторизации Polkit (`crates/ucc-dbus/src/auth.rs`)

Каждый D-Bus метод проходит `PolkitAuthorizer::check_auth` до захвата `OffloadLane`:

<!-- DIAGRAM PLACEHOLDER: Polkit system-bus-name authorization flow with per-sender rate limits and fallback -->

```text
D-Bus Caller (:1.xxx)
   |
   +--> 1. SenderAuthGuard::acquire(server_guid:sender, interactive)
   |       - Read: max 8 active / 64 queued per sender (wait timeout 100 ms)
   |       - Control/ManageHardware: max 2 concurrent interactive per sender
   |
   +--> 2. org.freedesktop.DBus.GetConnectionUnixUser(sender)
   |       - UID == 0  ==> ALLOW immediately
   |
   +--> 3. Global Polkit concurrency semaphore
   |       - Read: Semaphore(32), acquire timeout 2 s, RPC timeout 5 s, flags = 0x0
   |       - Control / ManageHardware: Semaphore(4), acquire timeout 5 s, RPC timeout 30 s, flags = 0x1 (AllowUserInteraction)
   |
   +--> 4. org.freedesktop.PolicyKit1.Authority.CheckAuthorization
   |       subject = ("system-bus-name", {"name": sender})  // CVE-2013-4288 race-free
   |       action  = com.uniwill.uccd.{read | control | manage-hardware}
   |       (fallback -> de.uniwill.uccd.* ONLY if primary action is unregistered)
   |
   +--> 5. Policy defaults (uccd/com.uniwill.uccd.policy):
           - com.uniwill.uccd.read:            allow_active=yes,  allow_any=no, allow_inactive=no
           - com.uniwill.uccd.control:         allow_active=yes,  allow_any=no, allow_inactive=no
           - com.uniwill.uccd.manage-hardware: allow_active=auth_admin_keep, allow_any=no, allow_inactive=no
```

Эскалация уровня прав для профилей выполняется динамически функцией `profile_requires_manage_hardware(profile_json)`: если профиль содержит непустой `odmPowerLimits` (кастомные TDP) или `fan.useControl == false`, требуется `AuthAction::ManageHardware` вместо `AuthAction::Control`.

---

## 4. Транзакционный движок профилей (`crates/uccd/src/backend/profile_tx.rs`)

Применение профиля (`apply_profile_txn`) реализовано как трёхфазная транзакция с оптимистическим версионированием (seqlock / epoch) и детерминированным откатом:

### 4.1. Фазы транзакции

| Фаза | Состав операций | Проверка конкурентности / отмены | Поведение при сбое |
|---|---|---|---|
| **Pre-flight** | Проверка `shutting_down`, `shutdown_wait_expired`, `is_read_only_preview()`, `profile.validate()`. Снимок `start_epoch = profile_commit_epoch.load(SeqCst)`. Запись baseline-снимков в `in_flight_*_baselines` под RAII `InFlightTxGuard`. | Если `vendor == Unsupported`, обновляет настройки/память и возвращает `ApplyOutcome::Committed` без аппаратных записей. | Возврат `ApplyOutcome::Failed` до мутации оборудования. |
| **Phase 1: CPU** | Обратимые записи в `/sys/devices/system/cpu`: `online`, `intel_pstate/no_turbo`, `hwp_dynamic_boost`, `scaling_governor`, `energy_performance_preference`, `scaling_min_freq`/`scaling_max_freq`. Сбор журнала `cpu_rollback: Vec<(PathBuf, String)>`. | Проверка `profile_commit_epoch != start_epoch` и `shutdown_wait_expired`. | При ошибке — `rollback_cpu_writes`; при вытеснении новой транзакцией — `reconcile_to_latest_committed`. |
| **Phase 2: Hardware (ODM, TDP, Charging, cTGP, Keyboard)** | Запись ODM perf profile (`resolve_odm_profile_code`, для 2-профильных Uniwill clamp до `<= 1`), запись TDP с проверкой readback (`write_tdp_verified`), порогов/типа/профиля/приоритета зарядки, NVIDIA cTGP offset, подсветки клавиатуры. Перед каждой записью инкрементируются атомарные счётчики `tdp_write_seq`, `odm_write_seq`, `ctgp_write_seq`, `kb_write_seq` (seqlock-снимки unmanaged baseline). | Проверка `profile_commit_epoch != start_epoch` и `shutting_down` после каждого аппаратного шага. | Откат выполненных шагов (`rollback_hardware` / `restore_device_writes` / `restore_ctgp_and_kb_writes`). Если транзакция вытеснена, выполняется `reconcile_to_latest_committed`. |
| **Phase 3: Atomic Commit** | Захват `hw_reconcile_lock`. Финальная проверка `profile_commit_epoch == start_epoch`. Применение вентиляторов и PowerDevices (`apply_fan_and_device_state_locked`), атомарное сохранение настроек/кастомных профилей (`MAX_CUSTOM_PROFILES = 64`, `MAX_PROFILE_JSON_BYTES = 1 MiB`), обновление `CommittedProfileRecord`, инкремент `profile_commit_epoch`, публикация `DbusSignalEvent::ProfileChanged`. | Если за время Phase 1–2 эпоха изменилась — отказ от коммита и передача `SupersededUndo` в `reconcile_to_latest_committed`. | Возврат `ApplyOutcome::SupersededReconciled` или откат с `ApplyOutcome::Failed`. |

### 4.2. Семантика отката, Reconcile и Shutdown Deadline

- **Seqlock / In-flight Baselines**: параллельные транзакции регистрируют исходные значения оборудования в `in_flight_{tdp,odm,ctgp,kb}_baselines` с монотонно растущим `tx_id`. При снятии baseline функция `snapshot_unmanaged_*_baseline` повторяет чтение до 4 раз, проверяя совпадение `*_write_seq` до и после чтения и отсутствие записи от другой активной транзакции.
- **Неполный откат**: если `rollback_hardware` не смог восстановить часть параметров, под `hw_reconcile_lock` вызывается `sync_active_profile_from_hw_readback()`, синхронизирующий `active_profile` с фактически подтверждённым состоянием железа.
- **Ограниченное завершение (`DaemonRuntime::shutdown`)**:
  1. Устанавливается `begin_shutdown()`, останавливаются `worker_stop` (Condvar) и `stop_tx` (watch), прерываются async-задачи Tokio (бюджет `5 ms`).
  2. Регистрируется `ShutdownHandoffGuard` в глобальном реестре `IN_FLIGHT_SHUTDOWNS` до освобождения имени на шине.
  3. Немедленно снимаются D-Bus объекты (`/com/uniwill/uccd`, `/de/uniwill/uccd`) и освобождается `com.uniwill.uccd`.
  4. В отсоединённом OS-потоке `uccd-shutdown` вызываются `wait_in_flight_hw_ops()`, `backend.shutdown()` (возврат вентиляторов в авто-режим, восстановление CPU defaults) и передача невосстановленных CPU defaults преемнику (`ShutdownOutcome::Completed(Option<CpuDefaultsSnapshot>)`).
  5. Основной поток ждёт не дольше `timeout = 5 s` (`uccd/src/main.rs`); при истечении дедлайна выставляется `shutdown_wait_expired = true`, что немедленно прерывает любые оставшиеся циклы записи/отката (`abort_apply_on_shutdown`), а `systemd` дополнительно запускает `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto` (`TimeoutStopSec=10s`).

---

## 5. Конечный автомат сна и пробуждения (`ucc-sleep` & `Type=dbus`)

Интеграция с `systemd` (`uccd-sleep.service` / `uccd-pre-sleep.service`, `RequiredBy=sleep.target`, `Before=sleep.target nvidia-suspend.service nvidia-hibernate.service`) сохраняет волатильное состояние рантайма (`manual_mode`, `cooler_enabled`, `was_active`) в `/run/ucc/sleep-state.json` (версия формата `version: 1`, максимум `4096` байт).

<!-- DIAGRAM PLACEHOLDER: Sleep/resume state machine and Type=dbus handoff timeline -->

```text
[RUNNING uccd]
      |
      | systemd enters sleep.target -> ExecStart=/usr/lib/ucc/ucc-sleep prepare (UID=0, UMask=0077)
      v
1. Read ConfigManager -> powerDevices.lid.wakeOnlyLidAndKeyboard (default: true)
2. WakeupPolicy::apply() -> /sys/devices/.../power/wakeup & /proc/acpi/wakeup
3. systemctl show uccd.service --property=ActiveState (timeout 20 s)
      |-- inactive/failed --> if no prior snapshot: save {was_active: false, manual_mode: "auto", cooler_enabled: false}
      |                       else keep prior snapshot
      +-- active/activating/reloading -->
            4. System D-Bus (timeout 5 s): GetPowerPolicyJSON + IsWaterCoolerEnabled
            5. Atomic save /run/ucc/sleep-state.json (dir 0700 root, file 0600 root, O_NOFOLLOW, rename)
            6. systemctl stop uccd.service (uccd restores fan-auto & disconnects BLE)
      |
      v
[SYSTEM SUSPENDED / HIBERNATED]
      |
      | systemd leaves sleep.target -> ExecStopPost=/usr/lib/ucc/ucc-sleep resume
      v
7. SleepStateStore::load() (verify uid==0, S_IFREG, mode & 0o077 == 0, size <= 4096, version == 1)
      |-- was_active == false --> SleepStateStore::clear() (unlink snapshot)
      +-- was_active == true  --> systemctl start uccd.service (DO NOT unlink snapshot yet!)
                                        |
                                        v
                          [uccd Type=dbus STARTUP HANDOFF]
                          a. set_activation_pending(true)
                          b. Read-only load_sleep_state_read_only() + prime_read_only() (budget 1500 ms / UCC_PRIME_TIMEOUT_MS)
                          c. Export /com/uniwill/uccd & /de/uniwill/uccd
                          d. RequestName("com.uniwill.uccd", DoNotQueue)  <-- systemctl start returns HERE
                          e. wait_for_predecessor_shutdown("com.uniwill.uccd")
                          f. uccd-activate thread: load_sleep_state_read_only() -> activate_hardware()
                             -> clear_consumed_sleep_state() (unlinks /run/ucc/sleep-state.json)
                          g. Spawn periodic workers + BLE + logind loops
```

### Гарантии безопасности и `Type=dbus` handoff

- **Защита от подмены снимка**: `SleepStateStore` открывает файл с `O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK`, проверяет через `fstat` регулярный тип файла, владельца `owner_uid == 0`, отсутствие прав группы/прочих (`mode & 0o077 == 0`) и проверяет директорию `/run/ucc` (`0700`, `uid == 0`, `mode & 0o022 == 0`). Запись выполняется через временный файл `.sleep-state.tmp.<pid>.<ns>` (`create_new(true)`, `sync_all()`, `rename()`).
- **Отложенное удаление снимка**: при `Type=dbus` команда `systemctl start uccd.service` завершается в момент получения `BusName=com.uniwill.uccd` (шаг `d`), до завершения активации оборудования в `uccd-activate` (шаг `f`). Поэтому `ucc-sleep resume` не удаляет файл; его удаляет сам `uccd` методом `clear_consumed_sleep_state()` строго после успешного восстановления состояния оборудования.

---

## 6. Платформенные швы расширения (`ADR-05`, `ADR-06`)

Бизнес-логика `uccd`, `FanController`, `KeyboardBacklightController` и `LightbarController` отделена от конкретных ядерных драйверов и ОС тремя доменными трейтами:

| Шов (trait / модуль) | Текущая реализация (Linux M0–M13) | Целевая реализация Linux `uwcc-rs` (`M14`, `ADR-05`) | Кроссплатформенная реализация Windows (`M15`, `ADR-06`) |
|---|---|---|---|
| `PlatformBackend` (`crates/ucc-hardware/src/platform.rs`) | `TuxedoIoBackend<TuxedoIoDevice>` (`/dev/tuxedo_io` ioctl) + `MockPlatformBackend` | `UwccBackend` (`/dev/uwcc` Rust-for-Linux DKMS: `ECRR`/`ECRW`, `SMRW`, `PWUP`, `RamFan 1.5` `0xF00–0xF5C`, тахометр RPM `0x464/0x465`, `0x46C/0x46B`, нативный `hwmon`; взаимоисключение с `tuxedo-drivers`) | `WindowsWmiEcBackend` поверх `WmiEcTransport` (`root\WMI` `AcpiTest_MULong` через встроенный `wmiacpi.sys`, без подписанного KMDF-драйвера) |
| `LightingTransport` (`crates/ucc-lighting/src/transport.rs`) | `SysfsLedTransport` (`/sys/bus/hid/drivers/ite_8291`, `/sys/class/leds/*`) | `UniwillHidTransport` поверх `HidDeviceTransport` (`/dev/hidraw*` для `048D:600B`, `048D:7001`/`048D:7000`, EC-питание полосы `0x767`/`0x768`) | `UniwillHidTransport` поверх `HidDeviceTransport` (`HidD_SetFeature` / `hidapi`) |
| `ucc-dbus::ipc` + `ucc-config::platform_paths` | System D-Bus (`com.uniwill.uccd`), пути `/etc/ucc`, `~/.config/ucc`, `/run/ucc` | Без изменений (D-Bus API `com.uniwill.uccd` и интеграции Hyprland/Noctalia сохраняются) | JSON-RPC с newline-фреймингом (`encode_frame`/`decode_frame`/`dispatch_json_rpc`) поверх Named Pipe `\\.\pipe\ucc-rusted`; пути `%ProgramData%\ucc-rusted`, `%APPDATA%\ucc-rusted`, `%ProgramData%\ucc-rusted\run` |

### Граница модулей ОС (`#[cfg(unix)]` vs переносимый слой)

- **Только `#[cfg(unix)]`**: `TuxedoIoDevice` (`/dev/tuxedo_io`), `PolkitAuthorizer`, `systemd` unit lifecycle и `ucc-sleep`, sysfs `cpufreq`/`aspm`/`power/wakeup`, BlueZ D-Bus GATT, `ksni` SNI-трей, `pidfd_open`, `hyprctl`/`kscreen-doctor`.
- **Кроссплатформенный слой**: `ucc-core` (профили, валидация, кривые `FanControlLogic`), `ucc-config` (атомарное сохранение и `platform_paths`), `PlatformBackend`, `WindowsWmiEcBackend`, `NvmlWrapper` (`libnvidia-ml.so.1` на Unix / `nvml.dll` на Windows), `LightingTransport` / `UniwillHidTransport`, `ucc-dbus::ipc`.
