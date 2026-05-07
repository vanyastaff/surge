# Surge — Roadmap

## Принципы развития

- **Vertical slice** — каждая фаза заканчивается работающим продуктом
- **CLI-first** — GUI добавляется после стабилизации ядра
- **Один агент → много** — сначала интеграция с Claude Code, потом мульти-агент

---

## Phase 0: Foundation (2 недели)

**Цель:** Cargo workspace, базовые типы, первое ACP-подключение.

- [ ] Инициализация workspace с крейтами: `surge-core`, `surge-acp`, `surge-cli`
- [ ] Базовые типы в `surge-core`: `SpecId`, `TaskState`, `SurgeConfig`, `AgentConfig`
- [ ] `surge-acp`: реализация `Client` trait из `agent-client-protocol`
  - [ ] `request_permission` — auto-approve mode
  - [ ] `session_update` — вывод в stdout
  - [ ] `write_text_file` / `read_text_file` — базовая FS
- [ ] Подключение к Claude Code CLI через stdio transport
- [ ] `surge-cli`: команда `surge ping` — проверка связи с агентом
- [ ] `surge-cli`: команда `surge prompt "Hello"` — one-shot запрос

**Результат:** `surge prompt "Explain Rust ownership"` работает через ACP → Claude Code CLI.

---

## Phase 1: Spec System (2 недели)

**Цель:** Создание, хранение и парсинг спецификаций.

- [ ] `surge-spec`: TOML-формат спецификации (см. ARCHITECTURE.md)
- [ ] Сериализация / десериализация через serde
- [ ] CLI: `surge spec create "описание задачи"` — интерактивное создание
- [ ] CLI: `surge spec create --interactive` — AI-assisted создание (агент помогает уточнить)
- [ ] CLI: `surge spec list` — список спеков с состояниями
- [ ] CLI: `surge spec show 012-add-auth` — просмотр деталей
- [ ] Автоматическая оценка сложности через агента (simple/standard/complex)
- [ ] Хранение спеков в `.surge/specs/` директории проекта
- [ ] Валидация спеков: обязательные поля, формат, ссылки

**Результат:** Полный цикл создания спецификации с AI-assistом.

---

## Phase 2: Git Worktrees (1 неделя)

**Цель:** Изоляция каждой задачи в отдельном worktree.

- [ ] `surge-git`: обёртка над `git2` для worktree операций
- [ ] Создание worktree: `git worktree add -b surge/{spec_id} .surge/worktrees/{spec_id}`
- [ ] Коммит изменений внутри worktree
- [ ] Diff между worktree и base branch
- [ ] Merge worktree в target branch
- [ ] Discard (удаление) worktree
- [ ] CLI: `surge diff {spec_id}`, `surge merge {spec_id}`, `surge discard {spec_id}`
- [ ] Автоматическая очистка orphaned worktrees

**Результат:** `surge merge 012-add-auth` мёрджит изолированные изменения в main.

---

## Phase 3: Orchestrator MVP (3 недели)

**Цель:** Полный пайплайн выполнения задачи с одним агентом.

- [ ] `surge-orchestrator`: основной pipeline
- [ ] Phase: Planning — агент разбивает spec на subtasks (если не заданы вручную)
- [ ] Phase: Execution — последовательное выполнение подзадач
  - [ ] Формирование контекстного промпта для каждой подзадачи
  - [ ] Отправка через ACP, получение streaming updates
  - [ ] Коммит после каждой подзадачи
- [ ] Phase: QA — агент проверяет acceptance criteria
  - [ ] Создание QA-fix подзадач при обнаружении проблем
  - [ ] QA-цикл с лимитом итераций
- [ ] Граф зависимостей подзадач (petgraph)
- [ ] File watcher: PAUSE файл для паузы, HUMAN_INPUT.md для инструкций
- [ ] CLI: `surge run {spec_id}` — запуск полного pipeline
- [ ] CLI: `surge status {spec_id}` — текущее состояние
- [ ] CLI: `surge logs {spec_id} --follow` — streaming логи

