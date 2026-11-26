# Тотальное ревью проекта UCC-Revorked: аудит коммитов, архитектура и полный Issue List

**Дата аудита:** 2026-11-26  
**Ветка / базовый коммит:** `rust/migration` @ `e16f9ea` (грязное рабочее дерево — см. раздел 1)  
**Объём ревью:** все 9 Rust-крейтов (`crates/*`), C++/Qt6 клиенты и сервисы (`ucc-gui/`, `libucc-dbus/`, `ucc-session/`, `ucc-desktop/`, `ucc-tray/`, `uccd/`), GNOME-расширение (`ucc-gnome/`), сборка и упаковка (`CMakeLists.txt`, `Makefile`, `debian/*`, `ucc.spec`, `PKGBUILD`, `package.nix`, `flake.nix`, `nix/`, `.github/workflows/*`), systemd/D-Bus/Polkit, Git-гигиена и документация миграции (`docs/rust/execution/*`).

---

## 1. Честный статус Git-коммитов и гигиены репозитория

> [!IMPORTANT]
> **Нет, после коммита `e16f9ea` изменения в ветке `rust/migration` не коммитились.**  
> Вся работа волн `M5`–`M8`, `Wave 1`, закрытие `UCC-01..22`, `RECHECK-01..09`, `FOLLOWUP-01..07`, 5 недавних `m8_outcomes_recheck`-фиксов, удаление `crates/ucc-gui`, а также доработки C++ клиентов (`ucc-gui`, `ucc-session`, `uccd`, `tests`) и `docs/rust/execution/*` находятся **в незакоммиченном рабочем дереве**.

### Фактическое состояние `git status` на момент ревью
- **HEAD:** `e16f9ea` (`fix(review): resolve findings from recheck-e1d5d0a audit`) — опережает `origin/rust/migration` (`dab6b37`) на 3 коммита.
- **Изменённые отслеживаемые файлы (`M` / `D`):** 53+ файлов (включая `D crates/ucc-gui/*` из-за перехода на C++ Qt6 по `ADR-01`).
- **Неотслеживаемые файлы и каталоги (`??`):** 29 позиций, причём среди них **критические для сборки и тестов файлы** (`.cargo/config.toml`, `stand-fixtures/*.c`, `extra-fixtures/*.c`, `crates/ucc-lighting/`, `crates/uccd/src/{backend,lid,orchestrator,lib}.rs`, `crates/ucc-hardware/src/{bluez_cooler,fan_controller,platform,telemetry/*}.rs`, `docs/rust/execution/decisions/ADR-05-uwcc-platform-backend.md`, `tests/test_lid_wakeup.cpp`, `ucc-session/LidDisplayHandler.hpp`, `uccd/inc/{LidController,WakeupPolicy}.hpp`).
- **Загрязнения в репозитории и корне:**
  - В `nvidia_findings/` лежат **10 скомпилированных x86_64 ELF-бинарников** с debug info, отслеживаемых Git.
  - В корне оставлены временные каталоги/файлы: `audit-cpp-build/`, `consolidated-gui-xdg/`, `extended-nvml/`, `fake-nvml/`, `followup-fixtures/`, `rust-dbus-alias.xml`, `rust-dbus-primary.xml`, а также **3 устаревших worktree** в `.worktrees/{r2,w1,w2}` от 7 ноября (`23a8499`, `ab40f84`, `7ece3f8`).
  - В `.gitignore:39-40` добавлены глобальные шаблоны `*.so` / `*.so.*`, которые маскируют скомпилированные тестовые библиотеки в корне.

### Рекомендуемая нарезка на атомарные коммиты (перед тем как идти дальше)
Чтобы вернуть историю Git в управляемое состояние, накопленный working tree стоит разбить на 8 логических коммитов:
1. `chore(git): untrack ELF binaries in nvidia_findings, add .cargo/config.toml & tracked test fixtures, clean scratch dirs`
2. `feat(ucc-core,ucc-config): cooling/power/cpu domain types, C++ compatibility adapters, and atomic backup/recovery`
3. `feat(ucc-hardware): PlatformBackend, FanController, BluezCoolerClient, NVML/NvAPI, RAPL/GPU telemetry, PowerDeviceController`
4. `feat(ucc-lighting): keyboard RGB & lightbar sysfs controllers and animation engine`
5. `feat(ucc-dbus): zbus service interface (123 methods, 7 signals), legacy alias, and Polkit authorizer`
6. `feat(uccd): LiveUccdBackend, DaemonRuntime Type=dbus handoff, transaction engine, and regression tests`
7. `feat(ucc-cli,xtask,ucc-desktop): Rust CLI, completions/manpage xtask, and tray crate scaffold`
8. `refactor(cpp,docs): remove Iced crates/ucc-gui per ADR-01, sync C++ GUI/session parity, update execution ledgers`

---

## 2. Архитектурный разбор: «Монолит vs Микросервисы vs Целевая архитектура»

### 2.1. Диагноз текущего состояния
По внешним процессам проект **уже разделён** (клиент-сервер на D-Bus: `uccd`, `ucc-gui`, `ucc-cli`, `ucc-desktop-tray`, `ucc-session-agent`, `ucc-gnome`, `ucc-tray`, `ucc-sleep`, `ucc-hardware`), а Cargo workspace состоит из **9 отдельных крейтов**.

Однако внутри процесса `uccd` образовался **двойной перекос**:
1. **God-Object монолит внутри `crates/uccd/src/backend.rs` (~5850 строк):**
   - Структура `LiveUccdBackend` содержит >35 полей/мьютексов/атомиков и ~90 вызовов `.unwrap()`, совмещая в одном типе: транзакции профилей, прямые sysfs-записи CPU, управление вентиляторами, подсветкой, бэклайтом, зарядкой, вебкамерой/Fn-Lock, парсинг `/proc/cpuinfo` и `pci.ids`, обнаружение GPU, PowerPolicy, PowerDeviceController, телеметрию и форматирование 123 D-Bus JSON DTO.
2. **Дублирование модулей с «мёртвым кодом» в библиотеках:**
   - В `crates/ucc-hardware` и `crates/ucc-lighting` и `crates/uccd/src/lid.rs` написаны чистые доменные компоненты (`CpuController`, `CpuWorker`, `BluezCoolerClient`, `spawn_fan_worker`, `LidController`, `WakeupPolicy`, `apply_profile_keyboard_json`), **но `backend.rs` их не использует**, дублируя упрощённые (и багованные) копии внутри себя.

### 2.2. Почему «классические микросервисы» для `uccd` — плохая идея
Разбивать сам `uccd` на 6–8 отдельных системных root-демонов (отдельный демон TDP, отдельный демон CPU, отдельный демон вентиляторов, отдельный демон подсветки, отдельный демон телеметрии, отдельный демон конфига) **технически вредно** для системного софта уровня железа Linux:

1. **Единое железо и кросс-доменные аппаратные транзакции.**  
   На ноутбуках Uniwill/Clevo `/dev/tuxedo_io` (и будущий `/dev/uwcc`), EC-регистры и ODM-режимы связаны: переключение `ODM PerfProfile` в ядре/EC сбрасывает лимиты PL1/PL2/PL4, а смена профиля питания должна **атомарно** применить `CPU -> ODM -> Pass-2 TDP -> яркость/вебкамеру -> диск -> вентиляторы/СЖО` и при сбое на одном канале **откатить всё назад** (см. уже реализованную фазовую транзакцию в `backend.rs`). Между 5 демонами это превратится в распределённый 2PC поверх D-Bus с новыми гонками.
2. **Взаимные блокировки и накладные расходы.**  
   Каждый лишний root-демон — это дополнительный цикл опроса, лишняя память, лишние wakeups CPU, лишние Polkit/D-Bus round-trip'ы и риск рассинхронизации состояний при краше одного из них или при suspend/resume.
3. **Отраслевой стандарт Linux:**  
   `systemd`, `UPower`, `power-profiles-daemon`, `fwupd`, `asusctl/asusd` строятся **ровно не на веб-микросервисах**, а по другой модели.

### 2.3. Правильная целевая архитектура: **Privilege-Separated Multi-Process + Modular Actor/Domain Core**
Это называется **многопроцессная архитектура с разделением привилегий и доменов ответственности (Privilege-Separated / Failure-Isolated Multi-Process Architecture)** на уровне ОС + **модульный монолит с чёткими доменными сервисами (Modular Domain Core)** внутри `uccd`.

**Принцип разделения на процессы — НЕ «по таблицам/фичам», а по границе привилегий, пользовательской сессии и изоляции сбоев:**

