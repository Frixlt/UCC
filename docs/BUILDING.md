# Сборка, упаковка и установка `ucc-rusted`

[Проверки и тестирование](TESTING.md) · [Миграция и восстановление](MIGRATION_AND_RECOVERY.md)

Документ описывает сборку Rust-пространства `ucc-rusted`, автономного Qt6-клиента `ucc-gui`, сборки `UCC_READ_ONLY_PREVIEW`, автоматизацию через `cargo xtask` и упаковку для Arch Linux, Debian/Ubuntu, Fedora/RPM и Nix/NixOS.

## 1. Требования к окружению сборки

### Базовые зависимости (Rust runtime + автономный Qt6 GUI)

| Компонент | Минимальная версия / пакет | Назначение |
| --- | --- | --- |
| Rust / Cargo | MSRV `1.89` (`Cargo.toml`), закреплённый тулчейн `1.99.1` (`rust-toolchain.toml` с `rustfmt`, `clippy`) | Сборка всех 10 крейтов рабочего пространства (`edition = "2021"`) |
| Компилятор C++ | C++20 (`g++ >= 9` или совместимый Clang) | Сборка `libucc-dbus`, `ucc-desktop`, `ucc-gui` |
| CMake / Ninja | `cmake >= 3.20`, `ninja` | Конфигурация и сборка Qt6-клиента |
| Инструменты | `git`, `pkg-config` (`pkgconf`) | Определение git-хеша версии и поиск системных библиотек |
| Qt6 | `Qt6::Core`, `Qt6::DBus`, `Qt6::Gui`, `Qt6::Widgets`, `Qt6::Charts`, `Qt6::Svg` | Графический интерфейс `ucc-gui` и рендеринг SVG-иконок |
| Системные библиотеки | `libxcb`, `systemd-libs`, `dbus`, `polkit`, `hicolor-icon-theme`, `nlohmann-json` | X11 backdrop blur (`UCC_XCB`), D-Bus, Polkit, systemd-интеграция |
| Опционально (runtime) | `tuxedo-drivers`, `bluez`, `kscreen`, `qt6-wayland` | Интерфейс `/dev/tuxedo_io`, BLE-водянка, смена частоты в KDE, нативный Wayland |

### Архитектурное ограничение по зависимостям GUI (ADR-01)

Для релизной сборки автономного Qt6 GUI с Rust runtime (`-DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF`) **не требуются**:

- **KF6 / Plasma / ECM**: `extra-cmake-modules`, `KF6Config`, `KF6CoreAddons`, `KF6WindowSystem`, `KF6StatusNotifierItem`, `Plasma`;
- **Qt6 QML / Quick**: `Qt6Qml`, `Qt6Quick` (`qt6-declarative`);
- **Qt6 Bluetooth**: `Qt6Bluetooth` (`qt6-connectivity`).

`KF6` и `Plasma` запрашиваются только при `-DBUILD_TRAY=ON` (устаревший Plasma-апплет) или при `-DUCC_PACKAGE_RUST_RUNTIME=OFF`. `Qt6Bluetooth` и `libudev` требуются только при сборке legacy C++ `uccd` или C++ unit-тестов (`-DBUILD_TESTS=ON`).

### Установка зависимостей по дистрибутивам

**Arch Linux / CachyOS:**

```bash
# Минимальный набор для релизной сборки Rust runtime + автономного Qt6 GUI
sudo pacman -S --needed base-devel rust cmake ninja git pkgconf \
  qt6-base qt6-charts qt6-svg qt6-wayland systemd-libs \
  dbus polkit hicolor-icon-theme libxcb nlohmann-json

# Дополнительно для CTest (C++/Python/GNOME-тесты)
sudo pacman -S --needed qt6-connectivity python gjs nodejs
```

**Debian / Ubuntu** (соответствует `debian/control`):

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  debhelper cargo rustc cmake ninja-build g++ git pkgconf \
  nlohmann-json3-dev qt6-base-dev qt6-charts-dev qt6-svg-dev qt6-tools-dev \
  libsystemd-dev libxrandr-dev libudev-dev libxcb1-dev dbus policykit-1
