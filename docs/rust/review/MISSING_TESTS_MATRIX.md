# Матрица недостающих тестов UCC-Revorked (`MISSING_TESTS_MATRIX`)

Этот файл содержит полный перечень автотестов, интеграционных проверок, сборочных/пакетных дымовых тестов и стендовых/Wayland-проверок, которые **ещё не написаны** или не покрывают выявленные дефекты (`ISSUE-01`..`ISSUE-41` из [TOTAL_AUDIT_AND_ISSUES.md](./TOTAL_AUDIT_AND_ISSUES.md)).

> Текущее состояние: 387 проходящих Rust-тестов (`daemon_integration` 85/85) и 41 C++ CTest — но они используют изолированные моки на session bus и локальные фикстуры, поэтому не ловят описанные ниже баги безопасности, контрактов, упаковки и реального железа.

---

## 1. Безопасность, Polkit, D-Bus политика и права файлов (`T-SEC`)

| Test ID | Целевой крейт / файл | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-SEC-01` | `crates/uccd` / CLI integration | Запуск бинарника `uccd` при недоступном system D-Bus без явного тестового флага `--session-bus` | `ISSUE-10` | Интеграция (процесс) | Процесс завершается с кодом != 0 и **не** регистрируется на session bus |
| `T-SEC-02` | `crates/ucc-config/tests/` | `ConfigManager::resolve_read_path` при `euid == 0` и установленных `HOME`/`XDG_CONFIG_HOME` с подложенным зловредным `~/.config/ucc/settings` | `ISSUE-11` | Unit/Mock (fakeroot/unshare) | Читается исключительно `/etc/ucc/settings`, пользовательский файл игнорируется |
| `T-SEC-03` | `crates/ucc-config/tests/` | `write_atomic` поверх символической ссылки (`/etc/ucc/settings -> target` с правами `0644` или `0600`) | `ISSUE-16` | Unit (FS) | Симлинк и целевой файл сохраняют `0644`/`0600` (никогда `0777`), `.bak` также имеет `<= 0644` |
| `T-SEC-04` | `crates/ucc-dbus` + `uccd` | Шторм из 200 одновременных медленных `Control`-вызовов (`SetDisplayBrightness` / `ScanPowerDevices`) на D-Bus | `ISSUE-12` | Интеграция | Число ОС-потоков ограничено пулом/семафором (`<= limit`), память не растёт линейно, демон отвечает |
| `T-SEC-05` | `crates/ucc-dbus/tests/` | Мок `org.freedesktop.PolicyKit1.Authority`: проверка точного формата subject `("system-bus-name", {"name": sender})` вместо `unix-process` | `ISSUE-17` | Интеграция D-Bus | `Authority.CheckAuthorization` получает только `system-bus-name` с `sender` звонящего |
| `T-SEC-06` | `crates/ucc-dbus/tests/` | Проверка `flags == 0` для `AuthAction::Read` и возврат `AccessDenied` (без поддельных `""`/`"{}"`/`false`) для неактивного/SSH клиента | `ISSUE-17`, `ISSUE-41` | Интеграция D-Bus | Нет диалога `AllowUserInteraction (0x1)` на чтении; при отказе Polkit возвращается ошибка или валидный `"[]"`/`-1` |
| `T-SEC-07` | `crates/ucc-dbus/tests/` | Матрица авторизации: `SetWaterCoolerPumpVoltage`/`Fan`/`NVIDIAPowerOffset` -> `Control`; `ApplyProfile`/`SaveCustomProfile` с нестандартными TDP или `fan.useControl=false` -> `ManageHardware` | `ISSUE-17` | Интеграция D-Bus | Непривилегированный активный пользователь не может менять TDP или отключать вентиляторы без `ManageHardware` |
| `T-SEC-08` | Private `dbus-daemon` | Тест с реальным `uccd/com.uniwill.uccd.conf` (`<type>system</type>`): вызовы через интерфейс `de.uniwill.uccd` и попытка чужого UID перехватить broadcast-сигналы | `ISSUE-18` | Приватный `dbus-daemon` | `de.uniwill.uccd` разрешён по политике; не-method_call и постороннее прослушивание сигналов отклонены |
| `T-SEC-09` | `crates/uccd` | PID-файл при недоступном `/run`: используется `$XDG_RUNTIME_DIR/uccd.pid` (не `/tmp/uccd.pid`), а `uccd --stop` при PID процесса `ucc-gui`/`ucc-cli` отказывается его убивать | `ISSUE-10` | Юнит / процесс | `uccd --stop` не отправляет сигналы процессам с `comm != "uccd"` и не открывает pre-created файл в `/tmp` |
| `T-SEC-10` | `crates/ucc-hardware/tests/` | `PowerDeviceController`: таймаут дочерней команды (`sleep`) без гонки `libc::kill` по переиспользованному PID и использование абсолютных доверенных путей бинарников | `ISSUE-34` | Unit/Process | Зависший процесс убивается до `wait()` на одном дескрипторе; подмена `PATH` не перехватывает `nvidia-smi`/`iw`/`systemctl` |
| `T-SEC-11` | `crates/uccd/tests/` | Лимиты размера и количества профилей: `SaveCustomProfile`, `SaveCustomKeyboardProfile`, `SetFanProfile` с payload > 256 KiB или > 64 профилями | `ISSUE-34` | Интеграция D-Bus | Превышение лимитов отклоняется (`false`) без раздувания RAM и `/etc/ucc/settings` |
| `T-SEC-12` | `crates/uccd` + `ucc-hardware` | Запуск с `UCC_READ_ONLY_PREVIEW=1` при попытке записи в `MockPlatformBackend`, `MockSysfs` (CPU/LED/backlight) и `ConfigManager` | `ISSUE-35` | Интеграция | **Ноль** записей в `platform.journal()`, `sysfs.journal()` и файлы конфига |
| `T-SEC-13` | `crates/ucc-config/tests/` | Проверка отсутствия гонки `getenv`/`setenv` вокруг `UCC_READ_ONLY_PREVIEW` под множеством потоков (`--test-threads=16`) | `ISSUE-35` | Stress test | Конфигурация `read_only` передаётся явно без вызова `std::env::set_var` в многопоточных тестах |
| `T-SEC-14` | `crates/uccd` + `ucc-hardware` | Инъекция некорректных путей/интерфейсов в `scan_power_devices()` (напр. Wi-Fi имя с `../` или `;`) | `ISSUE-34` | Unit | Невалидные устройства отсеиваются валидацией *до* сохранения в `settings.json` |

---

## 2. Ядро демона `uccd`, транзакции, воркеры и lifecycle (`T-DAEMON`)

| Test ID | Целевой крейт / файл | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-DAEMON-01` | `crates/uccd/tests/daemon_integration.rs` | Круговой тест `SetWaterCoolerPumpVoltage` <-> `GetWaterCoolerPumpLevel` для всех кодов `0 (11V), 1 (reject!), 2 (7V), 3 (8V), 4 (Off)` и `-1 (Unset)` | `ISSUE-01` | Интеграция | `0->V11`, `1->false`, `2->V7`, `3->V8`, `4->Off`; геттер возвращает те же коды (`0/2/3/4/-1`), а при отключении `-1` |
| `T-DAEMON-02` | `crates/uccd/tests/` | Полный цикл `BluezCoolerClient` с мок BlueZ D-Bus: авто-поиск, загрузка/сохранение `aquaris.json`, сброс очереди GATT, дебаунс 3с (connect) / 10с (disconnect) -> `power_wc` | `ISSUE-02` | Интеграция BLE mock | `water_cooler_connected` переключается в `true`, команды доходят до GATT характеристики, `PowerStateChanged` уходит после дебаунса |
| `T-DAEMON-03` | `crates/uccd/tests/` | СЖО: температурная кривая помпы/вентилятора (асимметричный гистерезис 3°C) + режим LED `RgbState::Temperature` при изменении температуры CPU | `ISSUE-02` | Интеграция BLE mock | При нагреве скорость/напряжение растут сразу, при остывании удерживаются по гистерезису, цвет LED плавно меняется |
| `T-DAEMON-04` | `crates/uccd/tests/` | Перехват ручного режима СЖО (`test_watercooler_override`): `SetWaterCoolerFanSpeed` / `SetWaterCoolerPumpVoltage` отключают `autoControlWC`, сохраняют ручные значения и переживают смену профиля | `ISSUE-24` | Интеграция D-Bus | `auto_control_wc == false`, эмит `ProfileChanged`, ручные значения переигрываются после реконнекта, `water_cooler_enabled` сохраняется |
| `T-DAEMON-05` | `crates/uccd/tests/` | `LidController` + `logind` мок: захват ингибитора `handle-lid-switch`, закрытие крышки по разным `ac`/`type_c`/`bat` (`ignore`, `lock`, `suspend`, `hibernate`), валидация `lid` и обновление `WakeupPolicy` | `ISSUE-03` | Интеграция D-Bus | Ингибитор удерживается, правильный метод `login1` вызывается по активному источнику питания, `/proc/acpi/wakeup` и `power/wakeup` обновляются корректно (`with_sys_proc` без дубля `/sys/sys`) |
| `T-DAEMON-06` | `crates/ucc-hardware` (`ucc-sleep` bin) + `uccd` | `ucc-sleep prepare` -> `/run/ucc/sleep-state.json` (`0600`) -> рестарт `uccd` -> `ucc-sleep resume` (гонка `Type=dbus`) | `ISSUE-04` | Процесс + D-Bus | `uccd` восстанавливает `manualMode`, `water_cooler_enabled` и пропускает lightbar welcome *до* удаления `sleep-state.json` |
| `T-DAEMON-07` | `crates/ucc-hardware` (`ucc-hardware` bin) | `ucc-hardware --fan-auto` на не-MECHREVO DMI (напр. `TUXEDO` / `XMG` / `Clevo`) + `ExecStopPost` без `-` фейла при отсутствии драйвера | `ISSUE-05` | Бинарник + mock ioctl | `ucc-hardware --fan-auto` сбрасывает `set_fans_auto()` с кодом `0` независимо от DMI строки |
| `T-DAEMON-08` | `crates/uccd/tests/` | Разрешение именованного `fan.fanProfile` (`"fan-cool"`, custom) при пустых embedded таблицах + сброс `temporary_curves` при смене профиля + изоляция `SetFanProfileCPU` от GPU-кривой | `ISSUE-24` | Интеграция | Не срабатывает 100%-авария из-за пустых таблиц, временные кривые не утекают в новый профиль |
| `T-DAEMON-09` | `crates/uccd/tests/` | Инициализация глобальных настроек зарядки на старте `activate_hardware()` + применение per-profile зарядки и `nvidiaCTGPOffset` при смене профиля + чтение sysfs `charge_control_*_available_thresholds` | `ISSUE-08`, `ISSUE-24` | Интеграция | Пороги/приоритет/cTGP применяются при загрузке и смене профиля, список доступных порогов берётся из sysfs |
| `T-DAEMON-10` | `crates/uccd/tests/` | Работа на неподдерживаемом устройстве (`DeviceVendor::Unsupported`) в пассивном режиме | `ISSUE-24` | Интеграция | `uccd` отдаёт D-Bus API, но делает **0** записей в CPU/sysfs/PowerDevices |
| `T-DAEMON-11` | `crates/uccd/tests/` | Отравление (poison) `Mutex`/`RwLock` из-за паники в воркере с последующим вызовом всех non-offloaded Read D-Bus методов | `ISSUE-19` | Stress / Fault | Ни один D-Bus метод не паникует в Tokio runtime |
| `T-DAEMON-12` | `crates/uccd/tests/` | Блокирующий `prime-select query` (зависает на 10с) и медленный sysfs при одновременных Read RPC (`GetPrimeState`, `GetSystemInfoJSON`, `GetCpuTopologyJSON`) | `ISSUE-19` | Интеграция / Async | Tokio reactor остаётся отзывчивым (<50 мс) для параллельных запросов, `prime-select` прерывается по таймауту |
| `T-DAEMON-13` | `crates/uccd/tests/` | Эмиссия сигналов `ModeReapplyPendingChanged(b)` и `PowerDeviceStatusChanged(s)` при переключении AC/Type-C/Bat без изменения `pd_rev` | `ISSUE-34` | Интеграция D-Bus | Оба сигнала реально приходят подписчикам на шине при соответствующих переходах |
| `T-DAEMON-14` | `crates/uccd/tests/` | 10-секундный таймаут авто-отключения `sensor_data_collection` без вызовов `*ValuesJSON` (и пробуждение при опросе) | `ISSUE-34` | Интеграция (virtual clock) | После 10с простоя опрос NVML/RAPL прекращается; при следующем вызове возобновляется |
| `T-DAEMON-15` | `crates/uccd/tests/` | Жизненный цикл кастомных профилей: `AddCustomProfile` с пустым `id`, отказ на имя встроенного профиля, `DeleteCustomProfile` активного профиля (сразу переключает железо), разделение `GetProfilesJSON`/`GetCustomProfilesJSON` | `ISSUE-34` | Интеграция D-Bus | Уникальный ID генерируется, встроенные имена защищены, активный профиль и железо переключаются на запасной сразу |
| `T-DAEMON-16` | `crates/uccd/tests/` | Смена системного профиля с привязанным `selectedKeyboardProfile` и `SetKeyboardBacklightStatesJSON` с объектом `{"keyboardProfileId": ..., "states": [...]}` | `ISSUE-13` | Интеграция D-Bus | Клавиатура меняет цвет при смене системного профиля, эмитятся сигналы, sysfs проверяется *до* записи на диск |
| `T-DAEMON-17` | `crates/uccd/tests/` | `apply_profile_internal` с плавной PWM-рампой бэклайта (задержка `actual_brightness` на 100 мс) | `ISSUE-23` | Интеграция | Профиль CPU/ODM/TDP **не откатывается** из-за отставания `actual_brightness`; также проверяется MUX и инверсия `amdgpu_bl` |
| `T-DAEMON-18` | `crates/uccd/tests/` | Отказ всех 4 каналов на старте с `High Performance` на 2-профильном Uniwill (`perf_profiles_available == 2`) | `ISSUE-35` | Интеграция | Демон подменяет `"overboost"` на максимальный доступный код (`enthusiast`), старт успешен |

