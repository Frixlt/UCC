# UCC-Revorked v1.0.0 — Меж-агентная доска синхронизации (`release/sync-board`)

Базовый коммит `rust/migration`: `e1fe413`.
Каждый агент редактирует **ТОЛЬКО** свой файл `sync/agentN.md` и пушит через `git pull --rebase origin release/sync-board && git push origin release/sync-board`.
Подробный протокол: `docs/release-v1/SYNC_PROTOCOL.md` на ветке `rust/migration`.
