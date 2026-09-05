# Интерфейсы

Читается каждым исполнителем **до** первой строки кода.

## Правила проекта, которые нельзя вывести из тикета

- **Стек:** Rust 2024 edition, MSRV 1.96, `tokio`, `rusqlite` + `r2d2`,
  `agent-client-protocol` 0.10.2. Воркспейс — 12 крейтов под `crates/`.
- **Сборка:** `cargo check` → `cargo clippy --workspace --all-targets --all-features -- -D warnings` → `cargo nextest run` → `cargo fmt`.
  Зелёными должны быть все четыре. `cargo nextest` предпочтителен `cargo test`.
- **Ошибки:** `thiserror` в библиотечных крейтах, `anyhow` в CLI.
- **Запрещено:** `unwrap()` в библиотечном коде — только `?` или явная обработка.
- **`#[must_use]`** на функциях, возвращающих `Result`. Публичный API документирован `///`.
- **Тесты** живут рядом с кодом в `#[cfg(test)]`, интеграционные — в `tests/`.
- **ID** — ULID через крейт `ulid`. **Хеши** — переиспользуй `surge_core::content_hash`, не заводи второй механизм.
- **Никаких новых внешних зависимостей** без явной необходимости (R46). Ни одной зависимости
  от пакетов DeepSeek Harness, Cordis или любого проекта из разбора (R43, R46).
- **Отсутствующая зависимость или недоступный инструмент — это `BLOCKED` в отчёте, а не установка.**
- **Не трогать:** `.autopilot/`, `docs/competitive-plan-2026-09.md`, чужие незакоммиченные файлы
  (`crates/surge-ui/src/components.rs`, `crates/surge-ui/src/screens/inbox.rs`, `findings.json`).
- **Версионирование схем:** новый event-пейлоад или ключ `surge.toml` → бамп по `docs/schema-versioning.md` (R53i).
- **ADR:** несущее решение → файл в `docs/adr/` по форме существующих (R52i).
- **Исполнение (R42):** работа ведётся агентами Rust Code Studio — `rust-scout` для локации,
  `rust-builder` для реализации, `test-engineer` для тестов, `rust-reviewer` для ревью.

## Границы, решённые в спецификации

| Модуль | Владеет | Выставляет | Прячет |
|---|---|---|---|
| `surge-core::runtime` | множеством рантаймов и политикой версий | `RuntimeKind::DeepSeekHarness`, `version_policy(kind)` | bundled `versions.toml` |
| `surge-core::skill` | идентичностью и резолвом скилла | `SkillRef{name,provider,version,hash}`, `SkillCatalog::discover(roots)`, `SkillCatalog::resolve(&SkillRef) -> Result<ResolvedSkill>`, `ResolvedSkill{instructions,files,hash}` | парсинг frontmatter, `plugin.json`, обход каталогов, хеширование |
| `surge-core::memory` | формой утверждения памяти | `MemoryClaim{id,text,provenance,confidence,status}`, `Provenance{source,hash,verified_by,verified_at}`, `Confidence` | правила старения |
| `surge-core::context_pack` | сборкой контекста под бюджет | `ContextPack::build(claims, budget) -> (ContextPack, PackReceipt)`, `PackReceipt{selected,dropped,reason,budget,used}` | порядок отбора и оценку токенов |
| `surge-core::run_report` | формой и рендерингом отчёта | `RunReport::compile(events) -> RunReport`, `render_json/md/html(&RunReport)` | шаблоны и инлайн-стили |
| `surge-core::evidence` | предикатом доказанности | `is_evidence_backed(&NodeOutcome) -> bool` | правило «верификатор — единственный путь к done» |
| `surge-core::capacity` | моделью окна ёмкости | `CapacityWindow{account,window,remaining,resets_at,source}`, `CapacityPolicy::decide(estimate,&window) -> Dispatch\|Park{wake_at}` | обучение окна из наблюдений |
| `surge-orchestrator::guard` | гигиеной цикла ноды | `LoopGuard::observe(tool_call) -> Verdict`, `LoopGuard::deadline(node)` | окно повторов и счётчики |
| `surge-orchestrator::spill` | политикой большого вывода | `spill_if_oversized(output, cap, &artifact_store) -> ToolOutput` | пороги и форму локатора |
| `surge-persistence` | event log и хранением памяти | запись новых событий, выборка для `RunReport::compile` | схему таблиц |
| `surge-cli` | человеческими поверхностями | `surge skill …`, `surge memory audit`, `surge run report` | рендер и форматирование |

