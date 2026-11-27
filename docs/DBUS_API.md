# D-Bus API Reference (`uccd`)

Техническая спецификация D-Bus-интерфейса демона `uccd` (реализация `crates/ucc-dbus`, `crates/uccd`).

## 1. Адресация на шине и регистрация

Демон `uccd` работает на системной шине D-Bus (`system` bus). При заданной переменной `UCC_ALLOW_SESSION_BUS=1` и запуске от непривилегированного пользователя (`geteuid() != 0`) допускается fallback на сессионную шину (`session` bus).

| Параметр | Основное значение (primary) | Устаревший псевдоним (legacy alias) |
|---|---|---|
| Bus Name (well-known name) | `com.uniwill.uccd` | `de.uniwill.uccd` |
| Object Path | `/com/uniwill/uccd` | `/de/uniwill/uccd` |
| Interface Name | `com.uniwill.uccd` | `de.uniwill.uccd` |

Регистрация объектов (`ucc_dbus::register_uccd_service`) выполняется до запроса well-known имени, чтобы `Type=dbus` активация systemd немедленно видела готовые endpoints:

- На `/com/uniwill/uccd` зарегистрированы оба интерфейса: `com.uniwill.uccd` (`UccdInterface`) и `de.uniwill.uccd` (`UccdLegacyInterface`).
- На `/de/uniwill/uccd` зарегистрированы оба интерфейса: `com.uniwill.uccd` и `de.uniwill.uccd`.
- Все 7 сигналов транслируются одновременно в оба интерфейса на обоих путях через `DaemonRuntime`.

### Политика шины D-Bus (`uccd/com.uniwill.uccd.conf`)

- Пользователь `root`: разрешено владение именами `com.uniwill.uccd` и `de.uniwill.uccd`, отправка вызовов и приём сообщений.
- Контекст `default`:
  - разрешены `method_call` к `com.uniwill.uccd` и `de.uniwill.uccd` по интерфейсам `com.uniwill.uccd`, `de.uniwill.uccd`, `org.freedesktop.DBus.Introspectable`, `org.freedesktop.DBus.Properties`, `org.freedesktop.DBus.Peer`;
  - разрешён приём `signal` от `com.uniwill.uccd` (интерфейсы `com.uniwill.uccd`, `de.uniwill.uccd`) и от `de.uniwill.uccd` (интерфейс `de.uniwill.uccd`).

---

## 2. Модель авторизации Polkit и ограничения нагрузки

Авторизация всех 123 методов выполняется на стороне демона через `org.freedesktop.PolicyKit1.Authority` (`crates/ucc-dbus/src/auth.rs`, политика `uccd/com.uniwill.uccd.policy`).

### Действия Polkit (`AuthAction`)

| Уровень (`AuthAction`) | Основной Action ID | Legacy Fallback Action ID | `allow_any` | `allow_inactive` | `allow_active` | Флаг `CheckAuthorization` | Таймаут Polkit |
|---|---|---|---|---|---|---|---|
| `Read` | `com.uniwill.uccd.read` | `de.uniwill.uccd.read` | `no` | `no` | `yes` | `0x0` (неинтерактивный) | 5 с |
| `Control` | `com.uniwill.uccd.control` | `de.uniwill.uccd.control` | `no` | `no` | `yes` | `0x1` (`AllowUserInteraction`) | 30 с |
| `ManageHardware` | `com.uniwill.uccd.manage-hardware` | `de.uniwill.uccd.manage-hardware` | `no` | `no` | `auth_admin_keep` | `0x1` (`AllowUserInteraction`) | 30 с |

### Механизм проверки субъекта и эскалации

1. **Идентификация вызывающего**: из заголовка D-Bus извлекается уникальное имя `sender` (например, `:1.42`). Через `org.freedesktop.DBus.GetConnectionUnixUser` определяется UID.
2. **Обход для `root`**: если `uid == 0`, проверка немедленно возвращает `true`.
3. **Субъект Polkit**: используется `system-bus-name` с параметром `{"name": sender}` (защита от race condition PID reuse / CVE-2013-4288).
4. **Legacy fallback**: если `PolicyKit1` возвращает ошибку отсутствующего действия (`Action` / `does not exist` / `NotFound`), выполняется повторная проверка с `de.uniwill.uccd.*`.
5. **Динамическая эскалация для профилей (`Control | ManageHardware*`)**: методы `ApplyProfile`, `AddCustomProfile`, `SaveCustomProfile`, `UpdateCustomProfile` анализируют входной JSON (`profile_requires_manage_hardware`). Если в профиле задан непустой массив `odmPowerLimits` (или `odmPowerLimits.tdpValues`) либо установлено `fan.useControl == false`, требуется `com.uniwill.uccd.manage-hardware` вместо `com.uniwill.uccd.control`.
6. **Лимит размера JSON**: все мутирующие методы, принимающие JSON-строки (отмечены `[256KB]` в таблицах), отклоняют полезную нагрузку размером более `MAX_DBUS_JSON_PAYLOAD_BYTES = 262144` байт (256 KiB).
7. **Ограничения конкурентности Polkit**:
   - Глобальный семафор чтения: `MAX_CONCURRENT_READ_POLKIT = 32` (ожидание пермита до 2 с).
   - Глобальный семафор интерактивных проверок (`Control`/`ManageHardware`): `4` слота (ожидание пермита до 5 с).
   - Лимиты на одного `sender`: `MAX_PER_SENDER_READ_AUTH = 8` активных чтений, `MAX_PER_SENDER_READ_QUEUED = 64` в очереди (таймаут ожидания 100 мс), `MAX_PER_SENDER_INTERACTIVE_AUTH = 2` одновременных интерактивных запроса.