```

**Fedora / RHEL** (соответствует `ucc.spec`):

```bash
sudo dnf install -y \
  cmake ninja-build cargo rust gcc-c++ git pkgconf-pkg-config \
  nlohmann-json-devel qt6-qtbase-devel qt6-qtcharts-devel qt6-qtsvg-devel \
  systemd-devel libXrandr-devel libgudev-devel libxcb-devel dbus polkit
```

---

## 2. Сборка Rust-пространства и офлайн-архива

Рабочее пространство `Cargo.toml` включает 10 крейтов:

- `crates/ucc-core` — доменные модели, валидация, DTO;
- `crates/ucc-config` — загрузка, атомарная запись, резервные копии, импорт TCC;
- `crates/ucc-hardware` — `/dev/tuxedo_io`, CPU/GPU sysfs, NVML, BlueZ BLE;
- `crates/ucc-dbus` — контракты и интерфейсы `com.uniwill.uccd`;
- `crates/uccd` — демон `uccd` и root-утилиты `ucc-hardware`, `ucc-sleep`, `ucc-import-check`;
- `crates/ucc-cli` — CLI `ucc-cli`;
- `crates/ucc-desktop` — системный трей `ucc-desktop-tray` (StatusNotifierItem);
- `crates/ucc-session-agent` — пользовательский агент `ucc-session-agent`;
- `crates/ucc-lighting` — контроллеры клавиатуры и Lightbar;
- `crates/xtask` — задачи сборки, упаковки и верификации.

### Локальная сборка Rust

При наличии кеша зависимостей (`cargo fetch --locked`) или вендоринга:

```bash
cargo build --workspace --release --locked --offline
```

Сборка только исполняемых файлов в выделенный каталог (как в `Makefile`, `PKGBUILD`, `ucc.spec`):

```bash
cargo build --workspace --bins --release --locked --offline --target-dir target
```

Результирующие бинарники в `target/release/`: `uccd`, `ucc-cli`, `ucc-desktop-tray`, `ucc-session-agent`, `ucc-hardware`, `ucc-sleep`, `ucc-import-check`, `xtask`.

### Подготовка офлайн-архива с вендорингом зависимостей

Для воспроизводимой офлайн-сборки пакетов (`make deb`, `make rpm`, сборочные фермы без доступа в сеть) используется `make dist`:

```bash
make dist
```

Этапы выполнения `make dist`:
1. Экспорт `HEAD` через `git archive --format=tar --prefix=ucc-<VERSION>/`.
2. Запись короткого commit SHA в `.release-commit` (используется `xtask` при отсутствии каталога `.git`).
3. Вызов `bash contrib/package-rust-vendor.sh <export-dir>`:
   - выполняет `cargo vendor --locked --versioned-dirs vendor`;
   - дописывает секции `[source.crates-io]` и `[source.vendored-sources]` в `.cargo/config.toml` внутри экспорта.
4. Упаковка в `dist/ucc-<VERSION>-<GITHASH>.tar.gz` и генерация `dist/ucc-<VERSION>-<GITHASH>.tar.gz.sha256`.

Прямой вызов скрипта вендоринга на уже распакованном дереве исходников:

```bash
bash contrib/package-rust-vendor.sh /path/to/exported/ucc-0.0.1
```

---

## 3. Сборка автономного Qt6 `ucc-gui` и Read-Only Preview

### Релизная сборка Qt6 `ucc-gui` для Rust runtime

```bash
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_GUI=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_CLI=OFF \
  -DUCC_PACKAGE_RUST_RUNTIME=ON
