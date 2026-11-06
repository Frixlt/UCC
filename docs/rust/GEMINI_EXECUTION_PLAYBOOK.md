# Порядок исполнения Gemini: волны, четыре роли, интеграция

Этот документ конкретизирует [MIGRATION_PLAN.md](MIGRATION_PLAN.md). Прочитать его вместе с [master prompt](GEMINI_MASTER_PROMPT.md), [roles](GEMINI_AGENT_PROMPTS.md) и [test spec](GEMINI_TEST_SPEC.md). Vn соответствует Mn основного плана; все Mn.k из `gemini-task-seed.json` сохраняются и дробятся при необходимости.

## 1. Первая сессия — действия по порядку

1. Найти repo, прочитать инструкции, выполнить `git status`, определить HEAD/ветку/remotes и незакоммиченные пользовательские изменения.
2. Убедиться, что доступна исходная база `45921b0`; новая документация не повод делать checkout/reset назад.
3. Проверить, что `ControlCenter-Python` доступен, либо отметить отсутствие исторического reference, сохранив работу с текущим UCC.
4. Обнаружить реальные agent tools, выяснить concurrency cap и назначить четыре роли. Записать IDs, не создавать дополнительные recursive agents.
5. Прочитать сохранённый `execution/state.json`, если он уже есть. Создавать новую пустую историю разрешено только при её реальном отсутствии.
6. Скопировать initial registries в execution; completed M0.1/M0.2 относятся только к уже сохранённой базе/статической инвентаризации.
7. Выбрать integration branch. Если она уже существует — продолжить её; если нет — создать отдельную migration branch от текущего проверенного HEAD.
8. Создать W1/W2/R2 worktrees с отдельными ветками от integration base. R1 может читать их, не меняя production. Пользовательский текущий checkout не очищать.
9. Назначить ownership общих файлов, source/test fixtures и evidence paths. До этого два агента не начинают редактировать один manifest.
10. Запустить V0. Сначала получить действительный baseline build/test report, затем писать Rust foundation/prototypes по V1.

Допустимый пример имён веток: `rust/migration`, `rust/w1`, `rust/w2`, `rust/r2-tests`. Это пример, не право перезаписать одноимённые существующие ветки. Worktree находится в отдельной рабочей папке, а не в Git metadata другой checkout.

## 2. Состояние длительной работы

Создать следующую отслеживаемую структуру. Схемы и документы появляются как часть исполнения, а не как фиктивные результаты заранее:

```text
docs/rust/execution/
  state.json                 состояние coordinator/integration/roles/current wave
  backlog.json               все Mn.k и child tickets/dependencies/status
  ownership.json             path → role/ticket, общие файлы и ограничения
  feature-status.json        F01–F44 → implementation/tests/review/HIL
  dbus-status.json            122 methods + 7 signals → cases/results
  gui-status.json             10 pages → controls/actions/visual/input/errors
  findings.json               открытые/исправленные/перепроверенные findings
  intentional-differences.json
  environment.json           toolchain/dependencies/DE/driver availability
  decisions/                 ADR с accepted/pending/superseded статусом
  reports/                   компактные agent/review/test summaries
  RESUME.md                  точные next actions и ограничения
artifacts/rust-migration/     ignored logs, images, private fixtures, binaries
```

Из всех файлов `execution` только координатор меняет shared registries. Агенты присылают структурированные отчёты; он проверяет и переносит их, чтобы не было конфликтующих JSON writes.

Минимальный `state.json`:

```json
{
  "schema_version": 1,
  "objective": "Complete Frixit Control Center Rust migration",
  "baseline_sha": "45921b0b7021448a81575a840b2b77d93da8b298",
  "integration_branch": null,
  "integration_sha": null,
  "wave": "V0",
  "status": "not_started",
  "roles": {
    "W1": {"agent_id": null, "ticket": null, "worktree": null},
    "W2": {"agent_id": null, "ticket": null, "worktree": null},
    "R1": {"agent_id": null, "ticket": null, "worktree": null},
    "R2": {"agent_id": null, "ticket": null, "worktree": null}
  },
  "open_decisions": [],
  "environment_blockers": [],
  "next_ready_tickets": [],
  "last_verified_integration_sha": null
}
```

