# ucc-rusted

[English](README.md) | [Русский](README.ru.md)

[![CI Build](https://img.shields.io/github/actions/workflow/status/Frixlt/UCC/build.yml?label=CI%20Build)](https://github.com/Frixlt/UCC/actions/workflows/build.yml)
[![Nix Check](https://img.shields.io/github/actions/workflow/status/Frixlt/UCC/nix.yml?label=Nix%20Check)](https://github.com/Frixlt/UCC/actions/workflows/nix.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](COPYING)
[![Rust MSRV 1.89+](https://img.shields.io/badge/Rust-1.89%2B-orange.svg)](Cargo.toml)
[![GUI: C++20 / Qt6](https://img.shields.io/badge/GUI-C%2B%2B20%20%2F%20Qt6-41CD52.svg)](CMakeLists.txt)
[![Platform: Linux x86_64](https://img.shields.io/badge/platform-Linux%20x86__64-lightgrey.svg)](PKGBUILD)
[![D-Bus API: 123 methods | 7 signals](https://img.shields.io/badge/D--Bus%20API-123%20methods%20%7C%207%20signals-blueviolet.svg)](docs/DBUS_API.md)

**ucc-rusted** — набор системного управления для ноутбуков Uniwill, Clevo, Mechrevo и TUXEDO под Linux (`x86_64`), перенесённый с C++ на Rust из Uniwill Control Center (`UCC`). Фоновый демон, аппаратные бэкенды (ioctl, sysfs, procfs, BlueZ, NVML), контроллер подсветки, CLI, системный трей, сессионный агент и утилиты сна/восстановления реализованы на Rust (MSRV `1.89`, edition `2021`). Окно настроек (`ucc-gui`) является автономным D-Bus-клиентом на `C++20 / Qt6` без зависимостей `KF6` и `QML` согласно `ADR-01`.

<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/overview.png -->
> **Слот главного скриншота (`Overview`, тёмная тема):** `screenshots/current/dark/overview.png`

---

## Область проекта и архитектура процессов

Все привилегированные операции с оборудованием выполняются внутри единственного root-демона (`uccd`) с последовательной обработкой команд. Пользовательские клиенты работают без прав root и взаимодействуют с демоном исключительно через системную шину D-Bus (`com.uniwill.uccd` по пути `/com/uniwill/uccd` и прозрачный совместимый псевдоним `de.uniwill.uccd` по пути `/de/uniwill/uccd`: 123 метода и 7 сигналов).

### Исполняемые файлы времени выполнения (7 Rust + 1 автономный Qt6 GUI)

| Бинарный файл | Путь установки | Язык / стек | Права | Назначение |
| --- | --- | --- | --- | --- |
| `uccd` | `/usr/bin/uccd` | Rust (`crates/uccd`) | `root` (systemd) | Системный демон: аппаратный ввод-вывод, кривые вентиляторов, политики питания/устройств, подсветка, Bluetooth-водянка LCT, телеметрия, D-Bus-сервер |
| `ucc-cli` | `/usr/bin/ucc-cli` | Rust (`crates/ucc-cli`) | Пользователь | Интерфейс командной строки (`clap` 4.5): статус, мониторинг, профили, карта состояний, вентиляторы, клавиатура, водяное охлаждение, зарядка |
| `ucc-desktop-tray` | `/usr/bin/ucc-desktop-tray` | Rust (`crates/ucc-desktop`) | Пользователь | Самостоятельный трей StatusNotifierItem (SNI): режимы питания, мощность батареи, список пользовательских процессов NVIDIA (завершение через `pidfd`), передача Wayland activation token |
| `ucc-session-agent` | `/usr/bin/ucc-session-agent` | Rust (`crates/ucc-session-agent`) | Пользователь | Сессионный D-Bus-агент: запуск анимаций Lightbar по уведомлениям, переключение частоты экрана KDE (`kscreen`) / Hyprland (`hyprctl`), оповещение об ошибках драйвера при входе |
| `ucc-hardware` | `/usr/lib/ucc/ucc-hardware` | Rust (`crates/uccd`) | `root` (helper) | Аппаратная утилита, вызываемая через `ExecStopPost` в `uccd.service` (`--fan-auto`) для возврата управления вентиляторами прошивке при остановке или сбое демона |
| `ucc-sleep` | `/usr/lib/ucc/ucc-sleep` | Rust (`crates/uccd`) | `root` (helper) | Обработчик suspend/resume, вызываемый из `uccd-pre-sleep.service` и `uccd-sleep.service` |
| `ucc-import-check` | `/usr/lib/ucc/ucc-import-check` | Rust (`crates/uccd`) | Пользователь / `root` | Автономный импортёр и валидатор схемы конфигурации |
| `ucc-gui` | `/usr/bin/ucc-gui` | C++20 / `Qt6::Widgets` + `Qt6::Charts` | Пользователь (по запросу) | Автономное окно настроек (`ADR-01`): без `KF6`, `QML`, `Qt6::Bluetooth` и `libucc-dbus.so`; 0 МБ в фоне после закрытия |

Устаревший апплет Plasma на C++/QML (`ucc-tray`) исключён из целевых сборок (`-DBUILD_TRAY=OFF`, `ADR-03`); его функции полностью покрываются `ucc-desktop-tray` в KDE Plasma, Hyprland, Sway/Waybar и GNOME (при наличии SNI/AppIndicator host). Расширение `ucc-gnome` сохранено как необязательный внешний GJS D-Bus-адаптер.

### Структура Cargo Workspace (10 крейтов)

| Крейт | Тип | Назначение |
| --- | --- | --- |
| `ucc-core` | Библиотека | Доменные типы, профили, кривые вентиляторов, структуры телеметрии, типы ошибок |
| `ucc-config` | Библиотека | Хранение `/etc/ucc/settings` (с резервным чтением `/etc/ucc/settings.json`) и `~/.config/ucc/settings.json`, атомарная запись с восстановлением из `.bak`, автономный импортёр |
| `ucc-hardware` | Библиотека | Определение DMI/платформы, ioctl `tuxedo-drivers`, управление CPU/PCI/USB/ASPM/зарядкой через sysfs/procfs, NVML (`libloading`), BlueZ GATT |
| `ucc-lighting` | Библиотека | Поклавишная RGB-подсветка клавиатуры, физическая калибровка LED Mechrevo, контроллер Lightbar и 5 анимаций уведомлений |
| `ucc-dbus` | Библиотека | Интерфейс `zbus` 5.5 (`com.uniwill.uccd` + `de.uniwill.uccd`), клиентские прокси, авторизация Polkit |
| `uccd` | Бинарники | `uccd`, `ucc-hardware`, `ucc-sleep`, `ucc-import-check` |
| `ucc-cli` | Биб. + Бин. | `ucc-cli`, генерация автодополнений shell и man-страницы |
| `ucc-desktop` | Биб. + Бин. | Настольные утилиты и `ucc-desktop-tray` |
| `ucc-session-agent` | Биб. + Бин. | Интеграция с сессией и `ucc-session-agent` |
| `xtask` | Автоматизация | Сборка, установка runtime, проверка версий и релизные задачи (`install-runtime`, `check-versions`, `dist`, `dist-cli`, `capture-gallery`) |

---

## Разделы GUI и слоты для скриншотов

Интерфейс `ucc-gui` содержит 10 рабочих вкладок и баннер предупреждения о состоянии драйвера на вкладке `Overview`. Слоты ниже соответствуют путям из [`screenshots/current/manifest.json`](screenshots/current/manifest.json) (разрешение `1440x1000`, изолированная D-Bus-фикстура в режиме `offscreen`, без записи в реальное оборудование).

<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/overview.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/overview.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/profiles.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/profiles.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/cooler-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/cooler-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/watercool-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/watercool-settings.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/keyboard-hardware.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/keyboard-hardware.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/lightbar.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/lightbar.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/power-devices.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/power-devices.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/battery-processes.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/battery-processes.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/monitor.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/monitor.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/about.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/about.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/dark/driver-warning.png -->
<!-- SCREENSHOT PLACEHOLDER: screenshots/current/light/driver-warning.png -->

| Вкладка / состояние GUI | Содержимое | Слот (тёмная тема) | Слот (светлая тема) |
| --- | --- | --- | --- |
| Overview | Телеметрия CPU/GPU, активный профиль, быстрые контролы водянки, статус драйвера | `screenshots/current/dark/overview.png` | `screenshots/current/light/overview.png` |
| Profiles | Ядра и частоты CPU, лимиты мощности, зарядка, частота экрана, клавиатура, привязка профилей | `screenshots/current/dark/profiles.png` | `screenshots/current/light/profiles.png` |
| Cooler Settings | Температурные кривые CPU/GPU, копирование/сохранение пресетов, совместная скорость `sameSpeed` | `screenshots/current/dark/cooler-settings.png` | `screenshots/current/light/cooler-settings.png` |
| Watercool Settings | Подключение Bluetooth LCT, ручное управление вентилятором и помпой, LED, кривые | `screenshots/current/dark/watercool-settings.png` | `screenshots/current/light/watercool-settings.png` |
| Keyboard & Hardware | Поклавишный RGB-редактор, яркость, аппаратные переключатели (веб-камера, Fn Lock, экран) | `screenshots/current/dark/keyboard-hardware.png` | `screenshots/current/light/keyboard-hardware.png` |
| Lightbar | Цвет и яркость по режимам питания, 5 анимаций уведомлений, Silent, Welcome, Test | `screenshots/current/dark/lightbar.png` | `screenshots/current/light/lightbar.png` |
| Power & Devices | Раздельные политики CPU и устройств для AC / Type-C / Battery (governor, EPP, turbo, ASPM, PCI/USB PM, Wi-Fi) | `screenshots/current/dark/power-devices.png` | `screenshots/current/light/power-devices.png` |
| Battery & Processes | Расход батареи (Вт), пользовательские процессы NVIDIA, параметры запуска и частоты KDE | `screenshots/current/dark/battery-processes.png` | `screenshots/current/light/battery-processes.png` |
| Monitor | Графики истории (температура, мощность, частоты, обороты, напряжение), масштаб и маркеры | `screenshots/current/dark/monitor.png` | `screenshots/current/light/monitor.png` |
| About | Версия и коммит сборки, версия Qt, ссылки на исходники, авторы, лицензии, журналы | `screenshots/current/dark/about.png` | `screenshots/current/light/about.png` |
| Driver Warning | Красный баннер на `Overview` при сбое проверки `tuxedo-drivers` / аппаратного интерфейса | `screenshots/current/dark/driver-warning.png` | `screenshots/current/light/driver-warning.png` |

Повторная генерация галереи на изолированной шине D-Bus:

```bash
python3 contrib/frixit/capture-gallery.py --build build --output screenshots/current
```

---

## Сводка поддерживаемого оборудования

- **Семейства платформ**: ноутбуки Uniwill, Clevo, Mechrevo и TUXEDO, работающие через совместимые интерфейсы ядра `tuxedo-drivers`. Доступность функций (лимиты TDP/cTGP, поклавишный RGB, Lightbar, профили зарядки, переключатели веб-камеры/Fn Lock, Bluetooth-водянка) определяется динамически по возможностям драйвера и прошивки.
- **Основная проверенная система**: **MECHREVO YAOSHI Series-X6AR55xY** (Intel Core Ultra 9 275HX, NVIDIA RTX 5080 Laptop, CachyOS, Qt 6.12, KDE Wayland). Таблицы устройств upstream для Uniwill/Clevo/TUXEDO сохранены; полное повторное тестирование на остальных моделях не проводилось.
- **Физическая калибровка клавиатуры Mechrevo**: сохранены разреженные физические индексы LED: Enter = `56`, левый Shift = `21`, левый Ctrl = `0`.
- **Водяное охлаждение по Bluetooth (LCT)**: поддерживается на совместимых шасси Uniwill (`STELLARIS1XI04`, `STEPOL1XA04`, `STELLARIS1XI05`, `STELLARIS16I06`, `STELLARIS17I06`, `STELLARIS16A07`, `STELLARIS16I07`, `PCS_X6FR559Y`, `XNE16A25`, `XNE16E25`) через BlueZ GATT.
- **Требования к драйверу ядра**: `ucc-rusted` не включает модули ядра. Необходимы отдельно установленные совместимые `tuxedo-drivers` и заголовки текущего ядра (DKMS). Инструкции по восстановлению для Linux 7.3 приведены в [`contrib/frixit/drivers/README.md`](contrib/frixit/drivers/README.md), полная матрица устройств — в [`docs/HARDWARE_MATRIX.md`](docs/HARDWARE_MATRIX.md).
- **Исключённые функции**: принудительное ограничение температуры CPU и утилита `gpu-run` не перенесены. Необязательный пресет вентиляторов на 85 °C ([`contrib/frixit/fan-max-performance-85.json`](contrib/frixit/fan-max-performance-85.json)) выводит вентиляторы на 100% при 85 °C и не ограничивает температуру процессора.

---

## Установка

Основной репозиторий и релизы исходного кода: `https://github.com/Frixlt/UCC` (ветка `main`).

### Arch Linux / CachyOS (`PKGBUILD`)

Корневой [`PKGBUILD`](PKGBUILD) собирает пакет `ucc-rusted` (обеспечивает `uniwill-control-center`, заменяет `frixit-control-center`, конфликтует с `uniwill-control-center` и `uniwill-control-center-mechrevo`):

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC
UCC_SOURCE_DIR="$PWD" makepkg -si
```

### Debian / Ubuntu (DEB)

Метаданные пакета находятся в `debian/` (требуются `rustc >= 1.89`, `cargo`, `cmake >= 3.20`, `qt6-base-dev`, `qt6-charts-dev`, `nlohmann-json3-dev`, `libsystemd-dev`, `libxrandr-dev`, `libudev-dev`):

```bash
make deb
```

### Fedora / RHEL (RPM)

Спецификация пакета находится в [`ucc.spec`](ucc.spec):

```bash
make rpm
```

### Nix Flake

[`flake.nix`](flake.nix) экспортирует `packages.ucc`, `overlays.default` и `nixosModules.uccd`:

```bash
nix build .#ucc
nix flake check
```

> **Примечание**: рецепты DEB, RPM и Nix поддерживаются в репозитории, но не публикуются в виде проверенных бинарных релизов.

### Сборка из исходного кода

Зависимости: Rust toolchain `>= 1.89`, CMake `>= 3.20`, Ninja, компилятор C++20, `qt6-base`, `qt6-charts`, `nlohmann-json`, заголовочные файлы `systemd` / `dbus` / `polkit`.

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC

# 1. Сборка 7 Rust-бинарников времени выполнения и xtask
cargo build --workspace --bins --release --locked --target-dir target

# 2. Сборка автономного Qt6 GUI (без KF6, апплет Plasma отключён)
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_GUI=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_CLI=OFF \
  -DUCC_PACKAGE_RUST_RUNTIME=ON
cmake --build build --parallel 2

# 3. Установка GUI, Rust-бинарников, автодополнений и man-страницы
sudo cmake --install build
sudo ./target/release/xtask install-runtime /usr ./target/release
```

---

## Быстрый старт

### Службы systemd

Включение и запуск системного демона и обработчиков сна:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now uccd.service
sudo systemctl enable uccd-pre-sleep.service uccd-sleep.service
systemctl status uccd.service
```

### Командная строка (`ucc-cli`)

```bash
ucc-cli status                  # Сводка состояния системы (CPU, GPU, вентиляторы, водянка, зарядка)
ucc-cli monitor                 # Потоковый вывод телеметрии (псевдоним: mon)
ucc-cli profile list            # Список профилей (псевдоним: prof)
ucc-cli statemap                # Привязка профилей к AC / Battery / Type-C / Water Cooler
ucc-cli fan                     # Профили и кривые вентиляторов
ucc-cli keyboard                # Состояние подсветки клавиатуры (псевдоним: kb)
ucc-cli cooler                  # Управление Bluetooth-водянкой (псевдонимы: watercooler, wc)
ucc-cli charging                # Профили и пороги зарядки батареи (псевдоним: charge)
```

### GUI, трей и сессионный агент

```bash
ucc-gui                         # Запуск окна настроек Qt6 по требованию
ucc-desktop-tray                # Запуск автономного SNI-трея (автозапуск через XDG)
ucc-session-agent               # Запуск сессионного агента (автозапуск через XDG)
```

### Режим предпросмотра (только чтение, без записи в оборудование)

Запуск `ucc-gui` на изолированной фикстуре без обращения к `/etc/ucc` и реальным устройствам:

```bash
./run-preview.sh
```

---

## Конфигурация и инварианты безопасности

### Пути конфигурации и хранение

- **Системная конфигурация**: `/etc/ucc/settings` (основной путь, с резервным чтением `/etc/ucc/settings.json`; принадлежит `root`, создаётся или импортируется локально; персональные настройки и Bluetooth-адреса не включаются в публичные пакеты).
- **Пользовательские настройки**: `~/.config/ucc/settings.json`.
- **Атомарная запись**: обновление конфигурации выполняется через временный файл, синхронизацию на диск и атомарное переименование с сохранением резервной копии `.bak` для автоматического восстановления при повреждении файла.

### Разделение привилегий и изоляция systemd

- **Граница прав**: привилегированные операции с оборудованием выполняются только в `uccd` (и служебных root-утилитах `ucc-hardware`, `ucc-sleep`). `ucc-gui`, `ucc-cli`, `ucc-desktop-tray` и `ucc-session-agent` работают от имени обычного пользователя.
- **Авторизация Polkit**: вызовы D-Bus контролируются действиями `com.uniwill.uccd.read`, `com.uniwill.uccd.control` и `com.uniwill.uccd.manage-hardware` (с резервной проверкой `de.uniwill.uccd.*`). Активным локальным сессиям разрешено штатное управление.
- **Завершение процессов**: `ucc-desktop-tray` и `ucc-gui` завершают только NVIDIA-процессы текущего пользователя через `pidfd` после подтверждения. Привилегированный D-Bus API для завершения процессов в `uccd` отсутствует.
- **Ограничения `uccd.service`**: включены `ProtectSystem=strict`, `ProtectHome=yes`, `NoNewPrivileges=true`, `ProtectKernelModules=yes`, `ProtectControlGroups=yes`, `RestrictNamespaces=yes`, `LockPersonality=yes`, `MemoryDenyWriteExecute=yes` и `CapabilityBoundingSet=CAP_SYS_RAWIO CAP_NET_ADMIN CAP_SYS_ADMIN`.

### Аппаратные и поведенческие инварианты

- **Единственный контроллер оборудования**: `uccd.service` задаёт `Conflicts=tccd.service tuxedo-power.service`. Одновременно должен работать только один аппаратный демон.
- **Аварийный возврат вентиляторов**: `uccd.service` выполняет `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto`, возвращая управление вентиляторами прошивке даже при аварийном завершении демона.
- **Поведение кривых охлаждения**: при включённом `sameSpeed` оба внутренних вентилятора используют `max(cpu_fan_duty, gpu_fan_duty)`. Третий вентилятор не представлен отдельной редактируемой кривой в текущем интерфейсе драйвера. Ручные значения Fan/Pump для водянки применяются немедленно и сохраняются в системном профиле; режим `Auto` возвращает управление температурным кривым.
- **Приоритет политик питания**: ручной режим питания (`AC` / `Type-C` / `Battery`) имеет приоритет над автоматическим определением физического источника. Запросы D-Bus-приложений привязаны к соединению, могут повышать эффективный режим и автоматически снимаются при отключении клиента. При включённом управлении политиками устройств его параметры CPU имеют приоритет над активным профилем.
- **Приватность и режимы Lightbar**: режим `Flash (stays off)` удерживает Lightbar выключенным между уведомлениями после Save, смены питания и запуска демона. Режим `Silent` подавляет обычные анимации; `Test` обходит `Silent` и использует сохранённые параметры эффективного режима питания. Текст уведомлений в `uccd` не передаётся.

---

## Индекс документации

| Документ | Содержание |
| --- | --- |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Архитектура системы, границы крейтов, модель сериализации команд, `ADR-01` / `ADR-03` |
| [`docs/HARDWARE_MATRIX.md`](docs/HARDWARE_MATRIX.md) | Поддерживаемые шасси Uniwill/Clevo/Mechrevo/TUXEDO, проверки возможностей, калибровка LED |
| [`docs/DBUS_API.md`](docs/DBUS_API.md) | Полный справочник `com.uniwill.uccd` / `de.uniwill.uccd` (123 метода, 7 сигналов, действия Polkit) |
| [`docs/CLI_REFERENCE.md`](docs/CLI_REFERENCE.md) | Подкоманды `ucc-cli`, флаги, схемы JSON-вывода, коды возврата |
| [`docs/BUILDING.md`](docs/BUILDING.md) | Зависимости сборки, параметры CMake/Cargo, упаковка (Arch/DEB/RPM/Nix), режим предпросмотра |
| [`docs/TESTING.md`](docs/TESTING.md) | Тесты workspace, D-Bus interop-набор, фикстуры аудита GUI, границы проверок |
| [`docs/MIGRATION_AND_RECOVERY.md`](docs/MIGRATION_AND_RECOVERY.md) | Автономная миграция настроек, резервное копирование, откат, восстановление драйвера ядра |
| [`docs/COMPARISON.md`](docs/COMPARISON.md) | Функциональное и архитектурное сравнение с upstream UCC Revorked (`eb98bfa`) |
| [`docs/AI_AGENT_GUIDE.md`](docs/AI_AGENT_GUIDE.md) | Навигация по кодовой базе, тестовые стенды и ограничения для автоматизированных агентов |
| [`AGENTS.md`](AGENTS.md) | Правила коммитов, обязательные проверки Cargo, требования к фикстурам и привилегиям |

---

## Происхождение и лицензия

`ucc-rusted` сохраняет авторские заголовки, историю Git и уведомления об открытом ПО из проектов:

- [`noteMASTER11/UCC-Revorked`](https://github.com/noteMASTER11/UCC-Revorked) (базовый коммит upstream `eb98bfa`)
- [`nanomatters/ucc`](https://github.com/nanomatters/ucc) (исходная основа Uniwill Control Center)
- [`tuxedocomputers/tuxedo-control-center`](https://github.com/tuxedocomputers/tuxedo-control-center) (слой TUXEDO IO; внешние модули ядра распространяются под собственными лицензиями)

Подробная история происхождения и список сторонних компонентов приведены в [`PROJECT_ORIGINS.md`](PROJECT_ORIGINS.md) и [`THIRD_PARTY.md`](THIRD_PARTY.md).

Лицензия: **GPL-3.0-or-later**. Полный текст — [`COPYING`](COPYING).
