# Замороженный межмодульный контракт и правила спринта `UCC-Revorked v1.0.0`

**Репозиторий:** `https://github.com/Frixlt/UCC`  
**Базовая ветка:** `rust/migration`  
**Ветка меж-агентной координации:** `release/sync-board` (см. [`SYNC_PROTOCOL.md`](./SYNC_PROTOCOL.md))

---

## 1. Матрица строгого владения файлами (0 merge-конфликтов)

Ни один агент **не имеет права** изменять файлы вне своей зоны владения. Если требуется правка в чужой зоне — опиши запрос в своём файле `sync/agentN.md` на ветке `release/sync-board` с тегом `[NEEDS: AgentX]`.

| Агент | Ветка | Эксклюзивные пути |
| :---: | --- | --- |
| **Агент 1** | `release/a1-kernel-hardware` | `kernel/uwcc-rs/**`, `crates/ucc-hardware/**`, `crates/ucc-lighting/**` |
| **Агент 2** | `release/a2-core-config-ipc-daemon` | `crates/ucc-core/**`, `crates/ucc-config/**`, `crates/ucc-dbus/**`, `crates/uccd/**`, `uccd/**` |
| **Агент 3** | `release/a3-gui-desktop-cli` | `ucc-gui/**`, `libucc-dbus/**`, `crates/ucc-desktop/**`, `crates/ucc-session-agent/**`, `crates/ucc-cli/**`, `ucc-gnome/**` |
| **Агент 4** | `release/a4-build-deps-ci-release` | `Cargo.toml`, `deny.toml`, `.github/**`, `PKGBUILD`, `CMakeLists.txt`, `tests/CMakeLists.txt`, `Makefile`, `debian/**`, `ucc.spec`, `ucc.install`, `nix/**`, `crates/xtask/**`, удаление legacy C++ (`M13.7–M13.8`) |

---

## 2. Замороженный API-контракт (обязателен для компиляции после слияния)

### 2.1 `PlatformBackend` (`crates/ucc-hardware/src/platform.rs`, зона Агента 1)
Сохранить существующие сигнатуры:
- `fn read_tcc_offset(&self) -> Result<u8, PlatformError>` (возвращает offset в °C от TjMax, например `10` для `95 °C` при `TjMax=105`; на `0x1A` читает `0x0463 & 0x7F`).
- `fn set_tcc_offset(&self, offset_c: u8) -> Result<(), PlatformError>` (на `0x1A` пишет `0x80 | offset_c` в `0x0786`, допустимый диапазон `0..=40`, на `0x1A` пользовательский `80..=95 °C` -> `offset 10..=25`).

Добавить методы с дефолтной реализацией `Err(PlatformError::Unsupported(...))` (чтобы все моки в `uccd` и тестах компилировались без поломок):
- `fn read_dynamic_boost_watts(&self) -> Result<i32, PlatformError>` (на `0x1A` читает `0x07D5` live или `0x0746`).
- `fn set_dynamic_boost_watts(&self, watts: i32) -> Result<(), PlatformError>` (на `0x1A` пишет `0x0743 BIT(1)`, `0x0745 = 0xFF/0x00`, `0x0746 = watts`, `PWDB`).
- `fn read_pmsc_mode(&self) -> Result<&'static str, PlatformError>` (возвращает строго одно из: `"office"`, `"gaming"`, `"turbo"`, `"custom"`).

### 2.2 Схема профиля и настроек (`crates/ucc-core/`, зона Агента 2; читается в `ucc-gui/`, зона Агента 3)
- В `ProfileCpu` (`cpu` в JSON профиля):
  - `#[serde(rename = "maxTempC", default, skip_serializing_if = "Option::is_none")] pub max_temp_c: Option<u8>` — диапазон `80..=95` (`None` = не менять TCC).
- В `UccProfile` (JSON профиля):
  - `nvidia_ctgp_offset: Option<i32>` — валидация `0..=1000`.
  - `#[serde(rename = "nvidiaDynamicBoostW", default, skip_serializing_if = "Option::is_none")] pub nvidia_dynamic_boost_w: Option<u8>` — диапазон `0..=25`.
- В `UccSettings` (`settings.json`):
  - `#[serde(default)] pub experimental: ExperimentalSettings`
  - `ExperimentalSettings { #[serde(rename = "modeButton0x00b0Action", default = "default_mode_btn")] pub mode_button_0x00b0_action: String }`
  - Допустимые значения: `"firmware"` (по умолчанию), `"cycle_profiles"`, `"toggle_custom_max"`, `"emit_key_f14"`.

