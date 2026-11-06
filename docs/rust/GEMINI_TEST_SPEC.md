# Обязательная проверка всего Rust Control Center

Требование пользователя: тесты всего центра, с двумя независимыми потоками ревью/тестов. Этот документ задаёт минимальную полноту. Он дополняет, а не заменяет существующие C++/Python/Node/GJS tests и аппаратную матрицу основного плана.

## 1. Что означает «тесты всего»

1. Все F01–F44 имеют поведенческие assertions, положительные и ошибочные сценарии.
2. Каждый из 122 D-Bus methods и 7 signals имеет индивидуальный контракт и проверку фактического поведения.
3. Каждый существенный public API, преобразование данных, range/unit conversion, state transition и failure path проверяется напрямую или через public behavior.
4. Все 10 GUI pages и каждый их control/action проверяются на эффект, ошибки и сохранение данных.
5. Каждый binary/command/helper/maintenance path проверяется как запускаемый продукт, а не только как внутренний Rust-модуль.
6. Дистрибутивные packages/resources/upgrade/downgrade/rollback проверяются в staged/isolated окружении.
7. Отдельно выполняются необходимые аппаратные, compositor, реальный sleep и длительные проверки. Fake tests не заменяют их.

Покрытие каждой тривиальной строки getter отдельным тестом не является целью. Цель — отсутствие непроверенного значимого поведения. Для каждого такого поведения должна существовать связь requirement → production path → test → evidence → reviewed candidate.

Это не математическое доказательство отсутствия любых багов и не перебор бесконечного множества входов. Граничные значения, эквивалентные классы входов, property/fuzz/race checks и независимый review дают осмысленную полноту.

## 2. Начальные реестры

| Реестр | Содержание | Первоначальный статус |
|---|---|---|
| `gemini-feature-tests.json` | 44 области и 171 минимальный составной assertion | planned |
| `gemini-dbus-contracts.json` | 122 methods × 8 категорий cases; 7 signals × 5 категорий | planned |
| `gemini-task-seed.json` | 120 задач M0–M13 | 118 planned; M0.1/M0.2 — сохранённая ранее база/инвентаризация |

Категория cases — не уже написанный test. Каждый составной assertion разбивается на конкретные cases. Parametrized test допускается, если он действительно вызывает каждый API entry и проверяет его expected state/side effects. Нельзя объявить одинаковый ответ всех fake methods покрытием 122 production methods.

При чтении исходников добавлять отсутствующие cases и сохранять прежние ID. Новые aliases/исправленные методы, например SetWebcam, добавляются явно; legacy inventory не уменьшается ради green.

`not_applicable` допустим только для категории, которая не относится к данному методу/сценарию, с письменным R1 rationale. Нельзя сделать весь обязательный feature/API entry not_applicable из-за нехватки времени, железа или неработающей реализации.

## 3. Уровни тестов

| Уровень | Объект | Что наблюдать | Основной владелец |
|---|---|---|---|
| L0 | Pure domain/types/algorithms | Конкретный output, state, invariants, independent golden | W автор + R2 независимые cases |
| L1 | Persistence/IO adapters | Bytes/files/permissions/write order/errors/readback | W + R2 |
| L2 | D-Bus/client/controller integration | Real dispatch, signatures, auth, effects, signals | R2 |
| L3 | Binaries/daemon/session processes | Exit/stdout/stderr/lifecycle/IPC/recovery | R2 |
| L4 | GUI/tray/input/visual integration | Real user action → API → confirmed UI/persistence | W2 + R2; R1 parity |
| L5 | Packages/update/rollback | Staged files/service states/deps/settings preservation | R2 |
| L6 | Hardware/compositor/extended run | Actual hardware response, energy, sleep, BLE/activation | Согласованный этап, R2 протокол |

Успешный L0 не закрывает L2/L4/L6. Можно закончить offline implementation при pending L6, но нельзя назвать полный миграционный release проверенным.

## 4. Восемь обязательных классов сценариев

Для каждого substantive ticket составить applicability map:

