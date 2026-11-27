# Тестирование и верификация `ucc-rusted`

[Сборка и упаковка](BUILDING.md) · [Миграция и восстановление](MIGRATION_AND_RECOVERY.md)

Документ описывает автоматические проверки Rust, C++/Qt6 CTest-наборы, изолированные тестовые стенды (`dbus-run-session`, `LD_PRELOAD`-фикстуры, mock BlueZ, Wayland/Hyprland) и чек-лист аппаратной приёмки.

## 1. Набор проверок Rust

Все команды выполняются из корня репозитория. В офлайн-среде добавляется `--offline`.

### Полный прогон тестов, линтеров и форматирования

```bash
# 1. Паритет версий (6 манифестов), валидность JSON-метаданных, CycloneDX SBOM и инварианты импортёра
cargo xtask parser-check
cargo xtask check-versions

# 2. Проверка форматирования
cargo fmt --all -- --check

# 3. Статический анализ Clippy (все цели, предупреждения = ошибки)
cargo clippy --workspace --all-targets -- -D warnings

# 4. Полный прогон unit- и интеграционных тестов на изолированной шине D-Bus
dbus-run-session -- cargo test --workspace --locked --offline
```

В CI (`.github/workflows/build.yml`) тесты Rust выполняются на матрице MSRV `1.89.0` и `1.99.1` с флагом `--no-fail-fast`.

### Покрытие по крейтам (`crates/*/tests/`)

| Крейт | Ключевые тестовые наборы |
| --- | --- |
| `ucc-core` | `domain_validation`, `dto_mapping`, `property_invariants`, `core_validation_rules` |
| `ucc-config` | `config_persistence`, `corruption_recovery`, `offline_import`, `settings_manager_tests`, `config_security` |
| `ucc-hardware` | `ioctl_abi`, `ioctl_preconditions`, `cooling_engine`, `cpu_topology_and_control`, `power_devices_and_tunables`, `sysfs_failure_matrix`, `nvml_guard`, `fake_bluez_cooler`, `cooler_protocol`, `cooler_stress_tests`, `vendor_detection`, `telemetry_suite` |
| `ucc-dbus` | `auth_tests`, `dbus_security_limits`, `contract_tests`, `dbus_skeleton`, `ipc_framing` |
| `uccd` | `daemon_integration`, `fault_matrix`, `import_check_corpus`, `daemon_security` |
| `ucc-cli` | `cli_tests`, `cli_daemon_integration`, `cli_security` |
| `ucc-desktop` | `tray_menu`, `window_activation`, `tray_security` |
| `ucc-session-agent` | `session_agent_integration` |
| `ucc-lighting` | `lighting_tests`, `hid_transport`, `lighting_safety` |
| `xtask` | `packaging_contracts`, `build_verification`, `maintenance_workflows` |

---

## 2. Наборы C++ / Qt6 CTest

Для выявления выходов за границы контейнеров STL все тестовые CMake-сборки компилируются с `-DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS"`.

### Предварительная сборка `interop_daemon`

Кросс-языковые тесты `test_rust_uccd_interop` и `test_gui_rust_e2e` запускают бинарный пример Rust-демона `interop_daemon`:

```bash
cargo build -p uccd --example interop_daemon --locked --offline
```

Путь к собранному бинарнику передаётся через переменную окружения `UCC_INTEROP_DAEMON_BIN` (по умолчанию `target/debug/examples/interop_daemon`).

### Основной набор (Normal suite — 48 тестов в конфигурации без Plasma / до 51 при наличии `gjs` и `node`)

Конфигурация для автономного Rust runtime (`BUILD_TRAY=OFF`):

```bash
cmake -S . -B build-rust-runtime -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS" \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_TESTS=ON
cmake --build build-rust-runtime --parallel 2

# Проверка отсутствия неразрешённых shared-зависимостей у ucc-gui
! ldd build-rust-runtime/bin/ucc-gui | grep -q 'not found'

# Прогон CTest
UCC_INTEROP_DAEMON_BIN="$PWD/target/debug/examples/interop_daemon" \
  ctest --test-dir build-rust-runtime --output-on-failure --parallel 2
```

