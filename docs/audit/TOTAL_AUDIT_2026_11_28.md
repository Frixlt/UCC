# Тотальный 10-агентный аудит кодовой базы `UCC-Revorked` (2026-11-28)

Полный read-only аудит всех 10 подсистем репозитория с перекрёстной сверкой против спецификаций `docs/hardware/EC_01..05` и `UNIWILL_0X1A_REGISTERS.md`.

## Сводка по подсистемам

| # | Подсистема | P0 | P1 | P2 | Главные блокеры |
| :---: | --- | :---: | :---: | :---: | --- |
| 1 | `kernel/uwcc-rs/` | 2 | 6 | 3 | Опасные `0x0753`/`0x0762`/`0x07A4`/`0x07A5` в allowlist; нет атомарного RMW в ядре (`0x0727 PL4D`, `0x07C6 BIT(6)`, `0x07C5 BIT(4)`); `PWUP <= 3` вместо `<= 5`; `ec_version` читает `0x0770/0x0771` вместо `0x0454/0x047D` |
| 2 | `crates/ucc-hardware/src/uwcc/` + `detection.rs` | 7 | 8 | 4 | Сломанный `ramfan.rs` (`CTRL=0x0F00`/`COMMIT=0x0F5C`); нет `PL4` x2 (`0x0727 BIT(7)`); перепутаны `SMRW` `data[4]`/`data[7]`; `0x0743..0x0746` GPU power; `Fn Lock` (`0x07A4` вместо `0x074E`); `USB Sleep` (`0x0762` вместо `0x0767`); `detect_vendor` проверяет Clevo раньше Uniwill |
| 3 | `crates/ucc-hardware/src/` (non-uwcc, СЖО, CPU/sysfs) | 2 | 5 | 2 | Нет аварийной термозащиты СЖО в `Manual`; `CpuController`/`RealSysfs` обходят `SysfsRoot`/`O_NOFOLLOW`; TOCTOU в `SysfsRoot`; нет rollback по ядрам CPU; блокирующий I/O `FanController` на Tokio; нет таймаутов BlueZ |
| 4 | `crates/ucc-lighting/` + EC lighting | 5 | 6 | 5 | Захват `0xFF89` на `048D:600B`; сломанные протоколы `048D:7001`/`7000`; некорректный `0x0748`; конфликт `0x07C5 BIT(5)` с Whisper Mode; утечка потока `uccd-worker-lightbar` |
| 5 | `crates/uccd/src/` (ядро демона, `profile_tx.rs`) | 4 | 5 | 3 | `temp_profile_name` write-only; дыры отката `cTGP`/`charging`/`kb` в `profile_tx.rs`; `shutdown()` не сбрасывает `0x0741 BIT(0)`; ~50 D-Bus Read RPC блокируют Tokio |
| 6 | `crates/ucc-dbus/` + IPC/Polkit | 3 | 5 | 5 | Fail-open `ImpersonateNamedPipeClient` (`named_pipe.rs:728-733`); `GRGW` Pipe-Instance Hijacking; fail-open `_ => Read` (`ipc.rs:465`); обход `ManageHardware` на выключении помпы СЖО |
| 7 | `crates/ucc-core/`, `ucc-config/`, зависимости | 3 | 4 | 3 | TOCTOU symlink-гонка в `write_atomic`/`restore_from_backup`; сохранение `0o666`/`0o777`; нет валидации `nvidia_ctgp_offset` и схемы TCC/DB/Experimental; `NaN`/`Inf` в `TemperatureFilter`; нет `deny.toml`, `tokio/full` + `zbus/async-io` bloat |
| 8 | `ucc-desktop`, `ucc-session-agent`, `ucc-cli` | 2 | 7 | 3 | DBusMenu layout churn в `tray.rs` (причина глитча меню трея!); 3 флаки-теста Hyprland без `XDG_CURRENT_DESKTOP`; `exit(2)` + смерть D-Bus стримов в `ucc-session-agent`; OOM DoS по `zones` и сломанные exit-коды в `ucc-cli` |
| 9 | `ucc-gui/` + `libucc-dbus/` (Qt6 C++20) | 5 | 8 | 3 | UAF порядка разрушения `MainWindow` (`unique_ptr` + Qt parent); `nullptr` в `PowerDevicesTab`/`NotificationCenter`; `SIGABRT` в `KeyboardVisualizerWidget`; мёртвый `"GetGpuInfo"`; sync Polkit D-Bus на UI-потоке; утечка таймера СЖО |
| 10 | Сборка, упаковка, systemd, SPDX, CI | 5 | 6 | 3 | Невоспроизводимый `uwcc-dkms` PKGBUILD; `SKIP` checksum; слабый hardening `uccd-sleep.service` и `/run` в `uccd.service`; 220 файлов без SPDX; нет `deny.toml`/`cargo-audit`/`forbid(unsafe_code)` |

