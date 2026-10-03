# 12 — Планировщик: парковка до сброса, пробуждение, ротация

**Требования:** R37, R37.1, R38, R38.1, R41
**Blocked by:** 11
**Зона:** `crates/surge-orchestrator/src/engine/engine.rs` · `crates/surge-daemon/src/`
**Волна:** 4
**Status:** in repair — archetype estimator implemented; rotation and end-to-end lifecycle gate remain open

### Checkpoint 2026-10-03

`cargo test -p surge-daemon --test quota_recovery_route_test` passes both
real daemon/ACP cases: typed exhaustion routes A→B in the same workspace, and
exhausted candidates park before the scheduler wakes the same task. The fixtures
require `cargo build -p surge-acp --bin mock_acp_agent` and
`cargo build -p surge-cli --bin surge` for the stage MCP helper. This verifies
reactive task-owned recovery; pre-dispatch capacity rotation remains open.

### Pre-dispatch implementation boundary

Read-only architecture review found that `runtime_capacity` has no profile/account
scope, observation timestamp or trusted source reference. It cannot authorize an
account-specific skip. Existing reactive fallback requires an actual primary
`SessionOpened` and typed 429; those checks must remain intact.

The next implementation needs these linked pieces:

1. A host-owned planned-stage journal anchor with stable logical invocation,
   frozen policy hash and control generation, before any provider effect.
2. Typed binding origin (original opening or pre-dispatch plan), with migration
   backfill preserving existing original-opening evidence.
3. Recipe/account-scoped durable capacity observations with trusted source,
   timestamp, expiry/reset and revision. Separate skip evidence from typed 429.
4. Transactional pre-dispatch skip and reservation in frozen candidate order,
   checking opt-in, same provider family, current claim/control and fresh evidence.
5. Reuse the existing one-shot quota opening permit and prompt authorization;
   pass logical invocation separately from selected provider invocation.

Acceptance must show zero primary opens/prompts when a trusted fresh observation
permits skipping it, one fallback open in the retained workspace, rejection of
forged/stale observations and different-family targets, and no duplicate effect
after crashes at plan/reservation/admission/opening boundaries. Unknown availability
permits an attempt without asserting availability; exhausted alternatives park.

A prerequisite policy defect was reproduced by
`rotation_does_not_override_retry_after_observed_reset_elapsed`: rotation used to
override retry even at/after an observed reset. The fix allows the original profile
retry at that boundary; 65 capacity tests, strict core all-target/all-feature clippy
and workspace format checks pass. Independent code review returned ACCEPTABLE.

### Ordering observations across attempts

Reservation-scoped inspection is insufficient for pre-dispatch selection: a later
successful opening in another run may supersede its old exhausted marker. The next
storage step must assign a monotonic registry admission epoch before each comparable
provider-opening effect. Admission sets the exact configured recipe to Unknown;
a typed error updates that projection only if its epoch is still current. A late
error from an older attempt remains audit evidence and cannot resurrect exhaustion.

Scope is project + candidate/runtime/account evidence + model + launch fingerprint,
not proven account-global capacity. Current launch fingerprints omit inherited
authentication environment; Unknown account identity must remain Unknown. Historical
markers must not be backfilled into an actionable projection without a complete
ordering of subsequent opens. Primary, fallback, continuation and comparable
openings without quota recovery must all produce supersession barriers. Failed RPC
or host death after admission leaves Unknown; replay inspection grants no new RPC.

Projection inspection then loads the current sealed reservation evidence in one
read snapshot. Only after this ordering exists may pre-dispatch skip/reservation
consume it with the current claim/control and frozen policy fences.

### Reservation evidence implementation checkpoint

Added `inspect_fresh_typed_exhaustion` and opaque `FreshTypedExhaustion`. Inspection
uses one SQLite read snapshot, exact frozen candidate/recipe matching and the
intersection of original/latest observation validity and known resets. Available
evidence cancels exhaustion; unknown/unsupported probes cannot extend the original
typed response's validity. Stored marker reads verify original TTL against the
immutable frozen stage and reset against the retained provider delay, rejecting
inconsistent or older observed evidence.