```text
╔═══════════════════════════════════════════════════════════════════════════╗
║                        USER SESSION (без root)                            ║
║                                                                           ║
║  ┌──────────────────┐   ┌──────────────────┐   ┌───────────────────────┐  ║
║  │ ucc-gui (Qt6 C++)│   │ ucc-cli (Rust)   │   │ ucc-gnome (GJS ext)   │  ║
║  │ on-demand, 0 MB  │   │ on-demand        │   │ опциональный плагин   │  ║
║  └────────┬─────────┘   └────────┬─────────┘   └──────────┬────────────┘  ║
║           └──────────────────────┴────────────────────────┘               ║
║                                  │ System D-Bus + Polkit (system-bus-name)║
║  ┌───────────────────────────────┴─────────────────────────────────────┐  ║
║  │ ФОНОВЫЕ СЕРВИСЫ СЕССИИ (лёгкие, Rust, без Qt в фоне)                │  ║
║  │ • ucc-session-agent: Wayland/Hyprland/KWin Hz, DPMS крышки, notify  │  ║
║  │ • ucc-desktop-tray:  SNI трей (ksni), батарея, GPU-процессы (pidfd) │  ║
║  └───────────────────────────────┬─────────────────────────────────────┘  ║
╚══════════════════════════════════╪════════════════════════════════════════╝
                                   │ D-Bus (com.uniwill.uccd)
╔══════════════════════════════════▼════════════════════════════════════════╗
║                        SYSTEM / ROOT DOMAIN                               ║
║                                                                           ║
║  ┌─────────────────────────────────────────────────────────────────────┐  ║
║  │ uccd (единый системный арбитр железа, Rust)                         │  ║
║  │  ├── dbus_adapter     (тонкий zbus-адаптер + bounded semaphore/pool)│  ║
║  │  ├── profile_tx       (фазовый транзакционный движок + откат)       │  ║
║  │  ├── cpu_service      (делегирует в ucc-hardware::CpuController)    │  ║
║  │  ├── cooling_service  (FanController + BLE BluezCoolerClient)       │  ║
║  │  ├── power_service    (PowerPolicy + PowerDevices + Lid/logind + EC)│  ║
║  │  ├── display_gpu_svc  (DisplayBacklight + DRM + NVML/cTGP + SysInfo)│  ║
║  │  ├── lighting_service (ucc-lighting: RGB клавиатура + Lightbar)     │  ║
║  │  └── config_service   (ConfigManager + SleepStateStore)             │  ║
║  └─────────┬───────────────────────────────────┬───────────────────────┘  ║
║            │                                   │                          ║
║  ┌─────────▼──────────┐              ┌─────────▼──────────┐               ║
║  │ uccd-sleep.service │              │ ExecStopPost       │               ║
║  │ ucc-sleep (oneshot)│              │ ucc-hardware       │               ║
║  │ prepare / resume   │              │ --fan-auto / --audit               ║
║  └────────────────────┘              └────────────────────┘               ║
║  (Опционально в будущем: nvml-probe / ble-bridge sidecar, если C FFI      ║
║   libnvidia-ml или BlueZ потребуют жёсткой process-level crash-изоляции)  ║
╚═══════════════════════════════════════════════════════════════════════════╝
```

**Главный практический шаг прямо сейчас:** не дробить `uccd` на 7 D-Bus демонов, а:
1. Декомпозировать `crates/uccd/src/backend.rs` (~5850 строк) на 6–8 узких доменных модулей (`<= 500–600` строк каждый) + удалить ~90 паникующих `.unwrap()`.
2. Удалить дубликаты в `backend.rs` и **реально подключить** уже написанные `CpuController`/`CpuWorker`, `BluezCoolerClient`, `LidController`/`WakeupPolicy`/`logind`, `apply_profile_keyboard_json` и `DisplayBacklight`.
3. Допортировать в Rust недостающие лёгкие процессы сессии и oneshot-хелперы: `ucc-desktop-tray`, `ucc-session-agent`, `ucc-sleep`, `ucc-hardware`, `ucc-import-check`.

---

## 3. Сводная таблица проблем (41 группа)

| Критичность | Количество | Диапазон ID | Основные области |
|---|---:|---|---|
| **P0 (Блокер / Безопасность / Повреждение железа или конфига)** | **15** | `ISSUE-01` .. `ISSUE-15` | Инверсия помпы СЖО (выключение вместо 11V), мёртвые BLE/Lid/CPU/Sleep-компоненты, потеря полей зарядки C++ settings, `temp<=0` вентиляторов, fallback root-демона на session bus / `~/.config` / `/tmp` PID / спавн потоков, `ExecStopPost` личный DMI, сборка/пакеты без Rust, нерабочие клавиши RGB, непортированный desktop/session слой, сломанный чистый `cargo test` |
| **P1 (Серьёзный баг / Безопасность / Потеря паритета)** | **18** | `ISSUE-16` .. `ISSUE-33` | `0777` симлинк в `write_atomic`, Polkit `unix-process`/read-шторм/несовпадение action ID, D-Bus `.conf` блокирует `de.uniwill.uccd`, `uccd.service` hardening, 30+ sync RPC на Tokio + `.unwrap()` poison-паники, Uniwill TDP/D3cold/AMD/Intel-i915-RAPL/PWM-backlight/Hz, ручной режим СЖО/пресеты вентиляторов/зарядка/passive mode, баги `ucc-cli` и C++ `UccdClient`, нарушения `ADR-01` в CMake, `ucc-gnome`/`ucc-tray`/`ucc-desktop-tray`, сломанные `.deb`/`.rpm`/Nix/`xtask`, рассинхрон `docs/rust/execution/*` |
| **P2 (Средний дефект / Edge-case)** | **7** | `ISSUE-34` .. `ISSUE-40` | Пропуски D-Bus сигналов, 10s авто-стоп телеметрии, рост кастомных профилей и баги CRUD, `PowerDeviceController` PID-reuse kill & `$PATH`, `NVML` null-handle, `TuxedoIoBackend` обход `Sysfs`, баги `LightbarController`/Bluetooth NUS/`AboutPage`/`PumpCurve`/`ucc-session`/`ucc-cli`/toolchain/git-хлам |
| **P3 (Минорный дефект / Чистка)** | **1** | `ISSUE-41` | Косметические/edge-case дефекты JSON-ответов при отказе Polkit, обогащение `GetSettingsJSON`, порядок LED lightbar, алиасы `get_profile`, `.tmp_restore.`, Wayland alpha на `FluentSidebar`, неиспользуемые зависимости в `Cargo.toml` |

---

## 4. Полный дедуплицированный Issue List

### Критические блокеры (`P0`)

#### `ISSUE-01` — Инверсия кодов напряжения помпы СЖО (`11V` выключает помпу!) и несовпадение типов в `FanControlTab`
- **Где:** `crates/uccd/src/backend.rs:5714-5722, 5758-5774`, `crates/ucc-dbus/src/interface.rs:1319`, `crates/ucc-core/src/fan.rs:120-154`, `uccd/src/UccDBusService.cpp:1951-1952, 2005-2023`, `ucc-gui/src/FanControlTab.cpp:89-94, 218-221`
- **Доказательство:** По контракту C++ `UccDBusService.cpp`, `PumpVoltage::from_discrete_code` (`fan.rs:137-146`), `ucc-gui`, `ucc-tray` и `ucc-gnome`, коды D-Bus следующие: `0 = 11V (High)`, `1 = 12V (запрещён)`, `2 = 7V`, `3 = 8V`, `4 = Off`, `-1 = Unset`. Вместо этого `backend.rs:5762-5768` маппит `0 => Off`, `1|7 => V7`, `2|8 => V8`, `3|11 => V11`, `_ => false`. Запрос 11V (`0`) **выключает помпу**, запрос `Off` (`4`) падает, запрещённый `1` принимается! Вдобавок `get_water_cooler_pump_level` (`backend.rs:5714-5722`) отдаёт индексы `0, 1, 2, 3` (`Off => 0`, из-за чего клиенты показывают `"High" 11V` на выключенной помпе), а `FanControlTab.cpp:218-221` кладёт в комбобокс `QVariant::fromValue(PumpVoltage::...)` вместо `int`, из-за чего `findData(int)` всегда возвращает `-1`.
- **Критерий исправления:** В `set_water_cooler_pump_voltage` использовать строго `PumpVoltage::from_discrete_code(voltage)` (разрешены `{0, 2, 3, 4}`, отказ на `1`); в `get_water_cooler_pump_level` возвращать `-1` для `Unset`/отключения и `0/2/3/4` для активных уровней (`-1` и при отказе Polkit); в `FanControlTab.cpp:218-221` хранить `static_cast<int>(...)`.

#### `ISSUE-02` — `BluezCoolerClient` полностью не подключён в `uccd` (СЖО работает только в памяти)
- **Где:** `crates/ucc-hardware/src/bluez_cooler.rs:1-462`, `crates/uccd/src/backend.rs:350-353, 524, 545-548, 2083-2092, 3222-3275`, `crates/uccd/src/orchestrator.rs:547-585`, `uccd/src/workers/LCTWaterCoolerWorker.cpp:58-300`
- **Доказательство:** `BluezCoolerClient` / `BluezGattSession` нигде не импортируются в `crates/uccd/`. Поле `water_cooler_connected` создаётся как `false` и в продакшене не меняется; команды из `CoolerController` складываются в `VecDeque` и никуда не отправляются. Нет загрузки `/etc/ucc/aquaris.json`, нет асимметричного гистерезиса 3°C и градиентной RGB-подсветки от температуры, нет дебаунса подключения (3с) и отключения (10с) для переключения профиля `power_wc`.
- **Критерий исправления:** Запустить фоновую BLE-задачу в `DaemonRuntime` поверх `BluezCoolerClient` (когда СЖО поддерживается и включена), перенести `/etc/ucc/aquaris.json`, связать `tick_fans_once()` с `cooler_controller.apply_profile_temperature()`, сбрасывать очередь GATT на устройство, реализовать дебаунс 3с/10с + сигнал `WaterCoolerStatusChanged` + `reconcile_power_state`.

