# Руководство по кодовой базе `ucc-rusted` для AI-агентов

Документ дополняет корневой `AGENTS.md` и описывает архитектуру крейтов, точки входа, процедуру расширения D-Bus API, шаблоны изолированного тестирования без доступа к железу хоста и известные технические ограничения рантайма.

---

## 1. Карта крейтов и ключевые точки входа

Проект разделён на 10 Rust-крейтов в корневом `Cargo.toml` (`edition = "2021"`, `rust-version = "1.89"`, `GPL-3.0-or-later`) и автономный C++20/Qt6 клиент `ucc-gui` (`ADR-01`).

### 1.1 Rust Workspace (`crates/`)

| Крейт | Ключевые файлы и точки входа | Ответственность |
| --- | --- | --- |
| `crates/ucc-core` | `src/lib.rs`, `src/dto.rs`, `src/profile.rs`, `src/fan.rs`, `src/cooling.rs`, `src/cpu.rs`, `src/power.rs`, `src/charging.rs`, `src/led.rs`, `src/error.rs` | Чистые доменные модели, сериализация/десериализация DTO (совместимые с JSON-контрактом D-Bus), валидация профилей, кривых вентиляторов и помпы (`PumpVoltage`), расчёт политик питания. Зависимости от ОС и I/O отсутствуют. |
| `crates/ucc-config` | `src/manager.rs`, `src/persistence.rs`, `src/settings.rs`, `src/defaults.rs`, `src/import.rs`, `src/legacy.rs`, `src/platform_paths.rs` | Атомарное чтение/запись настроек (`O_NOFOLLOW` + запись во временный файл + `rename`), миграция и offline-импорт старых профилей TCC/UCC, разрешение путей для Unix (`/etc/ucc`, `~/.config/ucc`, `/run/ucc`) и Windows (`ADR-06`). |
| `crates/ucc-hardware` | `src/platform.rs`, `src/ioctl.rs`, `src/sysfs.rs`, `src/nvml.rs`, `src/fan_controller.rs`, `src/cooler.rs`, `src/bluez_cooler.rs`, `src/cpu/`, `src/devices/`, `src/telemetry/`, `src/windows_wmi.rs` | Абстракция `PlatformBackend` (`ADR-05`), бэкенд `TuxedoIoBackend` (`/dev/tuxedo_io`), заготовка `WindowsWmiEcBackend` (`ADR-06`), песочница `SysfsRoot` + `SysfsWriteJournal`, управление CPU/PCI/USB/Wi-Fi устройствами, контроллеры внутренних вентиляторов и BLE-СЖО LCT (`BlueZ GATT`), динамическая обёртка NVML (`libloading`) с защитой от пробуждения D3cold (`ADR-04`), сборщик телеметрии и история. |
| `crates/ucc-lighting` | `src/transport.rs`, `src/hid_transport.rs`, `src/keyboard.rs`, `src/lightbar.rs` | Абстракция `LightingTransport` (`SysfsLedTransport` поверх `ite_8291`/`ite_8291_lb` и общий `UniwillHidTransport` для `048D:600B`, `048D:7001`/`048D:7000` по `ADR-05`/`ADR-06`), физическая калибровка клавиатуры (`Enter=56`, `LShift=21`, `LCtrl=0`), 5 анимаций передней полосы Lightbar. |
| `crates/ucc-dbus` | `src/lib.rs`, `src/interface.rs`, `src/backend.rs`, `src/auth.rs`, `src/proxy.rs`, `src/ipc.rs` | Контракт D-Bus (`com.uniwill.uccd` и алиас `de.uniwill.uccd`, пути `/com/uniwill/uccd` и `/de/uniwill/uccd`), 123 метода и 7 сигналов через макрос `define_uccd_interface!`, трейт `UccdBackend`, `PolkitAuthorizer`, очереди `OffloadLanes`, клиентские прокси `UccdProxy`/`UccdLegacyProxy`, транспортно-независимый JSON-RPC (`ipc.rs`). |
| `crates/uccd` | `src/main.rs`, `src/orchestrator.rs`, `src/backend.rs`, `src/backend/{dbus_impl,cooling,cpu,display_gpu,lighting_power,profile_tx}.rs`, `src/lid.rs`, `src/sleep_state.rs`, `src/bin/{ucc-hardware,ucc-import-check,ucc-sleep}.rs` | Корневой системный демон (Tokio, single-writer actor model по `ADR-02`), связывание `UccdBackend` с аппаратными контроллерами, транзакции применения профилей (`profile_tx.rs`), восстановление после сна (`ucc-sleep`, `sleep_state.rs`), вспомогательные бинарники `ucc-hardware` и `ucc-import-check`, тестовый `examples/interop_daemon.rs`. |
| `crates/ucc-cli` | `src/main.rs`, `src/cli.rs`, `src/client.rs`, `src/format.rs`, `src/commands/*.rs` | CLI-клиент `ucc-cli` поверх `UccdProxy`: подкоманды `status`, `profile`, `power`, `power-limits`, `cpu`, `gpu`, `fan`, `cooler`, `charging`, `devices`, `hardware`, `keyboard`, `lightbar`, `monitor`, `statemap`; генерация man-страниц и автодополнения bash/zsh/fish. |
| `crates/ucc-desktop` | `src/main.rs`, `src/tray.rs`, `src/launcher.rs`, `src/telemetry.rs`, `src/pidfd.rs` | Автономный системный трей `ucc-desktop-tray` на протоколе SNI (`ksni`/`zbus`, `ADR-03`), чтение батареи и процессов GPU, безопасное завершение пользовательских процессов через `pidfd` (защита от PID reuse), запуск/активация `ucc-gui` с передачей `xdg-activation-v1` токена. |
| `crates/ucc-session-agent` | `src/main.rs`, `src/alerts.rs`, `src/lid_handler.rs`, `src/notification_monitor.rs`, `src/notifications.rs`, `src/refresh_advisor.rs` | Пользовательский агент сессии: пересылка событий уведомлений без передачи текста уведомления, предупреждения `DriverHealth`, обработка крышки, предложения смены частоты экрана KScreen/композитора. |
| `crates/xtask` | `src/main.rs`, `src/lib.rs`, `src/maintenance.rs` | Сборочные и релизные задачи: `check-versions`, `release`, `dist` (CycloneDX 1.5 SBOM + `RELEASE_MANIFEST.json`), `install-runtime`, `import-offline`, `maintenance`, `capture-gallery`, `parser-check`. |