---

## 3. Аппаратный слой `ucc-hardware`, телеметрия и `ucc-lighting` (`T-HW`)

| Test ID | Целевой крейт / файл | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-HW-01` | `crates/uccd` + `ucc-hardware` | Подключение `CpuController`/`CpuWorker`: расширение `min/max` частот без `EINVAL` при снижении `max < old_min`, сброс в `cpuinfo_min/max_freq` при `None`, сентинелы `-1`/`-2` | `ISSUE-07` | Интеграция | `scaling_max_freq`/`min_freq` никогда не дают `EINVAL`, `None` снимает прежний потолок частот |
| `T-HW-02` | `crates/ucc-hardware` + `uccd` | `intel_pstate` EPP `EBUSY` при `governor=performance`, запись AMD `/sys/devices/system/cpu/cpufreq/boost`, и `GetCpuCoreCount` при 4/16 отключённых ядрах | `ISSUE-07` | Unit/Mock | `EBUSY` EPP не роняет профиль; AMD `boost` пишется; `GetCpuCoreCount` возвращает 16 (`present`) |
| `T-HW-03` | `crates/uccd` + `ucc-hardware` | Кеширование `CpuTopology` P/E ядер до отключения ядер профилем и повторное чтение `GetCpuTopologyJSON` | `ISSUE-28` | Unit/Mock | Отключённые E-ядра сохраняют тип `"E"` вместо `"unknown"` |
| `T-HW-04` | `crates/ucc-hardware/tests/` | Uniwill `read_fan_temp` возвращает `0` -> трактуется как `SensorUnavailable`, `FanController` переводит вентиляторы в `set_fans_auto()` | `ISSUE-09` | Unit/Mock | `read_fan_temp == 0` **не** выставляет `0%` PWM, срабатывает auto-failsafe |
| `T-HW-05` | `crates/ucc-hardware/tests/` | Пробинг `R_UW_TDP2`, `R_UW_TDP1`, `R_UW_TDP0` для ноутбуков Uniwill с 0, 1, 2 и 3 каналами TDP | `ISSUE-20` | Unit/Mock | `caps.tdp_channels` точно равен числу рабочих каналов; активация профиля на 1/2-TDP ноутбуке не падает |
| `T-HW-06` | `crates/ucc-hardware/tests/` | `PowerDeviceController::apply` при `wake_nvidia = false` и NVIDIA dGPU в состоянии `power/runtime_status == "suspended"` | `ISSUE-21` | Unit/Mock ProcessRunner | `nvidia-smi -pm 0` **не** вызывается, dGPU не просыпается; `get_system_info_json` не читает `/proc/driver/nvidia` в D3cold |
| `T-HW-07` | `crates/ucc-hardware/tests/` | Авто-обнаружение AMD dGPU (`0x1002`), проверка `runtime_status == "active"` перед чтением `amd_dgpu_hwmon`, и ответ `GetDGpuInfoValuesJSON` при отсутствии dGPU | `ISSUE-21` | Unit/MockSysfs | AMD dGPU находится и не будится из D3cold; при отсутствии dGPU возвращается нейтральный `unknown`, а не `nvidia` |
| `T-HW-08` | `crates/ucc-hardware/tests/` | Intel `i915`/`xe` телеметрия: чтение `energy1_input` мкДж-дельты, путь `gt/gt0/rps_act_freq_mhz`, и сброс `RaplEnergyCounter` после S3/s2idle без спайка `> 1000 W` | `ISSUE-22` | Unit/MockSysfs | Корректный расчёт Вт из `energy1_input` и отсутствие пиков 100 кВт при сбросе `energy_uj` к 0 |
| `T-HW-09` | `crates/ucc-hardware/tests/` | `DisplayBacklight`: приоритизация активного eDP коннектора и `raw > native > firmware` + инверсия `amdgpu_bl` | `ISSUE-23` | Unit/MockSysfs | На гибридной системе выбирается активный контроллер панели, а не алфавитно первый |
| `T-HW-10` | `crates/ucc-hardware/tests/` | Linux KMS без `@Hz` в `/modes`: извлечение частот EDID/X11/Wayland и реализация `SetDisplayRefreshRate` | `ISSUE-23` | Unit/Mock | `GetDisplayModesJSON` отдаёт реальные Hz, `SetDisplayRefreshRate` реально переключает (или возвращает честный статус) |
| `T-HW-11` | `crates/ucc-hardware/tests/` | `NvApiState`: повторная инициализация после временного сбоя + сброс кешированных `gpu_handles` + защита от `NULL` handle при `NVML_SUCCESS` | `ISSUE-36` | Unit/FFI mock | Вольтметр восстанавливается при появлении GPU, null-handle не вызывает UB |
| `T-HW-12` | `crates/ucc-hardware/tests/` | `TuxedoIoBackend::read_perf_profile` с инъектированным `MockSysfs` при отсутствующем файле на реальном хосте `/sys` | `ISSUE-36` | Unit/MockSysfs | Читает данные из инъектированного `Sysfs`, а не `std::fs::read_to_string` |
| `T-HW-13` | `crates/ucc-hardware/tests/fake_bluez_cooler.rs` | `BluezCoolerClient` с посторонним BLE NUS-устройством (`Flipper-01`) и некэшированным кулером (`StartDiscovery`) | `ISSUE-36` | Unit/BLE mock | Вызывается `StartDiscovery`, посторонний NUS игнорируется, таймаут `Disconnect` >= 1с |
| `T-HW-14` | `crates/ucc-lighting/tests/` | `SysfsLedTransport::parse_zone_id` для `0003:048D:CE00.0001:rgb:kbd_backlight_21`, приоритет `rgb:lightbar` > `lightbar`, и сброс `anim_started` в `LightbarController::configure()` | `ISSUE-25`, `ISSUE-36`, `ISSUE-41` | Unit | Зона = `21`, выбирается `rgb:lightbar`, новая анимация стартует с `t=0` |

---

## 4. Конфигурация `ucc-core` и `ucc-config` (`T-CFG`)

| Test ID | Целевой крейт / файл | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-CFG-01` | `crates/ucc-config/tests/` | Round-trip реального C++ `/etc/ucc/settings` с 5 плоскими полями зарядки (`chargingProfile`..`chargeEndThreshold`), `nvidiaCTGPOffset`, `keyboard.brightness` и неизвестными полями | `ISSUE-08` | Unit | После `load` + `save` ни одно из этих полей не потеряно и читается обратно |
| `T-CFG-02` | `crates/ucc-config/tests/` | Поддержка всех sysfs `ChargeType` (`Trickle`, `Fast`, `Adaptive`, `LongLife`, `Bypass`) в `UccProfile` и `LegacyProfileDto` | `ISSUE-08` | Unit | Сериализация и десериализация проходят без ошибок и искажений |
| `T-CFG-03` | `crates/ucc-config/tests/` | Загрузка `/etc/ucc/settings`, где 1 неактивный старый профиль имеет `onlineCores: 0` / `TDP1 < TDP0` / равные точки кривой | `ISSUE-26` | Unit | Файл загружается успешно, здоровые профили и `stateMap` сохраняются |
| `T-CFG-04` | `crates/ucc-config/tests/` | Сравнение `defaults.rs` с эталоном C++ `DefaultProfiles.cpp`: ID `fan-*`, `auto_control_wc=true`, `enable_water_cooler=false`, таблицы моделей | `ISSUE-35` | Unit | Полное совпадение полей и модельных списков |
| `T-CFG-05` | `crates/ucc-config/tests/` | Кастомный профиль `"My Gaming Performance"` в `UccSettings::get_profile("My Gaming Performance")` после удаления | `ISSUE-41` | Unit | Возвращается `None`, а не `"High Performance [Built-in]"` по подстроке |
| `T-CFG-06` | `crates/ucc-config/tests/` | `cleanup_temp_files` после прерванного `restore_from_backup` (`.tmp_restore.*`) | `ISSUE-41` | Unit/FS | Временные файлы восстановления удаляются |
| `T-CFG-07` | `crates/ucc-config/tests/` | Единый источник при наличии `/etc/ucc/settings.json` vs `/etc/ucc/settings` | `ISSUE-11` | Unit/FS | Чтение и запись идут в один согласованный путь без рассинхрона |
| `T-CFG-08` | `crates/ucc-core/tests/` | Проверка сохранения сентинелов `-1` и `-2` в `ProfileCpu` при десериализации/сериализации | `ISSUE-07` | Unit | `-1` и `-2` не схлопываются в `None` |