В состав основного набора (`tests/CMakeLists.txt`) входят:
- 19 C++ unit-тестов логики (`test_state_utils`, `test_sleep_state`, `test_lightbar`, `test_driver_health`, `test_notification_monitor`, `test_power_policy`, `test_cpu_selection`, `test_power_devices`, `test_lid_wakeup`, `test_gpu_sysfs`, `test_display_backlight`, `test_keyboard_controller`, `test_fan_profile`, `test_profile_manager`, `test_settings_manager`, `test_metrics_history`, `test_aquaris_selection`, `test_session_telemetry`, `test_process_terminator`);
- 13 Python-тестов развёртывания, миграции и импорта (`test_deployment`, `test_prepare_deployment`, `test_desktop_transition`, `test_import_check`, `test_sleep_install`, `test_tcc_import`, `test_controlcenter_migration`, `test_control_authorization`, `test_watercooler_override`, `test_device_identification`, `test_bluetooth_retry`, `test_bluetooth_startup`, `test_display_identity`);
- 3 теста session-агента (`test_driver_alert`, `test_refresh_advisor_modes`, `test_refresh_advisor_lifecycle`);
- 3 теста GNOME Shell (`test_gnome_async` при наличии `gjs`, `test_gnome_launch` и `test_gnome_refresh` при наличии `node`);
- 10 GUI-тестов (`test_fan_curve_editor`, 5 шардов `test_gui_audit_*`, `test_gui_instance`, `test_system_monitor`, `test_app_logging`, `test_gui_rust_e2e`) и 1 клиентский interop-тест (`test_rust_uccd_interop`).

Дополнительно в `PKGBUILD` запускается тест миграции:

```bash
python3 contrib/mechrevo/tests/test_migration.py
```

### Набор Read-Only Preview (3 теста)

```bash
cmake -S . -B build-preview -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS" \
  -DUCC_READ_ONLY_PREVIEW=ON \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_TESTS=ON
cmake --build build-preview --parallel 2 \
  --target test_gui_audit test_gui_instance test_app_logging ucc-gui

ctest --test-dir build-preview \
  -R "^(test_gui_audit_preview_gallery|test_gui_instance|test_app_logging)$" \
  --output-on-failure
```

Проверяет блокировку всех путей записи (`previewMustBlockEveryWritePath`, `previewMustNotSaveLocalMonitorPreferences`), изоляцию инстансов (`test_gui_instance`) и логирование в preview-режиме (`test_app_logging`).

### Кросс-языковые тесты (`test_rust_uccd_interop` и `test_gui_rust_e2e`)

Запускаются через `dbus-run-session -- bash tests/run_rust_interop_test.sh <test-binary> <source-dir> [args...]`:
- `test_rust_uccd_interop`: проверяет совместимость C++ `UccdClient` (`libucc-dbus`) с живым Rust `interop_daemon` (JSON-контракты, сигналы, управление профилями и политиками, временная директория `UCC_INTEROP_CONFIG_DIR`).
- `test_gui_rust_e2e`: запускает полный Qt6 `MainWindow` против Rust `LiveUccdBackend` (`interop_daemon`), проверяет все 11 разделов сайдбара, переключение Light/Dark-тем, запись профилей и политик крышки (`F45 Lid`), отклонение невалидных команд без ложного подтверждения успеха и блокировку записей при `UCC_READ_ONLY_PREVIEW=1`.

---

## 3. Изолированные тестовые стенды

### Приватная системная шина D-Bus

Скрипт `tests/run_private_system_bus.sh` перенаправляет системную шину на временную сессионную шину внутри `dbus-run-session`:

```bash
dbus-run-session -- bash tests/run_private_system_bus.sh <команда>
```