The storage-boundary tests demonstrated behavioral RED before implementation and
again for reset tampering before repair. After repair, all 34 recovery-cycle tests,
strict scoped clippy and scoped format checks passed; independent re-review returned
ACCEPTABLE. The full persistence library suite also passed: 438 tests. This read-only
API grants no opening authority and does not yet establish
the latest account/recipe observation across different runs. T12 remains in repair.

### Actual-provider capacity recovery

Successful fallback execution cannot establish recovery of the configured primary.
The capacity gate now captures the journal prefix before dispatch and clears only
the runtime identified by the last actual opening for the same node after that
prefix, and only on a successful stage result. Authentication, configuration,
pre-opening and storage failures preserve recorded exhaustion. Missing current
opening identity or unreadable history also preserves it.

The authentication-failure regression failed on the old implementation. The existing
13 capacity tests, added zero-opening missing-binding regression and real daemon
A→B case passed after the fix; the latter directly verifies A remains exhausted
while B's stale observation clears. Independent spec/quality review returned
ACCEPTABLE. Strict orchestrator/daemon all-target/all-feature clippy, workspace fmt
and diff checks passed. This corrects attribution but does not implement pre-dispatch
rotation.

### Registry opening admission ordering

Added registry migration 0026 and `work_items::recipe_capacity`: every comparable
provider opening commits a unique execution-writer admission before the RPC.
AUTOINCREMENT epochs order a broad runtime + existing Debug-AgentKind launch-hash
partition across projects, models and accounts. The latest admission is Unknown
until its exact opening attaches a sealed typed exhaustion receipt. Failed RPCs
and host death retain the Unknown barrier; repeated writer IDs cannot replay an
RPC, even when the original result was missing. No historical admissions are
invented or backfilled.

`record_selected_rate_limit` publishes within its existing transaction only for
matching actual writer, invocation, runtime and launch hash at the latest epoch.
Older typed causes remain audit evidence. A conflicting receipt for the same
opening rejects and rolls back; the original association is immutable. The new
read-only `inspect_current_recipe_exhaustion` checks latest admission, exact
project/frozen candidate/model, original/latest TTL/reset and sealed opening in
one registry read snapshot. This evidence grants no provider-opening authority.

All engine construction paths delegate to one owner that wraps the raw facade.
Standalone production project-description, doctor and daemon-triage callers wrap
with the same canonical registry dependency; library functions receive their
facade rather than opening an unrelated default registry. The wrapper delegates
legacy adapter and all bridge operations, validates returned provider identity,
and closes mismatched openings. Production fixture runtime sentinels were replaced
with the actual runtime identity.

Behavioral RED reproduced reservation evidence surviving a newer no-quota opening
and duplicate-writer facade replay before enforcement. Targeted tests cover
supersession, late original error, missing historical admission, unrelated
runtime/hash, exact project/model/TTL, conflicting origin rollback, storage failure
with zero RPC, receipt mismatch and replay after restart. The daemon A→B case
uses production `new_full` and asserts an actionable exact primary marker plus
one admission per primary/fallback; the wake case checks the alternate constructor
and a barrier for every recorded continuation opening. Pre-dispatch planning,
skip/reservation and account/profile rotation remain open; T12 is still in repair.

### Continue acknowledgment race repair

The real wake suite exposed an intermittent race after `RunContinued`: the daemon
journal subscriber could call `confirm_continued` before the engine authorized the
quota wake prompt. Both belonged to the same operation/generation, but the prompt
fence accepted only ContinueReserved and rejected its legitimate Executing ack.
A deterministic interposed acknowledgment reproduced the same conflict (RED).
The repaired fence accepts either state while preserving exact operation,
generation, current established handoff and durable journal prefix checks. Separate
tests retain both event orderings and reject changed operation/generation; all
three targeted automatic-wake storage tests pass. Broad execution verification is
recorded separately after the complete suites finish.