| Класс | Примеры |
|---|---|
| Happy path | Правильный input даёт правильный effect и подтверждённый ответ |
| Boundary/invalid | min/max/±1, пустой/duplicate/oversize/malformed input |
| Unsupported/absent | Нет датчика/драйвера/возможности/optional library/service |
| Failure/partial failure | Write отказ, readback mismatch, disk full, auth reject, timeout |
| Persistence/lifecycle | Save/load/restart, start/stop/reconnect/sleep/resume |
| Ordering/concurrency | Два клиента, profile change during command, stale response, owner loss |
| Authorization/readonly | Правильный sender/action, deny, preview dispatch guards, no side effects |
| Compatibility | Old/new JSON/DBus/CLI/codec/IDs и documented intentional fixes |

Пограничные integer cases: minimum−1/minimum/minimum+1, interior, maximum−1/maximum/maximum+1, overflow и sign conversion. Float: zero, exact midpoint, finite positive, negative/missing sentinel, NaN/infinity, tolerance justification. Collections: empty, one, sparse, duplicate, reordered, maximum permitted, maximum+1.

## 5. Test harness и отсутствие побочных эффектов

- Sysfs/procfs roots инжектируются в adapters. Harness создаёт temporary tree и write journal.
- Если test должен использовать fake root, отсутствие fake root вызывает отказ, а не fallback на live `/sys`.
- D-Bus интеграция использует отдельную private bus; test runner проверяет bus identity перед запуском сервисов. Нельзя временно занять production system-bus name на рабочей системе.
- Clock/timer tests используют fake monotonic time; реальное ожидание нужно только там, где проверяется runtime/subprocess boundary.
- Process tests создают собственные child processes, знают их PID/startTime/UID и гарантированно убирают их после test. Нельзя выбирать случайные NVIDIA процессы пользователя.
- CommandRunner записывает program + отдельные arguments/env и умеет имитировать error/timeout/output. Строковый shell runner не используется для аппаратных действий.
- Fake BlueZ exposes реальные согласованные interfaces/characteristics/events; не подменяет весь проверяемый cooler state machine одной функцией `success`.
- Native GPU backend имеет fake symbols/error codes; проверка фактического ABI и настоящей библиотеки выделена отдельно.
- Config/user prefs/log tests устанавливают временные XDG roots до создания процессов и проверяют отсутствие изменений вне них.
- Каждый параллельный worker имеет свой bus, roots, temp paths и evidence dir. Global environment mutations не гоняются между тестами.
- При timeout тест завершает только свои процессы; cleanup не использует широкие `pkill`/`killall`.

## 6. Core, политика и профили

Проверить полный finite decision space, а не несколько любимых режимов:

- Physical source AC/Type-C/Battery/unknown + manual auto/ac/type_c/bat.
- Нулевой/один/несколько request owners, informational и каждый force rank, независимый порядок регистрации.
- Cooler disconnected/connected, power_wc assignment present/missing.
- Type-C assignment present/missing, профиль отсутствует/повреждён/built-in/custom.
- Revision меняется в соответствии с контрактом, старое решение не перезаписывает новое.
- Limits requests: 32 на owner/256 всего, name length 128/reason 512 и границы рядом.
- CRUD: immutable built-ins, copy new identity, references to keyboard/fan profiles, delete active/referenced item.
- Save отдельно от Apply; temporary changes/revert не меняют сохранённое незаметно.

Property tests должны проверять invariants: manual priority, order independence, no orphan owner, valid selected CPU set, bounded fan duty, monotonic accepted curves по применимому контракту. Expected не вычисляется повторным вызовом production evaluate.

## 7. Все данные и форматы

Для каждого settings/profile/INI/import поля: read, write, semantic round-trip, missing/default/null/invalid, значение на границе, неизвестное поле по legacy policy. Ссылки IDs, embedded tables и global charging проверяются вместе с dependent profile.

Обязательные regression cases:

- Непустой keyboardBacklightStates не исчезает из-за повторного ключа serializer.
- Все `enabledCores` сохраняются как точные sparse IDs, не превращаются в count.
- Built-in ID со словами/скобками не отвергается UUID-only type.
- Пользовательские RGB/brightness и missing zones не сдвигаются.
- Legacy per-profile charging fields читаются, но performance profile не меняет global charging.
- YCbCr cards/ports не теряются при parse непустого input.
- JSON escaping имён/description, Unicode/кириллица и string/map/array representations старых данных.
- Повреждённый основной settings, валидный/повреждённый backup; backup не затирается испорченной версией.
- Atomic write failures до/после temp creation/flush/rename; recovery не объявляет успешный fsync при отказе.
- Concurrent saves serialized; permissions/symlinks/path traversal/size caps проверены.
- User prefs читаются с правильного XDG root; normal и preview не портят настройки друг друга.

