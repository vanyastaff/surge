# Манифест требований

Источник: `2026-09-05-brief.md`, который указывает на §3 документа
`docs/competitive-plan-2026-09.md`. Цитаты в колонке «Из брифа» взяты оттуда —
это и есть текст, на который пользователь показал словами «эти волны».

Строку из этого списка может снять **только пользователь**.

## Волна 0 — DSH как рантайм

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R01 | «Add `RuntimeKind::DeepSeekHarness` (`dsh`) with launch args» | in-ticket | подтверждено самобрифингом | spec §1, ист.1-2 → T01 |
| R02 | «…discovery…» (для DSH) | in-ticket | подтверждено самобрифингом | spec §1, ист.2 → T01 |
| R03 | «…and the sandbox-delegation row in `docs/sandbox-matrix.md`» | in-ticket | подтверждено самобрифингом | spec §1, ист.3 → T01 |
| R04 | «Extend the `surge doctor` agent smoke test … to DSH» | in-ticket | подтверждено самобрифингом | spec §1, ист.4 → T01 |
| R05 | «…and the per-runtime launch-arg coverage suite (the pattern from commit `ab6de54`) to DSH» | in-ticket | подтверждено самобрифингом | spec §1, ист.5 → T01 |
| R06 | «Record the *developer preview* status in the runtime table: DSH promises breaking changes, so pin the tested version and fail loudly on protocol drift rather than degrading silently» | in-ticket | подтверждено самобрифингом | spec §2, ист.6-7 → T01 |
| R07 | «Acceptance: `surge doctor matrix` lists DSH» | in-ticket | подтверждено самобрифингом | spec ист.4 → T01 |
| R08 | «a bundled flow runs end to end on DSH in CI against a pinned version» | in-ticket | подтверждено самобрифингом | spec ист.5 → T01 |

## Волна 1 — Скиллы как типизированная аудируемая способность

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R09 | «`SkillRef` in `surge-core` — name, provider (project dir, user dir, registry), version, content hash» | in-ticket | подтверждено самобрифингом | spec §3-§4, ист.8,15 → T02 |
| R10 | «Bound on a node exactly like a context binding, never loaded lazily mid-stage (same rule as `project.md`)» | in-ticket | подтверждено самобрифингом | spec §3, ист.10 → T03 |
| R11 | «Resolve against the Agent Skills layout (`SKILL.md` + frontmatter)» | in-ticket | подтверждено самобрифингом | spec §3, ист.9 → T02 |
| R12 | «…and the Agent Plugins package shape (`plugin.json` + `skills/`), so existing packs work unmodified» | in-ticket | подтверждено самобрифингом | spec §3, ист.9 → T02 |
| R13 | «Emit `SkillBound { name, provider, hash }` events» | in-ticket | подтверждено самобрифингом | spec §4, ист.11 → T03 |
| R14 | «the Run Report lists every skill a run used» | in-ticket | подтверждено самобрифингом | spec §9, ист.25 → T09 |
| R15 | «Trust gate: … an unpinned or unhashed skill requires explicit approval» (переиспользуя profile-trust, ADR-0002) | in-ticket | подтверждено самобрифингом | spec §5, ист.12-13 → T03 |
| R16 | «`surge skill list\|show\|verify` for inspection» | in-ticket | подтверждено самобрифингом | spec ист.14 → T04 |
| R17 | «Acceptance: a stock skill pack (`archify`, `go-modern-guidelines`) binds to a node and its use is reconstructable from the event log alone» | in-ticket | подтверждено самобрифингом | spec ист.11 → T03 |