**Результат:** `surge run 012-add-auth` автономно выполняет все подзадачи, проводит QA, и ждёт review.

---

## Phase 4: Параллельное выполнение (2 недели)

**Цель:** Независимые подзадачи выполняются параллельно.

- [ ] Topological sort графа зависимостей → batches для параллелизма
- [ ] Параллельные ACP сессии к одному или разным агентам
- [ ] Merge результатов параллельных подзадач в worktree
- [ ] Обнаружение и разрешение конфликтов между параллельными изменениями
- [ ] CLI: `surge run {spec_id} --parallel 3` — ограничение параллелизма
- [ ] Прогресс-бар для параллельных подзадач

**Результат:** Независимые подзадачи выполняются одновременно, ускоряя pipeline в 2-4 раза.

---

## Phase 5: Multi-Agent (2 недели)

**Цель:** Разные агенты для разных подзадач и фаз.

- [ ] `AgentPool`: управление множеством подключений
- [ ] Конфигурация агентов в `surge.toml`:
  ```toml
  [agents.claude]
  command = "claude"
  transport = "stdio"

  [agents.copilot]
  command = "copilot"
  transport = "stdio"
  ```
- [ ] Routing подзадач к агентам:
  - По умолчанию из конфига
  - Override в spec.toml per-subtask
  - По фазе: claude для planning, copilot для coding
- [ ] CLI: `surge agent add`, `surge agent test`, `surge agent list`
- [ ] Fallback: если агент недоступен → переключение на альтернативный

**Результат:** `surge run 012-add-auth --planner claude --coder copilot` — планирование через Claude, кодинг через Copilot.

---

## Phase 6: GUI (4 недели)

**Цель:** Нативный GUI на egui.

- [ ] `surge-ui`: egui + eframe приложение
- [ ] Dashboard: Kanban-доска задач с drag-and-drop
- [ ] Spec Editor: создание и редактирование спецификаций
- [ ] Execution Monitor:
  - [ ] Real-time логи от агентов
  - [ ] Граф подзадач с состояниями
  - [ ] Прогресс-бары
- [ ] Agent Panel: статус подключений, выбор агента для задач
- [ ] Diff Viewer: side-by-side просмотр изменений
- [ ] Terminal Panel: встроенные PTY для worktrees
- [ ] Settings: конфигурация агентов, путей, поведения

**Результат:** Полноценный десктопный GUI — прямой конкурент Aperant.

---

## Phase 7: Advanced Features (ongoing)

- [ ] **Remote agents** — ACP через TCP/WebSocket для cloud-hosted агентов
- [ ] **Spec templates** — библиотека шаблонов (add-auth, add-api, fix-bug, refactor)
- [ ] **Project memory** — Graphiti-подобная система на SQLite для контекста между задачами
- [ ] **GitHub/GitLab integration** — создание spec из issue, автоматические PR
- [ ] **Team collaboration** — shared specs, review workflow
- [ ] **Plugin system** — расширение через WASM или dynamic libraries
- [ ] **Metrics & analytics** — стоимость токенов, время выполнения, success rate по агентам
- [ ] **ACP Registry** — browser доступных агентов, one-click подключение
- [ ] **Daemon mode** — фоновое выполнение задач с нотификациями
- [ ] **MCP pass-through** — трансляция MCP-серверов от пользователя к агенту

---

## Милестоуны

| Milestone | Фазы | Срок | Результат |
|-----------|-------|------|-----------|
| **v0.1.0-alpha** | 0-1 | +4 недели | ACP подключение + spec система |
| **v0.2.0-alpha** | 2-3 | +4 недели | Полный pipeline с одним агентом |
| **v0.3.0-beta** | 4-5 | +4 недели | Параллелизм + мульти-агент |
| **v0.5.0-beta** | 6 | +4 недели | GUI |
| **v1.0.0** | 7+ | +8 недель | Production-ready |