## 8. Linux hardware, CPU и devices

Проверить Clevo/Uniwill/Dummy, exact DMI/near misses, device unavailable/disappears/reappears. Ioctl numbers сравнить с reference на target ABI, включая pointer-sized legacy encoding. Buffer sizes/layout/alignment/owned FDs/native endianness проверяются отдельно от алгоритмов.

CPU cases:

- Sparse CPU IDs и offline present CPU, mandatory CPU без `online` file.
- Пустой/duplicate/unknown enabledCores; приоритет над onlineCores.
- Enable before disable; failure первого/среднего enable не приводит к отключению оставшихся рабочих CPU.
- P/E/unknown labels, affinity restoration при success/error; no offline enabling ради CPUID.
- Shared cpufreq policies, отсутствующий node, разные base frequencies P/E при noTurbo.
- Min/max writes в admissible order, governor/EPP support, retry reset и restoration на stop.

Device cases:

- Все три колонки, сохранение disconnected entries и user edits при rescan.
- Reject arbitrary sysfs/procfs/service/network names, traversal и invalid values.
- Partial apply возвращает per-device results и не маскируется overall success.
- NVIDIA wake/persistence/powerd order, disable keep-awake/restart/AC→Battery/Type-C.
- Policy disable прекращает дальнейшие device writes; no fictitious rollback.
- Current source/profile revision change во время queued apply отменяет obsolete writes согласно контракту.

## 9. Охлаждение, BLE и освещение

Fan tests используют независимые temperature traces: below first/exact point/midpoint/above last, C++ half rounding, asymmetric EWMA, hysteresis, critical boundaries 84/85/89/90, stale/missing sensor, sameSpeed, minimum/off capabilities. Raw critical fail-safe, если изменён, получает отдельное intentional difference.

BLE tests:

- Каждый production packet и model/command имеет golden byte fixture.
- Отсутствующий/поздний adapter, неизвестный/ambiguous saved device, missing service/characteristic/notify.
- Каждый transition и timeout state machine, backoff min/max, known-device reset, keepalive misses.
- Writes ordered/gap ≥80ms, disconnect cancels generation, repeated values coalesce допустимым образом.
- Pump 0/2/3/4 accepted, 1 rejected; RGB/modes/ranges проверены.
- Failed send не обновляет confirmed manual state; reconnect восстанавливает manual/auto правильно.
- Sleep/disable во время discovery/connect/write; stale callback не оживляет выключенное соединение.
- BLE latency и recovery не задерживают внутренние вентиляторы и shutdown.

Lighting tests:

- White/zone/per-key discovery, sparse physical IDs, calibrated keys и all-key writes.
- Buffer guard on error/cancellation; brightness hardware range vs UI percent.
- Пять Lightbar animations: фиксированные frames, начало/середина/конец и состояние после завершения.
- Flash off after save/start/mode switch, welcome once, resume no-repeat, Silent/Test semantics, no overlap.
- Auto majority color с black/LED0/ties/missing data; invalid colors/settings не пишут IO.

## 10. Telemetry и binary history

Проверить отдельно CPU, each iGPU/dGPU/backend; no CPU-temp substitution for missing GPU sensor и no iGPU substitution for sleeping dGPU. Нулевая частота в простое валидна.

- RAPL counter wrap/range/time delta, µ-units, current×voltage и ошибки чтения.
- i915 и xe paths, own GPU hwmon, RAPL domain name вместо fixed index.
- Optional NVML/NvAPI libraries/symbol versions/permission/unsupported/device loss.
- Runtime status active/suspending/suspended/unknown; запрещённый probe должен выявляться call counter с expected zero.
- Poll cadence при auto и cache clearing; initial init wake measured separately.
- 10 metrics, order/IDs, inclusive `since`, horizon 60/1800/7200 и outside bounds.
- Codec native-endian `u8/u32/(i64,f64)`: golden bytes, empty series, truncation каждого сегмента, huge count/multiplication overflow, unknown metric, non-finite payload policy.
- Monotonic scheduling vs wall timestamps; clock jumps не разрушают query/eviction assumptions.
- Long run: bounded history/queue/log allocation. Leak check не равен одному разовому RSS measurement.