#### `ISSUE-03` — `LidController`, `WakeupPolicy` (плюс баг путей) и `systemd-logind` полностью не подключены
- **Где:** `crates/uccd/src/lid.rs:1-319`, `crates/uccd/src/backend.rs:359, 553, 626, 4286-4325`, `crates/uccd/src/orchestrator.rs:376-595`, `crates/ucc-hardware/src/devices/whitelist.rs:182-268`, `uccd/inc/LidController.hpp:49-235`
- **Доказательство:** `LiveUccdBackend::lid_controller` создаётся с `ignore_lid_on_ac: false` и ни разу не вызывается. `uccd` не берёт `Inhibit("handle-lid-switch", ..., "block")` у `login1`, не слушает `LidClosed`/`PrepareForSleep`/`UPower.LidIsClosed`, поддерживает только булев `ignore_lid_on_ac` вместо per-source (`ac`, `type_c`, `bat`) действий `"screen_off"|"ignore"|"suspend"|"hibernate"|"poweroff"|"lock"`, а `whitelist.rs` молча игнорирует секцию `"lid"` при `SetPowerDeviceSettingsJSON`. Отдельный баг в `WakeupPolicy::with_sys_proc` (`lid.rs:132-138, 155, 178-179`): при инъекции путей `apply()` повторно дописывает `.join("sys")` и `.join("proc")` (получая `/sys/sys`, `/proc/proc`), а `proc/acpi/wakeup` ищется внутри `self.sys_root`.
- **Критерий исправления:** Реализовать полную per-source политику крышки и валидацию `"lid"` в `whitelist.rs`, починить пути в `WakeupPolicy::with_sys_proc`, применять `wakeOnlyLidAndKeyboard` на старте и в `SetPowerDeviceSettingsJSON`, подключить `login1` ингибитор и слушатели `LidClosed`/`LidIsClosed`/`PrepareForSleep` в `DaemonRuntime`.

#### `ISSUE-04` — Отсутствуют Rust-бинарники `ucc-hardware`, `ucc-sleep`, `ucc-import-check`, нет `sleep-state.json`, гонка `Type=dbus` при выходе из сна
- **Где:** `Cargo.toml:2-12`, `uccd/CMakeLists.txt:7-40`, `uccd/src/{hardware-helper,sleep-helper,import-check}.cpp`, `uccd/inc/SleepState.hpp`, `crates/uccd/src/orchestrator.rs:436-482`
- **Доказательство:** Workspace собирает только `uccd` и `ucc-cli`. В `crates/` нет `/run/ucc/sleep-state.json` — при выходе из сна теряются `manualMode` и `coolerEnabled` (и пропуск welcome-анимации). Более того, `DaemonRuntime::start_on_connection` (`orchestrator.rs:437, 472`) захватывает `BusName=com.uniwill.uccd` **раньше** `activate_hardware()`: для systemd `Type=dbus` демон считается стартовавшим сразу на строке `437`, поэтому `ucc-sleep resume` удаляет `sleep-state.json` ещё до инициализации демона. Также в `uccd/uccd-sleep.service:4-5` отсутствует `After=dbus.service`.
- **Критерий исправления:** Реализовать Rust-бинарники `ucc-hardware`, `ucc-sleep` (`0600` `/run/ucc/sleep-state.json`), `ucc-import-check`; загружать `sleep-state.json` в `uccd` **до** захвата `BusName`; добавить `After=dbus.service` в `uccd-sleep.service`.

#### `ISSUE-05` — `uccd.service` `ExecStopPost` без `-` и с захардкоженным DMI одного личного ноутбука (`exit 3` везде)
- **Где:** `uccd/uccd.service:14-15`, `uccd/src/hardware-helper.cpp:12-23`, `docs/rust/execution/backlog.json:1966` (`M8.5`)
- **Доказательство:** `hardware-helper.cpp:15-16` возвращает `exit 3` для любого устройства, кроме точного сочетания `MECHREVO / YAOSHI Series-X6AR55xY / Standard / 0001`, а `uccd.service:14` вызывает `ExecStopPost=/usr/lib/ucc/ucc-hardware --fan-auto` без префикса `-`. На всех остальных ноутбуках (включая TUXEDO/XMG/Clevo) при падении `uccd` вентиляторы **не сбрасываются в `auto`** (риск перегрева), а на каждом штатном `systemctl stop uccd` юнит переходит в состояние **`failed`**! В `backlog.json:1966` ошибочно закреплено требование перенести этот DMI gate в Rust.
- **Критерий исправления:** Убрать DMI gate из `--fan-auto` (сбрасывать `setFansAuto()` на любом поддерживаемом `/dev/tuxedo_io`), в `uccd.service:14` использовать `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto`, скорректировать требование `M8.5`.

#### `ISSUE-06` — Сборка, `.deb`/`.rpm`/Arch/Nix-пакеты и GitHub CI собирают 100% старый C++ и 0% Rust
- **Где:** `CMakeLists.txt:5-9, 136-168`, `debian/{control,rules,ucc.install}`, `ucc.spec`, `PKGBUILD`, `package.nix`, `flake.nix`, `default.nix`, `.github/workflows/{build,nix,release}.yml`
- **Доказательство:** Ни один сборочный скрипт, пакетировщик или CI workflow не вызывает `cargo build` / `cargo test` / `cargo clippy` и не указывает `rust`/`cargo` в зависимостях сборки. Команды `cmake --build`, `dpkg-buildpackage`, `rpmbuild`, `makepkg`, `nix build` и GitHub Release собирают и ставят legacy C++ `uccd`, `ucc-cli` и даже `ucc-tray` (отменённый по `ADR-03`), полностью игнорируя `crates/`.
- **Критерий исправления:** Встроить сборку и тестирование Rust (`uccd`, `ucc-cli`, completions/manpages из `xtask`, будущие Rust-хелперы и сервисы сессии) + C++ `KF6`-free `ucc-gui` во все пакеты и CI, удалив установку legacy C++ `uccd`/`libucc-dbus.so`/`ucc-tray`.

#### `ISSUE-07` — Мёртвые `CpuController`/`CpuWorker` в `ucc-hardware` и регрессивный дубль управления CPU в `uccd`
- **Где:** `crates/ucc-hardware/src/cpu/{controller.rs,worker.rs}`, `crates/uccd/src/backend.rs:1585-1750, 5640-5642`, `crates/ucc-core/src/{cpu.rs:114-125, dto.rs:206-207}`, `crates/uccd/src/orchestrator.rs:547-585`
- **Доказательство:** `ucc-hardware::cpu::{CpuController, CpuWorker}` не используются. Вместо них `backend.rs:1585-1750` содержит упрощённый прямой sysfs-писатель со следующими регрессиями против C++ и `CpuWorker`:
  1. Пишет `max_freq_khz` до `min_freq_khz` без предварительного расширения границ (`EINVAL` если `new_max < old_min` -> откат всего профиля!), без по-ядерного клампинга P/E и `base_frequency`, и без снаппинга к `scaling_available_frequencies`.
  2. На `None` не сбрасывает лимиты к аппаратным `cpuinfo_min/max_freq`, оставляя старый троттлинг навсегда.
  3. Стирает C++ сентинелы `-1` и `-2` (`cpu_sentinel_serde` превращает всё `< 0` в `None`).
  4. Падает с откатом профиля при `EBUSY` на `intel_pstate` EPP (`governor == "performance"`).
  5. Не поддерживает AMD `/sys/devices/system/cpu/cpufreq/boost`.
  6. Не имеет 10-секундного воркера `CpuWorker` (до 3 попыток переприменения при внешнем дрейфе).
  7. `GetCpuCoreCount` (`backend.rs:5640`) возвращает число **онлайн** ядер вместо **present** (ломается после выключения части ядер).
- **Критерий исправления:** Удалить дублирующий код из `backend.rs`, подключить `ucc-hardware::cpu::{CpuController, CpuWorker}`, сохранить `-1`/`-2` сентинелы, поддержку AMD `boost`, `EBUSY` EPP, по-ядерный клампинг и подсчёт ядер по `/sys/devices/system/cpu/present`.

#### `ISSUE-08` — Загрузка/сохранение C++ `/etc/ucc/settings` уничтожает все 5 полей зарядки, `nvidiaCTGPOffset`, `keyboard` и отбрасывает kernel `charge_types`
- **Где:** `crates/ucc-config/src/settings.rs:140, 214`, `crates/ucc-core/src/{profile.rs:206-324, charging.rs:5-30, dto.rs:64-85,279-283}` vs `uccd/inc/ProfileManager.hpp:301-329, 704-742`, `crates/uccd/src/backend.rs:2208-2950, 3374-3385`
- **Доказательство:**
  1. C++ хранит в каждом профиле плоские ключи `"chargingProfile"`, `"chargingPriority"`, `"chargeType"`, `"chargeStartThreshold"`, `"chargeEndThreshold"`. Rust `UccSettingsRaw.profiles` десериализует `UccProfile` напрямую (ожидая вложенный объект `"charging": { startThreshold, endThreshold, ... }` и без `#[serde(flatten)] pub extra`). При открытии и пересохранении C++ `/etc/ucc/settings` все 5 настроек зарядки **бесследно стираются**!
  2. `ChargeType` разрешает только `Standard | Custom`, отклоняя/теряя валидные sysfs-значения ядра (`Trickle`, `Fast`, `Adaptive`, `LongLife`, `Bypass`).
  3. Секция `keyboard` профиля теряет поля (например `brightness`) из-за строгой `ProfileKeyboard` без `flatten`.
  4. `LegacyProfileDto` и `UccProfile` вообще не имеют поля `nvidiaCTGPOffset`: оно теряется при импорте/сохранении и никогда не применяется в `apply_profile_internal`.
- **Критерий исправления:** Обеспечить 100% двустороннюю совместимость `UccProfile` с плоскими ключами C++ (`LegacyProfileDto` / `#[serde(flatten)]` extra), расширить `ChargeType`, поддержать и применять `nvidiaCTGPOffset` и настройки зарядки при активации профиля/старте.