Устанавливаемые переменные: `DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"`, `GIO_USE_VFS=local`. Обёртка `tests/run_rust_interop_test.sh` дополнительно создаёт изолированный временный `HOME` и `XDG_{CONFIG,DATA,STATE,CACHE}_HOME`, запускает `<test-binary>` дочерним процессом (без `exec`) с передачей дополнительных аргументов и кода возврата, гарантируя очистку временного каталога через `trap ... EXIT`.

### C-фикстуры `LD_PRELOAD` (`stand-fixtures/`, `extra-fixtures/`)

Компиляция всех 6 разделяемых библиотек выполняется командой:

```bash
UCC_FIXTURES_DIR=/tmp/ucc-fixtures cargo xtask build-fixtures
```

| Исходник | Артефакт | Назначение и переменные управления |
| --- | --- | --- |
| `stand-fixtures/open-ancestor.c`, `extra-fixtures/open-ancestor.c` | `stand/open-ancestor.so`, `extra/open-ancestor.so` | Перехват `open64` (`O_WRONLY`) на `UCC_E1_SWAP_OPEN_PATH`: переименовывает родительский каталог `UCC_E1_SWAP_ANCESTOR` и подменяет его симлинком на `UCC_E1_SWAP_DESTINATION` (проверка защиты от TOCTOU-атак на предка) |
| `stand-fixtures/open-swap.c` | `stand/open-swap.so` | Перехват `open64` (`O_WRONLY`) на `UCC_AUDIT_SWAP_TARGET`: выполняет `unlink` + `symlink` на `UCC_AUDIT_SWAP_DESTINATION` (проверка защиты от TOCTOU-подмены целевого файла) |
| `stand-fixtures/nvml.c` | `stand/libnvidia-ml.so.1` | Mock NVML с подсчётом ссылок (`nvmlAuditRefCount`) и обращений к спящему GPU (`nvmlAuditSleepingProbes`). Переменные: `UCC_E1_FIXTURE_SLOW_INIT`, `UCC_AUDIT_FAIL_FIRST_NVML_COUNT`, `UCC_E1_FIXTURE_REVERSED_ORDER`, `UCC_AUDIT_REVERSED_NVML_INDEX` |
| `extra-fixtures/nvml.c` | `extra/index/libnvidia-ml.so.1` | Расширенный mock NVML без `nvmlDeviceGetHandleByPciBusId_v2` (fallback по индексу, горячая подмена через `UCC_E16_FIXTURE_HOTPLUG_FROM`/`TO`, `UCC_E16_FIXTURE_FORWARD_ORDER`) |
| `extra-fixtures/nvml.c` (`-DAUDIT_WITH_BDF`) | `extra/bdf/libnvidia-ml.so.1` | Mock NVML с `nvmlDeviceGetHandleByPciBusId_v2` и счётчиком `nvmlAuditBdfCalls` (`UCC_E16_FIXTURE_REFUSE_BDF`, проверка отказоустойчивости BDF-маршрутизации) |

### Изолированный mock BlueZ для водяного охлаждения

Тест `crates/ucc-hardware/tests/fake_bluez_cooler.rs` поднимает на приватной шине D-Bus mock-объекты `org.bluez.Device1` (`name = "LCT21001-TEST"`, UUID Nordic UART Service) и `org.bluez.GattCharacteristic1` (TX/RX). Проверяет полный цикл подключения, обмена пакетами протокола LCT и повторных попыток `BluezCoolerClient` без физического Bluetooth-адаптера:

```bash
dbus-run-session -- cargo test -p ucc-hardware --test fake_bluez_cooler --locked --offline
```

### Проверка Hyprland / Wayland без прав root

Пользовательские клиенты (`ucc-gui`, `ucc-desktop-tray`, `ucc-session-agent`) запускаются строго от обычного пользователя:

- В headless-среде (CI, SSH) используется `QT_QPA_PLATFORM=offscreen` (задаётся автоматически для `test_fan_curve_editor` и `test_gui_instance`, а также в `run_gui_audit_test.sh`).
- Логика Hyprland 0.55+ (`hyprctl` Lua-обновление поля `mode` с сохранением разрешения, масштаба, положения и цветового профиля) и `org.freedesktop.Notifications` проверяется без root и без реального композитора тестами:

