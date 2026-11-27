# CLI Reference (`ucc-cli` и сопутствующие утилиты)

Технический справочник командной строки `ucc-cli` (`crates/ucc-cli`) и вспомогательных бинарных файлов `ucc-rusted`.

## 1. Общие сведения о `ucc-cli`

```text
ucc-cli [GLOBAL_FLAGS] <COMMAND> [ARGS...]
```

Запуск без аргументов выводит справку (`arg_required_else_help = true`) и завершается с кодом `1`.

### Глобальные флаги

| Флаг | Короткий | Алиас | Описание |
|---|---|---|---|
| `--json` | `-j` | — | Вывод структурированного JSON вместо текстовых таблиц (действует глобально до и после подкоманды). |
| `--version` | `-v` | `-V` | Вывод версии `ucc-cli` и завершение с кодом `0`. |
| `--help` | `-h` | — | Вывод справки по CLI или подкоманде и завершение с кодом `0`. |

### Коды возврата (Exit Codes)

| Код | Значение |
|---|---|
| `0` | Успешное выполнение команды (включая `--help`, `--version`, `completions`, `man`, `version`). |
| `1` | Ошибка разбора аргументов, невалидные параметры, отклонение операции демоном (`FAILED`) или ошибка RPC-вызова. |
| `2` | Невозможность подключиться к шине D-Bus либо сервис `com.uniwill.uccd` (или указанный в `UCCD_DBUS_SERVICE`) не имеет владельца на шине. |

### Переменные окружения и RPC-таймаут

| Переменная | Назначение |
|---|---|
| `UCCD_DBUS_SERVICE` | Переопределяет D-Bus имя сервиса (по умолчанию `com.uniwill.uccd`). Установка переменной автоматически переключает подключение на `session` bus. |
| `UCCD_DBUS_SESSION` | При наличии (любое значение) переключает подключение на `session` bus вместо `system` bus. |

- **Таймаут D-Bus RPC**: каждый вызов обёрнут в `DEFAULT_RPC_TIMEOUT = 1500 мс` (`with_rpc_timeout`). При превышении лимита возвращается ошибка `RPC call deadline exceeded`.
- Команды `completions`, `man` и `version` выполняются локально и не требуют запущенного `uccd`.

---

## 2. Подкоманды `ucc-cli`

### 2.1. `status`

Выводит сводку состояния системы: модель устройства, состояние питания, активный профиль, метрики CPU, dGPU, iGPU, яркость, веб-камеру, Fn-Lock, СЖО и зарядку АКБ.

```bash
ucc-cli status
ucc-cli status --json
```

Схема вывода `--json`:

```json
{
  "connected": true,
  "powerState": "power_ac",
  "activeProfile": { "id": "...", "name": "..." },
  "powerPolicy": { "real_power_source": "ac", "current_mode": "ac", "manual_mode": "auto", "profile_state": "power_ac", "water_cooler_connected": false, "registered_apps": [], "revision": 1 },
  "cpu": { "temperature": 55, "frequency": 3200, "power": 24.5, "fanPercent": 35, "fanRPM": 2400 },
  "gpu": { "temperature": 48, "frequency": 1350, "power": 18.2, "fanPercent": 30, "fanRPM": 2100 },
  "igpu": { "temperature": 45, "frequency": 800, "power": 5.1 },
  "hardware": { "displayBrightness": 80, "webcamEnabled": true, "fnLock": false },
  "waterCooler": { "connected": false, "supported": true, "enabled": false, "fanSpeed": 0, "pumpLevel": 4 },
  "charging": { "profile": "balanced", "priority": "battery", "type": "Standard", "startThreshold": 40, "endThreshold": 80 }
}
```

### 2.2. `monitor` (алиас: `mon`)

Периодически выводит однострочную сводку телеметрии CPU и dGPU.

| Флаг | Короткий | По умолчанию | Описание |
|---|---|---|---|
| `--count <N>` | `-n` | `0` | Количество итераций (`0` — бесконечно; значения `< 0` отклоняются с кодом `1`). |
| `--interval <SEC>` | `-i` | `2` | Интервал опроса в секундах (минимум `1`). |