## 11. D-Bus — каждый метод и сигнал

Для каждого entry `DB-Mnnn` R1 читает implementation и определяет mutation/auth class; нельзя классифицировать только по `Get`/`Set`. Например, `ConsumeModeReapplyPending` меняет состояние, хотя не начинается с Set.

Восемь case categories:

1. **wire_shape:** exact name/path/interface/in/out signatures и introspection; no accidental Rust name conversion.
2. **valid_behavior:** конкретный request, ожидаемые state/files/IO/signal/result, не только reply exists.
3. **invalid:** wrong arity/type, malformed/oversize JSON и поля/ranges по параметрам.
4. **authorization:** allow/deny/challenge/error/owner loss по действующей policy; никаких writes до successful auth.
5. **backend_failure:** absent/unsupported/write false/readback mismatch/timeout и корректный внешний result.
6. **lifecycle:** applicable save/load/restart/concurrent clients/connection ownership/cancel.
7. **preview:** mutation запрещена и побочные эффекты отсутствуют, в том числе indirect paths.
8. **client_handling:** unavailable/error/false/malformed/timeout/reconnect не становятся успехом в CLI/GUI.

Для `DB-Snnn`: exact signature/payload, emission после подтверждённого изменения, отсутствие ложного emission после отказа/stale reply, reconnect subscribers и определённый order. Если сигнал семантически сообщает pending/desire до IO, тестировать именно документированный контракт, а не навязывать постфактум hardware confirmation.

Запустить cross-version matrix: new Rust client ↔ baseline fake/реальный изолированный service, baseline client ↔ new Rust daemon fake hardware, new ↔ new. Static header inventory alone не доказывает runtime export.

## 12. CLI, binaries и maintenance

Инвентаризировать фактический dispatcher, а не только устаревший man. Каждая command/alias проверяется subprocess через production binary. `--help`/`--version` не требуют daemon; argument errors/connection failure/rejected operation имеют ожидаемые codes/stdout/stderr.

Покрыть status/monitor/profile/statemap/fan/keyboard/brightness/webcam/fnlock/watercooler/charging/cpu/gpu/power-limits/power/devices/lightbar. Для flags учитывать порядок `--json`, monitor count/interval/interrupt и GUI `--display N`/import/preferences-only/show/minimized.

Helper tests: daemon flags/singleton, hardware audit/fan-auto exact DMI gate, sleep prepare/resume/root gate на изолированных dependencies, import-check отказ при неверных curves/IDs/references.

Import/tools tests: UCC/TCC/Python fixtures, static ASAR bounds/no eval, actual source curves/modifiers/rounding, unresolved report, new output directory refusal, private permissions, checksums, changed-source refusal, staged installer failure/rollback. Existing-install upgrade — отдельный scenario, не повтор historical first-install поверх UCC.

## 13. GUI: каждое действие в каждом разделе

R2 создаёт `gui-status.json` со списком каждого control: page/control ID, old source/caller, action, expected API, confirmed effect, failure result, persistence, preview, keyboard/input и evidence. Минимум:

| Раздел | Обязательные действия |
|---|---|
| Overview | CPU/iGPU/dGPU metrics, profile selection, driver warning/recovery, quick cooler actions |
| Profiles | Create/copy/rename/delete, Save/Apply, assignments AC/Type-C/Battery/WC, CPU modes/P/E masks, governor/EPP/freq/HWP, display Hz/brightness, subprofile refs, ODM/cTGP |
| Cooler Settings | CPU/GPU nearest-point drag, reverse drag, release commit, profile switch mid-drag, Apply/Revert/Save/copy/delete, sameSpeed |
| Watercool Settings | enable/discovery/connect/error, manual/auto fan/pump, LED controls, WC fan/pump curve editors |
| Monitor | 10 series, grouped/unified, toggles/time window/persistence, zoom/pan/markers, units/axes, history updates, hidden-page throttling |
| Keyboard & Hardware | hover/single/multi selection, apply selected/all, colors/brightness, profile CRUD, calibrated IDs, FnLock/webcam/display capabilities |
| Power & Devices | Three independent columns, CPU overlay, Keep awake, filter/rescan, save/apply, missing device/status/errors |
| Lightbar | Three modes/settings, color/brightness/animations, Silent/Welcome/Test, saved vs draft and failed save |
| Battery & Processes | Global charging mode/priority/limits, custom current, battery summaries, process selection/confirmation/results, desktop prefs |
| About | Exact build/version/commit, credits/notices, logs toggle/folder, errors/rotation |