## Швы для тестов — три, все существующие

1. **Чистые типы `surge-core`** — резолв скилла, компиляция отчёта из вектора событий-фикстур,
   арифметика ёмкости, отбор в context pack. Без I/O.
2. **Существующий движковый харнесс** — mock-ACP-агент (`crates/surge-acp/src/bin/`) +
   durability fault-injection. Биндинг скилла, guard'ы, парковка и resume.
3. **Существующие CLI-тесты** `crates/surge-cli/tests/`.

Новых швов не заводить.

## Контракты, добавленные завершёнными тасками

<!-- каждый исполнитель дописывает сюда блок при возврате -->

## Из таска 05 — память как утверждения

- `surge_core::memory::{MemoryClaim{id,text,provenance,confidence,status}, Provenance{source,hash,verified_by,verified_at}, Confidence{Verified,NameMatched,Asserted}, ClaimStatus{Verified,Unverified}, MemoryClaimId}`
- `Confidence` реализует `Ord`: `Verified < NameMatched < Asserted` (самое доверенное первым).
  **Таск 06 сортирует контекст по этому порядку и не переопределяет его.**
- `MemoryClaim::from_transcript(text, source, ContentHash) -> MemoryClaim` — фиксирует
  `Asserted` + `Unverified` в конструкторе (путь ingest, R19).
  `MemoryClaim::new(id, text, provenance, confidence, status)` — общий случай.
- `surge_persistence::memory::MemoryStore::{add_claim(&MemoryClaim) -> Result<()>,
  get_claim(MemoryClaimId) -> Result<Option<MemoryClaim>>, list_claims() -> Result<Vec<MemoryClaim>>}`
- `schema::SCHEMA_VERSION` 1→2. Открытие v1-базы мигрирует автоматически: каждая
  legacy-строка становится `Unverified`/`Asserted` claim с сохранением исходного текста
  (`legacy:<table>:<id>` как источник). Legacy-таблицы остаются на месте —
  `surge-cli/src/commands/memory.rs` продолжает работать без изменений.
- `memory::Provenance` **намеренно не реэкспортирован** в корень крейта: имя
  конфликтует с существующим `profile::registry::Provenance`. Обращайся полным путём.

> ⚠️ **Общая точка: `crates/surge-core/src/lib.rs`.** Регистрация модуля (`pub mod …`)
> — единственная строка, которую пишут несколько тасков. Добавляй **только свою**
> строку, ничего рядом не трогай.

## Из таска 02 — скиллы: тип, провайдеры, резолв

- `surge_core::skill::{SkillProvider{ProjectDir,UserDir,Registry}, SkillRoot{provider,path},
  SkillRef{name,provider,version:Option<String>,hash:Option<ContentHash>},
  ResolvedSkill{instructions,files:Vec<PathBuf>,hash},
  SkillCatalog::discover(&[SkillRoot])->Self,
  SkillCatalog::skills()->impl Iterator<Item=&SkillRef>,
  SkillCatalog::resolve(&SkillRef)->Result<ResolvedSkill,SkillError>,
  SkillError{NotFound,MalformedFrontmatter{file,reason},MalformedPlugin{file,reason},Io}}`
- `resolve` матчит по `(name, provider[, version])`. `ResolvedSkill::hash` **всегда**
  считается заново с диска. **Логика сравнения с пином и approval намеренно оставлена
  вызывающему — это таск 03, не переизобретай её здесь.**
- Frontmatter разбирается **своим ограниченным парсером** (`skill/frontmatter.rs`):
  плоские `key: value`, скаляры в кавычках и без, инлайновые списки `[a, b]`.
  Всё остальное — вложенные карты, якоря `&`/`*`, блочные скаляры `|`/`>`, отсутствие
  разделителя — типизированная ошибка с именем файла, строкой и причиной.
  **Внешнего YAML-парсера в дереве нет и добавлять его нельзя** (`deny.toml:44`,
  `unmaintained = "workspace"`).
- Фикстуры: `crates/surge-core/tests/fixtures/skills/{project,user,broken}/` — оба формата,
  офлайн, без сети.