---

## 5. Контрактные и интеграционные тесты `ucc-cli` (`T-CLI`)

| Test ID | Целевой крейт / файл | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-CLI-01` | `crates/ucc-cli/tests/` | `ucc-cli keyboard color`, `keyboard get`, `keyboard activate <id>`, `keyboard info` против живого Rust `uccd` | `ISSUE-13` | Интеграция D-Bus | Все 4 команды работают без ошибок схемы/парсинга и меняют реальное состояние в `uccd` |
| `T-CLI-02` | `crates/ucc-cli/tests/` | `ucc-cli lightbar status` vs `lightbar get` и `ucc-cli devices status` vs `devices get` | `ISSUE-27` | Интеграция D-Bus | `status` вызывает `*StatusJSON`, `get` вызывает `*SettingsJSON` |
| `T-CLI-03` | `crates/ucc-cli/tests/` | `ucc-cli power-limits` с полным массивом объектов `[{"index", "min", "max", "current"}]` | `ISSUE-27` | Интеграция D-Bus | Печатает все лимиты, а не `"No ODM power limits available"` |
| `T-CLI-04` | `crates/ucc-cli/tests/` | `ucc-cli profile get/defaults/customs`: вывод `energyPerformancePreference`, `odmPowerLimits.tdpValues`, частот, `useBrightness`-гварда и тегов `[AC, BAT]` в `profile/fan/keyboard list` | `ISSUE-27` | Интеграция D-Bus | Все поля и теги `[AC, BAT]` присутствуют в тексте и `--json` |
| `T-CLI-05` | `crates/ucc-cli/tests/` | `ucc-cli watercooler pump {0,1,2,3,4}` против `uccd` | `ISSUE-01` | Интеграция D-Bus | `0` включает 11V, `4` выключает, `1` даёт ошибку и `exit != 0` |
| `T-CLI-06` | `crates/ucc-cli/tests/` | Матрица кодов возврата при D-Bus ошибке, отказе Polkit и `-1` яркости по всем 14 подкомандам (чтение и запись) | `ISSUE-27` | Интеграция D-Bus | Любой сбой/отказ или недоступный brightness даёт ненулевой код выхода и ошибку в `stderr` |
| `T-CLI-07` | `crates/ucc-cli/tests/` | Подвисающий `uccd` для всех 12 модулей помимо `status`/`charging` (`profile`, `fan`, `keyboard`, `lightbar`, `power`, `cooler`, и др.) | `ISSUE-27` | Интеграция D-Bus | Все команды укладываются в RPC-таймаут (`< 2s`) вместо 25с зависания |
| `T-CLI-08` | `crates/ucc-cli/tests/` | `ucc-cli version`, `webcam set enabled/disabled`, `monitor --count=-1` и `--interval 0` | `ISSUE-39` | CLI unit/proc | `version` и `enabled`/`disabled` работают; `--count=-1` отклоняется clap |
| `T-CLI-09` | `crates/ucc-cli/tests/` | `ucc-cli cooler status` при включённой но отключённой по BLE СЖО и `GetFanData*` с `timestamp == 0` | `ISSUE-39` | Интеграция D-Bus | Печатается статус подключения, не показываются фейковые `0 %` или нулевые сэмплы |
| `T-CLI-10` | `crates/ucc-cli/tests/` | `ucc-cli gpu` после вызова `SetNVIDIAPowerOffset` | `ISSUE-39` | Интеграция D-Bus | Актуальный оффсет отображается в выводе `gpu` |
| `T-CLI-11` | `crates/ucc-cli/tests/` | Совместимость `ucc-cli status --json` с полями, ожидаемыми внешними скриптами | `ISSUE-27` | Интеграция | Корректные ключи без `-1` в `displayBrightness` |
| `T-CLI-12` | `crates/ucc-cli` | Запуск всех 3 shells генерации completions и manpage | `ISSUE-31` | Unit | Валидный синтаксис и отсутствие паник |

---

## 6. Межъязыковые контракты C++ GUI / `ucc-session` / `ucc-gnome` / `ucc-tray` <-> Rust `uccd` (`T-CLIENT`)

| Test ID | Целевой файл / бинарник | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-CLIENT-01` | `tests/` (C++ QtTest + Rust `uccd`) | Проверка всех 14 `bool`-методов через `libucc-dbus/UccdClient` при возврате `false` от Rust `uccd` | `ISSUE-28` | Cross-lang D-Bus | Каждый метод возвращает `false` (не замаскировано `callVoidMethod` в `true`) |
| `T-CLIENT-02` | `tests/` + `ucc-gui` | Выбор профиля клавиатуры в `KeyboardTab` и профиль СЖО в `FanControlTab` против Rust `uccd` | `ISSUE-01`, `ISSUE-13` | Cross-lang D-Bus | `KeyboardTab` применяет цвета, `FanControlTab` корректно отображает и ставит `11V`/`Off` |
| `T-CLIENT-03` | `tests/test_gui_audit.cpp` | `BatterySettingsWidget::apply()` при поднятии окна `40–60` -> `60–90` против `LiveUccdBackend` | `ISSUE-28` | Cross-lang D-Bus | Сначала ставится `End=90`, потом `Start=60`, оба порога применены |
| `T-CLIENT-04` | `tests/test_gui_audit.cpp` | `MainWindow::onSaveClicked()`: снятие привязки `power_ac`/`power_bat`/`power_type_c`/`power_wc` и нажатие `Save` на встроенном профиле | `ISSUE-29` | Qt Widget Test | Старые ключи убираются из `stateMap`; спиннер `"Saving profile..."` не зависает |
| `T-CLIENT-05` | `tests/test_gui_audit.cpp` | `CpuCoreSelectionWidget` при реконнекте `uccd` с уже отключёнными E-ядрами | `ISSUE-28` | Cross-lang D-Bus | Виджет перезагружается при подключении, E-ядра остаются отмеченными `· E` |
| `T-CLIENT-06` | `tests/test_gui_audit.cpp` | `PumpCurveEditorWidget` drag-and-drop по X (`left=80`) и загрузка 3/4-точечной кривой помпы без пометки профиля dirty | `ISSUE-37` | Qt Widget Test | Температура при перетаскивании не прыгает, кривая не искажается |
| `T-CLIENT-07` | `tests/test_gui_audit.cpp` | `AppLogging::configure(true)` при системном правиле `QLoggingCategory::setFilterRules("*.debug=false")` | `ISSUE-37` | Qt Unit Test | Файл лога реально содержит записи уровня `DEBUG` (и `ucc.dbus`), `saveLogs` выключен по умолчанию |
| `T-CLIENT-08` | `tests/test_gui_audit.cpp` | Инициализация `LCTWaterCoolerController::m_connectedModel`, отсутствие синхронных D-Bus-вызовов в таймере/слайдере, `HardwareTab` при `WebcamSWAvailable=false` | `ISSUE-29`, `ISSUE-37` | Qt Unit/Valgrind | Нет UB/блокировок GUI, тумблеры недоступных функций отключены |
| `T-CLIENT-09` | `tests/` (`gjs` + `jq` + mock `uccd`) | Проверка синтаксиса `ucc-gnome/metadata.json` (`settings-schema`), вызов `GetKeyboardBacklightControlSupported`, `GetWaterCoolerConnected`, передача `fanProfileId` | `ISSUE-30`, `ISSUE-38` | GJS + D-Bus | Меню клавиатуры и СЖО отображаются при ручном режиме питания, выбор вентилятора не слетает через 5с |
| `T-CLIENT-10` | `tests/test_refresh_advisor.cpp` + новый тест | `ucc-session`: `NotificationMonitor` игнорирует собственные уведомления `ucc-session` и fallback при отсутствии `hyprctl eval` | `ISSUE-38` | Qt + Mock IPC | Отсутствие петли self-trigger на lightbar, работает `hyprctl dispatch dpms` / `keyword monitor` |
| `T-CLIENT-11` | `tests/` | `ucc-desktop-tray` C++ при запуске **до** появления `org.kde.StatusNotifierWatcher` и запуск `ucc-gui` из другого `PATH` / в режиме preview | `ISSUE-30` | Integration | Процесс не падает на логине Wayland и запускает нужный бинарник `ucc-gui` |
| `T-CLIENT-12` | `tests/` | Валидация бинарного `GetMonitorDataSince` + D-Bus сигналов через `UccdClient` на длинном прогоне | Baseline check | Cross-lang D-Bus | 0 пропусков, 0 утечек подключений при рестарте демона |