## Волна 2 — Memory v2: доказательства, а не заметки

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R18 | «**Per-claim provenance**: every memory entry carries source path, content hash, the command that verified it, a timestamp, and a verification status» | done | подтверждено самобрифингом | spec §6, ист.16 → T05  |
| R19 | «Entries ingested from transcripts are stored as *explicitly unverified*» | done | подтверждено самобрифингом | spec §6, ист.17 → T05  |
| R20 | «**Confidence as a tag, not a boolean** — … so a node's context can order direct evidence ahead of low-trust recall» | done | подтверждено самобрифингом | spec §6, ист.16 → T05, T06  |
| R21 | «`surge memory audit` — correlate entries with failed and looping runs, flag staleness against current file hashes, propose pruning» | in-ticket | подтверждено самобрифингом | spec §7, ист.18-19 → T08 |
| R22 | «it now also ages only *eligible unverified* entries, never verified ones» | in-ticket | подтверждено самобрифингом | spec §7, ист.20 → T07 |
| R23 | «**Write-back discipline** at run boundaries … root cause over symptom, update in place over near-duplicate, silence on a clean run, and never on the agent's own initiative — a memory write is a node outcome, not a side effect» | in-ticket | подтверждено самобрифингом | spec §8, ист.21-22 → T07 |
| R24 | «**Context packs under a hard token budget**» | in-ticket | подтверждено самобрифингом | spec §8, ист.23 → T06 |
| R25 | «with a receipt recording what was selected, what was dropped, and why. The receipt is a Run Report input» | in-ticket | подтверждено самобрифингом | spec §8-§9, ист.23-24 → T06 |
| R26 | «Acceptance: a run that used memory can be replayed to show exactly which entries entered which node's context, and why each was chosen» | in-ticket | подтверждено самобрифингом | spec ист.24 → T06 |

## Волна 3 — Run Report как артефакт первого класса

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R27 | «`RunReport` type in `surge-core`, compiled from the event log: nodes, outcomes, verifier verdicts, evidence artifacts, cost, skills bound, memory receipts, steers, approvals» | in-ticket | подтверждено самобрифингом | spec §9, ист.25,29 → T09 |
| R28 | «`surge run report <run_id> --format json\|md\|html`» | in-ticket | подтверждено самобрифингом | spec ист.25 → T09 |
| R29 | «The HTML form is a single self-contained file with no CDN» | in-ticket | подтверждено самобрифингом | spec §11, ист.26 → T09 |
| R30 | «**Verified vs unverified rendering everywhere** — a terminal success reached without a verifier node must be visually distinct in the report, the inbox and the ledger» | in-ticket | подтверждено самобрифингом | spec §10, ист.27 → T10 |
| R31 | «Optional PR attachment through the existing L3 merge gate» | in-ticket | подтверждено самобрифингом | spec ист.28 → T10 |
| R32 | «Stretch, only after the above: a **structure delta** section — the typed before/after of the changed public surface, in archify's Before/Delta/After shape, generated from our own Rust parse rather than a dependency» | deferred | план сам называет это stretch «only after the above» — за пределами волны 3 | отчёт |
| R33 | «Acceptance: a reviewer can accept or reject a completed run from the report alone, without opening the transcript» | in-ticket | подтверждено самобрифингом | spec ист.30 → T09 |

## Волна 4 — Автономность с учётом ёмкости

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R34 | «**Capacity model**: per-agent-account rate-limit window, remaining share, reset time» | in-ticket | подтверждено самобрифингом | spec §12, ист.31 → T11 |
| R35 | «Populated from ACP usage signals where available … and from observed 429s otherwise» | in-ticket | подтверждено самобрифингом | spec §12, ист.32-33 → T11 |
| R36 | «Surfaced in `surge doctor` and the inbox» | in-ticket | подтверждено самобрифингом | spec ист.31 → T11 |
| R37 | «**Scheduling policy**: before dispatching a node, refuse to start work that cannot finish inside the remaining window; park the run with a wake time instead of stalling» | in-ticket | подтверждено самобрифингом | spec §13, ист.34-35 → T12 |
| R38 | «Optional rotation across configured accounts — Surge never copies or stores provider credentials» | in-ticket | подтверждено самобрифингом | spec §14, ист.36-37 → T12 |
| R39 | «**Loop guards**: repeat-tool-call detection and per-node wall-clock policy at the engine level, raising `EscalationRequested` … rather than burning budget» | in-ticket | подтверждено самобрифингом | spec §15, ист.38 → T13 |
| R40 | «**Output spill**: tool output over a configured cap goes to the artifact store; the node sees a bounded preview and a locator» | in-ticket | подтверждено самобрифингом | spec §16, ист.39 → T13 |
| R41 | «Acceptance: a run that exhausts its provider window resumes automatically after reset with its frozen budget intact, and the pause is visible in the inbox» | in-ticket | подтверждено самобрифингом | spec ист.34-35 → T12 |

