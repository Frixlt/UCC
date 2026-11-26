# ADR-03: Политика интеграции с оболочками GNOME и KDE Plasma (M1.9)

## Статус
Принято (Accepted — 2026-11-26)

## Контекст
В историческом проекте существовали:
1. Встроенный C++/QML апплет для панели KDE Plasma (`ucc-tray`), требовавший `ECM`, `KF6Config`, `KF6CoreAddons`, `Plasma` и `libucc-dbus.so`.
2. Внешнее расширение GNOME Shell на GJS (`ucc-gnome`).
3. В целевой системе пользователя основным окружением является Hyprland + Noctalia, а пакеты KDE (`KF6`) планомерно удаляются.

## Решение
1. **Единственный трей в поставке — Standalone Rust SNI (`ucc-desktop-tray`)**:
   - Вся логика системного трея, меню переключения режимов питания, мониторинга батареи и запуска GUI реализуется на чистом Rust в `crates/ucc-desktop` поверх протокола `org.kde.StatusNotifierItem` (`ksni` / `zbus`).
   - Этот трей работает нативно в Hyprland/Noctalia, KDE Plasma, Swaybar/Waybar и в GNOME Shell (через AppIndicator/SNI host).
2. **Отказ от C++/QML Plasma-апплета (`ucc-tray`)**:
   - Сборка и поставка `ucc-tray` окончательно исключаются из целевого пакета: это убирает последние зависимости от `KF6`/Plasma/`QML` и `libucc-dbus.so`.
3. **Внешний GJS-адаптер (`ucc-gnome`)**:
   - Не входит в обязательный Rust-runtime; при сохранении в репозитории остаётся тонким внешним D-Bus клиентом без C++-зависимостей.
4. **Активация окна без `KF6WindowSystem`**:
   - Rust-трей `ucc-desktop-tray` передаёт Wayland activation token (`xdg-activation-v1`) по D-Bus (`ActivateWithToken`), а автономный `Qt6::Widgets` клиент `ucc-gui` принимает фокус штатным `XDG_ACTIVATION_TOKEN` + `QWindow::requestActivate()`.

## Последствия
- Основной пакет полностью свободен от `KF6`, `Plasma` и `QML`.
- Вся фоновая часть трея и активации написана на чистом Rust.
