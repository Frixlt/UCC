# Состояние миграции на Rust (Frixit Control Center)

## Статус: Этап V0-repair ЗАВЕРШЁН, переход к волне V1

- **Базовый коммит исходников:** `45921b0b7021448a81575a840b2b77d93da8b298`
- **Ветка интеграции:** `rust/migration`
- **Проверенный коммит эталона:** `582dd4a`
- **Режим исполнения ролей:** Последовательный fallback `single_operator_role_queues` с полной изоляцией воркдеревьев (`.worktrees/w1`, `.worktrees/w2`, `.worktrees/r2`) и раздельными свидетельствами.
- **Статус ревью:** `same_actor_self_check; independent_review_pending` (внешние независимые ревьюеры ожидаются при наличии доступа к subagents).

---

## Итоги V0-repair

1. **Фикстуры (M0.6):**
   - Устранено несоответствие схеме `SettingsManager`: создана фикстура `personal-import-settings.json` со всеми обязательными секциями (`stateMap`, `profiles`, `powerDevices`, `lightbar`, `keyboardLayout`, `keyboardBacklightStates`).
   - Проверено и подтверждено утилитой `ucc-import-check`: статус `PASS` (`{"devices":0,"keyboardZones":3,"ok":true,"profiles":3}`).
   - Сохранён воспроизводящий старый дефект файл `reproducer-rejected-old-settings.json` (возвращает `exit 1: key powerDevices not found`).
   - Созданы аутентичные D-Bus фикстуры телеметрии по реальным C++ форматам (`telemetry-cpu-power.json`, `telemetry-dgpu-info.json`, `telemetry-igpu-info.json`, `telemetry-fan-data.json`).
   - Удалён пустой `introspection.xml`, статус интроспекции честно обозначен: `static_contract_verified; live_private_bus_introspect_pending`.
2. **Матрица F01–F44 и Intentional Differences (M0.8):**
   - Полностью исправлена классификация и нумерация F01–F44 по разделу 8 `MIGRATION_PLAN.md` (F05 = App requests, F11 = Device matrix, F13 = NVIDIA power, F14 = CPU/GPU cooling, F15–F17 = Cooler transport/recovery/controls, F18 = Keyboard, F20 = Lightbar, F27 = Webcam/Fn Lock, F37 = Appearance, F42 = Shell interfaces).
   - Все 44 области имеют статус `requirement: required`. 39 областей доступны для автономных тестов, 5 помечены `hardware_pending` (F11, F12, F15, F16, F17).
   - Зафиксированы 5 подтверждённых дефектов legacy (`DIFF-01` .. `DIFF-05`) с привязкой к реальным путям.
3. **Проверки Preview и счётчики тестов (M0.5):**
   - Собраны и прогнаны тесты в режиме `UCC_READ_ONLY_PREVIEW=ON`.
   - Подтверждена работа защит preview: `InstanceTest::previewUsesSeparateSessionIdentity` прошёл успешно, блокировка аппаратных записей подтверждена (в `test_gui_audit` и `test_app_logging`).
   - Честно зафиксированы 2 ожидаемых отказа в preview-тестах (`test_system_monitor` и предсуществующая директория логов).
   - Счётчики тестов приведены в строгое соответствие: 40 CTest suites (40/40 normal pass, 38/40 preview pass), 13 python files (33 distinct passed test cases, 2 environment_blocked), 2 Node.js scripts passed.
4. **Инвентаризация GUI (M0.7):**
   - Восстановлены реальные 10 разделов GUI: Overview, Profiles, Cooler Settings, Watercool Settings, Monitor, Keyboard & Hardware, About, Power & Devices, Lightbar, Battery & Processes.
   - Зафиксированы 22 эталонных скриншота (11 Light + 11 Dark) в `artifacts/rust-migration/gallery-reference/`.
   - Флаги верификации контролов привязаны к конкретным пройденным тестам `test_gui_audit`.
5. **Метрики производительности (M0.9):**
   - Убраны неподтверждённые проценты и гипотетические диапазоны.
   - Зафиксированы факты: CLI `--help` среднее время 2.39 мс (10 замеров: min 2.12 мс, max 2.68 мс, stddev 0.17 мс); размер сжатого пакета: 1.5 МБ.

---

## Текущий этап: Волна V1 (Технические прототипы и Cargo Workspace)

1. **W1 (M1.1):** Создание минимального Cargo workspace (`crates/ucc-core`, `crates/ucc-config`, `crates/ucc-hardware`, `crates/ucc-dbus`, `crates/uccd`, `crates/ucc-cli`, `crates/ucc-desktop`, `crates/ucc-gui`), закрепление stable toolchain, MSRV (1.85+) и общих зависимостей.
2. **W1 (M1.6):** Harness для проверки ABI ioctl запросов `/dev/tuxedo_io`.
3. **W2 (M1.2):** Прототип GUI на Iced (Fluent shell, канвас кривых вентиляторов, таблица устройств, график телеметрии).
4. **R2 (M1.7):** Тестовый стенд Fake BlueZ для проверки GATT water cooler.