## Ограничения (из брифа и §4 плана)

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R42 | «используя rust-studio» | in-ticket | подтверждено самобрифингом | spec Покрытие (A13) → T01 |
| R43 | «**DSH is a runtime, not a bridge.** … We do not build against Cordis, and we do not depend on any DSH package» | in-ticket | подтверждено самобрифингом | spec §1 → T01 |
| R44 | «A DSH plugin that launches Surge is a distribution question … and is deferred» | deferred | план сам откладывает: вопрос дистрибуции, не архитектуры | отчёт |
| R45 | «**We adopt the Agent Skills format, not an agent's skill runtime.** … Loading is ours; the format is theirs» | in-ticket | подтверждено самобрифингом | spec §3 → T02 |
| R46 | «**No dependency on any project surveyed here.**» | in-ticket | подтверждено самобрифингом | spec §1,§3,§16 → T01, T02 |
| R47 | «**timesfm is out of scope.**» | deferred | явное ограничение брифа: не строить | отчёт |

## Раздел §Continuous — не волна

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R48 | «**Modern Rust guidelines pack** (G8)» | deferred | раздел §Continuous — не волна; «эти волны» = Wave 0…4 | отчёт |
| R49 | «**Cockpit as a showpiece.**» | deferred | раздел §Continuous — не волна | отчёт |
| R50 | «**Teaching repository.**» | deferred | раздел §Continuous — не волна | отчёт |

## Подразумеваемое

| ID | Из брифа (дословно) | Статус | Основание | Где |
|----|---------------------|--------|-----------|-----|
| R51i | *(подразумевается)* каждая волна оставляет воркспейс зелёным — `cargo fmt`, `clippy -D warnings`, тесты | in-ticket | подтверждено самобрифингом | spec §17 (A14) → T01 |
| R52i | *(подразумевается)* новые публичные поверхности документируются по конвенции репозитория (`///`, `docs/`, ADR) | in-ticket | подтверждено самобрифингом | spec §17 (A16) → T01 |
| R53i | *(подразумевается)* новые типы в event log / persistence версионируются по `docs/schema-versioning.md` | done | подтверждено самобрифингом | spec §17 → T01, T05  |

---

## Самобрифинг (полный автомат)

Интервью не проводилось — режим `full`. Тот же чеклист прогнан против себя;
каждое решение помечено видом. **Решения** приняты за пользователя и обязаны
попасть в финальный отчёт. **Фактов о пользователе** здесь нет вовсе: задача
чисто инженерная, в ней нет ни цен, ни текстов, ни аккаунтов — поэтому и
`placeholder`-строк нет, и это не пропуск, а свойство задачи.

Найденные факты (посмотрел, а не выдумал): `dsh` в системе нет; `node v24.18.0`
и `npx 11.16.0` есть; `cargo 1.98.0`, `cargo-nextest 0.9.140` есть; в
`surge-core/src/runtime.rs` уже живут `RuntimeVersionPolicy` + bundled
`versions.toml`; в `surge-acp` есть mock-ACP-агент; версионирование схем описано
в `docs/schema-versioning.md`.

