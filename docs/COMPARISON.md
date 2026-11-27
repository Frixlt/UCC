# Сравнение `ucc-rusted` с альтернативными инструментами управления оборудованием

Документ содержит фактическое сопоставление `ucc-rusted` с семью существующими проектами управления ноутбуками, системами охлаждения и видеокартами в Linux (и смежных платформах). Сравнение основано на исходном коде `ucc-rusted` (`0.0.1`) и публичной документации рассматриваемых проектов.

**Ограничение применимости**: `ucc-rusted` проверен на основной аппаратной конфигурации MECHREVO `X6AR55xY` (шасси Uniwill `0x1A`, Intel/NVIDIA Optimus + внешняя СЖО `LCT22002`, Hyprland). Синтетические тесты покрывают бизнес-логику, контракты D-Bus и изоляцию sysfs, но не заменяют аппаратную проверку на всех моделях линейки Uniwill/Clevo.

---

## Сводная таблица

| Критерий | `ucc-rusted` | [`open-uniwill-cc`](https://github.com/aspidovss/open-uniwill-cc) (`v1.9.2.2`) | [UCC Revorked](https://github.com/noteMASTER11/UCC-Revorked) (`eb98bfa`) | [TUXEDO Control Center](https://github.com/tuxedocomputers/tuxedo-control-center) | [Clevo Control Center (`ccenter`)](https://github.com/sawongam/clevo-control-center-linux) | LCT-утилиты ([`watercooler-manager`](https://github.com/tomups/watercooler-manager) / [`xmg-oasis-watercooler`](https://github.com/deadtrickster/xmg-oasis-watercooler)) | [CoreCtrl](https://gitlab.com/corectrl/corectrl) | [`asusctl` / `asusd`](https://github.com/OpenGamingCollective/asusctl) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **Целевое железо** | Uniwill / Tongfang (`INOU0000`, `0x1A`), Clevo (через `tuxedo_io`), LCT21001/LCT22002 | Uniwill (`INOU0000`: `0x10`, `0x1A`, `0x12`–`0x18`), эксп. Clevo/Tongfang (DCHU) и Thunderobot (`RW_GMWMI`), кулеры Flydigi BS1–BS3 Pro | Uniwill / Tongfang, Mechrevo, LCT21001/LCT22002 | Ноутбуки TUXEDO (Clevo и Uniwill/Tongfang), TUXEDO Aquaris | Ноутбуки Clevo с ACPI `CLV0001` (`\_SB.DCHU._DSM`) | Внешние BLE-СЖО `LCT21001` (Mk1) и `LCT22002` (Mk2): XMG OASIS, TUXEDO Aquaris, Eluktronics LPP, PCS Liquid | Десктопные и мобильные GPU (в первую очередь AMD AMDGPU) и CPU (`cpufreq`) | Ноутбуки ASUS ROG / TUF (`asus-nb-wmi`, `asus-armoury`) |
| **Поддерживаемые ОС** | Linux (активно); Windows запланирован в `M15` (`ADR-06`) | Linux и Windows 10/11 | Linux | Linux | Linux (порт Windows Clevo CC 3.0) | Linux и Windows (`watercooler-manager`); Linux (`xmg-oasis-watercooler`) | Linux | Linux |
| **Стек фонового демона** | Rust (Tokio, single-writer actor, `zbus`) | Python 3.9+ (`uwccd`) | C++20 / Qt6 (`uccd`) | Node.js 24 / TypeScript (`tccd`) | C# / .NET (`ccenterd`) | Python (`watercooler-manager` / `xmg-oasis-watercooler`, без root-демона) | C++ / Qt (helper через Polkit) | Rust (`asusd`, `<1 МБ` RAM в простое) |
| **GUI и системный трей** | Автономный C++20 `Qt6::Widgets` + `Qt6::Charts` GUI (0 МБ Qt в фоне); Rust SNI-трей (`ksni`), без `KF6`/`QML` | Windows: Edge App GUI (~150–200 МБ при открытии); Linux: трей `ayatana-appindicator3` | Qt6 Widgets/Charts GUI + опциональный KDE Plasma-апплет (`ucc-tray`, `KF6`/`QML`) | Electron + Angular GUI | Avalonia GUI (порт WPF) | GTK/AppIndicator трей (`watercooler-manager`) или PySide6 GUI (`xmg-oasis-watercooler`) | Qt GUI (минимизация в трей) | `rog-control-center` (Rust GUI + трей) |
| **CLI и внешний API** | `ucc-cli` (текст + JSON, completions, manpage), D-Bus (`com.uniwill.uccd`, 123 метода, 7 сигналов) | `uwccctl` (JSON, bash completion), HTTP/JSON API на `127.0.0.1` (опц. HTTPS для LAN / Home Assistant) | `ucc-cli`, D-Bus (`de.uniwill.uccd` / `com.uniwill.uccd`) | D-Bus (`com.tuxedocomputers.tccd`), без полноценного CLI уровня `ucc-cli` | `ccenterctl` | Нет системного D-Bus API для хоста | Нет CLI-демона (управление через GUI и профили приложений) | `asusctl` CLI + D-Bus (`xyz.ljones.Asusd`) |
| **Драйвер / доступ к EC** | `tuxedo-drivers-dkms` (`/dev/tuxedo_io`); запланирован Rust DKMS `uwcc-rs` (`M14`) | Linux: открытый DKMS `uwcc` (fallback `acpi_call`); Windows: `wmiacpi.sys` WMI или OEM `UWACPIDriver.sys` | `tuxedo-drivers-dkms` (`/dev/tuxedo_io`) | `tuxedo-drivers-dkms` (`/dev/tuxedo_io`) | Собственный DKMS `ccenter_dchu` (`/dev/ccenter_dchu`) | Не требуется (только BlueZ / Bluetooth LE GATT) | Стандартные драйверы ядра (`amdgpu`, `hwmon`, `cpufreq`) | Апстримный драйвер ядра (`asus-nb-wmi`, `asus-armoury` в mainline с Linux 6.19) |
| **Вентиляторы и кривые** | Кривые CPU/GPU, аппаратный `W_UW_FANAUTO` при сбое датчиков; `RamFan 1.5` запланирован в `M14` | 16-точечные кривые `RamFan 1.5` (флаги `CUME`/`UFME` на `0x1A`), Fan Boost, сброс в заводские | Кривые CPU/GPU через `tuxedo_io` | Кривые вентиляторов через `tuxedo_io`, профили | Режимы и пользовательские кривые вентиляторов Clevo | Только вентилятор и помпа внешней СЖО | Кривые вентиляторов дискретной GPU (преимущественно AMD) | Пользовательские кривые вентиляторов по профилям |
| **Лимиты мощности и GPU** | Профили AC / Type-C / Battery, PL/TDP, cTGP/Dynamic Boost, динамический NVML с защитой D3cold, per-mode матрица PCI/USB/Wi-Fi | PL1/PL2/PL4, TCC (60–100 °C), целевая температура GPU, cTGP, Dynamic Boost, `SMRW` (в Linux `uwcc`), `PWUP` (в Windows через OEM) | Параметры CPU/TDP в профилях, телеметрия NVIDIA | Профили CPU/частот/TDP, базовый мониторинг GPU | Режимы питания Clevo (Quiet/Power Saving/Performance/Entertainment); разгон CPU/GPU заблокирован | Нет | Разгон и андервольт AMD GPU (частоты ядра/VRAM, напряжения, power profiles), governor CPU, профили по исполняемому файлу | `platform_profile`, TDP (через `asus-armoury` на ядре 6.19+), GPU MUX (Integrated/Hybrid/Ultimate) |
| **Подсветка** | Клавиатура (калиброванные LED ID `Enter=56`, `LShift=21`, `LCtrl=0`), встроенный контроллер Lightbar (5 анимаций), прямые HID-транспорты (`M14`/`M15`) | Клавиатура (`048D:600B`, 17 аппаратных + 16 программных эффектов BetterRGB), раздельные зоны корпуса/логотипа/Lightbar (`048D:7001`) | Клавиатура (раскладки/профили upstream), без встроенного контроллера Lightbar | Подсветка клавиатуры (зоны/per-key на моделях TUXEDO) | Подсветка клавиатуры Clevo | Только RGB-подсветка внешней СЖО (head/fan LED на Mk2) | Нет | Aura RGB (встроенные эффекты и per-key), дисплеи AniMe Matrix / Slash |
| **СЖО (LCT21001/LCT22002)** | Встроена в `uccd` (BlueZ GATT, кривые Fan/Pump, запрет `PumpVoltage::V12`, keepalive, восстановление после сна) | Встроена в `uwccd` (кривые, профили, ограничения напряжения, контроль протока, цикл заполнения) | Встроена (Bluetooth, Fan/Pump/LED и кривые) | Поддержка TUXEDO Aquaris | Нет | Основная функция: пресеты, ручное напряжение 7V/8V/11V, RGB, контроль протока, цикл заполнения | Нет | Нет |
| **Авторизация и изоляция** | Polkit (`system-bus-name`, 3 уровня: `read`/`control`/`manage-hardware`), `O_NOFOLLOW`, `OffloadLanes`, `pidfd` | Ключ доступа на `127.0.0.1` (раздельные read/full ключи, группа `uwcc`), проверка раскладки `UniWillVariable`, подпись/MOK | D-Bus + Polkit | D-Bus + системный сервис root | Root-сервис `ccenterd` владеет `/dev/ccenter_dchu`; клиенты без root | Пользовательский процесс без root | Polkit helper для записи в sysfs | D-Bus + udev-активация `asusd.service` |
| **Лицензия** | `GPL-3.0-or-later` | Userspace: `MIT`; модуль ядра `uwcc`: `GPL-2.0`; опциональные OEM-файлы (`proprietary/`, `uwccd/lighting/oem/`) — проприетарные | `GPL-3.0-or-later` | `GPL-3.0` | `GPL-2.0` | Открытая лицензия (GitHub) | `GPL-3.0` | `MPL-2.0` |

---

## Архитектурные компромиссы по проектам

### 1. [`open-uniwill-cc`](https://github.com/aspidovss/open-uniwill-cc/blob/main/README.ru.md)

**Сильные стороны и ниша**:
- Реализована полноценная кроссплатформенная поддержка Linux и Windows 10/11, включая работу на Windows через штатный `wmiacpi.sys` (`root\WMI`, `AcpiTest_MULong`) без подписи собственного драйвера ядра.
- В Linux поставляет собственный открытый DKMS-модуль `uwcc` с поддержкой таблицы заводских пределов `SMRW`, 16-точечных кривых `RamFan 1.5` (`CUME`/`UFME`), температурных лимитов CPU (`TCC`) и GPU, а также прямого управления Lightbar `048D:7001`.
- Широкое покрытие периферии и интеграций: 16 программных эффектов BetterRGB, охлаждающие подставки Flydigi BS1–BS3 Pro, HTTP/JSON API (с опциональным HTTPS для Home Assistant) и экспериментальные бэкенды для Clevo/Tongfang (DCHU) и Thunderobot/Leishen (`RW_GMWMI`).

**Компромиссы относительно `ucc-rusted`**:
- Фоновая служба написана на Python 3.9+, а GUI в Windows использует Microsoft Edge в режиме приложения (~150–200 МБ ОЗУ при открытии).
- В Windows часть функций (`PWUP` и быстрый доступ к EC вместо ~125 мс через WMI) опирается на опциональные несвободные OEM-компоненты (`UWACPIDriver.sys`, `proprietary/`). В `ucc-rusted` проприетарные модули исключены архитектурно (`ADR-05`).
- Нет интеграции с D-Bus-экосистемой `com.uniwill.uccd` / Polkit, per-mode матрицы устройств (AC / Type-C / Battery для PCI/USB/Wi-Fi) и Wayland/KScreen session agent.

### 2. [UCC Revorked](https://github.com/noteMASTER11/UCC-Revorked) (C++ baseline, на основе [nanomatters/ucc](https://github.com/nanomatters/ucc))

**Сильные стороны и ниша**:
- Проверенная C++/Qt6 кодовая база с нативным Plasma-апплетом (`ucc-tray`) для пользователей KDE Plasma, не требующая Rust-инструментария.
- Полная функциональность профилей, вентиляторов, клавиатуры и BLE-СЖО из коробки для Linux на базе `tuxedo-drivers`.

**Компромиссы относительно `ucc-rusted`**:
- Фоновый демон `uccd` и сопутствующие сервисы написаны на C++/Qt и держат Qt-библиотеки в памяти постоянно; использование `ucc-tray` тянет зависимости `KF6` / `Plasma` / `QML`.
- Отсутствуют: трёхрежимная модель питания (`AC` / `Type-C` / `Battery`) с матрицей устройств и запросами приложений (`power requests`), встроенный контроллер анимаций Lightbar, изолированные offload-очереди D-Bus (`OffloadLanes`), `SysfsRoot`-песочница и динамический NVML с гарантией непробуждения D3cold.

### 3. [TUXEDO Control Center (TCC)](https://github.com/tuxedocomputers/tuxedo-control-center)

**Сильные стороны и ниша**:
- Официальная OEM-поддержка от TUXEDO Computers, готовые `.deb`/`.rpm` репозитории, гарантированная синхронизация с `tuxedo-drivers` и верифицированные профили для всего модельного ряда TUXEDO (как Clevo, так и Uniwill/Tongfang).

**Компромиссы относительно `ucc-rusted`**:
- Архитектура на базе Node.js 24 (`tccd`) и Electron/Angular GUI влечёт повышенное фоновое потребление ОЗУ и дискового пространства.
- Функции, специфичные для сторонних OEM-шасси (калиброванная раскладка Mechrevo `0x1A`, Lightbar `048D:7001`, per-mode матрица PCI/USB/Wi-Fi, Wayland-трей без Electron), не входят в область поддержки TCC.
- Нельзя запускать одновременно с `uccd` или `uwccd` из-за конфликта за EC и `/dev/tuxedo_io`.

### 4. [Clevo Control Center (`ccenter`)](https://github.com/sawongam/clevo-control-center-linux)

**Сильные стороны и ниша**:
- Прямой Linux-порт OEM Clevo Control Center 3.0 для плат Clevo с ACPI-устройством `CLV0001` (`\_SB.DCHU._DSM`): собственный компактный DKMS-модуль `ccenter_dchu`, сервис `ccenterd`, CLI `ccenterctl` и привычный интерфейс (Avalonia-порт WPF) для режимов питания, кривых вентиляторов и подсветки клавиатуры.

**Компромиссы относительно `ucc-rusted`**:
- Рассчитан исключительно на прошивки Clevo DCHU и неприменим к шасси Uniwill (`INOU0000`) и внешним СЖО LCT.
- На текущий момент подтверждён на ограниченном числе плат (в документации указана Clevo `NH5x_NH7xHP`), требует рантайм .NET/Avalonia; управление разгоном CPU/GPU не подключено к бэкенду.

### 5. LCT — специализированные утилиты для СЖО ([`watercooler-manager`](https://github.com/tomups/watercooler-manager), [`xmg-oasis-watercooler`](https://github.com/deadtrickster/xmg-oasis-watercooler))

**Сильные стороны и ниша**:
- Узкоспециализированные утилиты для внешних BLE-СЖО на контроллерах `LCT21001` (Mk1) и `LCT22002` (Mk2) (протокол задокументирован в [LCT BLE Protocol](https://gist.github.com/Chocapikk/0baa8e68b87f8ed0873c39504184ebc6)).
- Не требуют root-демона, DKMS-модулей ядра и вмешательства в EC ноутбука: работают в пользовательском пространстве поверх BlueZ (`watercooler-manager` также поддерживает Windows), включают цикл заполнения (priming), контроль протока, выбор напряжения помпы (7V/8V/11V) и RGB.
- Оптимальный выбор, если на ноутбуке уже используется другой контроллер EC или требуется минимальный инструмент только для СЖО.

**Компромиссы относительно `ucc-rusted`**:
- Не управляют EC ноутбука, профилями питания, TDP CPU/GPU, внутренними вентиляторами, клавиатурой, Lightbar и энергосбережением периферии.

### 6. [CoreCtrl](https://gitlab.com/corectrl/corectrl)

**Сильные стороны и ниша**:
- Один из наиболее функциональных инструментов для тонкой настройки и разгона/андервольта видеокарт AMD (`amdgpu`) в Linux: управление частотами GPU/VRAM, напряжениями, power profiles, кривыми вентиляторов видеокарты и governor CPU.
- Автоматическое переключение профилей по запущенному исполняемому файлу (удобно для десктопов и игровых профилей, см. [ArchWiki AMDGPU](https://wiki.archlinux.org/title/AMDGPU) и [Fedora GPU Overclocking](https://docs.fedoraproject.org/en-US/gaming/gpu-overclocking/)).

**Компромиссы относительно `ucc-rusted`**:
- Не предназначен для управления ноутбучным EC (Uniwill/Clevo), встроенными вентиляторами ноутбука, лимитами заряда батареи, клавиатурной подсветкой, Lightbar и BLE-СЖО.
- Не поддерживает разгон/управление NVIDIA Optimus dGPU с защитой от пробуждения D3cold и может конфликтовать с `lm-sensors`/`fancontrol` при захвате вентиляторов GPU.
- Может использоваться параллельно с `ucc-rusted` только на системах с AMD GPU при условии отсутствия конфликта по `cpufreq` governor.

### 7. [`asusctl` / `asusd`](https://github.com/OpenGamingCollective/asusctl) ([ArchWiki](https://wiki.archlinux.org/title/Asusctl))

**Сильные стороны и ниша**:
- Архитектурно наиболее близкий аналог в экосистеме Rust: минимальный системный демон `asusd` (`<1 МБ` ОЗУ в простое), безопасный D-Bus интерфейс (`xyz.ljones.Asusd`), CLI `asusctl` и GUI `rog-control-center`.
- Опирается на апстримные драйверы ядра Linux (`asus-nb-wmi` и `asus-armoury`, включённый в mainline с Linux 6.19) без необходимости поддерживать сторонние DKMS-модули.
- Глубокая поддержка аппаратных возможностей ASUS ROG/TUF: GPU MUX (Integrated/Hybrid/Ultimate с отложенным применением через `asus-shutdown.service`), Aura per-key RGB, дисплеи AniMe Matrix / Slash, зарядные пороги и кривые вентиляторов.

**Компромиссы относительно `ucc-rusted`**:
- Жёстко привязан к ACPI/WMI-интерфейсам ноутбуков ASUS; на шасси Uniwill/Tongfang/Clevo предоставляет лишь минимальный базовый функционал.

---

## Текущие ограничения `ucc-rusted`

Для объективной оценки границ применимости:
1. **Зависимость от `tuxedo-drivers-dkms` до `M14`**: до реализации собственного Rust-драйвера `uwcc-rs` (`ADR-05`) демон использует `/dev/tuxedo_io`, поэтому на новых ядрах Linux возможна потребность в патчах DKMS, а часть расширенных функций EC `0x1A` (`SMRW`, `TCC`, `PWUP`, аппаратный `RamFan 1.5`) ещё не активна.
2. **Только Linux в текущей поставке**: кроссплатформенные абстракции (`WindowsWmiEcBackend`, `UniwillHidTransport`, JSON-RPC IPC) заложены по `ADR-06`, но полноценная сборка и установщик Windows относятся к этапу `M15`.
3. **Ограниченная матрица проверенного железа**: основная верификация выполнена на MECHREVO `X6AR55xY` (`0x1A`) + `LCT22002`; для других моделей Uniwill/Clevo возможны отличия в раскладке LED и поддерживаемых регистрах EC.
4. **Нет разгона десктопных GPU**: `ucc-rusted` не заменяет [CoreCtrl](https://gitlab.com/corectrl/corectrl) или LACT для тонкой настройки частот/напряжений AMD GPU.
