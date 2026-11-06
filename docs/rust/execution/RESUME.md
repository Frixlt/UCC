# Состояние миграции на Rust (Frixit Control Center)

## Статус: Этап V0 (Подтверждённый эталон) ПОЛНОСТЬЮ ЗАВЕРШЁН

- **Integration Branch:** `rust/migration` (`93d4e1d44a6ca2eb28e76b9742f786ae787e69f0`)
- **Базовый коммит:** `45921b0b7021448a81575a840b2b77d93da8b298`
- **Режим выполнения ролей:** Последовательный fallback с сохранением полной изоляции воркдеревьев, веток и отчётов (`.worktrees/w1`, `.worktrees/w2`, `.worktrees/r2`), так как прямой вызов API субагентов блокируется по географическому признаку (`400 Location not supported`).

## Результаты этапа V0

1. **M0.1 / M0.2:** Сохранена исходная база и снята статическая инвентаризация 453 файлов (`docs/rust/BASELINE_INVENTORY.md`, `docs/rust/baseline-manifest.json`).
2. **M0.3:** Созданы ветка интеграции `rust/migration` и три выделенных git worktrees:
   - `.worktrees/w1` (ветка `rust/w1`)
   - `.worktrees/w2` (ветка `rust/w2`)
   - `.worktrees/r2` (ветка `rust/r2-tests`)
3. **M0.4 (W1):** Успешно собраны обе конфигурации C++ baseline (компилятор GCC 16.2.1, стандарт C++20):
   - Normal build: 85/85 целей скомпилированы и слинкованы (`uccd`, `ucc-gui`, `ucc-cli`, `libucc-dbus.so.0.3.0`, `ucc-desktop-tray`, `ucc-session-agent`, `ucc-hardware`, `ucc-sleep`, `ucc-import-check`).
   - Read-only preview: собрано с `-DUCC_READ_ONLY_PREVIEW=ON` и `-DBUILD_TRAY=OFF`.
   - Доказательства: `artifacts/rust-migration/baseline-build/M0.4-report.json`.
4. **M0.5 (R2):** Воспроизведены существующие тесты:
   - CTest: **40 из 40 тестов пройдены успешно (100% pass)**, время прогона 92.12 сек.
   - Python unit tests: 28/28 сценариев пройдены, сбои `test_import_check.py` устранены указанием скомпилированного бинарника.
   - Node.js suite: `test_gnome_launch.cjs` и `test_gnome_refresh.cjs` пройдены успешно.
   - Зарегистрированы блокировки окружения: `test_gnome_async.js` (`gjs` отсутствует в Hyprland), `test_ite8291_brightness.py` (требует внешний путь к `tuxedo-drivers`).
   - Доказательства: `artifacts/rust-migration/baseline-tests/M0.5-report.json`.
5. **M0.6 (W2):** Снята D-Bus интроспекция и подготовлены sanitized-фикстуры:
   - Сгенерирован официальный XML: **ровно 122 метода и 7 сигналов** (`artifacts/rust-migration/dbus-introspection/de.uniwill.uccd.xml`).
   - Подготовлены обезличенные фикстуры: `capabilities.json`, `profiles.json`, `settings.json`, `telemetry.json` (`artifacts/rust-migration/fixtures/M0.6-report.json`).
6. **M0.7 (W2):** Инвентаризированы 10 разделов GUI и снята reference-галерея:
   - **22 эталонных изображения** захвачены (11 в теме Light, 11 в теме Dark) через тест `renderReleaseGallery` в `artifacts/rust-migration/gallery-reference/`.
   - Зафиксированы все контролы, действия и сопоставления страниц в `docs/rust/execution/gui-status.json`.
   - Доказательства: `artifacts/rust-migration/gallery-reference/M0.7-report.json`.
7. **M0.8 (R1):** Проведён независимый аудит F01–F44 и контрактов:
   - 122 метода и 7 сигналов из `gemini-dbus-contracts.json` на 100% совпадают с реальным XML (0 расхождений).
   - Классифицированы 44 области: 36 обязательных функций ядра, 3 с ожиданием реального железа (F11 ioctl, F13/F14 BLE water cooler), 5 подтверждённых дефектов legacy (`DIFF-01` .. `DIFF-05`), 1 решение по интеграции оболочки (F37).
   - Доказательства: `artifacts/rust-migration/baseline-review/M0.8-report.json`.
8. **M0.9 (R2/W1):** Зафиксированы базовые метрики производительности:
   - Размер пакета: 1.5 МБ (`.pkg.tar.zst`).
   - Время старта C++ CLI: среднее 2.39 мс.
   - Доказательства: `artifacts/rust-migration/baseline-metrics/M0.9-report.json`.

---

## Следующий шаг: Волна V1 (Технические прототипы и Cargo workspace)
1. **M1.1 (W1):** Создать корневой `Cargo.toml` с workspace структурой (`crates/ucc-core`, `crates/ucc-config`, `crates/ucc-hardware`, `crates/ucc-dbus`, `crates/uccd`, `crates/ucc-cli`, `crates/ucc-desktop`, `crates/ucc-gui`), зафиксировать MSRV (Rust 1.99) и зависимости.
2. **M1.2 (W2):** Прототип интерфейса на Iced (Fluent shell, кривые вентиляторов, таблица устройств, живой график).
3. **M1.6 (W1):** Harness для проверки ABI ioctl запросов `/dev/tuxedo_io`.
4. **M1.7 (R2):** Тестовый стенд Fake BlueZ для проверки GATT water cooler.
