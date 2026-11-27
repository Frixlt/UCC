# Аппаратная матрица `ucc-rusted` (M13.11)

Документ фиксирует поддерживаемые платформы, EC-интерфейсы, `ioctl`-команды, `sysfs`/`procfs`-узлы, протоколы периферии и аппаратные ограничения, реализованные в `ucc-hardware`, `ucc-lighting`, `ucc-core` и `uccd`.

---

## 1. Платформы и EC-бэкенды (`PlatformBackend`)

### 1.1. Сравнительная матрица EC-бэкендов

| Параметр | Uniwill (`TuxedoIoBackend`) | Clevo (`TuxedoIoBackend`) | Windows WMI (`WindowsWmiEcBackend`) | Mock / Unsupported |
|---|---|---|---|---|
| `PlatformBackendKind` | `TuxedoIo` (`DeviceVendor::Uniwill`) | `TuxedoIo` (`DeviceVendor::Clevo`) | `WindowsWmi` (`DeviceVendor::Uniwill`) | `Mock` / `Unsupported` |
| Транспорт | `/dev/tuxedo_io` (`ioctl`, 64-bit pointer-size ABI = 8 байт) | `/dev/tuxedo_io` (`ioctl`, 64-bit pointer-size ABI = 8 байт) | `WmiEcTransport` (`root\WMI` `AcpiTest_MULong` через `wmiacpi.sys`) | In-memory `MockPlatformBackend` / no-op |
| Кол-во вентиляторов (`fan_idx`) | `2` (`0` = CPU, `1` = GPU) | `3` (`0..=2`, упаковка 3 байт в один `i32`) | `2` (`0` = CPU, `1` = GPU) | Настраивается / `0` |
| Шкала скважности (raw <-> %) | `0..=200` (`raw = round(200 * pct / 100)`, чтение `ceil(raw * 100 / 200)`) | `0..=255` (`raw = round(255 * pct / 100)`, чтение `round(raw * 100 / 255)`) | `0..=200` (`ec_duty = (pct * 200) / 100`, чтение `(raw * 100) / 200`) | `0..=100` |
| Возврат в авто-режим | `W_UW_FANAUTO` (`_IO`) | `W_CL_FANAUTO` с битовой маской `0x0F` (`bit 0..3`) | Запись `0` в EC-регистр `0x0751` (`FAN_MANUAL_MODE`) | Запись в журнал |
| Температуры вентиляторов | `R_UW_FAN_TEMP` / `R_UW_FAN_TEMP2` (`0..=150 °C`) | Из битов `R_CL_FANINFO1..3` | EC `0x043E` (CPU), `0x044F` (GPU); `0` -> `SensorUnavailable` | Настраивается |
| Лимиты скорости вентиляторов | `R_UW_FANS_MIN_SPEED`, `R_UW_FANS_OFF_AVAILABLE` | Через `FanControlLogic` | `WmiPlatformLimits` (default: `fans_min_speed = 20`, `fans_off_available = true`) | Настраивается |
| TDP-каналы (`0..=2`: PL1, PL2, PL4) | `R_UW_TDP0..2`, `R_UW_TDP0..2_MIN/MAX`, `W_UW_TDP0..2` (канал `>= 3` отклоняется fail-closed) | Не поддерживаются (`tdp_channels = 0`) | EC `0x0740` (PL1), `0x0741` (PL2), `0x0742` (PL4); default bounds `[(15,90), (20,120), (25,140)]` | Настраивается / нет |
| ODM-профили производительности | `1` = `power_save`/`powersave`/`quiet`; `2` = `enthusiast`/`balanced`/`office`; `3` = `overboost`/`performance`/`high_performance` (при `R_UW_PROFS_AVAILABLE == 2` код зажимается до `<= 1` в `profile_tx.rs`) | `0` = `quiet`/`silent`; `1` = `power_saving`/`powersave`; `2` = `performance`/`balanced`; `3` = `entertainment`/`overboost` | EC `0x0752` (`PERF_MODE`), диапазон `1..=perf_profiles_available` (default `3`) | Настраивается / нет |
| Переключатель веб-камеры | Нет ioctl в `tuxedo_io` (для `M14` `uwcc` — USB-отключение UVC) | `R_CL_WEBCAM_SW` / `W_CL_WEBCAM_SW` | Не реализован (`Unsupported`) | Опционально |
| Бренды DMI (`LaptopManufacturer`) | `TUXEDO`, `XMG`, `PCSpecialist`, `Uniwill`, `MECHREVO` (36 моделей `UniwillDeviceID`, в т.ч. `IBP14/17`, `Pulse`, `Aura`, `Polaris`, `Stellaris`, `Gemini`, `Sirius`, `XNE16E25/A25`, `PCS_X6FR559Y`) | Clevo OEM | Uniwill `INOU0000` | — |

