# Миграция, совместимость конфигурации и аварийное восстановление

[Сборка и упаковка](BUILDING.md) · [Проверки и тестирование](TESTING.md)

Документ описывает подготовку резервных копий перед обновлением, переход с `frixit-control-center` / C++ UCC и TUXEDO Control Center (TCC) на `ucc-rusted`, механизмы восстановления повреждённой конфигурации, аварийный сброс оборудования и полный откат к прежнему пакету.

## 1. Предмиграционный чек-лист резервного копирования

Выполните все шаги до установки или обновления пакета `ucc-rusted`.

```bash
# 1. Создать защищённый каталог резервной копии
BACKUP_DIR="/root/ucc-pre-migration-$(date +%Y%m%d-%H%M%S)"
sudo install -d -m 0700 "$BACKUP_DIR"

# 2. Сохранить системную конфигурацию и резервные копии (если существуют)
for f in /etc/ucc/settings /etc/ucc/settings.bak /etc/ucc/settings.backup /etc/ucc/aquaris.json; do
  if [ -f "$f" ]; then
    sudo install -m 0600 "$f" "$BACKUP_DIR/$(basename "$f")"
  fi
done

# 3. Зафиксировать контрольные суммы SHA-256
sudo sha256sum "$BACKUP_DIR"/* | sudo tee "$BACKUP_DIR/SHA256SUMS"

# 4. Зафиксировать состояние systemd-юнитов
{
  echo "=== is-enabled ==="
  for u in uccd.service uccd-sleep.service uccd-pre-sleep.service tccd.service tuxedo-power.service; do
    printf '%s: %s\n' "$u" "$(systemctl is-enabled "$u" 2>&1 || true)"
  done
  echo "=== is-active ==="
  for u in uccd.service uccd-sleep.service tccd.service tuxedo-power.service; do
    printf '%s: %s\n' "$u" "$(systemctl is-active "$u" 2>&1 || true)"
  done
} | sudo tee "$BACKUP_DIR/systemd-state.txt"
```

### Сохранение архива старого пакета для отката

- **Arch Linux:** скопируйте установленный пакет из кеша pacman:
  ```bash
  sudo cp -v /var/cache/pacman/pkg/frixit-control-center-*.pkg.tar.zst "$BACKUP_DIR/" 2>/dev/null || true
  sudo cp -v /var/cache/pacman/pkg/uniwill-control-center-*.pkg.tar.zst "$BACKUP_DIR/" 2>/dev/null || true
  ```
- **Debian / Ubuntu:** сохраните предыдущий `.deb` из `/var/cache/apt/archives/` или каталога сборки `dist/` в `$BACKUP_DIR/`.
- **Fedora / RPM:** сохраните предыдущий `.rpm` из `dist/rpmbuild/RPMS/` или локального кеша `dnf` в `$BACKUP_DIR/`.

---

## 2. Обновление с `frixit-control-center` / C++ UCC и импорт из TCC

### Переход с `frixit-control-center` или legacy C++ UCC

1. Остановите конкурирующие контроллеры (если активны):
   ```bash
   sudo systemctl disable --now tccd.service tuxedo-power.service 2>/dev/null || true
   ```
2. Установите новый пакет:
   - **Arch Linux:** `PKGBUILD` объявляет `provides=("uniwill-control-center=$pkgver" "frixit-control-center=$pkgver")`, `conflicts=('uniwill-control-center' 'uniwill-control-center-mechrevo' 'frixit-control-center')`, `replaces=('frixit-control-center')`.
     ```bash
     sudo pacman -U ./ucc-rusted-*.pkg.tar.zst
     ```
     Скрипт `ucc.install` автоматически выполняет:
     - в `pre_upgrade`: `ucc_preserve_settings` копирует `/etc/ucc/settings` (если это обычный файл, не симлинк) в `/var/lib/ucc/package-upgrade/settings` (`0600`) и создаёт маркер `/var/lib/ucc/package-upgrade/pending`;
     - в `post_upgrade`: `ucc_restore_settings` восстанавливает `/etc/ucc/settings`, если pacman перенёс старый package-owned файл в `.pacsave`, затем выполняет `systemctl daemon-reload` и перерегистрирует `uccd-sleep.service` (заменяя устаревшие ссылки `uccd-pre-sleep.service` в `suspend`/`hibernate` targets при сохранении явно отключённого состояния).
   - **Debian / Ubuntu:** `sudo apt install ./ucc_*.deb`
   - **Fedora / RPM:** `sudo dnf upgrade ./ucc-*.rpm`
3. Перезапустите системный демон и пользовательские клиенты:
   ```bash
   sudo systemctl daemon-reload
   sudo systemctl reload dbus
   sudo systemctl restart uccd.service
   systemctl status uccd.service --no-pager
   ```
   В пользовательской сессии перезапустите `ucc-gui`, `ucc-desktop-tray` и `ucc-session-agent` (или перелогиньтесь), чтобы загрузились новые бинарники.