#### `ISSUE-09` — Uniwill `read_fan_temp` не проверяет `temp <= 0`, сбрасывая вентилятор в `0%` при отсутствии датчика
- **Где:** `crates/ucc-hardware/src/{platform.rs:275-296, ioctl.rs:680-689, fan_controller.rs:173-179}` vs `uccd/3rdparty/tuxedo_io_lib/targets/uniwill.h:171-175`
- **Доказательство:** На Clevo `platform.rs` отсекает `temp <= 1`, а на Uniwill пропускает `0` (`Ok(0)`). В `FanController` значение `0` попадает в `(0..=150)`, поэтому неисправный/неподключённый сенсор Uniwill (`0°C`) трактуется как ледяной процессор и снижает PWM до минимума/`0%` вместо `PlatformError::SensorUnavailable` и аварийного `set_fans_auto()`.
- **Критерий исправления:** Возвращать `Err(PlatformError::SensorUnavailable)` при `temp <= 0` в Uniwill `read_fan_temp` (`uniwill.h:172-175`).

#### `ISSUE-10` — Root-демон падает на Session D-Bus (спуфинг Polkit), кладёт PID в `/tmp` и может убить `ucc-gui`/`ucc-cli`
- **Где:** `crates/uccd/src/main.rs:23-34, 42-64, 154-161, 275-287`, `crates/ucc-dbus/src/auth.rs:134-140`
- **Доказательство:**
  1. При ошибке подключения к system bus `main.rs:278-286` молча переключается на session D-Bus, продолжая работать с root/аппаратными правами; на session bus любой пользовательский процесс может зарегистрировать `org.freedesktop.PolicyKit1` и выдать себе `ManageHardware`.
  2. При недоступности `/run` PID-файл создаётся в `/tmp/uccd.pid`: при `fs.protected_regular >= 1` локальный пользователь может заблокировать старт/остановку демона (`EACCES`).
  3. `stop_daemon` (`main.rs:157`) проверяет `comm.starts_with("ucc")`, поэтому при совпадении/подмене PID `uccd --stop` способен послать `SIGTERM`/`SIGKILL` процессам `ucc-gui` или `ucc-cli`.
- **Критерий исправления:** Запретить fallback на session bus без явного тестового флага/env (с отказом под root в обычном режиме); использовать `$XDG_RUNTIME_DIR/uccd.pid` вместо `/tmp/uccd.pid` + проверку `st_uid == geteuid()` и точное совпадение `comm == "uccd"`.

#### `ISSUE-11` — Root `uccd` читает пользовательский `~/.config/ucc/settings` (`resolve_read_path`)
- **Где:** `crates/ucc-config/src/manager.rs:102-127` vs `134-150`, `crates/uccd/src/backend.rs:461`
- **Доказательство:** `resolve_write_path()` проверяет `geteuid() == 0` и всегда пишет в `/etc/ucc/settings`, а `resolve_read_path()` **не проверяет `geteuid() == 0`** и ставит `user_config_path()` (`$XDG_CONFIG_HOME` / `$HOME/.config/ucc/settings`) выше `/etc/ucc/settings`! При запуске `uccd` от `root` с сохранённым `HOME` (например `sudo -E uccd --start`), демон загружает пользовательский конфиг (TDP, PCI/USB sysfs, power devices) от имени `root` и при сохранении затирает `/etc/ucc/settings`.
- **Критерий исправления:** В `resolve_read_path()` при `geteuid() == 0` (и отсутствии `custom_path`) полностью игнорировать `user_config_path()` и читать только системный конфиг с проверкой владельца/прав.

#### `ISSUE-12` — Неограниченный спавн OS-потоков на каждый D-Bus RPC (`DoS` со стороны активной сессии)
- **Где:** `crates/ucc-dbus/src/interface.rs:39-45, 75-81, 117-123`
- **Доказательство:** Каждый мутирующий D-Bus вызов (`AuthAction::Control` не требует пароля в активной сессии) через `offload_or` / `offload_result` / `offload_fdo_result` вызывает `std::thread::Builder::new().spawn(...)` без очереди, пула или семафора. Активный клиент может заспамить сотни/тысячи вызовов (`SetDisplayBrightness`, `ScanPowerDevices`, `TriggerLightbarAnimation`), исчерпав потоки (`EAGAIN`) или `TasksMax` у root-демона.
- **Критерий исправления:** Ограничить параллелизм `offload_*` через bounded `tokio::sync::Semaphore` и/или фиксированный worker pool.

#### `ISSUE-13` — Управление RGB-клавиатурой сломано на всех клиентах (`ucc-cli`, `ucc-gui`, `ucc-tray`, `ucc-gnome`)
- **Где:** `crates/uccd/src/backend.rs:5031-5056, 2253-3107`, `crates/ucc-lighting/src/keyboard.rs:243-252`, `crates/ucc-cli/src/commands/keyboard.rs:50-68, 125-148, 177-191`, `ucc-gui/src/KeyboardTab.cpp:393-403`, `ucc-tray/src/TrayBackend.cpp:307-327`, `ucc-gnome/extension.js:696-706`
- **Доказательство:**
  1. В `crates/ucc-lighting/src/keyboard.rs:243` уже написана `apply_profile_keyboard_json`, но `LiveUccdBackend::set_keyboard_backlight_states_json` (`backend.rs:5035`) игнорирует её и парсит только плоский `Vec<KeyboardBacklightState>`. Все клиенты с объектным payload (`{"keyboardProfileId", "states"}` в `ucc-gui`/`ucc-tray`, `{"brightness", "states"}` в `ucc-cli keyboard color` / `ucc-gnome`) получают `false`!
  2. `ucc-cli keyboard get` ждёт объект `{brightness, states}`, а `GetKeyboardBacklightStatesJSON` отдаёт массив `[...]` -> всегда выводит `Zones: 0`.
  3. `ucc-cli keyboard activate` и `TrayBackend.cpp:314` ждут `"json"` строкой (`as_str()`/`toString()`), а `GetCustomKeyboardProfilesJSON` отдаёт вложенный JSON Value -> всегда ошибка!
  4. `apply_profile_internal` при смене системного профиля **вообще не применяет** привязанный профиль подсветки клавиатуры, а `set_keyboard_backlight_states_json` пишет на диск до успешной записи в sysfs и не эмитит `ProfileChanged`.
- **Критерий исправления:** Поддержать оба формата (массив и объект с `keyboardProfileId`) в `backend.rs`, исправить `ucc-cli`/`TrayBackend`, применять `selectedKeyboardProfile` в `apply_profile_internal`, синхронизировать `ProfileChanged` и засеять дефолтный 126-клавишный шаблон.

#### `ISSUE-14` — `crates/ucc-desktop` является библиотечной заглушкой; отсутствует Rust-реализация `ucc-desktop-tray` и всего `ucc-session-agent`
- **Где:** `crates/ucc-desktop/Cargo.toml:1-18`, `crates/ucc-desktop/src/tray.rs:1-165` vs C++ `ucc-desktop/*` (`DesktopTray`, `SessionTelemetry`, `ProcessTerminator`, `SessionProcessesWidget`) и весь `ucc-session/*` (`NotificationMonitor`, `DriverAlert`, `PowerSourceAlert`, `RefreshAdvisor`, `LidDisplayHandler`)
- **Доказательство:** В `crates/ucc-desktop` нет `src/main.rs`/`[[bin]]`, ни один крейт на него не ссылается, `spawn_tray()` не вызывается и не подключён к D-Bus `uccd`, мутации `UccTray` не вызывают `ksni::Handle::update`, пункты меню не обрабатываются. Целиком не портированы: мульти-батарейная + non-waking PCI + `/proc/<pid>/fd` телеметрия (`SessionTelemetry`), безопасный `pidfd_open`+`startTime` терминатор GPU-процессов (`ProcessTerminator`), передача `XDG_ACTIVATION_TOKEN` в `ucc-gui`, и все 6 модулей `ucc-session-agent`.
- **Критерий исправления:** Реализовать полноценные Rust-бинарники `ucc-desktop-tray` и `ucc-session-agent` с паритетом всех перечисленных функций C++ (`F23`–`F25`, `F31`–`F36`, `F39`, `F45`).

#### `ISSUE-15` — Чистый checkout ломает `cargo test --workspace` (неотслеживаемые фикстуры и `/home/frixit` в тестах)
- **Где:** `.cargo/config.toml` (`??`), `stand-fixtures/*.c` (`??`), `extra-fixtures/*.c` (`??`), `.gitignore:30` (`/artifacts/`), `crates/ucc-config/tests/{corruption_recovery.rs:27-35,405-544, config_persistence.rs:14-22,167-229}`, `crates/xtask/tests/build_verification.rs:43-48`
- **Доказательство:** На чистом клоне репозитория 8 тестов с C-мок библиотеками не найдут `stand-fixtures`/`extra-fixtures`, `build_verification.rs` упадёт из-за отсутствующего `.cargo/config.toml`, 4 теста `corruption_recovery.rs` упадут на `assert!(fixture_path.exists())`, а 3 теста `config_persistence.rs` молча пропустят проверки из-за жёсткого пути `/home/frixit/Рабочий стол/fork control center/UCC-Revorked/artifacts/rust-migration/fixtures` (папка игнорируется `.gitignore`).
- **Критерий исправления:** Добавить `.cargo/config.toml` и анонимизированные/синтетические C и JSON фикстуры в отслеживаемый каталог (например `tests/fixtures/`), убрать пути `/home/frixit/...`, заменить молчаливые `return` в `config_persistence.rs` на жёсткие `assert!`.

---

### Серьёзные баги, безопасность и паритет (`P1`)