```bash
ucc-cli monitor
ucc-cli mon -n 10 -i 1
ucc-cli monitor -n 5 --json
```

Схема каждой строки (NDJSON) при `--json`:

```json
{"cpu":{"fanPercent":35,"frequency":3200,"power":24.5,"temperature":55},"gpu":{"fanPercent":30,"frequency":1350,"power":18.2,"temperature":48}}
```

### 2.3. `profile` (алиас: `prof`)

Управление встроенными и пользовательскими профилями производительности. Без подкоманды выполняется `profile list`.

| Подкоманда | Алиасы | Аргументы | D-Bus метод | Описание |
|---|---|---|---|---|
| `list` | `ls` | — | `GetDefaultProfilesJSON`, `GetCustomProfilesJSON`, `GetProfilesJSON`, `GetActiveProfileJSON` | Список всех профилей с тегами привязок `[AC, BAT, ...]` и активным профилем. |
| `get` | `show`, `active` | `[ID]` | `GetActiveProfileJSON` / списки профилей | Детали активного профиля (если `ID` опущен) или профиля с указанным `ID`. |
| `set` | `activate` | `<ID>` | `SetActiveProfile` | Активация профиля по `ID`. |
| `defaults` | `default` | — | `GetDefaultProfilesJSON` | Список встроенных профилей. |
| `customs` | `custom` | — | `GetCustomProfilesJSON` | Список пользовательских профилей. |
| `apply` | — | `<JSON>` | `ApplyProfile` | Применение профиля напрямую из JSON-строки. |
| `save` | `create` | `<JSON>` | `SaveCustomProfile` | Сохранение пользовательского профиля из JSON-строки. |
| `delete` | `del`, `rm` | `<ID>` | `DeleteCustomProfile` | Удаление пользовательского профиля по `ID`. |

Схема `--json` для `profile list`: `{"builtin": [...], "custom": [...], "active": {...}}`. Для `get` — объект `UccProfile`; для `defaults`/`customs` — JSON-массив `UccProfile[]`.

```bash
ucc-cli profile list --json
ucc-cli prof get
ucc-cli profile set __balanced__
ucc-cli profile delete custom-gaming
```

### 2.4. `statemap` (алиас: `state-map`)

Просмотр настроек и привязка профилей к состояниям питания (`power_ac`, `power_bat`, `power_type_c`, `power_wc`). Без подкоманды выполняется `statemap get`.

| Подкоманда | Алиасы | Аргументы | D-Bus метод | Описание |
|---|---|---|---|---|
| `get` | `show` | — | `GetSettingsJSON` | Вывод таблицы `stateMap` и флагов подсистем. При `--json` — полный объект `GetSettingsJSON`. |
| `set` | — | `<STATE> <PROFILE_ID>` | `SetStateMap` | Привязка состояния питания к `PROFILE_ID`. |

```bash
ucc-cli statemap get --json
ucc-cli state-map set power_bat __quiet__
```

### 2.5. `fan`

Управление профилями и кривыми вентиляторов. Без подкоманды выполняется `fan list`.

| Подкоманда | Алиасы | Аргументы | D-Bus метод | Описание |
|---|---|---|---|---|
| `list` | `ls` | — | `GetFanProfileNames`, `GetCustomFanProfilesJSON` | Список встроенных и пользовательских профилей вентиляторов. При `--json`: `{"builtin": [...], "custom": [...]}`. |
| `get` | `show` | `<ID>` | `GetFanProfile` / `GetCustomFanProfilesJSON` | Таблицы кривых (`tableCPU`, `tableGPU`, `tablePump`, `tableWaterCoolerFan`). При `--json` — объект кривых. |
| `set` | `activate` | `<ID>` | `ApplyFanProfiles` | Загружает профиль по `ID`, преобразует ключи `tableCPU->cpu`, `tableGPU->gpu`, `tablePump->pump`, `tableWaterCoolerFan->waterCoolerFan` и применяет. |
| `apply` | — | `<JSON>` | `ApplyFanProfiles` | Прямое применение JSON с таблицами кривых. |
| `revert` | — | — | `RevertFanProfiles` | Возврат кривых вентиляторов к значениям по умолчанию активного профиля. |