### 1.2. Полная таблица `ioctl`-команд `/dev/tuxedo_io`

Минимальная версия API модуля ядра: `MOD_API_MIN_VERSION = "0.2.6"`. На 64-битном Linux все `_IOR`/`_IOW`-команды кодируют размер указателя `8` байт (`_ior_ptr` / `_iow_ptr`), строго сохраняя ABI `uccd/3rdparty/tuxedo_io_lib/tuxedo_io_ioctl.h`.

| Константа | Magic | Номер (`nr`) | Направление / макрос | Аргумент / назначение |
|---|---|---|---|---|
| `R_MOD_VERSION` | `0xEC` (`IOCTL_MAGIC`) | `0x00` | `_IOR(..., char*)` | Строка версии драйвера (буфер `20` байт) |
| `R_HWCHECK_CL` | `0xEC` (`IOCTL_MAGIC`) | `0x05` | `_IOR(..., int32_t*)` | Проверка наличия Clevo-интерфейса |
| `R_HWCHECK_UW` | `0xEC` (`IOCTL_MAGIC`) | `0x06` | `_IOR(..., int32_t*)` | Проверка наличия Uniwill-интерфейса |
| `R_CL_HW_IF_STR` | `0xED` (`MAGIC_READ_CL`) | `0x00` | `_IOR(..., char*)` | Строка интерфейса Clevo (буфер `50` байт) |
| `R_CL_FANINFO1` / `2` / `3` | `0xED` (`MAGIC_READ_CL`) | `0x10` / `0x11` / `0x12` | `_IOR(..., int32_t*)` | Статус вентиляторов Clevo `0..=2` (младший байт `0..255` — скважность) |
| `R_CL_WEBCAM_SW` | `0xED` (`MAGIC_READ_CL`) | `0x13` | `_IOR(..., int32_t*)` | Чтение состояния веб-камеры Clevo |
| `R_CL_FLIGHTMODE_SW` | `0xED` (`MAGIC_READ_CL`) | `0x14` | `_IOR(..., int32_t*)` | Чтение состояния режима полёта Clevo |
| `R_CL_TOUCHPAD_SW` | `0xED` (`MAGIC_READ_CL`) | `0x15` | `_IOR(..., int32_t*)` | Чтение состояния тачпада Clevo |
| `W_CL_FANSPEED` | `0xEE` (`MAGIC_WRITE_CL`) | `0x10` | `_IOW(..., int32_t*)` | Упакованные скорости 3 вентиляторов: `f0 \| (f1 << 8) \| (f2 << 16)` |
| `W_CL_FANAUTO` | `0xEE` (`MAGIC_WRITE_CL`) | `0x11` | `_IOW(..., int32_t*)` | Авто-режим вентиляторов (битовая маска `0x0F`) |
| `W_CL_WEBCAM_SW` | `0xEE` (`MAGIC_WRITE_CL`) | `0x12` | `_IOW(..., int32_t*)` | Переключатель веб-камеры Clevo (`0`/`1`) |
| `W_CL_FLIGHTMODE_SW` | `0xEE` (`MAGIC_WRITE_CL`) | `0x13` | `_IOW(..., int32_t*)` | Переключатель режима полёта Clevo |
| `W_CL_TOUCHPAD_SW` | `0xEE` (`MAGIC_WRITE_CL`) | `0x14` | `_IOW(..., int32_t*)` | Переключатель тачпада Clevo |
| `W_CL_PERF_PROFILE` | `0xEE` (`MAGIC_WRITE_CL`) | `0x15` | `_IOW(..., int32_t*)` | Профиль производительности Clevo (`0..=3`) |
| `R_UW_HW_IF_STR` | `0xEF` (`MAGIC_READ_UW`) | `0x00` | `_IOR(..., char*)` | Строка интерфейса Uniwill |
| `R_UW_MODEL_ID` | `0xEF` (`MAGIC_READ_UW`) | `0x01` | `_IOR(..., int32_t*)` | Идентификатор модели Uniwill |
| `R_UW_FANSPEED` / `FANSPEED2` | `0xEF` (`MAGIC_READ_UW`) | `0x10` / `0x11` | `_IOR(..., int32_t*)` | Скважность вентиляторов CPU / GPU (`0..=200`) |
| `R_UW_FAN_TEMP` / `FAN_TEMP2` | `0xEF` (`MAGIC_READ_UW`) | `0x12` / `0x13` | `_IOR(..., int32_t*)` | Температура датчиков CPU / GPU (°C) |
| `R_UW_MODE` / `MODE_ENABLE` | `0xEF` (`MAGIC_READ_UW`) | `0x14` / `0x15` | `_IOR(..., int32_t*)` | Текущий режим Uniwill и флаг программного переопределения |
| `R_UW_FANS_OFF_AVAILABLE` | `0xEF` (`MAGIC_READ_UW`) | `0x16` | `_IOR(..., int32_t*)` | Поддержка полной остановки вентиляторов (0%) |
| `R_UW_FANS_MIN_SPEED` | `0xEF` (`MAGIC_READ_UW`) | `0x17` | `_IOR(..., int32_t*)` | Минимальная допустимая скорость вентиляторов (%) |
| `R_UW_TDP0` / `TDP1` / `TDP2` | `0xEF` (`MAGIC_READ_UW`) | `0x18` / `0x19` / `0x1A` | `_IOR(..., int32_t*)` | Текущие лимиты мощности PL1 / PL2 / PL4 (Вт) |
| `R_UW_TDP0..2_MIN` | `0xEF` (`MAGIC_READ_UW`) | `0x1B` / `0x1C` / `0x1D` | `_IOR(..., int32_t*)` | Аппаратные минимумы PL1 / PL2 / PL4 (Вт) |
| `R_UW_TDP0..2_MAX` | `0xEF` (`MAGIC_READ_UW`) | `0x1E` / `0x1F` / `0x20` | `_IOR(..., int32_t*)` | Аппаратные максимумы PL1 / PL2 / PL4 (Вт) |
| `R_UW_PROFS_AVAILABLE` | `0xEF` (`MAGIC_READ_UW`) | `0x21` | `_IOR(..., int32_t*)` | Количество доступных ODM-профилей производительности |
| `W_UW_FANSPEED` / `FANSPEED2` | `0xF0` (`MAGIC_WRITE_UW`) | `0x10` / `0x11` | `_IOW(..., int32_t*)` | Установка скважности вентиляторов CPU / GPU (`0..=200`) |
| `W_UW_MODE` / `MODE_ENABLE` | `0xF0` (`MAGIC_WRITE_UW`) | `0x12` / `0x13` | `_IOW(..., int32_t*)` | Запись индекса режима (`>= 0`) и включение (`0`/`1`) |
| `W_UW_FANAUTO` | `0xF0` (`MAGIC_WRITE_UW`) | `0x14` | `_IO(...)` | Отмена всех `W_UW_FANSPEED*`, возврат в авто-режим EC |
| `W_UW_TDP0` / `TDP1` / `TDP2` | `0xF0` (`MAGIC_WRITE_UW`) | `0x15` / `0x16` / `0x17` | `_IOW(..., int32_t*)` | Запись PL1 / PL2 / PL4 (Вт) после валидации `[min..=max]` |
| `W_UW_PERF_PROF` | `0xF0` (`MAGIC_WRITE_UW`) | `0x18` | `_IOW(..., int32_t*)` | Запись ODM-профиля Uniwill (`1..=max`) |

