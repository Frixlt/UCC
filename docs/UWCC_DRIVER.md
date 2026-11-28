# Драйвер ядра `uwcc-rs` (`/dev/uwcc`, `ADR-05` / `M14`)

Техническая спецификация модуля ядра `uwcc-rs` (Rust-for-Linux 7.3+, `GPL-2.0-only`), поставляемого в `kernel/uwcc-rs/` в рамках этапа `M14` (`ADR-05`).

## 1. Архитектура и лицензионные границы

Модуль `uwcc_rs.ko` разделён на две изолированные части:

1. **Ядро драйвера на Rust (`kernel/uwcc-rs/src/uwcc_rs.rs`, `validation.rs`, `ring_buffer.rs`)**:
   - жизненный цикл модуля (`kernel::Module`, `module!`) и состояние драйвера, защищённое IRQ-безопасным `spinlock_t`;
   - экспортируемые `extern "C"` обработчики валидации и диспетчеризации `ioctl`, а также операций кольцевого буфера WMI (`push`/`pop`/`has_data`);
   - строгая проверка адресов EC, диапазонов, длины буферов (`SMRW`), параметров `PWUP`, режима дескриптора (`FMODE_WRITE`) и привилегий `CAP_SYS_ADMIN`;
   - неблокирующий кольцевой буфер WMI-событий фиксированной ёмкости (`64` записи) с монотонными метками времени (`CLOCK_MONOTONIC`).
   - Модули `ring_buffer.rs` и `validation.rs` не зависят от `std` (`no_std`) и тестируются на хосте отдельно от дерева ядра.
2. **Минимальный C FFI-шим (`kernel/uwcc-rs/src/acpi_shim.c`, `acpi_shim.h`)**:
   - регистрирует символьное устройство `/dev/uwcc` (`struct miscdevice`) с полным набором `struct file_operations` (`.open`, `.unlocked_ioctl`, `.compat_ioctl`, `.read`, `.poll`), делегируя валидацию, диспетчеризацию и работу с очередью событий в Rust;
   - оборачивает вызовы Linux 7.3+, не имеющие стабильных безопасных Rust-обёрток в ядре: `acpi_evaluate_integer` / `acpi_evaluate_object` для устройства ACPI `INOU0000` (`ECRR`, `ECRW`, `SMRW`, `PWUP`), современный `struct wmi_driver` (`wmi_driver_register` / `wmi_driver_unregister`) и `hwmon_device_register_with_info`;
   - сериализует все обращения к EC через внутренний `mutex` (`uwcc_ec_lock`) и защищает состояние драйвера через `spinlock_t` (`uwcc_state_lock`).

**Лицензия и clean-room:** все файлы в `kernel/uwcc-rs/` распространяются под лицензией `GPL-2.0-only` (UAPI-заголовок с `WITH Linux-syscall-note`). Реализация выполнена по открытой спецификации ACPI `INOU0000` и HID-протоколов ITE без проприетарных или OEM-компонентов.

## 2. UAPI ABI (`/dev/uwcc`)

Контракт между модулем ядра и userspace (`UwccBackend`) зафиксирован в `kernel/uwcc-rs/include/uapi/uwcc.h`.

- **Устройство:** `/dev/uwcc` (`misc` device, имя `"uwcc"`).
- **Magic:** `'U'` (`0x55`).
- **Версия API:** `UWCC_API_VERSION = 1`.

### 2.1. Структуры ABI

| Структура | Размер (байт) | Поля и назначение |
|---|---|---|
| `struct uwcc_ec_reg` | `4` | `__u16 addr; __u8 val; __u8 _pad;` — чтение/запись одного регистра EC (`_pad` обязан быть `0`) |
| `struct uwcc_smrw` | `68` | `__u16 offset; __u16 len; __u8 data[64];` — чтение заводской таблицы пределов ACPI `SMRW` (`1 <= len <= 64`, `offset + len <= 0x0400`) |
| `struct uwcc_pwup` | `8` | `__u32 enable; __u32 level;` — управление режимом повышенной мощности для СЖО (`enable in 0..=1`, `level in 0..=3`, при `enable == 0` требуется `level == 0`) |
| `struct uwcc_info` | `16` | `__u32 api_version; __u32 project_id; __u32 ec_version; __u32 flags;` — версия ABI, идентификатор шасси (например, `0x1A`), версия EC, битовая маска возможностей (`UWCC_FLAG_*`) |
| `struct uwcc_wmi_event` | `16` | `__u64 timestamp_ns; __u32 code; __u32 payload;` — запись события WMI для `read()` (монотонное время `ktime_get_ns()`) |

