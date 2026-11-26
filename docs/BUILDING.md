# Сборка, установка и запуск

[Frixit/UCC README](../README.md) · [Проверки](TESTING.md) ·
[Миграция Python/TCC](../contrib/frixit/README.md)

## Зависимости

Для релизной сборки нужны Linux, Rust и Cargo, CMake 3.20+, C++20-компилятор,
Ninja, Git, pkg-config, Qt 6 Core/DBus/Gui/Widgets/Charts и libxcb. Версия Rust
закреплена в [rust-toolchain.toml](../rust-toolchain.toml). Для тестов C++ также
нужны Qt Test, Qt Bluetooth, libudev и dbus-run-session. GJS и Node.js запускают
проверки GNOME.

В Arch/CachyOS:

```bash
sudo pacman -S --needed base-devel rust cmake ninja git pkgconf \
  qt6-base qt6-charts qt6-connectivity qt6-wayland systemd-libs \
  dbus polkit bluez python gjs nodejs libxcb nlohmann-json
```

В Debian потребуются зависимости из [debian/control](../debian/control).
Релизный GUI использует Qt Widgets. Rust-трей регистрируется в
StatusNotifierItem, а пользовательский агент запускается через XDG autostart.

## Исходники и опции

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC
make build
```

Окно GUI собирается в `build/bin/ucc-gui`. Rust-демон, CLI, фоновые клиенты и
утилиты находятся в `target/release/`. Для изолированных legacy-тестов
конфигурируйте отдельный CMake build с `BUILD_TESTS=ON`, `BUILD_TRAY=OFF`.

| Опция | По умолчанию | Назначение |
| --- | --- | --- |
| BUILD_GUI | ON | Основное окно |
| BUILD_CLI | ON | Командная строка |
| BUILD_DESKTOP_TRAY | ON | Самостоятельный Qt-трей |
| BUILD_SESSION_AGENT | ON | Lightbar relay, уведомления питания/драйвера и предложения KDE/Hyprland |
| BUILD_TRAY | ON | Дополнительный Plasma-апплет; для обычной Qt-сборки задайте OFF |
| BUILD_GNOME | ON | Установка файлов GNOME extension |
| BUILD_TESTS | OFF | Тесты Qt/Python/GNOME |
| UCC_PACKAGE_RUST_RUNTIME | OFF | Установка GUI и интеграционных файлов для Rust-бинарников |
| UCC_READ_ONLY_PREVIEW | OFF | Клиенты с заблокированными аппаратными записями |

## Arch-пакет

Основной [PKGBUILD](../PKGBUILD) собирает исходники релиза `v0.3.0` из Frixlt/UCC,
прогоняет изолированные проверки и создаёт `frixit-control-center`.
Для сборки из текущего checkout задайте `UCC_SOURCE_DIR`:

```bash
UCC_SOURCE_DIR="$PWD" makepkg -f
# Установить уже проверенный пакет:
sudo pacman -U ./frixit-control-center-0.3.0-1-x86_64.pkg.tar.zst
```

Пакет не содержит импортированных личных настроек и сам не включает/запускает
службу при первой установке. Для перехода с Python/TCC сначала прочитайте
[DEPLOYMENT.md](../contrib/frixit/DEPLOYMENT.md). Старый рецепт в
`contrib/mechrevo/arch/ucc` оставлен как совместимый вход к основному PKGBUILD.

## Ручная установка

Используйте пакет или ручную установку; не смешивайте владение одними файлами
между pacman и `cmake --install`.

```bash
sudo cmake --install build
sudo target/release/xtask install-runtime /usr target/release
sudo install -d -m 0755 /etc/ucc
sudo systemctl daemon-reload
sudo systemctl reload dbus
sudo systemctl enable --now uccd.service
sudo systemctl enable uccd-sleep.service
ucc-gui
```

Перед запуском uccd остановите старые контроллеры TCC/Python. Нужен совместимый
аппаратный драйвер отдельно от приложения; после обновления ядра проверьте DKMS.
[Восстановление на Linux 7.3](../contrib/frixit/drivers/README.md).

Демон установлен в `/usr/bin/uccd`, helper-программы — `/usr/lib/ucc`.
Основная конфигурация — `/etc/ucc/settings`; необязательный private файл водянки —
`/etc/ucc/aquaris.json`. Сохраняйте резервные копии этих файлов локально.
GUI запускается пользователем. Трей и session agent устанавливают XDG-autostart
файлы и запускаются при следующем входе. Для текущей сессии можно выполнить
`ucc-desktop-tray` и `ucc-session-agent`; повторные экземпляры не нужны.

`ucc-gui --minimized` учитывает наличие трея; `ucc-gui --show` явно показывает окно.
Настройка start minimized находится в Battery & Processes. Состояние темы и запуска
хранится в пользовательских настройках Qt под `UniwillControlCenter`.

`uccd-sleep.service` сохраняет ручной режим и enable-состояние водянки,
останавливает демон перед сном и восстанавливает его после сна, если он работал.
`uccd-pre-sleep.service` — совместимый alias. При обновлении старого вручную
включённого hook используйте `sudo systemctl reenable uccd-sleep.service`.
Настоящий аппаратный suspend/resume ещё не прошёл подтверждённую проверку.

## Обновление

Сохраните `/etc/ucc` и установите новый пакет. При обновлении раннего private
пакета hook сохраняет прежний package-owned settings и восстанавливает его,
если pacman убрал файл в `.pacsave`; существующие новые настройки не заменяются. Затем перезапустите демон и
пользовательские GUI/трей/session agent, чтобы они загрузили новые бинарники.
При необходимости сохраните текущий ручной режим/водянку через `ucc-sleep`
перед плановым рестартом; это helper для root, а не команда пользовательского GUI.
Не используйте installer первоначального перехода для обновления существующего UCC.

```bash
systemctl status uccd.service
journalctl -u uccd.service -b
ucc-cli --help
ucc-cli power status
ucc-cli devices status
ucc-cli lightbar status
```

## Read-only preview и другие дистрибутивы

`./run-preview.sh` использует отдельный `build-preview/`. Он читает данные демона,
но блокирует аппаратные команды и сохранение профилей/политик. Не устанавливайте
preview вместо рабочего пакета.

`make dist` архивирует текущий коммит и добавляет зависимости Rust из `Cargo.lock`.
Зависимости загружаются на этапе подготовки архива. `make deb` и `make rpm`
собирают из этого архива в офлайн-режиме Cargo. Для Rust-комплекта во временном
каталоге используйте `cargo xtask dist <каталог> target/release` после сборки.

Nix input форка — `github:Frixlt/UCC`, модуль `ucc.nixosModules.default` и опция
`services.uccd.enable`. Для локального checkout используйте `nix build .#ucc`.