### Изоляция очередей выполнения (`OffloadLane`)

Блокирующие операции записи/управления аппаратурой выносятся в пул потоков с раздельными семафорами по подсистемам, чтобы зависание одной подсистемы не блокировало остальные.

| Lane (`OffloadLane`) | Ёмкость семафора | Назначение |
|---|---|---|
| `Read` | 16 | Фоновые операции чтения / настройки горизонта истории мониторинга |
| `Profile` | 4 | Применение, сохранение, удаление профилей, `StateMap`, пользовательских профилей вентиляторов |
| `Charging` | 4 | Профили, приоритеты, пороги и тип зарядки АКБ |
| `Lighting` | 4 | Подсветка клавиатуры, Lightbar, анимации |
| `PowerGpu` | 4 | Политика питания, устройства питания, NVIDIA cTGP/D0, кривые вентиляторов CPU/dGPU |
| `Cooler` | 4 | Внешняя СЖО (LCT): подключение, вентилятор, помпа, LED, автоконтроль |
| `Display` | 4 | Яркость дисплея, частота обновления, программный переключатель веб-камеры |
| `Misc` | 8 | Fn-Lock, включение/отключение сбора телеметрии |

- Таймаут ожидания слота в очереди (`UCC_OFFLOAD_QUEUE_TIMEOUT_MS`): по умолчанию `500` мс (минимум `10` мс).
- Если ожидание в очереди превысило `25` мс, проверяется живость клиента через `org.freedesktop.DBus.NameHasOwner`; при отключении клиента задача отменяется.
- Большинство методов при отказе Polkit или ошибке очереди возвращают безопасное значение по умолчанию (`false`, `""`, `"{}"`, `[]`, `-1`, `0`). Методы с возвращаемым типом `zbus::fdo::Result<()>` (`SetFnLockStatus`, `SetSensorDataCollectionStatus`, `SetDGpuD0Metrics`, `SetMonitorHistoryHorizon`) возвращают D-Bus ошибку `org.freedesktop.DBus.Error.AccessDenied` / `Failed`.

---

## 3. Методы D-Bus (всего 123)

Обозначения в столбце **Polkit**:
- `Read` = `com.uniwill.uccd.read`
- `Control` = `com.uniwill.uccd.control`
- `ManageHardware` = `com.uniwill.uccd.manage-hardware`
- `Control | ManageHardware*` = `Control`, но эскалируется до `ManageHardware` при заданных `odmPowerLimits` или `fan.useControl == false`.

### 3.1. Система, информация об устройстве и диагностика (10 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 1 | `GetDeviceName` | `() -> (s)` | `() -> String` | `Read` | — | Имя модели устройства (DMI / платформа). |
| 2 | `GetSystemInfoJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON с моделью ноутбука, CPU, dGPU, iGPU (`laptopModel`, `cpuModel`, `dGpuModel`, `iGpuModel`). |
| 3 | `IsDeviceSupported` | `() -> (b)` | `() -> bool` | `Read` | — | Признак поддержки платформы драйвером `tuxedo_io` / Clevo / Uniwill. |
| 4 | `UccdVersion` | `() -> (s)` | `() -> String` | `Read` | — | Строка версии демона `uccd`. |
| 5 | `TuxedoWmiAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Доступность аппаратного бэкенда `tuxedo_io` / WMI. |
| 6 | `FanHwmonAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Доступность управления/чтения вентиляторов (`number_of_fans > 0`). |
| 7 | `GetDriverHealthJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON диагностики драйверов ядра (`/sys/module/tuxedo_io`, `/dev/tuxedo_io`, ITE 8291, Lightbar). |
| 8 | `GetIsX11` | `() -> (b)` | `() -> bool` | `Read` | — | Флаг наличия X11-сессии (`false` на Wayland/headless). |
| 9 | `GetSensorDataCollectionStatus` | `() -> (b)` | `() -> bool` | `Read` | — | Текущий статус фонового сбора телеметрии датчиков. |
| 10 | `SetSensorDataCollectionStatus` | `(b) -> ()` | `(status: bool) -> zbus::fdo::Result<()>` | `Control` | `Misc` | Включает или приостанавливает фоновый сбор телеметрии. |

### 3.2. Дисплей, веб-камера и клавиша Fn-Lock (11 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 11 | `GetDisplayModesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON доступных режимов дисплея (разрешения и частоты обновления). |
| 12 | `GetForceYUV420OutputSwitchAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Доступность переключения принудительного вывода YUV420. |
| 13 | `GetDisplayBrightness` | `() -> (i)` | `() -> i32` | `Read` | — | Текущая яркость экрана в процентах (`0..=100`, `-1` при ошибке). |
| 14 | `SetDisplayBrightness` | `(i) -> (b)` | `(brightness: i32) -> bool` | `Control` | `Display` | Устанавливает яркость экрана (`0..=100`). |
| 15 | `SetDisplayRefreshRate` | `(si) -> (b)` | `(display: &str, refresh_rate: i32) -> bool` | `Control` | `Display` | Устанавливает частоту обновления (Гц) для указанного вывода. |
| 16 | `WebcamSWAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Доступность программного выключателя веб-камеры (EC/WMI). |
| 17 | `GetWebcamSWStatus` | `() -> (b)` | `() -> bool` | `Read` | — | Текущее состояние веб-камеры (`true` — включена). |
| 18 | `SetWebcam` | `(b) -> (b)` | `(enabled: bool) -> bool` | `Control` | `Display` | Включает или отключает веб-камеру через EC-переключатель. |
| 19 | `GetFnLockSupported` | `() -> (b)` | `() -> bool` | `Read` | — | Поддержка управления Fn-Lock на текущей платформе. |
| 20 | `GetFnLockStatus` | `() -> (b)` | `() -> bool` | `Read` | — | Текущее состояние Fn-Lock (`true` — активен). |
| 21 | `SetFnLockStatus` | `(b) -> ()` | `(status: bool) -> zbus::fdo::Result<()>` | `Control` | `Misc` | Включает или отключает Fn-Lock. |