`null` заполняется реальным значением после действия. Не подставлять вымышленный agent ID, SHA или зелёный status.

Карточка дочернего ticket содержит: ID/parent Mn.k/features/API IDs, dependencies, owner/worktree/base SHA, allowed paths, contract revision, inputs/outputs, acceptance assertions/test IDs, implementation SHA, R1 verdict/SHA, R2 evidence/SHA, integration SHA, status, blockers и next action.

Evidence — не один boolean `tests_passed`. Для каждой команды сохранять argv/cwd/environment, source SHA/tree/dirty, duration, exit code, passed/failed/skipped counts, log path/hash и конкретный scope. Воспроизводящий test failure сохраняется даже после успешного исправления, со ссылкой на fix SHA.

Избегать бесконечного цикла «записали SHA результата → новый commit → снова всё устарело». Сначала создать clean product candidate и проверить его; затем отдельным evidence/docs commit записать результаты, ссылающиеся на проверенный product SHA. Если второй commit меняет только отчёты/документацию, зафиксировать неизменность production/test/build/resource tree fingerprints и не выдавать его за новый runtime. Если он меняет код, tests, dependency lock, assets, build или package resources, нужные checks повторяются. SHA нельзя подставить в собственный commit заранее; использовать checked source SHA и отдельный recorded-in commit.

## 3. Общие правила волн

- Одновременно W1/W2 работают над двумя **независимыми ready tickets**, а не разрывают один тесно связанный файл.
- R1/R2 имеют полезную очередь с начала: source analysis, test design, fixtures, independent tests, review, aggregate regression. Они не простаивают до последнего дня.
- Раз в цикл координатор принимает сообщения всех ролей, проверяет blockers, фиксирует ownership/dependencies и выдаёт следующие задания.
- Ревьюировать можно по компоненту; интегрировать — только после author checks и первичного R1/R2 verdict. Final verification делается на объединённом SHA.
- Переход на следующую волну требует её dependencies. Независимые prototypes/UI models можно делать заранее, но это не закрывает более раннюю непроверенную волну.
- Один failure не запускает бесконечные одинаковые retries. После воспроизводимого отказа диагностировать причину, изменить код/среду осмысленно, повторить нужный check.
- Если dependency blocked, зафиксировать причину и выбрать независимую ready задачу. Не заменять отсутствующий backend constant-success stub.
- Не проводить одновременно несколько тяжёлых C++/Rust builds на машине с ограниченной RAM. Согласовать build budget; параллельность ролей не означает неограниченное `-j`.

## 4. План работы по волнам

### V0 / M0 — подтверждённый эталон

**W1:** собрать C++ baseline normal/preview в отдельных build dirs; описать flags/dependencies, доступный daemon/backend API. Проверить, какие источники реально входят в targets.

**W2:** инвентаризировать GUI/CLI/session controls, fixtures/INI/resources/launch flags; составить карту десяти страниц и текущих shell controls. Подготовить обезличенные inputs без записи личных настроек.

**R1:** сверить 122 declarations с implementation/callers и описанными defects; выделить реально неработающие paths и intentional-difference candidates. Проверить baseline provenance.

**R2:** воспроизвести existing CTest/focused GUI/Node/GJS/Python suites по доступности, получить real exit codes/logs. Проверить private bus/fake roots и различие existing fail vs unavailable.

**Artifacts:** environment/baseline reports, runtime introspection XML, fixtures, GUI/CLI inventories, initial measurements. Исходный hash manifest остаётся неизменным.

**Gate:** известна воспроизводимость эталона и все baseline failures. Отсутствие GJS или нужного DE указано как ограничение, не как passed.

### V1 / M1 — выбрать технологии на прототипах

**W1:** Cargo foundation и shared types draft; ioctl number/layout harness; minimal fake-DBus server; NVIDIA/BlueZ dependency coverage notes.

**W2:** GUI shell + interactive curve/keyboard/chart prototypes; tray menu/activation path. Прототипы не получают права live hardware control.

**R1:** проверить architecture/ABI/ownership/native library boundaries и выбор GTK/Qt-free основного runtime. Сверить packet/symbol coverage и GUI dependency assumptions.