cmake --build build --parallel 2
```

При `-DUCC_PACKAGE_RUST_RUNTIME=ON`:
- CMake собирает `libucc-dbus`, `ucc-desktop-support` и `ucc-gui` (с определением `UCC_PACKAGE_RUST_RUNTIME=1`);
- `uccd/CMakeLists.txt` устанавливает только systemd/D-Bus/Polkit-файлы и не собирает C++ `uccd` (если `-DBUILD_TESTS=OFF`);
- `ucc-desktop/CMakeLists.txt` и `ucc-session/CMakeLists.txt` устанавливают только `.desktop`-файлы автозапуска, не перезаписывая Rust-бинарники `ucc-desktop-tray` и `ucc-session-agent`;
- `KF6WindowSystem` и `KF6StatusNotifierItem` не ищутся и не линкуются.

Быстрая полная локальная сборка (Rust + Qt6 GUI):

```bash
make build
```

### Таблица опций CMake (`CMakeLists.txt`)

| Опция | По умолчанию | Назначение |
| --- | --- | --- |
| `BUILD_GUI` | `ON` | Сборка основного окна `ucc-gui` |
| `BUILD_CLI` | `ON` | Сборка legacy C++ CLI (при `UCC_PACKAGE_RUST_RUNTIME=ON` и `BUILD_TESTS=OFF` пропускается) |
| `BUILD_DESKTOP_TRAY` | `ON` | Поддержка `ucc-desktop-support` и установка `ucc-desktop-tray.desktop` |
| `BUILD_TRAY` | `ON` | Устаревший Plasma-апплет (требует `ECM`/`KF6`/`Plasma`); для Rust runtime задавать `OFF` |
| `BUILD_SESSION_AGENT` | `ON` | Установка `ucc-session-agent.desktop` (и C++-библиотеки для тестов) |
| `BUILD_GNOME` | `ON` | Установка расширения GNOME Shell (`ucc@uniwill.com`) |
| `BUILD_TESTS` | `OFF` | Включение CTest (`Qt6::Test`, C++/Python/GNOME/interop-тесты) |
| `UCC_PACKAGE_RUST_RUNTIME` | `OFF` | Режим упаковки Qt6 GUI и системных интеграций поверх Rust-бинарников |
| `UCC_READ_ONLY_PREVIEW` | `OFF` | Сборка preview-клиентов с блокировкой всех аппаратных и конфигурационных записей |
| `UCC_AUTOSTART_DIR` | `${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/autostart` | Каталог установки `.desktop`-файлов XDG-автозапуска |

### Сборка Read-Only Preview (`UCC_READ_ONLY_PREVIEW=ON`)

Read-Only Preview используется для безопасной проверки UI без изменения состояния оборудования и файлов конфигурации. Собирается в отдельный каталог `build-preview/`:

```bash
cmake -S . -B build-preview -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DUCC_READ_ONLY_PREVIEW=ON \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_CLI=OFF \
  -DBUILD_GNOME=OFF \
  -DBUILD_TESTS=ON
cmake --build build-preview --target ucc-gui --parallel 2
./build-preview/bin/ucc-gui
```

Для быстрого локального запуска предусмотрен скрипт `./run-preview.sh` (использует `CMAKE_BUILD_TYPE=Debug` и каталог `build-preview/`). Не устанавливайте preview-сборку в `/usr` поверх рабочего пакета.

---

## 4. Автоматизация через `cargo xtask`

В `.cargo/config.toml` задан алиас `xtask = "run --package xtask --"`. В офлайн-среде добавляйте `--locked --offline` через `cargo run -p xtask --locked --offline -- <команда>` или вызывайте собранный `target/release/xtask <команда>`.

| Команда | Назначение |
| --- | --- |
| `cargo xtask dist [dest [bin-dir]]` | Стадирует полный Rust-комплект в `dest` (по умолчанию `target/dist`): 7 бинарников, man-страницу, completions, systemd/D-Bus/Polkit-файлы, симлинк `uccd-pre-sleep.service -> uccd-sleep.service`, а также `share/doc/ucc/RELEASE_MANIFEST.json` (SHA-256 всех бинарников) и CycloneDX 1.5 `share/doc/ucc/SBOM.json` |
| `cargo xtask install-runtime <dest> <bin-dir>` | Устанавливает 7 Rust-бинарников в `<dest>/bin` и `<dest>/lib/ucc`, а также `ucc-cli.1` и shell completions (bash, zsh, fish). Используется в `PKGBUILD`, `debian/rules`, `ucc.spec`, `package.nix` |
| `cargo xtask check-versions` | Проверяет строгое совпадение SemVer во всех 6 манифестах: `Cargo.toml`, `CMakeLists.txt`, `PKGBUILD`, `ucc.spec`, `package.nix`, `debian/changelog` |
| `cargo xtask parser-check` | Проверяет паритет версий (6 манифестов), валидность и отсутствие дубликатов ключей в 11 служебных JSON-файлах, корректность CycloneDX SBOM и инварианты офлайн-импортёра в памяти |
| `cargo xtask release <X.Y.Z[-pre]> [--dry-run]` | Обновляет версию во всех 6 манифестах (со сбросом `pkgrel=1` и `Release: 1%{?dist}`), при наличии `git-cliff` обновляет `CHANGELOG.md`; с `--dry-run` только выводит список проверяемых файлов |
| `cargo xtask completions [out-dir]` | Генерирует completions `ucc-cli.bash`, `_ucc-cli`, `ucc-cli.fish` (по умолчанию в `target/completions`) |
| `cargo xtask manpages [out-dir]` | Генерирует man-страницу `ucc-cli.1` (по умолчанию в `target/man`) |
| `cargo xtask dist-cli` (алиас `assets`) | Запускает генерацию completions (`target/completions`) и man-страниц (`target/man`) |
| `cargo xtask build-fixtures` (алиас `fixtures`) | Компилирует C-фикстуры `LD_PRELOAD` (`stand-fixtures/`, `extra-fixtures/`) в `$UCC_FIXTURES_DIR` или `/tmp/ucc-fixtures-<pid>` |
| `cargo xtask import-offline ...` | Офлайн-конвертация пользовательского `Data/` и TCC-конфигурации без обращения к системным путям |
| `cargo xtask maintenance <prepare-deployment\|deploy\|desktop-transition>` | Подготовка пакета развёртывания, установка/откат (`--install`/`--rollback`) и переключение пользовательской сессии |
| `cargo xtask capture-gallery --build <dir> [--output <dir>]` | Снятие и проверка галереи скриншотов GUI на изолированной шине D-Bus |

---

## 5. Упаковка по дистрибутивам и ручная установка

### Arch Linux (`PKGBUILD`)

Пакет: `ucc-rusted` (`provides=("uniwill-control-center=$pkgver" "frixit-control-center=$pkgver")`, `conflicts=('uniwill-control-center' 'uniwill-control-center-mechrevo' 'frixit-control-center')`, `replaces=('frixit-control-center')`, хук `ucc.install`).

Сборка из локального checkout:

```bash
make arch
# или напрямую:
UCC_SOURCE_DIR="$PWD" makepkg -sf --noconfirm
sudo pacman -U ./dist/arch/ucc-rusted-*.pkg.tar.zst
```

Стадия `check()` в `PKGBUILD` автоматически выполняет `dbus-run-session -- cargo test --workspace --locked --offline`, собирает `interop_daemon`, прогоняет `ctest -j1 --output-on-failure` и `contrib/mechrevo/tests/test_migration.py`.

### Debian / Ubuntu (`debian/*`)

Сборка из офлайн-архива (`make dist` -> `debuild -uc -us`):

```bash
make deb
```

`debian/rules` использует `dh $@ -Scmake` с `DEB_BUILD_MAINT_OPTIONS=hardening=+all`, собирает Qt6 GUI (`-DCMAKE_BUILD_TYPE=RelWithDebInfo -DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF -DBUILD_CLI=OFF`), затем выполняет `cargo build --workspace --release --locked --offline --bins --target-dir target/deb` и `xtask install-runtime debian/tmp/usr target/deb/release`.

### Fedora / RPM (`ucc.spec`)

Сборка бинарного RPM и SRPM из вендорированного архива:

```bash
make rpm    # результат: dist/rpmbuild/RPMS/ и dist/rpmbuild/SRPMS/
make srpm   # только SRPM
```

`ucc.spec` включает `%global _hardened_build 1`, `Provides: ucc-rusted = %{version}-%{release}`, собирает Rust-бинарники в `target/rpm` и устанавливает их через `xtask install-runtime "%{buildroot}%{_prefix}" target/rpm/release`.

### Nix / NixOS (`flake.nix`, `package.nix`, `nix/nixos-module.nix`)

Локальная сборка и проверки flake:

```bash
nix build .#ucc
nix flake check --print-build-logs
nix develop   # devShell с cmake, pkg-config, deadnix, nixfmt, statix, treefmt
```

`package.nix` импортирует `Cargo.lock` через `rustPlatform.importCargoLock`, собирает Qt6 GUI с `-DUCC_AUTOSTART_DIR=etc/xdg/autostart`, оборачивает `ucc-gui` (`wrapQtApp`) и `uccd` (`wrapProgram` с `PATH`), а также патчит пути `/usr/bin/uccd`, `/usr/lib/ucc/ucc-hardware` и `/usr/lib/ucc/ucc-sleep` в systemd/D-Bus-файлах на `$out/...`.

Подключение NixOS-модуля в `flake.nix` системы:

```nix
{
  inputs.ucc.url = "github:Frixlt/UCC";

  outputs = { nixpkgs, ucc, ... }: {
    nixosConfigurations.laptop = nixpkgs.lib.nixosSystem {
      modules = [
        ucc.nixosModules.default
        {
          services.uccd = {
            enable = true;
            enableSleepHandler = true;
            extraArgs = [ ]; # например, [ "--verbose" ]
          };
        }
      ];
    };
  };
}
```

### Ручная установка (без пакетного менеджера)

Не смешивайте ручную установку с пакетным менеджером дистрибутива.

```bash
sudo cmake --install build
sudo target/release/xtask install-runtime /usr target/release
sudo install -d -m 0755 /etc/ucc
sudo systemctl daemon-reload
sudo systemctl reload dbus
sudo systemctl enable --now uccd.service
sudo systemctl enable uccd-sleep.service
```

---

## 6. Схема установленной файловой системы

| Путь | Компонент / назначение |
| --- | --- |
| `/usr/bin/uccd` | Системный демон `com.uniwill.uccd` (Rust) |
| `/usr/bin/ucc-gui` | Основное приложение GUI (Qt6 Widgets / Charts) |
| `/usr/bin/ucc-cli` | Утилита командной строки (Rust) |
| `/usr/bin/ucc-desktop-tray` | Иконка трея StatusNotifierItem (Rust) |
| `/usr/bin/ucc-session-agent` | Пользовательский агент уведомлений, Lightbar и частоты экрана (Rust) |
| `/usr/lib/ucc/ucc-hardware` | Root-утилита аудита датчиков (`--audit`) и аварийного возврата вентиляторов (`--fan-auto`) |
| `/usr/lib/ucc/ucc-sleep` | Root-утилита сохранения/восстановления состояния сна (`prepare` / `resume`) |
| `/usr/lib/ucc/ucc-import-check` | Офлайн-валидатор импортируемых конфигураций |
| `/usr/lib/systemd/system/uccd.service` | Основной systemd-юнит демона (`Type=dbus`, `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto`) |
| `/usr/lib/systemd/system/uccd-sleep.service` | Юнит сна (`RequiredBy=sleep.target`, `Before=sleep.target nvidia-suspend.service nvidia-hibernate.service`) |
| `/usr/lib/systemd/system/uccd-pre-sleep.service` | Симлинк-алиас на `uccd-sleep.service` |
| `/usr/share/dbus-1/system-services/com.uniwill.uccd.service` | D-Bus-активация системного сервиса |
| `/usr/share/dbus-1/system.d/com.uniwill.uccd.conf` | Политика доступа к системной шине D-Bus |
| `/usr/share/polkit-1/actions/com.uniwill.uccd.policy` | Политика авторизации Polkit для привилегированных операций |
| `/etc/xdg/autostart/ucc-desktop-tray.desktop` | XDG-автозапуск трея |
| `/etc/xdg/autostart/ucc-session-agent.desktop` | XDG-автозапуск пользовательского агента |
| `/etc/ucc/` | Каталог системной конфигурации (`/etc/ucc/settings`, опционально `/etc/ucc/aquaris.json`) |
| `/usr/share/applications/ucc-gui.desktop` | Ярлык приложения `ucc-gui` |
| `/usr/share/icons/hicolor/{scalable,16x16..128x128}/apps/ucc-{gui,tray}.{svg,png}` | Иконки GUI и трея |
| `/usr/share/man/man1/ucc-cli.1` | Man-страница CLI |
| `/usr/share/bash-completion/completions/ucc-cli` | Автодополнение Bash |
| `/usr/share/zsh/site-functions/_ucc-cli` | Автодополнение Zsh |
| `/usr/share/fish/vendor_completions.d/ucc-cli.fish` | Автодополнение Fish |
| `/usr/share/gnome-shell/extensions/ucc@uniwill.com/` | Расширение GNOME Shell (`metadata.json`, `extension.js`, `uccdClient.js`, `stylesheet.css`) |
| `/usr/share/doc/ucc/` | Документация, лицензии, `RELEASE_MANIFEST.json` и `SBOM.json` (при `xtask dist`) |
