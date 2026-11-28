# UCC-Rusted — Единый план миграции (Single Source of Truth)

> **Статус:** Актуальный единый план миграции (`M0`–`M15`) для ветки `rust/migration`.
> Все устаревшие дубликаты планов, оркестрационные промпты и исторические срезы консолидированы в этот документ.
> Технические спецификации смежных треков: [`docs/UWCC_DRIVER.md`](../UWCC_DRIVER.md) (`M14`), [`docs/rust/WINDOWS_PORT.md`](WINDOWS_PORT.md) (`M15`), runbook перехода и отката: [`docs/MIGRATION_AND_RECOVERY.md`](../MIGRATION_AND_RECOVERY.md), архитектурные решения: [`docs/rust/execution/decisions/ADR-01..06.md`](execution/decisions/).

---

## 1. Целевая архитектура

- **100% Rust в фоне (0 МБ Qt/KF6/QML/Bluetooth в фоновых процессах):**
  - `uccd` — системный демон (Tokio actors, D-Bus `com.uniwill.uccd` + `de.uniwill.uccd`, Named Pipe JSON-RPC на Windows, Polkit/SDDL, телеметрия, профили, охлаждение, BLE LCT22002, подсветка).
  - `ucc-cli` — кросс-платформенная CLI-утилита и диагностика.
  - `ucc-desktop-tray` — лёгкий SNI-трей на Rust (без Qt/KDE).
  - `ucc-session-agent` — пользовательский агент сессии (уведомления, Wayland/Hyprland/KDE refresh, `LidDisplayHandler` DPMS).
  - `ucc-sleep`, `ucc-hardware`, `ucc-import-check` — системные помощники.
  - `kernel/uwcc-rs/` — собственный чистый Rust-модуль ядра Linux (`GPL-2.0-only`, ADR-05) для `INOU0000` (`ECRR`/`ECRW`/`SMRW`/`PWUP`/`PWBT`/`PWDB`), Parade `PS8461E` MUX (`MXDS`/`MXDM`), WMI-событий и `hwmon`.
- **Автономный клиент GUI:** `ucc-gui` на чистом `Qt6::Widgets` (`ADR-01`, без `KF6`, `QML`, `Qt6Bluetooth`, `libucc-dbus.so`), запускается по требованию; плюс расширение `ucc-gnome`.

---

## 2. Сводная матрица этапов (`M0`–`M15`)

