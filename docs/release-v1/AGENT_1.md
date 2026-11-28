# Runbook Агента 1 — Kernel `uwcc-rs`, `ucc-hardware` и `ucc-lighting`

- **Ветка:** `release/a1-kernel-hardware` (от `rust/migration`)
- **Эксклюзивная зона владения:** `kernel/uwcc-rs/**`, `crates/ucc-hardware/**`, `crates/ucc-lighting/**`
- **Обязательное чтение перед стартом:** `docs/release-v1/CONTRACT.md`, `docs/release-v1/SYNC_PROTOCOL.md`, `docs/audit/TOTAL_AUDIT_2026_11_28.md`, `docs/hardware/EC_01..05`, `docs/hardware/UNIWILL_0X1A_REGISTERS.md`.

## Фаза 1 — 5 параллельных субагентов-исполнителей

### Субагент 1.1: `kernel/uwcc-rs/` (безопасность, allowlist, RMW, DKMS/udev)
- `kernel/uwcc-rs/src/validation.rs`:
  - **Удалить опасные адреса** из `is_valid_ec_write_addr()`: `0x0753` (VRM current limit), `0x0762`, `0x07A4` (RO BIOS byte), `0x07A5` (OEM_3/авария); сузить `0x0751..=0x0753` до `0x0751` (и `0x0752` только если нужно для legacy non-0x1A).
  - **Добавить безопасные адреса:** `0x0746` (`DB_MAX_TGP`), `0x074E` (`Fn Lock`), `0x0787` (`FAN_SWITCH_SPEED`), `0x07D0` (`CHARGE_LIMIT_DOWN`), `0x07E2..=0x07E5` (`LIGHTBAR_BAT_CTRL` + RGB).
  - **Ядерный атомарный RMW (`uwcc_acpi_ec_rmw` под `uwcc_ec_lock`)** + таблица масок в `validation.rs`:
    - `0x0727`: разрешить только `BIT(6)` (`0x40`, строго запретить изменение `BIT(7) PL4D`!).
    - `0x07C6`: разрешить только `BIT(2)` (`0x04`) и `BIT(1..0)` (`0x03`), строго сохранять `BIT(6)` (кнопка питания на EC 77.0) и `BIT(3..4)`.
    - `0x07C5`: строго запретить установку `BIT(4)` (`0x10`, отключение тепловой защиты).
    - `0x0726`: разрешить только `BIT(7)` и `BIT(3)` (сохранять `bits 4..6`).
    - `0x07B9`: сбрасывать/отклонять `BIT(7)` (`CHARGE_CTRL_REACHED`).
  - Поднять `UWCC_PWUP_MAX_LEVEL` до `7` (или `5`) в `validation.rs`, `acpi_shim.c:322`, `include/uapi/uwcc.h:76`.
- `kernel/uwcc-rs/src/acpi_shim.c`:
  - Исправить `UWCC_EC_REG_EC_VER_HI/LO` с `0x0770`/`0x0771` на `0x0454`/`0x047D`.
  - Исправить цикл `ACPI_TYPE_BUFFER` в `SMRW` (убрать безусловный `break;` при коротком буфере, не скрывать `AE_AML_BUFFER_LIMIT` под `ret = 0`).
  - Добавить C-проверки границ в `uwcc_acpi_gpu_power()`, передавать реальные `writable` и `capable(CAP_SYS_ADMIN)` в `uwcc_rs.rs`, добавить `nonseekable_open()` в `uwcc_dev_open()`.
- `kernel/uwcc-rs/src/uwcc_rs.rs`: добавить `PhantomData<*mut ()>` (`!Send`) в `StateGuard`.
- `kernel/uwcc-rs/dkms.conf`, `Makefile`, `udev/99-uwcc.rules`, `modprobe/`, `modules-load/`:
  - Добавить `BUILD_EXCLUSIVE_CONFIG="CONFIG_RUST"` в `dkms.conf`; `SUBSYSTEMS=="usb"` в правила `hidraw` и убрать опасную рекомендацию `uaccess` из `99-uwcc.rules`; добавить `# SPDX-License-Identifier: GPL-2.0-only` в `modules-load/uwcc_rs.conf` и `packaging/arch/uwcc-dkms.install`.
- **Тесты:** расширить unit-тесты `validation.rs` на все новые/удалённые адреса, RMW-маски и `PWUP <= 7`; прогнать `rustc --test kernel/uwcc-rs/src/validation.rs && ./validation`.