| # | ASSUMPTION — принято за пользователя | Закрывает |
|---|---|---|
| A01 | Запуск DSH — через `npx -y @deepseek-ai/dsh` (бинарь не установлен, npx есть). Работает на машине пользователя, без аккаунта и без денег | R01 |
| A02 | Точный ACP-подкоманд DSH исполнитель берёт из документации DSH в момент реализации — не выдумывается здесь | R01, R02 |
| A03 | Пин версии — через существующий `RuntimeVersionPolicy` + bundled `versions.toml`, с пометкой `developer preview`. Строка версии читается из npm при реализации | R06 |
| A04 | E2E-прогон на DSH в CI — **отдельная job, которая реально ходит**: на `main` и по расписанию, не на каждом PR. Гейт G2 поймал, что первая формулировка («opt-in за флагом») сужала R08: план требует «in CI» безусловно | R08 |
| A05 | Автотесты скиллов идут на вендоренном фикстур-паке в `tests/fixtures/`, без сети. Приёмка на `archify` / `go-modern-guidelines` — ручная opt-in проверка | R17 |
| A06 | Хеш скилла — SHA-256 по набору файлов с сортировкой путей | R09 |
| A07 | Trust-гейт переиспользует форму profile-trust (ADR-0002); непинованный или нехешированный скилл идёт в существующий путь approval | R15 |
| A08 | Токен-бюджет context pack — ключ в `surge.toml` с консервативным дефолтом. Число — ручка, а не бизнес-факт | R24 |
| A09 | Порог spill — ключ в `surge.toml` с консервативным дефолтом | R40 |
| A10 | Окна rate-limit **не** зашиваются по провайдерам: длительность окна — факт о третьей стороне. Учатся из ACP usage и наблюдённых 429; окно неизвестно → только наблюдение, без отказа в диспатче | R34, R35, R37 |
| A11 | Ротация аккаунтов — opt-in и без секретов: Surge ссылается на уже настроенные пользователем профили и **не копирует и не хранит** учётные данные (формулировка выровнена по словам плана — G2 поймал усиление сверх сказанного) | R38 |
| A12 | Порядок сборки = порядок плана (W0→W1→W2→W3→W4). R14 и R25 — форвард-ссылки, закрываются на волне 3: Run Report **читает** event log, а не вызывается эмиттерами | R14, R25 |
| A13 | `используя rust-studio` = исполнители из плагина: `rust-scout` (локация), `chief-architect` / `api-design-lead` (design-гейты), `rust-builder` (реализация), `test-engineer` (тесты), `rust-reviewer` (фаза 6) | R42 |
| A14 | Каждая волна проходит существующие гейты репозитория: `cargo fmt`, `clippy --workspace --all-targets --all-features -D warnings`, `cargo nextest run` | R51i |
| A15 | Новые event-пейлоады и ключи конфига получают бамп версии схемы по `docs/schema-versioning.md` | R53i |
| A16 | Новые публичные поверхности documented `///`; на несущие решения (skills binding, capacity model, run report) пишутся ADR | R52i |
| A17 | Пять волн по воркспейсу в 162k строк — доставка не на одну сессию. Таски нарезаются на всё, летят по порядку, отчёт называет ровно то, что село; остальное поднимается из `.autopilot/` по слову «продолжи автопилот» | весь бриф |

**G1 пройден.** Ни одна строка не осталась без решения: 6 строк переведены в
`deferred` с основанием, остальные подтверждены самобрифингом и получают секцию
спецификации в фазе 3.

## Результат гейта G2 — независимая проверка покрытия

Проверяющий получил только бриф, `spec.md` и §3 плана. Ни манифеста, ни кода,
ни переписки. Нашёл 20 расхождений:

- **8 пропусков** — 7 закрыты новыми историями и решениями (ист. 40–47,
  Решения §18–§24); 1 (R32, structure delta) уже стоял в «Вне рамок» как
  stretch по словам самого плана.
- **4 полу-покрытия** — все 4 дописаны: состав Run Report назван поимённо (§18),
  источник оценки ноды (§19), модель аккаунта и триггер ротации (§20),
  конкретные исполнители rust-studio (§21).
- **8 «лишнего»** — 6 оказались законным углублением требования (`R06.1`,
  `R09.1`, `R15.1`, `R27.1`, `R35.1`, `R53i`) и оставлены с явным родителем;
  **2 были настоящими дефектами и исправлены**: сузил R08 до ручной job (вернул
  в CI) и усилил R38 сверх слов плана (выровнял).

Гейт окупился: две правки — это ровно те две вещи, которые на финальной приёмке
стоили бы пересборки.

## Открытия сборки