| Этап | Область | Статус | Краткий итог |
| :--- | :--- | :--- | :--- |
| **M0** | Воспроизводимая база и инвентаризация | **Done** | Baseline, аудит контрактов, фикстуры и схемы готовы. |
| **M1** | Архитектурные решения (ADR-01..06) | **Done** | Все 6 ADR приняты; `M1.2–M1.4` отменены `ADR-01`. |
| **M2** | Доменная модель (`ucc-core`, `ucc-config`) | **Done** | Типы, валидация, атомарная персистентность, `platform_paths`, оффлайн-импорт. |
| **M3** | D-Bus API (123 метода, 7 сигналов) и `ucc-cli` | **Done** | Двойной алиас `com/de.uniwill.uccd`, Polkit, offload-линии, `ucc-cli`. |
| **M4** | Аппаратный слой (`ucc-hardware`, `ucc-lighting`) | **Done** | `PlatformBackend`, CPU/RAPL, устройства, охлаждение, BLE LCT22002, 126-зонная RGB-клавиатура, Lightbar. |
| **M5–M6** | Интегрированный `uccd` + `ucc-sleep` + `ucc-hardware` + `LidController` | **Done** | Полный цикл жизни, транзакции профилей, sleep/resume, wakeup policy. |
| **M7** | Телеметрия CPU/GPU и 10-серийная история | **Almost Done** | Код и кодек готовы (`M7.1–M7.4, M7.6–M7.7`); осталась проверка `D3cold` на чистом простое (`M7.5`). |
| **M8** | Расширенная валидация, fuzz/proptest/stress | **Done** | Property/fuzz/stress-сьют интегрирован и проходит в CI. |
| **M9** | Desktop & Session (`ucc-desktop-tray`, `ucc-session-agent`) | **Almost Done** | SNI-трей, Wayland/KDE/Hyprland refresh, DPMS, `pidfd`, `ActivateWithToken` готовы; осталась проверка рестарта композитора/мультисессии (`M9.8`). `M9.5` (X11) — **Deferred**. |
| **M10** | Автономный `Qt6::Widgets` `ucc-gui` | **Almost Done** | Отвязан от `KF6`/`QML`/`Bluetooth`/`libucc-dbus.so`, `crates/ucc-gui` удалён, вкладка `Experimental` и `test_gui_audit` готовы; остались живая проверка клика из трея (`M10.7`) и дизайн-согласование иконок/`modules.d` (`M10.10`). |
| **M11** | Утилиты и `ucc-gnome` | **Almost Done** | Rust `import-offline`, `xtask maintenance`, `parser-check`, чистая граница `ucc-gnome` готовы; осталась живая приёмка quick controls (`M11.5`). |
| **M12** | Упаковка и CI/CD | **Almost Done** | `xtask dist`, Arch `PKGBUILD`, Linux/Windows/Nix/Release CI и SBOM готовы; остались `post_upgrade`/clean-install/downgrade drill (`M12.4`, `M12.9`) и автогенерация CLI assets (`BUILD-02`). `M12.3` (DEB/RPM/Nix) — **Deferred**. |
| **M13** | Финальный cutover, документация и удаление legacy C++ | **In Progress** | `M13.10` (ребрендинг `ucc-rusted`) и `M13.11` (строгая документация EN/RU/AI без `README.en.md`) закрыты; остались живая приёмка (`M13.1–M13.6`), удаление legacy C++ runtime из `CMakeLists.txt` (`M13.7–M13.8`) и релизный тег (`M13.9`). |
| **M14** | Собственный Rust-драйвер ядра `uwcc-rs` (ADR-05) | **Code Done, HW Pending** | `kernel/uwcc-rs/` (собирается на Linux 7.3.6 + `rustc 1.100.0` + `LLVM=1`), `ECRR`/`ECRW`/`SMRW`/`PWUP`/`PWBT`/`PWDB`, MUX `PS8461E`, `UwccBackend`, `RamFan 1.5`, USB webcam, `HidrawDeviceTransport`, DKMS/Arch/udev/modprobe и CI UAPI/`no_std` готовы (`M14.1–M14.7`); остались `uwcc-dkms` install/rollback (`M14.8`) и полная приёмка на `MECHREVO X6AR55xY` (`M14.9`). |
| **M15** | Порт на Windows 11 (ADR-06) | **In Progress** | Фундамент `M15.1–M15.4` + CI `windows-check.yml` в коде (`platform_paths`, Named Pipe JSON-RPC, SCM service, `WindowsWmiEcBackend`, `nvml.dll`/Power/Display/Lid, WinRT BLE, `WindowsHidTransport`); остались Unix-гейтинг `nix` в `ucc-hardware`/`uccd`, Windows-трей (`Shell_NotifyIconW`), Named Pipe в `ucc-gui`, MSI/WiX/winget (`M15.6`) и тест на живой Windows 11. |

---

## 3. Осознанные отличия от legacy C++ baseline