### 1.3. Карта регистров EC для Windows WMI (`crates/ucc-hardware/src/windows_wmi.rs`)

| Константа `ec_reg` | Адрес EC | Назначение |
|---|---|---|
| `CPU_TEMP_C` / `GPU_TEMP_C` | `0x043E` / `0x044F` | Температура CPU / GPU (°C) |
| `PL1_WATTS` / `PL2_WATTS` / `PL4_WATTS` | `0x0740` / `0x0741` / `0x0742` | Лимиты TDP (Вт) |
| `FAN_MANUAL_MODE` | `0x0751` | `1` = ручное управление вентиляторами, `0` = авто-режим EC |
| `PERF_MODE` | `0x0752` | Код профиля производительности (`1..=3`) |
| `CPU_FAN_DUTY` / `GPU_FAN_DUTY` | `0x1804` / `0x1809` | Скважность вентиляторов (`0..=200`) |

Чтение активного платформенного профиля в `TuxedoIoBackend` дополнительно опрашивает `/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile` и (при отсутствии `DmiQuirks::has_acpi_platform_profile_quirk`) `/sys/firmware/acpi/platform_profile`.

---

## 2. Управление CPU (`crates/ucc-hardware/src/cpu/`)

### 2.1. Драйверы масштабирования и sysfs-узлы