**R2:** независимые prototype checks: fake BlueZ, XML/signatures, Canvas input/race, glyph/IME/DPI/theme, native Wayland/X11/tray token по доступной среде.

**Artifacts:** принятые ADR: GUI, async/actors, FFI, BLE, shell policy; proof-of-concept outputs с ограничениями.

**Gate:** кандидат toolkit прошёл нужные трудные сценарии либо заменён обоснованно. Shell embedding decision может остаться pending до ответа пользователя; это не блокирует core/daemon работу, но блокирует удаление shell UI и final release claim.

### V2 / M2 — предметная модель и persistence

**W1:** `ucc-core`: profiles/types/units/capabilities/defaults/power decisions/request ownership; freeze contract на границе config.

**W2:** `ucc-config`: legacy JSON/INI/autosave/Aquaris/sleep formats, atomic save/backup/recovery; опираться на frozen core contract.

**R1:** сверить все поля/IDs/optional/default/sentinel/unknown semantics; проверить CRUD references, Type-C fallback, global charging separation.

**R2:** property/differential/round-trip/invalid/duplicate-key/disk-failure tests. Мутанты: поменять mode priority, выбросить unknown field, сдвинуть CPU/LED ID.

**Gate:** fixtures читаются и повторно пишутся без недопустимой потери данных; все F02–F07 и persistence cases имеют evidence.

### V3 / M3 — публичный IPC и CLI

**W1:** server adapters, authorization boundary и command queue в назначенных `ucc-dbus` paths.

**W2:** client proxies/reconnect и Rust CLI со всеми commands/aliases/JSON/exit codes. Общие lib.rs/DTO/Cargo.lock имеют одного владельца.

**R1:** проверить XML/in/out signatures, Polkit mapping, owner identity, bool false vs transport error, client→server callers и intentional aliases.

**R2:** contract harness для всех 122 methods/7 signals; old/new client-server matrix на private bus; malformed/timeout/denied/disconnect cases; CLI subprocess tests.

**Gate:** у каждого API entry есть проверка wire shape и meaningful behavior case, а не одинаковый ответ `{}`. Hardware-dependent methods пока используют **явный test backend**, не fake production backend.

### V4 / M4 — Linux IO, CPU и device policy

**W1:** ioctl detection/Clevo/Uniwill, CPU topology/hotplug/governor/EPP/turbo/frequencies и typed IO traits.

**W2:** device scan/merge/three-mode policy, allowed runners/services/persistence/ASPM/system controls; independent module paths.

**R1:** проверить exact DMI/ABI/pointer-size ioctl, CPUID affinity restore, mandatory CPU, ordering/failure semantics, whitelist и no shell injection.

**R2:** fake sysfs sparse IDs, enable-failure журнал, shared cpufreq policy, driver differences, invalid paths/services, missing devices, persistence ordering.

**Gate:** positive/negative/partial-write/readback cases проходят; драйверные read/write probes на hardware только в согласованной части.

### V5 / M5 — внутреннее охлаждение и BLE

**W1:** fan curves/interpolation/filter/hysteresis/sameSpeed/min/off/critical/stale-sensor policy и независимый fan loop.

**W2:** cooler protocol/state machine/BlueZ transport, UUID/packets/pump/LED/manual/auto/reconnect и ordered command queue.

**R1:** сравнить C++ rounding/step semantics, fail-safe choices, pump code mapping и state transitions/deadlines; исключить unsafe broad adapter resets.

**R2:** deterministic temperature traces и fake clock; fake BlueZ/GATT loss/recovery/late adapter/stale callback; verify ≥80ms gap; crash/shutdown tests с fake recovery.

**Gate:** BLE не блокирует fan loop; manual state обновляется после успешного действия; аппаратный coolant test отдельно pending до проверки.

### V6 / M6 — keyboard и Lightbar

**W1:** keyboard detection/mapping/physical identities/brightness/buffer guard.

**W2:** Lightbar modes/settings/color/frames/timer/silent/welcome/test/idle.

**R1:** проверить sparse LED mapping, calibrated IDs, global brightness units, no shifting missing zones и буфер при ошибке.

**R2:** exact animation-frame fixtures, no-overlap/cancel/suspend/save cases; Flash idle zero; write failure; selection-only no write; physical mapping regression.