### 1.2 Активный C++20 / Qt6 клиент и legacy-каталоги

- **Активный GUI**: `ucc-gui/` (`src/MainWindow.cpp`, `src/ProfileManager.cpp`, `src/DashboardTab.cpp`, `src/PowerDevicesTab.cpp`, `src/LightbarTab.cpp`, `src/KeyboardTab.cpp`, `src/FanControlTab.cpp`, `src/HardwareTab.cpp`, `src/LCTWaterCoolerController.cpp`, `src/SystemMonitor.cpp`) + статически линкуемая обёртка `libucc-dbus/` (`UccdClient.hpp`, `UccdClient.cpp`, `AsyncRead.hpp`, `GuiLauncher.hpp`). При сборке с `-DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF` в бинарнике `ucc-gui` допустимы только `Qt6::{Core, Gui, Widgets, DBus, Charts}`.
- **Legacy C++ (вывод из активной сборки в `M13.7+`)**: `uccd/` (кроме `uccd/com.uniwill.uccd.policy` и unit-файлов до их переноса), `ucc-cli/`, `ucc-tray/` (KDE Plasma апплет на KF6/QML), `ucc-session/`, опциональный GJS-адаптер `ucc-gnome/`.

---

## 2. Безопасное добавление нового D-Bus метода

Изменение D-Bus контракта затрагивает до 7 слоёв. Все слои должны оставаться синхронизированы, иначе упадут контрактные или кросс-языковые тесты.