| Параметр / драйвер | Sysfs-путь (относительно `/sys/devices/system/cpu/`) | Поддерживаемые значения / поведение |
|---|---|---|
| `ScalingDriver` | `cpu<N>/cpufreq/scaling_driver` | `intel_pstate`, `amd-pstate`, `amd-pstate-epp`, `acpi-cpufreq`, `Unknown` |
| Список ядер | `present` (fallback `possible`) | Парсинг диапазонов `0-1,8-9` (`<= 4096` ID) без вывода offline-ядер в online |
| Online / hotplug | `cpu<N>/online` | `1` / `0`. Ядро `cpu0` без файла `online` считается обязательным (`hotpluggable = false`, всегда `online = true`) |
| Governor | `cpu<N>/cpufreq/scaling_governor`, `scaling_available_governors` | Белый список: `""`, `performance`, `powersave`, `ondemand`, `conservative`, `schedutil`, `userspace` |
| EPP | `cpu<N>/cpufreq/energy_performance_preference`, `energy_performance_available_preferences` | Белый список: `""`, `default`, `performance`, `balance_performance`, `balance_power`, `power`. При `no_epp_write_quirk = true` запись пропускается |
| Частоты | `cpu<N>/cpufreq/scaling_{cur,min,max}_freq`, `scaling_available_frequencies` | Значения в kHz. Перед установкой новых границ `CpuWorker::apply_cpu_profile` сбрасывает `min`/`max` во избежание конфликта clamping |
| Intel Turbo / HWP Boost | `intel_pstate/no_turbo`, `intel_pstate/hwp_dynamic_boost` | `no_turbo`: `1` = выкл., `0` = вкл.; `hwp_dynamic_boost`: `1` = вкл., `0` = выкл. |
| AMD / ACPI Boost | `cpufreq/boost` | Инвертированная логика относительно `no_turbo`: `0` = turbo выкл., `1` = turbo вкл. |
| Контроль дрейфа | `CpuWorker` (`uccd-worker-cpu`, каждые `10 s`) | До `MAX_REAPPLY_ATTEMPTS = 3` попыток повторного применения при внешнем изменении |

### 2.2. Определение гибридной топологии P/E-ядер (`CpuTopology`)

| Источник данных | Механизм | Результат / кэширование |
|---|---|---|
| Физическое ядро и SMT | `cpu<N>/topology/core_id`, `cpu<N>/topology/thread_siblings_list` | Кэшируется в `CpuTopology.cache`, чтобы отключённые (`online = 0`) ядра сохраняли метаданные в UI/телеметрии |
| Тип ядра (`P` / `E`) | `CPUID` leaf `0x1A` (subleaf `0`, `eax >> 24`) на Linux x86/x86_64 с RAII `AffinityGuard` (`sched_getaffinity` -> `sched_setaffinity(id)` -> восстановление исходной маски) | `0x40` -> `"P"` (Performance), `0x20` -> `"E"` (Efficient), иначе `"unknown"`. Никогда не вызывается при нестандартном `root_path != /sys/devices/system/cpu` |

---

## 3. Телеметрия и управление питанием GPU (`ADR-04`)

### 3.1. Матрица вендоров GPU

| Вендор / драйвер | Источники телеметрии (`sysfs` / библиотеки) | Собираемые метрики | Управление питанием |
|---|---|---|---|
| **Intel iGPU (`i915` single-GT)** | `cardX/gt_act_freq_mhz` (fallback `gt_cur_freq_mhz` при RC6 = 0), `gt_RP0_freq_mhz`, `device/hwmon/hwmon*/{temp1_input,power1_average,power1_input,energy1_input}` | Частота (MHz), max RP0 (MHz), температура (°C, строго из per-GPU hwmon, без подмены CPU package), мощность (W) / энергия (uJ), PCI BDF (`device/uevent` `PCI_SLOT_NAME`) | RAPL fallback: `/sys/class/powercap/intel-rapl:*` с именем `uncore`/`gpu`/`graphics`, привязка к CPU package через `device/local_cpulist` -> `physical_package_id` (fail-closed при неоднозначности) |
| **Intel iGPU (`i915` multi-GT)** | `cardX/gt/gt0/rps_{act,cur,RP0}_freq_mhz` + `device/hwmon` | Аналогично single-GT | Аналогично single-GT |
| **Intel iGPU (`xe`)** | `cardX/device/tile0/gt0/freq0/{act_freq,cur_freq,rp0_freq}` + `device/hwmon` | Аналогично `i915` | Аналогично `i915` |
| **AMD GPU (`amdgpu`)** | `hwmon*/temp1_input` (`/1000`), `freq1_input` (`/1e6`), `power1_average` / `power1_input` (`/1e6`), `device/pp_dpm_sclk` (парсинг max `*MHz`), `device/uevent` | Температура (°C), частота ядра и max (MHz), мощность (W), PCI BDF | Sysfs / PowerDevices PCI PM |
| **NVIDIA dGPU (NVML + NvAPI)** | Динамическая загрузка через `libloading`: `libnvidia-ml.so.1` (Unix) / `nvml.dll` (Windows); опционально NvAPI (`NVAPI_INITIALIZE_ID = 0x0150E828`, `NVAPI_VOLTAGE_ID = 0x465F9BCF`, сопоставление по PCI bus/slot `0x1BE0B8E5`/`0x2A0A350F`) | Температура, GR/MEM частоты и offsets (`NVML_CLOCK_OFFSET_VER1`), VRAM (`NVML_MEMORY_V2`), загрузка GPU/MEM/ENC/DEC, P-state, мощность и лимиты (default/max/enforced), `perf_limit_reason` (9 битовых флагов троттлинга `0x01..0x100`), напряжение ядра (mV) | NVML `set_power_limit` (mW); cTGP offset через `/sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset` или `/sys/devices/platform/tuxedo_nvidia_ctgp/ctgp_offset`; `nvidia-powerd.service` через PowerDevices |