```bash
ucc-cli fan list
ucc-cli fan get Balanced --json
ucc-cli fan set Balanced
ucc-cli fan revert
```

### 2.6. `cooler` (алиасы: `watercooler`, `wc`)

Управление внешней системой жидкостного охлаждения (LCT). Без подкоманды выполняется `cooler status`.

| Подкоманда | Алиасы | Аргументы | D-Bus метод | Описание |
|---|---|---|---|---|
| `status` | — | — | `GetWaterCoolerSupported`, `GetWaterCoolerConnected`, `IsWaterCoolerEnabled`, `GetWaterCoolerFanSpeed`, `GetWaterCoolerPumpLevel` | Статус СЖО. При `--json`: `{"supported", "connected", "enabled", "fanSpeed", "pumpLevel"}`. |
| `enable` | `on` | — | `EnableWaterCooler(true)` | Включение подключения СЖО. |
| `disable` | `off` | — | `EnableWaterCooler(false)` | Отключение СЖО. |
| `fan` | — | `<PERCENT>` (`0..=100`) | `SetWaterCoolerFanSpeed` | Скорость вентилятора СЖО в %. |
| `pump` | — | `<CODE>` | `SetWaterCoolerPumpVoltage` | Код напряжения помпы: `0`=11V High, `2`=7V Low, `3`=8V Med, `4`=Off. Код `1` (12V) и прочие отклоняются локально с кодом `1`. |
| `led` | — | `<R> <G> <B> <MODE>` | `SetWaterCoolerLEDColor` | Цвет RGB (`0..=255`) и режим (`0`=static, `1`=breathing и др.). |
| `led-off` | `led_off` | — | `TurnOffWaterCoolerLED` | Выключение LED СЖО. |

```bash
ucc-cli wc status --json
ucc-cli cooler enable
ucc-cli cooler fan 60
ucc-cli cooler pump 3
ucc-cli cooler led 0 128 255 0
ucc-cli cooler led-off
```

### 2.7. `cpu`

Информация о CPU, доступных governors, EPP и пределах частот. Без подкоманды выполняется `cpu status`.

| Подкоманда | Аргументы | D-Bus метод | Описание |
|---|---|---|---|
| `status` | — | `GetCpuCoreCount`, `GetFanDataCPU`, `GetCpuFrequencyMHz`, `GetCpuPowerValuesJSON`, `GetAvailableGovernors`, `GetAvailableEPPs`, `GetCpuFrequencyLimitsJSON` | Полная сводка CPU. При `--json`: `{"coreCount", "temperature", "frequency", "power", "availableGovernors", "availableEPPs", "minFrequencyMHz", "maxFrequencyMHz"}`. |
| `governor` | `[NAME]` | `GetAvailableGovernors` | Без аргумента — выводит список доступных governors (JSON-массив при `--json`). С аргументом возвращает код `1` (настройка выполняется через профиль). |
| `epp` | `[PREFERENCE]` | `GetAvailableEPPs` | Без аргумента — выводит список доступных EPP (JSON-массив при `--json`). С аргументом возвращает код `1` (настройка выполняется через профиль). |
| `frequency` | `<MIN> <MAX>` | — | Возвращает код `1`: прямая установка частот через D-Bus не поддерживается, выполняется через профиль. |

```bash
ucc-cli cpu status --json
ucc-cli cpu governor
ucc-cli cpu epp --json
```

### 2.8. `gpu`

Вывод телеметрии дискретного GPU, контроллера NVIDIA Power Control (cTGP) и встроенного GPU.

```bash
ucc-cli gpu
ucc-cli gpu --json
```

Схема `--json`:

