# Состояние миграции на Rust (Frixit Control Center)

## Интеграция Rust в пакеты

- `d0e12e9` сохраняет исходные настройки CPU для восстановления следующим экземпляром демона, если запись при остановке не удалась.
- `610f8a8` переводит Arch, DEB, RPM и Nix на общий `xtask install-runtime`. Пакеты включают семь Rust-бинарников и отдельный Qt6 GUI согласно ADR-01.
- Собран и проверен нативный Arch-пакет, включая автозапуск, службы, man-страницу и completions. Идентификаторы всех семи ELF совпадают с результатами Cargo.
- Прошли 465 тестов Rust и 41 тест CTest. Clippy без предупреждений, read-only preview GUI собирается. Исходный архив проходит `cargo check --workspace --bins --locked --offline` с пустым кэшем Cargo.
- Открыты нативные сборки DEB/RPM/Nix, установка и откат на целевых системах, проверки Wayland и физического оборудования. Текущие результаты записаны в `state.json` и `backlog.json`.

## Исторический срез acceptance recheck на `e16f9ea`

- **Базовый коммит исходников C++:** `45921b0b7021448a81575a840b2b77d93da8b298`
- **Ветка интеграции:** `rust/migration`
- **Проверенный коммит среза:** `e16f9ea` + правки приёмки от 2026-11-08
- **Результаты проверки:**
  - **4 независимые пробы acceptance recheck:** `4/4 passed` (`config.rs`, `reversed-nvml`, `daemon-test-module.rs`).
  - **Тесты workspace:** `100% passed`, 0 failed, 0 ignored.
  - **Clippy:** `exit 0`, 0 warnings (`--workspace --all-targets -- -D warnings`).
  - **PID & Symlink Defense:** Защита `PidLock::acquire` (`O_NOFOLLOW`, отказ на симлинках), `resolve_pid_file` без деструктивных truncate-проверок, `stop_daemon` с валидацией PID > 1 и проверкой `/proc/<pid>/comm`.
  - **NVML Identity:** Fail-closed сопоставление по BDF без эвристик через переменные окружения; при отсутствии BDF API в многопроцессорных системах с неактивными GPU немедленный отказ `GpuSuspended` исключает обращение к спящей карте.
  - **Config Validation & Durability:** Единый валидатор `ConfigManager::validate_settings` (проверка порогов зарядки `0..=100`, `start < end` и профилей); ротация `.bak` через атомарное создание уникального временного файла с `O_NOFOLLOW` и удалением сторонних симлинков.
  - **D-Bus Backend:** Валидация профилей и фанов, исключение ложного успеха на некорректном JSON.

---

## Итоги аудита `e16f9ea`

По результатам проверки зафиксированы 33 замечания (`HW-01..09`, `CORE-01..02`, `CFG-01..05`, `IPC-01..04`, `CLI-01..05`, `GUI-01..03`, `DESK-01`, `BUILD-01..02`, `TEST-01..02`). Отметка `ready_for_hardware_acceptance` снята до полного устранения блокеров.

### Матрица слоёв приёмки

| Слой приёмки | Статус на `e16f9ea` | Свидетельства и ограничения |
|---|---|---|
| **Wire Parity (D-Bus)** | **VERIFIED** | Все 122 метода и 7 сигналов совпадают с C++ XML `de.uniwill.uccd.xml` на обоих интерфейсах (`/com/uniwill/uccd` и `/de/uniwill/uccd`). Проверено тестами `crates/ucc-dbus/tests/contract_tests.rs` и `v4_audit_introspection.rs`. |
| **CLI Parity** | **AUDITED / PARITY ISSUES TRACKED** | Команды сверены с эталонным C++ бинарником `/tmp/ucc-full-audit-5cqyt_e7/audit-cpp-build/bin/ucc-cli`. Выявлены расхождения CLI-01..05. |
| **Semantics** | **PARTIAL / REMEDIATION** | Базовые модели и хэндлеры реализованы. 14 проб воспроизводят семантические дефекты (CPU apply reporting, pump curve, device apply bypass). |
| **Authorization (Polkit)** | **DEFECTS TRACKED** | Выявлены IPC-01 (fallback bypass), IPC-02 (потеря identity вызывающего), IPC-03 (отсутствие rate-limiting/очереди). |
| **Error Handling** | **PARTIAL** | CLI маскирует ошибки D-Bus через `unwrap_or(false)` (CLI-04); CPU контроллер не сообщает ошибки записи sysfs (HW-04). |
| **Signals** | **SIMULATED ONLY** | 7 сигналов wire-валидны, эмиссия проверена на mock; интеграция с живым аппаратным демоном ожидает завершения M4. |
| **Hardware Writes** | **STRICTLY BLOCKED** | Прямые записи в реальные `/sys` и `/dev` заблокированы до закрытия всех 19 P1 замечаний и прохождения тестов на изоляторах. |
| **Packaging & Install** | **PLANNED** | Зафиксированы BUILD-01 (MSRV) и BUILD-02 (xtask генерация completions/man). Сборка пакетов запланирована на M12/M13. |

