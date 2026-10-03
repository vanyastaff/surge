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

## Что должно заработать

Перед диспатчем ноды движок смотрит, влезет ли работа в остаток окна. Не влезает — ран паркуется с временем пробуждения и виден в inbox как ожидающий, а не как молча вставший. После сброса он просыпается сам, с замороженным бюджетом, который переармируется точно так же, как при обычном resume. Если ротация включена, вместо парковки берётся следующий настроенный профиль того же рантайма.

## Из брифа, дословно

> «before dispatching a node, refuse to start work that cannot finish inside the remaining window; park the run with a wake time instead of stalling»
> «Optional rotation across configured accounts — Surge never copies or stores provider credentials»
> «a run that exhausts its provider window resumes automatically after reset with its frozen budget intact, and the pause is visible in the inbox»

## Разделы спецификации

Истории 34–37, 49–50. Решения §13, §14, §19, §20. Границы: `surge-core::capacity`. Швы §2 и §3.

## Критерии приёмки

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