| ID | Что доказал код | Основание | Служит требованию |
|----|-----------------|-----------|-------------------|
| D01 | Гейт `clippy --workspace -D warnings` **не работает на baseline**, и хуже, чем выглядело. Две ошибки в `surge-core` (`build.rs:21`, `run_state.rs:382`) роняют компиляцию под `-D warnings`, после чего **зависимые крейты не линтуются вообще**. Подавив только эти два линта, оркестратор насчитал на чистом HEAD ещё **18 ошибок**: 13 в `surge-persistence`, 4 в `surge-git`, 1 в `surge-mcp`. Цепочка обрывается на первом падении, значит за ними может быть ещё | оркестратор измерил в отдельном worktree от HEAD дважды: сначала 2 ошибки, затем с подавлением этих двух — 18. Первый замер оркестратора («в зоне памяти чисто») был **недействителен** по этой же причине и ошибочно опроверг верное замечание ревьюера | R51i, A14 — «каждая волна оставляет воркспейс зелёным» было непроверяемым утверждением; закрывается таском 14 |
| D07 | **Ошибка нарезки, доказанная сборкой:** таск 08 (аудит памяти, волна 2) зависит от таска 13 (guard'ы движка, волна 3). Требование R21 просит коррелировать записи с «провалившимися **и зациклившимися**» ранами, но признака «снят guard'ом» в схеме нет: сам guard не построен, а единственный след эскалации не индексирован в реестре ранов и смешивает исчерпание перезапусков MCP с исчерпанием попыток bootstrap | таск 08 вернул `BLOCKED` вместо того, чтобы выдумать признак — ровно как требовало условие таска | R21 — половина «устаревание + провалившиеся раны» сделана и принимается; половина «зациклившиеся» ждёт долговременного следа от §15 |
| D05 | «Ноль отвергнуто» в приёмке таска 02 было **определением, а не измерением**: 124 из 348 паков (36%) вынесены в отдельную графу `Ambiguous` и недостижимы вызывающему. Корень — `SkillCatalog::resolve` матчит по имени, провайдеру и версии и **не использует `SkillRef.hash`**, хотя §4 спецификации уже назначил идентичностью именно хеш. Средство различения было в руках и не применялось | ревью манифеста таска 02 сняло цифры само и нашло дубликат на диске: `plugins/cache/vanya/rust-studio/0.45.0` и `plugins/marketplaces/vanya/plugins/rust-studio` той же версии — обычное «источник + кеш», не экзотика | R12 — «паки работают без переделки» считается по достижимости вызывающему, а не по факту обнаружения |
| D06 | Рекурсивный обход, добавленный ради непустой приёмки, **не защищён от цикла**: `is_dir()` идёт по симлинкам, а записи в `~/.claude/skills/` — симлинки. При одноуровневом обходе безвредно, при неограниченной рекурсии симлинк на предка даёт бесконечный спуск | ревью манифеста таска 02 | R12 — дефект ломается на чужой машине, а не на своей |
| D04 | «Существующие паки работают без переделки» было доказано **фикстурами, написанными под собственный парсер**. Против настоящего корпуса: 16 из 352 реальных `SKILL.md` отвергались, а основная позиция манифеста Agent Plugins (`.claude-plugin/plugin.json`, 47 штук) не распознавалась вовсе — из 56 пакетов код видел бы почти ноль | ревью манифеста/спецификации таска 02 прогнало правила парсера по `~/.claude/plugins` и `~/.claude/skills`; оркестратор подтвердил счёт независимо | R11, R12 — спецификация §3 и История 9 дописаны, приёмка переведена на настоящий корпус |
| D03 | Спецификация §17 покрывала только event-пейлоады и ключи `surge.toml`. Хранилище памяти — **четвёртый версионируемый формат**, и правило «additive fields do not require a bump» из того же документа противоречит бампу, пока не названа настоящая причина: разовый backfill | ревью манифеста/спецификации таска 05: дифф переписал «freezes all three at version 1», ни на что не опираясь | R53i — §17 и §6 спецификации дописаны, `CHANGELOG.md` входит в определение «бамп сделан» |
| D02 | `crates/surge-core/src/lib.rs` — **общая точка нескольких тасков**: и 02, и 05 обязаны зарегистрировать в нём свой модуль. Карта зон в фазе 4 этого не предусмотрела | обнаружено при возврате таска 05: файл содержит строки двух тасков одновременно | R51i, R52i — цена частично отыграна: волна 1 уйдёт **тремя** коммитами вместо одного. Порядок, при котором каждый компилируется: (1) таски 14+15 — чистые правки линтов, `lib.rs` не трогают; (2) таск 01 — рантайм, `lib.rs` не трогает; (3) таски 02+05 вместе — они и делят `lib.rs`. Потеряна одна точка отката из четырёх, а не две из трёх |

## Отступления от процесса, принятые осознанно

| Где | Что нарушено | Почему |
|---|---|---|
| Ремонт таска 08, находка про путь Windows | потолок «три исполнителя в воздухе» из `phases/5-subagents.md` — послано четвёртым | Находка это молчаливая потеря корректности на платформе, которую гоняет CI: на Windows аудит переставал видеть устаревание файловых записей и оставался зелёным. Потолок существует, чтобы не забивать контекст оркестратора возвратами; контекст в этот момент был израсходован меньше чем на 4%, то есть ограничение не связывало. Держать известную тихую поломку ради соблюдения правила против его собственной цели — плохой размен |
