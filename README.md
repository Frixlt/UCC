# Frixit Control Center

**Наш ControlCenter на Python + UCC Revorked в одном приложении Qt 6 / C++20.**
Форк сохраняет управление профилями, охлаждением и водянкой из UCC и переносит
личные политики питания, устройства, калиброванную клавиатуру, Lightbar и трей.

[English](README.en.md) · [Релизы](https://github.com/Frixlt/UCC/releases) ·
[Сравнение с оригиналом](docs/COMPARISON.md) · [Сборка](docs/BUILDING.md) ·
[Проверки](docs/TESTING.md) · [Происхождение](PROJECT_ORIGINS.md) ·
[Архив Python ControlCenter](https://github.com/Frixlt/ControlCenter-Python)

![Frixit Control Center — Overview, тёмная тема](screenshots/current/dark/overview.png)

## Чем отличается наш форк

| Возможность | UCC Revorked `eb98bfa` | Frixit Control Center 0.3.0 |
| --- | --- | --- |
| Qt-интерфейс, мониторинг, профили, CPU/GPU-кривые, водянка | Есть | Сохранены |
| Независимые AC / USB-C / Battery политики устройств | Нет личной модели ControlCenter | CPU governor/EPP/turbo, ASPM, PCI/USB runtime PM, Wi-Fi, системные параметры и службы |
| Ручной режим питания | Профили UCC | Auto / AC / Type-C / Battery, физический источник показан отдельно |
| Клавиатура Mechrevo | Раскладка upstream | Наша физическая калибровка: Enter 56, левый Shift 21, левый Ctrl 0 |
| Lightbar и уведомления | Нет перенесённого Python-контроллера | Пять анимаций, цвет/яркость по питанию, Silent, Welcome и Test |
| Самостоятельный трей | Plasma-апплет | Дополнительный Qt-трей: батарея, режимы, процессы NVIDIA; Plasma-апплет сохранён |
| Ошибки драйверов | Нет новой диагностики | Уведомление при входе и красная плашка на Overview |
| Миграция личных настроек | Отдельный конвертер TCC | Конвертер ControlCenter с приоритетом пользовательских данных |

Полная таблица, архитектура и ограничения — в [сравнении проектов](docs/COMPARISON.md).
Исходная реализация лимита температуры CPU и `gpu-run` в форк не перенесены.
Пресет вентиляторов достигает 100% при 85 °C; он **не ограничивает температуру CPU**.

## Что доступно в интерфейсе

| Раздел | Назначение |
| --- | --- |
| Overview | CPU/GPU, датчики, активный профиль, быстрые органы управления водянкой и состояние драйверов |
| Profiles | CPU, частоты, поддерживаемые лимиты мощности, зарядка, экран, клавиатура и назначение профилей |
| Cooler Settings | Кривые CPU/GPU, копирование/сохранение, совместная скорость вентиляторов |
| Watercool Settings | Подключение Bluetooth, вентилятор, помпа, LED и температурные кривые |
| Monitor | История температуры, мощности, частот, вентиляторов и напряжения; масштабирование и маркеры |
| Keyboard & Hardware | Поклавишный RGB, яркость, выделение клавиш, webcam/Fn Lock/экран при поддержке оборудования |
| About | Версия/коммит, Qt, исходники нашего форка, авторы, лицензии и журналы |
| Power & Devices | Отдельные значения CPU и устройств для AC / Type-C / Battery, повторное обнаружение устройств |
| Lightbar | Режимы питания, цвет, яркость, анимации уведомлений и тест сохранённых настроек |
| Battery & Processes | Расход батареи, пользовательские процессы NVIDIA, настройки запуска и предложения частоты экрана KDE |

Темы **Light / Dark / Auto**, история уведомлений и единственный экземпляр окна
сохранены. В KDE Wayland клик по трею передаёт токен активации: окно получает фокус
и открывается на своём рабочем столе. Явный клик открывает окно даже при настройке
«запускать свёрнутым»; автоматический запуск учитывает эту настройку.

### Поведение питания и подсветки

Ручной режим имеет приоритет над автоматическим определением питания. Запросы
приложений могут повысить эффективный режим; исчезновение D-Bus-клиента убирает его
запрос. Политики устройств и CPU задаются отдельно от выбора профиля водянки.
Если управление политиками включено, его CPU-настройки имеют приоритет над профилем.

**Flash (stays off)** оставляет Lightbar выключенным между уведомлениями, в том числе
после Save, смены питания и запуска демона. Test использует сохранённые настройки
текущего эффективного режима. Silent подавляет обычные анимации; Test его обходит.
Содержимое уведомлений в демон не передаётся.

Ручные значения Fan/Pump применяются сразу; сохранение системного профиля сохраняет
их для повторного применения и переподключения водянки. Auto возвращает управление
температурным кривым. При `sameSpeed` оба внутренних вентилятора используют большую
из рассчитанных скоростей CPU/GPU. Третий вентилятор не представлен отдельной
управляемой кривой в текущем интерфейсе драйвера.

## Скриншоты версии 0.3.0

Это снимки **текущего Qt-приложения**, снятые 22 октября 2026 года в изолированной
D-Bus-сессии. Значения датчиков, имена процессов и профили демонстрационные.
Метод воспроизведения и контрольные суммы — в [screenshots/README.md](screenshots/README.md).

| Экран | Тёмная тема | Светлая тема |
| --- | --- | --- |
| Overview | ![Overview — Dark](screenshots/current/dark/overview.png) | ![Overview — Light](screenshots/current/light/overview.png) |
| Profiles | ![Profiles — Dark](screenshots/current/dark/profiles.png) | ![Profiles — Light](screenshots/current/light/profiles.png) |
| Cooler Settings | ![Cooler Settings — Dark](screenshots/current/dark/cooler-settings.png) | ![Cooler Settings — Light](screenshots/current/light/cooler-settings.png) |
| Watercool Settings | ![Watercool Settings — Dark](screenshots/current/dark/watercool-settings.png) | ![Watercool Settings — Light](screenshots/current/light/watercool-settings.png) |
| Monitor | ![Monitor — Dark](screenshots/current/dark/monitor.png) | ![Monitor — Light](screenshots/current/light/monitor.png) |
| Keyboard & Hardware | ![Keyboard & Hardware — Dark](screenshots/current/dark/keyboard-hardware.png) | ![Keyboard & Hardware — Light](screenshots/current/light/keyboard-hardware.png) |
| About | ![About — Dark](screenshots/current/dark/about.png) | ![About — Light](screenshots/current/light/about.png) |
| Power & Devices | ![Power & Devices — Dark](screenshots/current/dark/power-devices.png) | ![Power & Devices — Light](screenshots/current/light/power-devices.png) |
| Lightbar | ![Lightbar — Dark](screenshots/current/dark/lightbar.png) | ![Lightbar — Light](screenshots/current/light/lightbar.png) |
| Battery & Processes | ![Battery & Processes — Dark](screenshots/current/dark/battery-processes.png) | ![Battery & Processes — Light](screenshots/current/light/battery-processes.png) |
| Предупреждение о драйверах | ![Предупреждение о драйверах — Dark](screenshots/current/dark/driver-warning.png) | ![Предупреждение о драйверах — Light](screenshots/current/light/driver-warning.png) |

## Установка и обновление

Основной репозиторий и релизы: **[Frixlt/UCC](https://github.com/Frixlt/UCC)**,
ветка `main`. Для сборки клонируйте именно его:

```bash
git clone https://github.com/Frixlt/UCC.git
cd UCC
cmake -S . -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_TRAY=OFF -DBUILD_TESTS=ON
cmake --build build -j2
```

Зависимости, установка, запуск и read-only preview описаны в
[руководстве по сборке](docs/BUILDING.md). Для Arch/CachyOS основной `PKGBUILD`
собирает `frixit-control-center` из релиза; Plasma-апплет отключён, самостоятельный
трей и агент уведомлений включены. Рецепты DEB/RPM/Nix сохранены, но не являются
подтверждёнными бинарными релизами нашего форка.

Для переноса Python/TCC-настроек используйте [инструкцию миграции](contrib/frixit/README.md).
[DEPLOYMENT.md](contrib/frixit/DEPLOYMENT.md) описывает первоначальный переход,
резервирование и откат. Личные настройки, Bluetooth-адреса, снимки конфигурации
и локальные пакеты с импортированными настройками в публичный релиз не входят.

### Драйверы и поддерживаемое оборудование

Основная проверенная машина — **MECHREVO YAOSHI Series-X6AR55xY**,
Intel Core Ultra 9 275HX / RTX 5080 Laptop, CachyOS, Qt 6.12, KDE Wayland.
Поддержка остальных устройств upstream сохранена; полного повторного тестирования
на других моделях не было. Доступность функций определяется драйвером и прошивкой.

Нужны совместимые `tuxedo-drivers`, заголовки текущего ядра и успешная сборка DKMS.
Установка приложения сама по себе не устанавливает драйвер. Для локального custom
снимка на Linux 7.3 сохранены [патч и инструкция восстановления](contrib/frixit/drivers/README.md).
Старый рецепт драйвера 4.23.0 в `contrib/mechrevo` — отдельная историческая база.

`uccd` выполняет аппаратные операции от root; GUI, трей и агент работают от
пользователя. Обычное управление разрешается Polkit активной локальной сессии.
Одновременно должен работать один контроллер: служба конфликтует с TCC/tuxedo-power.

## Проверка и известные ограничения

Для 0.3.0 публикуется [отчёт о проверках](docs/TESTING.md), отдельно от старых
дизайн-аудитов. Проверены восстановление драйвера/Lightbar на текущем ядре и
переключение рабочих столов кликом по трею. Настоящий цикл suspend/resume и вся
матрица оборудования ещё требуют проверки. Полный старый GUI-аудит имел тайм-аут;
его результат не подменяется результатами отдельных успешных проверок.

## Авторы и лицензия

Личный ControlCenter и объединённый форк — **Frixit**.
Основа интерфейса и backend — **[noteMASTER11/UCC-Revorked](https://github.com/noteMASTER11/UCC-Revorked)**,
предыдущая основа — **[nanomatters/ucc](https://github.com/nanomatters/ucc)**.
Используются TUXEDO IO и отдельные драйверы TUXEDO. История upstream и авторские
уведомления сохранены: [PROJECT_ORIGINS.md](PROJECT_ORIGINS.md),
[THIRD_PARTY.md](THIRD_PARTY.md).

**GPL-3.0-or-later.** Полный текст — [COPYING](COPYING).