```json
{
  "dgpu": { "temperature": 48, "frequency": 1350, "power": 18.2, "fanPercent": 30, "fanRPM": 2100 },
  "nvidiaPowerControl": { "available": true, "maxPowerLimit": 140, "defaultPowerLimit": 115, "ctgpOffset": 15 },
  "igpu": { "temperature": 45, "frequency": 800, "power": 5.1 }
}
```

### 2.9. `power`

Чтение политики питания и переключение ручного режима питания.

| Подкоманда | Аргументы | D-Bus метод | Описание |
|---|---|---|---|
| `status` | — | `GetPowerPolicyJSON` | Вывод JSON политики питания (при `--json` — с форматированием). |
| `mode` | `<MODE>` | `SetManualPowerMode` | Установка ручного режима (`auto`, `ac`, `type_c`, `bat`). |

```bash
ucc-cli power status --json
ucc-cli power mode ac
ucc-cli power mode auto
```

### 2.10. `power-limits` (алиас: `odm`)

Вывод ODM-лимитов мощности CPU (PL1 Sustained, PL2 Boost, PL4 Peak) через `ODMPowerLimitsJSON`.

```bash
ucc-cli power-limits
ucc-cli odm --json
```

### 2.11. `charging` (алиас: `charge`)

Управление профилями, приоритетами и порогами зарядки аккумулятора. Без подкоманды выполняется `charging status`.

| Подкоманда | Алиасы | Аргументы | D-Bus метод | Описание |
|---|---|---|---|---|
| `status` | — | — | `GetCurrentChargingProfile`, `GetChargingProfilesAvailable`, `GetCurrentChargingPriority`, `GetChargingPrioritiesAvailable`, `GetChargeType`, `GetChargeStartThreshold`, `GetChargeEndThreshold` | Текущее состояние зарядки. При `--json`: `{"available", "profile", "priority", "type", "startThreshold", "endThreshold", "availableProfiles", "availablePriorities"}`. |
| `set-profile` | `set_profile` | `<PROFILE>` | `SetChargingProfile` | Установка профиля зарядки. |
| `set-priority` | `set_priority` | `<PRIORITY>` | `SetChargingPriority` | Установка приоритета зарядки. |
| `set-thresholds` | `set_thresholds` | `<START> <END>` | `SetChargeStartThreshold`, `SetChargeEndThreshold` | Установка порогов (`0 <= start < end <= 100`). Применяет в безопасном порядке относительно предыдущего `end` и откатывает первый порог при ошибке второго. Требует Polkit `manage-hardware`. |

```bash
ucc-cli charging status --json
ucc-cli charge set-profile balanced
ucc-cli charging set-thresholds 40 80
```

### 2.12. `keyboard` (алиас: `kb`)

Управление подсветкой клавиатуры и сохранёнными профилями подсветки. Без подкоманды выполняется `keyboard info`.

| Подкоманда | Алиасы | Аргументы | D-Bus метод | Описание |
|---|---|---|---|---|
| `info` | `caps` | — | `GetKeyboardBacklightCapabilitiesJSON` | Возможности подсветки (зоны, максимумы каналов, режимы). При `--json` — объект capabilities. |
| `get` | `show` | — | `GetKeyboardBacklightStatesJSON` | Текущее состояние зон (единый цвет или таблица по зонам). При `--json` — сырой JSON состояний. |
| `set` | — | `<JSON>` | `SetKeyboardBacklightStatesJSON` | Применение конфигурации подсветки из JSON. |
| `color` | — | `<R> <G> <B> [BRIGHTNESS]` | `GetKeyboardBacklightCapabilitiesJSON`, `SetKeyboardBacklightStatesJSON` | Установка статического цвета (`mode: 0`) на все зоны; `BRIGHTNESS` по умолчанию `128` (`0..=255`). |
| `profiles` | `profile-list`, `ls` | — | `GetCustomKeyboardProfilesJSON` | Список пользовательских профилей подсветки с тегами привязок `[AC, ...]`. |
| `activate` | `use` | `<ID>` | `GetCustomKeyboardProfilesJSON`, `SetKeyboardBacklightStatesJSON` | Активация сохранённого профиля по `id` или `name`. |

