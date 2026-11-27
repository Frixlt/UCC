# Полный переход Frixit Control Center на Rust

Дата: **7 ноября 2026 года**. Статус: **план реализации после изучения текущего проекта; переписывание ещё не начато**.

База сохранена и отправлена в `origin/codex/frixit-control-center`: [`45921b0`](https://github.com/Frixlt/UCC/commit/45921b0b7021448a81575a840b2b77d93da8b298). Этот коммит включает все 42 изменённых и новых файла, существовавших перед подготовкой плана. Исторический `ControlCenter-Python` чистый, commit `ba6fd4a`.

Связанные материалы:

- [Исполняемое задание для Gemini: четыре роли 2+2, этапы, тесты и реестры](GEMINI_START_HERE.md). Это дополнение по организации реализации и полной проверке, подготовленное по отдельному указанию пользователя.
- [Инвентаризация компонентов, файлов, 122 D-Bus-методов и 7 сигналов](BASELINE_INVENTORY.md).
- [Машиночитаемый манифест исходной базы с SHA-256](baseline-manifest.json).
- [Текущие правила сборки](../BUILDING.md), [проверки и известные ограничения](../TESTING.md), [происхождение проекта](../../PROJECT_ORIGINS.md).

## 1. Результат, к которому идём

Переписать весь фоновый системный и пользовательский стек на Rust: системный демон, аппаратные контроллеры, политики, хранение настроек, IPC-клиент/сервер, CLI, самостоятельный трей (`ucc-desktop-tray`), `session agent`, `sleep`/helper-программы, импортеры, используемые инструменты сопровождения и в перспективе собственный `uwcc` (`M14`). Окно настроек `ucc-gui` по пересмотренному [`ADR-01`](execution/decisions/ADR-01-gui-toolkit.md) (модель `frixit-huion`) остаётся автономным D-Bus-клиентом на `C++20 / Qt6::Widgets + Qt6::Charts` без `KF6`, `QML`, `Qt6::Bluetooth` и `libucc-dbus.so`, запускаемым только по требованию и полностью выгружающимся из памяти при закрытии (0 фоновых процессов C++/Qt).

Переход считается завершённым по возможностям и проверкам, а не по факту появления нового окна. Временный мост к старому демону допустим на промежуточном этапе, но не является конечной архитектурой.

Приложение остаётся Linux control center. Ядро Linux, BlueZ, systemd, Polkit, графические драйверы и поставляемые NVIDIA библиотеки остаются внешними системными компонентами. На этапах `M0–M13` внешний `tuxedo-drivers`/DKMS сохраняется как совместимый базовый бэкенд, а последующий этап **`M14`** ([`ADR-05`](execution/decisions/ADR-05-uwcc-platform-backend.md)) реализует переход на собственный открытый Rust `uwcc` (модуль ядра `/dev/uwcc` + userspace `UwccBackend` + прямой `hidraw` для подсветки). «Весь центр на Rust» не означает переписывание всего ядра Linux или сторонних графических драйверов.

Архив Python используется как первоисточник поведения и входных форматов. Его собственный HTTP/FastAPI/Unix socket/SSE API не восстанавливаем: текущие интеграции UCC используют D-Bus.

### Особая граница GNOME и Plasma (`ADR-03`, принято)

По обновлённому [`ADR-03`](execution/decisions/ADR-03-shell-embedding-policy.md) единственным треем в поставке является самостоятельный Rust SNI-трей (`ucc-desktop-tray` на `ksni`/`zbus`), работающий в Hyprland/Noctalia, KDE Plasma, Waybar/Sway и GNOME (при наличии AppIndicator/SNI host). Устаревший C++/QML Plasma-апплет `ucc-tray` окончательно исключается из целевой сборки (в том числе для полного отказа от пакетов `KF6`/Plasma). Внешнее GJS-расширение `ucc-gnome` не входит в обязательный Rust-runtime и сохраняется только как необязательный внешний D-Bus-клиент.

## 2. Что находится в текущем проекте

Основной проект — `UCC-Revorked`, объединённый Frixit Control Center 0.3.0, C++20/Qt 6. Архив `ControlCenter-Python` содержит исторические daemon/FastAPI, PyQt6 GUI, два трея и отдельный Lightbar.

В основной runtime-части, включая shell-интерфейсы и общие заголовки, около **47 тысяч строк** кода с комментариями. Ещё около 4,9 тысячи строк в `tests`, инструменты миграции, рецепты дистрибутивов, ресурсы и исследовательские материалы. Это оценка размера по файлам, а не оценка трудозатрат.

| Компонент | Текущее назначение | Цель на Rust |
|---|---|---|
| `uccd` | Root daemon, D-Bus, workers, оборудование, профили | `uccd`, предметные библиотеки и аппаратные адаптеры |
| `uccd/3rdparty/tuxedo_io_lib` | C++ доступ к `/dev/tuxedo_io`, Clevo/Uniwill/Dummy | Собственный Rust ioctl backend с совместимым ABI |
| `libucc-dbus` | Общий Qt D-Bus client, signals, preview guard | Внутренний статический D-Bus-клиент внутри `ucc-gui` (публичная `libucc-dbus.so` удаляется) |
| `ucc-gui` | Fluent Qt Widgets/Charts, редакторы, история уведомлений | Автономный `C++20 / Qt6::Widgets + Qt6::Charts` клиент поверх Rust `uccd` без `KF6`/`QML`/`Bluetooth` (`ADR-01`) |
| `ucc-cli` | Команды, JSON, live monitor, совместимость скриптов | Rust CLI с сохранением команд и кодов выхода |
| `ucc-desktop` | Tray, батареи, NVIDIA-процессы, pidfd termination | Rust desktop library + отдельный Rust tray binary |
| `ucc-session` | Уведомления, Lightbar relay, KDE/Hyprland refresh | Rust session agent |
| `ucc-tray` | Необязательный Plasma applet и Qt backend | Исключается из целевой поставки (`ADR-03`); функции полностью покрыты Rust `ucc-desktop-tray` |
| `ucc-gnome` | GJS extension с быстрыми аппаратными контролами | Внешний необязательный D-Bus адаптер без C++-зависимостей (`ADR-03`) |
| `contrib/frixit`, `contrib/mechrevo` | Offline import, deploy/rollback, драйверные патчи | Rust importer/maintenance tools; драйверные материалы отдельно |
| `tests` | QtTest, Python, Node/GJS, fake sysfs/procfs/D-Bus | Rust unit/integration/contract/UI tests + fixtures |
| Build/package | CMake, Arch, DEB, RPM, Nix, CI | Cargo workspace + distro recipes + Rust xtask |

### Последние изменения уже входят в базу

- Hyprland refresh backend через `hyprctl`, уведомления о физическом источнике питания.
- Частота экрана внутри профиля с автоматическим применением агентом.
- Общая зарядка ноутбука отдельно от профилей производительности.
- Индивидуальный выбор логических CPU ID, P/E topology, сохранение обязательного CPU.
- i915/xe telemetry и снижение частоты NVML-проб при разрешённом runtime suspend.
- Выключение NVIDIA persistence при отмене Keep awake.
- Исправления drag редактора вентиляторов, асинхронного мониторинга и CI.
- **Пост-базовые изменения C++ (ноябрь 2026), подлежащие переносу в Rust:**
  - восстановление частоты встроенной панели Hyprland после готовности DPMS (`ucc-session`);
  - настраиваемые действия при закрытии крышки отдельно для `AC`/`Type-C`/`Battery` (`powerDevices.lid`, ингибитор `handle-lid-switch`, отложенный переход в сон при отключении питания, DPMS только встроенной панели в Hyprland) и строгий режим пробуждения только от крышки и встроенной клавиатуры (`WakeupPolicy` в `uccd` и `ucc-sleep`).

Ориентироваться только на README от 22 октября недостаточно: он описывает не все изменения из базы 7 ноября.

Наличие файла также не означает, что он входит в текущий runtime. Основной GUI собирается из Qt Widgets sources; `ucc-gui/qml` не является его активным основным интерфейсом. `uccd/3rdparty/tuxedo_io_napi.cc` не входит в текущий daemon target: действующий `tuxedo_io_lib` подключается как INTERFACE library с заголовками. Исследовательские C-пробы NVIDIA учитываются в инвентаризации, но не становятся обязательными production-процессами новой версии.

## 3. Подход к переносу

Конечная цель — полная замена приложения. Реализация идёт отдельными проверяемыми слоями:

1. Зафиксировать контракты и эталонные входы/выходы текущего центра.
2. Проверить самые неопределённые технологии: GUI/Wayland/tray, ioctl ABI, Bluetooth/NVIDIA границы.
3. Перенести чистые модели, политики и сериализацию.
4. Перенести D-Bus и CLI, затем аппаратные адаптеры и демон.
5. Перенести все desktop-функции и GUI.
6. Подготовить обновление, откат, аппаратную приёмку.
7. Удалить старые runtime-компоненты после достижения равенства по утверждённой матрице.

В ходе разработки можно сравнивать старые и новые компоненты на одних fixtures. На живом оборудовании одновременно работает **один** управляющий демон. Для теневого сравнения Rust получает сохранённую телеметрию или read-only источник, а не начинает второе управление вентиляторами, CPU, подсветкой или BLE.

Демон и GUI можно разрабатывать независимо благодаря сохранению D-Bus. При промежуточной проверке Rust-клиента со старым демоном его право записи должно включаться только в специально выбранном сценарии.

## 4. Предлагаемый стек и проверка его пригодности

Это проектное предложение, а не утверждение, что выбранные библиотеки уже проверены на данном ноутбуке. Точные стабильные версии и MSRV фиксируются в M1; использование `latest` или ветки разработки в production не допускается.

| Область | Предложение | Причина и ограничение |
|---|---|---|
| Workspace | Cargo, Rust edition 2024, закреплённый stable toolchain | Единая сборка бинарников и библиотек; версия выбирается по зависимостям |
| Async daemon | Tokio, bounded channels, cancellation | События питания, D-Bus, BLE, таймеры; блокирующее оборудование выносится отдельно |
| D-Bus | `zbus`/`zvariant` | Сервер, клиент, signals; имена и сигнатуры задаются явно |
| Модели/JSON | `serde`, `serde_json`, `uuid`, `thiserror` | Одна модель данных вместо нескольких ручных сериализаторов |
| GUI | **Автономный C++20 / Qt6::Widgets + Qt6::Charts** | По образцу `frixit-huion` (`ADR-01`): 0 МБ Qt в фоне, без `KF6`/`QML` и без пробуждения dGPU от `wgpu`; стабильный Wayland/HiDPI/IME |
| GUI прототип (отклонён) | Iced + wgpu | Отклонён в `ADR-01` (пересмотр 2026-11-26): частые ломки API до 1.0, риск пробуждения NVIDIA через Vulkan, ~150+ лишних крейтов |
| Tray | `ksni` / StatusNotifierItem + D-Bus menu | Rust tray; отдельно проверяются KDE activation token и host в других DE |
| Bluetooth | `bluer` или прямой BlueZ D-Bus через `zbus` | Выбор после проверки GATT, reconnect и runtime-зависимостей |
| NVIDIA | `nvml-wrapper` для покрытых API; `libloading` для узкого FFI | Нужна карта покрытия текущего wrapper, особенно voltage/OC |
| Linux IO | `rustix`/`nix` и минимальный `libc`, udev по необходимости | OwnedFd, pidfd, ioctl, sysfs; unsafe изолирован |
| CLI | `clap`, совместимый formatter | Стандартный parser не должен случайно менять текущие aliases/exit codes |
| Diagnostics | `tracing`, journal/stderr и file rotation | Сохранение пользовательского переключателя, ограничений логов и ошибок |
| Проверки | Cargo tests, property tests, fuzz targets, fake backends | Отдельно модель, transport, аппаратная граница, GUI и установка |
| Maintenance | `cargo xtask` / `ucc-migrate` | Убрать используемые Python-инструменты из финального рабочего процесса |

Поддержка Canvas в Iced подтверждается [документацией](https://docs.rs/iced/latest/iced/widget/canvas/). Это подтверждает механизм рисования, но не готовность наших сложных редакторов или доступность всех нужных Wayland API. [Официальная документация Iced](https://docs.iced.rs/iced/) также прямо обозначает экспериментальный статус библиотеки; поэтому M1 предшествует массовому переносу UI.

[zbus](https://docs.rs/zbus/latest/zbus/) поддерживает async D-Bus. Его [interface macro](https://docs.rs/zbus/latest/zbus/attr.interface.html) по умолчанию допускает параллельную обработку вызовов. Порядок аппаратных операций обеспечиваем собственным command actor, а не предположением о последовательности D-Bus.

[BlueR](https://docs.rs/bluer/latest/bluer/) предоставляет Rust API BlueZ/GATT. [ksni](https://docs.rs/ksni/latest/ksni/) реализует StatusNotifierItem и имеет API текущего XDG activation token. Наличие этих API не заменяет проверку пути tray → IPC → уже открытое окно в KDE/Hyprland.

## 5. Целевая структура

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
crates/
  ucc-core/          профили, типы, capabilities, power policy, ошибки
  ucc-config/        JSON/legacy settings, атомарная запись, backup, import models
  ucc-dbus/          совместимый API, proxies, DTO и signals
  ucc-hardware/      sysfs/procfs/ioctl, PlatformBackend (TuxedoIoBackend + UwccBackend), CPU/GPU/charging/display, FFI
  ucc-cooling/       алгоритмы вентиляторов и кривых
  ucc-cooler/        BLE protocol, connection state machine, command queue
  ucc-lighting/      клавиатура, LED identity, Lightbar, анимации и LightingTransport (sysfs + hidraw)
  ucc-telemetry/     выборки, freshness, история и legacy binary codec
  ucc-desktop/       батареи, процессы, notifications, display session backends
kernel/
  uwcc-rs/           собственный Rust-модуль ядра (/dev/uwcc, DKMS, этап M14 / ADR-05)
apps/
  uccd/             системный демон (Rust)
  ucc-cli/          CLI (Rust)
  ucc-desktop-tray/ самостоятельный SNI tray и popup (Rust)
  ucc-session-agent/ (Rust)
  ucc-sleep/         (Rust)
  ucc-hardware/     аварийный firmware fan-auto helper (Rust)
  ucc-import-check/ (Rust)
  ucc-migrate/      offline conversion и deployment maintenance subcommands (Rust)
ucc-gui/            автономное окно настроек (C++20 / Qt6::Widgets + Qt6::Charts D-Bus client, без KF6/QML)
xtask/
tests/
  fixtures/         обезличенные старые settings, DBus/JSON/BLE/metrics samples
  contract/         старый клиент ↔ новый сервер и обратное направление
  integration/      private bus, temporary sysfs/procfs, fault injection
  ui/               input scenarios и reference images
packaging/
  systemd/ dbus/ polkit/ desktop/ arch/ debian/ rpm/ nix/
```

Это логические границы. Маленькие библиотеки можно объединить, если появится лишняя связность; главное — сохранить разделение UI, policy, IO и IPC. `ucc-core` не зависит от Qt, Iced, D-Bus, sysfs и async runtime. Переиспользовать C++ через постоянный FFI-мост в финальном продукте не планируется.

### Процессы и полномочия

```mermaid
flowchart TB
  GUI["ucc-gui · desktop user · Qt6::Widgets D-Bus client (on demand)"] --> BUS["system D-Bus · совместимый API"]
  CLI["ucc-cli · user · Rust"] --> BUS
  TRAY["ucc-desktop-tray · user · Rust"] --> BUS
  AGENT["ucc-session-agent · user · Rust"] --> BUS
  BUS --> AUTH["Polkit / проверка вызывающего"]
  AUTH --> DAEMON["uccd · root · Rust"]
  DAEMON --> IO["ioctl / sysfs / CPU / GPU / LEDs / battery"]
  DAEMON --> BLE["BlueZ · BLE cooler"]
  AGENT --> SESSION["session D-Bus / KScreen / Hyprland / X11"]
  TRAY --> ACT["GUI activation endpoint + token"]
  SLEEP["systemd → ucc-sleep"] --> DAEMON
  STOP["ExecStopPost → ucc-hardware --fan-auto"] --> IO
```

Root daemon не получает UI-зависимости. Пользовательские процессы не пишут аппаратные sysfs напрямую. Уведомления, завершение пользовательских процессов, выбор режима compositor и настройки внешнего вида остаются в пользовательской сессии.

## 6. Модель исполнения демона

- Один orchestrator владеет желаемым состоянием: источник питания, override, profile assignments, активный профиль, разрешённые per-mode overlays, cooler state.
- Команды изменения состояния идут через ограниченную очередь с ответом. Каждая команда имеет revision/generation; устаревшие результаты не перезаписывают новые настройки.
- Аппаратные блокирующие операции исполняются выделенным worker/actor. Его очередь и задержка ограничены; отмена async future сама по себе не считается отменой ioctl или чужой native library.
- Вентиляторы получают отдельный предсказуемый цикл, не зависящий от медленного Polkit, UI, BLE discovery или NVIDIA.
- BLE владеет собственной state machine и единой очередью UART writes. GUI и fan controller не отправляют конкурирующие команды напрямую.
- Клиенты читают immutable snapshots; сериализация и построение графиков не держат hardware lock.
- На отключение, reset, hotplug и restart существуют явные переходы. В логах видны запрошенное значение, подтверждённое значение и причина ошибки.
- Shutdown прекращает приём команд, останавливает циклы записи, восстанавливает firmware fan control, возвращает CPU в исходное допустимое состояние, закрывает BLE/NVML и только затем освобождает service name.
- Внешний `ExecStopPost` сохраняется для crash/SIGKILL. Rust Drop не срабатывает при SIGKILL и не заменяет аппаратный recovery helper.

Применение профиля включает несколько неатомарных sysfs/ioctl/BLE действий. Поэтому сначала проверяется весь запрос, затем строится упорядоченный план операций. При частичном отказе возвращается отчёт по контроллерам; выполненные действия не объявляются автоматически отменёнными. Компенсация разрешена только для операций с известным безопасным исходным состоянием. Повторный Apply идемпотентен по возможности; потерявший актуальность job не продолжает запись после нового профиля. Для зарядки start/end limits, CPU min/max и GPU wake/power фиксируется отдельный порядок действий.

Периоды текущей базы используются как начальная точка: fan/service tick 1 с, hardware monitor около 800 мс, CPU validation 10 с, display worker 3 с, GUI metrics 2 с. После переноса их меняют только по измерениям и тестам, с отдельным учётом энергопотребления.

## 7. Контракты совместимости

### 7.1 D-Bus

Сохраняются:

- System service/interface `com.uniwill.uccd`, object path `/com/uniwill/uccd`.
- Все **122 объявленных метода**, **7 сигналов**, регистр имён, порядок аргументов и D-Bus-типы.
- JSON как D-Bus `s`, byte arrays как `ay`, карты вентиляторов как `a{sv}` с вложенными timestamp/data.
- Существующие boolean results, void methods, ошибки transport и поведение при отсутствии сервиса.
- Owner lifecycle для power requests: ключ `(unique sender, name)`, удаление после `NameOwnerChanged`/disconnect; нельзя доверять имени владельца из JSON клиента.
- Совместимость старого C++ GUI/CLI с Rust daemon на переходном этапе и Rust-клиентов со старым демоном в пределах реально работающего API.
- Session endpoint `com.uniwill.UccGui`, `/com/uniwill/UccGui`, `Activate`/`ActivateWithToken`; отдельные имена preview. Tray singleton также сохраняет смысл существующего session service.

Автоматическая таблица методов находится в инвентаризации. Перед реализацией снять XML introspection и эталонные ответы на **изолированной** шине: парсинг заголовка не доказывает фактический runtime export. Каждый метод получает fixture, уровень авторизации, допустимые диапазоны, побочные эффекты и failure case.

Внутри Rust используются типы `Result` и структурированные ошибки. На совместимой внешней границе они преобразуются в прежний контракт. Новый расширенный API, если понадобится, добавляется отдельно и версионируется; существующие имена не переопределяются молча.

Сохранение D-Bus wire API не означает сохранение C++ ABI `libucc-dbus.so`. Старый уже собранный Plasma plugin продолжает зависеть от этой библиотеки: её нельзя удалить из промежуточного пакета, пока такой plugin ещё устанавливается. Финальное удаление библиотеки связано с выбранным новым shell backend. Для Polkit предпочтителен subject реального system-bus unique name; при использовании process subject нужны достоверные credentials/start time, а не доверие PID с неопределённым временем старта. Правила прав доступа и отказа фиксируются тестами, legacy-недочёты не переносятся как требование безопасности.

### 7.2 Файлы и настройки

| Объект | Что нужно сохранить |
|---|---|
| `/etc/ucc/settings` | stateMap, profiles, customKeyboardProfiles, customFanProfiles, keyboard layout/states, powerDevices, lightbar, global charging, flags, workaround fields |
| `/etc/ucc/settings.backup` | Восстановление при повреждении основной версии; backup не заменяется испорченными данными |
| `/etc/ucc/autosave` | displayBrightness и совместимость старого файла |
| `/etc/ucc/aquaris.json` | Saved BLE identity/LED state; приватные права и проверка формата |
| `/run/ucc/sleep-state.json` | version, wasActive, manualMode, coolerEnabled; состояние живёт до reboot |
| `~/.config/uccrc` | Группа MonitorTab: выбранные серии, unified mode, time window |
| QSettings `UniwillControlCenter/desktop` | startMinimized, refreshRateNotifications |
| QSettings `UniwillControlCenter/appearance` | themeMode: Light/Dark/Auto |
| QSettings `UniwillControlCenter/diagnostics` | saveLogs |
| Исторический Python Data | config, keyboard_colors, lightbar, gui_settings через offline importer |

Пути QSettings нужно подтвердить на fixture с учётом `XDG_CONFIG_HOME`; нельзя механически считать их одним `uccrc`. Сначала читаем legacy INI-формат, не меняя пользовательские значения. Для нового user-config допустим однократный импорт с отчётом и marker, но только при сохранении отката.

JSON читается через typed models плюс сохранение неизвестных полей там, где нужен round-trip. Значения `null`, отсутствие поля, `-1`/`-999`, пустая строка и 0 различаются на legacy-границе. Profile IDs сохраняются как строки: built-in IDs не обязательно UUID.

Запись settings: validate → сериализация в память → временный файл в том же каталоге → корректные права → flush/fsync → atomic rename → fsync каталога. Отдельно проверяются concurrent writes, symlinks, полный диск, отказ прав и авария между шагами. Миграция формата не должна делать старый пакет неспособным прочитать восстановленную резервную копию.

### 7.3 История метрик

Старый `GetMonitorDataSince` передаёт блоки **native endian**, только для local IPC:

```text
u8 metric_id
u32 point_count
point_count × (i64 timestamp_ms + f64 value)
```

Перенести точный codec без `transmute` и выравнивающих предположений: разбор через byte arrays, `from_ne_bytes`, проверку длины и переполнений. Сохранить порядок 10 MetricId и фильтр timestamp `>= since`. Horizon: default 1800 с, минимум 60 с, максимум 7200 с. Новый portable формат может появиться лишь в другом API.

Таймеры используют monotonic clock, экспорт timestamp остаётся Unix milliseconds. Для history отдельно решается скачок wall clock: текущее предположение о сортировке не превращать в скрытую ошибку binary search.

## 8. Полная функциональная матрица

Каждая строка должна иметь Rust implementation, автоматические проверки и, где указано, аппаратную приёмку. «Не поддерживается железом» — допустимый подтверждённый capability result; имитация успеха — нет.

| ID | Область | Обязательное поведение при переносе | Основные исходники/эталоны |
|---|---|---|---|
| F01 | Определение устройства | DMI, Clevo/Uniwill/Dummy, exact Mechrevo tuple, near misses, реальные capabilities | SystemInfo, identifyDevice, DriverHealth, device tests |
| F02 | Источник питания | Учитывать все supplies; USB-C имеет приоритет при одновременном Mains; отсутствие данных отличается от battery | StateUtils, power_policy tests |
| F03 | Эффективный режим | Auto/AC/Type-C/Battery; manual выше requests; requests только повышают режим; physical source показан отдельно | PowerPolicy |
| F04 | WC profile state | Water-cooler profile при разрешённом auto-выборе; override меняет profile state согласно текущей policy | PowerPolicy, startup/profile tests |
| F05 | App requests | 256 всего/32 на owner, имя 128/reason 512, ownership, disconnect cleanup | PowerPolicy, service watcher |
| F06 | Назначения профилей | power_ac/power_type_c/power_bat/power_wc; Type-C fallback на AC; отсутствие назначения не создаёт произвольный профиль | TccSettings, applyProfileForCurrentState |
| F07 | Профили | Built-in/custom, create/copy/rename/delete, references, Save отдельно от Apply, temporary apply/revert | Два ProfileManager, MainWindow, CLI |
| F08 | CPU policy | governor/EPP/turbo/boost/HWP, defaults, частоты в правильных единицах, driver quirks | CpuController/CpuWorker |
| F09 | CPU selection | Точные sparse IDs, enabledCores выше onlineCores, обязательный CPU, P/E/unknown, enable-before-disable | CpuTopology, CPU tests |
| F10 | CPU validation | Ограниченные reapply attempts при конфликтующем сервисе, reset на новом профиле, восстановление исходного CPU состояния | CpuWorker |
| F11 | Device matrix | Независимые ac/type_c/bat значения, overlay CPU поверх профиля, rescan сохраняет отсутствующие устройства | PowerDeviceController/PowerDevicesTab |
| F12 | System controls | PCI/USB runtime PM, ASPM, Wi-Fi power save, audio powersave, NMI watchdog, writeback, только разрешённый nvidia-powerd | PowerDeviceController |
| F13 | NVIDIA power | Keep awake выше runtime auto; wake до power operations; undo persistence при выключении; per-device errors | PowerDeviceController/ProfileSettingsWorker |
| F14 | CPU/GPU cooling | CPU/GPU curves, interpolation, EWMA/hysteresis, sameSpeed, min/off limits, temporary/saved curves | FanControlWorker/FanProfile |
| F15 | Cooler transport | LCT models, saved identity, Nordic UART UUID, exact packet encoding, GATT readiness | LCTWaterCoolerWorker/AquarisSelection |
| F16 | Cooler recovery | Discovery/reconnect/backoff/keepalive, late adapter, rotating address при единственном допустимом совпадении, restart/sleep | Bluetooth tests |
| F17 | Cooler controls | Fan 0–100, pump codes 0/2/3/4, LED modes/colors, auto/manual persistence, reconnect restore | Watercooler override tests |
| F18 | Keyboard | White, zone RGB, per-key; physical LED IDs, missing zones, global brightness, batched RGB | KeyboardBacklightController, AssignKeyZones |
| F19 | Личная раскладка | Enter 56, LShift 21, LCtrl 0; 101 physical keys при 126 обнаруженных zones на документированной машине | keyboard-layout.json, GUI calibration tests |
| F20 | Lightbar | Managed flag, независимые power settings, пять анимаций, Auto majority color, Silent/Welcome/Test | LightbarController |
| F21 | Lightbar idle | Flash выключен сразу после Save/start/mode change и между событиями; нет наложения анимаций, welcome не повторяется после resume | Lightbar tests |
| F22 | Display brightness | Реальная активная панель hybrid GPU, MUX/hotplug, rounding, legacy AMD inversion | DisplayBacklight, brightness tests |
| F23 | Display profile | useRefRate/refreshRate сохраняются; profile Hz имеет приоритет над offers, сохраняется разрешение | MainWindow/RefreshAdvisor |
| F24 | Session refresh | KDE реальные mode IDs; Hyprland доступные режимы и сохранение scale/position/color rule; X11 backend | RefreshAdvisor/DisplayWorker |
| F25 | Refresh offers | Только после кнопки уведомления, stale consent отменяется при смене topology/source/sleep; readback после команды | refresh tests |
| F26 | Charging | Общие profile/priority/type/thresholds сохраняются независимо от performance profile; старые поля профиля читаются, не применяются | BatterySettingsWidget/SettingsManager |
| F27 | Webcam/Fn Lock | Capability-aware controls и фактический ответ backend; разрыв SetWebcam сначала фиксируется отдельно | HardwareTab/FnLockController |
| F28 | Telemetry | CPU/RAPL, GPU NVIDIA/AMD/Intel i915/xe, power/clock/temp/fan/util/VRAM/voltage, units/sentinels | HardwareMonitorWorker/NvmlWrapper |
| F29 | Sleeping NVIDIA | Не probe при suspended/unknown; cadence допускает idle timeout при auto; sleeping dGPU не показывает iGPU как замену | GpuSysfs/SystemMonitor tests |
| F30 | History/Monitor | 10 metrics, horizon, grouped/unified charts, zoom/pan/markers/toggles, local prefs, устойчивое обновление | MetricsHistoryStore/MonitorTab |
| F31 | Notifications | Event relay без forwarding/logging текста; rate limit, restart monitor, driver/source alerts, GUI notification history | NotificationMonitor/NotificationCenter |
| F32 | Battery telemetry | Несколько system batteries, исключение device batteries, power_now или current×voltage, корректные µ-units | SessionTelemetry |
| F33 | NVIDIA processes | Только процессы текущего UID, /dev/nvidia fd, bounded list, active-page sampling | SessionTelemetry |
| F34 | Termination | Явное подтверждение, pidfd + startTime/UID recheck, SIGTERM → grace → SIGKILL, preview запрет | ProcessTerminator |
| F35 | Tray | Иконка режима/батареи/manual override, меню, процессы, ошибки, adaptive polling | DesktopTray |
| F36 | Activation | Один GUI, concurrent launch, activation token, existing desktop focus, --show против startMinimized | GuiInstance/WindowActivation |
| F37 | Appearance | Fluent layout, Light/Dark/Auto, масштабирование, выбор страницы, focus/keyboard, optional entrance animations | FluentTheme/Sidebar/Entrance |
| F38 | Diagnostics/About | build/version/commit, credits/licenses, driver banner/recovery, bounded local logs, folder action | AppLogging/AboutPage/DriverHealth |
| F39 | Preview | Ни аппаратных, ни config/profile/user-pref/process writes; отдельный instance ID | PreviewMode, GUI preview tests |
| F40 | Sleep/recovery | Manual/cooler snapshot, stop before NVIDIA sleep, start only if previously active, failed-start retry, private snapshot | SleepState, units, helper |
| F41 | Import/deploy | UCC/TCC/Python, exact LED colors, unresolved report, no guessed limits, offline checks, package upgrade/rollback | contrib и import/deploy tests |
| F42 | Shell interfaces | Все текущие быстрые controls GNOME/Plasma учтены; перенос функций и решение о встраивании отдельно | TrayBackend, extension.js |
| F43 | CLI/API users | Существующие команды, aliases, --json, monitor, codes 0/1/2, completion/man | ucc-cli/main.cpp |
| F44 | Distribution | Arch, DEB/RPM/Nix recipes, service/autostart, driver separation, licenses/assets/source release | Корневые рецепты, CI |
| F45 | Крышка и пробуждение | Независимые действия при закрытии крышки (`screen_off`/`suspend`/`lock`/`hibernate`/`poweroff`/`ignore`) для `AC`/`Type-C`/`Battery`, ингибитор `handle-lid-switch`, отложенный переход в сон при отключении питания, DPMS только встроенной панели в сессии, строгий `WakeupPolicy` (крышка `PNP0C0D` и клавиатура `i8042 KBD`) | PowerDeviceController, LidController, WakeupPolicy, LidDisplayHandler, PowerDevicesTab |
| F46 | Собственный Rust `uwcc` backend (`ADR-05`, `M14`) | Доменный `PlatformBackend` (`TuxedoIoBackend` ↔ `UwccBackend`), Rust-модуль ядра `/dev/uwcc` (`ECRR`/`ECRW`/`SMRW`/`PWUP`/WMI events), EC-менеджер `0x1A` (`RamFan 1.5`, `SMRW`, TCC лимит температуры CPU, `PWUP`, зарядка и переключатели через EC), `uinput` для WMI-клавиш, прямой USB `hidraw` для `048D:600B` (клавиатура) и `048D:7001` / `048D:7000` (Lightbar + подсветка логотипа крышки / logo), взаимоисключение с `tuxedo-drivers` и откат | `ADR-05`, открытая спецификация EC/ACPI `0x1A` (без `proprietary/` и `oem/`) |

## 9. Детали, которые критично сохранить

### CPU и per-mode устройства

Политики применяются в определённом порядке: physical source → manual/request decision → profile state → profile → разрешённый CPU overlay → device policy. Нельзя свести эти состояния к одному enum «battery/performance».

`cpu.enabledCores` — массив логических CPU ID, а не номера строк таблицы. До выключения CPU должны быть успешно включены все требуемые новые CPU. Ошибка включения прекращает последовательность. CPUID P/E probing выполняется только на доступных online CPU, с восстановлением affinity; offline ядра не включаются ради подписи в UI. Кеш топологии сохраняет известные метки.

При turbo off base frequency может отличаться для P/E. Минимум/максимум применяются в допустимом порядке, чтобы sysfs не отклонил переход. Учитываются shared cpufreq policies и отсутствие отдельного cpufreq узла; агрегированные пределы не заменяют проверку конкретного CPU.

При выключении per-mode policy дальнейшие device writes прекращаются, CPU возвращается к выбранному профилю. Текущая база **не откатывает автоматически все значения устройств**. Если захотим добавить восстановление, это отдельное поведение с сохранением исходных значений и тестами.

### Вентиляторы и отказ датчиков

В текущем FanControlLogic есть асимметричные фильтры: temperature EWMA 0,5 вверх/0,15 вниз, hysteresis 3 °C, speed EWMA 0,4 вверх/0,08 вниз. Critical override сейчас использует фильтрованную температуру: от 85 °C минимум 80%, от 90 °C 100%. Отдельный пользовательский preset «100% при 85 °C» — таблица, а не CPU thermal limit.

Проверить округление C++ `std::round` при переносе, особенно полуцелые значения. CPU/GPU curves интерполируются, а pump curve выбирает ступень. GPU fan 2 не объявляется независимым третьим редактируемым вентилятором без подтверждённого аппаратного API.

Изменение fail-safe при пропавшем/устаревшем датчике выделяется отдельно от parity. Требуемый новый результат: потеря всех датчиков не должна превращаться в команду 0% из-за начальных нулей; выбрать firmware auto или проверенный аварийный duty, подтвердить на целевом backend. Быстрое аварийное поведение по raw temperature, если вводится, оформляется явно, без утверждения, что так уже работает старый код.

### Водянка

Переносится конечный автомат Disconnected/Discovering/Reconnecting/Connecting/Connected/Error и дополнительное suspended/disabled состояние. Из базы извлекаются: tick 1 с, fast reconnect timeout 8 с, discovery hard timeout 15 с, connect timeout 12 с, error backoff 5…120 с, очистка fast target после 3 неудач, keepalive 10 с/3 пропуска, gap BLE writes не менее 80 мс.

Точная политика cache purge/adapter reset рассматривается вместе с её воздействием на общий Bluetooth adapter. Нельзя превращать любой локальный timeout в постоянный reset всех Bluetooth-устройств. Изменения recovery-policy документируются как изменения, а не случайные различия библиотек.

Pump code `0` — высокий режим 11V, `2` — 7V, `3` — 8V, `4` — off. Код `1`/12V в проверенной интеграции запрещён. Значение процентов нельзя напрямую послать вместо voltage code. Сохранённые manual fan/pump обновляются только после успешной отправки, восстанавливаются после reconnect, auto-control снимает manual override по существующей семантике.

### NVIDIA

Покрыть обычный NVML и реально используемый NvAPI voltage path. Каталог `nvidia_findings` содержит исследования и probe-бинарники; наличие найденного символа не означает, что он является активной функцией приложения. Его экспериментальные записи не переносятся в production по умолчанию.

Составить таблицу: текущий symbol → Rust wrapper method/FFI → read/write → capability → test. Проверить ABI/version структур NVML v1/v2, fallback symbols, driver-not-present, permission denied, unsupported operations. Объекты библиотеки должны жить дольше function pointers и handles.

Не считать NVML «бесплатным read-only»: probe может продлевать active D0. Сохраняется проверка runtime_status до обращения и интервал около 16 с при `power/control=auto`; при sleep данные очищаются, нулевые реальные измерения не заменяются missing. Отдельно измеряется wake при первичной инициализации. Если native call может зависнуть и мешать shutdown, M1/M7 решают вопрос отдельного GPU helper process с таймаутами; async timeout не прерывает зависший FFI.

### Экран и пользовательская сессия

Новая session architecture должна запускать KDE/Hyprland/X11 commands в сессии пользователя. Старый root DisplayWorker ищет DISPLAY/XAUTHORITY в `/proc`; перенос всей этой механики в root Rust daemon не является обязательным, если совместимый метод маршрутизируется безопасным session backend и поведение сохраняется.

Нужны отдельные проверки explicit profile rate и suggestion mode. Явно сохранённая частота профиля применяется автоматически; предложения без profile preference требуют действия в уведомлении. Topology/source/profile revisions отменяют устаревшие async replies и consent. После успешного exit code сверяется фактический режим. Разрешение, scale, положение и цветовой профиль сохраняются.

## 10. Найденные расхождения: не переносить молча

Это статические наблюдения по исходникам сохранённого коммита. Они требуют воспроизводящих тестов; свежая аппаратная диагностика в рамках составления плана не проводилась.

| Наблюдение | Где | Действие при переносе |
|---|---|---|
| Serializer пишет `keyboardBacklightStates` дважды; последний ключ содержит `[]` | `uccd/inc/SettingsManager.hpp`, `settingsToJSON` | Создать fixture с непустым состоянием; исправить сериализацию, сохранить чтение legacy files, не копировать потерю данных |
| Клиенты вызывают `SetWebcam`, но слот/реализация в адаптере отсутствует | `libucc-dbus/UccdClient.cpp`, `ucc-gnome/uccdClient.js`, адаптер | Разделить чтение capability и запись; восстановить поддержанный hardware path с отрицательным тестом, учесть расширение legacy API |
| GNOME client использует `GetKeyboardBacklightControlEnabled`, адаптер экспортирует Supported | `ucc-gnome/uccdClient.js` | Проверить фактический сценарий; совместимый alias или исправление адаптера, без ложного false |
| Несколько client setters являются заглушками | `libucc-dbus/UccdClient.cpp` | Привязать UI к работающим profile paths; не считать stub существующей backend-возможностью |
| Часть wrappers проверяет только отсутствие D-Bus error, хотя сервер возвращает bool | `callVoidMethod` и использующие его setters | Fixture: reply `false` без transport error; Rust должен показать отказ, а не успех |
| YCbCr parser ограничивает цикл исходно пустым target vector | `SettingsManager::parseSettingsJSONInternal` | Fixture c непустыми cards/ports; решить supported/legacy статус и сохранить данные |
| Несколько serializer paths профиля/settings | `ProfileManager.hpp`, service helpers, GUI builders | Один typed model и legacy adapters; semantic round-trip всех сохранённых полей |
| Nix module имеет собственные units и старую sleep-схему | `nix/nixos-module.nix` против актуальных root units | Свести lifecycle к общему контракту; Nix-store paths и writable sysfs проверить отдельно |
| GNOME/CLI man описывают не весь актуальный API | Документация и текущий dispatcher | Исправить справку по рабочим командам; source/contract fixtures — первичный эталон |
| Полный исторический GUI audit имел timeout | `docs/TESTING.md` | Разделить причины, воспроизвести/устранить; исключённый тест не объявлять прошедшим |
| Настоящий suspend/resume и долгие BLE outages не полностью подтверждены | Текущий отчёт о проверках | Обязательная аппаратная приёмка новой версии; не наследовать «проверено» из synthetic tests |

Все intentional differences получают отдельный список с причиной, regression test и влиянием на откат. Перед массовым differential testing такие различия должны быть известны; иначе исправление старого дефекта будет ошибочно считаться регрессией Rust.

## 11. Этапы реализации и критерии выхода

Статусы ниже синхронизированы с `docs/rust/execution/backlog.json` (всего 139 задач: выполнено 64, заменено ADR-01 — 3, частично выполнено — 26, отложено на сообщество — 2, запланировано — 44). Этап нельзя закрыть заглушкой, если она покрывает обязательную функцию.

### M0 — воспроизводимая исходная база

**Вход:** commit `45921b0`, оба исходных проекта, текущие ресурсы и тесты.

- [x] M0.1 Сохранить все текущие рабочие изменения отдельным коммитом и push.
- [x] M0.2 Снять статическую инвентаризацию компонентов/методов/файлов с SHA-256.
- [x] M0.3 Создать отдельную ветку/worktree для Rust, не переписывая историю базы.
- [x] M0.4 Собрать C++ normal и read-only preview на базе коммита; сохранить toolchain/config/logs.
- [x] M0.5 Запустить текущий набор и сфокусированные GUI scenarios; зафиксировать failures/skips отдельно.
- [x] M0.6 Снять XML D-Bus и обезличенные fixtures settings/profiles/telemetry/capabilities.
- [x] M0.7 Зафиксировать reference images всех 10 разделов и driver-warning в обеих темах; существующая галерея содержит 22 изображения.
- [x] M0.8 Создать реестр F01–F44 со статусами: обязательная функция, аппаратно недоступна, подтверждённый legacy defect, shell decision.
- [x] M0.9 Зафиксировать текущие CPU/RSS/idle wakeups/UI latency/package footprint для последующего сравнения (дополнительно сняты живые Wayland map latency на Hyprland при 1.25x/2.0x и RSS/wakeups/размеры release Rust бинарников в `artifacts/rust-migration/baseline-metrics/M0.9-report.json`).

**Выход:** любой разработчик воспроизводит базу; известные сбои и непроверенные аппаратные сценарии названы явно. Личные данные остаются в private fixture storage, публичные fixtures обезличены.

### M1 — технические прототипы и архитектурные решения

- [x] M1.1 Создать минимальный Cargo workspace и закрепить stable toolchain/MSRV/dependencies.
- [x] M1.2 *(заменено ADR-01)* Iced prototype: Fluent shell, таблица устройств, curve canvas, keyboard canvas, live chart на полном horizon.
- [x] M1.3 *(заменено ADR-01)* Проверить кириллицу/IME, keyboard navigation, focus, tooltips, DPI 100/125/150/200%, Light/Dark/Auto и доступность ключевых controls.
- [x] M1.4 *(заменено ADR-01)* Проверить native Wayland KDE/Hyprland и X11; исключить неявный выбор NVIDIA renderer, который держит dGPU активной.
- [x] M1.5 Проверить ksni menu, tokens, existing-window activation и переход на desktop окна; определить необходимый toolkit integration patch (`ksni::current_xdg_activation_token()` + `ActivateWithToken` + проверка активации существующего окна в живой сессии Hyprland).
- [x] M1.6 Проверить Rust ioctl ABI: request numbers/struct layouts против установленного driver ABI и C++ reference harness.
- [x] M1.7 На fake BlueZ проверить GATT interface, уведомления, reconnect и выбор bluer/direct-zbus.
- [x] M1.8 Проверить покрытие NVIDIA wrapper и возможную изоляцию native GPU calls отдельным процессом.
- [x] M1.9 Зафиксировать решение GNOME/Plasma: минимальные adapters как исключение или согласованная замена embedding на Rust popup.
- [x] M1.10 Записать ADR по GUI, concurrency, data compatibility, privileges и external-library boundaries.

**Выход:** есть демонстрируемые прототипы самых рискованных мест. Toolkit не выбирается по одному красивому окну. Если activation/chart/input проваливается, решение меняется здесь, до переноса всех страниц.

### M2 — доменная модель и настройки

- [x] M2.1 Перенести ProfileState, physical/effective/profile states и PowerPolicy как pure functions/state machine.
- [x] M2.2 Перенести UccProfile, fan/keyboard profiles, stateMap, global charging, capability types, units и supported defaults.
- [x] M2.3 Развести legacy wire DTO и внутреннюю модель `Option`/typed values.
- [x] M2.4 Перенести default profiles и device-specific defaults с сохранением IDs; исключить личные power limits из public defaults.
- [x] M2.5 Реализовать read/validate/round-trip settings, autosave, Aquaris, user INI prefs и sleep snapshots.
- [x] M2.6 Реализовать atomic persistence, backups/recovery и сериализацию без повторных ключей.
- [x] M2.7 Перенести CPU/fan/profile parsers и validation с fixtures повреждённых/неполных/старых файлов.
- [x] M2.8 Прогнать property/differential tests policy matrix и сериализации; intentional differences записать отдельно.

**Выход:** Rust загружает действующие обезличенные настройки без потерь; повторная запись не теряет ни включённые CPU ID, ни RGB, ни частоты, ни global charging.

### M3 — D-Bus и CLI

- [x] M3.1 Реализовать `ucc-dbus`: все legacy methods/signals и typed proxies.
- [x] M3.2 Реализовать fake service и контрактные тесты 122 методов, включая bool false, malformed JSON, timeout, unavailable.
- [x] M3.3 Перенести Polkit action mapping read/control/manage-hardware; проверять реального sender и его lifecycle.
- [x] M3.4 Не блокировать event loop во время auth prompt; ограничить параллелизм, очередь и размер JSON.
- [x] M3.5 Перенести CLI status/monitor, profile/statemap, fan/keyboard, hardware/watercooler/charging/cpu/gpu/power-limits, power/devices/lightbar.
- [x] M3.6 Сохранить aliases, --json, monitor count/interval и exit codes; справка/completion/man генерируются из актуальной команды.
- [x] M3.7 Проверить Rust CLI → C++ daemon и C++ test-client → Rust fake/daemon backend на private bus.
- [x] M3.8 Учесть старые semantic defects client wrappers и отсутствующие методы как явные contract deviations.

**Выход:** transport/CLI полностью работоспособны с fake hardware; совместимость проверяется автоматически. Простой запуск `--help` не считается готовностью CLI.

### M4 — аппаратная основа и CPU/device policies

- [x] M4.1 Ввести traits HardwareIo, Sysfs, ProcessRunner, Clock, GpuBackend и тестовые реализации с write journal.
- [x] M4.2 Перенести Clevo/Uniwill detection, `/dev/tuxedo_io`, descriptors, capabilities и exact DMI quirks.
- [x] M4.3 Реализовать ioctl запросы с точным encoding, включая существующее использование pointer-sized `_IOR/_IOW`, не заменяя его автоматически `size_of::<i32>()`.
- [x] M4.4 Перенести CPU discovery, sparse topology, CPUID affinity guard, selection и enable-before-disable.
- [x] M4.5 Перенести governor/EPP/min-max/turbo/HWP, defaults/reapply/restore и ошибки записи.
- [x] M4.6 Перенести devices scan/merge, whitelist paths/values, отсутствующие devices и независимые три колонки.
- [x] M4.7 Перенести ASPM/Wi-Fi/system tunables/nvidia-powerd/persistence ordering без shell interpolation.
- [x] M4.8 Прогнать fake sysfs/procfs failure matrix и провести ограниченные аппаратные read/write проверки на живом железе (проверены CPU/ODM TDPs 210/210/250 Вт, governor/EPP `performance`, cTGP `+95`, brightness 100%, `PowerDeviceStatusJSON`, `ucc-cli status`).
- [x] M4.9 Ввести доменный трейт `PlatformBackend` поверх `HardwareIo`/`Sysfs`, изолирующий `TuxedoIoBackend` от логики демона и готовящий плавный переход к `UwccBackend` (`ADR-05`).

**Выход:** CPU/device actions соответствуют fixture journal и подтверждаемому readback; неподдержанное оборудование не получает guessed writes.

### M5 — охлаждение и BLE

- [x] M5.1 Перенести FanProfile interpolation, pump steps, filters/hysteresis/limits и sameSpeed.
- [x] M5.2 Проверить таблицы и округление на температурных трассах; отдельно протестировать critical/failure policies.
- [x] M5.3 Реализовать независимый fan loop и firmware-auto recovery при штатном/аварийном завершении.
- [x] M5.4 Перенести BLE packets/UUID/model mapping и encode/decode tests без реального устройства.
- [x] M5.5 Реализовать discovery/reconnect state machine, deadlines/backoff, keepalive, поздний adapter, saved identity.
- [x] M5.6 Реализовать ordered writes с gap ≥80 мс, coalescing значений и cancellation поколения соединения.
- [x] M5.7 Сохранить manual/auto semantics, Fan/Pump/LED restore после reconnect; запрещённый pump code отвергается.
- [x] M5.8 Проверить настоящую водянку: initial connect, disconnect, disable/enable, power cycle, manual/auto, отсутствие скачка на неподдержанный режим (проверено на физической `CoolingSystem LCT22002` `DE:C9:A0:A9:FC:84`: connect `fan=7/pump=2`, manual `fan=50/pump=3`, отказ `pump=99 -> false` с сохранением `pump=3`, `SetWaterCoolerAutoControl(true)`, `EnableWaterCooler(false)` -> disconnect -> `EnableWaterCooler(true)` -> reconnect и восстановление состояния).
- [x] M5.9 Исправить надёжное восстановление профиля/состояния СЖО в текущем C++ UCC (если требуется хотфикс) и во всех 5 причинах в Rust `uccd` и `ucc-hardware` (`manual_fan_speed`/`manual_pump_voltage` из профиля, мгновенный пересчёт кривой и replay после `WC_CONNECT_DEBOUNCE`, `spawn_blocking` для `reconcile_power_state`, загрузка LED из `aquaris.json`, `PrepareForSleep` teardown/resume, авто-включение сканирования при настроенном `state_map["power_wc"]`; по решению пользователя legacy C++ оставлен без хотфиксов — все причины и регрессионные тесты реализованы в Rust).

**Выход:** охлаждение работает автономно от GUI; BLE failures не задерживают fan loop, Polkit или shutdown.

### M6 — клавиатура и Lightbar

- [x] M6.1 Перенести обнаружение white/zone/per-key LEDs и физические ID.
- [x] M6.2 Перенести калиброванную раскладку и mapping LED-ID → hardware path без сдвига missing zones.
- [x] M6.3 Реализовать brightness scaling и buffered RGB writes; guard восстанавливает buffer_input и при ошибке.
- [x] M6.4 Перенести Lightbar discovery/settings, power modes, majority color, defaults и все 5 frame functions.
- [x] M6.5 Реализовать таймер анимации без sleep в event loop, no overlap, proper idle/Flash, Welcome/Silent/Test.
- [x] M6.6 Проверить private fixture цвета и аппаратный readback, изменения профиля и power mode, отказ записи и resume без повторного welcome (проверены 126 зон `ite_8291` и физический readback `/sys/class/leds/rgb:kbd_backlight/{brightness,multi_intensity}` + `GetLightbarStatusJSON` `ok:true` на хосте).
- [x] M6.7 Отделить логику клавиатуры и Lightbar от способа доставки (`LightingTransport`: `SysfsLedTransport` для `M6–M13` и точка подключения `HidrawTransport` для `M14`).

**Выход:** старые цвета/яркость и physical mapping сохранены; idle и error states воспроизводимы.

### M7 — telemetry и история

- [x] M7.1 Перенести CPU sensors/RAPL energy counters, wrap-around/time delta и единицы.
- [x] M7.2 Перенести Intel i915/xe, AMD и NVIDIA paths с per-GPU sensor identity и отсутствующими датчиками.
- [x] M7.3 Перенести реально используемые NVML/NvAPI symbols, capabilities, optional libraries и ограниченные OC paths.
- [x] M7.4 Ввести freshness/validity в модели; legacy sentinels формируются на IPC boundary.
- [ ] M7.5 *(частично выполнено — ждёт проверки D3cold без нагрузки Hyprland/mpvpaper)* Сохранить sleeping dGPU guard/cadence и проверить idle без постоянного пробуждения GPU (живая телеметрия RTX 5080 по NVML подтверждена на хосте: `temp=43–44 °C`, `powerDraw=7.9–8.1 W`, `vram=16303 MiB`; GPU оставалась в `active` из-за композитора Hyprland/`mpvpaper`, D3cold-цикл не проверялся).
- [x] M7.6 Реализовать bounded history, 10 series, horizon и legacy binary codec с malformed/truncated payload tests.
- [x] M7.7 Проверить shared ownership/init/shutdown NVIDIA, no-GPU machine и error injection; при необходимости подключить isolated GPU helper.

**Выход:** telemetry не смешивает GPU, не рисует выдуманные значения и не мешает энергосбережению; старый Monitor получает совместимый бинарный ответ.

### M8 — интегрированный Rust daemon и helpers

- [x] M8.1 Связать orchestrator, workers, snapshots и D-Bus; startup order воспроизводит загрузку capabilities/settings/profile (подтверждено на живом `uccd --start` на system bus и установленном `uccd.service`, `995 ms` до готовности D-Bus, полная активация оборудования с реальным `/etc/ucc/settings`).
- [x] M8.2 Реализовать single-instance/name ownership и запуск Type=dbus; не сообщать readiness до готовности API (эксклюзивный захват `com.uniwill.uccd` и `/de/uniwill/uccd`, второй `uccd --start` завершается с кодом `1`).
- [x] M8.3 Реализовать сигналы завершения и bounded shutdown с fan-auto, CPU restore, BLE/NVML cleanup (`SIGTERM` завершился с кодом `0` за `480 ms`, имя `com.uniwill.uccd` освобождено, `ucc-hardware --audit` подтвердил восстановление).
- [x] M8.4 Перенести `ucc-sleep` prepare/resume, ownership/permissions/symlink validation и retry после failed start (проверены живой `/usr/lib/ucc/ucc-sleep prepare` -> `/run/ucc/sleep-state.json` `0:0 0600`, сохранение снимка при отказе `systemctl start` с `rc=1` и успешный повторный `resume` с `rc=0`).
- [x] M8.5 Перенести `ucc-hardware --audit`/`--fan-auto` с проверкой поддержки устройства (проверены `--audit` `fan=0/1 EC-duty=28` и `--fan-auto -> OK` на реальном `/dev/tuxedo_io`).
- [x] M8.6 Перенести `ucc-import-check` с проверкой всех профилей/curves/LED IDs (валидация CLI + корпус в `crates/uccd/tests/import_check_corpus.rs`).
- [x] M8.7 Проверить restart, corrupt config, unavailable device, D-Bus restart, slow auth, queue overload, sleep cancellation (7-кейсовая матрица на частной шине в `crates/uccd/tests/fault_matrix.rs`).
- [x] M8.8 Старый C++ GUI/CLI тестируется с новым Rust daemon на изолированной шине и в управляемом аппаратном переходе (пройдены interop-тесты на частной шине, совместимость `/usr/bin/ucc-cli` и переход системы на пакет `ucc-rusted 0.0.1-1` с живым `/usr/bin/ucc-gui`, треем, агентом и демоном).
- [x] M8.9 Перенести `LidController` (ингибитор `handle-lid-switch`, debounce при отключении питания) и `WakeupPolicy` в `uccd` и `ucc-sleep prepare` (подтверждён блок-ингибитор `handle-lid-switch` в `systemd-inhibit --list` и обход `/sys/devices` за ~45 мс без зацикливания симлинков).
- [x] M8.10 Гасить подсветку клавиатуры и (опционально) подсветку подключённой СЖО при закрытии крышки и восстанавливать из RAM при открытии или выходе из сна (настройки на карточке `Lid` в `PowerDevicesTab`: `turnOffKeyboardBacklightOnLidClose` и `turnOffWaterCoolerLedOnLidClose`, без изменения активного профиля/`settings.json`, плюс подписка на `login1` `LidClosed` / `UPower` `LidIsClosed` в `run_logind_monitor_loop` и тесты).

**Выход:** все backend-функции F01–F29/F40 работают без C++ daemon и без запущенного GUI.

### M9 — пользовательская desktop-часть

- [x] M9.1 Перенести батареи/process sampler с activity-aware polling, coalescing и bounded results (подтверждены чтение `/sys/class/power_supply/BAT0` и `cargo test -p ucc-desktop`).
- [x] M9.2 Перенести pidfd termination, startTime/UID/GPU-fd recheck, grace/escalation и UI confirmation contract.
- [x] M9.3 Перенести notification observer, rate limit/restart, driver alert/recovery и source debounce без текста уведомлений (подтверждены живой `org.freedesktop.Notifications`, работа `ucc-session-agent` в Hyprland и чистый выход `SIGTERM` `rc=0` в `4f766cc`).
- [x] M9.4 Перенести KDE/Hyprland refresh backend, profile preferences, consent invalidation и readback (подтверждён живой `hyprctl monitors -j`: `eDP-1 2560x1600@240Hz scale 1.25`).
- [ ] M9.5 *(отложено на сообщество)* Обеспечить X11 backend в пользовательской сессии и совместимый маршрут SetDisplayRefreshRate.
- [x] M9.6 Перенести tray icon/menu/process popup, command failures и адаптивный polling (подтверждена регистрация в `org.kde.StatusNotifierWatcher` и владение `com.uniwill.UccDesktopTray` в живой сессии).
- [x] M9.7 Реализовать GUI activation endpoint с token forwarding и отдельные service names preview.
- [ ] M9.8 *(частично выполнено — ждёт compositor restart и нескольких пользовательских сессий)* Проверить autostart, singleton, compositor restart, отсутствие notification server/tray host и несколько пользовательских сессий (на живой сессии Hyprland `wayland-1` подтверждены singleton и чистый `SIGTERM` за `2–3 ms` для `ucc-desktop-tray` и `ucc-session-agent`, регистрация SNI и работа без `StatusNotifierWatcher`/`Notifications`).
- [x] M9.9 Перенести `LidDisplayHandler` (DPMS только встроенной панели `eDP-*`/`LVDS-*`) и восстановление частоты после готовности DPMS в `ucc-session-agent`.

**Выход:** tray и session agent работают без открытого главного окна и без Qt.

### M10 — автономный `Qt6::Widgets` GUI поверх Rust `uccd` (`ADR-01`)

- [x] M10.1 Отвязать CMake-сборку `ucc-gui` от удаляемых C++-компонентов (`uccd`, `ucc-session`, `ucc-desktop-tray`, `ucc-tray`) и лишних зависимостей (`Qml`, `Quick`, `QuickControls2`, `Qt6::Bluetooth`, `KF6WindowSystem`, `KF6StatusNotifierItem`), оставив только `Qt6::{Core,Gui,Widgets,DBus,Charts}`.
- [x] M10.2 Заменить активацию окна через `KF6WindowSystem` на нативный путь Qt6 Wayland (`XDG_ACTIVATION_TOKEN` + `QWindow::requestActivate()`) и статически встроить клиент D-Bus (подтверждено на живой сессии Hyprland `wayland-1` и через `ldd ucc-gui`).
- [x] M10.3 Перевести вкладку `Battery & Processes` на данные/команды Rust-стека (или изолированный легковесный Qt-виджет без `ucc-desktop-tray`), сохранив безопасное завершение пользовательских процессов NVIDIA и настройки глобальной зарядки.
- [x] M10.4 Обеспечить жизненный цикл «0 МБ Qt в фоне»: закрытие окна `ucc-gui` полностью завершает процесс, а клик в Rust-трее `ucc-desktop-tray` мгновенно запускает или фокусирует `ucc-gui` с передачей Wayland activation token.
- [x] M10.5 Исключить прототип `crates/ucc-gui` (`iced`/`wgpu`) из `Cargo.toml` и `Cargo.lock`, очистив Rust-workspace от графических зависимостей.
- [x] M10.6 Прогнать полный `test_gui_audit` (все 10 разделов, включая карточку `Lid close action & sleep wakeup` `F45`, обе темы, `UCC_READ_ONLY_PREVIEW` и отказ команд без fake success) в связке с новым Rust-демоном `uccd` на изолированной шине.
- [ ] M10.7 *(частично выполнено — ждёт ручного клика из трея и проверки D3cold при спящей RTX 5080)* Проверить на живой сессии Hyprland: масштабирование `1.25`/`2.0` (`404.8 ms` / `540.3 ms`), тему `qt6ct`, фокус по `XDG_ACTIVATION_TOKEN` и `0` `/dev/nvidia*` fds в установленном `/usr/bin/ucc-gui`; осталось подтвердить ручной клик по новому системному трею и D3cold sleep без нагрузки композитора/`mpvpaper`.
- [x] M10.8 Дополировка GUI: добавить уникальные векторные иконки в `FluentTheme::icon()` для вкладок `Power & Devices`, `Lightbar` и `Battery & Processes` вместо fallback-иконки клавиатуры.
- [x] M10.9 Добавить вкладку `Experimental` в `ucc-gui` с отдельной иконкой в сайдбаре, предупреждением о тестовом статусе и расширяемой структурой для будущих экспериментальных настроек.

**Выход:** все 10 разделов `ucc-gui` работают с Rust `uccd` без зависимостей `KF6`/`QML`/`libucc-dbus.so`, а в фоне нет ни одного Qt-процесса.

### M11 — миграция инструментов и shell-интеграций

- [x] M11.1 Перенести offline import текущего UCC и исторических TCC/Python Data в Rust (`crates/ucc-config/src/import.rs`, `cargo xtask import-offline`).
- [x] M11.2 Сохранить report/pending/desktop-preferences, unresolved refs, curve rounding/offsets и приоритет пользовательских данных.
- [x] M11.3 Перенести используемые prepare-deployment/deploy/desktop-transition/rollback сценарии в maintenance subcommands (`cargo xtask maintenance ...`).
- [x] M11.4 Перенести capture-gallery и parser-check workflows; старые Python tools оставить только в архиве после parity.
- [ ] M11.5 (частично выполнено) Выполнить выбранное в M1 решение GNOME/Plasma; покрыть каждый quick control, notification и activation path (код, pure-GJS boundary и `ucc-desktop-tray` проверены; живая приёмка GNOME/Hyprland остаётся на M13).
- [x] M11.6 Если остаются GJS/QML adapters, удалить их C++ backend, оставить transport/presentation и явно назвать исключение из Rust-only; такой результат не выдаётся за «весь исполняемый код на Rust».
- [x] *(заменено ADR-03)* M11.7 Если выбран strict Rust, проверить Rust popup в каждой DE и только затем согласованно исключить embedded extensions из нового пакета.
- [x] M11.8 Research probes/C N-API не включать в runtime; исторические материалы и attribution сохранить.

**Выход:** для сборки/установки/миграции основного продукта не нужен собственный Python код; судьба каждого shell component зафиксирована, скрытых потерь функций нет.

### M12 — пакеты, CI и обновление

- [x] M12.1 Создать `cargo xtask dist` для release artifacts, staging tree, manifests, version/commit metadata.
- [x] M12.2 Обновить Arch PKGBUILD, provides/conflicts, install hooks; settings не включаются в public package.
- [ ] M12.3 *(отложено на сообщество)* Обновить DEB/RPM/Nix recipes, services/dbus/polkit/autostart/icons/man/completions; проверить Nix-store helper paths.
- [ ] M12.4 *(частично выполнено — ждёт проверки одноимённого `post_upgrade` и отката на чистом окружении)* Сохранить enabled/disabled/masked sleep-hook state при upgrade, settings backup и private→public переход (при живой замене `frixit-control-center 0.3.0-1` → `ucc-rusted 0.0.1-1` `/etc/ucc/settings` сохранён побайтно без перезаписи, а `uccd`/`uccd-sleep` восстановлены в `enabled`; из-за смены `pkgname` отрабатывали `pre_remove`+`post_install`, поэтому одноимённый `post_upgrade` и masked/rollback-сценарии на чистом стенде остаются в M12.9).
- [x] M12.5 Настроить fmt/clippy/tests/contract tests/fuzz smoke/build/packaging/UI jobs под непривилегированным пользователем.
- [x] M12.6 Проверить normal (`-DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF`, 48/48 CTest) и read-only preview (`-DUCC_READ_ONLY_PREVIEW=ON`, 3/3 CTest) сборки, а также `xtask dist` (18 staged файлов) без доступа к системному `/sys` и system bus.
- [x] M12.7 Проверить dependencies/runtime links: основным binaries не нужны Qt/libucc-dbus; ожидаемые системные/FFI библиотеки перечислены.
- [x] M12.8 Сформировать license/dependency notices, CycloneDX SBOM и source archive (`docs/MIGRATION_AND_RECOVERY.md` перенесено в `M13.11`).
- [ ] M12.9 Установить staged package на чистое окружение и обновить копию существующей установки; затем выполнить downgrade/rollback.
- [x] M12.10 Закрепить единый code style, commit convention и алгоритм релизов: `rustfmt.toml`, `.editorconfig`, `cargo fmt --check` в CI, `CONTRIBUTING.md`, устранение хардкода `docs/releases/0.3.0.md` в `.github/workflows/release.yml`, синхронная проверка/обновление версий (`Cargo.toml`, `CMakeLists.txt`, `PKGBUILD`, `ucc.spec`, `package.nix`) и генерация `CHANGELOG`.

**Выход:** пакет воспроизводимо собирается и устанавливается, а переход и откат подтверждены тестами, не только скриптом с успешным exit code.

### M13 — аппаратная приёмка, переключение и удаление legacy

- [ ] M13.1 Сохранить старый пакет, хэши, root/user configs, enabled/active states и рабочую процедуру отката.
- [ ] M13.2 Проверить release по аппаратной матрице ниже; системное переключение проводится с одним writer.
- [ ] M13.3 Проверить idle battery consumption, dGPU runtime suspend, UI latency/RSS и отсутствие busy polling.
- [ ] M13.4 Провести продолжительные прогоны и реальные sleep/resume/power transitions; failures исправить и повторить затронутые проверки.
- [ ] M13.5 *(частично выполнено — ждёт reboot/login/autostart и rollback drill)* Установить Rust package вместо C++ package, проверить reboot/login/autostart и rollback drill (`ucc-rusted 0.0.1-1` установлен поверх `frixit-control-center 0.3.0-1`, запущены `uccd.service`, `ucc-cli`, `ucc-desktop-tray`, `ucc-session-agent` и `/usr/bin/ucc-gui`).
- [ ] M13.6 Пройти F01–F44, 122 methods/7 signals, все 10 GUI pages, инструменты и выбранную shell policy.
- [ ] M13.7 Удалить C++/Qt runtime из active build/install; старый код остаётся в Git/tag/archive для истории.
- [ ] M13.8 Удалить временные compatibility bridges, source-extraction tests и старые package dependencies после переноса проверок.
- [ ] M13.9 Обновить README/BUILDING/TESTING/COMPARISON/origins и выпустить проверенный release.
- [x] M13.10 Переименовать проект из `Frixit Control Center` в выбранное Rust-ориентированное имя во всех пользовательских runtime-поверхностях (GUI, tray, CLI, desktop-файлы, PKGBUILD, packaging-метаданные) с сохранением совместимости D-Bus/миграции (`ucc-rusted`; документация `README.md`, `README.ru.md`, `docs/BUILDING.md` вынесена в `M13.11`).
- [ ] M13.11 Подготовить строгую техническую документацию без маркетинга и нейрослопа (`наш` и т.п.) и документацию для AI-агентов для быстрой ориентации в проекте (`AGENTS.md`, `docs/AI_AGENT_GUIDE.md`), `README.md` (EN), `README.ru.md` (RU), архитектуру, справочники D-Bus API и CLI, матрицу железа, руководство по миграции и восстановлению (`docs/MIGRATION_AND_RECOVERY.md`) и объективное сравнение в `docs/COMPARISON.md` с [`open-uniwill-cc`](https://github.com/aspidovss/open-uniwill-cc/blob/main/README.ru.md), upstream `UCC Revorked`, `TUXEDO Control Center`, `Clevo Control Center`, `LCT`, `CoreCtrl` и `asusctl` (полностью убрать упоминание «Личный Python ControlCenter»).

**Выход:** достигается определение готовности из раздела 16. Наличие незавершённого hardware test или несогласованной потери embedded shell UI оставляет соответствующий пункт открытым.

### M14 — переход с tuxedo-drivers на собственный Rust `uwcc` (`ADR-05`)

**Вход:** завершённый и принятый Rust-сервис `M0–M13` с доменным швом `PlatformBackend` (`M4.9`) и `LightingTransport` (`M6.7`); на целевом ядре включён `CONFIG_RUST=y`, присутствует ACPI `INOU0000`.
**Лицензионное правило:** переносится только чистая открытая логика (MIT / GPL-2.0 / GPL-3.0-or-later); любые файлы из `proprietary/` и `uwccd/lighting/oem/` в этот проект не попадают.

- [ ] M14.1 Прототип Rust-for-Linux модуля `kernel/uwcc-rs` на целевом ядре (`CONFIG_RUST=y`): проверить доступность безопасных kernel Rust-абстракций для ACPI (`INOU0000` `ECRR`/`ECRW`/`SMRW`/`PWUP`) и WMI; если часть ACPI/WMI API не обёрнута, изолировать минимальный тонкий C-шим, оставив логику драйвера, блокировки и валидацию на Rust.
- [ ] M14.2 Реализовать `/dev/uwcc` (`ioctl` `READ`/`WRITE`/`SMRW`/`PWUP`, `read()/poll()` кольцевой буфер WMI-событий, строгая проверка границ), нативный `/sys/class/hwmon/hwmonX` (`fan1_input`/`fan2_input` в реальных RPM через `0x464/0x465` и `0x46C/0x46B`, `pwm1`/`pwm2` через `0x75B/0x75C`, EC `temp*_input` с сохранением `D3cold` для RTX) и DKMS-рецепт под GPL-2.0.
- [ ] M14.3 Реализовать `UwccBackend` в `crates/ucc-hardware`: профиль платформы `0x1A` (MECHREVO Yaoshi X6AR55xY), чтение таблицы пределов `SMRW`, реальные обороты тахометра (`0x464/0x465`, `0x46C/0x46B`) и EC-датчики в `PlatformBackend`, запись `PL1`/`PL2`/`PL4`, **CPU TCC temperature limit**, `cTGP`/`Dynamic Boost` и `PWUP` для СЖО.
- [ ] M14.4 Реализовать нативное управление `RamFan 1.5` в EC (`0xF00–0xF5C`, флаги `CUME`/`UFME`, 16-точечные кривые CPU/GPU, `Fan Boost`, возврат `firmware auto` при штатном и аварийном останове).
- [ ] M14.5 Реализовать управление зарядкой батареи, EC-переключателями (Fn Lock, Win Lock, питание полосы `0x767`/`0x768`, USB sleep charging), отключение комбинированной USB RGB+IR веб-камеры на шине USB (для Uniwill без Clevo `W_CL_WEBCAM_SW`), а также трансляцию WMI-событий `/dev/uwcc` в `uinput`/действия демона (замена `tuxedo_keyboard`).
- [ ] M14.6 Реализовать прямой USB `HidrawTransport` в `crates/ucc-lighting` для `048D:600B` (с сохранением физической калибровки `Enter 56`/`LShift 21`/`LCtrl 0`) и `048D:7001` / `048D:7000` (передняя полоса Lightbar, зоны подсветки корпуса/логотипа крышки, EC-выключатель `0x767`/`0x768`, 5 анимаций уведомлений и `Flash (stays off)` — только открытый протокол без `uwccd/lighting/oem/`) с полным отказом от `ite_8291`/`ite_8291_lb`.
- [ ] M14.7 Интегрировать выбор и автодетект бэкенда (`auto` / `uwcc` / `tuxedo_io`) в `uccd`, `ucc-hardware --fan-auto` и `ucc-sleep`, передачу реальных RPM тахометра (взамен оценки `% * 60`) и опциональных зон корпуса/логотипа и `TCC` через D-Bus/CLI/GUI с сохранением контракта `com.uniwill.uccd`.
- [ ] M14.8 Собрать пакет `uwcc-dkms` с взаимоисключающим `modprobe` blacklist (`tuxedo_io`, `uniwill_wmi`, `tuxedo_keyboard`, `tuxedo_nb02_nvidia_power_ctrl`, `ite_8291`, `ite_8291_lb`) и проверить процедуру чистого отката на `TuxedoIoBackend` + `tuxedo-drivers`.
- [ ] M14.9 Провести полную аппаратную приёмку `UwccBackend` + `uwcc-rs` на `MECHREVO YAOSHI X6AR55xY (0x1A)` (охлаждение, нативный `/sys/class/hwmon` в `sensors`/`btop` с RPM вентиляторов и EC-температурами без пробуждения RTX из `D3cold`, `SMRW`/TCC, СЖО LCT22002, клавиатура `600B`, Lightbar `7001`/`7000` и логотип крышки, USB-выключатель веб-камеры RGB+IR, зарядка, Fn-клавиши, крышка/wakeup, sleep/resume, откат).

**Выход:** система работает без `tuxedo-drivers-dkms` на собственном Rust `uwcc` стеке, сохраняя весь D-Bus/Hyprland/Noctalia функционал и возможность отката.

### M15 — порт на Windows (после M14)

- [ ] M15.1 Абстрагировать IPC-слой (D-Bus на Linux / Named Pipes + ACL/SID на Windows) и сервисную обёртку (`systemd` / `windows-service`) для кроссплатформенного запуска `uccd`.
- [ ] M15.2 Исследовать и реализовать Windows `PlatformBackend` поверх ACPI WMI (`root\WMI`, `INOU0000`): оценить покрытие TDP/ODM/RamFan/зарядки/Fn-Lock без неподписанного KMDF-драйвера и зафиксировать границы требований EV/WHQL.
- [ ] M15.3 Реализовать Windows-бэкенды телеметрии, питания, дисплея и крышки: `nvml.dll`, Windows Power Management API (EPP/частоты/turbo), `ChangeDisplaySettingsExW`/`WmiMonitorBrightnessMethods`, `RegisterPowerSettingNotification` (`GUID_LIDSWITCH_STATE_CHANGE`).
- [ ] M15.4 Реализовать Windows-транспорты периферии: WinRT BLE GATT для СЖО (взамен BlueZ) и прямой USB HID (`hidapi`/`HidD_SetFeature`) для клавиатуры `048D:600B`, Lightbar и логотипа крышки (`logo`) `048D:7001` / `048D:7000`.
- [ ] M15.5 Адаптировать трей и GUI под Windows: нативный `Shell_NotifyIconW` / `tray-icon`, сборка автономного `Qt6::Widgets` `ucc-gui` под MSVC/MinGW.
- [ ] M15.6 Собрать и проверить Windows-инсталлятор (MSI через WiX или Inno Setup), сценарий установки службы, обновления и удаления (`winget` manifest).

**Выход:** `uccd`, CLI и `Qt6::Widgets` GUI работают на Windows 11 через WMI/WinRT/HID без Linux-зависимостей; функции, требующие прямого EC I/O сверх ACPI WMI, явно ограничены наличием EV/WHQL-подписанного KMDF-драйвера.

## 12. Проверки и перенос тестов

| Слой | Проверяем | Источник текущих сценариев |
|---|---|---|
| Pure logic | Source/mode/profile matrix, owners, limits, interpolation, pump steps, rounding, animation frames | power_policy, state_utils, fan_profile, lightbar |
| Config | Semantic round-trip, unknown fields, duplicate keys, corrupt backup, sparse CPU IDs, global charging | settings_manager, profile_manager, migration/import tests |
| IO | Реальные пути/units/value constraints, write order, partial failure/readback, absent hardware | cpu_selection, power_devices, keyboard_controller, display_backlight, gpu_sysfs |
| D-Bus | XML/signatures, methods/results/errors/signals, owner cleanup, auth decisions, reconnect | control_authorization, client/service tests |
| BLE | Packets, state transitions, timeouts/backoff/throttle, stale callbacks, adapter hotplug | aquaris_selection, bluetooth_startup/retry, watercooler_override |
| Desktop | батареи/процессы/pidfd, source/driver notifications, display profile/consent | session_telemetry, process_terminator, notification_monitor, refresh_advisor |
| GUI | Edit/apply/save/revert, nearest-point drag, no write on selection, stale replies, readonly guards | gui_audit, fan_curve_editor, system_monitor, gui_instance |
| Package | staged file tree, upgrade/private settings, sleep alias/masks, rollback on failure | deployment, prepare_deployment, desktop_transition, sleep_install |

Часть Python-тестов извлекает куски C++ source и компилирует их с test stubs. Это промежуточный способ проверки существующего кода, а не архитектура Rust suite. Переносятся поведенческие assertions и fixtures; поиск строки в C++ файле не становится финальным Rust-тестом.

Для differential tests оба backend исполняют один сценарий на fake hardware и дают journal: путь/операция/значение/порядок/результат. JSON сравнивается семантически после нормализации допустимых timestamps/IDs, бинарный history codec — побайтно на данном host. Ошибки старого serializer и ложный успех client wrappers учитываются как утверждённые исправления.

GUI notification history и observer внешних desktop notifications — разные данные: первая содержит события самого центра, ограничена 200 записями и поддерживает unread/read-all/clear; второй только запускает анимацию по факту вызова Notify. Перенос не должен начать сохранять чужие сообщения в истории центра.

Property tests полезны для power matrix, CPU masks, sorted curves, codec bounds, unit conversions. Fuzz targets нужны на JSON import, legacy binary metrics, BLE frames, user-config parsing. Конкурентные сценарии проверяют смену профиля во время ответа, reconnect, отмену, owner disconnect и очереди при burst commands.

Предварительные целевые бюджеты для M0/M1: UI event processing без задержек более 100 мс от IPC; стабильное взаимодействие при задержке daemon reply 300–3000 мс; bounded queues/history/logs; штатный stop укладывается в service TimeoutStopSec 10 с либо заранее выявляет изолированный зависший backend. Порог памяти/idle CPU/энергии задаётся после измерения C++ базы, а не придумывается как обещание ускорения Rust.

## 13. Матрица аппаратной приёмки

Основная машина — документированная exact Mechrevo-конфигурация. Поддержка других моделей не выводится из одного успешного теста. Для возможностей, которые нельзя проверить имеющимся железом, фиксируются fixture coverage и статус «аппаратно не проверено».

| Сценарий | Что наблюдать | Критерий |
|---|---|---|
| AC → Type-C → Battery и обратно | Реальный source, effective mode, профиль, overlay/device writes, tray, уведомления | Все три колонки и stateMap не смешиваются |
| Manual + app requests + cooler | Приоритеты и owner disconnect | Поведение совпадает с утверждённой policy matrix |
| CPU profiles | governor/EPP/turbo/freq/readback, sparse IDs, P/E, mandatory CPU | Выбранные CPU и лимиты сохраняются; отказ не отключает рабочий набор |
| Cooling under load/idle | curves, EWMA, sameSpeed, critical/failure policy | Нет потери управления; recovery возвращает firmware auto |
| Cooler | initial/reconnect/power cycle/manual/auto/LED | Нет unsupported pump mode и stale writes после reconnect |
| RGB/Lightbar | physical colors/brightness, Flash idle, all animations | LED IDs и исходные настройки сохранены |
| NVIDIA awake/auto/sleep | runtime_status, suspended time, NVML cadence, UI closed/open | Мониторинг не блокирует runtime suspend постоянно |
| Display KDE/Hyprland/X11 | profile Hz и offers, topology/MUX, brightness | Разрешение/scale/position/color rule сохранены, readback подтверждён |
| Charging | restart и performance profile changes | Общая зарядка не меняется от профиля |
| User process actions | selected/current UID/startTime/pidfd, exited/reused PID | Только выбранная актуальная цель получает signal после подтверждения |
| Sleep/hibernate/resume | snapshot, NVIDIA ordering, manual mode, cooler, paused jobs | Возврат к предыдущему допустимому состоянию без повторного welcome |
| Crash/kill/restart | ExecStopPost, CPU/fan recovery, startup | Recovery не зависит от Rust Drop |
| Reboot/login | daemon + session agent + tray, minimized/window token | Один экземпляр каждого процесса; окно открывается явным действием |
| Upgrade/downgrade | package/settings/user prefs/sleep masks | Старый пакет восстанавливается с прежними данными |
| Extended run | idle/load, смены питания, BLE drop, logs/history growth | Нет утечек, runaway retry и неограниченного накопления |

Начальный план soak: несколько повторов каждой смены питания и sleep/resume, затем 24–72 часа смешанного режима. Точное число циклов/длительность фиксируются в протоколе M13 по результатам первых прогонов. Реальный sleep и завершение приложений не запускаются автоматически обычными unit tests.

## 14. Переход на установленной системе и откат

1. Подготовить release candidate и staged file manifest; закончить offline/contract/UI проверки.
2. Сделать private backup root/user settings, runtime profile/manual/cooler state, package и enabled/active/masked states. Записать проверяемые хэши.
3. Остановить user clients и session agent, затем текущий writer daemon. Проверить завершение и firmware fan-auto.
4. Установить новый пакет обычным package manager. Сохранить имена бинарников/units/D-Bus, чтобы существующие launcher scripts продолжили работать.
5. Валидировать настройки новым `ucc-import-check` до аппаратного применения. При ошибке вернуть старый пакет/настройки, не запускать defaults поверх личных данных.
6. Запустить Rust daemon, проверить name ownership, capabilities, active profile, source/mode, критические controllers и ошибки.
7. Запустить user session agent/tray/GUI; проверить activation, prefs, charging и live state.
8. Выполнить короткий аппаратный smoke; затем полноценную M13 приёмку, reboot и soak.
9. При отказе: stop Rust clients/daemon → fan-auto → package downgrade → восстановление совместимой копии settings → восстановление прежних enabled/masked states → start прежнего controller/session clients.

Нельзя повторно использовать first-install `deploy-system.py` без адаптации: текущая установка UCC уже существует, а исторический installer специально отказывается от такой установки. Новый upgrade path обязан работать с существующим UCC. Нельзя force-overwrite package files вручную или удалять рабочий старый controller до проверенного rollback.

## 15. Риски и что закрывает каждый из них

| Риск | Приоритет | Мера и этап |
|---|---|---|
| Потеря cooling при ошибке/зависании | Критический | Независимый loop, sensor-failure policy, external fan-auto helper, fault tests M4/M5/M8 |
| Неверный ioctl ABI | Критический | C++/Rust request-number/layout comparison, exact driver compatibility, M1/M4 |
| Потеря личных настроек | Высокий | Legacy fixtures, single serializer, atomic write/backup, downgrade test M2/M12 |
| NVIDIA не засыпает/зависает native call | Высокий | Guard/cadence/init measurements, independent fan loop, helper isolation M1/M7 |
| BLE reconnect меняет порядок/значения | Высокий | Explicit state machine, generation-scoped queue, packet fixtures, live M5 |
| Случайное сохранение лишних зависимостей KF6/QML в `ucc-gui` | Высокий | Исключение `KF6`/`QML`/`Bluetooth` из CMake и `PKGBUILD`, проверка через `ldd` в `M10.1` и `M12.7` |
| Скрытая потеря GNOME/Plasma embedding | Высокий | Отдельное решение M1/M11, mapping всех controls, не выдавать Rust tray за встроенный applet |
| Изменение порядка async operations | Высокий | Command actor, revisions, bounded queues, race tests M3/M8 |
| Ложный успех аппаратной записи | Высокий | Result/readback/partial report; failure assertions от клиента до GUI |
| Разные sleep схемы у дистрибутивов | Высокий | Общий lifecycle, generated/staged units, Nix-specific tests M8/M12 |
| Поломка скриптов/старых клиентов | Средний | D-Bus/CLI contract suite, old/new matrix, stable executable names |
| Регрессия батареи/idle CPU | Высокий | Baseline measurements, hidden-page throttling, iGPU renderer choice, M0/M1/M7/M13 |
| Перенос исследовательских небезопасных probes как функций | Средний | Использовать только подтверждённый runtime symbol map; исследования отдельно |
| «Всё на Rust», но старые helpers остаются в пакете | Средний | Package file/runtime-dependency audit и итоговый F01–F44 checklist M12/M13 |
| Конфликт `tuxedo-drivers` и `uwcc` за EC `INOU0000` / HID | Критический | Взаимоисключающий `modprobe` blacklist в `uwcc-dkms`, выбор активного `PlatformBackend` в `uccd` и чистый откат `M14.8` |
| Дрейф out-of-tree Rust-for-Linux API между ядрами | Высокий | Проверка доступных kernel Rust-абстракций и изоляция минимального ACPI/WMI шима в `M14.1` |

## 16. Определение полного завершения

- [ ] Все F01–F46 имеют подтверждённый исход; весь фоновый и системный стек (`uccd`, `ucc-cli`, `ucc-desktop-tray`, `ucc-session-agent`, helpers, maintenance tools) написан на чистом Rust без фоновых C++/Qt процессов.
- [ ] Окно настроек `ucc-gui` работает как автономный `C++20 / Qt6::Widgets + Qt6::Charts` D-Bus клиент без `KF6`, `QML`, `Qt6::Bluetooth` и `libucc-dbus.so` (`ADR-01`), выгружаясь из памяти при закрытии.
- [ ] Прямой `/dev/tuxedo_io` backend реализован на Rust; постоянного моста к C++ tuxedo_io_lib нет.
- [ ] Все 122 legacy methods и 7 signals покрыты контрактами; intentional fixes/aliases перечислены.
- [ ] Сохранены settings/profiles/LED mapping/user prefs/global charging/manual/cooler state по применимым lifecycle boundaries.
- [ ] Все 10 GUI-разделов, tray, CLI, session agent, helpers, import/deploy/rollback и выбранные shell interfaces работают.
- [ ] Решение по shell-интеграции выполнено (`ADR-03`): основной трей — Rust `ucc-desktop-tray` (SNI), C++/QML Plasma-апплет `ucc-tray` исключён.
- [ ] Все обязательные tests проходят; skips и known failures не скрыты, полный GUI suite не объявлен green через исключение проблемного теста.
- [ ] Пройдена аппаратная приёмка, реальные sleep/resume и extended run; непроверенные другие машины названы отдельно.
- [ ] Package upgrade/downgrade сохраняет настройки; старый рабочий пакет и recovery procedure доступны.
- [ ] Фоновые службы и CLI собираются на чистом Rust без Qt/C++/Node/Python; `ucc-gui` использует только `qt6-base` + `qt6-charts` (без `KF6`/`QML`/`libucc-dbus.so`).
- [ ] Runtime внешних системных библиотек/драйверов перечислен; наличие native FFI не маскируется как memory-safe Rust.
- [ ] Build/version/commit metadata, notices, исходники и пользовательская документация соответствуют новой реализации.
- [ ] Legacy runtime удалён из active build/install только после выполнения перечисленного.

## 17. Порядок выполнения и оценка объёма

Зависимости основных этапов:

```text
M0 → M1 → M2 → M3
              ├→ M4 → M5/M6/M7 → M8
              └→ UI models/prototypes → M10
M3 + M8 → M9 → завершение M10
M2 + M8/M9/M10 → M11 → M12 → M13 → M14 (собственный Rust uwcc, ADR-05)
```

Работы над UI на fake backend можно вести до готовности всего daemon. Реальное переключение writer происходит только после M8 и подготовки пакета/rollback. Исправления критических legacy дефектов могут отдельными commits вернуться в C++ baseline, если это потребуется для надёжного эталона, с обязательной фиксацией новой базы.

Предварительная инженерная оценка для одного опытного разработчика, работающего над задачей полностью: **примерно 5–9 месяцев до полного проверенного релиза**, с существенной неопределённостью GUI/Wayland, Bluetooth и аппаратного soak. Это не измеренная скорость и не обещанная дата. M0/M1 должны уточнить диапазон по реальным prototypes и contract coverage.

| Блок | Ориентир трудозатрат, рабочие недели |
|---|---:|
| Baseline, контракты, technology prototypes | 2–3 |
| Domain/config/D-Bus/CLI | 3–5 |
| IO/CPU/devices/cooling/BLE/lighting/telemetry/daemon | 6–10 |
| Desktop/tray/session + адаптация `Qt6::Widgets` GUI | 2–3 |
| Import/packaging/CI/upgrade/cleanup | 2–4 |
| Hardware acceptance/soak/fixes | 2–4 |

Сумма даёт около 17–29 рабочих недель для базовой миграции `M0–M13` (плюс `M14` для перехода на собственный Rust `uwcc`), часть ожидания soak и независимых работ может пересекаться. Неопределённость снимается рабочими компонентами и измерениями, а не увеличением количества файлов `.rs`.

Первая реализационная порция после принятия направления: M0.3–M0.9 и M1 с Cargo skeleton, fake D-Bus, Canvas editor/keyboard/chart prototypes и реальной проверкой tray activation. Затем M2/M3. Переписывать сразу весь `UccDBusService.cpp` и весь `MainWindow.cpp` без контрактного каркаса не следует: это крупнейшие точки концентрации поведения и регрессий.

## 18. Границы проведённого изучения и источники

Для плана выполнены статическая инвентаризация обоих репозиториев, разбор архитектуры процессов/сборки, публичного D-Bus, моделей/сериализаторов, policy/CPU/device/cooling/BLE/LED/display/GPU/desktop кода, GUI сценариев, существующих тестов, рецептов установки и миграции. Учитывались текущие рабочие изменения, теперь сохранённые в `45921b0`, а не только HEAD до них. Манифест охватывает 395 отслеживаемых файлов UCC и 58 файлов Python-архива.

На этом этапе **не выполнялись** новая полная сборка/test suite, запись live hardware settings, переключение демонов, настоящий suspend/resume или установка Rust. Существующие `docs/TESTING.md` и локальные журналы описывают прежние проверки; они не выдаются за свежий прогон. Выполнены проверка diff и подтверждение remote commit исходной базы.

Внешняя проверка технологических кандидатов ограничена первичными источниками:

- [Iced stable Canvas](https://docs.rs/iced/latest/iced/widget/canvas/) и [основная документация](https://docs.iced.rs/iced/).
- [zbus](https://docs.rs/zbus/latest/zbus/) и [interface macro/dispatch](https://docs.rs/zbus/latest/zbus/attr.interface.html).
- [BlueR/BlueZ Rust API](https://docs.rs/bluer/latest/bluer/).
- [ksni StatusNotifierItem](https://docs.rs/ksni/latest/ksni/).
- [winit activation token](https://docs.rs/winit/latest/winit/window/struct.ActivationToken.html) и [startup notification](https://docs.rs/winit/latest/winit/platform/startup_notify/).
- [nvml-wrapper](https://docs.rs/nvml-wrapper/latest/nvml_wrapper/).
- [Plasma widget documentation](https://develop.kde.org/docs/plasma/widget/).
- [Slint toolkit/альтернатива](https://slint.dev/faqs).

Перенос сохраняет текущую GPL-3.0-or-later основу и attribution проекта/внешнего кода. Новые dependency licenses проверяются по реально закреплённому Cargo.lock; переписывание языка само по себе не является основанием удалять существующие notices.