### Субагент 1.2: `crates/ucc-hardware/src/uwcc/{mod.rs, ramfan.rs}`
- **`ramfan.rs` (P0):** переписать на реальную 96-байтную раскладку `0x0F00..=0x0F5F` (6×16: `CPU_UpT 0x0F00..0x0F0F`, `CPU_DownT 0x0F10..0x0F1F`, `CPU_Duty 0x0F20..0x0F2F`, `GPU_UpT 0x0F30..0x0F3F`, `GPU_DownT 0x0F40..0x0F4F`, `GPU_Duty 0x0F50..0x0F5F`), маркер `Duty[0] = 0`, гистерезис `DownT[1..=active]`, padding `0xFF`/`last_duty`, протокол записи `0x07C6 BIT(2)=0 -> 0x07C5 BIT(7) -> 96 bytes -> 0x07C6 BIT(2)=1`, `set_fan_boost` через `0x0751 BIT(6)`.
- **`uwcc/mod.rs` (P0/P1):**
  - `PL4` удвоенное кодирование: читать RO `pl4_doubled = (ec_read(0x0727) & 0x80) != 0` при инициализации (НИКОГДА не менять `BIT(7)`); при `pl4_doubled` делить на 2 при записи в `0x0785` и умножать на 2 при чтении `0x0785`/`0x07A9`/`SMRW[2]`.
  - `SMRW` 12-byte декодер: исправить перепутанные `data[7]` (`DefaultTGP` = base) и `data[4]` (`ConfigurableTGP` = max offset сверх base), не смешивать `dynamic_boost_max_w` (`data[5]`) в `ctgp_max_w`, парсить `data[3]` (`TccOffset`).
  - `TCC`: реализовать `read_tcc_offset()` через `ec_read(0x0463) & 0x7F` и `set_tcc_offset(offset)` через `ec_write(0x0786, 0x80 | offset)` (`0..=40`, на `0x1A` `TjMax=105`).
  - GPU power `0x0743..=0x0746` + live readback `0x0788`/`0x07D5`: `set_ctgp()` пишет offset `watts - base_tgp` в `0x0744` + `0x0743 BIT(0)|BIT(2)`; `set_dynamic_boost_watts()` пишет `0xFF`/`0x00` в `0x0745`, ватты в `0x0746` и `0x0743 BIT(1)`; `read_ctgp_watts()` возвращает `base_tgp + ec_read(0x0788)` (допуская offset `0`); реализовать `read_dynamic_boost_watts()` (`0x07D5`) и `read_pmsc_mode()` (`"office"|"gaming"|"turbo"|"custom"`).
  - Исправить `Fn Lock` -> RMW `0x074E BIT(4)`; `USB Sleep Charging` -> RMW `0x0767 BIT(4)`; нижний порог заряда `0x07D0` + `0x07B9` (`0 -> 100%`, не писать обратно `BIT(7)`); `0x07A6` маска `0x30` (`0x00/0x10/0x20`).
  - Внедрить tear-free чтение RPM (`hi1 -> lo -> hi2`, до 3 попыток); пробрасывать ошибки RMW в `set_fan_speed_percent`; в `Drop for UwccBackend` сбрасывать `0x07C6 BIT(2)`, `0x0751 BIT(6)`, `CUME`/`0x0726 BIT(7)` и `0x0741 BIT(0)` в правильном порядке; проверять `0x078E BIT(6)` перед чтением `0x07A7..=0x07A9`.
- **Тесты:** полные регрессионные тесты в `crates/ucc-hardware/tests/uwcc_backend.rs` на каждый пункт.

### Субагент 1.3: `detection.rs`, `platform.rs`, `uwcc/{wmi_events.rs, uapi.rs, webcam.rs}`
- `detection.rs`: в `detect_vendor()` проверять `R_HWCHECK_UW` (Uniwill) **перед** `R_HWCHECK_CL` (Clevo); добавить `ChassisProfile` с разведением Intel `X6AR55xY` vs AMD `X6FR5xxY` (`board_name` + CPU vendor).
- `platform.rs`: добавить дефолтные методы `read_dynamic_boost_watts`, `set_dynamic_boost_watts`, `read_pmsc_mode` (строго по `CONTRACT.md`), поднять `PWUP` level до `7`, исправить doc-комментарий `set_lightbar_power`.
- `wmi_events.rs`: синхронизировать все `wmi_code` с `uniwill-wmi.h` (`TOUCHPAD` `0x0004`/`0x0005`, `PERF_MODE_CYCLE` `0x00B0`, `KBD_BRIGHTNESS_DOWN` `0x00B1`, `WEBCAM` `0x00CF`, `FAN_BOOST` `0x00A7`, `WIN_LOCK` `0x0040`/`0x0041`/`0x00A5` + системные `0x00AB`, `0x00AE`, `0x00BB`, `0x00C2`); добавить `const assert` на `InputEvent`; при обрыве в `write_batch_all` после key-down отправлять key-up.
- `uapi.rs` & `webcam.rs`: `const assert` `size_of` на все 7 UAPI-структур; поднять `PWUP` до `7`; не маскировать partial read в `poll_event` как `Ok(None)`; не проглатывать ошибки `sysfs` в `webcam.rs`.

