# Актуальная галерея Frixit Control Center

Снимки для версии **0.3.0**, 22 октября 2026 года, находятся в `current/light`
и `current/dark`. 10 страниц приложения и отдельный пример driver warning.
Старая галерея UCC и устаревшие verification captures удалены из текущего дерева;
исторические файлы доступны через Git. Дизайн-макеты в `docs/design/references`
помечены как исторические концепции и не являются скриншотами релиза.

Это Qt Widgets, собранный из текущего кода, с тестовым D-Bus-демоном.
Числа, профили и имя GPU-процесса — fixture, не реальные пользовательские данные.
Изображения не генерируются нейросетью. Используется offscreen-платформа Qt;
оформление оконного менеджера не включено. Снимок About может показывать коммит
сборки до финального коммита галереи; точная версия сохранена в manifest.

## Повторное получение

Соберите `test_gui_audit` с `BUILD_TESTS=ON`, затем из корня проекта:

```bash
for theme in light dark; do
  UCC_AUDIT_THEME="$theme" UCC_SCREENSHOT_DIR="$PWD/screenshots/current/$theme" \
    dbus-run-session -- bash tests/run_gui_audit_test.sh \
      "$PWD/build/bin/test_gui_audit" renderReleaseGallery
done
```

Тест выделяет временный HOME, заменяет system bus приватной сессией,
проверяет отсутствие команд записи и не подключается к аппаратному демону.
Для воспроизводимого набора файлов используйте `contrib/frixit/capture-gallery.py`:

```bash
python3 contrib/frixit/capture-gallery.py --build ./build
```

Скрипт сохраняет только заявленные 22 PNG и `current/manifest.json` с SHA-256,
версией Qt, хешем исходников и параметрами съёмки. Журналы сохраняются локально
в `artifacts/gallery-0.3.0/`.