### 3.2. Политика изоляции NVIDIA D3cold (`ADR-04`, `NvidiaPowerGuard`)

| Условие (`/sys/bus/pci/devices/<bdf>/`) | Поведение телеметрии и NVML |
|---|---|
| `vendor == 0x10de`, `class == 0x03*`, `power/runtime_status != "active"` (например, `suspended`) | Вызовы NVML **запрещены**. Возвращается `DGpuSnapshot::sleeping_nvidia` (`is_sleeping = true`, все сенсоры `None`). |
| `power/runtime_status == "active"` и `power/control == "on"` | Опрос NVML разрешён на каждом цикле (`nvidia_telemetry_due == true`). |
| `power/runtime_status == "active"` и `power/control == "auto"` | Опрос NVML выполняется только при `cycle % 20 == 0` (~16 с окно покоя при шаге воркера `800 ms`), чтобы ядро успевало переводить dGPU в D3cold autosuspend. |

---

## 4. Внешняя СЖО (`LCT22002` / `LCT21001` / Aquaris BLE GATT)

Реализовано в `crates/ucc-hardware/src/cooler.rs`, `bluez_cooler.rs` и `crates/uccd/src/backend/cooling.rs`.

### 4.1. Идентификаторы BlueZ GATT (Nordic UART Service)

| Параметр | Значение |
|---|---|
| Поддерживаемые модели (`LctDeviceModel`) | `Lct22002` (имя содержит `lct22002`), `Lct21001` (имя содержит `lct21001`, `coolingsystem`, `aquaris`) |
| Service UUID (`NORDIC_UART_SERVICE_UUID`) | `6e400001-b5a3-f393-e0a9-e50e24dcca9e` |
| TX Characteristic (`NORDIC_UART_CHAR_TX`) | `6e400002-b5a3-f393-e0a9-e50e24dcca9e` |
| RX Characteristic (`NORDIC_UART_CHAR_RX`) | `6e400003-b5a3-f393-e0a9-e50e24dcca9e` |
| CCCD (`CLIENT_CHAR_CONFIG_DESCRIPTOR_UUID`) | `00002902-0000-1000-8000-00805f9b34fb` |
| D-Bus интерфейсы BlueZ | `org.bluez`: `org.freedesktop.DBus.ObjectManager`, `org.bluez.Device1`, `org.bluez.GattCharacteristic1` |
| Конфигурация сопряжения и RGB | `/etc/ucc/aquaris.json` (`AQUARIS_CONFIG_PATH`): сохраняет MAC-адрес (приоритет 1), уникальное advertised-имя (приоритет 2 при ротации случайного BLE MAC) и последнее состояние RGB |

### 4.2. Формат 8-байтных пакетов (`0xFE ... 0xEF`) и коды помпы

| Команда | Байты `[0..8]` | Допустимые параметры и ограничения |
|---|---|---|
| `CMD_RESET` (`0x19`) | `FE 19 00 01 00 00 00 EF` | Сброс контроллера |
| `CMD_FAN` (`0x1B`) | `FE 1B 01 <duty> 00 00 00 EF` | `duty`: `0..=100` (%); выключение: `FE 1B 00 00 00 00 00 EF` |
| `CMD_PUMP` (`0x1C`) | `FE 1C 01 3C <voltage> 00 00 EF` | Фиксированная скважность `60` (`0x3C`); при `Off` отправляется `turn_off(0x1C)` (`FE 1C 00 00 00 00 00 EF`) |
| `CMD_RGB` (`0x1E`) | `FE 1E 01 <R> <G> <B> <state> EF` | `R/G/B`: `0..=255`; выключение: `FE 1E 00 00 00 00 00 EF` |