**Gate:** все 5 анимаций и 3 power-mode profiles проверены; ни один обязательный LED не потерян из-за нумерации.

### V7 / M7 — telemetry, GPU и history

**W1:** CPU/RAPL/iGPU/AMD/NVIDIA hardware sampling, FFI lifecycle, runtime-suspend guard/cadence.

**W2:** telemetry model/freshness/ring buffers/10 MetricIds/legacy binary codec/query horizon.

**R1:** проверить units, sensor identity, v1/v2 ABI, optional symbols, voltage/OC active subset, wall/monotonic clock semantics и stale value clearing.

**R2:** energy wrap/zero dt/missing/NaN; i915/xe fixtures; inactive GPU must never invoke probe; truncated/overflow/native-endian codec; bounded memory and timeout behavior.

**Gate:** не подставляются iGPU values в dGPU; asleep GPU не будится poll loop; native-call cancellation limitations названы, нужная isolation реализована.

### V8 / M8 — интегрированный daemon и root helpers

**W1:** orchestrator/startup/readiness/command revision/shutdown/signal handling и соединение контроллеров.

**W2:** ucc-sleep, ucc-hardware, ucc-import-check binaries и service resource staging по единому lifecycle.

**R1:** проверить порядок capabilities→settings→profile, partial failures, sender rights, recovery при SIGKILL и дублирование sleep units.

**R2:** subprocess daemon с fake hardware/private bus; restart/burst/auth delay/owner loss/corrupt config; sleep store symlink/ownership/failed-start; old C++ clients against new daemon.

**Gate:** backend работает автономно без GUI/старого daemon; реальное переключение установленного центра пока не требуется.

### V9 / M9 — desktop/tray/session

**W1:** session telemetry/process identity/pidfd termination и notification observer/alerts с test injection.

**W2:** tray menu/icon/activation, session display adapters KDE/Hyprland/X11, profile-Hz vs offers и user singleton/autostart.

**R1:** проверить user/root boundaries, PID reuse/UID, privacy notification event, physical source vs mode, stale consent, token routing и host availability.

**R2:** child-process termination harness, fake notifications/compositor command outputs, action-invoked only behavior, readback failure, generation cancel, tray/GUI launch concurrency.

**Gate:** tray/agent работают без главного окна; unsupported desktop имеет честный результат; shared bus use не ломает other user sessions.

### V10 / M10 — все пользовательские страницы

Распределять страницы W1/W2 по зрелым shared widgets; историческое имя W1 не запрещает ему user-facing задачу после backend waves.

**W1:** Profiles/CPU/charging/power-devices и связанные state models, либо согласованная половина очереди.

**W2:** Overview/Monitor/Keyboard/Lightbar/cooling/About/notifications по независимым tickets. Общие shell/theme/navigation/widgets сначала freeze и один owner.

**R1:** сверить каждую кнопку/редактор/field с old callers; no local-only controls, Save vs Apply, unsupported state, read-only bypass и theme/focus regressions.

**R2:** state-model tests + actual input/GUI integration и visual gallery; delayed answers/failed commands/drag-profile-switch/reconnect/cold launch; обе темы/DPI.

**Gate:** все десять страниц имеют control/action registry и actual action evidence. Screenshot-only acceptance запрещён.

### V11 / M11 — import/tools и shell policy

**W1:** UCC/TCC/Python offline import, static ASAR extraction, parser-check и отчёты/unknown refs.

**W2:** prepare/update/rollback/desktop-transition/capture-gallery tools; исполнение принятой shell integration policy.

**R1:** сверить source priority/offset rounding/IDs/private data и existing-install restrictions; проверить отсутствие тихой потери embedded UI.

**R2:** all converter fixtures, invalid/missing source/new output refusal, path traversal/oversize, install/update failure rollback на staged roots, existing disabled/masked services.

**Gate:** используемые maintenance paths работают на Rust и проверены; старые Python helpers не остаются скрытой runtime dependency.

### V12 / M12 — distribution и CI

**W1:** distro services/DBus/Polkit resources и Arch/DEB/RPM/Nix recipes, correct writable paths/store helper paths.

**W2:** xtask dist/test/check registries, release/source artifacts, version/commit/license metadata, docs/autostart/assets/completions.