#### `ISSUE-16` — `write_atomic` превращает симлинк `/etc/ucc/settings` в `0777` (доступный всем на запись!) и ломается о `readonly()`
- **Где:** `crates/ucc-config/src/persistence.rs:69, 81, 104-118, 179-186, 210`
- **Доказательство:** `fs::symlink_metadata(target_path)` на Linux для симлинка всегда возвращает права `0o777`. Затем `write_atomic` ставит `chmod 0777` на временный файл и `rename` затирает симлинк реальным root-файлом с правами `0777` (и `.bak` с `0777`!). Проверка `readonly()` (`mode & 0o222 == 0`) ошибочно запрещает `rename` даже от `root`. Временный файл создаётся без `.mode(...)` и `O_NOFOLLOW`, а ошибка `set_permissions` игнорируется.
- **Критерий исправления:** Разыменовывать симлинк до конечного файла (`canonicalize`), писать temp-файл в ту же директорию с `OpenOptions::mode(mode & 0o666)` + `O_NOFOLLOW`, проверять ошибку `set_permissions`, убрать ложную проверку `readonly()` перед `rename`.

#### `ISSUE-17` — Проблемы безопасности Polkit (`unix-process` с `start-time=0`, `AllowUserInteraction` на чтении, шторм IPC, несовпадение Action ID)
- **Где:** `crates/ucc-dbus/src/auth.rs:17-19, 87-186`, `crates/ucc-dbus/src/interface.rs:314-420, 702-720, 806-844, 1243-1250, 1351-1394`, `uccd/com.uniwill.uccd.policy:13-53`, `tests/test_control_authorization.py:8-29`
- **Доказательство:**
  1. Субъект `unix-process` с `start-time: 0` уязвим к PID-reuse race (`CVE-2013-4288`) и делает 2 лишних D-Bus запроса вместо атомарного `("system-bus-name", {"name": sender})`.
  2. Все 84 read-only метода (`interface.rs:315+`) вызывают `CheckAuthorization` с `AllowUserInteraction (0x1)` (~30–60 лишних IPC/сек при открытом GUI/трее; при этом на SSH/неактивном VT возвращают поддельные `false`/`""`/`"{}"` вместо `AccessDenied`, ломая CLI).
  3. При любой ошибке Polkit (`timeout`/disconnect) делается повторный запрос с незарегистрированным action `de.uniwill.uccd.*`, и нет лимита на параллельные интерактивные запросы.
  4. **Перекос политик:** `SetWaterCoolerPumpVoltage`, `TurnOffWaterCoolerFan/Pump`, `SetNVIDIAPowerOffset` ошибочно требуют `ManageHardware` (пароль админа!) вместо `Control` по `test_control_authorization.py` и `df9c34c`; при этом `ApplyProfile`/`SaveCustomProfile`/`SetActiveProfile` с произвольными `odm_power_limits` (TDP) и `fan.use_control = false` требуют лишь `Control` без `ManageHardware`, обходя описание `com.uniwill.uccd.policy:40-43`.
- **Критерий исправления:** Перейти на `system-bus-name`, убрать `AllowUserInteraction` на `AuthAction::Read` (или возвращать явный `AccessDenied`), ограничить конкурентные интерактивные запросы семафором/таймаутом, согласовать `ManageHardware` vs `Control` между `interface.rs`, C++ `UccDBusService.cpp` и `.policy`.

#### `ISSUE-18` — `com.uniwill.uccd.conf` блокирует интерфейс `de.uniwill.uccd` и разрешает прослушку сигналов всем UID; конфликты `uccd.service`
- **Где:** `uccd/com.uniwill.uccd.conf:6-21`, `uccd/uccd.service:18, 27, 38-40`, `crates/ucc-dbus/src/lib.rs:24, 35`
- **Доказательство:** `com.uniwill.uccd.conf` разрешает только `send_interface="com.uniwill.uccd"`, поэтому реальный system bus отбросит любой вызов legacy `de.uniwill.uccd` (`AccessDenied`/`ServiceUnknown`, вопреки `DIFF-09`), а `send_type="method_call"` отсутствует. Строка `<allow receive_sender="com.uniwill.uccd" />` в `context="default"` позволяет любым фоновым/удалённым UID читать все broadcast-сигналы с конфигом/подсветкой. В `uccd.service` `CapabilityBoundingSet` отрезает `CAP_DAC_OVERRIDE` / `CAP_DAC_READ_SEARCH` (может ломать `/sys/kernel/debug/dri/...` и дочерний `nvidia-smi`), а `AmbientCapabilities` бесполезны при `User=root` + `NoNewPrivileges=true`.
- **Критерий исправления:** Добавить правило для `de.uniwill.uccd` (или уточнить `DIFF-09`), добавить `send_type="method_call"`, сузить `receive_sender`, убрать лишние `AmbientCapabilities` и проверить права на debugfs/`nvidia-smi` на реальной системе.

#### `ISSUE-19` — 30+ синхронных Read-методов на Tokio executor + ~90 `.unwrap()` на `Mutex`/`RwLock` (каскадная паника после сбоя)
- **Где:** `crates/ucc-dbus/src/interface.rs:323-620+`, `crates/uccd/src/backend.rs` (~90 `.lock().unwrap()` / `.read().unwrap()`), `crates/ucc-dbus/src/auth.rs:245`
- **Доказательство:** 84+ `AuthAction::Read` метода вызывают `self.backend.*()` прямо в async-контексте Tokio, выполняя без оффлоада: `Command::new("prime-select")` без таймаута, чтение 1.3 МБ `/usr/share/hwdata/pci.ids` на каждый `GetSystemInfoJSON`, `/dev/tuxedo_io` `ioctl`, ~100 sysfs-чтений (`GetCpuTopologyJSON`), захват `std::sync::Mutex` воркера анимации. При панике в оффлоаженном потоке или воркере (`catch_unwind` ловит панику, оставляя мьютекс poisoned) **любой последующий Read RPC паникует внутри Tokio**!
- **Критерий исправления:** Заменить `.unwrap()` на `.unwrap_or_else(|e| e.into_inner())` в `backend.rs`/`auth.rs`, кэшировать статичный `SystemInfoJSON` при старте, добавить таймаут `prime-select`, перенести блокирующие чтение-RPC на bounded `spawn_blocking` / кэшированные снимки.

#### `ISSUE-20` — Uniwill `tdp_channels = 3` захардкожен без пробинга `R_UW_TDP0..2`
- **Где:** `crates/ucc-hardware/src/detection.rs:589-592`, `crates/ucc-hardware/src/platform.rs:221`, `crates/uccd/src/backend.rs:2319-2335`, `uccd/3rdparty/tuxedo_io_lib/targets/uniwill.h:256-272`
- **Доказательство:** C++ `getNumberTDPs` проверяет `R_UW_TDP2..0`. Rust всегда ставит `tdp_channels = 3`, из-за чего на моделях Uniwill с 0/1/2 TDP `apply_profile_internal` падает на каждом применении профиля.
- **Критерий исправления:** Определять реальное число каналов `0..=3` пробингом `R_UW_TDP2..0` в `detection.rs`.

#### `ISSUE-21` — Пробуждение NVIDIA dGPU из `D3cold` (`nvidia-smi -pm 0`, `/proc/driver/nvidia`) и пробелы AMD dGPU
- **Где:** `crates/ucc-hardware/src/devices/controller.rs:256-261`, `crates/ucc-hardware/src/telemetry/collector.rs:108,119,267-312,601-637`, `crates/uccd/src/backend.rs:611-615, 3483-3499, 4224-4226`
- **Доказательство:**
  1. `PowerDeviceController::apply` запускает `nvidia-smi -pm 0` при `wake_nvidia == false` **даже когда dGPU уже в `suspended` (`D3cold`)**, будя видеокарту (скачок +15–30 Вт!). То же с чтением `/proc/driver/nvidia/gpus/*/information` в `backend.rs:3483`.
  2. `TelemetryCollector` не авто-обнаруживает дискретные AMD GPU (`0x1002`), а `get_dgpu_info_values_json` возвращает фейковый `sleeping_nvidia` на Intel-only/AMD системах. При заданном `amd_dgpu_hwmon` тик читает sysfs без проверки `power/runtime_status == "active"`, будя AMD dGPU из BOCO/D3cold.
- **Критерий исправления:** Проверять `power/runtime_status == "active"` перед `nvidia-smi -pm` (если persistence не включался UCC), `/proc/driver/nvidia` и `amd_dgpu_hwmon`; авто-детектить AMD dGPU; отдавать нейтральный `vendor: "unknown"` / `-1` когда dGPU нет.

#### `ISSUE-22` — Intel `i915`/`xe` (`energy1_input`, `gt/gt0/rps_act_freq_mhz`) и скачок RAPL после сна
- **Где:** `crates/ucc-hardware/src/telemetry/{gpu.rs:143-182, cpu.rs:66-76}`
- **Доказательство:** Intel hwmon экспонирует `energy1_input` (мкДж), а не `power1_average`/`power1_input`, и `i915` на Linux 6.2+ использует `gt/gt0/rps_act_freq_mhz` — оба отсутствуют. Кроме того, при сбросе `energy_uj` к `0` после S3/s2idle `curr < prev` даёт ложный скачок мощности на ~`max_energy_range_uj` (~100 000+ Вт) из-за отсутствия сброса по сну и верхней отсечки (`<= 1000 W`).
- **Критерий исправления:** Считать мощность Intel hwmon по дельте `energy1_input`, добавить путь `gt/gt0/rps_act_freq_mhz`, сбрасывать `RaplEnergyCounter` на resume и отбрасывать неправдоподобные wraparound-спайки (`> 1000 W`).