### Офлайн-импорт из TUXEDO Control Center (TCC) и проверка через `ucc-import-check`

Офлайн-конвертация (`crates/ucc-config/src/import.rs`) не обращается к оборудованию, D-Bus и установленным системным файлам, и **никогда не исполняет JavaScript**: таблицы вентиляторов `defaultFanProfiles` / `customFanPreset` извлекаются бинарным разбором ASAR-архива (`dist/tuxedo-control-center/service-app/common/models/TccFanTable.js`) и запекаются по каждому градусу `0..=100` °C с учётом `minimumFanspeed`, `maximumFanspeed`, `offsetFanspeed` и `nvidiaPowerCTRLProfile.cTGPOffset`.

#### Шаг 1. Офлайн-конвертация через `cargo xtask import-offline`

```bash
cargo xtask import-offline \
  /path/to/exported/Data \
  /tmp/ucc-import-out \
  --capabilities /path/to/keyboard-capabilities.json \
  --tcc-dir /etc/tcc \
  --tcc-archive /opt/tuxedo-control-center/resources/app.asar
```

Опциональные флаги:
- `--settings <path>` — базовый JSON-файл UCC (явные профили и `customFanProfiles` из него имеют приоритет над TCC);
- `--brightness <N>` — переопределение начальной яркости клавиатуры.

Каталог вывода (`/tmp/ucc-import-out`) содержит сгенерированный файл `settings`, отчёт `report.json`, отложенные поля и файл предпочтений рабочего стола.

#### Шаг 2. Preflight-валидация через `/usr/lib/ucc/ucc-import-check`

Перед копированием результата в `/etc/ucc/settings` выполните обязательную проверку:

```bash
/usr/lib/ucc/ucc-import-check /tmp/ucc-import-out/settings /path/to/keyboard-capabilities.json
```

Что проверяет `ucc-import-check`:
- ограничение размера каждого входного JSON-файла (`<= 16 MiB`);
- разбор `UccSettings::from_json` и доменную валидацию каждого профиля (`profiles`);
- валидность секций `powerDevices` (`PowerDeviceController::validate`) и `lightbar` (`LightbarSettings::normalize_json`);
- соответствие `keyboardBacklightStates` и `customKeyboardProfiles.*.json.states` аппаратным возможностям (`zones` в `1..=1024`, `maxBrightness > 0`, опциональный массив уникальных неотрицательных `ledIds` длины `zones`, каналы RGB `0..=255`);
- диапазоны кривых вентиляторов `tableCPU`, `tableGPU`, `tablePump`, `tableWaterCoolerFan` (`temp` в `0..=150`, `speed` в `0..=100`);
- ссылки `selectedKeyboardProfile` на существующие ключи `customKeyboardProfiles` (кроме встроенных `__*`);
- наличие и корректность всех трёх записей `stateMap`: `power_ac`, `power_type_c`, `power_bat`.

Коды возврата:
- `0` — проверка пройдена, в stdout выводится JSON `{"ok":true,"profiles":3,"keyboardZones":...,"devices":...}`;
- `1` — конфигурация отклонена, причина выведена в stderr (`Import rejected: ...`);
- `2` — неверное число аргументов (`Usage: ucc-import-check SETTINGS CAPABILITIES`).

#### Шаг 3. Установка проверенной конфигурации

```bash
sudo systemctl stop uccd.service
sudo install -m 0600 /tmp/ucc-import-out/settings /etc/ucc/settings
sudo systemctl start uccd.service
```

---

## 3. Совместимость формата конфигурации и восстановление после повреждения

### Разрешение путей (`ConfigManager`)

- Системный путь по умолчанию: `/etc/ucc/settings` (резервный вариант при чтении — `/etc/ucc/settings.json`).
- Пользовательский путь (только при `geteuid() != 0`): `$XDG_CONFIG_HOME/ucc/settings` или `~/.config/ucc/settings` (резервный вариант — `settings.json`).
- Процесс с правами `root` (`uccd`) всегда читает и записывает системный путь `/etc/ucc/settings`.

### Совместимость со схемой C++ UCC