**R1:** review staged contents/dependencies/config preservation/driver separation/build features/no-Qt product; match versions and resource paths.

**R2:** clean-env builds/packages, normal/preview, all agreed feature combinations, update/downgrade/rollback, missing optional libraries, test harness isolation.

**Gate:** каждый заявленный supported distribution имеет реальный build/install evidence; experimental/unavailable targets не объявляются проверенными.

### V13 / M13 — финальная приёмка и legacy removal

**W1/W2:** исправляют найденные integration/hardware defects отдельными tickets. Legacy удаляется из active build/install последним шагом, с сохранением Git archive.

**R1:** финальный F01–F44/API/GUI/tool/package audit, проверка всех intentional differences/open findings и shell decision.

**R2:** final candidate suite, согласованный hardware matrix/soak/reboot/login/sleep/rollback и performance/idle power comparisons.

**Gate:** final DoD master/основного плана выполнен целиком; кандидат и пакет имеют проверенные hashes; никакие pending hardware claims не превращены в passed.

## 5. Работа с общими файлами

| Объект | Владелец по умолчанию | Правило |
|---|---|---|
| Root Cargo.toml/Cargo.lock/toolchain | W1 в foundation, затем явно назначенный ticket owner | Одна mutation очередь |
| Core DTO/profile/public traits | W1 + R1 review | Contract revision до dependent implementation |
| DBus interface definitions | W1 server ticket, W2 согласует client use | Exact legacy name/signature tests R2 |
| GUI shell/theme/common widgets | W2 | W1 page tickets используют frozen contract |
| Integration harness/shared fixtures | R2 | W1/W2 добавляют свои unit fixtures без конкуренции |
| CI/package shared resources | Owner V12 | После изменения тестируются paths всех пакетов |
| execution registries/backlog | Coordinator | Агенты присылают отчёты вместо simultaneous edits |

Если production contract невозможно реализовать безопасно, автор подаёт change request с конкретным API/type difference, причиной и affected tickets. После решения обновляются callers/tests/ADR; старый агент не продолжает работу со старым контрактом молча.

## 6. Принятие diff и устранение конфликтов

1. Проверить чистый integration worktree и конкретные source commits W1/W2/R2.
2. Проверить ownership: нет случайного чужого кода/private logs/lockfile race.
3. Принять общие contract commits первыми, затем implementation и tests в определённом порядке.
4. При conflict прочитать обе стороны и контракт, не выбирать `ours/theirs` для всего файла автоматически.
5. Новый merged tree получает новый SHA. Повторить compilation, affected tests и required aggregate suite.
6. Изменение conflict resolution рассматривается как новый diff для R1.
7. Verdicts обновляются на реальный интегрированный SHA. Старый green сохраняется в истории, но не присваивается новому автоматически.
8. После gate commit/push разрешённой migration branch и обновление RESUME/state.

R2 не должен быть единственным автором production fix и единственным независимым проверяющим этого fix. Если ему пришлось изменить production для тестируемости, W1/W2 принимают соответствующий implementation ticket, а R1 делает независимое review.

## 7. Регулярный контроль полноты

В конце каждой волны координатор проверяет:

- Каждый закрытый Mn.k имеет implementation/review/evidence и reference SHA.
- Feature cases не исчезли из реестра; новые обнаруженные behaviors добавлены.
- DBus methods/signals и GUI controls имеют coverage mapping.
- Открытые findings не закрыты словами «не воспроизводится» без записанного сценария/окружения.
- Passed относится к source SHA, not_run/skipped/blocked отражены честно.
- Нет production TODO/stub/constant-success/temporary bridge в заявленном готовом scope.
- Старые interfaces/runtime dependencies не удалены до functional replacement gate.
- Рабочие изменения и artifacts сохранены, resume содержит следующий конкретный ticket.

## 8. Что писать пользователю

Краткий прогресс: волна/tickets, какие два implementation потока активны, что R1 обнаружил, что R2 реально запустил, значимые blockers и следующий шаг. Не перечислять каждую команду без причины и не писать «всё готово», пока финальная матрица не закрыта.

При остановке из-за лимита среды: текущий integration SHA, состояние agents/worktrees, verified features, failed/pending gates и инструкция продолжения. Незавершённая миграция остаётся незавершённой.