### Verification resource limit

The requested broad `cargo nextest run -p surge-orchestrator -p surge-daemon`
failed during test-binary linking with `No space left on device`, before test
execution. It is **not** a passing nextest gate. Concurrently queued verification
commands also ended with ENOSPC fingerprint-write failures. All those handles
are terminal. Package-scoped build-cache cleanup restored space without changing
source; verification now runs bounded affected suites sequentially. The complete
T12 nextest gate remains unverified until sufficient build capacity is available.

Whole-workspace strict clippy passed on the admission + acknowledgment repair
and fixture changes: `cargo clippy --workspace --all-targets --all-features --
-D warnings` (exit 0, 2m27s). Workspace fmt and diff checks passed. The unrelated
future-dependency notice for `block 0.1.6` was informational.

Final bounded verification after the acknowledgment repair and complete fixture
corrections passed with `CARGO_INCREMENTAL=0` and sequential linking:

- Persistence library: 443 passed.
- Orchestrator affected suites: archetypes 1, actual ACP/MCP permission 18, budget 3,
  capacity parking 14, recipe-admission facade 7 passed.
- Daemon affected suites: parity 1, resume stream 6 and real quota routing/wake 2
  passed. Both constructors and primary/fallback/continuation barriers were checked.
- Strict all-feature clippy for the final modified ACP permission fixture passed;
  workspace format and diff checks passed.

The root independently reran final whole-workspace strict clippy with
`CARGO_INCREMENTAL=0` (exit 0, 1m29s), and
`cargo nextest run -p surge-daemon --test quota_recovery_route_test` passed both
actual provider routing and automatic wake cases (2 passed, 0 skipped).

The broader nextest linker ENOSPC failure is retained above; these scoped results
do not replace the full T12 gate. No requirement was retired.

## Что должно заработать

### Ordinary flow ownership normalization (reviewed next dependency)

Configured rotation on ordinary CLI/daemon flow launches must enter the same
durable task owner, workspace, opening fences and automatic wake lifecycle as
task launches. The current task-owned pre-dispatch implementation does not close
this criterion. Until normalization exists, unowned rotation must fail explicitly
before any provider effect; freezing a policy alone is not support.

The reviewed next unit creates a real graph-backed task and accepted revision,
reserves an attempt with the host's run ID, and records a durable owned-flow
launch intent and operation result in one registry transaction. The exact graph
hash is a typed accepted-contract origin, without invented feature criteria.
Initial prompt and every launch input survive acknowledgment. The coordinator
does not launch detached work; existing admission, claims and startup recovery
remain the effect owners.

Operation replay is inspected before current source-cleanliness checks, discovery
or config freezing. Exact replay returns the original attempt, frozen config and
workspace intent; any changed explicit request field, including complete
`run_config`, conflicts. First acceptance captures a clean immutable Git base
and freezes host config before the transaction. Workspace preparation uses that
retained base. Dirty source or an arbitrary explicit `--worktree` is rejected on
first acceptance without changing source files, index or HEAD.

A reserved attempt alone is not a recoverable queued request: existing startup
reconciliation intentionally skips Reserved attempts without startup history.
Only an explicit durable owned-flow launch intent authorizes reconciliation of
that case. It permits startup attempts, not provider effects, and does not weaken
the generic Reserved safeguard. CLI launches must retain daemon ownership and
automatic wake rather than create a separate local recovery lifecycle.

Acceptance must cover CLI and daemon ordinary flow starts with zero exhausted-A
opens and one selected-B open; matching stream/registry run identity; concurrent
exact replay; changed `run_config` rejection; replay after source/config changes;
crashes after acceptance and queued acknowledgment before startup; retained
workspace and frozen budget through park/wake; and unchanged dirty files, index
and HEAD on first-acceptance rejection. Inventory other unowned launch surfaces
before declaring T12 complete. This is a reviewed plan, not implemented evidence.