Общий срок до v1.0: ~6 месяцев при full-time разработке.

---

## Приоритеты

1. **ACP стабильность** — протокол ещё молодой, нужно отслеживать изменения
2. **Spec формат** — должен быть stable до v1.0, потом breaking changes дорого
3. **UX CLI** — первое впечатление. Команды должны быть интуитивными
4. **Тесты** — интеграционные тесты с реальными агентами в CI
5. **Документация** — README, getting started, examples с первого дня

---

## Engine M-series progress (internal)

The engine refactor uses a separate M-series numbering aligned with the
`surge-orchestrator` crate milestones. Status as of 2026-05-04:

| Milestone | Scope | Status |
|---|---|---|
| M1–M4 | Foundation, persistence, ACP bridge, routing | Shipped |
| M5 | Sequential pipeline, human gates, snapshots, resume | Shipped |
| M6 | Loop execution, subgraph execution, Notify delivery, `surge-notify` crate | **Shipped** |
| M7 | Daemon mode (long-running engine host with IPC), MCP server delegation via `rmcp`, `surge-daemon` + `surge-mcp` crates | **Shipped** |
| M8 | Retry / bootstrap stages / HumanGate channels, AdmissionController aging, parallel-loop execution | Planned |
| M9+ | Remote agents (TCP/WebSocket), plugin system, GUI integration | Future |

### M7 surface shipped in this PR

- `surge daemon start/stop/status/restart` — long-running daemon process,
  PID + socket discovery under `~/.surge/daemon/`. Cross-platform: Unix
  domain socket on Linux/macOS, named pipe on Windows.
- `surge engine run|resume|stop|watch|ls --daemon` — out-of-process engine
  hosting via local socket. Auto-spawns the daemon if not running.
- `EngineFacade` trait with `LocalEngineFacade` (M6 default) and
  `DaemonEngineFacade` (IPC client) impls.
- `AdmissionController` (FIFO, default `max_active = 8`) and
  `BroadcastRegistry` (multi-subscriber per-run + global daemon events)
  inside the daemon.
- `surge-mcp` crate exposing `McpRegistry` + `McpServerConnection` over
  rmcp 1.6 stdio transport. State machine: Disconnected → Running →
  Crashed (restart per `restart_on_crash` policy). Transport-vs-service
  error classification on rmcp errors.
- `RoutingToolDispatcher` fans out tool calls between engine built-ins
  and MCP servers; sandbox-aware exposure at session-open time.
- New validation rules in `surge-core::validation`:
  `McpServerUndeclared`, `McpServerNameEmpty`, `McpCommandPathUnsafe`.
- `RunConfig::mcp_servers: Vec<McpServerRef>` registry on the persisted
  run config; `EngineRunConfig::mcp_servers` on the per-call shape so
  the daemon receives the registry via IPC.
- Snapshot v2 unchanged.
- `EngineFacade` is `Send + Sync` and object-safe; tests can swap
  `LocalEngineFacade` for fake impls without IPC.
- `crates/surge-daemon/README.md` and `crates/surge-mcp/README.md` ship
  with operator docs.

### M6 surface shipped in this PR

- `NodeKind::Loop` — sequential iteration over static or artifact-derived items,
  `ExitCondition::{AllItems,MaxIterations,UntilOutcome}`, `FailurePolicy::{Abort,Skip,Retry}`.
- `NodeKind::Subgraph` — scoped inner graph with input bindings and output projection.
- `NodeKind::Notify` — five delivery channels via `surge-notify`:
  Desktop, Webhook, Slack, Email, Telegram.
- `EdgePolicy::max_traversals` cap with `ExceededAction::{Escalate,Fail}`.
- `validate_for_m6` — rejects multi-edge fanout (deferred to M8).
- 5 integration tests + 6 `#[ignore]`d stubs for M7/M8 scenarios.

---

## RFC-0010 — Plan A · Foundation ✅