```bash
ucc-cli kb info --json
ucc-cli keyboard get
ucc-cli keyboard color 255 128 0 200
ucc-cli keyboard activate night-amber
```

### 2.13. `lightbar`

Управление передней RGB-полосой Lightbar.

| Подкоманда | Аргументы | D-Bus метод | Описание |
|---|---|---|---|
| `get` | — | `GetLightbarSettingsJSON` | Чтение конфигурации Lightbar. |
| `status` | — | `GetLightbarStatusJSON` | Чтение текущего состояния Lightbar. |
| `set` | `<JSON>` | `SetLightbarSettingsJSON` | Применение настроек Lightbar из JSON. |
| `test` | — | `TriggerLightbarAnimation(true)` | Принудительный запуск тестовой анимации. |
| `notify` | — | `TriggerLightbarAnimation(false)` | Запуск анимации уведомления (с учётом `managed`/`silent_mode`). |

```bash
ucc-cli lightbar get --json
ucc-cli lightbar status --json
ucc-cli lightbar test
```

### 2.14. `brightness` (алиас: `br`)

| Подкоманда | Аргументы | D-Bus метод | Вывод `--json` |
|---|---|---|---|
| `get` | — | `GetDisplayBrightness` | `{"brightness": 80}` |
| `set` | `<VALUE>` (`0..=100`) | `SetDisplayBrightness` | `OK` / `FAILED` |

```bash
ucc-cli br get --json
ucc-cli brightness set 75
```

### 2.15. `webcam`

| Подкоманда | Аргументы | D-Bus метод | Вывод `--json` |
|---|---|---|---|
| `get` | — | `GetWebcamSWStatus` | `{"webcamEnabled": true}` |
| `set` | `<VALUE>` (`on`/`off`, `true`/`false`, `1`/`0`, `yes`/`no`, `enable`/`enabled`, `disable`/`disabled`) | `SetWebcam` | `OK` / `FAILED` |

```bash
ucc-cli webcam get --json
ucc-cli webcam set off
```

### 2.16. `fnlock` (алиас: `fn-lock`)

| Подкоманда | Аргументы | D-Bus метод | Вывод `--json` |
|---|---|---|---|
| `get` | — | `GetFnLockStatus` | `{"fnLock": false}` |
| `set` | `<VALUE>` (`on`/`off`, `true`/`false`, `1`/`0`, `yes`/`no`, `enable`/`enabled`, `disable`/`disabled`) | `SetFnLockStatus` | `OK` / `FAILED` |

```bash
ucc-cli fnlock get --json
ucc-cli fn-lock set on
```

### 2.17. `devices`

Управление подсистемой энергосбережения периферийных устройств (`powerDevices`).

| Подкоманда | Аргументы | D-Bus метод | Описание |
|---|---|---|---|
| `get` | — | `GetPowerDeviceSettingsJSON` | Чтение настроек устройств питания. |
| `status` | — | `GetPowerDeviceStatusJSON` | Чтение текущего статуса устройств питания. |
| `scan` | — | `ScanPowerDevices` | Запуск сканирования sysfs-устройств. |
| `set` | `<JSON>` | `SetPowerDeviceSettingsJSON` | Запись настроек устройств питания из JSON. |

```bash
ucc-cli devices status --json
ucc-cli devices scan
```

### 2.18. `completions`, `man`, `version`

| Подкоманда | Аргументы | Описание |
|---|---|---|
| `completions` | `<SHELL>` (`bash`, `zsh`, `fish`, `powershell`, `elvish`) | Генерация скрипта автодополнения в `stdout` через `clap_complete`. |
| `man` | — | Генерация man-страницы в формате groff в `stdout` через `clap_mangen`. |
| `version` | — | Вывод `ucc-cli <CARGO_PKG_VERSION>` в `stdout`. |

---

## 3. Сопутствующие бинарные утилиты

### 3.1. `uccd` (`crates/uccd/src/main.rs`)