## Hyprland

Session agent работает через стандартный `org.freedesktop.Notifications`: уведомления
об изменении физического источника питания не зависят от KDE и настройки
предложений частоты. Требуется работающий сервер уведомлений (например, Noctalia,
mako или dunst) и запущенный `ucc-session-agent` в пользовательской сессии.
XDG-autostart обычно запускает агент автоматически; при ручном запуске Hyprland
без autostart добавьте `ucc-session-agent` в запуск сессии. Повторный экземпляр
отсеивается через пользовательский lock-файл.

Предложения частоты поддерживают KDE (`kscreen-doctor`) и Hyprland 0.55+ с Lua
(`hyprctl`). Используются реальные доступные режимы внутреннего экрана, разрешение
сохраняется. Изменение выполняется только по действию уведомления и проверяется
повторным чтением состояния. Lua-обновление меняет поле `mode` существующего
правила монитора, сохраняя масштаб, положение и цветовой профиль.
[Документация hyprctl](https://wiki.hypr.land/configuring/core/advanced-configuration/using-hyprctl/)
и [режимов монитора](https://wiki.hypr.land/configuring/core/monitors/modes/).

Для разрешения сна NVIDIA снимите **Power Devices → Keep NVIDIA awake (Blackwell
workaround)** и нажмите **Save and apply**. В колонке активного режима для NVIDIA
должно стоять `auto`. Демон также отключает persistence mode при переходе к `auto`;
аппаратные задачи или подключённый к NVIDIA экран могут продолжать держать карту
активной. Периодический мониторинг пропускает NVML-пробы спящей карты. При `auto` NVML
опрашивается не чаще раза в 16 секунд, чтобы дать драйверу завершить переход
в сон; между опросами сохраняется последняя выборка, при сне она очищается.

Частоты Intel GPU читаются через интерфейсы обоих драйверов: DRM-атрибуты `i915`
и `device/tile0/gt0/freq0` для `xe`. Показание 0 МГц допустимо в простое. Мощность
берётся из GPU hwmon или RAPL-домена по имени. Температура отображается только
при наличии собственного датчика GPU; температура CPU не подставляется вместо неё.

## Частота экрана в профиле и общая настройка зарядки

В **Profiles → Display and Keyboard → Display refresh rate** выберите частоту
или **Do not change**. Поля `display.useRefRate` и `display.refreshRate` сохраняются
в профиле. Для профиля батареи можно выбрать 60 Гц, для профиля сети — 240 Гц;
назначение профилей AC / Battery / USB-C по-прежнему задаётся кнопками активации.
Сохранение и применение — отдельные действия. В KDE/Hyprland частоту применяет
`ucc-session-agent` при запуске, активации профиля и пробуждении. Сохраняются
текущее разрешение, масштаб и правило цветового профиля; неподдерживаемая частота
не заменяется произвольным режимом. Настроенная частота профиля имеет приоритет
над предложениями частоты в уведомлениях.

**Battery & Processes → Battery charging** содержит общие режим зарядки,
приоритет USB-C и поддерживаемые батареей пороги. **Save and apply** сохраняет
их отдельно от профилей, в основных настройках демона, и восстанавливает после
его запуска. Смена профилей производительности не меняет зарядку. Поля зарядки
в старых профилях сохраняются для совместимости, но больше не применяются; при
сохранении профиля из нового GUI эти поля не включаются.

В личной Hyprland-конфигурации `frixit-hypr-power` управление частотой передано
агенту UCC; политика обоев и эффектов остаётся в личном скрипте.

## Выбор отдельных ядер CPU

В **Profiles → System performance → CPU frequency control → CPU selection**
выберите **Choose individual CPUs**. Отмеченные CPU останутся включёнными,
снятые галочки отключают соответствующие логические CPU. Кнопки **All CPUs**,
**P-cores**, **E-cores** заполняют набор; обязательные CPU сохраняются в каждом
наборе. На проверенной Mechrevo-машине ядро Linux не предоставляет hotplug для
CPU 0, поэтому оно остаётся отмеченным и заблокированным. P/E определяется через
CPUID на доступных CPU и сохраняется в памяти демона при отключении ядра.
Неизвестный тип показывается без обозначения P/E.

**Save** сохраняет набор в профиле, **Apply** применяет его. При активации
другого профиля применяется его собственный набор. Режим **By number of cores**
сохраняет прежнюю работу ползунка для старых профилей. Новый массив
`cpu.enabledCores` содержит точные логические CPU ID и имеет приоритет над
`cpu.onlineCores`. Пустые наборы, дубликаты, отсутствующие CPU и попытки убрать
обязательный CPU отклоняются. Новые ядра включаются перед отключением остальных.

Настройки governor/EPP/частот применяются ко всем выбранным CPU, включая ядра
после разрыва в нумерации. При отключённом turbo верхняя граница частоты каждого
Intel-ядра ограничивается его `base_frequency`; это предотвращает сброс набора
из-за различных частот P- и E-ядер.