### Шаг 1. Типы и валидация (`crates/ucc-core`)
- Добавьте или расширьте доменную структуру / DTO в `crates/ucc-core/src/dto.rs` (или профильном модуле `fan.rs`, `power.rs`, `charging.rs`, `led.rs`).
- Всю валидацию диапазонов, запрещённых кодов (например, `PumpVoltage::V12 = 1`) и ошибок (`ValidationError` в `crates/ucc-core/src/error.rs`) размещайте в `ucc-core`, а не в обработчиках D-Bus.

### Шаг 2. Контракт, авторизация и offload (`crates/ucc-dbus`)
1. **`crates/ucc-dbus/src/backend.rs`**: добавьте метод в трейт `UccdBackend` и его реализацию по умолчанию в `DefaultUccdBackend`.
2. **`crates/ucc-dbus/src/interface.rs`**:
   - Добавьте метод внутрь макроса `define_uccd_interface!` — он автоматически сгенерирует синхронные эндпоинты для `UccdInterface` (`com.uniwill.uccd`) и `UccdLegacyInterface` (`de.uniwill.uccd`) на обоих путях `/com/uniwill/uccd` и `/de/uniwill/uccd`.
   - Если метод принимает JSON-строку, первым делом проверьте `payload_within_limit(method_name, &payload)` (лимит `MAX_DBUS_JSON_PAYLOAD_BYTES = 256 KiB`).
   - Выберите уровень авторизации `AuthAction`:
     - `Read` — только чтение состояния/телеметрии;
     - `Control` — обычное управление профилями, подсветкой, кривыми;
     - `ManageHardware` — изменение TDP/лимитов мощности, отключение управления вентиляторами (`fan.useControl == false`), произвольные пороги/тип зарядки, ручное напряжение помпы.
   - Выберите соответствующую линию `OffloadLane` (`Read`, `Profile`, `Charging`, `Lighting`, `PowerGpu`, `Cooler`, `Display`, `Misc`), чтобы блокирующий вызов не замедлял соседние подсистемы.
3. **`crates/ucc-dbus/src/proxy.rs`**: добавьте метод в `#[zbus::proxy]` трейты `UccdProxy` и `UccdLegacyProxy` с точной сигнатурой (PascalCase-имя D-Bus метода).
4. **`crates/ucc-dbus/src/ipc.rs`**: если метод должен быть доступен через кроссплатформенный JSON-RPC (`ADR-06`), добавьте его ветку в `dispatch_json_rpc`.

### Шаг 3. Реализация в демоне (`crates/uccd`)
- Прокиньте метод в `crates/uccd/src/backend/dbus_impl.rs` и реализуйте бизнес-логику в соответствующем модуле (`crates/uccd/src/backend/{cooling,cpu,display_gpu,lighting_power,profile_tx}.rs`).
- Соблюдайте правило отсутствия паник (`unwrap`/`expect` запрещены) и single-writer модель для аппаратного доступа (`ADR-02`).

### Шаг 4. CLI (`crates/ucc-cli`)
- Добавьте обёртку в `crates/ucc-cli/src/client.rs`, аргументы в `crates/ucc-cli/src/cli.rs` и обработчик в `crates/ucc-cli/src/commands/<subsystem>.rs`.
- Поддержите как человекочитаемый вывод, так и `--json` (через `crates/ucc-cli/src/format.rs`).

### Шаг 5. C++ D-Bus клиент (`libucc-dbus` и `ucc-gui`)
- Объявите и реализуйте метод в `libucc-dbus/UccdClient.hpp` и `libucc-dbus/UccdClient.cpp`.
- Для вызовов из UI-потока `ucc-gui/` используйте асинхронные вызовы (`AsyncRead.hpp` / `QDBusPendingCallWatcher`), чтобы не блокировать event loop окна.

### Шаг 6. Политика Polkit (`uccd/com.uniwill.uccd.policy`)
- Новые методы, как правило, должны ложиться в существующие 3 действия: `com.uniwill.uccd.read`, `com.uniwill.uccd.control`, `com.uniwill.uccd.manage-hardware`.
- Если вводится новое действие Polkit, обновите `uccd/com.uniwill.uccd.policy`, константы в `crates/ucc-dbus/src/auth.rs` и тесты авторизации.