## Из таска 01 — DSH как рантайм

- `surge_core::runtime::RuntimeKind::DeepSeekHarness`, wire-форма `"dsh"`.
- Реестр `surge-acp`: id `"dsh-acp"`, алиас `"dsh" → "dsh-acp"`.
- Запуск: `npx -y @deepseek-ai/dsh --profile acp` — **проверено по исходникам DSH**
  (`packages/bundle/acp-app/src/index.ts`), команда без опций.
- `RuntimeVersionPolicy{dsh, "=0.1.2-rc.1"}` — точный пин, версия из `npm view`.
  DSH — developer preview, ломающие изменения обещаны ими самими.
- Четыре строки `sandbox_matrix` для dsh × {ReadOnly, WorkspaceWrite, WorkspaceNetwork,
  FullAccess} — все `declared-unverified`: у ACP-профиля DSH нет флагов песочницы,
  на которые можно отобразить режим. Та же позиция, что у Cursor/Copilot/OpenCode/Goose.
- `.github/workflows/dsh-canary.yml` — push в `main`, ежедневный cron, `workflow_dispatch`.
  Триггера на pull_request нет намеренно.
- `semver` подключён в `surge-cli` из уже существующих workspace-зависимостей — новых крейтов нет.

> ⚠️ **Найдено попутно, вне зоны таска:** `resolve_launch_flags` / `ResolveContext::Run`
> не подключены к боевому пути запуска **ни для одного** рантайма. Это существующий
> пробел репозитория, не регресс; строка матрицы песочницы сегодня ничем не потребляется
> в рантайме. Несут в финальный отчёт.

> ⚠️ **`MemoryClaim::from_transcript` пока не имеет ни одного вызывающего.**
> Путь ingest (R19 «stored as explicitly unverified») принадлежит **таску 07** —
> write-back на границе рана. Таск 07 обязан вызывать именно её, а не собирать
> `MemoryClaim::new` со своими значениями `confidence`/`status`.

## Правило приёмки, добавленное после волны 1 — действует на все оставшиеся таски

Дважды подряд зелёный набор тестов доказывал **форму, выбранную под фикстуру**:
таск 02 подтверждал «паки работают без переделки» фикстурами меньшинственной
раскладки, таск 05 — «записи переезжают без потери» одной legacy-таблицей из четырёх.

> **Если требование звучит как «существующее продолжает работать», приёмка обязана
> трогать существующий артефакт, а не свой.** Настоящий пак, настоящая таблица,
> настоящий ран. Фикстура доказывает, что код делает то, что задумано; она не
> доказывает, что задумано было верно.

Настоящий корпус паков на этой машине: `~/.claude/plugins`, `~/.claude/skills`
(352 `SKILL.md`, 47 манифестов в `.claude-plugin/plugin.json`). Сети для этого не нужно.

## Поправка к контракту таска 05 (после ремонта)

- `MemoryClaimId` теперь порождается `define_id!(MemoryClaimId, "claim")` в
  `crates/surge-core/src/id.rs` — там же, где `SpecId`/`TaskId`/`RunId`/`SessionId`.
  Ошибка `FromStr` — `ulid::DecodeError`. **Своих ULID-newtype не заводить.**
- **Сигнатура изменилась:** `MemoryClaim::new(id, text, provenance, confidence, status)
  -> Result<Self, UnprovenVerifiedStatus>`. Конструктор отвергает `ClaimStatus::Verified`,
  если `provenance` не несёт команду проверки и время. Таски 06 и 07 обязаны исходить
  из **фаллибельной** формы.
- `MemoryClaim::from_transcript(text, source, ContentHash) -> Self` остаётся инфаллибельной.
- `ParseConfidenceError` / `ParseClaimStatusError` доступны как `surge_core::memory::…`,
  не из корня крейта.