**Таблица кодов напряжения помпы (`PumpVoltage`):**

| Код (`u8`) | Enum | Напряжение | Статус |
|---|---|---|---|
| `0x00` | `PumpVoltage::V11` | `11 V` | Разрешено |
| `0x01` | `PumpVoltage::V12` | `12 V` | **ЗАПРЕЩЕНО (`CoolerError::V12Forbidden`)** — блокируется энкодером на всех LCT-устройствах |
| `0x02` | `PumpVoltage::V7` | `7 V` | Разрешено |
| `0x03` | `PumpVoltage::V8` | `8 V` | Разрешено |
| `0x04` | `PumpVoltage::Off` | `0 V` (выкл.) | Разрешено (`turn_off(CMD_PUMP)`) |

**Режимы RGB СЖО (`RgbState`):** `0x00` = `Static`, `0x01` = `Breathe`, `0x02` = `Colorful`, `0x03` = `BreatheColor`, `0x04` = `Temperature`.

### 4.3. Таймауты, backoff и конечный автомат (`CoolerStateMachine`)

Состояния: `Disconnected`, `Discovering`, `Connecting`, `Connected`, `Reconnecting`, `Failed`.

| Константа | Значение | Назначение |
|---|---|---|
| `BLE_WRITE_GAP` | `80 ms` | Минимальный интервал между последовательными GATT-записями (`BleWriteLimiter`) |
| `KEEPALIVE_INTERVAL` | `10 s` | Интервал проверки активности соединения |
| `KEEPALIVE_MISS_THRESHOLD` | `3` | Число пропущенных keepalive до объявления разрыва (`Disconnected`) |
| `FAST_RECONNECT_MAX_FAILURES` | `3` | Число последовательных ошибок подключения до перехода в `Failed` (триггер полного сканирования) |
| `GATT_CACHE_PURGE_FAILURE_THRESHOLD` | `3` | Порог ошибок для сброса GATT-кэша |
| `DEFAULT_GATT_RESOLVE_TIMEOUT` / `GATT_POLL_INTERVAL` | `5 s` / `25 ms` | Ожидание появления характеристик TX/RX в `ObjectManager` |
| Отключение при выключении/сне | `3 s` timeout на `sess.disconnect()`, пауза цикла `500 ms` при `cooler_suspending` | Гарантия чистого разрыва BLE до перехода в suspend |

---

## 5. Подсветка клавиатуры и Lightbar (`crates/ucc-lighting`)

### 5.1. Клавиатура RGB (`SysfsLedTransport` и `UniwillHidTransport`)

| Режим / транспорт | Пути обнаружения / USB VID:PID | Зоны и протокол записи |
|---|---|---|
| Белая однозонная (sysfs) | `/sys/devices/platform/tuxedo_keyboard/leds/white:kbd_backlight`, `/sys/bus/platform/devices/tuxedo_nb05_kbd_backlight/leds/white:kbd_backlight` | `zones = 1`, `led_ids = [0]`, только `brightness` (`max_brightness`) |
| Per-key RGB (sysfs, 126 зон) | Драйверы в порядке приоритета: `/sys/bus/hid/drivers/tuxedo-keyboard-ite`, `ite_829x`, `ite_8291`, `/sys/bus/platform/drivers/tuxedo_nb04_kbd_backlight` -> `*/leds/*rgb:kbd_backlight*` | `multi_intensity` (`"R G B"`) + перезапись `brightness` для копирования в `row_data`. Атомарный батчинг через `device/controls/buffer_input` (`1` -> запись зон -> `0` под RAII `BufferInputGuard`) |
| 1..3-зонная RGB (sysfs fallback) | `/sys/devices/platform/tuxedo_keyboard/leds/rgb:kbd_backlight{,_1,_2}` | `zones = 1..3`, `multi_intensity` + `brightness`, опционально `buffer_input` |
| Прямой USB HID (`UniwillHidTransport`) | `048D:600B` (`ITE_VENDOR_ID = 0x048D`, `ITE_KEYBOARD_PRODUCT_ID = 0x600B`) | `126` зон (`led_ids = 0..126`), `max_brightness = 255`. Feature reports: `buffer_input` = `[0x09, 0x02, en, 0, 0, 0, 0, 0]`; `brightness` = `[0x08, 0x02, 0x01, 0x05, lvl, 0, 0, 0]`; `zones` = `[0x14, count, r0, g0, b0, ...]` |