Перед диспатчем ноды движок смотрит, влезет ли работа в остаток окна. Не влезает — ран паркуется с временем пробуждения и виден в inbox как ожидающий, а не как молча вставший. После сброса он просыпается сам, с замороженным бюджетом, который переармируется точно так же, как при обычном resume. Если ротация включена, вместо парковки берётся следующий настроенный профиль того же рантайма.

## Из брифа, дословно

> «before dispatching a node, refuse to start work that cannot finish inside the remaining window; park the run with a wake time instead of stalling»
> «Optional rotation across configured accounts — Surge never copies or stores provider credentials»
> «a run that exhausts its provider window resumes automatically after reset with its frozen budget intact, and the pause is visible in the inbox»

## Разделы спецификации

Истории 34–37, 49–50. Решения §13, §14, §19, §20. Границы: `surge-core::capacity`. Швы §2 и §3.

## Критерии приёмки

### ACP usage measurement boundary (2026-10-03)

The real warmup-before-planned-park fixture exposed an existing accounting gap:
`surge-acp::bridge::tokens::extract_usage` returns no spent-token snapshot.
The mock emits both context-window `UsageUpdate` and end-turn `PromptResponse`
usage, but current production consumption does not charge the latter. Nonzero
real ACP spending and its conservation therefore remain unproved; a host-owned
budget oracle can verify scheduling/rearm behavior without claiming measured
provider spending.