Системный демон управления оборудованием и D-Bus сервер `com.uniwill.uccd` / `de.uniwill.uccd`.

```text
uccd [OPTIONS]
```

| Флаг | Описание |
|---|---|
| `-v`, `--version` | Вывод версии `uccd` и выход с кодом `0`. |
| `-h`, `--help` | Вывод справки и выход с кодом `0`. |
| `--start` | Запуск демона (режим по умолчанию). |
| `--debug` | Запуск с подробным логированием (`debug,zbus=info`). |
| `--stop` | Остановка работающего экземпляра по PID-файлу (`SIGTERM`, ожидание 5 с, затем `SIGKILL`). |

- **PID-файл**: `/run/uccd.pid` (при отсутствии прав записи в `/run` — `$XDG_RUNTIME_DIR/uccd.pid`, иначе `/run/user/<euid>/uccd.pid`). Проверяются отсутствие symlink (`O_NOFOLLOW`), владелец и `/proc/<pid>/comm == "uccd"`.
- **Переменные окружения**:
  - `RUST_LOG` — фильтр `tracing_subscriber` (по умолчанию `info`; перекрывается флагом `--debug`).
  - `UCC_ALLOW_SESSION_BUS=1` — разрешает fallback на сессионную шину D-Bus при запуске не от `root`.
  - `UCC_OFFLOAD_QUEUE_TIMEOUT_MS` — таймаут ожидания слота в D-Bus offload lane (по умолчанию `500` мс, минимум `10` мс).
- **Завершение**: обработка `SIGINT` и `SIGTERM` с ограниченным таймаутом остановки (`5` с).

### 3.2. `ucc-sleep` (`crates/uccd/src/bin/ucc-sleep.rs`)

Привилегированный (`root`-only) помощник systemd sleep/resume для сохранения и восстановления временного состояния демона при засыпании.

```text
ucc-sleep prepare|resume
```

- `prepare`: применяет политику пробуждения (`wakeOnlyLidAndKeyboard` из конфигурации), захватывает `manual_mode` и `IsWaterCoolerEnabled` по системному D-Bus (таймаут 5 с), сохраняет состояние в `SleepStateStore` и останавливает `uccd.service`.
- `resume`: запускает `uccd.service` (если был активен до сна) и восстанавливает сохранённое состояние.
- **Переменные окружения**: `UCC_SYSTEMCTL` (переопределение пути к `systemctl`, по умолчанию `/usr/bin/systemctl` или `systemctl`; таймаут каждого вызова — 20 с).
- **Ограничения**: требует `geteuid() == 0`; отключён в режиме read-only preview. Коды возврата: `0` — успех, `1` — ошибка аргументов, прав или выполнения.

### 3.3. `ucc-hardware` (`crates/uccd/src/bin/ucc-hardware.rs`)

Привилегированный инструмент прямой диагностики `/dev/tuxedo_io` и аварийного восстановления автоматического управления вентиляторами (используется в `ExecStopPost` systemd-юнита).

```text
ucc-hardware --audit | --fan-auto
```

| Флаг | Действие |
|---|---|
| `--audit` | Считывает температуру и EC duty cycle для всех вентиляторов (`fan=<i> EC-duty=<val> temp=<val>`). |
| `--fan-auto` | Возвращает управление вентиляторами прошивке (`set_fans_auto()`), выводит `Firmware fan control restore: OK|FAILED`. |

Коды возврата: `0` — успех, `1` — аппаратная ошибка чтения/записи, `2` — неверные аргументы, `4` — устройство `/dev/tuxedo_io` недоступно.

### 3.4. `ucc-import-check` (`crates/uccd/src/bin/ucc-import-check.rs`)

Офлайн-валидатор импорта конфигурации. Не обращается к оборудованию, D-Bus и установленным файлам настроек.

```text
ucc-import-check <SETTINGS_JSON> <CAPABILITIES_JSON>
```

