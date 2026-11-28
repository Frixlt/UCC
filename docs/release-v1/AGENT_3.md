# Runbook Агента 3 — Qt6 C++20 GUI, `libucc-dbus`, `ucc-desktop`, `ucc-session-agent`, `ucc-cli`, `ucc-gnome`

- **Ветка:** `release/a3-gui-desktop-cli` (от `rust/migration`)
- **Эксклюзивная зона владения:** `ucc-gui/**`, `libucc-dbus/**`, `crates/ucc-desktop/**`, `crates/ucc-session-agent/**`, `crates/ucc-cli/**`, `ucc-gnome/**`
- **Обязательное чтение перед стартом:** `docs/release-v1/CONTRACT.md`, `docs/release-v1/SYNC_PROTOCOL.md`, `docs/audit/TOTAL_AUDIT_2026_11_28.md`.

## Фаза 1 — 5 параллельных субагентов-исполнителей

### Субагент 3.1: `ucc-gui/` P0/P1 безопасность памяти, lifetime и таймеры
- Убрать двойное владение (`std::unique_ptr` + `this` как `QObject*` parent) в `MainWindow`, `SystemMonitor`, `ProfileManager` (хранить как Qt-дети с `QPointer` или без `this` parent с явным порядком разрушения до дочерних виджетов).
- Починить разыменования `nullptr`: `PowerDevicesTab.cpp:276`, `NotificationCenter.cpp:107` (fallback `QGuiApplication::primaryScreen()`), `BatterySettingsWidget.hpp:36`, `PowerDevicesTab.cpp:178-182`, `MonitorTab.cpp:158, 381`.
- Починить риск `SIGABRT` в `KeyboardVisualizerWidget.cpp:81-115` + `DetectLayout.cpp:76-102` (проверять `isInterruptionRequested()` на каждом шаге и корректно завершать поток).
- `FluentTheme.cpp:32-60, 152-171`: перевести `ThemeEvents` на `static QPointer<ThemeEvents>` с проверкой `qApp`; добавить обработку `QEvent::ChildAdded` в `CardVisibility`; убрать захардкоженный тёмный stylesheet `m_driverHealthBanner` в `DashboardTab.cpp:180`.
- `FanControlTab.cpp:381-390, 720-733`: в `updateWaterCoolerPolling()` останавливать `m_waterCoolerPollTimer` при `!m_waterCoolerSupported`; всегда создавать sub-tab СЖО и скрывать через `setTabVisible(1, supported)`.
- `MonitorTab.cpp:68`: передать `QObject* context` в 3-аргументный `QTimer::singleShot`; поправить narrowing `ptrdiff_t -> int` (`NotificationCenter.cpp:39`) и `long -> int` (`main.cpp:41`).

### Субагент 3.2: `libucc-dbus/` + `ucc-gui/` async D-Bus и UI state sync
- `libucc-dbus/AsyncRead.hpp:25-32` & `UccdClient.cpp:61, 111-124, 895-902, 1326-1476`:
  - Удалить мёртвые `"GetGpuInfo"` и `"SetKeyboardBacklight"` и неиспользуемые стабы.
  - Починить утечку `QDBusPendingCallWatcher` при `consumer == nullptr` и проверку `if (ready)` в `AsyncRead.hpp` и `setDisplayBrightnessAsync()` (+ `setAutoStartService(false)`).
  - Добавить `getModeButtonActionAsync()` и `setModeButtonActionAsync()` строго по `CONTRACT.md`.
- Перевести интерактивные Polkit-сеттеры (`DashboardTab`, `BatterySettingsWidget`, `SystemMonitor`, `MainWindow::saveProfile/setBatchStateMap`, `PowerDevicesTab`, `LightbarTab`, `KeyboardTab`) и стартовые чтения на `*Async` / `readUccdBatch`.
- Починить dirty-state и слайдеры в `MainWindow.cpp`:
  - `QSignalBlocker` на `m_eppCombo`, `m_hwpDynamicBoostCheckBox`, `m_sameFanSpeedCheckBox`, `m_cpuCoresSlider`, связки `minFreq/maxFreq` и `TDP1..3`.
  - Обновлять `m_minFrequencyValue` в `loadProfileDetails()`, подключить `m_sameFanSpeedCheckBox` к `markChanged()`, управлять `m_saveButton` централизованно в `updateButtonStates()`.
  - Исправить `builtinFanProfiles().contains(currentProfile)` -> проверка по `fanProfileId` (`MainWindow.cpp:3468`); исправить `m_keyboardProfileCombo` (`findData`); добавить `QSignalBlocker` в `FanControlTab::reloadFanProfiles()`; откатывать тумблеры в `HardwareTab` при ошибке Polkit.

### Субагент 3.3: Новые фичи GUI для `v1.0.0` (строго по `CONTRACT.md`)
1. **Слайдер максимальной температуры CPU (TCC `80..=95 °C`)** в редакторе профилей (`MainWindow.hpp`/`.cpp`):
   - Слайдер в карточке `"System performance"` (диапазон `tccMinC..=tccMaxC`, по умолчанию `80..=95 °C`), скрывать если `!tccSupported` из `GetSystemInfoJSON`.
   - Чтение/запись `cpu.maxTempC` в `loadProfileDetails()` / `buildProfileJSON()` с `QSignalBlocker`.