#### `ISSUE-23` — Яркость (PWM-рампа -> откат всего профиля, инверсия `amdgpu_bl`, игнор MUX) + заглушка Hz + пустые `refreshRates` в KMS
- **Где:** `crates/uccd/src/backend.rs:1379-1397, 2770-2793, 3115-3164, 3786-3892, 3980-3985, 4185-4201`, `uccd/inc/DisplayBacklight.hpp`, `uccd/src/workers/DisplayWorker.cpp`
- **Доказательство:**
  1. `set_display_brightness_internal` ждёт совпадения `actual_brightness` всего 4 мс; на панелях с плавной PWM-рампой (50–200 мс) это считается ошибкой и **откатывает CPU/ODM/TDP всего профиля**! Плюс выбор первого алфавитного `/sys/class/backlight` игнорирует eDP DRM MUX и инверсию `amdgpu_bl`.
  2. `SetDisplayRefreshRate` всегда возвращает `false`, `profile.display.use_ref_rate` не применяется, а на Linux atomic KMS `/sys/class/drm/card*-*/modes` не содержит `@Hz` и файла `mode` -> `GetDisplayModesJSON` отдаёт пустой `activeMode` и `refreshRates: []` (подтверждено на текущем хосте).
- **Критерий исправления:** Портировать логику выбора/инверсии `DisplayBacklight.hpp` и не откатывать CPU/TDP из-за лага `actual_brightness`; брать частоты обновления через DRM/EDID/X11 и реализовать `SetDisplayRefreshRate`.

#### `ISSUE-24` — Сломан ручной режим СЖО, игнорируются `fanProfile`-пресеты/кривые утекают между профилями, не применяется зарядка и passive-режим
- **Где:** `crates/uccd/src/backend.rs:630-637, 1124-1167, 2072-2093, 4566-4610, 5293-5313, 5724-5842`, `crates/ucc-hardware/src/cooler.rs:712-730`, `crates/uccd/src/orchestrator.rs:547-585` vs `tests/test_watercooler_override.py`
- **Доказательство:**
  1. `SetWaterCoolerFanSpeed`/`SetWaterCoolerPumpVoltage` не переводят `CoolerController` в `Manual`, не сохраняют `manualFanSpeed`/`manualPumpVoltage`/`autoControlWC=false` в `active_profile`, не эмитят `ProfileChanged`; `apply_fan_and_device_state_locked` перезаписывает runtime-состояние `water_cooler_enabled` из профиля; `SetWaterCoolerAutoControl` шлёт нестандартный сигнал вместо `ProfileChanged`.
  2. `fan_profile_from_ucc_profile` не резолвит `p.fan.fan_profile` (`"fan-silent"`, `"fan-cool"` и кастомные), если `table_cpu`/`table_gpu` пусты -> срабатывает 100%-авария/auto; `apply_fan_and_device_state_locked` не очищает `temporary_curves`; `set_fan_profile_cpu` затирает временную GPU-кривую и наоборот.
  3. `activate_hardware()` не применяет сохранённые глобальные настройки зарядки, а `get_charge_*_available_thresholds` захардкожены `0..=99`/`1..=100` вместо чтения sysfs.
  4. На неподдерживаемом железе (`!is_device_supported()`) `uccd` всё равно меняет CPU/sysfs вместо пассивного D-Bus режима C++.
- **Критерий исправления:** Привести к полному паритету с `UccDBusService.cpp` и `test_watercooler_override.py`.

#### `ISSUE-25` — `SysfsLedTransport::parse_zone_id` путает зоны per-key RGB при HID/ITE-префиксах
- **Где:** `crates/ucc-lighting/src/transport.rs:107-117, 141-175`
- **Доказательство:** Фильтр ищет `.contains("rgb:kbd_backlight")`, но `parse_zone_id` делает `strip_prefix("rgb:kbd_backlight_")`. На реальных именах вида `0003:048D:CE00.0001:rgb:kbd_backlight_21` префикс не совпадает и зоны сортируются по случайному порядку `readdir`!
- **Критерий исправления:** Парсить индекс после `"rgb:kbd_backlight_"` (`split_once` / `rsplit_once('_')`).

#### `ISSUE-26` — Жёсткая всефайловая валидация `ConfigManager::load_from_path` отвергает весь `/etc/ucc/settings` из-за одного легаси-профиля
- **Где:** `crates/ucc-config/src/manager.rs:165-199`, `crates/ucc-core/src/{cpu.rs:30-69, 114-125, fan.rs:47-73}`
- **Доказательство:** Нулевые сентинелы (`onlineCores: 0`, `scalingMinFrequency: 0`), немонотонный TDP (`TDP1 < TDP0`) или равные соседние точки температуры в одном старом пользовательском профиле проваливают `validate_settings` и **бракуют весь `/etc/ucc/settings`** при запуске демона!
- **Критерий исправления:** Маппить `0` в `None` при загрузке легаси, при чтении изолировать/санитайзить проблемный единичный профиль без сброса всего файла конфига, оставив строгие отказы на границе D-Bus.

#### `ISSUE-27` — Множественные баги контрактов, кодов возврата и таймаутов в `ucc-cli`
- **Где:** `crates/ucc-cli/src/{cli.rs:407-430, client.rs:13-20, 470-501}` и `crates/ucc-cli/src/commands/{keyboard,fan,lightbar,devices,power_limits,profile,hardware,statemap,power,cpu,gpu,charging,cooler}.rs`
- **Доказательство:**
  - `lightbar status` и `devices status` являются алиасами `Get` и вызывают `*SettingsJSON` вместо `*StatusJSON`.
  - `power-limits` ожидает `[i64]` вместо `[{"index", "min", "max", "current"}]` — всегда пусто.
  - `profile get/defaults/customs` ищет `cpu["epp"]` (вместо `energyPerformancePreference`) и массив `odmPowerLimits` (вместо `{tdpValues: [...]}`), печатает яркость при `useBrightness: false` и теряет половину полей.
  - `load_local_assignments` ищет `settings["customProfiles"]` (массив) вместо `settings["profiles"]` (map) — теги `[AC, BAT]` в `fan list`/`keyboard profiles` не работают.
  - `fan get/set` для кастомных пресетов ждёт `item["json"].as_str()` вместо вложенного `Value`.
  - Массовый возврат `exit 0` при D-Bus/Polkit ошибках (`.unwrap_or(false)`, `.unwrap_or_default()`, `-1` у `brightness get`), а 12 из 14 модулей команд вообще **обходят `with_rpc_timeout`** (подвисают на 25с+ вместо 1.5с).
- **Критерий исправления:** Починить парсинг JSON во всех командах, разделить `status`/`get`, пропускать все RPC через `with_rpc_timeout`, возвращать ненулевой код выхода на `-1` и ошибки транспорта/Polkit.

#### `ISSUE-28` — Критические контракты C++ GUI <-> Rust `uccd`: `callVoidMethod` (14 методов), порядок зарядки и кеш `GetCpuTopologyJSON`
- **Где:** `libucc-dbus/UccdClient.cpp:227-258, 409-789`, `ucc-gui/inc/BatterySettingsWidget.hpp:36,70,90-95`, `ucc-gui/inc/CpuCoreSelectionWidget.hpp:43-45`, `crates/uccd/src/backend.rs:5644-5651`
- **Доказательство:**
  1. `UccdClient::callVoidMethod` игнорирует `bool`-ответ и возвращает `true` если не было D-Bus исключения; таким методом обёрнуты 14 `bool`-методов (`SetActiveProfile`, `ApplyProfile`, `SaveCustomProfile`, `SetCharging*`, `SetNVIDIAPowerOffset`, `SetDisplayRefreshRate` и др.), поэтому GUI пишет «Сохранено» даже при отказе Polkit или валидации!
  2. `BatterySettingsWidget::apply()` всегда пишет `StartThreshold` до `EndThreshold`: при поднятии окна выше текущего `End` (`40-60` -> `60-90`) демон отклоняет `Start`, а из-за `callVoidMethod` GUI говорит об успехе.
  3. `GetCpuTopologyJSON` каждый раз создаёт новый `CpuTopology::default()` без кеша: для ядер, уже отключённых текущим профилем, `sched_setaffinity` даёт `EINVAL`, E-ядра становятся `"unknown"`, а `CpuCoreSelectionWidget` читает топологию только в конструкторе через собственный временный `UccdClient`.
- **Критерий исправления:** Перевести все 14 обёрток на `callMethod<bool>(...).value_or(false)`, выбирать порядок `End`/`Start` в `BatterySettingsWidget` в зависимости от направления сдвига, кешировать `CpuTopology` в `LiveUccdBackend` при старте и обновлять `CpuCoreSelectionWidget` при реконнекте `uccd`.

#### `ISSUE-29` — Прямые нарушения `ADR-01` в CMake и критические баги интерфейса/потока в `ucc-gui`
- **Где:** `CMakeLists.txt:67-75, 118-165`, `ucc-gui/CMakeLists.txt:20-27, 77, 82, 127`, `libucc-dbus/CMakeLists.txt:19, 54-56`, `include/WindowActivation.hpp:7-26`, `ucc-gui/src/{MainWindow.cpp:2054-2068, 2340-2370, DetectLayout.cpp:83-231, LCTWaterCoolerController.cpp:44-52, 269-303, FanControlTab.cpp:185, 580-595}`, `ucc-gui/qml/*`
- **Доказательство:**
  1. Нарушение `ADR-01`: `ucc-gui` линкует `Qt6::Bluetooth` и (опционально) `KF6::WindowSystem` / `KF6::StatusNotifierItem`; `CMakeLists.txt` требует `Qml/Quick/QuickControls2` безусловно; `libucc-dbus` собирается `SHARED` вместо `STATIC`; мёртвый QML ещё лежит в `ucc-gui/qml/*`; без `KF6` `WindowActivation.hpp:21` игнорирует `XDG_ACTIVATION_TOKEN`.
  2. `MainWindow::onSaveClicked()` никогда не снимает старые привязки `power_*`, а при нажатии `Save` на встроенном профиле без чекбоксов навсегда вешает спиннер `m_saveInProgress = true`.
  3. UI-поток фризится: `DetectLayout` запускает до 6 синхронных процессов по 3с (до 18с, плюс запуск XWayland на чистом Wayland и ложное `contains("it")`), `LCTWaterCoolerController` делает 2 синхронных D-Bus `call()` раз в секунду (и не инициализирует `m_connectedModel`), `FanControlTab` на каждом пикселе перетаскивания слайдера создаёт синхронный `QDBusInterface` (`Introspect()` + вызов).