---

## Аудит паритета CLI (Rust `ucc-cli` против C++ `ucc-cli`)

Проведено прямое сопоставление вывода команд на живом демоне и mock-окружении:

1. **Команды верхнего уровня**:
   - `status`, `monitor`: структура вывода и текстовые секции совпадают.
   - `status --json`: JSON-схемы совпадают по составу ключевых секций (`activeProfile`, `cpu`, `gpu`, `hardware`, `powerPolicy`, `waterCooler`, `charging`).
   - `charging status`, `power status`, `brightness get`, `webcam get`, `fnlock get`, `watercooler status`, `cpu`: формат вывода идентичен.
2. **Выявленные расхождения и дефекты CLI**:
   - **CLI-01 (P1)**: CLI выполняет RPC без индивидуального per-call deadline. При подвисании демона после handshake команда зависает бессрочно.
   - **CLI-02 (P2)**: Отсутствие проверок границ чисел при конверсии u64 -> u8 в телеметрии; расчёт RPM по формуле `percent * 60` вместо чтения датчика; риск арифметического переполнения в GPU телеметрии.
   - **CLI-03 (P1)**: `charging set-thresholds` выполняет два независимых вызова RPC без транзакционной целостности; при сбое второго порога первый остаётся применённым.
   - **CLI-04 (P2)**: Опция `-v` в C++ выводит версию, в Rust вызывает ошибку разбора аргументов (поддерживается только `-V`). В командах-мутаторах `unwrap_or(false)` скрывает причину ошибки D-Bus.
   - **CLI-05 (P2)**: При отсутствии температуры dGPU клиент C++ (строка 1069 `UccdClient.cpp`) ошибочно подставлял температуру iGPU. В Rust CLI аналогичный fallback перенесён и смешивает источники данных.

---

## Реестр 33 замечаний сводного аудита

### Hardware (`HW-*`)
- `HW-01` (P1): NVML выбирает чужой/спящий GPU при отказе BDF lookup или смене порядка перечисления.
- `HW-02` (P1): Подмена корня SysfsRoot меняет границу проверки симлинков.
- `HW-03` (P1): Контроллеры устройств (`devices/controller.rs`) пишут напрямую через `std::fs`, минуя защищённый `SysfsRoot`.
- `HW-04` (P1): CPU controller сообщает `Ok(())` при отказе записи в sysfs (например, при попытке записи в директорию).
- `HW-05` (P1): Выбор частоты CPU пересекает рассчитанный потолок `no-turbo`.
- `HW-06` (P1): Низкоуровневый ioctl API обходит доменные ограничения и проверки прав устройства.
- `HW-07` (P1): Переход NVIDIA power policy продолжается даже при отказе подготовительного шага persistence mode.
- `HW-08` (P2): NVML возвращает нулевые показания при отказе измерений вместо `None`/ошибки.
- `HW-09` (P2): Разбор CPU topology не ограничивает максимальные диапазоны ядер.