| ID | Отличие | Поведение в `ucc-rusted` |
| :--- | :--- | :--- |
| `DIFF-01` | Дубликаты ключей `keyboardBacklightStates` | Сериализация только уникальных валидированных ключей. |
| `DIFF-02` | Отсутствовавший D-Bus метод `SetWebcam` | Добавлен типизированный `SetWebcam(b) -> b` (всего 123 метода). |
| `DIFF-03` | Несуществующий метод в `ucc-gnome` | Вызывается `GetKeyboardBacklightControlSupported`. |
| `DIFF-04` | Ложные `true` при ошибках записи железа | Строгое возвращение `false` / `Err` при отказе записи. |
| `DIFF-05` | Крах парсера YCbCr/YUV420 на пустом списке | Безопасная обработка пустого списка режимов дисплея. |
| `DIFF-06` | Схлопывание ошибок транспорта/Polkit в `false` | Типизированный `Result<bool, zbus::Error>` в Rust-клиентах и CLI. |
| `DIFF-07` | Жёсткий 1500 мс таймаут `AsyncRead` | Фоновый Tokio-опрос с lockless-снимками (`ADR-02`). |
| `DIFF-08` | Возврат `false` вместо D-Bus ошибки | Типизированные `zbus::fdo::Error` на невалидный JSON и отказ Polkit. |
| `DIFF-09` | Раздельные namespace `com/de.uniwill.uccd` | Одновременная регистрация обоих путей и имён на шине. |
| `DIFF-10` | Путаница ступеней помпы `0..4` и кода `1` (`12V`) | Нормализованные ступени `0..=4` в профилях; `12V` жёстко запрещён в энкодере. |

---

## 4. Единый чек-лист: выполнено vs осталось

### Выполнено (закрыто в коде и тестах)
- [x] `M0`–`M6`, `M8`: базовые контракты, `ADR-01..06`, `ucc-core`, `ucc-config`, `ucc-dbus` (123 метода + 7 сигналов), `ucc-cli`, `ucc-hardware`, `ucc-lighting`, `uccd`, `ucc-sleep`, `ucc-hardware`, `ucc-import-check`, `LidController`, `WakeupPolicy`, fuzz/proptest/stress-сьют.
- [x] `M7.1–M7.4, M7.6–M7.7`: телеметрия CPU/RAPL, i915/xe/AMD/NVML, 10-серийная история и бинарный кодек.
- [x] `M9.1–M9.4, M9.6–M9.7, M9.9`: Rust `ucc-desktop-tray` (SNI), `ucc-session-agent` (уведомления, Hyprland/KDE refresh, DPMS), `pidfd`, `ActivateWithToken`.
- [x] `M10.1–M10.6, M10.8–M10.9`: автономный `Qt6::Widgets` `ucc-gui` без `KF6`/`QML`/`Bluetooth`/`libucc-dbus.so`, удаление `crates/ucc-gui`, вкладка `Experimental`, `test_gui_audit`.
- [x] `M11.1–M11.4, M11.6, M11.8` (`M11.7` заменено `ADR-03`): Rust `import-offline`, `xtask maintenance`, `parser-check`, чистая граница `ucc-gnome`.
- [x] `M12.1–M12.2, M12.5–M12.8, M12.10`: `xtask dist`, Arch `PKGBUILD` (`ucc-rusted`), Linux/Windows/Nix/Release CI и SBOM.
- [x] `M13.10`, `M13.11`: ребрендинг `ucc-rusted`, актуализация документации EN/RU/AI, удаление устаревшего `README.en.md` и мусорных планов.
- [x] `M14.1–M14.7`: `kernel/uwcc-rs` (`uwcc_rs.rs`, `validation.rs`, `ring_buffer.rs`, `acpi_shim.c` с `ECRR`/`ECRW`/`SMRW`/`PWUP`/`PWBT`/`PWDB` и MUX `PS8461E`), `UwccBackend`, `RamFan 1.5`, USB webcam, `HidrawDeviceTransport`, выбор бэкенда в `uccd`, DKMS/Arch/modprobe/udev, CI UAPI/`no_std` + C++20 проверки.
- [x] `M15.1–M15.4` + CI `M15.5`: `platform_paths`, Named Pipe JSON-RPC, SCM service, `WindowsWmiEcBackend`, `nvml.dll`/Power/Display/Lid, WinRT BLE, `WindowsHidTransport`, `windows-check.yml`.

### Осталось выполнить