Общие сценарии:

- Loading/offline/disconnected/reconnect; delayed/error/false reply.
- Draft не затирается monitor update; changed revision invalidates stale response.
- Rapid navigation/profile/theme changes, hidden/minimized state.
- Normal/preview, new/existing/concurrent GUI, startMinimized/--show/token.
- Both themes/Auto, 100/125/150/200% DPI, кириллица/IME, keyboard-only focus/activation и доступность ключевых controls.
- Selection/hover/render/opening page сами по себе не пишут hardware/config.
- Actual action test проходит через production update/view/IPC boundary; один вызов update model не закрывает реальный mouse drag или Wayland activation.

Для curve/keyboard widgets нужны geometry/hit-test/model tests и настоящий input integration; для visual regression — reference images и осмысленный diff. Допустимые renderer/font tolerances фиксируются; нельзя принять любой diff слишком большим threshold.

Если harness не умеет управлять native GUI, сначала выбрать/реализовать поддержанный driver или проверяемые test hooks с R1 review. Не утверждать наличие GUI tests по одному screenshot или тесту внутренней структуры.

## 14. Desktop и session integration

- System batteries/µ-units/fallback, no-device batteries, unknown values.
- NVIDIA fd process discovery, same UID/startTime/pidfd, controlled child escalation and cancellation.
- Notification observer: only method events, fragmented/oversize lines, rate limit/restart, никакого хранения foreign message text.
- Driver/source alerts: launch delay/debounce/no startup false change/recovery/retry, unsupported notification server.
- KDE/Hyprland/X11: actual available mode IDs, resolution/scale/position/color rule, profile automatic vs offer action-only, stale consent invalidation, repeated readback.
- Tray: dynamic menu/icon/manual marker/errors/activity polling/host restart, ability to work without GUI.
- Window singleton/token: same desktop restore, existing/new window, invalid/expired/missing token, user explicit click vs automatic start.
- Sleep signal during pending command kills stale profile/display jobs and recovers cleanly.

Native desktop checks require the corresponding DE session. Private D-Bus mock tests verify logic, not compositor policy. Unavailable DE remains explicit pending for the support claim it gates.

## 15. Package, systemd и переход

Обязательные suites:

- Clean normal/preview builds for согласованные feature combinations; не использовать `--all-features` вслепую, если features взаимоисключающие.
- Headless daemon/CLI without GUI; no-NVIDIA/no-BLE optional environment; user UI without root.
- Stage exact binaries/icons/man/completions/autostart/DBus/Polkit/units/licenses/source metadata.
- Verify Type=dbus/readiness/BusName/ExecStopPost/fan-auto/sleep alias/writable sysfs/namespaced paths.
- Arch/DEB/RPM/Nix declared support has matching build/stage/install checks, including Nix store executable/helper paths.
- Upgrade preserves settings/private→public transition, enables only previously intended hooks and honors masks.
- Downgrade/rollback after each meaningful failed install/start/validation step restores previous files/package/service/user states.
- Package does not contain private Data/MAC/logs/probe binaries/old runtime bridges by accident.
- Runtime link/process inspection proves main app independence from own Qt/C++/Python backend; external vendor libs are listed honestly.
- Source/tag/version/Cargo/package version/checksum/license notices correspond to exact candidate.

## 16. Автоматические команды и quality gates

Команды ниже — пример обычного Cargo baseline, **не отчёт об уже выполненном запуске**. Конкретные feature/test target flags определяются workspace и ADR. Сначала проверить tools/help и реальные targets; не создавать «успешные» скрипты без реализации suite.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