2. **Живые лимиты EC на Dashboard** (`DashboardTab.cpp` + `SystemMonitor.hpp/.cpp`):
   - Парсить опциональные `pl1W`, `pl2W`, `pl4W`, `tccLimitC`, `pmscMode` из `GetCpuPowerValuesJSON` и `ctgpW`, `dynamicBoostW` из `GetDGpuInfoValuesJSON`.
   - В карточке CPU добавить компактную подстроку `m_cpuLimitsRow` (`PL1 / PL2 / PL4 · TCC · Mode`), в карточке GPU (`m_dGpuExtraRow`) добавить `cTGP / DB` (автоматически скрывать, если демон не отдаёт поля).
3. **Экспериментальные настройки — переназначение кнопки режима `0x00B0`** (`MainWindow.cpp`, `m_experimentalPage`):
   - Добавить карточку в `m_experimentalPage` (заблокированную до `m_experimentalRisksAccepted`): выбор действия (`Firmware default` / `Cycle UCC profiles` / `Toggle Custom Max` / `Emit F14`) + асинхронное чтение/применение через `GetModeButtonAction` / `SetModeButtonAction`.

### Субагент 3.4: `crates/ucc-desktop/` + `crates/ucc-session-agent/`
- **`crates/ucc-desktop/` (`tray.rs`, `main.rs`, `launcher.rs`):**
  - В `tray.rs`: держать пункт `"Service unavailable (reconnecting...)"` постоянно с `visible: !daemon_connected` (устраняет DBusMenu layout churn и баг перемешивания меню трея!).
  - Передавать начальный `daemon_connected` в `UccTray`, объединить 4 `handle.update()` в один атомарный `handle.update(|tray| { ... }).await`.
  - Выставить `MissedTickBehavior::Delay`, распараллелить `fetch_snapshot()`, вынести `TrayAction::Quit` в отдельный `watch`/`oneshot` (нельзя дропать под backpressure), сбрасывать `fan_auto` при реконнекте, заменить OS-поток в `launcher.rs` на `tokio::process::Command`.
- **`crates/ucc-session-agent/`:**
  - В `refresh_advisor.rs`: определять Hyprland также по `HYPRLAND_INSTANCE_SIGNATURE` (и KDE по `KDE_SESSION_VERSION`), а в 3 интеграционных тестах Hyprland явно задавать `XDG_CURRENT_DESKTOP=Hyprland` — устранить 100% флаковость!
  - Регистрировать `SIGTERM` синхронно до `tokio::spawn` и `acquire_single_instance_lock()`; перевести lock на `Result<Option<File>, io::Error>` с приватным каталогом `0700`.
  - Добавить цикл переподключения к `uccd` и переподписки стримов при рестарте демона (вместо `exit(2)` и вечного `pending()`), `MissedTickBehavior::Delay`, экспоненциальный backoff для `dbus-monitor`, сброс `awaiting_app_name` при обрезке длинной строки в `notification_monitor.rs`, и отмену фоновых задач при shutdown.

### Субагент 3.5: `crates/ucc-cli/` + `ucc-gnome/`
- **`crates/ucc-cli/`:**
  - Включить `#![forbid(unsafe_code)]` в `crates/ucc-cli/src/lib.rs`.
  - Добавить `clap` `value_parser` диапазоны (`0..=100`, `0..=255`, `>= 1` для `--interval`, `ValueEnum` для `power mode`/`statemap`), локальную проверку `serde_json::from_str` до D-Bus.
  - Ограничить `zones.clamp(1, 256)` в `keyboard.rs:174-190` (закрыть OOM DoS).
  - Печатать конкретные ошибки D-Bus в `stderr` без молчаливого `map_err(|_| 1)`, отличать ошибку RPC от «не поддерживается» (ненулевой exit code при отказе), защитить `stdout` от паники `SIGPIPE`, убрать `i32` overflow/truncation.
- **`ucc-gnome/`:**
  - Перенести метку `'Water Cooler'` внутрь `this._wcMetricsBox`, перевести обработчики СЖО/fan-профиля на `async` с откатом UI при отказе D-Bus, заменить `Gio.bus_get_sync` на асинхронный `Gio.bus_get`.

## Фаза 2 — 5 независимых adversarial-ревьюеров + Финальный Gate
Запусти 5 независимых ревьюеров (при необходимости запросив у пользователя «продолжай» для сброса лимита субагентов), исправь все замечания и прогони:
```bash
export CARGO_TARGET_DIR="$HOME/ucc-target-slim" TMPDIR="$HOME/ucc-tmp" RUSTFLAGS="-D warnings -C debuginfo=0"
cargo fmt --all -- --check
dbus-run-session -- cargo test -j 2 -p ucc-desktop -p ucc-session-agent -p ucc-cli
cmake -B build-a3 -DBUILD_TESTS=ON && cmake --build build-a3 -j 2 && ctest --test-dir build-a3 --output-on-failure
```
Запушь в `release/a3-gui-desktop-cli` и обнови `sync/agent3.md` до `READY_FOR_MERGE`.