### Core & Config (`CORE-*`, `CFG-*`)
- `CORE-01` (P1): Валидированная pump curve пропускает допустимую ступень охлаждения.
- `CORE-02` (P1): Legacy профиль V12 представим в модели без явной политики исполнения (риск перегрева).
- `CFG-01` (P1): Ротация `.bak` пишет через symlink, создавая риск перезаписи внешних файлов.
- `CFG-02` (P1): Сохранение конфигурации повышает права доступа (permission leak) к приватному файлу.
- `CFG-03` (P1): Глобальные charging thresholds обходят валидацию допустимого диапазона (start < end).
- `CFG-04` (P1): Ротация и restore не образуют согласованную durable транзакцию при аварийном завершении.
- `CFG-05` (P2): Два формата записи дают разную сохранность полей при импорте legacy профилей.

### IPC & D-Bus (`IPC-*`, `CLI-*`)
- `IPC-01` (P1): Запасное правило Polkit отменяет явный отказ авторизации.
- `IPC-02` (P1): Идентификатор владельца power request теряется перед вызовом backend.
- `IPC-03` (P1): Доверие и нагрузка Polkit не ограничены на границе сервиса (отсутствует rate-limiting).
- `IPC-04` (P1): Публичный default backend сообщает успех для `SetWebcam` и `SetDisplayBrightness` без эффекта.
- `CLI-01` (P1): Блокировка команд без deadline при зависании сервиса.
- `CLI-02` (P2): Небезопасные числовые преобразования в телеметрии.
- `CLI-03` (P1): Частичное применение порогов зарядки при сбое одного из двух RPC.
- `CLI-04` (P2): Неполный CLI контракт (`-v` alias, стирание ошибок D-Bus).
- `CLI-05` (P2): Смешение показаний iGPU и dGPU в выводе status.

### Frontend, GUI & Desktop (`GUI-*`, `DESK-*`)
- `GUI-01` (P1): Сохранение после старта добавляет демонстрационную кривую в пользовательский профиль.
- `GUI-02` (P2): Падение при перетаскивании точек (drag & drop) после замены набора точек кривой.
- `GUI-03` (P2): Экран выглядит подключённым при демонстрационном состоянии.
- `DESK-01` (P2): Трей считает отправку в очередь за успешное применение настройки.

### Сборка и тестирование (`BUILD-*`, `TEST-*`)
- `BUILD-01` (P1): Заявленный MSRV 1.85 несовместим с зависимостями из lockfile.
- `BUILD-02` (P2): Генерация completions и man pages через `xtask` не встроена в декларативный интерфейс сборки.
- `TEST-01` (P1): Штатный параллельный suite падает на разделяемых фикстурах и переменных окружения NVML.
- `TEST-02` (P1): Реестр приёмки объявляет готовность без связки результатов и требований (устранено R1).

---

## Первоначальный план закрытия замечаний аудита `e16f9ea`

1. **W1 (Backend):**
   - Устранить `HW-01..HW-07` в `crates/ucc-hardware` (NVML PCI BDF identity, SysfsRoot containment, CPU write error propagation, no-turbo clamping).
   - Устранить `CORE-01..02`, `CFG-01..04` в `crates/ucc-core` и `crates/ucc-config` (pump steps, durable atomic backup, charging thresholds validation).
   - Устранить `IPC-01..04` и `CLI-01..04` в `crates/ucc-dbus` и `crates/ucc-cli` (polkit fallback, owner identity, timeouts, CLI `-v`).
2. **W2 (Frontend):**
   - Устранить `GUI-01..03` в `crates/ucc-gui` (устранение demo-curve side effects, стабильность точек кривых вентилятора).
   - Устранить `DESK-01` в `crates/ucc-desktop` (подтверждение применения настроек из трея).
3. **R1 (Parity Reviewer):**
   - Проведение паритетного аудита правок против C++ baseline на каждом кандидатном SHA.
   - Актуализация `state.json`, `feature-status.json`, `dbus-status.json`, `findings.json` и `RESUME.md`.
4. **R2 (Test Engineer):**
   - Валидация воспроизведения 14 consolidated probes и 4 NVML/sysfs probes.
   - Обеспечение изоляции параллельного test suite (`TEST-01`).