Это не полный test plan. Добавить private-bus, preview, feature matrix, UI, contract, staged package и hardware suites. Если suite реализован в `xtask`, его имя должно существовать и запускать реальные checks с корректным exit code.

Не маскировать failure через `... | tee ...` без сохранения exit статуса исходной команды. Отдельно писать exit code и log hash. Timeout возвращает failed/timed_out, не passed. Отсутствующий compiler/display/dependency — environment_blocked.

Coverage targets как дополнительный gate после настройки инструмента:

- Pure core/config/policy/codec: стремиться не ниже 95% executable lines; все существенные decisions и negative cases обязательны независимо от процента.
- Hardware/desktop state machines: стремиться не ниже 90% собственных executable lines через fake/integration tests; настоящая FFI/driver часть имеет отдельный evidence.
- UI: обязательны все registered controls/actions и error flows; общий line percentage не заменяет этот gate.
- Branch coverage, если поддерживается выбранным toolchain, собирается отдельно; не заявлять её по line coverage.
- Exclusions допускаются только для generated/third-party/non-executable кода с R1 rationale. Сложный собственный код нельзя исключить, чтобы поднять процент.

Недостижение численного target не скрывается. R1/R2 проверяют непокрытые строки/decisions, добавляют substantive tests или обосновывают конкретный unreachable case. Цифра может уточняться по обоснованному ADR, но обязательное feature/API coverage не ослабляется.

## 17. Проверка силы тестов

R2 должен показать, что критические regression tests умеют падать. Примеры изолированных мутантов:

- Manual priority ниже app request.
- Type-C fallback ошибочно выбирает Battery.
- Boolean false превращён в success.
- CPU ID заменён row index или disable выполняется до enable.
- LED ID 56/21/0 перепутаны.
- NVML probe вызван для suspended GPU.
- Убран owner-disconnect cleanup.
- Flash оставляет ненулевой idle.
- Старый callback применён к новому профилю/соединению.
- pidfd/startTime recheck пропущен.
- Global charging снова зависит от performance profile.
- Preview запрещает кнопку, но допускает прямой dispatch.
- Binary count multiplication не проверена на overflow.
- Installer забывает восстановить masked/disabled состояние.

Мутанты живут только в отдельной test copy/worktree, никогда не в integration. Тест должен падать по нужному assertion, а не по syntax/compile error. Property/fuzz/mutation checks дополняют review, не заменяют его.

## 18. Реальные аппаратные и длительные проверки

Перед live stage: accepted candidate, package/staging/offline suites, backup и рабочая rollback procedure, один writer, конкретные разрешённые действия. Обычные тесты не должны неожиданно вызывать sleep/reboot/Bluetooth reset или signals пользовательским приложениям.

Минимальная приёмка — раздел 13 основного плана: power transitions, manual/requests/WC priorities, CPU selection, internal cooling under load/idle, cooler power-cycle/reconnect/auto/manual, RGB/Lightbar, GPU runtime suspend/idle consumption, screen/brightness/MUX, global charging, singleton/autostart, sleep/hibernate/resume, crash recovery, reboot/login, update/downgrade и 24–72h mixed soak по согласованному протоколу.

В протоколе: candidate/package hashes, ноутбук/DMI/ядро/драйвер/DE, исходное состояние, действия, фактические измерения/readback/logs, результат каждого case и restoration. Наблюдение пользователя записывается как manual evidence с точным scope, не как автоматический suite.

## 19. Итоговая таблица принятия

Для каждого ticket и финального candidate нужны:

| Проверка | Разрешённые итоговые статусы |
|---|---|
| Implementation | implemented / missing / partial |
| R1 review | accept / changes_requested / blocked_missing_evidence |
| R2 automatic checks | passed / failed / environment_blocked / not_run |
| Hardware/compositor | passed / manual_verified / hardware_pending / failed / not_applicable с обоснованием |
| Integration candidate | verified exact SHA / unverified / stale_after_change |

`done` разрешён только после всех applicable required checks. При pending final hardware/environment gate можно отчитаться «offline часть проверена, финальная приёмка не завершена». Нельзя отчёт о таком состоянии озаглавить «всё переписано и полностью проверено».