**Калибровка физической раскладки Mechrevo / Uniwill `0x1A` (`keyboard.rs`):** `MECHREVO_EXPECTED_ZONES = 126`, `MECHREVO_PHYSICAL_KEY_COUNT = 101`, якорные `led_id`: `LCtrl = 0`, `LShift = 21`, `Enter = 56`. Функция `map_states_by_led_id` при `states.len() > caps.zones` индексирует цвета строго по `caps.led_ids`, исключая сдвиг клавиш.

### 5.2. Передняя полоса Lightbar и 5 анимаций (`lightbar.rs`)

| Транспорт | Пути / USB VID:PID | Зоны / протокол |
|---|---|---|
| Sysfs (`ite_8291_lb` / LED class) | `/sys/class/leds/*` (приоритет `rgb:lightbar` > `lightbar` > имена с `lightbar`/`front`, исключая `keyboard`/`kbd`/`input`) | `brightness` (`max_brightness <= 65535`), `multi_intensity` (число RGB-триплетов = `zones`) |
| Прямой USB HID (`UniwillHidTransport`) | `048D:7001` (4 зоны — передняя полоса) и `048D:7000` (5 зон — полоса + логотип крышки) | Feature reports: `brightness` = `[0x08, 0x01, lvl, 0, 0, 0, 0, 0]`; `color` = `[0x14, 0x01, r, g, b, 0, 0, 0]` |

**Анимации Lightbar (частота кадров: `LIGHTBAR_FRAME_INTERVAL = 20 ms` / 50 Гц):**

| Идентификатор (`as_str`) | Длительность цикла (`duration_ms`) | Алгоритм кадра |
|---|---|---|
| `Pulse (stays on)` | `1640 ms` | Треугольная огибающая `abs(1.0 - t / 820.0)` к базовому цвету |
| `Flash (stays off)` | `1640 ms` | Инвертированная огибающая `1.0 - abs(1.0 - t / 820.0)` |
| `Rainbow` | `3000 ms` | Полный цикл HSV hue `t / 3000.0` |
| `Police Strobe` | `1440 ms` | Шаг `60 ms`: фаза `0` = `(255,0,0)`, фаза `2` = `(0,0,255)`, фазы `1,3` = `BLACK` |
| `Disco` | `1350 ms` | Шаг `90 ms`: `0..60 ms` цвет `hue((step*137 + 41) % 360)`, `60..90 ms` = `BLACK` |

Настройки `LightbarSettings` хранят независимые профили (`enabled`, `brightness_val`, `mode`, `color_mode = Auto | Fixed(RGB)`) для режимов `ac` (default brightness `255`), `type_c` (`150`) и `bat`, а также флаги `managed`, `silent_mode`, `welcome_anim`.

---

## 6. Зарядка, дисплей, крышка, пробуждение и Power Devices

### 6.1. Управление зарядкой батареи (`crates/uccd/src/backend/display_gpu.rs`)

| Функция | Sysfs-путь | Семантика и транзакционность |
|---|---|---|
| Пороги заряда | `/sys/class/power_supply/BAT*/charge_control_start_threshold`, `charge_control_end_threshold` | Запись с сохранением предыдущих значений в `ChargingRollback` для отката при ошибке |
| Тип зарядки | `/sys/class/power_supply/BAT*/charge_type` | Токен с проверкой чтения (`write_charging_token_checked`) |
| Профиль зарядки | `/sys/devices/platform/tuxedo_keyboard/charging_profile/{charging_profiles_available,charging_profile}` | Проверка принадлежности списку `*_available` перед записью |
| Приоритет зарядки | `/sys/devices/platform/tuxedo_keyboard/charging_priority/{charging_priorities_available,charging_priority}` | Проверка принадлежности списку `*_available` перед записью |

### 6.2. Подсветка и частота обновления дисплея