### 3.3. CPU, GPU, телеметрия и история мониторинга (23 метода)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 22 | `GetFanDataCPU` | `() -> (a{sv})` | `() -> HashMap<String, OwnedValue>` | `Read` | — | Словарь `{"temp": {"timestamp": i64, "data": i32}, "speed": {"timestamp": i64, "data": i32}}` для вентилятора CPU (индекс 0). |
| 23 | `GetFanDataGPU1` | `() -> (a{sv})` | `() -> HashMap<String, OwnedValue>` | `Read` | — | Данные температуры и скорости вентилятора GPU1 (индекс 1). |
| 24 | `GetFanDataGPU2` | `() -> (a{sv})` | `() -> HashMap<String, OwnedValue>` | `Read` | — | Данные температуры и скорости вентилятора GPU2 (индекс 2). |
| 25 | `GetDGpuInfoValuesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON телеметрии дискретного GPU (температура, частоты, мощность, утилизация, VRAM, P-state). |
| 26 | `GetIGpuInfoValuesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON телеметрии встроенного GPU (температура, частоты, мощность). |
| 27 | `GetCpuPowerValuesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON энергопотребления CPU (`powerDraw`, опционально `maxPowerLimit`). |
| 28 | `GetPrimeState` | `() -> (s)` | `() -> String` | `Read` | — | Текущий режим NVIDIA PRIME / гибридной графики. |
| 29 | `GetCpuFrequencyLimitsJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON пределов частоты CPU (`min`, `max` в кГц). |
| 30 | `GetAvailableGovernors` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив доступных scaling governors ядра. |
| 31 | `GetAvailableEPPs` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив доступных значений Energy Performance Preference (EPP). |
| 32 | `GetCpuCoreCount` | `() -> (i)` | `() -> i32` | `Read` | — | Количество логических ядер CPU. |
| 33 | `GetCpuTopologyJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON топологии ядер CPU (физические ядра, потоки, кластеры). |
| 34 | `GetCpuFrequencyMHz` | `() -> (i)` | `() -> i32` | `Read` | — | Текущая усреднённая/актуальная частота CPU в МГц. |
| 35 | `SetDGpuD0Metrics` | `(b) -> ()` | `(status: bool) -> zbus::fdo::Result<()>` | `Control` | `PowerGpu` | Разрешает или запрещает пробуждение спящего dGPU (D3cold -> D0) для опроса метрик. |
| 36 | `GetNVIDIAPowerCTRLAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Доступность управления NVIDIA cTGP / Power Limit. |
| 37 | `GetNVIDIAPowerCTRLDefaultPowerLimit` | `() -> (i)` | `() -> i32` | `Read` | — | Базовый (default) лимит мощности NVIDIA dGPU в Вт. |
| 38 | `GetNVIDIAPowerCTRLMaxPowerLimit` | `() -> (i)` | `() -> i32` | `Read` | — | Максимальный лимит мощности NVIDIA dGPU в Вт. |
| 39 | `SetNVIDIAPowerOffset` | `(i) -> (b)` | `(offset: i32) -> bool` | `Control` | `PowerGpu` | Устанавливает смещение NVIDIA cTGP (Вт). |
| 40 | `GetCTGPAdjustmentSupported` | `() -> (b)` | `() -> bool` | `Read` | — | Аппаратная поддержка регулировки cTGP. |
| 41 | `GetHwpDynamicBoostSupported` | `() -> (b)` | `() -> bool` | `Read` | — | Поддержка Intel HWP Dynamic Boost. |
| 42 | `GetMonitorDataSince` | `(x) -> (ay)` | `(since_timestamp_ms: i64) -> Vec<u8>` | `Read` | — | Бинарная выгрузка 10 временных рядов телеметрии с момента `since_timestamp_ms` (Unix мс). |
| 43 | `SetMonitorHistoryHorizon` | `(i) -> ()` | `(seconds: i32) -> zbus::fdo::Result<()>` | `Control` | `Read` | Устанавливает горизонт хранения истории мониторинга в секундах. |
| 44 | `GetMonitorHistoryHorizon` | `() -> (i)` | `() -> i32` | `Read` | — | Возвращает текущий горизонт хранения истории мониторинга в секундах (по умолчанию `60`). |