Биты `uwcc_info.flags`:
- `UWCC_FLAG_HAS_EC` (`1 << 0`) — доступен ACPI `INOU0000` (`ECRR`);
- `UWCC_FLAG_HAS_SMRW` (`1 << 1`) — доступен метод `SMRW`;
- `UWCC_FLAG_HAS_PWUP` (`1 << 2`) — доступен метод `PWUP`;
- `UWCC_FLAG_HAS_WMI_EVENTS` (`1 << 3`) — зарегистрирован обработчик WMI-уведомлений;
- `UWCC_FLAG_HAS_HWMON` (`1 << 4`) — зарегистрировано устройство `/sys/class/hwmon/hwmonX`.

### 2.2. Команды `ioctl`

| Макрос | Кодировка | Права доступа | Назначение |
|---|---|---|---|
| `UWCC_IOC_GET_INFO` | `_IOR('U', 0x01, struct uwcc_info)` | Чтение `/dev/uwcc` | Получение версии API, `project_id`, `ec_version` и флагов возможностей |
| `UWCC_IOC_EC_READ` | `_IOWR('U', 0x02, struct uwcc_ec_reg)` | Чтение `/dev/uwcc` | Чтение 1 байта EC (`0x0400..=0x18FF`) через `ECRR` |
| `UWCC_IOC_EC_WRITE` | `_IOW('U', 0x03, struct uwcc_ec_reg)` | `FMODE_WRITE` + `CAP_SYS_ADMIN` | Запись 1 байта EC через `ECRW` строго по белому списку разрешённых адресов |
| `UWCC_IOC_SMRW` | `_IOWR('U', 0x04, struct uwcc_smrw)` | Чтение `/dev/uwcc` | Чтение до 64 байт таблицы заводских пределов `SMRW` |
| `UWCC_IOC_PWUP` | `_IOW('U', 0x05, struct uwcc_pwup)` | `FMODE_WRITE` + `CAP_SYS_ADMIN` | Вызов метода `PWUP(enable, level)` |

### 2.3. Белый список записи EC и обработка ошибок

Перед передачей вызова в `acpi_shim.c` модуль `validation.rs` применяет fail-closed фильтрацию:

- Разрешённые диапазоны записи: `0x0740..=0x0745` (TDP `PL1`/`PL2`/`PL4`, тепловые и энергетические лимиты; регистр `0x0748` `UWCC_EC_REG_PROJECT_ID` исключён), `0x0751..=0x075C` (режимы вентиляторов/производительности, зарядка, PWM), `0x0760..=0x0768` (`FN_LOCK`, `WIN_LOCK`, `USB_SLEEP_CHARGING`, пороги заряда `0x0765`/`0x0766`, питание Lightbar `0x0767`/`0x0768`), `0x0770..=0x078F` (дополнительные аппаратные переключатели EC), `0x0F00..=0x0F5F` (таблицы `RamFan 1.5` и флаги `CUME`/`UFME`), `0x1804` и `0x1809` (legacy-скважность вентиляторов).
- Запись в датчики (`0x043E`, `0x044F`, `0x0464..=0x046C`) и регистр идентификации шасси (`0x0748`) отклоняется с `-EACCES`.
- Адреса вне `0x0400..=0x18FF` и выход `SMRW` за пределы таблицы отклоняются с `-ERANGE`.
- Отсутствие `FMODE_WRITE` или `CAP_SYS_ADMIN` для `EC_WRITE`/`PWUP` отклоняется с `-EPERM`.
- Невалидная длина `SMRW` (`0` или `> 64`), ненулевой `_pad` или невалидные параметры `PWUP` отклоняются с `-EINVAL`.

### 2.4. Поток WMI-событий (`read()` / `poll()`)

- Уведомления WMI (GUID `ABBC0F72-8EA1-11D1-00A0-C90629100000`) помещаются в кольцевой буфер на `64` записи; при переполнении вытесняется самая старая запись с инкрементом счётчика `dropped`.
- `read()` возвращает кратные `sizeof(struct uwcc_wmi_event)` (`16` байт) записи; при пустом буфере блокируется (`wait_event_interruptible`, `-ERESTARTSYS` при сигнале) либо возвращает `-EAGAIN` при `O_NONBLOCK`.
- `poll()` регистрирует очередь ожидания и возвращает `EPOLLIN | EPOLLRDNORM` при наличии непрочитанных событий.