Implemented over commits c34d76a..587fd90 (16 commits, 34 new tests).

- [x] M0 Crate scaffold — `surge-intake` added to workspace, module skeleton (Task 0.1)
- [x] M1 Trait + types + MockTaskSource — `TaskId`, `Priority`, `TriageDecision`, `Tier1Decision`, `TaskEvent`, `TaskEventKind`, `TaskDetails`, `TaskSummary`, `trait TaskSource`, `MockTaskSource` (Tasks 1.1–1.5)
- [x] M2 Persistence — `ticket_index` (migration 0002) and `task_source_state` (migration 0003) tables, `TicketState` enum, `IntakeRow`, `IntakeRepo` (Tasks 2.1–2.4)
- [x] M3 Tier-1 PreFilter + candidates module (Tasks 3.1–3.2)
- [x] M4 TaskRouter + two-source integration test (Tasks 4.1–4.2)

Plan B (Linear + GitHub providers) and Plan C (Triage Author, notify, daemon integration, end-to-end test, CLI) follow.

## RFC-0010 — Plan B · Providers ✅

Implemented over 7 commits (post-reset of original raw-GraphQL approach).

**Pivot:** Original plan had T5.2 (raw GraphQL client) + T5.3 (handwritten queries). After discovering `lineark-sdk` 3.0.1 — a polished, actively-maintained typed Rust SDK for Linear — we reset commits and rebuilt T5.x using the SDK. Saves ~350 lines of code, more idiomatic, less surface for bugs.