| Подсистема | Механизм / пути | Детали реализации |
|---|---|---|
| Подсветка (`uccd`) | `/sys/class/backlight/*` (`brightness`, `actual_brightness`, `max_brightness`, `type`) | Выбор устройства по `score = topology_score * 10 + type_bonus` (внутренний DRM-коннектор + приоритет `raw(2) > platform(1) > firmware(0)`). Для `amdgpu_bl*` применяется инверсия `max_val - raw`. Верификация рампы: до `250 ms` с шагом `5 ms` и откатом при несовпадении |
| Режимы дисплея (`uccd`) | `/sys/class/drm/card*-*/{status,modes,mode}` | Приоритет внутренних панелей (`eDP*`, `LVDS*`, `DSI*`) со `status == connected`; экспорт `GetDisplayModesJSON` |
| Частота обновления и DPMS (`ucc-session-agent`) | **Hyprland**: `hyprctl` (`dispatch dpms on/off <mon>`, JSON `monitors` -> `availableModes`); **KDE Plasma**: `kscreen-doctor` (`output.<mon>.enable/disable`); **X11 / прочие**: `RefreshBackend::Unsupported` (без мутаций композитора) | Детект по `XDG_CURRENT_DESKTOP`. Применяется только к внутренним панелям `eDP-*` / `LVDS-*`; проверяется принадлежность к NVIDIA (`0x10de`) в DRM |

### 6.3. Политика крышки и пробуждения (`crates/uccd/src/lid.rs`)

| Параметр | Значения / пути | Правила применения |
|---|---|---|
| Действия крышки (`powerDevices.lid.{ac,type_c,bat}`) | `screen_off`, `ignore`, `suspend`, `hibernate`, `poweroff`, `lock` | Default: `ac = ignore`, `type_c = ignore`, `bat = suspend`. Ингибитор `systemd-logind` `handle-lid-switch` удерживается, если действие `!= suspend`. Debounce после отключения AC: `DEFAULT_AC_UNPLUG_DEBOUNCE = 3 s` |
| Дополнительные флаги крышки | `wakeOnlyLidAndKeyboard` (default `true`), `turnOffKeyboardBacklightOnLidClose` (default `true`), `turnOffWaterCoolerLedOnLidClose` (default `false`) | Выключение подсветки клавиатуры / LED СЖО при закрытии крышки |
| `WakeupPolicy` (`sysfs`) | `/sys/devices/.../power/wakeup` (обход до глубины `16`, без перехода по симлинкам, запрет `..`) | При `wakeOnlyLidAndKeyboard = true` включаются **только**: `platform/PNP0C0D:<id>` (ACPI Lid Switch) и `platform/i8042/serio<N>` с `"KBD"` в `description`; все остальные `enabled` переводятся в `disabled` |
| `WakeupPolicy` (`procfs`) | `/proc/acpi/wakeup` | Все устройства со статусом `*enabled` переключаются записью имени устройства `<dev>\n` (однократно на устройство) |

### 6.4. Матрица Power Devices (`ac` / `type_c` / `bat`, `crates/ucc-hardware/src/devices/`)

Все записи проходят строгий белый список (`whitelist.rs`, максимум `2048` устройств, длина имени `<= 512`, запрет `..`, таймаут внешних команд `iw`/`systemctl`/`nvidia-smi` = `3 s`, запуск без shell-интерполяции).

| Тип (`type`) | Разрешённый шаблон `path` | Допустимые значения (`ac_val`, `type_c_val`, `bat_val`) | Способ применения |
|---|---|---|---|
| `pci` | `/sys/bus/pci/devices/[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}.[0-7]/power/control` | `on`, `auto` | Запись в `sysfs` |
| `usb` | `/sys/bus/usb/devices/[A-Za-z0-9_.:-]+/power/control` | `on`, `auto` | Запись в `sysfs` |
| `wifi` | Имя интерфейса `1..=32` символов `[A-Za-z0-9_.:-]` | `on`, `off` | `/usr/bin/iw` (или `/usr/sbin/iw`, `/sbin/iw`) |
| `service` | `nvidia-powerd`, `nvidia-powerd.service` | `active`, `inactive` | `/usr/bin/systemctl` |
| `sys` | `/sys/module/snd_hda_intel/parameters/power_save` | Целое `0..=3600` | Запись в `sysfs` |
| `sys` | `/proc/sys/vm/dirty_writeback_centisecs` | Целое `0..=60000` | Запись в `procfs` |
| `sys` | `/proc/sys/kernel/nmi_watchdog` | `0`, `1` | Запись в `procfs` |
| `sys` | `/sys/devices/system/cpu/intel_pstate/no_turbo` | `0`, `1` | Запись в `sysfs` |
| `cpu.{ac,type_c,bat}` | `governor`, `epp`, `aspm`, `noTurbo` | `aspm`: `""`, `default`, `performance`, `powersave`, `powersupersave` (`/sys/module/pcie_aspm/parameters/policy`); `governor`/`epp` по белым спискам раздела 2.1 | Применение при смене режима питания |