### Шаг 7. Контрактные и интеграционные тесты
- Обновите `crates/ucc-dbus/tests/contract_tests/mock_backend.rs`, `rpc_groups.rs` и `introspection_and_auth.rs` (проверка XML-интроспекции и матрицы Polkit Allow/Deny).
- При изменении состава методов актуализируйте `docs/rust/execution/dbus-status.json` и тесты безопасности в `crates/ucc-dbus/tests/dbus_security_limits.rs`.
- Добавьте интеграционный сценарий в `crates/uccd/tests/daemon_integration/` и прогоните:
  ```bash
  export PATH="$HOME/.cargo/bin:$PATH"
  cargo clippy --workspace --all-targets --locked --offline -- -D warnings
  dbus-run-session -- cargo test --workspace --locked --offline --no-fail-fast
  ```

---

## 3. Изолированное тестирование (Sysfs, Hardware, BLE, D-Bus)

Все автотесты должны быть воспроизводимы на машине без Uniwill EC, без NVIDIA GPU, без подключённой СЖО и без root-прав.

### 3.1 Sysfs и Procfs через `SysfsRoot`
- Используйте `ucc_hardware::sysfs::SysfsRoot::new(tempdir.path())` (с `tempfile::TempDir`) и прикреплённый `SysfsWriteJournal`.
- Создавайте синтетическую иерархию файлов (`sys/bus/pci/devices/.../power/runtime_status`, `sys/class/power_supply/...`, `sys/devices/system/cpu/...`, `sys/class/leds/...`) внутри `TempDir`.
- Проверяйте не только возвращаемый `Result`, но и журнал записей (`journal.entries()`), чтобы подтвердить порядок и содержимое записей.
- Проверяйте защиту от path traversal (`..`) и симлинков (`SysfsError::UnsafePath` / `O_NOFOLLOW`).

### 3.2 Аппаратные ioctl и `PlatformBackend`
- Не открывайте `/dev/tuxedo_io` в тестах. Подставляйте mock-реализацию трейта `PlatformBackend` (`crates/ucc-hardware/src/platform.rs`) или `LightingTransport` (`crates/ucc-lighting/src/transport.rs`).
- Для проверки D3cold-защиты NVML (`crates/ucc-hardware/src/nvml.rs`) создайте фиктивный PCI-каталог с `power/runtime_status = "suspended"` и убедитесь, что возвращается `NvmlError::GpuSuspended` без попытки вызова `libnvidia-ml.so.1`.

### 3.3 BLE-СЖО (LCT21001 / LCT22002)
- Тестируйте протокол 8-байтных фреймов (`0xFE ... 0xEF`), keepalive (интервал 10 с, порог 3), минимальную паузу между записями (`BLE_WRITE_GAP >= 80 ms`) и генерационные таймауты через mock-транспорт без подключения к системному `bluetoothd`.
- Обязательно включайте негативный тест на отклонение `PumpVoltage::V12` (код `1`).

### 3.4 D-Bus и кросс-языковая совместимость
- Запускайте тесты D-Bus под изолированной сессионной шиной: `dbus-run-session -- cargo test --workspace --locked --offline --no-fail-fast`.
- Для модульной проверки логики интерфейса и прав доступа используйте встроенные тестовые авторизаторы из `crates/ucc-dbus/src/auth.rs`:
  - `AllowAllAuthorizer` — позитивные функциональные тесты;
  - `DenyAllAuthorizer` — проверка fail-closed ответов (`false`, `"{}"`, `""`) при отказе Polkit;
  - `TrackingAuthorizer` — верификация того, что конкретный метод запросил именно ожидаемый `AuthAction` (`Read` / `Control` / `ManageHardware`).
- Для проверки C++ клиента `libucc-dbus` и `ucc-gui` против Rust-демона собирайте `cargo build -p uccd --example interop_daemon --locked --offline` и передавайте путь через `UCC_INTEROP_DAEMON_BIN` в `ctest`.
- Для тестов `xtask` (`crates/xtask/tests/`) используйте mock-реализацию `CommandRunner` вместо реальных вызовов `systemctl`/`pacman`/`cmake`.