### 3.4. Профили производительности, настройки и StateMap (18 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 45 | `GetActiveProfileJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON текущего активного профиля (`UccProfile`). |
| 46 | `GetProfilesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив всех доступных профилей (встроенных и пользовательских). |
| 47 | `GetDefaultProfilesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив встроенных профилей. |
| 48 | `GetCustomProfilesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив пользовательских профилей. |
| 49 | `GetDefaultValuesProfileJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON шаблонного профиля со значениями по умолчанию для текущей платформы. |
| 50 | `SetActiveProfile` | `(s) -> (b)` | `(id: &str) -> bool` | `Control` | `Profile` | Активирует профиль по его `id` (с обновлением конфигурации и аппаратного состояния). |
| 51 | `SetTempProfile` | `(s) -> (b)` | `(profile_name: &str) -> bool` | `Control` | `Profile` | Временно активирует профиль по имени/ID без изменения постоянной привязки. |
| 52 | `ApplyProfile` | `(s) -> (b)` | `(profile_json: &str) -> bool` | `Control \| ManageHardware*` | `Profile` `[256KB]` | Непосредственно применяет переданный JSON-профиль к оборудованию. |
| 53 | `AddCustomProfile` | `(s) -> (b)` | `(profile_json: &str) -> bool` | `Control \| ManageHardware*` | `Profile` `[256KB]` | Добавляет новый пользовательский профиль в хранилище настроек. |
| 54 | `SaveCustomProfile` | `(s) -> (b)` | `(profile_json: &str) -> bool` | `Control \| ManageHardware*` | `Profile` `[256KB]` | Создаёт или перезаписывает пользовательский профиль по `id`. |
| 55 | `UpdateCustomProfile` | `(s) -> (b)` | `(profile_json: &str) -> bool` | `Control \| ManageHardware*` | `Profile` `[256KB]` | Обновляет существующий пользовательский профиль. |
| 56 | `DeleteCustomProfile` | `(s) -> (b)` | `(profile_id: &str) -> bool` | `Control` | `Profile` | Удаляет пользовательский профиль по `profile_id`. |
| 57 | `GetSettingsJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON глобальных настроек (`stateMap`, `cpuSettingsEnabled`, `fanControlEnabled`, `fahrenheit`, `chargingProfile`, и др.). |
| 58 | `SetStateMap` | `(ss) -> (b)` | `(state: &str, profile_id: &str) -> bool` | `Control` | `Profile` | Привязывает состояние питания (`power_ac`, `power_bat`, `power_type_c`, `power_wc`) к `profile_id`. |
| 59 | `SetBatchStateMap` | `(s) -> (b)` | `(state_map_json: &str) -> bool` | `Control` | `Profile` `[256KB]` | Атомарно обновляет несколько привязок `stateMap` из JSON-объекта. |
| 60 | `ODMProfilesAvailable` | `() -> (as)` | `() -> Vec<String>` | `Read` | — | Список имён аппаратных ODM-профилей (например, `quiet`, `balanced`, `enthusiast`). |
| 61 | `ODMPowerLimitsJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON текущих и допустимых ODM-лимитов мощности (PL1, PL2, PL4). |
| 62 | `ConsumeModeReapplyPending` | `() -> (b)` | `() -> bool` | `Control` | — | Атомарно считывает и сбрасывает флаг необходимости повторного применения режима; при сбросе `true -> false` шлёт `mode_reapply_pending_changed(false)`. |