---

## 7. Rust Desktop Tray & Session Agent (`T-DESK`)

| Test ID | Целевой крейт / файл | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-DESK-01` | `crates/ucc-desktop/tests/` | Запуск бинарника `ucc-desktop-tray`, регистрация SNI на session D-Bus, вызов `ksni::Handle::update` при `ProfileChanged`/`PowerPolicyChanged` | `ISSUE-14` | Интеграция D-Bus | Панель получает `LayoutUpdated` / `NewIcon` / `NewToolTip`, команды меню доходят до `uccd` |
| `T-DESK-02` | `crates/ucc-desktop/tests/` | Запуск `ucc-desktop-tray` от root (должен отказаться) и повторный запуск второго экземпляра (singleton) | `ISSUE-14` | Process test | Отказ при `uid == 0`; второй инстанс корректно завершается/активирует первый |
| `T-DESK-03` | `crates/ucc-desktop/tests/` | `SessionTelemetry`: расчёт суммарных Вт по двум батареям, статус dGPU при `runtime_status=suspended` (0 пробуждений) и сканирование `/proc/<pid>/fd` с защитой от зомби и смены `startTime` | `ISSUE-14` | Unit/MockProc | Точная мощность, отфильтрованы чужие UID и `scope=Device`, нет пробуждения dGPU |
| `T-DESK-04` | `crates/ucc-desktop/tests/` | `ProcessTerminator`: `pidfd_open` + проверка `startTime` перед `SIGTERM`/`SIGKILL` (и отказ при подмене процесса или `readOnlyPreview`) | `ISSUE-14` | Process test | Убивает только целевой процесс, защищён от PID reuse и preview-режима |
| `T-DESK-05` | `crates/ucc-desktop/tests/` | Передача `XDG_ACTIVATION_TOKEN` при клике «Открыть UCC» в `com.uniwill.UccGui` или запуске `ucc-gui --show` | `ISSUE-14` | D-Bus mock | Токен доходит до `ActivateWithToken` или в окружении запускаемого процесса |
| `T-DESK-06` | `crates/ucc-session` (новый крейт) | Rust `NotificationMonitor`: реакция на сторонний `org.freedesktop.Notifications.Notify` с дебаунсом и игнор собственных уведомлений | `ISSUE-14`, `ISSUE-38` | D-Bus session mock | Вызывается `TriggerLightbarAnimation(false)` при включённом режиме |
| `T-DESK-07` | `crates/ucc-session` (новый крейт) | Rust `DriverAlert` и `PowerSourceAlert`: уведомление при ошибке `GetDriverHealthJSON` + `CloseNotification` при выздоровлении; алерт смены источника питания | `ISSUE-14` | D-Bus session mock | Корректные `urgency=2` уведомления и авто-закрытие |
| `T-DESK-08` | `crates/ucc-session` (новый крейт) | Rust `RefreshAdvisor` & `LidDisplayHandler`: смена Hz и DPMS только для встроенной `eDP-*` панели в Hyprland/KDE, отмена предложения при `PrepareForSleep` | `ISSUE-14` | Mock IPC + D-Bus | Внешние мониторы не затрагиваются, целевая частота проверяется через 250 мс |

---

## 8. Сборка, пакеты, CI и чистый checkout (`T-PKG`)

| Test ID | Контур / скрипт | Что проверяет (сценарий и инвариант) | ISSUE | Уровень | Критерий прохождения |
|---|---|---|---|---|---|
| `T-PKG-01` | Git / CI | `git worktree add` чистого дерева -> `cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` без локальных путей `/home/frixit` и без `artifacts/` | `ISSUE-15` | Clean checkout CI | 100% сборка и прохождение всех тестов в изолированном каталоге |
| `T-PKG-02` | `crates/xtask/tests/` | Проверка `build_verification.rs` против реальных `[workspace].members`, проверка не-UTF-8 путей в `xtask`, запуск `cargo xtask dist` в изолированную staging-директорию | `ISSUE-39`, `ISSUE-31` | Unit/CLI | Никаких записей в `/tmp/ucc-*`; в staging есть все нужные бинарники, юниты и ассеты |
| `T-PKG-03` | CMake + `ldd` | Сборка C++ `ucc-gui` по `ADR-01` и проверка бинарника `ldd ucc-gui` | `ISSUE-29` | Build audit | `0` зависимостей на `KF6*`, `Qt6Qml`, `Qt6Quick`, `Qt6Bluetooth`, `libucc-dbus.so`; передача `XDG_ACTIVATION_TOKEN` работает |
| `T-PKG-04` | Debian (`dpkg-buildpackage`) | Полная сборка `.deb` в чистом окружении (эмуляция non-x86_64/x86_64) + `dpkg -c` + `lintian` | `ISSUE-06`, `ISSUE-31` | Packaging CI | В `.deb` установлены Rust `uccd`, `ucc-cli`, хелперы `/usr/lib/ucc/*`, `ucc-gui`; нет `ucc-tray`/`libucc-dbus.so`; 0 ошибок `dh_missing`/lintian |
| `T-PKG-05` | RPM (`rpmbuild`) | Сборка `ucc.spec` с включённой проверкой `%files` (check-files) | `ISSUE-06`, `ISSUE-31` | Packaging CI | 0 неупакованных файлов, 0 битых симлинков, в `%post`/`%preun` корректно обрабатываются юниты |
| `T-PKG-06` | Arch (`makepkg`) | Сборка `PKGBUILD` в чистом окружении (`makepkg -s`) | `ISSUE-06` | Packaging CI | Пакет содержит Rust-бинарники и сгенерированные completions/manpages |
| `T-PKG-07` | Nix (`nix build` + NixOS test) | Сборка `nix build .#ucc` (`package.nix`) и NixOS VM-тест `nix/nixos-module.nix` | `ISSUE-32` | Nix build / VM | `postFixup` проходит без ошибок; нет ссылок на `/usr/bin/systemctl` или `/usr/lib/ucc`; `ExecStopPost --fan-auto` и `ReadWritePaths` присутствуют |
| `T-PKG-08` | `systemd-analyze verify` | Валидация всех unit-файлов (`uccd.service`, `uccd-sleep.service`, `com.uniwill.uccd.service`) и `com.uniwill.uccd.conf` / `.policy` | `ISSUE-05`, `ISSUE-18`, `ISSUE-04` | Static / container check | `systemd-analyze verify` без фатальных предупреждений, `xmllint` для XML конфигов |

---

## 9. Живой аппаратный стенд и реальные Wayland-сессии (`T-LIVE`)

| Test ID | Платформа / стенд | Что проверяет (сценарий и инвариант) | ISSUE | Среда | Критерий прохождения |
|---|---|---|---|---|---|
| `T-LIVE-01` | Ноутбук Uniwill + Clevo (`/dev/tuxedo_io`) | Реальные ioctl: чтение/запись кривых вентиляторов, 0/1/2/3 каналов TDP, ODM perf profiles, webcam, Fn-Lock, `temp <= 0` при отсутствующем датчике | `ISSUE-09`, `ISSUE-20`, `M13` | Живое железо | Реальные регистры EC меняются и считываются без `EIO`/`EINVAL` и зависаний |
| `T-LIVE-02` | Ноутбук с NVIDIA Optimus (`D3cold`) | 10 минут на батарее с включённой телеметрией и сменами профилей (где `wake_nvidia=false`) | `ISSUE-21`, `M13` | Живое железо | `power/runtime_status` остаётся `"suspended"` (`0` пробуждений от `uccd`, `nvidia-smi` не будит GPU) |
| `T-LIVE-03` | Intel (`i915`/`xe`) + AMD APU/dGPU | Точность счётчиков мощности (`energy1_input`, RAPL) и частот (`gt/gt0/rps_act_freq_mhz`), плюс проход через `S3`/`s2idle` | `ISSUE-21`, `ISSUE-22` | Живое железо | Нет пиков >1000 Вт после сна, частоты и ватты совпадают с `turbostat`/`intel_gpu_top`/`sensors` |
| `T-LIVE-04` | MECHREVO/TUXEDO Aquaris (`LCT22002`) BLE | Длительный прогон СЖО: обнаружение, переключения 7V/8V/11V/Off, ручной vs авто режим, обрыв BLE, сон/пробуждение | `ISSUE-01`, `ISSUE-02`, `ISSUE-24`, `M5.8` | Живая СЖО LCT | Помпа реально регулирует напряжение (11V = макс, 4 = стоп), реконнект <= 10с, нет залипаний |
| `T-LIVE-05` | Real `systemd` + Polkit | `sudo systemctl start/stop uccd`, принудительный `SIGKILL -9 uccd` (проверка `ExecStopPost` `--fan-auto`), проверка D-Bus прав от активного юзера, SSH и `root` | `ISSUE-05`, `ISSUE-17`, `ISSUE-18` | Живая система + systemd | После `SIGKILL` EC возвращается в `auto`; `uccd.service` не падает в `failed`; SSH получает отказ на запись |
| `T-LIVE-06` | Реальный S3/s2idle сон (`uccd-sleep.service`) | Перевод ноутбука в сон с ручным режимом питания и включённой СЖО -> пробуждение | `ISSUE-04` | Живая система | После пробуждения `manualMode` и СЖО восстановлены автоматически без гонки `Type=dbus` |
| `T-LIVE-07` | Реальный eDP бэклайт + MUX + charging EC | Регулировка яркости с PWM-рампой (проверка отсутствия ложного отката профиля), пороги `charge_control_*`, `nvidiaCTGPOffset` | `ISSUE-08`, `ISSUE-23`, `ISSUE-24` | Живое железо | Профиль не откатывается на лаге `actual_brightness`; пороги заряда держат лимит |
| `T-LIVE-08` | Hyprland Wayland session | Живой прогон: закрытие крышки (DPMS `screen_off` / `suspend` / `ignore`), смена Hz встроенного экрана `RefreshAdvisor` при смене AC/Bat, клик из `ucc-desktop-tray` -> фокус `ucc-gui` (`XDG_ACTIVATION_TOKEN`) | `ISSUE-03`, `ISSUE-14`, `ISSUE-38` | Реальный Hyprland | Монитор гаснет и зажигается без фризов, Hz переключается и не сбивает внешний монитор, `ucc-gui` получает фокус |
| `T-LIVE-09` | KDE Plasma 6 Wayland session | Тест SNI-трея, отложенного старта `StatusNotifierWatcher`, HiDPI 100/125/150/200%, кириллицы/IME, таб-навигации и `kscreen-doctor` Hz | `ISSUE-14`, `ISSUE-30`, `M1.3`-`M1.5` | Реальный Plasma 6 | Трей появляется при логине, масштабирование/фокус/ввод кириллицы работают без артефактов |
| `T-LIVE-10` | Real GNOME Shell 45–48 | Установка `ucc-gnome`, включение, управление профилем/вентиляторами/клавиатурой/СЖО (в том числе при ручном питании) | `ISSUE-30`, `ISSUE-38` | Реальный GNOME Shell | 0 ошибок `ExtensionError` и `UnknownMethod`, UI не откатывает радиокнопки через 5с |