- **Критерий исправления:** Очистить CMake/`ucc-gui` строго под `ADR-01` (передать `XDG_ACTIVATION_TOKEN` через `qputenv` + `requestActivate`), починить снятие `stateMap` и флаг `m_saveInProgress`, вынести `DetectLayout`/D-Bus вызовы из UI-потока в async, инициализировать `m_connectedModel`.

#### `ISSUE-30` — Баги `ucc-gnome`, `ucc-tray` и C++ `ucc-desktop-tray` (сломанная схема, `GetPowerState == power_wc`, краш на логине Wayland)
- **Где:** `ucc-gnome/{metadata.json:8, uccdClient.js:255, extension.js:1035-1044}`, `ucc-tray/src/TrayBackend.cpp:169`, `ucc-desktop/{main.cpp:16, DesktopTray.cpp:120-144}`
- **Доказательство:**
  1. `ucc-gnome/metadata.json:8` ссылается на несуществующую GSettings-схему `org.gnome.shell.extensions.ucc` и вызывает удалённый метод `GetKeyboardBacklightControlEnabled` (`DIFF-03`).
  2. `ucc-gnome` и `ucc-tray` определяют подключение СЖО по `GetPowerState == "power_wc"` вместо `GetWaterCoolerConnected`: при ручном режиме (`ac`/`bat`/`type_c`) или power-request СЖО пропадает из UI, даже будучи подключённой.
  3. `ucc-desktop-tray` (`main.cpp:16`) выходит с `exit 1` если трей на старте Wayland-сессии ещё не поднялся (`!isSystemTrayAvailable()`), а `openControlCenter()` не знает про `readOnlyPreview` и ищет `ucc-gui` только в одном каталоге.
- **Критерий исправления:** Исправить `metadata.json` и `uccdClient.js`, перейти на `GetWaterCoolerConnected`, убрать фатальный выход в `ucc-desktop/main.cpp` и использовать `GuiLauncher.hpp`.

#### `ISSUE-31` — Сломанные `.rpm` (`ucc.spec`) и `.deb` (`debian/*`) манифесты, `obj-x86_64-linux-gnu` и отсутствующие systemd-скрипты
- **Где:** `ucc.spec:55-142`, `debian/{rules:9-27, ucc.install:1-29, not-installed:1-4, ucc.lintian-overrides:1-9}`, `Makefile:23-151`
- **Доказательство:** `ucc.spec` падает на check-files (теряет `ucc-hardware`, `ucc-sleep`, `ucc-import-check`, `ucc-desktop-tray`, `ucc-session-agent`, `.desktop`, docs) и не управляет `uccd-sleep.service`. `debian/rules:18` хардкодит `obj-x86_64-linux-gnu` при `Architecture: any`, `debian/ucc.install` + `not-installed` не покрывают все бинарники, нет `postinst`/`prerm`, lintian-overrides устарели на `0.2.0`. В `Makefile` `make build` вызывает несуществующий `./build-local.sh`, а `dist`/`rpm`/`deb`/`nix` сломаны (`ucc-` vs `UCC-`, `0.2.0`, фейковый Nix-хэш, нет исключений `target`/`artifacts`).
- **Критерий исправления:** Унифицировать пакетные манифесты под реальные цели установки и архитектуру multiarch, заменить сломанные цели `Makefile` на `xtask`.

#### `ISSUE-32` — `package.nix` гарантированно падает в `postFixup`, ломает пути FHS и `nix/nixos-module.nix` расходится с `uccd.service`
- **Где:** `package.nix:86-113`, `uccd/uccd-sleep.service:11,13`, `uccd/src/sleep-helper.cpp:21`, `nix/nixos-module.nix:80-173`
- **Доказательство:** `package.nix:98-103` делает `substituteInPlace --replace-fail "/usr/bin/systemctl"` в `uccd-sleep.service`, но в нём нет `/usr/bin/systemctl` (там `/usr/lib/ucc/ucc-sleep`) -> мгновенный сбой сборки Nix! Пути `/usr/lib/ucc/*` в юнитах и `/usr/bin/systemctl` в `sleep-helper` не пропатчены. В `nix/nixos-module.nix` потеряны `ExecStopPost --fan-auto`, `Conflicts`, `After=systemd-modules-load.service`, часть `ReadWritePaths` (`/proc/sys/...`, `/sys/module/...`), а pre-sleep/post-sleep вызывают `systemctl start/stop` в обход `SleepState`.
- **Критерий исправления:** Починить `substituteInPlace` в `package.nix`, патчить все `/usr/lib/ucc/` пути и вызов `systemctl` через `PATH`/параметр, синхронизировать `nix/nixos-module.nix` с юнитами и `ucc-sleep`.

#### `ISSUE-33` — Сильный дрейф и противоречия в `docs/rust/execution/*` (`state.json`, `backlog.json`, `findings.json`, `feature-status.json`, `RESUME.md`, `dbus-status.json`)
- **Где:** `docs/rust/execution/{state.json, backlog.json, findings.json, feature-status.json, dbus-status.json, RESUME.md}`
- **Доказательство:** В одном `state.json` одновременно указаны закрытие M7/M8 и открытые `M7.1-M7.7`/`M6.5`/`M8` + мёртвые ID субагентов `W1/W2/R1/R2` от M4. `backlog.json` помечает `M2.5` (`autosave`/`aquaris`/`uccrc`/`sleep-state`) как `"done"` при 0% реализации, `M10.5` `"done"` при `"planned"` зависимости `M10.1`, а в `M7.*`/`M8.*` сохраняет `"No Rust implementation"` в `status_basis`. `findings.json` преждевременно закрыл `IPC-03`, `DESK-01`, `BUILD-02`, `TEST-01`, и поставил `resolved` на удалённый `GUI-01..03`. `feature-status.json` имеет дублирующие невалидные ключи `"status"` рядом с `"implementation_status"`. `RESUME.md` и `dbus-status.json` застряли на 8 ноября.
- **Критерий исправления:** Синхронизировать все реестры с реальным состоянием кода и настоящего issue-листа: переоткрыть незакрытые дефекты, убрать старых агентов M4 и Iced `M1.2..M1.4` (пометить `superseded_by_adr_01`).

---

### Средние дефекты и edge cases (`P2`)

#### `ISSUE-34` — Пробелы сигналов D-Bus (`ModeReapplyPendingChanged`, `PowerDeviceStatusChanged`), круглосуточный опрос телеметрии, баги CRUD профилей и невалидированный `spawn_command`
- **Где:** `crates/uccd/src/backend.rs:48-50, 1290, 554, 3169-3200, 3660-3721, 4539-4550, 4770-4800, 5068-5156`, `crates/ucc-hardware/src/devices/{controller.rs:19-33, 179-185, 403-412, whitelist.rs}`
- **Суть:**
  - `ModeReapplyPendingChanged` никогда не отправляется; `PowerDeviceStatusChanged` не отправляется при переключении AC/Type-C/Bat (когда `pd_rev` не менялся).
  - Нет 10-секундного авто-сна `sensor_data_collection` из C++ (телеметрия опрашивает NVML/RAPL/sysfs 24/7 каждые 800 мс).
  - Нет лимита размера/числа кастомных профилей (`AuthAction::Control` может раздувать `/etc/ucc/settings`); `AddCustomProfile` не генерирует ID при `""` и не сверяет коллизии имён со встроенными; `DeleteCustomProfile` оставляет удалённый профиль в RAM/железе без `reconcile_power_state`; `GetProfilesJSON` дублирует кастомные профили поверх `GetCustomProfilesJSON`; `GetDeviceName` отдаёт маркетинговое имя вместо ID/пустой строки.
  - `PowerDeviceController` имеет гонку `libc::kill(pid, SIGKILL)` по переиспользованному PID после `child.wait()` (поток не джойнится) и запускает `iw`/`nvidia-smi`/`systemctl` через `$PATH`, а `scan_power_devices` пишет на диск до валидации.

#### `ISSUE-35` — `UCC_READ_ONLY_PREVIEW` (UB при гонке `env::set_var` и запись в железо), трипликация моделей конфига и несоответствие `DefaultProfiles`
- **Где:** `crates/ucc-config/src/{persistence.rs:15-23, 251-299, legacy.rs:31-364, defaults.rs:48-290}`, `crates/ucc-core/src/dto.rs:423-462`
- **Суть:** Тесты `ucc-config` вызывают `std::env::set_var` параллельно с `getenv` в соседних модулях (UB); preview-режим в `uccd` блокирует сохранение JSON, но **не блокирует** реальные ioctl/sysfs-записи в `ucc-hardware` и `ucc-lighting`. Конфиг триплицирован (`UccSettings` vs `legacy::SettingsManager` vs `LegacySettingsDto`), `restore_from_backup` дублирует логику `write_atomic` с разными правами (`0600`/`0644`). В `defaults.rs` `fan_profile` имеет неправильные ID (`"quiet"` вместо `"fan-quiet"`), в `High Performance` насильно включены `enableWaterCooler = true` и ручные 85% СЖО, и есть расхождения по моделям/2-профильным Uniwill (`"overboost"` падает при `perf_profiles_available == 2`).