Парсер `ucc-config` (`manager.rs`, `legacy.rs`) поддерживает прямой round-trip формата C++ UCC (`TccSettings`):
- сохраняются ключи `stateMap`, `profiles`, `customKeyboardProfiles`, `customFanProfiles`, `powerDevices`, `lightbar`, `keyboardBacklightStates`, `keyboardLayout`, `ycbcr420Workaround`, `cpuSettingsEnabled`, `fanControlEnabled`, `keyboardBacklightControlSupported`, `fahrenheit`, `shutdownTime`;
- глобальные настройки зарядки (`chargingProfile`, `chargingPriority`, `chargeType`, `chargeStartThreshold`, `chargeEndThreshold`) валидируются на верхнем уровне (`0..=100`, `start < end`) и применяются независимо от профилей производительности;
- при сериализации устраняется устаревший дубликат ключа `keyboardBacklightStates`;
- **карантин неиспользуемых профилей**: если отдельный профиль, не указанный в `stateMap`, содержит устаревшие невалидные поля, но в файле остаётся хотя бы один корректный профиль, загрузчик отбрасывает проблемный профиль вместо отказа всей конфигурации; если повреждён активный профиль из `stateMap` или все профили невалидны, запускается восстановление из резервной копии.

### Атомарная запись и ротация `.bak` / `.backup` (`persistence.rs`)

Функция `write_atomic` гарантирует целостность `/etc/ucc/settings` при сбое питания или падении процесса:
1. Блокирует запись, если активна переменная `UCC_READ_ONLY_PREVIEW` (возвращает `ConfigError::ReadOnlyPreview`).
2. Разрешает симлинк целевого пути, чтобы обновлять целевой файл, а не перезаписывать саму ссылку.
3. Записывает новые данные во временный файл `.<name>.tmp.<pid>.<ns>` в том же каталоге с флагом `O_NOFOLLOW` и выполняет `flush()` + `sync_all()`.
4. Выполняет ротацию резервных копий `<path>.bak` и `<path>.backup` (также через временный файл и `rename(2)`). **Защита от порчи бэкапа:** если текущий основной файл повреждён или не проходит валидацию, существующие валидные `.bak` / `.backup` не перезаписываются.
5. Атомарно заменяет целевой файл через `rename(2)` и вызывает `sync_all()` на файловом дескрипторе родительского каталога.

### Автоматическое и ручное восстановление повреждённого файла

При запуске `ConfigManager::load_with_recovery` при ошибке чтения или валидации `/etc/ucc/settings` автоматически пытается загрузить `/etc/ucc/settings.bak`, а затем `/etc/ucc/settings.backup`.

Ручное восстановление при повреждении `/etc/ucc/settings`:

```bash
# 1. Остановить демон
sudo systemctl stop uccd.service

# 2. Проверить наличие резервных копий и валидность JSON
sudo ls -la /etc/ucc/
sudo python3 -m json.tool /etc/ucc/settings.bak >/dev/null && echo "settings.bak OK"

# 3. Сохранить повреждённый файл для диагностики и восстановить из .bak
sudo cp -a /etc/ucc/settings /etc/ucc/settings.corrupted.$(date +%Y%m%d-%H%M%S)
sudo install -m 0600 /etc/ucc/settings.bak /etc/ucc/settings

# 4. Удалить осиротевшие временные файлы после аварийного прерывания (если есть)
sudo rm -f /etc/ucc/.*.tmp.* /etc/ucc/.*.tmp_restore.*

# 5. Запустить демон и проверить журнал
sudo systemctl start uccd.service
journalctl -u uccd.service -n 50 --no-pager
```

---

## 4. Аварийное восстановление оборудования

### 4.1. Сброс застрявших ручных вентиляторов

`uccd.service` настроен на автоматический вызов `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto`. Если демон упал до срабатывания хука или вентиляторы остались на фиксированных оборотах:

```bash
# Вернуть управление вентиляторами прошивке EC (Auto)
sudo /usr/lib/ucc/ucc-hardware --fan-auto

# Проверить текущие обороты EC и температуры
sudo /usr/lib/ucc/ucc-hardware --audit
```

Коды возврата `/usr/lib/ucc/ucc-hardware`:
- `0` — операция выполнена успешно;
- `1` — ошибка ioctl-записи/чтения;
- `2` — неверные аргументы командной строки;
- `4` — устройство `/dev/tuxedo_io` недоступно (проверьте загрузку модуля `tuxedo_io` / DKMS).

### 4.2. Восстановление после падения демона или сбоя сна (`/run/ucc/sleep-state.json`)

Во время перехода в сон `ucc-sleep prepare` сохраняет снимок ручного режима и состояния водяного охлаждения в `/run/ucc/sleep-state.json` и останавливает `uccd.service`. Если цикл сна прервался и демон не запустился обратно:

```bash
# 1. Проверить наличие и содержимое снимка сна
sudo ls -la /run/ucc/sleep-state.json
sudo cat /run/ucc/sleep-state.json 2>/dev/null || true

# 2. Попытаться штатно завершить фазу восстановления
sudo /usr/lib/ucc/ucc-sleep resume

# 3. Если снимок повреждён или resume завершился с ошибкой — очистить и перезапустить uccd
sudo rm -f /run/ucc/sleep-state.json
sudo systemctl reset-failed uccd.service uccd-sleep.service
sudo systemctl start uccd.service

# 4. Проверить журнал
journalctl -u uccd-sleep.service -u uccd.service -b --no-pager | tail -n 60
```