---

## Приоритетный план закрытия (по волнам)

### Волна A — P0 Безопасность, Стабильность и Термозащита (Этап 0 / `v1.0.0` blockers)
1. **IPC / Авторизация (`crates/ucc-dbus/`):**
   - Закрыть fail-open в `named_pipe.rs:726-748` (`ImpersonateNamedPipeClient` → fail-closed `Result`), убрать `GW` (`FILE_CREATE_PIPE_INSTANCE`) из SDDL `IU`.
   - Закрыть fail-open `_ => Read` в `ipc.rs:465` (перевести на `Option<IpcAccessLevel>`) и скрыть `dispatch_json_rpc`.
   - Перевести `TurnOffWaterCoolerPump`/`TurnOffWaterCoolerFan` на `AuthAction::ManageHardware`.
   - Ограничить `MAX_CUSTOM_FAN_PROFILES = 64` / `MAX_CUSTOM_KEYBOARD_PROFILES = 64` и валидировать JSON-схемы.
2. **Аварийная термозащита СЖО (`crates/ucc-hardware/src/cooler.rs`):**
   - Ввести аппаратный failsafe перегрева (`>= 90 °C` триггер / `< 80 °C` сброс) поверх `CoolerMode::Manual` с принудительными `FanSpeed(100)` + `PumpVoltage::V11`.
3. **Config / Persistence / Core (`crates/ucc-core/`, `crates/ucc-config/`):**
   - Перевести `write_atomic`/`restore_from_backup` на `openat`/`renameat` с `O_NOFOLLOW`, лимитом размера и безопасной маской прав (`0o644`/`0o600`).
   - Добавить защиту от `NaN`/`Inf` в `TemperatureFilter::new`/`filtered_value()`.
   - Добавить валидацию `nvidia_ctgp_offset` и типизированные поля схемы для `tcc_max_temp_c` (`80..=95 °C`), `nvidia_dynamic_boost_w` и `experimental` (включая `mode_button_0x00b0`).
   - Добавить `#![forbid(unsafe_code)]` в `ucc-core`, `ucc-config`, `ucc-cli`, `xtask`, а в `ucc-dbus`/`uccd` убрать Unix `unsafe` и поставить `#![cfg_attr(not(windows), forbid(unsafe_code))]`.
4. **Демон `uccd` (`profile_tx.rs`, `backend.rs`, `orchestrator.rs`):**
   - Починить `temp_profile_name` (учёт в 200ms reconcile и реальный сброс в `set_temp_profile("")`).
   - Свести откат транзакции `apply_profile_txn` в единый `TxUndo` (включая `cTGP`, `charging`, `kb`).
   - Явно вызывать `set_mode_enable(false)` (сброс `0x0741 BIT(0)`) и `set_fans_auto()` в `shutdown()` и `on_system_suspend()`; джойнить OS-потоки воркеров.
   - Убрать блокирующие D-Bus Read RPC с Tokio-потоков и обернуть `discover_coolers()` BlueZ в `tokio::time::timeout`.
5. **GUI (`ucc-gui/`, `libucc-dbus/`):**
   - Убрать двойное владение (`unique_ptr` + `this` parent) в `MainWindow`, `SystemMonitor`, `ProfileManager`.
   - Починить `nullptr`-проверки (`PowerDevicesTab:276`, `NotificationCenter:107`), `KeyboardVisualizerWidget` `QThread` interruption, `ThemeEvents` `QPointer`.
   - Удалить мёртвый `"GetGpuInfo"` и `"SetKeyboardBacklight"` из `UccdClient.cpp`; остановить вечный таймер СЖО на системах без СЖО.