> ✅ **Baseline починен (таски 14–16). Гейт снова меряет то, что должен.**
> Прогоняй **без подавлений**: `cargo clippy --workspace --exclude surge-ui --all-targets --all-features -- -D warnings`.
> Флаги `-A`, которые раздавались, пока baseline был красным, **больше использовать нельзя**:
> подавление, заведённое под чужой шум, начинает прятать твой собственный дефект в ту же
> минуту, когда чужой шум убрали. Ровно это и случилось с `print_literal` у таска 04.
>
> <details><summary>История долга (для понимания, почему это было)</summary>
>
> **Гейт clippy на baseline не работал — это меняло то, как проверять себя.**
> Две ошибки в `surge-core` (`build.rs:21`, `run_state.rs:382`) роняют компиляцию под
> `-D warnings`, и **зависимые крейты после этого не линтуются вообще**: за ними прячется
> ещё 18 ошибок (13 в `surge-persistence`, 4 в `surge-git`, 1 в `surge-mcp`).
> Пока таск 14 их не починил, прогоняй свою зону так:
> `cargo clippy -p <твой крейт> --all-targets --all-features -- -D warnings -A clippy::unneeded_wildcard_pattern -A clippy::collapsible_if`
> Иначе «клиппи чист» означало бы «клиппи не запускался».
> </details>

## Поправка №2 к контракту таска 05 — **несущая для тасков 06, 07, 08**

Поля `MemoryClaim` **приватны**. Читать только через геттеры:
`claim.id() -> MemoryClaimId`, `.text() -> &str`, `.provenance() -> &Provenance`,
`.confidence() -> Confidence`, `.status() -> ClaimStatus`.

Конструирование — только `MemoryClaim::new(...) -> Result<Self, UnprovenVerifiedStatus>`
либо `MemoryClaim::from_transcript(text, source, hash) -> Self` (инфаллибельная).

`Deserialize` написан **руками** и проходит через тот же валидирующий путь: JSON со
`status: "verified"` без `verified_by`/`verified_at` отвергается. Причина, по которой
`derive` не годился: он разворачивается внутри модуля-владельца и видит приватные поля,
то есть приватность одна инвариант не держит.

Поля `Provenance` (`source`, `hash`, `verified_by`, `verified_at`) остались публичными.

## Контракт локаторов памяти — **обязателен для таска 07**

Аудит (таск 08) разбирает `Provenance.source` по форме и **молча пропускает всё,
что в неё не попало**. Ревью нашло, что сегодня аудит на реальной машине разобрал бы
ноль записей и остался зелёным: единственное, что наполняет `memory_claims`, —
backfill из v1 с источниками `legacy:<table>:<id>`, исключёнными из обеих проверок.

Чтобы этого не повторилось, форма локатора фиксируется здесь как контракт, а не как
соглашение внутри одного модуля:

| Вид записи | `Provenance.source` | Что с ней делает аудит |
|---|---|---|
| Утверждение о файле репозитория | путь к файлу относительно корня проекта | сверяет `Provenance.hash` с текущим `ContentHash::compute` содержимого; исчезнувший файл → `SourceMissing` |
| Утверждение, извлечённое из хода рана | `transcript:run-<ULID>#turn-N` | коррелирует с исходом рана `<ULID>` |
| Мигрированная запись из v1 | `legacy:<table>:<id>` | не проверяется ни на устаревание, ни на корреляцию — источника уже нет |

**Таск 07 обязан писать одну из первых двух форм.** Собственная форма локатора —
это молчаливое отключение аудита для всех записей, которые он произведёт: аудит не
упадёт и не пожалуется, он просто ничего не найдёт.

## Предупреждение для таска 03 — форма `SkillRef.hash`

Резолв скилла считает идентичностью **хеш** (§4 спецификации): кандидаты с одинаковым
хешем — это один пак, найденный по двум путям, и любой из них корректный ответ;
`SkillError::Ambiguous` остаётся только для кандидатов с **различающимся** содержимым.

**Форма поля (закрыто):** `hash: Option<ContentHash>`. `None` означает «матчить по
имени, провайдеру и версии» — ровно то, что порождает объявление `skills = [...]` на ноде
флоу. Каждый `SkillRef`, вернувшийся из `discover()`, несёт `Some`: обнаружение всегда
знает настоящий хеш. Сентинела `ContentHash::compute(b"")` больше нет — он был скрытым
протоколом в комментарии и неотличим от честного хеша пустого пака.

**Обход** защищён множеством с ключом `(SkillProvider, PathBuf)`, а не одним каноническим
путём: один физический каталог, объявленный и как `ProjectDir`, и как `UserDir`, даёт паки
под обоими провайдерами. Ключ только по пути схлопывал второй корень молча.