### 3.5. Политика питания и устройства питания (9 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 63 | `GetPowerState` | `() -> (s)` | `() -> String` | `Read` | — | Текущее состояние питания (`power_ac`, `power_bat`, `power_type_c`, `power_wc`). |
| 64 | `GetPowerPolicyJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON текущего решения политики питания (`real_power_source`, `current_mode`, `manual_mode`, `profile_state`, `registered_apps`, `revision`). |
| 65 | `SetManualPowerMode` | `(s) -> (b)` | `(mode_val: &str) -> bool` | `Control` | `PowerGpu` | Устанавливает ручной режим питания (`auto`, `ac`, `type_c`, `bat`). |
| 66 | `RegisterPowerRequest` | `(sss) -> (b)` | `(name: &str, reason: &str, mode_val: &str) -> bool` | `Control` | `PowerGpu` | Регистрирует запрос приложения на удержание режима питания (привязан к `sender`, автоматически снимается при отключении клиента). |
| 67 | `UnregisterPowerRequest` | `(s) -> (b)` | `(name: &str) -> bool` | `Control` | `PowerGpu` | Снимает ранее зарегистрированный запрос питания приложения по `name`. |
| 68 | `GetPowerDeviceSettingsJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON настроек управления питанием периферийных устройств (PCI/USB/крышка/NVIDIA wake). |
| 69 | `GetPowerDeviceStatusJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON актуального статуса обнаруженных и управляемых устройств питания. |
| 70 | `SetPowerDeviceSettingsJSON` | `(s) -> (b)` | `(json: &str) -> bool` | `Control` | `PowerGpu` `[256KB]` | Валидирует, сохраняет и применяет настройки `powerDevices`. |
| 71 | `ScanPowerDevices` | `() -> (b)` | `() -> bool` | `Control` | `PowerGpu` | Запускает повторное сканирование sysfs-устройств питания и обновляет статус. |


### 3.6. Профили и кривые вентиляторов (12 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 72 | `GetFanProfileNames` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив встроенных профилей вентиляторов. |
| 73 | `GetFanProfile` | `(s) -> (s)` | `(name: &str) -> String` | `Read` | — | JSON таблиц кривых (`tableCPU`, `tableGPU`, `tablePump`, `tableWaterCoolerFan`) для указанного профиля вентиляторов. |
| 74 | `SetFanProfile` | `(ss) -> (b)` | `(name: &str, json: &str) -> bool` | `Control` | `Profile` `[256KB]` | Устанавливает/обновляет профиль вентиляторов по имени. |
| 75 | `SetFanProfileCPU` | `(s) -> (b)` | `(points_json: &str) -> bool` | `Control` | `PowerGpu` `[256KB]` | Задаёт кривую вентилятора CPU (JSON-массив точек `{temp, speed}`). |
| 76 | `SetFanProfileDGPU` | `(s) -> (b)` | `(points_json: &str) -> bool` | `Control` | `PowerGpu` `[256KB]` | Задаёт кривую вентилятора dGPU (JSON-массив точек `{temp, speed}`). |
| 77 | `ApplyFanProfiles` | `(s) -> (b)` | `(fan_profiles_json: &str) -> bool` | `Control` | `PowerGpu` `[256KB]` | Применяет набор кривых (`cpu`, `gpu`, `pump`, `waterCoolerFan`) к контроллеру вентиляторов. |
| 78 | `RevertFanProfiles` | `() -> (b)` | `() -> bool` | `Control` | `PowerGpu` | Возвращает кривые вентиляторов к настройкам активного профиля / автоматическому управлению. |
| 79 | `GetCustomFanProfilesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив пользовательских профилей вентиляторов. |
| 80 | `SaveCustomFanProfile` | `(sss) -> (b)` | `(id: &str, name: &str, json: &str) -> bool` | `Control` | `Profile` `[256KB]` | Сохраняет пользовательский профиль вентиляторов. |
| 81 | `DeleteCustomFanProfile` | `(s) -> (b)` | `(id: &str) -> bool` | `Control` | `Profile` | Удаляет пользовательский профиль вентиляторов по `id`. |
| 82 | `GetFansMinSpeed` | `() -> (i)` | `() -> i32` | `Read` | — | Минимально допустимая скорость вентиляторов (в процентах). |
| 83 | `GetFansOffAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Поддерживается ли полная остановка вентиляторов (0%). |

### 3.7. Подсветка клавиатуры и Lightbar (11 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 84 | `GetKeyboardBacklightControlSupported` | `() -> (b)` | `() -> bool` | `Read` | — | Флаг наличия поддерживаемой подсветки клавиатуры. |
| 85 | `GetKeyboardBacklightCapabilitiesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON возможностей подсветки (`modes`, `zones`, `maxBrightness`, `maxRed`, `maxGreen`, `maxBlue`, `ledIds`) или `"null"`. |
| 86 | `GetKeyboardBacklightStatesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив текущих состояний зон подсветки (`[{brightness, red, green, blue}, ...]`). |
| 87 | `SetKeyboardBacklightStatesJSON` | `(s) -> (b)` | `(keyboard_backlight_states_json: &str) -> bool` | `Control` | `Lighting` `[256KB]` | Применяет состояние подсветки клавиатуры по зонам (поддерживает массив зон или объект `{brightness, states}`). |
| 88 | `GetCustomKeyboardProfilesJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив пользовательских профилей подсветки клавиатуры. |
| 89 | `SaveCustomKeyboardProfile` | `(sss) -> (b)` | `(id: &str, name: &str, json: &str) -> bool` | `Control` | `Lighting` `[256KB]` | Сохраняет пользовательский профиль подсветки клавиатуры. |
| 90 | `DeleteCustomKeyboardProfile` | `(s) -> (b)` | `(id: &str) -> bool` | `Control` | `Lighting` | Удаляет пользовательский профиль подсветки клавиатуры по `id`. |
| 91 | `GetLightbarSettingsJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON конфигурации передней полосы Lightbar (`managed`, `silent_mode`, `welcome_anim`, `power_settings`). |
| 92 | `GetLightbarStatusJSON` | `() -> (s)` | `() -> String` | `Read` | — | JSON текущего состояния Lightbar (доступность, активная анимация, базовый цвет). |
| 93 | `SetLightbarSettingsJSON` | `(s) -> (b)` | `(json: &str) -> bool` | `Control` | `Lighting` `[256KB]` | Нормализует, сохраняет и применяет настройки Lightbar; эмитит `lightbar_settings_changed`. |
| 94 | `TriggerLightbarAnimation` | `(b) -> (b)` | `(force: bool) -> bool` | `Control` | `Lighting` | Запускает анимацию Lightbar (`force=true` — тестовый прогон даже при `silent_mode`; `force=false` — уведомление с учётом политики). |

### 3.8. Зарядка аккумулятора (14 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 95 | `GetChargingProfilesAvailable` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив доступных профилей зарядки (например, `["high_capacity", "balanced", "stationary"]`). |
| 96 | `GetCurrentChargingProfile` | `() -> (s)` | `() -> String` | `Read` | — | Дескриптор активного профиля зарядки. |
| 97 | `SetChargingProfile` | `(s) -> (b)` | `(profile_descriptor: &str) -> bool` | `Control` | `Charging` | Устанавливает профиль зарядки АКБ. |
| 98 | `GetChargingPrioritiesAvailable` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив доступных приоритетов зарядки. |
| 99 | `GetCurrentChargingPriority` | `() -> (s)` | `() -> String` | `Read` | — | Дескриптор текущего приоритета зарядки. |
| 100 | `SetChargingPriority` | `(s) -> (b)` | `(priority_descriptor: &str) -> bool` | `Control` | `Charging` | Устанавливает приоритет зарядки АКБ. |
| 101 | `GetChargeStartAvailableThresholds` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив допустимых порогов начала зарядки (%). |
| 102 | `GetChargeEndAvailableThresholds` | `() -> (s)` | `() -> String` | `Read` | — | JSON-массив допустимых порогов окончания зарядки (%). |
| 103 | `GetChargeStartThreshold` | `() -> (i)` | `() -> i32` | `Read` | — | Текущий порог начала зарядки (%, `-1` если недоступно). |
| 104 | `GetChargeEndThreshold` | `() -> (i)` | `() -> i32` | `Read` | — | Текущий порог окончания зарядки (%, `-1` если недоступно). |
| 105 | `SetChargeStartThreshold` | `(i) -> (b)` | `(value: i32) -> bool` | `ManageHardware` | `Charging` | Устанавливает порог начала зарядки (%). Требует `manage-hardware`. |
| 106 | `SetChargeEndThreshold` | `(i) -> (b)` | `(value: i32) -> bool` | `ManageHardware` | `Charging` | Устанавливает порог окончания зарядки (%). Требует `manage-hardware`. |
| 107 | `GetChargeType` | `() -> (s)` | `() -> String` | `Read` | — | Текущий sysfs `charge_type` аккумулятора. |
| 108 | `SetChargeType` | `(s) -> (b)` | `(type_val: &str) -> bool` | `ManageHardware` | `Charging` | Устанавливает sysfs `charge_type`. Требует `manage-hardware`. |

### 3.9. Система жидкостного охлаждения / Water Cooler LCT (15 методов)

| # | Метод | Сигнатура D-Bus | Rust-аргументы -> возврат | Polkit | Lane | Описание |
|---|---|---|---|---|---|---|
| 109 | `GetWaterCoolerSupported` | `() -> (b)` | `() -> bool` | `Read` | — | Признак аппаратной поддержки LCT СЖО. |
| 110 | `GetWaterCoolerAvailable` | `() -> (b)` | `() -> bool` | `Read` | — | Признак доступности BLE-транспорта / контроллера СЖО. |
| 111 | `GetWaterCoolerConnected` | `() -> (b)` | `() -> bool` | `Read` | — | Текущий статус BLE-подключения к внешнему блоку СЖО. |
| 112 | `IsWaterCoolerEnabled` | `() -> (b)` | `() -> bool` | `Read` | — | Включён ли режим работы с СЖО (автопоиск/удержание соединения). |
| 113 | `EnableWaterCooler` | `(b) -> (b)` | `(enable: bool) -> bool` | `Control` | `Cooler` | Включает или отключает подключение к СЖО. |
| 114 | `GetWaterCoolerFanSpeed` | `() -> (i)` | `() -> i32` | `Read` | — | Текущая скорость вентилятора СЖО (%, `-1` если не подключено). |
| 115 | `SetWaterCoolerFanSpeed` | `(i) -> (b)` | `(duty_cycle_percent: i32) -> bool` | `Control` | `Cooler` | Задаёт скорость вентилятора СЖО (`0..=100`). |
| 116 | `TurnOffWaterCoolerFan` | `() -> (b)` | `() -> bool` | `Control` | `Cooler` | Останавливает вентилятор СЖО. |
| 117 | `GetWaterCoolerPumpLevel` | `() -> (i)` | `() -> i32` | `Read` | — | Текущий код уровня/напряжения помпы СЖО. |
| 118 | `SetWaterCoolerPumpVoltage` | `(i) -> (b)` | `(voltage: i32) -> bool` | `Control` | `Cooler` | Устанавливает код напряжения помпы (`0`=11V High, `2`=7V Low, `3`=8V Med, `4`=Off; код `1`=12V отклоняется). |
| 119 | `TurnOffWaterCoolerPump` | `() -> (b)` | `() -> bool` | `Control` | `Cooler` | Останавливает помпу СЖО. |
| 120 | `SetWaterCoolerLEDColor` | `(iiii) -> (b)` | `(red: i32, green: i32, blue: i32, mode_val: i32) -> bool` | `Control` | `Cooler` | Задаёт RGB-цвет (`0..=255`) и режим подсветки блока СЖО (`0`=static, `1`=breathing и т.д.). |
| 121 | `TurnOffWaterCoolerLED` | `() -> (b)` | `() -> bool` | `Control` | `Cooler` | Отключает LED-подсветку блока СЖО. |
| 122 | `IsWaterCoolerAutoControlEnabled` | `() -> (b)` | `() -> bool` | `Read` | — | Включено ли автоматическое управление помпой и вентилятором СЖО по кривым. |
| 123 | `SetWaterCoolerAutoControl` | `(b) -> (b)` | `(enabled: bool) -> bool` | `Control` | `Cooler` | Включает или отключает автоматическое управление СЖО. |

---

## 4. Сигналы D-Bus (всего 7)

Все сигналы публикуются на обоих интерфейсах (`com.uniwill.uccd` и `de.uniwill.uccd`) и на обоих путях (`/com/uniwill/uccd` и `/de/uniwill/uccd`).

| # | Сигнал | Сигнатура D-Bus | Аргументы | Условие генерации |
|---|---|---|---|---|
| 1 | `profile_changed` | `(sss)` | `profile_id: &str`, `keyboard_profile_id: &str`, `fan_profile_id: &str` | Смена активного профиля (`SetActiveProfile`, `ApplyProfile`, смена источника питания по `stateMap`), а также обновление связанного профиля вентиляторов или подсветки клавиатуры. |
| 2 | `mode_reapply_pending_changed` | `(b)` | `pending: bool` | Изменение флага отложенного повторного применения режима (`true` при обнаружении рассинхронизации, `false` после вызова `ConsumeModeReapplyPending`). |
| 3 | `power_state_changed` | `(s)` | `state: &str` | Изменение активного состояния питания (`power_ac`, `power_bat`, `power_type_c`, `power_wc`). |
| 4 | `power_policy_changed` | `(s)` | `json: &str` | Изменение решения политики питания (смена источника питания, `SetManualPowerMode`, `RegisterPowerRequest`/`UnregisterPowerRequest`, подключение/отключение СЖО). Payload совпадает с `GetPowerPolicyJSON`. |
| 5 | `power_device_status_changed` | `(s)` | `json: &str` | Обновление статуса периферийных устройств питания после сканирования (`ScanPowerDevices`), смены настроек (`SetPowerDeviceSettingsJSON`) или смены режима питания. Payload совпадает с `GetPowerDeviceStatusJSON`. |
| 6 | `lightbar_settings_changed` | `(s)` | `json: &str` | Сохранение новых настроек Lightbar через `SetLightbarSettingsJSON`. Payload совпадает с `GetLightbarSettingsJSON`. |
| 7 | `water_cooler_status_changed` | `(s)` | `status: &str` | Изменение состояния подключения, питания, вентилятора, помпы или автоконтроля СЖО LCT. |

---

## 5. Основные схемы данных

### 5.1. `ActiveProfile` (`GetActiveProfileJSON`, `ApplyProfile`, `SaveCustomProfile`)

Сериализация использует `camelCase` (`crates/ucc-core/src/profile.rs`):

```json
{
  "id": "frixit-ac",
  "name": "Performance",
  "description": "AC performance profile",
  "display": {
    "brightness": 100,
    "useBrightness": false,
    "refreshRate": 165,
    "useRefRate": true,
    "xResolution": -1,
    "yResolution": -1,
    "useResolution": false
  },
  "cpu": {
    "onlineCores": 16,
    "useMaxPerfGov": false,
    "scalingMinFrequency": 400000,
    "scalingMaxFrequency": 5100000,
    "governor": "powersave",
    "energyPerformancePreference": "balance_performance",
    "noTurbo": false
  },
  "webcam": {
    "status": true,
    "useStatus": false
  },
  "fan": {
    "useControl": true,
    "fanProfile": "Balanced",
    "sameSpeed": true,
    "autoControlWC": true
  },
  "keyboard": {
    "keyboardProfileName": "",
    "keyboardProfileId": "",
    "keyboardProfileData": ""
  },
  "odmProfile": { "name": "enthusiast" },
  "odmPowerLimits": { "tdpValues": [45, 65, 90] },
  "nvidiaCTGPOffset": 15,
  "selectedKeyboardProfile": "default"
}
```

Примечание: `odmPowerLimits` при десериализации принимает как объект `{"tdpValues": [PL1, PL2, PL4]}`, так и плоский массив `[PL1, PL2, PL4]`.

### 5.2. `PowerPolicy` (`GetPowerPolicyJSON`, `power_policy_changed`)

```json
{
  "real_power_source": "ac",
  "current_mode": "ac",
  "manual_mode": "auto",
  "profile_state": "power_ac",
  "water_cooler_connected": false,
  "registered_apps": [
    {
      "owner": ":1.84",
      "name": "blender",
      "reason": "render",
      "force_mode": "ac"
    }
  ],
  "revision": 4
}
```

Допустимые значения:
- `real_power_source`, `current_mode`: `"ac"`, `"type_c"`, `"bat"`.
- `manual_mode`: `"auto"`, `"ac"`, `"type_c"`, `"bat"`.
- `profile_state`: `"power_ac"`, `"power_type_c"`, `"power_bat"`, `"power_wc"`.

### 5.3. `FanData` (`GetFanDataCPU`, `GetFanDataGPU1`, `GetFanDataGPU2`)

Тип D-Bus: `a{sv}` (`HashMap<String, OwnedValue>`). Каждый ключ содержит вложенный словарь `a{sv}` с меткой времени (`i64`, Unix мс) и значением (`i32`, `-1` при недоступности):

```text
{
  "temp":  { "timestamp": <i64 ms>, "data": <i32 °C> },
  "speed": { "timestamp": <i64 ms>, "data": <i32 %> }
}
```

### 5.4. `DGpuInfo` и `IGpuInfo` (`GetDGpuInfoValuesJSON`, `GetIGpuInfoValuesJSON`)

Для отсутствующих сенсоров на границе IPC возвращаются сентинелы `-1` / `-1.0` (для смещений частот `-999`, для строковых полей `""`, для `vendor` — `"unknown"`).

`GetDGpuInfoValuesJSON`:

```json
{
  "vendor": "nvidia",
  "isSleeping": false,
  "d0MetricsUsage": true,
  "temp": 52.0,
  "coreFrequency": 1410.0,
  "maxCoreFrequency": 2100.0,
  "vramFrequency": 7000.0,
  "powerDraw": 34.5,
  "maxPowerLimit": 140.0,
  "enforcedPowerLimit": 115.0,
  "computeUtilPct": 28,
  "memoryUtilPct": 14,
  "vramUsedMiB": 1536,
  "vramTotalMiB": 8192,
  "perfLimitReason": "None",
  "encoderUtilPct": 0,
  "decoderUtilPct": 0,
  "currentPstate": 0,
  "grClockOffsetMHz": 0,
  "memClockOffsetMHz": 0,
  "coreVoltageMv": 825
}
```

`GetIGpuInfoValuesJSON`:

```json
{
  "vendor": "amd",
  "temp": 46.0,
  "coreFrequency": 800.0,
  "maxCoreFrequency": 2200.0,
  "powerDraw": 6.2
}
```

### 5.5. `KeyboardBacklightStates` и `Capabilities`

`GetKeyboardBacklightCapabilitiesJSON`:

```json
{
  "modes": [0],
  "zones": 4,
  "maxBrightness": 255,
  "maxRed": 255,
  "maxGreen": 255,
  "maxBlue": 255,
  "ledIds": [0, 1, 2, 3]
}
```

`GetKeyboardBacklightStatesJSON` (массив длины `zones`):

```json
[
  { "brightness": 128, "red": 255, "green": 128, "blue": 0 },
  { "brightness": 128, "red": 255, "green": 128, "blue": 0 }
]
```

### 5.6. `LightbarSettings` (`GetLightbarSettingsJSON`, `SetLightbarSettingsJSON`)

```json
{
  "managed": true,
  "silent_mode": false,
  "welcome_anim": false,
  "power_settings": {
    "ac": {
      "enabled": true,
      "brightness_val": 255,
      "mode": "Pulse (stays on)",
      "color_mode": "Auto"
    },
    "type_c": {
      "enabled": true,
      "brightness_val": 150,
      "mode": "Pulse (stays on)",
      "color_mode": [0, 128, 255]
    },
    "bat": {
      "enabled": false,
      "brightness_val": 50,
      "mode": "Flash (stays off)",
      "color_mode": "Auto"
    }
  }
}
```

Допустимые значения `mode`: `"Pulse (stays on)"`, `"Flash (stays off)"`, `"Rainbow"`, `"Police Strobe"`, `"Disco"`.
Допустимые значения `color_mode`: `"Auto"` либо массив `[r, g, b]` (`0..=255`).

### 5.7. `PowerDeviceSettings` (`GetPowerDeviceSettingsJSON`, `SetPowerDeviceSettingsJSON`)

```json
{
  "enabled": false,
  "keepNvidiaAwake": false,
  "cpu": {},
  "devices": [],
  "lid": {
    "wakeOnlyLidAndKeyboard": true
  }
}
```

### 5.8. `DriverHealth` (`GetDriverHealthJSON`)

```json
{
  "healthy": true,
  "kernel": "6.12.10-arch1-1",
  "issues": [],
  "message": ""
}
```

При наличии проблем `healthy` равен `false`, `issues` содержит массив описаний, `message` формируется в виде многострочной сводки с указанием версии ядра.

### 5.9. Бинарный формат `GetMonitorDataSince`

Метод `GetMonitorDataSince(since_timestamp_ms: i64) -> Vec<u8>` возвращает последовательность блоков в **native-endian** порядке байт (`crates/ucc-hardware/src/telemetry/history.rs`). Пустые серии опускаются.

Структура одного блока серии:

| Смещение | Размер | Тип | Поле |
|---|---|---|---|
| `0` | 1 байт | `u8` | `metric_id` (`0..=9`) |
| `1` | 4 байта | `u32` (native-endian) | `count` (число точек в серии) |
| `5 + k*16` | 8 байт | `i64` (native-endian) | `timestamp_ms` (Unix timestamp в миллисекундах) |
| `13 + k*16` | 8 байт | `f64` (native-endian) | `value` (значение метрики) |

Таблица идентификаторов `MetricId`:

| `metric_id` | Имя (`MetricId::name`) | Метрика |
|---|---|---|
| `0` | `cpuTemp` | Температура CPU (°C) |
| `1` | `cpuFanDuty` | Заполнение вентилятора CPU (%) |
| `2` | `cpuPower` | Потребление CPU (Вт) |
| `3` | `cpuFrequency` | Частота CPU (МГц) |
| `4` | `gpuTemp` | Температура dGPU (°C) |
| `5` | `gpuFanDuty` | Заполнение вентилятора dGPU (%) |
| `6` | `gpuPower` | Потребление dGPU (Вт) |
| `7` | `gpuFrequency` | Частота ядра dGPU (МГц) |
| `8` | `gpuVramFrequency` | Частота VRAM dGPU (МГц) |
| `9` | `gpuCoreVoltage` | Напряжение ядра dGPU (мВ) |

Ограничения буфера и fail-closed декодера (`decode_history_binary`):
- В памяти удерживается не более `MAX_POINTS_PER_SERIES = 10_000` точек на серию.
- Декодер отклоняет буфер при `count > 1_000_000` (`MAX_DECODE_POINTS_PER_SERIES`), суммарном числе точек `> 1_000_000` (`MAX_DECODE_TOTAL_POINTS`), дублирующемся `metric_id`, неизвестном `metric_id > 9` или усечённых байтах.

---

## 6. Примеры инспекции через `busctl` и `gdbus`

### Интроспекция и проверка доступности сервиса

```bash
busctl status com.uniwill.uccd
busctl introspect com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd
busctl introspect de.uniwill.uccd /de/uniwill/uccd de.uniwill.uccd
```

### Вызовы методов через `busctl`

```bash
# Версия демона и здоровье драйверов
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd UccdVersion
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd GetDriverHealthJSON

# Активный профиль и политика питания
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd GetActiveProfileJSON
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd GetPowerPolicyJSON

# Активация профиля (требует com.uniwill.uccd.control)
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd SetActiveProfile s "__balanced__"

# Установка порога окончания зарядки (требует com.uniwill.uccd.manage-hardware)
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd SetChargeEndThreshold i 80

# Бинарная телеметрия за последнюю минуту (since_timestamp_ms = 0 -> вся история)
busctl call com.uniwill.uccd /com/uniwill/uccd com.uniwill.uccd GetMonitorDataSince x 0
```

### Вызовы методов и мониторинг сигналов через `gdbus`

```bash
# Интроспекция XML
gdbus introspect --system --dest com.uniwill.uccd --object-path /com/uniwill/uccd

# Вызов метода
gdbus call --system \
  --dest com.uniwill.uccd \
  --object-path /com/uniwill/uccd \
  --method com.uniwill.uccd.GetDGpuInfoValuesJSON

# Подписка на все сигналы демона
gdbus monitor --system \
  --dest com.uniwill.uccd \
  --object-path /com/uniwill/uccd
```
