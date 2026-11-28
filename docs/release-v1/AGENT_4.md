# Runbook Агента 4 — Зависимости, `deny.toml`, Упаковка, CMake, CI и Legacy Cleanup

- **Ветка:** `release/a4-build-deps-ci-release` (от `rust/migration`)
- **Эксклюзивная зона владения:** `Cargo.toml`, `deny.toml`, `.github/**`, `PKGBUILD`, `CMakeLists.txt`, `tests/CMakeLists.txt`, `Makefile`, `debian/**`, `ucc.spec`, `ucc.install`, `nix/**`, `crates/xtask/**`, удаление legacy C++ runtime (`M13.7–M13.8`)
- **Обязательное чтение перед стартом:** `docs/release-v1/CONTRACT.md`, `docs/release-v1/SYNC_PROTOCOL.md`, `docs/audit/TOTAL_AUDIT_2026_11_28.md`, `docs/rust/MIGRATION_PLAN.md`.

## Фаза 1 — 5 параллельных субагентов-исполнителей

### Субагент 4.1: `Cargo.toml` + `deny.toml` + `crates/xtask/`
- Создать строгий `deny.toml` в корне репозитория (`[advisories]`, `[licenses]` с GPL-3.0-совместимым allowlist, `[bans]` с контролем дублей, `[sources]`).
- В корневом `Cargo.toml`:
  - Заменить `tokio = { version = "1.43", features = ["full"] }` на минимально нужные фичи (`rt-multi-thread`, `macros`, `sync`, `time`, `signal`, `process`, `io-util`, `net`, `fs`).
  - Переключить `zbus` на `default-features = false, features = ["tokio"]`, чтобы убрать дублирующий async-рантайм (`async-io`, `async-executor`, `blocking`, `polling`).
  - Централизовать `libc = "0.2"` и `tempfile = "3"` в `[workspace.dependencies]`.
- В `crates/xtask/`: включить `#![forbid(unsafe_code)]` в `src/lib.rs`, удалить неиспользуемый `thiserror` из `Cargo.toml`, автоматизировать обновление SHA-256 в `PKGBUILD` в `xtask release`.

### Субагент 4.2: Упаковка (`PKGBUILD`, `debian/`, `ucc.spec`, `ucc.install`, `nix/`, `Makefile`, user systemd units)
- `PKGBUILD`: убрать лишнюю сборку legacy C++ бинарников и `nlohmann-json` из `makedepends`, заменить `SKIP` в `sha256sums` на релизный механизм (с поддержкой локального `UCC_SOURCE_DIR`).
- `debian/control` & `ucc.spec`: заменить жёсткую зависимость `tuxedo-drivers` на `Recommends: uwcc-dkms | tuxedo-drivers` (DEB) и `Recommends: (uwcc-dkms or tuxedo-drivers)` (RPM); выровнять `%autosetup` с `Source0` в `ucc.spec`.
- `nix/nixos-module.nix`: добавить `-/sys/firmware/acpi/platform_profile` в `ReadWritePaths` и `RuntimeDirectory = "ucc"`.
- `ucc.install` & `Makefile`: убрать дублирующие вызовы `gtk-update-icon-cache`/`update-desktop-database`, выровнять `VERSION` в `Makefile`.
- Добавить пользовательские systemd-юниты `ucc-desktop-tray.service` и `ucc-session-agent.service` (`PartOf=graphical-session.target`, `Restart=on-failure`) и согласовать их с `crates/xtask/src/maintenance.rs`.

### Субагент 4.3: CMake (`CMakeLists.txt`, `tests/CMakeLists.txt`)
- Исправить битые include-пути `${CMAKE_SOURCE_DIR}/../libucc-dbus` -> `${CMAKE_SOURCE_DIR}/libucc-dbus` и убрать захардкоженный `/usr/include`.
- Вынести `-Werror` под CMake-опцию `UCC_WERROR` (по умолчанию `OFF`, `ON` в CI).
- В `tests/CMakeLists.txt`: переключить `test_import_check` на Rust-бинарник, удалить дублирующие `find_program(PYTHON3_EXECUTABLE)` и `find_program(NODE_EXECUTABLE)`.
- *(Важно: не трогай файлы внутри `ucc-gui/` — их держит Агент 3; если нужно убрать `LCTWaterCoolerController` из `ucc-gui/CMakeLists.txt`, оставь заметку `[NEEDS: Agent3]` в `sync/agent4.md`!)*

### Субагент 4.4: CI (`.github/workflows/**`)
- Добавить джобу `security-audit` (`cargo audit` + `cargo deny check`).
- Убрать `|| true` в `shellcheck` и расширить проверку на `tests/*.sh`, `run-preview.sh`, `ucc.install`, `PKGBUILD`, `kernel/uwcc-rs/packaging/arch/uwcc-dkms.install`.
- Добавить `kernel/uwcc-rs/src/uwcc_rs.rs` в `rustfmt --check`, а `uwcc-dkms.install` и `modules-load/uwcc_rs.conf` — в `uwcc-kernel-uapi`.
- Добавить проверку `SPDX-License-Identifier` в `docs-hygiene` (как warning до финального прохода SPDX, затем error).

### Субагент 4.5: Wave 1 / `M13.7–M13.8` Legacy C++ Cleanup
- Удалить дубликат `uccd/uccd-pre-sleep.service` (оставить генерацию симлинка при установке).
- Удалить устаревшие C++ runtime-директории (`uccd/src/*`, `uccd/inc/*`, `ucc-cli/*`, `ucc-desktop/*`, `ucc-session/*`, `ucc-tray/*`) только после проверки, что `CMakeLists.txt` и `PKGBUILD` полностью отвязаны от них.
- Перенести сиротский `tests/test_ite8291_brightness.py` в `contrib/frixit/drivers/`.
- Очистить untracked-мусор (`rust-dbus-*.xml`, `tests/__pycache__`) и обновить `.gitignore`.

## Фаза 2 — 5 независимых adversarial-ревьюеров + Финальный Gate
Запусти 5 независимых ревьюеров (при необходимости запросив у пользователя «продолжай» для сброса лимита субагентов), исправь все замечания и прогони:
```bash
export CARGO_TARGET_DIR="$HOME/ucc-target-slim" TMPDIR="$HOME/ucc-tmp" RUSTFLAGS="-D warnings -C debuginfo=0"
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -j 2 -p xtask
cmake -B build-a4 -DBUILD_TESTS=ON && cmake --build build-a4 -j 2 && ctest --test-dir build-a4 --output-on-failure
```
Запушь в `release/a4-build-deps-ci-release` и обнови `sync/agent4.md` до `READY_FOR_MERGE`.

> **Важно:** массовую простановку `SPDX-License-Identifier` в 218 файлов userspace делаем **после** слияния веток `a1 + a2 + a3 + a4` в `release/v1.0.0-rc1`, чтобы не создавать merge-конфликты с Агентами 1–3.