### Субагент 1.4: Не-`uwcc` модули `crates/ucc-hardware/src/` (`cooler.rs`, `bluez_cooler.rs`, `fan_controller.rs`, `sysfs.rs`, `cpu/*`, `traits.rs`, `windows_wmi.rs`, `nvml.rs`)
- **`cooler.rs` (P0):** реализовать аварийную термозащиту СЖО поверх `CoolerMode::Manual` (`OVERHEAT_TRIGGER_TEMP_C = 90`, `OVERHEAT_RELEASE_TEMP_C = 80`, `overheat_override: bool` -> принудительно `FanSpeed(100)` + `PumpVoltage::V11`, блокировка `turn_off_manual_fan/pump` до остывания `< 80 °C`).
- **`cpu/controller.rs` + `traits.rs` + `sysfs.rs` (P0/P1):** перевести записи CPU и `RealSysfs` на `SysfsRoot` + `O_NOFOLLOW` (`openat`), убрать TOCTOU `.exists()` и `file.set_len(0)` для sysfs, добавить rollback по ядрам CPU при частичной ошибке.
- **`fan_controller.rs` (P1):** убрать блокирующий `tick` с async-воркера Tokio (`spawn_blocking`/OS-поток), синхронный best-effort `set_fans_auto()` в `Drop`, backoff-восстановление из `write_failsafe_active`, не пересоздавать `FanControlLogic` при неизменных кривых, возвращать реальные `read_fan_speed_percent`.
- **`bluez_cooler.rs` (P1):** обернуть `get_managed_objects`, `start_discovery`, `disconnect`, `Proxy::new` в `tokio::time::timeout`; брать `Instant::now()` *после* `write_packet().await`; подключить `GATT_CACHE_PURGE_FAILURE_THRESHOLD` + экспоненциальный backoff.
- **`windows_wmi.rs`, `ioctl.rs`, `nvml.rs`, `devices/controller.rs`, `telemetry/*`:** `ptr::read_unaligned` для `BSTR`, таймаут вместо `WBEM_INFINITE`, убрать все `.lock().unwrap()`/`.read().unwrap()`/`.write().unwrap()`.

### Субагент 1.5: `crates/ucc-lighting/src/**` + `uwcc/mod.rs` lighting helpers
- **`hid_transport.rs` + `hidraw_linux.rs` + `windows_hid.rs` (P0/P1):**
  - Исключить `0xFF89`; ранжированный выбор Usage Page `0xFF03 > 0xFF12 > 0xFF02`; парсить все коллекции в `hidraw_linux.rs`.
  - Починить HID-протоколы `048D:7001` (Zone 2 `power_on` `0x1A 0x02 0x01 0x04 ... 0x01` + `0x08 0x22` + полная 4-пакетная последовательность выключения) и `048D:7000` (`0xA2` опрос топологии + `flag=1|2|3` + `ctrl=0x21|0x22|0x23`).
  - Добавить поддержку `048D:CE00` (`usage_page`/`version_number`).
  - Унифицировать framing `get_feature_report` (Linux/Windows), добавить межпакетную паузу `1 ms` на живом транспорте, убрать двойной ретрай и TOCTOU на `cached_colors`, заменить `.expect("cached hidraw handle")`.
- **`lightbar.rs` + `keyboard.rs`:** снизить частоту анимации до ~30 Гц (`33 ms`), не проглатывать ошибки в `advance()`, добавить `KeyboardBacklightController::on_resume()`.
- **EC lighting (`uwcc/mod.rs`):** в `set_lightbar_power()` управлять `0x0748 BIT(2)` (`S0_OFF`) без сброса `BIT(0)` (`LIGHTBAR_APP_EXISTS`); защитить `0x07C5 BIT(5)` от конфликта с Whisper Mode и добавить симметричный сброс + `read_lid_logo_ec()`.

## Фаза 2 — 5 независимых adversarial-ревьюеров + Финальный Gate
После Фазы 1 (при необходимости запросив у пользователя «продолжай» для сброса лимита субагентов!) запусти 5 независимых ревьюеров по 1 на каждый блок 1.1–1.5, исправь все замечания и прогони:
```bash
export CARGO_TARGET_DIR="$HOME/ucc-target-slim" TMPDIR="$HOME/ucc-tmp" RUSTFLAGS="-D warnings -C debuginfo=0"
rustc --test kernel/uwcc-rs/src/validation.rs -o "$HOME/ucc-tmp/test_validation" && "$HOME/ucc-tmp/test_validation"
cargo fmt --all -- --check
dbus-run-session -- cargo test -j 2 -p ucc-hardware -p ucc-lighting
```
Запушь в `release/a1-kernel-hardware` и обнови `sync/agent1.md` до `READY_FOR_MERGE`.