### 4.3. Сброс лимитов частот CPU, отключённых ядер и политики пробуждения

Если после некорректного завершения `uccd` часть логических CPU осталась в offline или с заниженным `scaling_max_freq`:

```bash
# 1. Вернуть все логические CPU в online (cpu0 обычно не имеет файла online)
for f in /sys/devices/system/cpu/cpu[0-9]*/online; do
  [ -w "$f" ] && echo 1 | sudo tee "$f" >/dev/null
done

# 2. Сбросить лимиты частот к аппаратным границам cpuinfo_min_freq / cpuinfo_max_freq
for d in /sys/devices/system/cpu/cpu[0-9]*/cpufreq; do
  [ -r "$d/cpuinfo_min_freq" ] && cat "$d/cpuinfo_min_freq" | sudo tee "$d/scaling_min_freq" >/dev/null
  [ -r "$d/cpuinfo_max_freq" ] && cat "$d/cpuinfo_max_freq" | sudo tee "$d/scaling_max_freq" >/dev/null
done

# 3. Снова разрешить Turbo Boost (Intel pstate или общий cpufreq)
[ -w /sys/devices/system/cpu/intel_pstate/no_turbo ] && echo 0 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo >/dev/null
[ -w /sys/devices/system/cpu/cpufreq/boost ] && echo 1 | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
```

### 4.4. Проверка и сброс политики пробуждения (`WakeupPolicy`)

`ucc-sleep prepare` и контроллер крышки управляют файлами `/sys/devices/.../power/wakeup` и таблицей `/proc/acpi/wakeup` (ограничивая пробуждение крышкой и клавиатурой при `lid.wakeOnlyLidAndKeyboard: true`). Для диагностики и ручного сброса:

```bash
# Просмотр текущих источников пробуждения ACPI
cat /proc/acpi/wakeup

# Переключение конкретного ACPI-устройства (например, XHC0) выполняется записью его имени
# echo XHC0 | sudo tee /proc/acpi/wakeup

# Включение wakeup для конкретного PCI/USB-устройства в sysfs
# echo enabled | sudo tee /sys/bus/usb/devices/<DEV>/power/wakeup
```

После перезапуска `uccd.service` с `wakeOnlyLidAndKeyboard: false` политика пробуждения пересчитывается и применяется повторно.

---

## 5. Полный откат (Downgrade / Rollback) к legacy C++ пакету

### Вариант A. Откат пакетной установки (Arch / Debian / Fedora)

```bash
# 1. Остановить службы и пользовательские клиенты
sudo systemctl stop uccd-sleep.service uccd.service
pkill -x ucc-gui ucc-desktop-tray ucc-session-agent 2>/dev/null || true

# 2. Вернуть вентиляторы в авто-режим и очистить снимок сна
sudo /usr/lib/ucc/ucc-hardware --fan-auto || true
sudo rm -f /run/ucc/sleep-state.json

# 3. Установить сохранённый пакет предыдущей версии
# Arch Linux:
sudo pacman -U "$BACKUP_DIR"/frixit-control-center-*.pkg.tar.zst
# Debian / Ubuntu:
# sudo dpkg -i "$BACKUP_DIR"/ucc_*.deb
# Fedora / RPM:
# sudo rpm -Uvh --oldpackage "$BACKUP_DIR"/ucc-*.rpm

# 4. Восстановить предмиграционный /etc/ucc/settings и сверить SHA-256
sudo install -m 0600 "$BACKUP_DIR/settings" /etc/ucc/settings
sudo sha256sum /etc/ucc/settings
cat "$BACKUP_DIR/SHA256SUMS"

# 5. Перечитать конфигурации systemd/D-Bus и запустить службы
sudo systemctl daemon-reload
sudo systemctl reload dbus
sudo systemctl enable --now uccd.service
sudo systemctl enable uccd-sleep.service
```

### Вариант B. Откат развёртывания через `cargo xtask maintenance deploy`

Если установка выполнялась через `cargo xtask maintenance deploy --install <bundle>`, откат выполняется по каталогу резервной копии, выведенному при установке:

```bash
sudo cargo xtask maintenance deploy --rollback /path/to/deploy-backup
```

### Верификация после отката

```bash
# 1. Статус демона и отсутствие ошибок запуска
systemctl status uccd.service --no-pager
journalctl -u uccd.service -n 40 --no-pager

# 2. Проверка D-Bus-интерфейса и аппаратных датчиков
busctl status com.uniwill.uccd
sudo /usr/lib/ucc/ucc-hardware --audit

# 3. Проверка состояния sleep-хука
systemctl is-enabled uccd-sleep.service
```