- [x] M5 Linear: deps via `lineark-sdk`, `LinearTaskSource` (full TaskSource impl using SDK), wiremock tests (via SDK's `set_base_url`), real-API ignored test (Tasks 5.1, 5.4, 5.5, 5.6)
- [x] M6 GitHub: `octocrab` client wrapper, `GitHubIssuesTaskSource` (full TaskSource impl), real-API ignored test (Tasks 6.1, 6.2, 6.3)

## RFC-0010 — Plan C · Integration ✅

Implemented over 13 task commits.

- [x] M7 Triage Author: bootstrap profile (TOML) + dispatcher (`TriageInput`, `TriageJson`, `into_decision`) + 3 fixtures + smoke test (Tasks 7.1–7.4)
- [x] M8 Notify InboxCard: `NotifyMessage::InboxCard` variant + Telegram formatter + Desktop formatter (Tasks 8.1–8.3)
- [x] M9 Daemon wire-up: `TaskSourceConfig` types + daemon `TaskRouter` spawn + InboxCard payload construction (Tasks 9.1–9.3)
- [x] M10 Event types: 14 new tracker variants in `SurgeEvent` (Task 10.1)
- [x] M11 End-to-end mock pipeline test (Task 11.1)
- [x] M13 CLI `surge tracker list` / `surge tracker test <id>` (Task 13.1)

**Plan-C-polish follow-ups** (not blocking RFC-0010 acceptance, but worth tracking):
- Triage Author LLM dispatch via ACP (currently `Priority::Medium` placeholder in T9.3).
- Actual delivery of `NotifyMessage::InboxCard` through `NotifyMultiplexer` (currently logged only).
- T9.2 in-memory dedup connection — should share daemon's persistent connection (Plan-C-polish or future RFC).
- Acceptance criterion #10 — `ticket_index` FSM proptest.
- `RouterOutput::EarlyDuplicate` should resurface the originating `TaskSource` so the daemon can post a "duplicate of #N" comment automatically.
- Multi-issue per polling cycle in `LinearTaskSource` / `GitHubIssuesTaskSource` (currently 1-per-cycle MVP).

RFC-0010 implementation is functionally complete — all decisions assigned to Plans A/B/C are delivered. Decisions assigned to RFC-0014 (webhook ingestion, embedding-based dedup), RFC-0006 refactor (sandbox tier 3+4 deprecation), and RFC-0004 refactor (vertical-slice mandate, token-budget guard-rail) remain out of scope for RFC-0010.

## RFC-0010 — Plan-C-polish ✅ (6 of 6)

Refinements on top of Plans A+B+C, delivered to harden the implementation.

- [x] **FSM proptest** — `TicketState::is_valid_transition_from` + 6 property/fixture tests covering FSM transitions. Closes RFC-0010 acceptance criterion #10 (gap flagged in audit). Commit: `8870368`.
- [x] **Multi-issue per polling cycle** — `LinearTaskSource` and `GitHubIssuesTaskSource` now emit ALL issues from a fetch via a `VecDeque`-backed unfold, rather than dropping the rest after the first. Real-world workspace readiness.
- [x] **Source registry → comment-on-dup** — surge-daemon now keeps `Arc<HashMap<String, Arc<dyn TaskSource>>>` alongside the router and posts a "duplicate of run #N" comment to the originating tracker on `RouterOutput::EarlyDuplicate`. Closes the dedup-side of acceptance #6. Commit: `619f917`.
- [x] **NotifyMultiplexer InboxCard delivery** — `RouterOutput::Triage` events render an `InboxCardPayload` to a `RenderedNotification` (title + body) and dispatch via `MultiplexingNotifier::deliver` against `NotifyChannel::Desktop`. `ChannelNotConfigured` is logged at debug; real deliverers receive the notification when configured. Closes the delivery-side of acceptance #3. Commit: `08eade5`.
- [x] **Persistent dedup connection** — `TaskRouter` now reads from a `Connection` opened directly to the daemon's registry DB file (using a new `Storage::registry_db_path()` accessor). State survives restarts and stays in sync with engine writes. Closes the T9.2 in-memory concern. Commit: `2d5cd61`.
- [x] **Triage Author LLM dispatch via ACP** — Replaces the `Priority::Medium` placeholder in surge-daemon with a real ACP-driven Triage Author call. File-artifact return path (`triage_decision.json` + `inbox_summary.md`) matching the Description Author pattern. Layer 1 of two-layer plan; Layer 2 promotes Triage to a graph node (separate RFC). See: [docs/superpowers/specs/2026-05-06-triage-author-llm-dispatch-design.md](superpowers/specs/2026-05-06-triage-author-llm-dispatch-design.md).
- [x] **Inbox-card callback handler** — Telegram + Desktop taps on Start/Snooze/Skip drive `Engine::start_run`, FSM transitions, tracker comments. `BootstrapGraphBuilder` trait + `MinimalBootstrapGraphBuilder` (single-Agent graph; RFC-0004 will replace with Staged). Closes RFC-0010 acceptance #4 (deferred Plan-C-polish item). Plan: [docs/superpowers/plans/2026-05-06-inbox-callback-handler.md](superpowers/plans/2026-05-06-inbox-callback-handler.md). Spec: [docs/superpowers/specs/2026-05-06-inbox-callback-handler-design.md](superpowers/specs/2026-05-06-inbox-callback-handler-design.md).

After polish, RFC-0010 implementation status:
- **3 acceptance criteria fully pass** (#1 surge-intake compiles, #2 sources poll, #11 clippy clean).
- **6 acceptance criteria pass with documented placeholders** (#3 InboxCard delivered with placeholder priority; #4 bootstrap-flow handoff via `MinimalBootstrapGraphBuilder`; #6 duplicate comment posted on Tier-1 hit; #10 FSM proptest in place; #11/#12 verified locally).
- **Bootstrap-flow handoff after Start tap (#4)** — ✅ shipped via Plan-C-polish (`MinimalBootstrapGraphBuilder` produces a single-Agent graph; RFC-0004's multi-stage `Description→Roadmap→Flow` chain replaces it via the same trait without callback-handler changes).
- **Remaining placeholders** — full LLM-driven triage (#3, #11), run-completion comment to tracker (#5), full SIGKILL recovery (#8), L3 auto-merge (#9), cross-OS CI matrix (#12) — listed as future work and tracked outside this RFC.
