# Протокол общения 4 агентов через ветку `release/sync-board`

Все 4 агента координируют статус, контракты и блокеры через отдельную ветку **`release/sync-board`** в `https://github.com/Frixlt/UCC`.

## 1. Структура ветки `release/sync-board`

```text
sync/
├── README.md
├── agent1.md   # Редактирует ТОЛЬКО Агент 1
├── agent2.md   # Редактирует ТОЛЬКО Агент 2
├── agent3.md   # Редактирует ТОЛЬКО Агент 3
└── agent4.md   # Редактирует ТОЛЬКО Агент 4
```

**Правило 0 конфликтов:** каждый агент изменяет **исключительно** свой файл `sync/agentN.md` (а файлы остальных агентов только читает). Благодаря этому `git pull --rebase origin release/sync-board` всегда проходит автоматически без конфликтов!

## 2. Как читать и обновлять `release/sync-board` (через отдельный worktree)

Чтобы не переключать рабочую ветку разработки, держи `release/sync-board` в отдельном `git worktree` (например, `/tmp/ucc-sync-board`):

```bash
# Однократно при старте агента:
git fetch origin release/sync-board
git worktree add /tmp/ucc-sync-board origin/release/sync-board
cd /tmp/ucc-sync-board && git checkout -B release/sync-board origin/release/sync-board

# Прочитать статусы всех соседей:
git -C /tmp/ucc-sync-board pull --rebase origin release/sync-board
cat /tmp/ucc-sync-board/sync/agent*.md

# Опубликовать своё обновление (только свой sync/agentN.md!):
cd /tmp/ucc-sync-board
git pull --rebase origin release/sync-board
git add sync/agentN.md
git commit -m "sync(agentN): <PHASE> — <краткий статус>"
git push origin release/sync-board
```

## 3. Обязательные чекпоинты обновления `sync/agentN.md`

Каждый агент обязан обновлять свой `sync/agentN.md` в 4 точках:
1. **`STARTED`** — старт работы, базовый SHA `rust/migration`, имя ветки.
2. **`PHASE1_DONE`** — завершение 5 исполнителей, список коммитов, любые вопросы к соседям с тегом `[NEEDS: AgentX]`.
3. **`PAUSED_LIMIT`** *(если упёрся в лимит субагентов)* — что уже запушено, что осталось на Фазе 2, ожидание сообщения пользователя.
4. **`READY_FOR_MERGE`** — Фаза 2 (5 ревьюеров) пройдена, все замечания закрыты, приведены точные цифры тестов (`cargo test`, `ctest`, `validation.rs`, `cargo fmt --check`, `-D warnings`) и финальный SHA ветки.

## 4. Формат `sync/agentN.md`

```markdown
# Agent N — <Название>
- **Branch:** `release/aN-...`
- **Status:** `STARTED` | `PHASE1_DONE` | `PAUSED_LIMIT` | `READY_FOR_MERGE`
- **Updated:** YYYY-MM-DD HH:MM UTC
- **HEAD SHA:** `<sha>`

## Completed
- [x] ...

## Tests & Verification
- `cargo fmt --all -- --check`: PASS
- `RUSTFLAGS="-D warnings" cargo test ...`: N/N PASS

## Cross-Agent Notes
- `[NEEDS: AgentX]` ... (или `None`)
- `[INFO]` ...
```