---

## 4. Известные грабли и правила конкурентности

### 4.1 Rust 2021: область жизни `MutexGuard` в `if let` и `match`
В Rust Edition 2021 временные значения в условии `if let` или `match` (включая `std::sync::MutexGuard`) **не освобождаются до конца всего блока `if / else` или `match`**.
- **Проблема**:
  ```rust
  // ОПАСНО в Rust 2021: guard удерживается внутри ветки if и else!
  if let Some(val) = state.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
      helper_that_also_locks(&state).await; // Deadlock или ошибка Send!
  }
  ```
- **Правило**: всегда копируйте/клонируйте данные в отдельную локальную переменную с явным блоком `{ ... }` до `.await` и до вызова любых функций, способных повторно захватить мьютекс:
  ```rust
  let val = {
      let guard = state.lock().unwrap_or_else(|e| e.into_inner());
      guard.get(&key).cloned()
  };
  if let Some(val) = val {
      helper_that_also_locks(&state).await;
  }
  ```
- Никогда не удерживайте `std::sync::MutexGuard` через точку `.await`.

### 4.2 Tokio blocking offload lanes (`OffloadLanes`)
В `crates/ucc-dbus/src/interface.rs` каждый экземпляр интерфейса создаёт изолированные семафоры `OffloadLanes`:
- `Read`: 16 слотов
- `Profile`, `Charging`, `Lighting`, `PowerGpu`, `Cooler`, `Display`: по 4 слота
- `Misc`: 8 слотов

Таймаут ожидания слота по умолчанию — `500 ms` (переопределяется переменной `UCC_OFFLOAD_QUEUE_TIMEOUT_MS`, минимум `10 ms`). Если запрос ждал в очереди более `25 ms`, вызывается `sender_is_alive` для отсечения уже отключившихся клиентов.
- **Правило**: любые синхронные операции (sysfs, ioctl, файловая запись конфигов, блокирующие вызовы драйверов) должны исполняться через `OffloadLanes` (сейчас используется `std::thread::Builder::new().spawn` на ограниченном семафоре; миграция на общий пул воркеров и объединение 4 экземпляров `OffloadLanes` — открытый долг `R22`), а не напрямую на async worker-потоках Tokio. Не ставьте взаимозависимые операции на одну и ту же линию без учёта лимита слотов.

### 4.3 `ENV_LOCK` в параллельных тестах
`cargo test` запускает тесты внутри одного бинарника параллельно в разных потоках. Вызовы `std::env::set_var` / `std::env::remove_var` глобальны для процесса и приводят к гонкам (а также небезопасны при параллельном чтении окружения).
- **Правило**: любой тест, изменяющий переменные окружения (например, `HOME`, `XDG_CONFIG_HOME`, `UCC_OFFLOAD_QUEUE_TIMEOUT_MS`), обязан захватывать общий статический мьютекс `static ENV_LOCK: Mutex<()> = Mutex::new(());` (см. образец в `crates/ucc-config/tests/config_persistence.rs`) и восстанавливать прежнее состояние окружения до освобождения блокировки (желательно через RAII-guard).

### 4.4 `libc::O_NOFOLLOW` при работе с файлами от `root`
`uccd` и `ucc-sleep` выполняются с правами `root`. Обычный `std::fs::write` или `File::open` по пути в пользовательском или общем каталоге уязвим к symlink-подмене (TOCTOU).
- **Правило**: при открытии файлов конфигов, state-файлов сна и узлов sysfs используйте `OpenOptions::new().custom_flags(libc::O_NOFOLLOW)` (в `uccd/src/sleep_state.rs` дополнительно `libc::O_CLOEXEC | libc::O_NONBLOCK`; для записи конфигов в `crates/ucc-config/src/persistence.rs` — `O_NOFOLLOW` + атомарный `rename` внутри того же каталога). Все обращения к `libc::O_NOFOLLOW` должны быть обёрнуты в `#[cfg(unix)]` для сохранения компиляции кроссплатформенного кода (`ADR-06`).