Проверяет размер входных файлов (лимит `16 MiB`), синтаксис `UccSettings`, секции `powerDevices` и `lightbar`, соответствие `keyboardBacklightStates` и `customKeyboardProfiles` числу зон и `maxBrightness`/`ledIds` из `CAPABILITIES_JSON`, диапазоны точек кривых `customFanProfiles` (`temp` `0..=150`, `speed` `0..=100`), валидность всех профилей и наличие привязок `power_ac`, `power_type_c`, `power_bat` в `stateMap`.

- Успешный вывод (`exit 0`): `{"ok":true,"profiles":3,"keyboardZones":<N>,"devices":<M>}`.
- Коды возврата: `0` — валидно, `1` — импорт отклонён (причина в `stderr`), `2` — неверное число аргументов.

### 3.5. `ucc-desktop-tray` (`crates/ucc-desktop/src/main.rs`)

Иконка системного трея (`StatusNotifierItem`) для быстрого переключения привязанных профилей, авторежима вентиляторов, СЖО и запуска GUI.

```text
ucc-desktop-tray [--preview] [-h|--help] [-V|--version]
```

| Флаг | Описание |
|---|---|
| `--preview` | Режим предпросмотра (регистрирует имя `com.uniwill.UccDesktopTrayPreview`, игнорирует мутирующие действия). |
| `-h`, `--help` | Вывод краткой справки и выход. |
| `-V`, `--version` | Вывод версии и выход. |

- **Singleton на сессионной шине**: `com.uniwill.UccDesktopTray` (в preview — `com.uniwill.UccDesktopTrayPreview`).
- **Переменные окружения**: `UCC_PREVIEW` (`1`/`true`), `UCCD_DBUS_SERVICE`, `UCCD_DBUS_SESSION`, `XDG_ACTIVATION_TOKEN`.
- **Защита**: отказывается запускаться от `root` (`exit 1`). При недоступности сессионной шины завершается с кодом `2`.

### 3.6. `ucc-session-agent` (`crates/ucc-session-agent/src/main.rs`)

Фоновый сессионный агент: транслирует уведомления рабочего стола на Lightbar, отслеживает `GetDriverHealthJSON`, выводит уведомления о смене источника питания и предлагает переключение частоты обновления экрана (KDE/Hyprland), а также обрабатывает события закрытия крышки (`logind` / `UPower`).

```text
ucc-session-agent [-h|--help] [-V|--version]
```

- **Блокировка единственного экземпляра**: `flock` на `$XDG_RUNTIME_DIR/ucc-session-agent.lock` (fallback `/tmp/ucc-session-agent.lock`).
- **Переменные окружения**: `UCCD_DBUS_SERVICE`, `UCCD_DBUS_SESSION`, `UCC_LID_BUS_SESSION` (переключает подписку `login1`/`UPower` на сессионную шину для тестов).
- **Коды возврата**: `0` — нормальное завершение (или уже запущен экземпляр / режим read-only preview), `2` — ошибка подключения к D-Bus `uccd`.

---

## 4. Автодополнение оболочек и man-страница

### Генерация автодополнений (`bash`, `zsh`, `fish`)

```bash
# Bash
ucc-cli completions bash | sudo tee /usr/share/bash-completion/completions/ucc-cli >/dev/null

# Zsh
ucc-cli completions zsh | sudo tee /usr/share/zsh/site-functions/_ucc-cli >/dev/null

# Fish
ucc-cli completions fish | sudo tee /usr/share/fish/vendor_completions.d/ucc-cli.fish >/dev/null
```

Также поддерживаются цели `powershell` и `elvish` (`ucc-cli completions <shell>`). В репозитории присутствует статический скрипт `ucc-cli/ucc-cli.bash`.

### Man-страница (`ucc-cli.1`)

Статическая man-страница расположена в `ucc-cli/ucc-cli.1` и устанавливается пакетами в `/usr/share/man/man1/ucc-cli.1`. Актуальную страницу также можно сгенерировать напрямую из определений `clap`:

```bash
ucc-cli man > ucc-cli.1
man ./ucc-cli.1
```