#### `ISSUE-36` — Мелкие аппаратные баги (`NVML` null-handle и одноразовая блокировка `NvApiState`, обход `Sysfs` в `TuxedoIoBackend`, ложное подключение к любому BLE NUS-устройству, origin анимации Lightbar)
- **Где:** `crates/ucc-hardware/src/{nvml.rs:307-373, 883-888, platform.rs:188, 354, bluez_cooler.rs:84-99, 175-205, 305-341}`, `crates/ucc-lighting/src/lightbar.rs:526-540`, `crates/uccd/src/orchestrator.rs:576`
- **Суть:** `nvml.rs:884` не проверяет `handle.is_null()`, а `NvApiState` после первой ошибки навсегда отключает вольтметр; `TuxedoIoBackend` читает `std::fs::read_to_string` напрямую в обход мока `Sysfs`; `BluezCoolerClient` не вызывает `Adapter1.StartDiscovery`, подключается к **любому** BLE-устройству с Nordic UART Service без фильтра `"lct"` (риск слать команды часам/ESP32) и даёт 20 мс таймаут на `Disconnect`; `anim_started` хранится снаружи `LightbarController`, из-за чего переключение режима скачет в середину анимации.

#### `ISSUE-37` — `AboutPage` (`eb98bfa`) не включает `DEBUG`-логи из-за системного `qtlogging.ini` + спам логов + баги виджетов C++ GUI и `UccdClient`
- **Где:** `ucc-gui/src/{AppLogging.cpp:71-94, AboutPage.cpp:86-143, MainWindow.cpp:1214-1286, PumpCurveEditorWidget.cpp:34-84, 373, HardwareTab.cpp:50-106, KeyboardVisualizerWidget.cpp:395}`, `libucc-dbus/UccdClient.cpp:486-493, 900-913, 1299-1307`, `crates/uccd/src/backend.rs:186`
- **Суть:** `AppLogging::configure(true)` не вызывает `QLoggingCategory::setFilterRules`, поэтому на Arch/Fedora/KDE (где `*.debug=false` в `/etc/xdg/QtProject/qtlogging.ini`) галочка *"DEBUG and above"* пишет **ноль debug-строк**; при включении спамит 6 раз/тик `[Crosshair] CPU raw`, и `saveLogs` включён по умолчанию. В `PumpCurveEditorWidget` расходится `left` (80 vs 110 -> скачок точки на 30 px при drag) и режется 4-точечная кривая помпы. `HardwareTab` создаётся дважды и не проверяет `WebcamSWAvailable`/`FnLockSupported`/`readOnlyPreview`. `KeyboardVisualizerWidget::updateFromJSON` путает `i` и `m_ledIds[i]`. В `UccdClient` 3 ODM-метода и `subscribe*` — заглушки, а `getGpuInfo` зовёт несуществующий D-Bus метод.

#### `ISSUE-38` — Баги расширений и агентов сессии (`ucc-gnome`, `ucc-tray`, `ucc-session`)
- **Где:** `ucc-gnome/extension.js:156-184, 500-526, 681-694`, `ucc-tray/{CMakeLists.txt, src/CMakeLists.txt, applet/CMakeLists.txt, applet/*.qml}`, `ucc-session/{NotificationMonitor.hpp:30, LidDisplayHandler.hpp:129-135, RefreshAdvisor.cpp:413-415}`
- **Суть:** `ucc-gnome` не передаёт `fanProfileId` в `ApplyFanProfiles` (радиокнопка сбрасывается каждые 5с), дёргает `dbus` на каждый тик слайдера и (вместе с `TrayBackend`) следит за мёртвым `~/.config/uccrc`. `ucc-tray` компилирует `TrayBackend.cpp` дважды в две QML-библиотеки, не проверяет `FnLock`/`Webcam` поддержку и блокирует `Choose Color` в режиме `Breathe`. В `ucc-session` `NotificationMonitor` триггерит анимацию lightbar на собственные уведомления `PowerSourceAlert`/`DriverAlert`, а `hyprctl eval "<lua>"` ломается на стандартных сборках Hyprland без Lua-плагина.

#### `ISSUE-39` — Расхождения CLI parity и тулчейна (`ucc-cli`, `xtask`, `rust-toolchain.toml`, `MIGRATION_PLAN.md`, `README*`)
- **Где:** `crates/ucc-cli/src/{cli.rs:28-148, format.rs:113-118, client.rs:90-152}`, `crates/ucc-cli/src/commands/{cooler,gpu}.rs`, `crates/xtask/src/main.rs:79-148`, `crates/xtask/tests/build_verification.rs:10-25`, `rust-toolchain.toml`, `Cargo.toml:17-18`, `docs/BUILDING.md`, `contrib/frixit/desktop-transition.py`
- **Суть:** В `ucc-cli` нет подкоманды `version`, не принимаются `"enabled"`/`"disabled"`, `cooler status` при отключённой СЖО выводит `0 %` вместо `"not connected"`, `monitor --count=-1` запускает бесконечный цикл, не фильтруется `timestamp == 0` у вентиляторов, `gpu` не отражает живой `SetNVIDIAPowerOffset`. `xtask` не имеет цели `dist`, паникует на не-UTF-8 путях и собирает `.so` в публичный `/tmp/ucc-*`. `rust-toolchain.toml` не закреплён (`channel = "stable"`), `edition` расходится с `MIGRATION_PLAN.md`, `README`/`BUILDING`/`TESTING`/`desktop-transition.py` устарели.

#### `ISSUE-40` — Мусор в Git и корне репозитория
- **Где:** `nvidia_findings/*`, `contrib/frixit/drivers/update-hook.patch:4,95`, `.vscode/*`, `.gitignore:39-40`, корневые `audit-cpp-build/`, `consolidated-gui-xdg/`, `extended-nvml/`, `fake-nvml/`, `followup-fixtures/`, `rust-dbus-*.xml`, `.worktrees/{w1,w2,r2}`
- **Суть:** 10 скомпилированных ELF-бинарников в Git, захардкоженные пути `/home/frixit/...`, `/opt/devel/git/ucc`, `/home/erhan/...`, 7 временных директорий в корне и 3 замороженных worktree.

---

### Минорные замечания и чистка (`P3`)

#### `ISSUE-41` — Косметические и мелкие чистки по крейтам и GUI
- **Где:**
  - `crates/ucc-dbus/src/interface.rs`: при отказе `AuthAction::Read` массивные JSON-методы отдают `"{}"`/пустую строку вместо `"[]"`, а целочисленные — `0` вместо `-1`.
  - `crates/uccd/src/backend.rs:4867-4873`: `GetSettingsJSON` не обогащает JSON живыми полями `hwpDynamicBoostSupported` и дефолтными объектами как C++ `buildSettingsJSON`.
  - `crates/ucc-lighting/src/transport.rs:380-389`: `detect_lightbar` инвертирует приоритет `"rgb:lightbar"` vs `"lightbar"`.
  - `crates/ucc-config/src/{settings.rs:338-352, persistence.rs:239}` vs `manager.rs:249`: substring `contains(...)` в `get_profile` перекрывает кастомные профили; `cleanup_temp_files` не чистит `.tmp_restore.`.
  - `ucc-gui/src/{MainWindow.cpp:152, FluentSidebar.cpp:22-59}`: `Qt::WA_TranslucentBackground` без Wayland blur оставляет полупрозрачный сайдбар без размытия.
  - `Cargo.toml:39`, `crates/{ucc-cli,ucc-desktop,xtask}/Cargo.toml`: неиспользуемые зависимости (`thiserror`, `ucc-core`, `ucc-config`, `serde`, `nix` и др.).

---

## 5. Рекомендуемый порядок устранения (Дорожная карта)

1. **Фаза 0 (Git и гигиена):** Очистить временные папки/старые worktree, убрать ELF из `nvidia_findings/`, вынести фикстуры в отслеживаемый каталог, нарезать текущий working tree в чистые коммиты.
2. **Фаза 1 (P0 безопасность, железо, данные):** `ISSUE-01` (помпа СЖО), `ISSUE-05` (`--fan-auto` без личного DMI), `ISSUE-08` (сохранность полей зарядки/cTGP в конфиге), `ISSUE-09` (`temp <= 0`), `ISSUE-10`/`11`/`12`/`16`/`17` (root session bus, `/etc/ucc` `0777`, `~/.config`, bounded RPC pool, Polkit), `ISSUE-13` (клавиатура), `ISSUE-15` (чистый `cargo test`).
3. **Фаза 2 (Декомпозиция `uccd` и подключение мёртвых модулей):** Разбить `backend.rs` на модули, убрать `.unwrap()`, реально подключить `CpuController`/`CpuWorker` (`ISSUE-07`), `BluezCoolerClient` (`ISSUE-02`), `LidController`/`WakeupPolicy`/`logind` (`ISSUE-03`), `ucc-hardware`/`ucc-sleep`/`ucc-import-check` (`ISSUE-04`), починить D3cold, AMD/Intel телеметрию, PWM-яркость, СЖО/пресеты (`ISSUE-19`..`ISSUE-26`).
4. **Фаза 3 (Паритет клиентов и отказ от legacy C++):** Добить `ucc-cli` (`ISSUE-27`), `UccdClient`/`ucc-gui` + чистка CMake под `ADR-01` (`ISSUE-28`/`29`/`37`), портировать `ucc-desktop-tray` и `ucc-session-agent` на Rust (`ISSUE-14`), починить `ucc-gnome` (`ISSUE-30`).
5. **Фаза 4 (Упаковка и приёмка на железе):** Интегрировать Rust + `xtask dist` в `.deb`/`.rpm`/`PKGBUILD`/`Nix`/CI (`ISSUE-06`, `ISSUE-31`, `ISSUE-32`), обновить `docs/rust/execution/*` (`ISSUE-33`) и пройти стенд из `MISSING_TESTS_MATRIX.md`.