#### A. Расширенное тестовое покрытие и полировка (Linux + Windows)
- [ ] Закрыть все оставшиеся пункты матрицы тестов (Волна 2) для `ucc-core`, `ucc-config`, `ucc-hardware` (Linux + Windows), `ucc-lighting`, `ucc-dbus`, `uccd`, `ucc-cli`, `ucc-session-agent`, `xtask`, `kernel/uwcc-rs`, `libucc-dbus`, `ucc-gui`, `ucc-gnome`.
- [ ] Закрыть открытую минорную находку `FINDING-V2-06` (обезличивание личных ID профилей в тестовой фикстуре `settings.json`).

#### B. Аппаратная и системная приёмка Linux (`M7.5`, `M9.8`, `M10.7`, `M11.5`, `M12.4`, `M12.9`, `M13.1–M13.6`)
- [ ] Подтвердить `D3cold` NVIDIA RTX 5080 на простое без нагрузки композитора/`mpvpaper` (`M7.5`) и интерактивный клик из трея (`M10.7`).
- [ ] Проверить падение/перезапуск композитора и мультисессии (`M9.8`), а также живую работу quick controls и уведомлений (`M11.5`).
- [ ] Выполнить чистую установку, одноимённый `post_upgrade`, reboot/autostart и downgrade/rollback drill (`M12.4`, `M12.9`, `M13.1`, `M13.5`).
- [ ] Пройти аппаратную матрицу: все 10 страниц `ucc-gui`, реальный suspend/resume, переключения AC/BAT и 24–72 ч soak-тест (`M13.2–M13.4`, `M13.6`).

#### C. Финализация Linux-ветки и удаление legacy C++ (`M13.7–M13.9`, `M10.10`, `BUILD-02`)
- [ ] Удалить legacy C++ runtime (`uccd`, `ucc-cli`, `ucc-session`, `ucc-desktop`, `ucc-tray`) из `CMakeLists.txt` и активной поставки (`M13.7–M13.8`).
- [ ] Встроить автогенерацию CLI-ассетов (manpages/completions) в build-пайплайн (`BUILD-02`).
- [ ] Согласовать и реализовать единый дизайн/динамические иконки трея/`modules.d` (`M10.10`).
- [ ] Обновить `CHANGELOG.md` и выпустить проверенный релизный тег (`M13.9`).

#### D. Живая приёмка `uwcc-rs` (`M14.8–M14.9`)
- [ ] Собрать и установить `uwcc-dkms` на живом ядре, проверить blacklist `tuxedo-drivers` и чистый откат (`M14.8`).
- [ ] Провести полную аппаратную приёмку `uwcc-rs` + `UwccBackend` + `HidrawDeviceTransport` на `MECHREVO X6AR55xY` (`M14.9`).

#### E. Завершение Windows-порта (`M15`)
- [ ] Вынести `nix` под `[target.'cfg(unix)'.dependencies]` в `crates/ucc-hardware/Cargo.toml` и `crates/uccd/Cargo.toml`, заменить негейтированный `nix::Error` и расширить `windows-check.yml` на `ucc-hardware`, `ucc-lighting`, `ucc-cli`, `uccd`.
- [ ] Реализовать нативный Windows-трей (`Shell_NotifyIconW` в `crates/ucc-desktop`) и Named Pipe клиент в `ucc-gui` (`M15.5`).
- [ ] Завершить живую реализацию `LiveWinRtGattBridge` и `WmiBrightnessBridge`, выровнять карту EC-регистров `windows_wmi` ↔ `uwcc`.
- [ ] Собрать и проверить инсталлятор MSI/WiX/Inno + `winget` (`M15.6`) и провести живое тестирование на Windows 11.

#### Отложено на сообщество (Deferred)
- `M9.5` — X11 session refresh adapter.
- `M12.3` — нативные пакетные сборки DEB/RPM/Nix.

---

## 5. Критерии готовности и откат

Релиз считается готовым после закрытия разделов **A–D** для Linux (и **E** для Windows-релизов) при зелёном CI (`.github/workflows/build.yml`, `windows-check.yml`, `nix.yml`, `release.yml`).
Процедура аварийного отката на `tuxedo-drivers` и резервные копии конфигов описана в [`docs/MIGRATION_AND_RECOVERY.md`](../MIGRATION_AND_RECOVERY.md).
