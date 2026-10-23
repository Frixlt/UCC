# Проверки релиза 0.3.0

Дата: 22 октября 2026 года. Qt 6.12, CachyOS, KDE Wayland.
[README](../README.md) · [Сборка](BUILDING.md)

## Воспроизведение без управления ноутбуком

После сборки с `BUILD_TESTS=ON`:

```bash
ctest --test-dir build -j1 --output-on-failure -E '^test_gui_audit$'
dbus-run-session -- bash tests/run_gui_audit_test.sh \
  "$PWD/build/bin/test_gui_audit" \
  dashboardShowsDriverFailureAndClearsAfterRecovery \
  aboutBuildCreditsAndBundledLicense
python3 contrib/mechrevo/tests/test_migration.py
```

Первый набор содержит 35 CTest entries в локальной сборке с GUI, standalone tray,
session agent и Node.js. Наличие GJS добавляет отдельный GNOME async entry. Тесты используют fake sysfs/procfs, временные
конфигурации и отдельные D-Bus-сессии. Они покрывают профили, политики питания,
устройства, клавиатуру, Lightbar, события уведомлений, KScreen offers, процессы,
авторизацию, импортеры, sleep snapshots, сохранение settings при переходе
с private-пакета на public-пакет и driver health.

Галерея имеет отдельную проверку `renderReleaseGallery`, которая показывает все
10 разделов и предупреждение о драйвере, затем проверяет отсутствие аппаратных
записей. Команды для обоих оформлений: [screenshots/README.md](../screenshots/README.md).

## Зафиксированная проверка

- Полная сборка daemon/GUI/CLI/standalone tray/session agent выполнена локально.
- 35/35 CTest entries вне `test_gui_audit` прошли при восстановлении драйвера.
- Пять отдельных GUI-сценариев прошли: driver banner, About, физическая
  клавиатура, Lightbar и три колонки политик; вместе с init/cleanup — 7 QtTest cases.
- Все 22 снимка новой галереи получены и просмотрены; обе темы прошли
  `renderReleaseGallery` с проверкой отсутствия аппаратных записей.
- Ранее проведённые normal/preview проверки подтверждали read-only guards,
  физические LED ID, матрицу устройств, обработку ошибок и миграцию.

Полный старый `test_gui_audit` имел тайм-аут в сценариях истории уведомлений.
Он остаётся известным ограничением. Исключение этого entry из быстрого набора
и отдельные успешные сценарии не означают успешного полного GUI-аудита.
CI также разделяет эти наборы, чтобы результат был видимым.

## Проверено на работающей системе

- Linux 7.3.0-2-cachyos: custom DKMS собрал все 27 модулей; появился
  `/dev/tuxedo_io`, датчики CPU/GPU, 126 зон клавиатуры и Lightbar.
- Lightbar: анимация меняет RGB и заканчивается нулевым RGB в Flash.
- Демон сообщает `healthy: true` после восстановления драйвера.
- Установлены порядок загрузки модулей и старт uccd после systemd-modules-load.
- Пользователь подтвердил: клик по трею сразу активирует GUI и переключает
  на его рабочий стол в KDE Wayland.
- Старые TCC/Python-контроллеры остановлены; локальные настройки при обновлении
  сохранены. Private deployment packages в публичный релиз не включаются.

## Ещё не подтверждено

Настоящий цикл аппаратного suspend/resume, длительные обрывы внешней водянки,
полная матрица других ноутбуков и дистрибутивов. Автозагрузка модулей настроена,
но повторная перезагрузка после последнего восстановления отдельно не заявляется.
Сборка пакета и синтетические тесты не доказывают работоспособность отсутствующего
или заблокированного драйвером аппаратного интерфейса.

### Brightness regressions

`test_display_backlight` uses a fake sysfs tree with Intel and NVIDIA internal
connectors. It covers active-panel selection, a MUX topology change, disconnected
panels, generic backlights, raw-driver preference, percentage rounding, legacy
AMD inversion and failed writes (including deferred flush errors).

`test_system_monitor` delays brightness replies by 300 ms and checks that the UI
event loop stays responsive, intermediate requests coalesce, failures do not
replace the confirmed value and the synchronous client honors daemon rejection.
The focused GUI case `displayBrightnessCommitsOnReleaseAndIgnoresMonitorDuringDrag`
checks that dragging previews the value without hardware writes and commits once
on release, even if monitoring sends an update during the drag.

The optional ITE callback regression and live driver verification are described in
[the driver notes](../contrib/frixit/drivers/README.md#keyboard-brightness-with-kde--upower).

On 23 October 2026, the brightness fix passed 36/36 non-GUI CTest entries,
the six focused GUI scenarios (8 QtTest cases including setup/cleanup), the
ITE callback regression and four Python migration tests. The historical full
GUI audit was not rerun. The patched driver was also rebuilt for both installed
kernels and tested live; see the driver notes for the spot timing measurements.