```bash
ctest --test-dir build-rust-runtime -R "^test_refresh_advisor_(modes|lifecycle)$" --output-on-failure
```

- На живой Wayland-сессии проверяются: активация окна по клику на иконку трея, отсутствие второго экземпляра `ucc-session-agent` (lock-файл пользователя), доставка уведомлений об изменении физического источника питания (AC / Battery / USB-C) и применение частоты экрана только после явного подтверждения или переключения профиля.

---

## 4. Чек-лист аппаратной приёмки

### A. Неразрушающие проверки (Non-destructive)

Выполняются до любых записей в аппаратные регистры:

1. Наличие драйвера и узла устройства:
   ```bash
   ls -l /dev/tuxedo_io
   systemctl status uccd.service --no-pager
   ```
2. Аудит датчиков вентиляторов (только чтение EC):
   ```bash
   sudo /usr/lib/ucc/ucc-hardware --audit
   ```
3. Проверка статуса демона и телеметрии через CLI:
   ```bash
   ucc-cli power status
   ucc-cli devices status
   ucc-cli lightbar status
   ```
4. Проверка Read-Only Preview: запуск `./run-preview.sh`, переключение вкладок, подтверждение отсутствия изменений в `/etc/ucc/settings` и аппаратных регистрах.

### B. Проверки на живом оборудовании (Live Hardware)

1. **D3cold и сон дискретной NVIDIA GPU**:
   - В `Power Devices` отключить **Keep NVIDIA awake (Blackwell workaround)** и применить политику.
   - Убедиться, что режим питания PCI установлен в `auto`, persistence mode выключен, и при отсутствии активных GPU-задач и внешних мониторов на NVIDIA статус переходит в `suspended`:
     ```bash
     cat /sys/bus/pci/devices/<NVIDIA_BDF>/power/control
     cat /sys/bus/pci/devices/<NVIDIA_BDF>/power/runtime_status
     ```
   - Убедиться, что фоновый мониторинг `uccd` не пробуждает спящий GPU (интервал NVML-опроса при `auto` не чаще 1 раза в 16 с, при состоянии `suspended` пробы пропускаются).

2. **BLE-водяное охлаждение LCT22002 / LCT21001**:
   - Проверить видимость устройства в `bluetoothctl` и подключение через `uccd`.
   - Проверить переключение режимов (Auto / Manual), изменение скорости вентилятора и напряжения помпы, а также отсутствие остановки охлаждения при открытии ручных контролов в GUI.
   - Проверить восстановление после кратковременного разрыва Bluetooth-соединения.

3. **Цикл Suspend / Resume**:
   - Убедиться, что `uccd-sleep.service` включён (`systemctl is-enabled uccd-sleep.service`).
   - Перед сном перевести систему в ручной режим (или включить водянку), выполнить `systemctl suspend`, затем пробудить ноутбук.
   - Проверить журнал `journalctl -u uccd-sleep.service -u uccd.service -b`:
     - `ucc-sleep prepare` применил `WakeupPolicy`, записал `/run/ucc/sleep-state.json` и остановил `uccd.service`;
     - `ucc-sleep resume` запустил `uccd.service`, демон прочитал и применил сохранённое состояние до регистрации имени на шине, а файл `/run/ucc/sleep-state.json` удалён.

4. **CPU, вентиляторы и подсветка**:
   - Проверить `cpu.enabledCores` (выбор отдельных ядер P/E, сохранение заблокированного ядром `cpu0`, восстановление всех ядер при остановке).
   - Проверить аварийный возврат вентиляторов в авто-режим: `sudo /usr/lib/ucc/ucc-hardware --fan-auto` (код возврата `0`).
   - Проверить подсветку клавиатуры (физические LED ID) и Lightbar (изменение цвета и корректное завершение анимации).