## 3. Карта нативного `hwmon` и изоляция `D3cold`

Модуль регистрирует устройство `uwcc` в `/sys/class/hwmon/hwmonX` (режим `0444`, только чтение):

| Атрибут `hwmon` | Метка (`*_label`) | Регистры EC | Преобразование и единицы |
|---|---|---|---|
| `fan1_input` | `CPU Fan` | `0x0464` (MSB), `0x0465` (LSB) | `((u16)hi << 8) \| lo`, реальные RPM (`0xFFFF` -> `-ENODATA`) |
| `fan2_input` | `GPU Fan` | `0x046C` (MSB), `0x046B` (LSB) | `((u16)hi << 8) \| lo`, реальные RPM (`0xFFFF` -> `-ENODATA`) |
| `pwm1` | — | `0x075B` | Шкала Uniwill `0..=200` -> стандартная `hwmon` `0..=255` (`(raw * 255 + 100) / 200`) |
| `pwm2` | — | `0x075C` | Шкала Uniwill `0..=200` -> стандартная `hwmon` `0..=255` (`(raw * 255 + 100) / 200`) |
| `temp1_input` | `CPU (EC)` | `0x043E` | `raw * 1000` м°C (`0` или `> 150` -> `-ENODATA`) |
| `temp2_input` | `GPU (EC)` | `0x044F` | `raw * 1000` м°C (`0` или `> 150` -> `-ENODATA`) |

**Инвариант `D3cold` (`ADR-04` / `ADR-05`):** чтение `temp2_input` выполняется исключительно из регистра EC `0x044F` через ACPI `ECRR`. Драйвер не обращается к PCI-конфигурации NVIDIA RTX, NVML или `drm`, поэтому опрос `sensors`, `btop`, `MangoHud` или `Waybar` не выводит дискретную GPU из состояния `D3cold` (`suspended`).

## 4. Сборка и установка DKMS (Linux 7.3+, `CONFIG_RUST=y`)

### 4.1. Требования к целевому ядру

- Ядро Linux `7.3+` с заголовками (`linux-cachyos-headers` или аналогичный пакет `headers`).
- Включённая опция `CONFIG_RUST=y` (проверка: `zgrep '^CONFIG_RUST=y' /proc/config.gz` или `grep '^CONFIG_RUST=y' /lib/modules/$(uname -r)/build/.config`).
- Присутствие ACPI-устройства `/sys/bus/acpi/devices/INOU0000:00`.
- Инструментарий сборки ядра: `dkms`, `make`, компилятор C и `rustc`/`bindgen`, соответствующие сборке ядра.

### 4.2. Локальная сборка модуля

```bash
cd kernel/uwcc-rs
make KDIR=/lib/modules/$(uname -r)/build
sudo make KDIR=/lib/modules/$(uname -r)/build install
```

### 4.3. Установка через Arch Linux `PKGBUILD` (`uwcc-dkms`)

Пакет `uwcc-dkms` конфликтует с `tuxedo-drivers-dkms`, устанавливает исходники DKMS в `/usr/src/uwcc-rs-0.0.1/`, правила udev в `/usr/lib/udev/rules.d/99-uwcc.rules` и blacklist модулей `tuxedo-drivers` в `/usr/lib/modprobe.d/uwcc-blacklist-tuxedo.conf`:

```bash
cd kernel/uwcc-rs/packaging/arch
makepkg -si
```

После установки и перезагрузки (или ручной перезагрузки модулей):

```bash
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo modprobe uwcc_rs
ls -l /dev/uwcc
sensors | grep -A 8 uwcc
```

### 4.4. Правила `udev` и `modprobe`

- `kernel/uwcc-rs/udev/99-uwcc.rules`: задаёт `MODE="0660", GROUP="root"` для `/dev/uwcc` и `hidraw`-устройств ITE (`048D:600B`, `048D:7001`, `048D:7000`).
- `kernel/uwcc-rs/modprobe/uwcc-blacklist-tuxedo.conf`: блокирует загрузку `tuxedo_io`, `uniwill_wmi`, `tuxedo_keyboard`, `tuxedo_nb02_nvidia_power_ctrl`, `ite_8291` и `ite_8291_lb`.

## 5. Процедура чистого отката на `tuxedo-drivers-dkms` + `TuxedoIoBackend`