### 2.3 D-Bus контракт (`com.uniwill.uccd`, зона Агента 2 + клиент в `libucc-dbus`, зона Агента 3)
1. **`GetCpuPowerValuesJSON() -> s`** — сохранить существующие поля (`powerDraw`, `maxPowerLimit`) и добавить опциональные:
   - `"pl1W": i32`, `"pl2W": i32`, `"pl4W": i32`, `"tccLimitC": i32`, `"pmscMode": "office"|"gaming"|"turbo"|"custom"`.
2. **`GetDGpuInfoValuesJSON() -> s`** — сохранить существующие поля и добавить опциональные:
   - `"ctgpW": i32`, `"dynamicBoostW": i32`.
3. **`GetSystemInfoJSON() -> s`** — добавить:
   - `"tccSupported": bool`, `"tccMinC": 80`, `"tccMaxC": 95`.
4. **Новые экспериментальные методы (Polkit `ManageHardware` для записи, `Read` для чтения):**
   - `GetModeButtonAction() -> s`
   - `SetModeButtonAction(action: s) -> b`

---

## 3. Жёсткие правила качества (Anti-Slacking & Zero Shortcuts)

1. **Никакой халтуры и заглушек:** запрещены `todo!()`, `unimplemented!()`, `let _ =` на критичных путях (EC/HID/IPC/persistence/rollback), новые `#[allow(dead_code)]`, закомментированный код или обход тестов через `#[ignore]`.
2. **Обязательные регрессионные тесты:** на **каждый** исправленный P0 и P1 баг должен быть добавлен или обновлён unit/integration тест, который воспроизводит прежний дефект и подтверждает исправление.
3. **Обязательное 2-фазное выполнение в 10 субагентов (5 исполнителей + 5 независимых ревьюеров):**
   - **Фаза 1:** 5 параллельных субагентов-исполнителей по непересекающимся подзадачам.
   - **Фаза 2:** 5 **новых независимых** adversarial-ревьюеров, которые читают `git diff` построчно, ищут гонки, утечки, `unsafe`, пропущенные граничные случаи, нарушения контракта `CONTRACT.md` и невыполненные пункты аудита.
   - **Фаза 3:** исправление **всех** найденных P0/P1/P2 замечаний ревьюеров и финальный прогон всех проверок.
4. **ОБЯЗАТЕЛЬНЫЙ ПРОТОКОЛ СБРОСА ЛИМИТА ПО СУБАГЕНТАМ:**
   - Если ты упёрся в лимит субагентов (или завершил Фазу 1 из 5 исполнителей и нужно запустить Фазу 2 из 5 ревьюеров, либо `invoke_subagent` вернул ошибку лимита), **НЕ делай работу халтурно в основном потоке и НЕ пропускай Фазу 2 ревью!**
   - Вместо этого: (а) закоммить текущий проверенный прогресс в свою ветку и запушь; (б) обнови свой статус в `release/sync-board`; (в) выдай пользователю короткий чёткий статус и **прямо попроси написать короткое сообщение (например «продолжай»)** для сброса лимита по субагентам!
5. **Чистая сборка и окружение в VM/sandbox:**
   - Чтобы не упереться в место на диске и `rust-lld SIGBUS` при линковке, всегда используй:
     `export CARGO_TARGET_DIR="$HOME/ucc-target-slim" TMPDIR="$HOME/ucc-tmp" RUSTFLAGS="-D warnings -C debuginfo=0"` и `-j 2` для `cargo test`.
   - Для интеграционных тестов с BlueZ/D-Bus запускай под `dbus-run-session -- cargo test ...`.
   - При любом HTTP-запросе из VM устанавливай `User-Agent: Mozilla/5.0 (compatible; Google-Agent; +https://developers.google.com/crawling/docs/crawlers-fetchers/google-agent)`.
6. **Безопасность токена:**
   - НИКОГДА не коммить GitHub-токен, не пиши его в `.git/config`, в логи, в `sync/agentN.md` или в сообщения чата. Используй `GIT_ASKPASS` (файл вне репозитория с правами `chmod 700`) или одноразовый `git push`.
7. **Clean-room лицензирование:**
   - `kernel/uwcc-rs/**` — строго `SPDX-License-Identifier: GPL-2.0-only`.
   - Userspace (`crates/**`, `ucc-gui/**`, `libucc-dbus/**`, `ucc-gnome/**`) — строго `SPDX-License-Identifier: GPL-3.0-or-later`.
   - Никогда не копировать исходный код из `MechControl-RE` или `open-uniwill-cc` — использовать только задокументированные адреса регистров, битовые маски и протоколы из `docs/hardware/`.