6. **Трей и Session Agent (`ucc-desktop`, `ucc-session-agent`):**
   - Починить DBusMenu layout churn в `tray.rs` (`visible: !daemon_connected` вместо добавления/удаления узла + один атомарный `handle.update()`).
   - Починить 3 флаки-теста Hyprland (`detect_from_env()` по `HYPRLAND_INSTANCE_SIGNATURE`) и гонку `SIGTERM` в `ucc-session-agent`.
7. **Упаковка, CI, `deny.toml` и SPDX:**
   - Добавить `deny.toml`, убрать `tokio/full` и `zbus/async-io` bloat, усилить `uccd.service` и `uccd-sleep.service`, проставить SPDX-заголовки.

### Волна B — P0/P1 Аппаратные фиксы `uwcc-rs` и `ucc-hardware` (по `docs/hardware/EC_01..05`)
1. **Ядро `kernel/uwcc-rs/src/validation.rs` + `acpi_shim.c`:**
   - Удалить опасные `0x0753`, `0x0762`, `0x07A4`, `0x07A5` из write allowlist; добавить безопасные `0x0746`, `0x074E`, `0x0787`, `0x07D0`, `0x07E2..=0x07E5`.
   - Внедрить ядерный атомарный `uwcc_acpi_ec_rmw()` с битовыми масками для `0x0727` (запрет `BIT(7) PL4D`), `0x07C6` (запрет `BIT(6)` кнопки питания), `0x07C5` (запрет `BIT(4)`), `0x0726`, `0x07B9` (сброс `BIT(7)`).
   - Поднять `UWCC_PWUP_MAX_LEVEL` до `5` (или `7`); исправить чтение `ec_version` на `0x0454`/`0x047D`.
2. **Userspace `crates/ucc-hardware/src/uwcc/` + `detection.rs`:**
   - Переписать `ramfan.rs` на реальную 96-байтную раскладку `0x0F00..=0x0F5F` + `0x07C5`/`0x07C6`.
   - Подключить реальный **`TCC` (`0x0786` write / `0x0463` readback)**, удвоение **`PL4` (`0x0727 BIT(7)`)**, правильный декодер **`SMRW` (`data[7]` base TGP, `data[4]` cTGP offset)**, регистры GPU **`0x0743..=0x0746`** + live readback **`0x0788`/`0x07D5`**.
   - Исправить `Fn Lock` (`0x074E BIT(4)`), `USB Sleep Charging` (`0x0767 BIT(4)`), нижний порог заряда (`0x07D0`), маску `0x07A6` (`0x30`), tear-free тахометры, приоритет Uniwill перед Clevo в `detect_vendor()`, и коды WMI в `wmi_events.rs`.
   - Развести Intel `X6AR55xY` vs AMD `X6FR5xxY` по `DMI board_name` + `CPU vendor`.
3. **Подсветка и HID (`crates/ucc-lighting/`):**
   - Исключить `0xFF89` в `hid_transport.rs` (приоритет `0xFF03 > 0xFF12 > 0xFF02`), починить протоколы `048D:7001`/`7000`, добавить `048D:CE00`, исправить `0x0748` (`BIT(2)` `S0_OFF`) и конфликт `0x07C5 BIT(5)`.

### Волна C — Новые фичи (Этап 1 / `M16`)
1. **Слайдер максимальной температуры CPU (TCC `80..=95 °C`)** в редакторе профиля (`MainWindow.cpp` + `ucc-core` + `uccd` + `0x0786`/`0x0463`).
2. **Живые лимиты EC на Dashboard (`M16.05`):** `PL1/PL2/PL4`, действующий `TCC`, `cTGP`/`DB`, активный режим `PMSC` (`Office / Gaming / Turbo / Custom`) в карточках CPU/GPU.
3. **Экспериментальные настройки (`m_experimentalPage`):** переназначение физической кнопки режима `0x00B0` (с явным управлением `0x0741 BIT(0)` и безопасным возвратом EC при выключении/сне).
