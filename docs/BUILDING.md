# Сборка, установка и запуск

[Frixit/UCC README](../README.md) · [Проверки](TESTING.md) ·
[Миграция Python/TCC](../contrib/frixit/README.md)

## Зависимости

Linux, CMake 3.20+, C++20-компилятор, Ninja/Make, Git, pkg-config,
Qt 6 Core/DBus/Gui/Widgets/Qml/Quick/QuickControls2/Bluetooth/Charts и libudev.
Для тестов нужны Qt Test, Python 3 и dbus-run-session. GJS и Node.js добавляют
проверки GNOME. Локально проверено с Qt 6.12; это версия проверки, не минимальная
заявленная версия Qt.

В Arch/CachyOS:

```bash
sudo pacman -S --needed base-devel cmake ninja git pkgconf \
  qt6-base qt6-charts qt6-connectivity qt6-declarative qt6-wayland \
  systemd-libs dbus polkit bluez python gjs nodejs libxcb nlohmann-json \
  kwindowsystem kstatusnotifieritem
```

KWindowSystem и KStatusNotifierItem нужны для проверенного пути активации трея
в KDE Wayland. CMake умеет собрать fallback без них, но его поведение фокуса
зависит от Qt/композитора. KDE Plasma для самостоятельного трея не требуется.

Для Qt-сборки без Plasma-апплета в Debian 13:

```bash
sudo apt install build-essential cmake ninja-build git pkg-config \
  qt6-base-dev qt6-declarative-dev qt6-connectivity-dev qt6-charts-dev \
  qt6-svg-dev qt6-wayland libudev-dev libxcb1-dev \
  libkf6windowsystem-dev libkf6statusnotifieritem-dev \
  dbus bluez polkitd python3 gjs nodejs nlohmann-json3-dev
```

Набор пакетов Debian приведён как инструкция для сборки; бинарный DEB этого
релиза не проверен. Для `BUILD_TRAY=ON` дополнительно нужны ECM, KF6 Config,
CoreAddons и Plasma development files.

## Исходники и опции

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC
git checkout v0.3.0
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_TRAY=OFF -DBUILD_TESTS=ON
cmake --build build -j2
```

На занятом ноутбуке используйте `-j1`; тестирование не требует нагрузочного
прогона CPU/GPU. Бинарники находятся в `build/bin`, библиотека — `build/lib`.

| Опция | По умолчанию | Назначение |
| --- | --- | --- |
| BUILD_GUI | ON | Основное окно |
| BUILD_CLI | ON | Командная строка |
| BUILD_DESKTOP_TRAY | ON | Самостоятельный Qt-трей |
| BUILD_SESSION_AGENT | ON | Lightbar relay, уведомления драйвера и предложения KScreen |
| BUILD_TRAY | ON | Дополнительный Plasma-апплет; для обычной Qt-сборки задайте OFF |
| BUILD_GNOME | ON | Установка файлов GNOME extension |
| BUILD_TESTS | OFF | Тесты Qt/Python/GNOME |
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
sudo ldconfig
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

DEB/RPM/Nix-рецепты унаследованы и требуют отдельной проверки установленных файлов,
зависимостей и service paths. Nix input нашего форка: `github:Frixlt/UCC`, модуль
`ucc.nixosModules.default`, опция `services.uccd.enable`. Наличие рецепта не означает
проверенный релиз для этого дистрибутива.