The pinned schema 1.9.1 describes response usage as per-turn while its component
fields describe session totals. The contradiction and divergent implementations
are tracked in [upstream issue 1860](https://github.com/agentclientprotocol/agent-client-protocol/issues/1860).
Context-window occupancy must not be substituted for spent tokens. A linked
implementation must make accumulation semantics and measurement coverage explicit,
preserve unknown/missing usage, and prove no double charging through multiple
turns, resume, reconnect and event replay. Token history/estimation requirements
remain open wherever real protocol usage is required; no requirement is retired.

The reviewed accounting dependency freezes a versioned adapter usage contract:
unknown, per-turn, or provider-session cumulative. Keep raw independent `u64`
token buckets, context occupancy and monetary cost separate; do not infer a
provider's contract from monotonic samples or fill in a model/currency. Before a
prompt RPC, persist a host turn identity. Store its raw usage receipt, normalized
delta, measurement coverage and cumulative baseline atomically, with exact
replay idempotence and conflicting replay rejection. A loaded session needs a
trusted baseline; missing reports, interrupted turns and declining cumulative
counters preserve incomplete coverage rather than becoming zero spending.

Acceptance must distinguish per-turn `100/300/50` from cumulative
`100/400/450`, charging `450` in both explicitly contracted cases. It must cover
duplicate delivery, restart, loaded sessions, missing usage, occupancy updates,
wide counters, cache buckets and non-USD cost. Task totals must expose coverage
separately from pricing. This remains a reviewed next dependency, not implemented
accounting evidence for the routing change.

- [ ] `CapacityPolicy::decide(estimate,&window) -> Dispatch | Park{wake_at}`
- [ ] `estimate` — медиана длительности и расхода по архетипу ноды из **существующих** таблиц аналитики
- [ ] Нет истории по архетипу → оценки нет → **отказа в диспатче нет**
- [ ] Парковка видна в `surge inbox` с временем пробуждения
- [ ] Пробуждение после сброса автоматическое; замороженный бюджет переармируется как в `1ed5caa`
- [ ] Ротация — opt-in; триггер: окно исчерпано И ротация включена → следующий профиль того же рантайма
- [ ] Учётные данные не копируются и не хранятся
- [ ] Тест на движковом харнессе: исчерпание → парковка → пробуждение → бюджет цел

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.

## Реализация: сверка M3 (2026-10-03)

- Перед каждым agent dispatch работает `CapacityPolicy::decide`; отсутствие
  наблюдения или оценки не блокирует первый dispatch. Доказательство production
  пути: `engine_capacity_park_test::estimate_none_and_never_observed_does_not_block_dispatch`.
- На повторной сверке прежняя оценка по узлу в текущем ране была удалена: она не
  отвечала критерию §19 и могла выдать нерелевантную историю за archetype estimate.
- `RunHistoryWorkEstimator` читает завершённые раны того же archetype из
  `PipelineMaterialized` и медианы строк `stage_executions`; выборка ограничена
  1000 последними завершёнными ранами. Миграция 0007 привязывает usage-сессии к
  попытке узла; 0008 хранит известную стоимость отдельно от неизвестной. Если
  хотя бы одно usage-событие не содержит цены, стоимость этой попытки исключается
  из spend median. Без archetype metadata/history estimate остаётся `None`.
- Тест проверяет фильтр архетипа, median времени, median известной стоимости,
  исключение незавершённых ран и отсутствие подмены отсутствующей цены нулём.

Проверки текущей сверки: `cargo test -p surge-orchestrator --lib` — 410 passed;
`cargo test -p surge-orchestrator --test engine_capacity_park_test` — 13 passed;
`cargo test -p surge-persistence --lib` — 433 passed; `cargo check --workspace`,
`cargo clippy -p surge-persistence -p surge-orchestrator --all-targets
--all-features -- -D warnings`, `cargo fmt --all -- --check` и `git diff --check`
— passed.

До durable account/profile handoff `Decision::Rotate` теперь возвращается к
настроенной политике парковки: без reset используется `blind_backoff`, а без
него не изобретается время пробуждения. Engine test фиксирует 77 секунд при
включённом, но ещё не реализованном rotation candidate.

Открыто: критерий ротации профиля пока не работает в production dispatch path;
движковый тест полного цикла `exhaustion → rotate/park → wake → resume` ещё не
закрывает все ветки.

Проверки: `cargo test -j2 -p surge-orchestrator --lib run_history_estimator` — 2
passed; `cargo test -j2 -p surge-orchestrator --test engine_capacity_park_test
estimate_none_and_never_observed_does_not_block_dispatch` — 1 passed.

## Перенесено из ревью таска 11 (2026-09-06) — читать до начала работы

Оба ревьюера, независимо друг от друга, уткнулись в одно и то же ограничение таска 11.
Оно там не дефект — таск 11 не обязан был его снимать, — но **этот таск об него споткнётся
на первом же шаге**, если начать с `decide`, а не с источника.

### 1. Ёмкость сегодня живёт в памяти процесса. Тебе она нужна до отправки.

`surge doctor` строит окно из **своего же только что упавшего** smoke-вызова (ветка
`Err(detail)`), а `HealthTracker` — структура в памяти процесса. Межпроцессной памяти
о наблюдённых 429 нет вовсе. То есть окно **не переживает процесс**: `doctor` покажет
ёмкость, только если отказ прилетел ему самому, здесь и сейчас.

`CapacityPolicy::decide(estimate, &window)` вызывается **перед диспатчем**, и в этот
момент у свежего процесса окно будет пустым всегда — не потому что провайдер щедр,
а потому что никто в этом процессе ещё не получал отказа. Политика, построенная на таком
источнике, будет диспатчить в исчерпанное окно и узнавать об этом из собственного 429 —
ровно то поведение, которое требование R37 запрещает («refuse to start work that cannot
finish inside the remaining window»).

→ **Условие: источник ёмкости — событие рана, а не память процесса.** Наблюдённый 429
уже пишется в лог как `StageFailed`; окно должно собираться оттуда. Это первый шаг таска,
а не последний.

### 2. Половина R35.1 сегодня не утверждаема, и это твоя зона

Ревью craft проверило и зафиксировало честно: «оценка не блокирует диспатч» **нельзя
утверждать на таске 11**, потому что `CapacityPolicy::decide` и `Park` не существуют
нигде в воркспейсе — грепом пусто. То есть «не блокирует» сегодня истинно структурно
(блокировать нечего), а не доказано.

→ Критерий «нет истории по архетипу → оценки нет → отказа в диспатче нет» — это **не
формальность и не унаследованная галочка**. Это единственное место, где утверждение
станет проверяемым. Тест должен краснеть, если отсутствие оценки начнёт означать отказ.

### 3. Классификатор рейт-лимита — один, и он уже есть

Таск 11 свёл два разошедшихся классификатора в один, живущий в `surge-core::capacity`;
`surge-acp::pool` теперь зовёт его. **Третьего не заводи** — если тебе нужен образец,
которого нет, добавляй в существующий и прогоняй тесты пула до и после.

Там же удержана граница, которую легко снести: дефолт «60 секунд» — это **политика
бэкоффа пула**, а не наблюдение. `parse_retry_after` в `capacity` возвращает `Option`
и обязан отказываться выдумывать. Парковка обязана вести себя так же: нет времени
сброса — нет выдуманного `wake_at`.

## Перенесено из ревью M1 (2026-09-06) — не забыть на следующих этапах

**В приёмку M3 (решение), обязательно:**

1. **Конфиг уже виден оператору, но его никто не читает.** `[capacity]` с
   `blind_backoff = "5m"` сериализуется в **каждый** `surge.toml`, который пишет
   `surge init` — проверено. То есть человек может отредактировать задокументированную
   ручку, которая ни на что не влияет, пока M3 не свяжет `CapacityPolicy` с
   `SurgeConfig.capacity`. Это не дефект M1 (потребитель по плану на M3), но M3 обязан
   закрыть разрыв, а не оставить ручку-обманку.
2. **`validate()` закрывает только один слой из двух.** Он отвергает непредставимый для
   `TimeDelta` бэкофф, но не календарный: `blind_backoff = "9000000000000s"` **загружается
   чисто** и тихо клампится в `+262142-12-31` внутри `decide` — ровно тот исход, который
   комментарий самой валидации объявляет предотвращаемым («явная ошибка конфига лучше, чем
   тихий кламп тремя слоями ниже»). Паникой это больше не является, поэтому не блокировало
   M1. → Добавить `Utc::now().checked_add_signed(delta).is_some()`, либо ограничить поле
   потолком, который сообщение уже обещает («минуты и дни»).

**В приёмку M5 (поверхность):**

3. **Группа WAITING не закреплена.** Тест проверяет **метку**, а не **группу**: удаление
   всего блока `if !waiting.is_empty()` из `print_inbox` оставляет **все 138 тестов
   `surge-cli` зелёными** (мутация G ревьюера). Исходный дефект — припаркованный ран,
   видимый только через `--json`, — по-прежнему не прибит.
   Корневая причина названа: строковый шов `attention: &'static str` позволяет `classify` и
   `print_inbox` расходиться независимо. Настоящее решение — enum, но это **пред-существующая
   форма**, не введённая этим таском; решать, менять ли её, на M5.

**Различение, которое надо удержать при приёмке M2–M5.** Ревью применило правило «грепни
боевых вызывающих у каждой новой `pub`» ко всему, что добавил M1, и нашло нулевых
вызывающих у `CapacityPolicy`, `WorkEstimate`, `RotationPolicy`, `Degraded`. **Это не тот же
дефект, что был у `parse_reset_hint`:**

- у них потребитель — `run_task.rs` на M3, и план **прямо называет** это место посадки;
- `parse_reset_hint` был недостижим потому, что **уже живой** производитель не обновили,
  пока ADR утверждал, что поведение переехало.

Первое — факт плана, второе было дефектом. Разница в том, **утверждал ли кто-то, что оно
уже работает**.

## Решения, принятые оркестратором на ревью M2 (2026-09-06)

**1. `CanonicalRuntimeId` — обязательный критерий приёмки M3, newtype назван умолчанием.**

Контракт «вызывающий уже нормализовал ключ» держится **прозой на голом `&str`** и
**отказывает открыто**: если M3 передаст `claude`, а строка в `runtime_capacity` лежит под
`claude-acp`, то `status()` вернёт `NeverObserved` → `decide` вернёт `Dispatch{None}` → ран
уйдёт **в исчерпанный рантайм**. Это направление ошибки, которое таблица рисков этого же
тикета исключает: «избыточный отказ терпим, недостаточный — нет».

В M2 не делалось намеренно: `CanonicalRuntimeId` рябит в `CapacityWindow.runtime: String`,
закоммиченный на M1, а единственный вызывающий живёт на M3.

→ **M3 обязан** либо ввести newtype, конструируемый только через
`Registry::normalize_agent_id`, либо доказать тестом, что не-нормализованный алиас на
вызывающей стороне невозможен. Прозы недостаточно — её недостаточность уже доказана
тестом `unnormalized_aliases_are_not_collapsed_by_this_store_alone`.

**2. `observed_at_ms` убран из `runtime_capacity`.**

Колонку писал каждый `observe` и не читал **никто**: ни одного `SELECT` во всём воркспейсе,
проекция `status()` её не берёт, ни один тест её не утверждает. При этом она заставляла бы
M3 выдумывать значение. Устаревание уже представимо слоем выше — через
`seconds_until_reset`, о чём говорит комментарий в той же миграции.

Ничего не отгружено, поэтому удаление бесплатно; возврат — одна аддитивная миграция.

**3. Поправка к моему собственному критерию приёмки.** Требование «тест на `NaN` писать на
колонке SQLite, в отличие от JSON» стояло на **ложной посылке**: SQLite молча переписывает
`NaN` в `NULL` при записи. Достижимая порча — значение вне диапазона (`1.5`), и `+inf`,
который хранится честно. См. память проекта, `surge-sqlite-nan-becomes-null`.

## Сознательное сужение на M5: терминальные раны больше не показывают ёмкость (2026-09-07)

**Это откат того, за что боролось ревью таска 11 — и он намеренный.** Записано, чтобы
следующий не прочитал как регресс и не «починил» назад.

**Что было на таске 11.** `classify` короткозамыкал на `summary.status.is_terminal()` и
возвращал `capacity: None`. Ревью показало, что это баг: сигнал 429 живёт как раз в
**терминальных упавших** ранах (`StageFailed` с текстом рейт-лимита), то есть сканировался
класс, где сигнала нет, и пропускался тот, где он есть. Починили — стали сканировать
`Failed | Aborted | Crashed`.

**Что стало на M5.** Колонка ёмкости переехала из фолда журнала в точечную выборку по
`runtime_capacity`, чтобы у факта остался **один дом**: журнал рана A физически не знает
о 429, который получил ран B на том же рантайме. Побочно снят долг производительности
таска 11 — измерено ~200–380 мс на сотне терминальных ранов против долей миллисекунды.

Но у мёртвого не-припаркованного рана **нет дешёвого честного способа приписать рантайм**:
`RunParked.runtime` у него отсутствует (он не парковался), а `SessionOpened.agent_id` в
старых журналах сырой, и чтение его — это ровно то полное чтение журнала, которое и было
долгом. Гадать по совпавшей строке реестра — тот же грех, что у старого скана.

→ Поэтому `Failed`/`Aborted`/`Crashed` дают `NeverObserved`, а живая колонка остаётся
только у `Parked` (там рантайм известен из собственного `RunParked.runtime`).

**Причины разные, и это существенно.** На таске 11 было **короткое замыкание** — ответ
неверен по построению. Сейчас — **отсутствие атрибуции**: ответа честно нет.

**Что потерял оператор:** для рана, умершего по рейт-лимиту, инбокс больше не показывает
ёмкость рантайма рядом с ним. Причина смерти по-прежнему видна в тексте отказа.

**Чем закрыть, если понадобится:** писать канонический рантайм в событие отказа так же,
как это делает `RunParked`, — тогда атрибуция станет дешёвой и сужение можно снять.