Модули `uwcc_rs` и `tuxedo-drivers` взаимоисключающие и не должны загружаться одновременно. Для полного возврата на стек `M0–M13`:

```bash
# 1. Остановить демон управления
sudo systemctl stop uccd.service

# 2. Выгрузить uwcc_rs и удалить пакет uwcc-dkms (удаляет modprobe blacklist и 99-uwcc.rules)
sudo modprobe -r uwcc_rs || true
sudo pacman -Rns uwcc-dkms

# 3. Установить tuxedo-drivers-dkms обратно (если был удалён)
sudo pacman -S tuxedo-drivers-dkms

# 4. Обновить initramfs и правила udev, загрузить модули tuxedo-drivers
sudo mkinitcpio -P
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo modprobe tuxedo_io uniwill_wmi tuxedo_keyboard ite_8291 ite_8291_lb

# 5. Запустить демон (автодетект выберет TuxedoIoBackend по /dev/tuxedo_io)
sudo systemctl start uccd.service
```

Проверка отката:
- `/dev/uwcc` отсутствует, `lsmod | grep uwcc_rs` пуст;
- `/dev/tuxedo_io` присутствует, `ucc-cli status` работает через `TuxedoIoBackend`.

## 6. Чек-лист аппаратной приёмки `M14.9` — MECHREVO YAOSHI X6AR55xY (`0x1A`)

| # | Проверка | Критерий приёмки |
|---|---|---|
| 1 | Сборка DKMS и загрузка модуля | `dkms status` показывает `uwcc-rs/0.0.1, installed`; `dmesg` содержит строку инициализации `project_id=0x1a` без `BUG`/`WARNING` |
| 2 | Устройство `/dev/uwcc` и `UWCC_IOC_GET_INFO` | Права `0660 root:root`; `api_version == 1`, `project_id == 0x1A`, выставлены флаги `HAS_EC` и `HAS_HWMON` |
| 3 | Валидация границ и прав | Непривилегированный `EC_WRITE`/`PWUP` возвращает `EPERM`; запись в `0x043E`/`0x044F`/`0x0464` возвращает `EACCES`; `SMRW` с `len=0`/`len=65` возвращает `EINVAL` |
| 4 | Нативный `hwmon` (`sensors` / `btop`) | Присутствуют `fan1_input`, `fan2_input` (реальные RPM), `pwm1`, `pwm2`, `temp1_input`, `temp2_input` |
| 5 | Сохранение `D3cold` NVIDIA RTX | При простое `/sys/bus/pci/devices/<rtx_bdf>/power/runtime_status` остаётся `suspended` во время многократного опроса `sensors` и `hwmon` |
| 6 | Таблица `SMRW`, `PL1`/`PL2`/`PL4`, CPU `TCC` | `UWCC_IOC_SMRW` возвращает ненулевые заводские лимиты; запись `PL1`/`PL2`/`PL4` и `TCC` подтверждается чтением EC |
| 7 | `RamFan 1.5` и аварийный возврат авто-режима | 16-точечные кривые (`0xF00–0xF5C`, `CUME`/`UFME`) и `Fan Boost` применяются; останов `uccd` и `ucc-hardware --fan-auto` возвращают firmware auto |
| 8 | Внешняя СЖО `LCT22002` и `PWUP` | `UWCC_IOC_PWUP` переключает уровень мощности СЖО; `PumpVoltage::V12` (`1`) остаётся заблокированным |
| 9 | Прямой `hidraw` подсветки (`048D:600B`, `7001`/`7000`) | Клавиатура сохраняет калибровку (`LCtrl=0`, `LShift=21`, `Enter=56`); работают Lightbar, логотип крышки, EC-выключатель `0x767`/`0x768` и 5 анимаций без `ite_8291*` |
| 10 | Зарядка, EC-переключатели и USB-камера | Применяются пороги/режимы заряда, Fn Lock, Win Lock, USB sleep charging и отключение комбинированной USB RGB+IR веб-камеры |
| 11 | WMI-события (`read()`/`poll()`), крышка и sleep/resume | Нажатия Fn-клавиш доставляются в `/dev/uwcc` с монотонными метками; переходы suspend/resume не ломают EC/hwmon/BLE |
| 12 | Чистый откат (`M14.8`) | Процедура из раздела 5 восстанавливает `tuxedo-drivers-dkms` + `TuxedoIoBackend` без остаточных конфликтов модулей |
