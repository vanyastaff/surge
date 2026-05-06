# Inbox-Card Callback Handler — Design

**Status:** Design (drafted 2026-05-06)
**Closes:** RFC-0010 acceptance criterion #4 (deferred Plan-C-polish item).
**Predecessor:** RFC-0010 Plan A/B/C (shipped). Plan-C-polish (5 of 6 done; commit `d6810dd`).
**Successor placeholder:** RFC-0004 Bootstrap & Flow Generation (will replace
`MinimalBootstrapGraphBuilder` with `StagedBootstrapGraphBuilder`); future
`surge-tg` standalone bot service binary (will extract the bot loop without
redesign).

> **One-line scope.** When a user taps `▶ Start` / `⏸ Snooze 24h` / `✕ Skip`
> on an inbox card delivered via Telegram or Desktop, the corresponding action
> takes effect on the originating ticket: `Start` creates a real run via
> `Engine::start_run`, `Snooze` defers the card for 24 h, `Skip` marks the
> ticket and labels it back in the tracker. State is durable in `ticket_index`,
> tracker comments are posted via the existing `Arc<dyn TaskSource>` registry,
> and the bot loop is structured for extraction into a separate
> `surge-tg` binary in a future iteration.

---

## 1. Goals and non-goals

### 1.1 In scope

- **Telegram callback-query receiver** — a `teloxide`-based bot loop running
  inside `surge-daemon`, listening on `callback_query` updates and writing
  `InboxRunStartRequested` / `InboxSnoozeRequested` / `InboxSkipRequested`
  events to the event log (cross-process-ready transport).
- **Desktop action receiver** — `notify-rust` `wait_for_action` callback
  forwarded as the same set of events.
- **`InboxActionConsumer` in `surge-daemon`** — polls the event log for the
  three new request events, validates `ticket_index` FSM, dispatches.
- **`BootstrapGraphBuilder` trait in `surge-orchestrator`** — abstracts how a
  user prompt becomes a `Graph`. Today's `MinimalBootstrapGraphBuilder` ships
  as the only impl: a single `NodeKind::Agent` graph with the ticket
  title+description+url interpolated into the system prompt. Future
  RFC-0004's `StagedBootstrapGraphBuilder` will swap in via DI without
  changes to the consumer.
- **Real `Engine::start_run` invocation on Start** — produces a true `RunId`
  in the `runs` table, satisfying the FK on `ticket_index.run_id`.
- **`TicketStateSync` in `surge-daemon`** — subscribes to `RunHandle.events`
  for runs created by inbox actions, applies FSM transitions
  (`RunStarted → Active`, `Terminal(Completed) → Completed`,
  `Terminal(Failed) → Failed`), posts tracker comments via the source
  registry.
- **Snooze re-emission scheduler** — tokio loop polling
  `ticket_index WHERE state='Snoozed' AND snooze_until <= now()` every
  5 minutes; re-issues the inbox card with a freshly-generated callback
  token.
- **`ticket_index.callback_token` column** — short ULID, `UNIQUE`-indexed,
  separate from `run_id` (which still keeps its FK to `runs(id)`). Generated
  on each card emission; rotated on snooze re-emission; cleared on Start.
- **`ticket_index.tg_chat_id` / `tg_message_id` columns** — captured at
  send-time; used by the bot loop to call `editMessageReplyMarkup` /
  `editMessageText` after an action so the original message stops looking
  actionable. (Hooks for the future `surge-tg` extraction.)
- **`InboxCardPayload` schema change** — replace `run_id: String` with
  `callback_token: String`. The token is what the keyboard buttons encode;
  the actual `RunId` is generated only when `Engine::start_run` runs.
- **Setup glue** — minimal: a `SURGE_TELEGRAM_CHAT_ID` env var (or
  `[telegram] chat_id` in `surge.toml`) that the bot loop uses as the
  destination for inbox cards. The full `surge telegram setup`
  binding-token flow from RFC-0007 is out of scope for this task.
- **Three new `EventPayload` variants** in `surge-core::run_event` (or
  the equivalent core-level event enum) — `InboxRunStartRequested`,
  `InboxSnoozeRequested`, `InboxSkipRequested`. Plus `InboxRunStarted`,
  `InboxRunCompleted`, `InboxRunFailed` as observability events.
- **Integration test** — `crates/surge-daemon/tests/inbox_callback_e2e.rs`
  drives a `MockTaskSource` → router → InboxCard → simulated callback
  with mocked engine + mocked TaskSource, asserting state transitions
  and tracker comment calls.

### 1.2 Out of scope

- **Full RFC-0004 bootstrap chain** (Description Author → Roadmap Planner
  → Flow Generator + HumanGate approvals between stages). The
  `BootstrapGraphBuilder` trait shape is chosen so that
  `StagedBootstrapGraphBuilder` is a drop-in replacement when RFC-0004
  ships, but the staged graph itself is not built here.
- **Triage Author LLM dispatch** — still out (Plan-C-polish remaining
  item). Daemon continues to use `Priority::Medium` placeholder when
  building inbox cards from `RouterOutput::Triage`.
- **Standalone `surge-tg` binary extraction** — bot loop runs as a
  `tokio::spawn` inside `surge-daemon`. Module layout makes future
  extraction mechanical.
- **`surge telegram setup` binding-token flow** (RFC-0007 §Setup) — chat
  ID provided via env var / `surge.toml` for now.
- **Webhook delivery mode** — long-poll only; webhook deferred to
  RFC-0014.
- **MarkdownV2 escaping & secrets filtering** (RFC-0007 §Privacy) — body
  is sent as plain text; tracker URLs are the only embedded URLs.
  Filtering is a follow-up that doesn't change the callback-handler
  design.
- **`/run`, `/list`, `/status` slash commands** (RFC-0007) — only
  `callback_query` updates are handled; non-callback messages are
  ignored.
- **Per-source chat overrides / multi-chat routing** (RFC-0007 §Multiple
  users) — single `SURGE_TELEGRAM_CHAT_ID`.
- **`editMessageText` after action with the visual confirmation banner**
  (RFC-0007 §After-decision rendering) — stub: send a separate "✓
  accepted" message instead. Edit-in-place is a follow-up.

### 1.3 Architectural invariants (established now, not changed by RFC-0004)

- All inbox actions transit through the **event log** (SQLite), not
  in-process channels. This is the load-bearing decision that lets
  `surge-tg` extract later without touching the consumer.
- `BootstrapGraphBuilder` is the **single chokepoint** between
  user-intake (CLI / Telegram / inbox-card / future Slack / web) and
  `Engine::start_run`. Replaces nothing, adds the missing seam.
- `ticket_index` is the **source of truth** for ticket lifecycle. FSM
  transitions go through `IntakeRepo::update_state_validated` (new
  helper); raw `update_state` becomes internal.

---

## 2. Architecture

### 2.1 High-level

```
                 ┌──────────────────────────────────────┐
                 │ User intake (entry-points)           │
                 ├──────────────────────────────────────┤
                 │ • CLI: surge run "..."               │
                 │ • Telegram: /run "..."   (future)    │
                 │ • Inbox-card "Start" tap (THIS TASK) │
                 │ • Slack interactive button (future)  │
                 └──────────────┬───────────────────────┘
                                │
                                ▼ writes Inbox*Requested events
                  ┌──────────────────────────────┐
                  │ Event log (SQLite, WAL mode) │
                  └──────────────┬───────────────┘
                                 │
                                 ▼ polled by surge-daemon
                  ┌──────────────────────────────────────┐
                  │ surge-daemon: InboxActionConsumer    │
                  │  • lookup ticket by callback_token   │
                  │  • validate FSM transition           │
                  │  • dispatch by action kind           │
                  └─────┬───────────┬────────────┬───────┘
                  Start │     Snooze│        Skip│
                        ▼           ▼            ▼
            ┌─────────────────┐ ┌─────────┐ ┌──────────────┐
            │ BootstrapGraph- │ │ snooze_ │ │ set_label    │
            │ Builder.build() │ │ until + │ │ surge:skipped│
            │ → Engine::      │ │ 24h     │ │ + post       │
            │ start_run       │ │ + clear │ │ comment      │
            │ → ticket_index  │ │ tg_msg  │ │              │
            │   row=RunStart  │ │ refs    │ │              │
            │ + post comment  │ │         │ │              │
            └────────┬────────┘ └─────────┘ └──────────────┘
                     │
                     ▼
            ┌─────────────────────────────────────┐
            │ TicketStateSync (per spawned run)   │
            │  • subscribe RunHandle.events       │
            │  • RunStarted → state=Active        │
            │  • Terminal(Completed) → state=     │
            │    Completed + comment "✅ done"    │
            │  • Terminal(Failed) → state=Failed  │
            │    + comment "❌ failed: <reason>"  │
            └─────────────────────────────────────┘
```

### 2.2 Where the bot loop lives

A `tokio::spawn`-ed task inside `surge-daemon::main` runs:

- The **outgoing leg**: every 500 ms, scan event log for
  `InboxCardSent` events that don't yet have a corresponding
  `InboxCardDelivered` ack, render the payload, send via teloxide
  `bot.send_message` with inline keyboard, write `InboxCardDelivered`
  with `tg_chat_id` + `tg_message_id`.
- The **incoming leg**: `teloxide::Dispatcher` long-polls Telegram for
  `callback_query` updates. Handler parses `callback_data` (form
  `inbox:<action>:<callback_token>`), looks up `ticket_index` by
  `callback_token`, writes the corresponding `Inbox*Requested` event
  to the log.

The Desktop receiver is a separate `tokio::task::spawn_blocking` that
calls `notify-rust::Notification::show()` and forwards the
`wait_for_action` result through the same event-log write.

Both receivers share a `secrets::WriteEvents` handle (the storage
layer). They never call `Engine::start_run` directly — that's the
consumer's job, and it lives behind the event-log boundary so that
the future `surge-tg` extraction is a pure code move.

### 2.3 Why event log, not mpsc

In-process mpsc would be ~1 line of code shorter and run a millisecond
faster, but it would lock us into co-location of the bot service and
the daemon. RFC-0007 explicitly says the bot is a singleton process
(`vibe-tg` / `surge-tg`) communicating with per-run engine daemons
through SQLite. The event log:

- Survives bot/daemon crashes independently (each side is stateless
  beyond polling).
- Is the same mechanism RFC-0007 already specifies for
  `ApprovalRequested` ↔ `ApprovalDecided`.
- Records a durable audit trail of every inbox action (who decided
  what, when, via what channel).
- Means future Slack / Discord / web receivers slot in by writing the
  same event-log rows, without consumer changes.

The cost — a 500 ms outgoing-poll latency and an extra DB hop per
action — is negligible at the cadence of ticket triage (seconds, not
milliseconds).

---

## 3. Components

### 3.1 `BootstrapGraphBuilder` trait (`surge-orchestrator`)

```rust
// crates/surge-orchestrator/src/bootstrap.rs (new file)

use async_trait::async_trait;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use std::path::PathBuf;
use thiserror::Error;

/// Build the initial `Graph` that every user-initiated run begins with.
///
/// Implementations decide the structure of the bootstrap pipeline. Today's
/// `MinimalBootstrapGraphBuilder` produces a 1-stage Agent graph that takes
/// the prompt directly. The future `StagedBootstrapGraphBuilder` (RFC-0004)
/// will produce the 6-node Description → Approve → Roadmap → Approve →
/// Flow → Approve prelude.
///
/// Object-safe so the daemon can hold `Arc<dyn BootstrapGraphBuilder>` and
/// swap implementations via DI / config.
#[async_trait]
pub trait BootstrapGraphBuilder: Send + Sync {
    /// Build a bootstrap graph for the given prompt and project context.
    ///
    /// `run_id` is the engine RunId already allocated by the caller; the
    /// builder may bake it into node IDs or leave it implicit. `worktree`
    /// is the absolute path of the worktree the run will execute in;
    /// builders that read project context (existing files, git status)
    /// should consult this directory only.
    async fn build(
        &self,
        run_id: RunId,
        prompt: BootstrapPrompt,
        worktree: PathBuf,
    ) -> Result<Graph, BootstrapBuildError>;
}

/// Free-text prompt + structured metadata that the builder may use.
///
/// `MinimalBootstrapGraphBuilder` only reads `description`. Future
/// `StagedBootstrapGraphBuilder` will read `title`, `tracker_url`,
/// `priority`, `labels` to populate the Description Author's prompt
/// preamble more richly.
#[derive(Debug, Clone)]
pub struct BootstrapPrompt {
    pub title: String,
    pub description: String,
    pub tracker_url: Option<String>,
    pub priority: Option<surge_intake::types::Priority>,
    pub labels: Vec<String>,
}

#[derive(Debug, Error)]
pub enum BootstrapBuildError {
    #[error("invalid prompt: {0}")]
    InvalidPrompt(String),
    #[error("graph construction failed: {0}")]
    GraphBuild(String),
    #[error("profile not available: {0}")]
    ProfileMissing(String),
}
```

#### 3.1.1 `MinimalBootstrapGraphBuilder`

The only production impl shipped in this task. Constructs a graph with:

- One `NodeKind::Agent` node (`id = "minimal-bootstrap-implementer"`).
  Profile = `implementer` (existing profile, ships with the engine; no
  new profile authoring required).
- System prompt prefix injected via `prompt_overrides`:
  ```
  You are working on this ticket from {tracker}:

  Title: {title}
  URL: {url}

  Description:
  {description}

  Implement the request directly in this worktree. Run tests before
  reporting done. If the request is ambiguous, escalate.
  ```
- Two outcomes: `done` (forward → Terminal::Success), `blocked`
  (forward → Terminal::Failure).
- One terminal-success node, one terminal-failure node.

This produces a real run that the engine drives end-to-end, with the
agent given full ticket context. It's "minimal" only in graph
structure, not in capability — the agent has the same tools and
sandbox as a CLI-initiated run.

#### 3.1.2 Why `BootstrapPrompt`, not just `&str`

Future RFC-0004 stages need typed access to title, URL, labels for
prompt template variables. We build the typed shape now so RFC-0004
doesn't have to widen the trait later.

#### 3.1.3 Wiring

`Engine` constructor (or its dependency-injection point in
`surge-daemon::main`) gains:

```rust
let bootstrap: Arc<dyn BootstrapGraphBuilder> =
    Arc::new(MinimalBootstrapGraphBuilder::new());
```

`InboxActionConsumer` holds an `Arc<dyn BootstrapGraphBuilder>`. When
RFC-0004 lands, the construction site changes — nothing else.

### 3.2 `InboxActionConsumer` (`surge-daemon`)

```rust
// crates/surge-daemon/src/inbox/consumer.rs (new file)

pub struct InboxActionConsumer {
    storage: Arc<Storage>,
    bootstrap: Arc<dyn BootstrapGraphBuilder>,
    engine: Arc<dyn EngineFacade>,
    sources: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    notifier: Arc<dyn NotifyDeliverer>,
    intake_db: Arc<TokioMutex<rusqlite::Connection>>,
    worktrees_root: PathBuf,
}

impl InboxActionConsumer {
    pub async fn run(self, shutdown: CancellationToken) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "inbox consumer tick failed");
            }
        }
        Ok(())
    }

    async fn tick(&self) -> Result<()> {
        // 1. Read pending Inbox*Requested events since last cursor.
        let events = self.storage.read_pending_inbox_requests().await?;
        for ev in events {
            match ev.payload {
                EventPayload::InboxRunStartRequested { task_id, callback_token, decided_via } => {
                    self.handle_start(&task_id, &callback_token, decided_via).await?;
                }
                EventPayload::InboxSnoozeRequested { task_id, callback_token, until, decided_via } => {
                    self.handle_snooze(&task_id, &callback_token, until, decided_via).await?;
                }
                EventPayload::InboxSkipRequested { task_id, callback_token, decided_via } => {
                    self.handle_skip(&task_id, &callback_token, decided_via).await?;
                }
                _ => continue,
            }
            self.storage.advance_inbox_cursor(ev.seq).await?;
        }
        Ok(())
    }
}
```

#### 3.2.1 Start handler

```rust
async fn handle_start(&self, task_id: &TaskId, callback_token: &str, via: ActionChannel) -> Result<()> {
    let row = {
        let conn = self.intake_db.lock().await;
        IntakeRepo::new(&conn).fetch_by_callback_token(callback_token)?
            .ok_or(InboxError::TokenNotFound)?
    };

    // Idempotency: if state already past InboxNotified, ignore.
    if !matches!(row.state, TicketState::InboxNotified | TicketState::Snoozed) {
        warn!(task_id = %task_id, state = ?row.state, "Start ignored — ticket no longer awaiting decision");
        return Ok(());
    }

    // Fetch fresh ticket details.
    let source = self.sources.get(&row.source_id).ok_or(InboxError::SourceNotConfigured)?;
    let details = source.fetch_task(task_id).await?;

    // Provision worktree.
    let run_id = RunId::new();
    let worktree = self.provision_worktree(&run_id).await?;

    // Build graph.
    let prompt = BootstrapPrompt {
        title: details.title.clone(),
        description: details.description.clone(),
        tracker_url: Some(details.url.clone()),
        priority: row.priority.as_deref().and_then(parse_priority),
        labels: details.labels.clone(),
    };
    let graph = self.bootstrap.build(run_id, prompt, worktree.clone()).await?;

    // Start the run.
    let handle = self.engine.start_run(run_id, graph, worktree, EngineRunConfig::default()).await?;

    // Update ticket_index: state RunStarted, run_id set, callback_token cleared.
    {
        let conn = self.intake_db.lock().await;
        let repo = IntakeRepo::new(&conn);
        repo.update_state_validated(task_id.as_str(), TicketState::RunStarted)?;
        repo.set_run_id(task_id.as_str(), run_id.to_string())?;
        repo.clear_callback_token(task_id.as_str())?;
    }

    // Post tracker comment.
    let comment = format!(
        "Surge run #{run_id} started — see {via} for progress.",
        run_id = run_id,
        via = via,
    );
    if let Err(e) = source.post_comment(task_id, &comment).await {
        warn!(error = %e, task_id = %task_id, "tracker comment on Start failed");
        self.storage.append_event(&run_id, EventPayload::TrackerCommentPostFailed {
            task_id: task_id.as_str().into(),
            attempt: 1,
            error: e.to_string(),
        }).await?;
    }

    // Spawn TicketStateSync to follow this run.
    let sync = TicketStateSync::new(
        task_id.clone(),
        Arc::clone(&self.intake_db),
        Arc::clone(source),
    );
    tokio::spawn(sync.run(handle));

    Ok(())
}
```

#### 3.2.2 Snooze handler

```rust
async fn handle_snooze(&self, task_id: &TaskId, callback_token: &str, until: DateTime<Utc>, via: ActionChannel) -> Result<()> {
    let row = {
        let conn = self.intake_db.lock().await;
        IntakeRepo::new(&conn).fetch_by_callback_token(callback_token)?
            .ok_or(InboxError::TokenNotFound)?
    };
    if !matches!(row.state, TicketState::InboxNotified) {
        return Ok(());
    }

    let conn = self.intake_db.lock().await;
    let repo = IntakeRepo::new(&conn);
    repo.update_state_validated(task_id.as_str(), TicketState::Snoozed)?;
    repo.set_snooze_until(task_id.as_str(), until)?;

    // Edit-in-place hook: future `surge-tg` extension reads
    // tg_chat_id+tg_message_id and calls editMessageText. MVP just leaves
    // the message; the user sees no immediate confirmation in the bot
    // beyond the answerCallbackQuery toast.
    Ok(())
}
```

#### 3.2.3 Skip handler

```rust
async fn handle_skip(&self, task_id: &TaskId, callback_token: &str, via: ActionChannel) -> Result<()> {
    let row = {
        let conn = self.intake_db.lock().await;
        IntakeRepo::new(&conn).fetch_by_callback_token(callback_token)?
            .ok_or(InboxError::TokenNotFound)?
    };
    if !matches!(row.state, TicketState::InboxNotified | TicketState::Snoozed) {
        return Ok(());
    }
    let source = self.sources.get(&row.source_id).ok_or(InboxError::SourceNotConfigured)?;

    {
        let conn = self.intake_db.lock().await;
        IntakeRepo::new(&conn).update_state_validated(task_id.as_str(), TicketState::Skipped)?;
    }

    if let Err(e) = source.set_label(task_id, "surge:skipped", true).await {
        warn!(error = %e, task_id = %task_id, "set_label surge:skipped failed");
    }
    if let Err(e) = source.post_comment(task_id, "Surge: ticket skipped by user.").await {
        warn!(error = %e, task_id = %task_id, "tracker comment on Skip failed");
    }
    Ok(())
}
```

### 3.3 `TicketStateSync` (per-run, in `surge-daemon`)

```rust
// crates/surge-daemon/src/inbox/state_sync.rs (new file)

pub struct TicketStateSync {
    task_id: TaskId,
    intake_db: Arc<TokioMutex<rusqlite::Connection>>,
    source: Arc<dyn TaskSource>,
}

impl TicketStateSync {
    pub async fn run(self, mut handle: RunHandle) {
        // Drive ticket_index FSM from engine events.
        loop {
            match handle.events.recv().await {
                Ok(EngineRunEvent::Persisted { payload, .. }) => {
                    if let EventPayload::RunStarted { .. } = payload {
                        let conn = self.intake_db.lock().await;
                        let _ = IntakeRepo::new(&conn).update_state_validated(
                            self.task_id.as_str(),
                            TicketState::Active,
                        );
                    }
                }
                Ok(EngineRunEvent::Terminal(outcome)) => {
                    self.on_terminal(&outcome).await;
                    return;
                }
                Err(_) => return, // sender dropped
            }
        }
    }

    async fn on_terminal(&self, outcome: &RunOutcome) {
        let (state, comment) = match outcome {
            RunOutcome::Completed { .. } => (TicketState::Completed, "✅ Surge run complete."),
            RunOutcome::Failed { error } => (TicketState::Failed, &format!("❌ Surge run failed: {error}")[..]),
            RunOutcome::Aborted { reason } => (TicketState::Aborted, &format!("Surge run aborted: {reason}")[..]),
        };
        {
            let conn = self.intake_db.lock().await;
            let _ = IntakeRepo::new(&conn).update_state_validated(self.task_id.as_str(), state);
        }
        let _ = self.source.post_comment(&self.task_id, comment).await;
    }
}
```

This subsystem is the **same shape** the future RFC-0004 staged
bootstrap will need — it doesn't care whether the run had a 1-node
graph or a 6-node graph. Engine events are engine events.

### 3.4 `TgInboxBot` (`surge-daemon`)

```rust
// crates/surge-daemon/src/inbox/tg_bot.rs (new file)
// (Module path chosen for easy extraction to a separate `surge-tg`
// crate later — keep dependencies minimal.)

pub struct TgInboxBot {
    bot: teloxide::Bot,
    chat_id: ChatId,
    storage: Arc<Storage>,
    intake_db: Arc<TokioMutex<rusqlite::Connection>>,
}

impl TgInboxBot {
    pub async fn run(self, shutdown: CancellationToken) -> Result<()> {
        // Outgoing: poll event log for InboxCardSent events lacking a
        // delivery ack, render and send.
        let outgoing = tokio::spawn(self.clone().outgoing_loop(shutdown.clone()));

        // Incoming: teloxide dispatcher on callback_query.
        let incoming = tokio::spawn(self.clone().incoming_loop(shutdown.clone()));

        let _ = tokio::join!(outgoing, incoming);
        Ok(())
    }

    async fn outgoing_loop(self, shutdown: CancellationToken) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        while !shutdown.is_cancelled() {
            interval.tick().await;
            let pending = self.storage.list_undelivered_inbox_cards().await?;
            for (event_seq, payload) in pending {
                let rendered = surge_notify::telegram::format_inbox_card(&payload);
                let kb = render_keyboard(&rendered.keyboard);
                let msg = self.bot
                    .send_message(self.chat_id, rendered.body)
                    .reply_markup(kb)
                    .await?;
                self.storage.record_inbox_card_delivered(
                    event_seq,
                    msg.id.0,
                    self.chat_id.0,
                ).await?;
                // Persist tg_chat_id + tg_message_id on ticket_index for
                // future edit-in-place.
                let conn = self.intake_db.lock().await;
                IntakeRepo::new(&conn).set_tg_message_ref(
                    payload.task_id.as_str(),
                    self.chat_id.0,
                    msg.id.0,
                )?;
            }
        }
        Ok(())
    }

    async fn incoming_loop(self, shutdown: CancellationToken) -> Result<()> {
        let handler = teloxide::dptree::entry()
            .branch(Update::filter_callback_query().endpoint(Self::on_callback));
        Dispatcher::builder(self.bot.clone(), handler)
            .dependencies(dptree::deps![self.storage.clone()])
            .build()
            .dispatch_with_listener(
                teloxide::update_listeners::polling_default(self.bot.clone()).await,
                LoggingErrorHandler::with_custom_text("teloxide error"),
            )
            .await;
        Ok(())
    }

    async fn on_callback(bot: Bot, q: CallbackQuery, storage: Arc<Storage>) -> ResponseResult<()> {
        let data = q.data.as_deref().unwrap_or("");
        let parts: Vec<&str> = data.splitn(3, ':').collect();
        let (action, token) = match parts.as_slice() {
            ["inbox", action, token] => (*action, *token),
            _ => {
                bot.answer_callback_query(q.id).text("Invalid action").await?;
                return Ok(());
            }
        };

        // Look up task_id by callback_token.
        let task_id = match storage.intake_lookup_task_by_token(token).await {
            Ok(Some(id)) => id,
            _ => {
                bot.answer_callback_query(q.id).text("Card expired").await?;
                return Ok(());
            }
        };

        let payload = match action {
            "start" => EventPayload::InboxRunStartRequested {
                task_id: task_id.clone(),
                callback_token: token.into(),
                decided_via: ActionChannel::Telegram,
            },
            "snooze" => EventPayload::InboxSnoozeRequested {
                task_id: task_id.clone(),
                callback_token: token.into(),
                until: Utc::now() + chrono::Duration::hours(24),
                decided_via: ActionChannel::Telegram,
            },
            "skip" => EventPayload::InboxSkipRequested {
                task_id: task_id.clone(),
                callback_token: token.into(),
                decided_via: ActionChannel::Telegram,
            },
            _ => {
                bot.answer_callback_query(q.id).text("Unknown action").await?;
                return Ok(());
            }
        };

        storage.append_inbox_request(payload).await?;
        bot.answer_callback_query(q.id).text("Recorded").await?;
        Ok(())
    }
}
```

#### 3.4.1 Why one cancellation token, two loops

teloxide's `Dispatcher` doesn't natively integrate with
`CancellationToken`; we let the outgoing loop self-terminate on
cancellation, while the incoming loop runs until the daemon's tokio
runtime drops. The shutdown grace window (`args.shutdown_grace`)
gives each in-flight Telegram call a chance to complete. Forcible
abort on grace-expiry happens via `JoinHandle::abort` in
`lifecycle::drain`.

### 3.5 Desktop receiver

`DesktopDeliverer` is augmented with action support:

```rust
// crates/surge-notify/src/desktop.rs (modify existing)

#[async_trait]
impl NotifyDeliverer for DesktopDeliverer {
    async fn deliver(...) -> Result<(), NotifyError> {
        // existing path...

        // For InboxCard, capture wait_for_action.
        if let Some(actions) = &rendered.actions /* new field */ {
            let task_id = actions.task_id.clone();
            let token = actions.callback_token.clone();
            let event_writer = Arc::clone(&self.event_writer);

            tokio::task::spawn_blocking(move || {
                let mut n = notify_rust::Notification::new();
                n.summary(&title)
                    .body(&body)
                    .action("inbox:start", "Start")
                    .action("inbox:snooze", "Snooze 24h")
                    .action("inbox:skip", "Skip")
                    .timeout(Timeout::Never);
                let handle = n.show().map_err(...)?;
                handle.wait_for_action(move |action_id| {
                    let _ = futures::executor::block_on(
                        forward_desktop_action(action_id, &task_id, &token, &event_writer)
                    );
                });
                Ok::<_, NotifyError>(())
            })
            .await
            .map_err(|e| NotifyError::Transport(format!("blocking task: {e}")))??;
        }
        Ok(())
    }
}
```

`forward_desktop_action` writes the same `Inbox*Requested` event as
the Telegram path, with `decided_via = ActionChannel::Desktop`.

`DesktopDeliverer` gains a constructor variant
`with_event_writer(...)` that surge-daemon uses; the existing
`new()` keeps compiling so other callers (tests, basic
notifications) work unchanged.

### 3.6 Snooze re-emission scheduler

```rust
// crates/surge-daemon/src/inbox/snooze_scheduler.rs (new file)

pub struct SnoozeScheduler {
    intake_db: Arc<TokioMutex<rusqlite::Connection>>,
    storage: Arc<Storage>,
    poll_interval: Duration,  // default 5 min
}

impl SnoozeScheduler {
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "snooze scheduler tick failed");
            }
        }
    }

    async fn tick(&self) -> Result<()> {
        let now = Utc::now();
        let due_rows: Vec<IntakeRow> = {
            let conn = self.intake_db.lock().await;
            IntakeRepo::new(&conn).fetch_due_snoozed(now)?
        };
        for mut row in due_rows {
            // Generate new callback token, transition state, re-emit.
            let new_token = ulid::Ulid::new().to_string();
            {
                let conn = self.intake_db.lock().await;
                let repo = IntakeRepo::new(&conn);
                repo.update_state_validated(&row.task_id, TicketState::InboxNotified)?;
                repo.set_callback_token(&row.task_id, &new_token)?;
                repo.clear_snooze_until(&row.task_id)?;
            }
            // Write fresh InboxCardSent event so TgInboxBot picks it up
            // and re-sends the card with the new token.
            self.storage.append_event_global(EventPayload::InboxCardSent {
                task_id: row.task_id.clone(),
                callback_token: new_token,
                channels: vec!["telegram".into(), "desktop".into()],
            }).await?;
        }
        Ok(())
    }
}
```

### 3.7 Schema migration: `0003_inbox_callback_columns.sql`

```sql
-- crates/surge-persistence/src/runs/migrations/registry/0003_inbox_callback_columns.sql

ALTER TABLE ticket_index ADD COLUMN callback_token TEXT;
ALTER TABLE ticket_index ADD COLUMN tg_chat_id INTEGER;
ALTER TABLE ticket_index ADD COLUMN tg_message_id INTEGER;

CREATE UNIQUE INDEX IF NOT EXISTS idx_ticket_index_callback_token
    ON ticket_index(callback_token)
    WHERE callback_token IS NOT NULL;
```

`UNIQUE` partial index — multiple rows can have NULL token (e.g.,
post-Start), but no two open cards can collide on token. This is
SQLite-supported (since 3.8.0).

### 3.8 New `IntakeRepo` methods

Added to `crates/surge-persistence/src/intake.rs`:

```rust
impl<'a> IntakeRepo<'a> {
    /// Validated state transition. Returns Err(InvalidTransition) if
    /// `to.is_valid_transition_from(current)` returns false.
    pub fn update_state_validated(&self, task_id: &str, to: TicketState)
        -> Result<(), IntakeError>;

    pub fn set_callback_token(&self, task_id: &str, token: &str) -> rusqlite::Result<()>;
    pub fn clear_callback_token(&self, task_id: &str) -> rusqlite::Result<()>;
    pub fn fetch_by_callback_token(&self, token: &str) -> rusqlite::Result<Option<IntakeRow>>;

    pub fn set_tg_message_ref(&self, task_id: &str, chat_id: i64, msg_id: i32)
        -> rusqlite::Result<()>;

    pub fn set_run_id(&self, task_id: &str, run_id: String) -> rusqlite::Result<()>;

    pub fn set_snooze_until(&self, task_id: &str, until: DateTime<Utc>)
        -> rusqlite::Result<()>;
    pub fn clear_snooze_until(&self, task_id: &str) -> rusqlite::Result<()>;

    pub fn fetch_due_snoozed(&self, now: DateTime<Utc>)
        -> rusqlite::Result<Vec<IntakeRow>>;
}
```

`update_state_validated` is the **only** way the new code mutates
state. The raw `update_state` becomes `pub(crate)` (internal use:
crash recovery, tests).

`IntakeRow` gains the three new columns as `Option<...>` fields.

### 3.9 `Storage` extensions for inbox events

Added to `crates/surge-persistence/src/runs/storage.rs` (or wherever
the global event log lives):

```rust
impl Storage {
    /// Append a global (not run-scoped) event. Currently the only callers
    /// are inbox actions (Inbox*Requested) and inbox card emissions
    /// (InboxCardSent). Stored in a new `global_events` table or the
    /// existing event log keyed with a synthetic `run_id = "global"`
    /// marker — see migration 0003 for the chosen approach.
    pub async fn append_event_global(&self, payload: EventPayload) -> Result<u64>;

    /// Read all Inbox*Requested events with seq > last cursor.
    pub async fn read_pending_inbox_requests(&self) -> Result<Vec<GlobalEventRow>>;
    pub async fn advance_inbox_cursor(&self, seq: u64) -> Result<()>;

    /// Outgoing leg of the bot loop.
    pub async fn list_undelivered_inbox_cards(&self) -> Result<Vec<(u64, InboxCardPayload)>>;
    pub async fn record_inbox_card_delivered(&self, seq: u64, msg_id: i32, chat_id: i64)
        -> Result<()>;

    /// Bot callback handler shortcut.
    pub async fn intake_lookup_task_by_token(&self, token: &str) -> Result<Option<TaskId>>;
    pub async fn append_inbox_request(&self, payload: EventPayload) -> Result<u64>;
}
```

`global_events` table schema (migration 0003 second part):

```sql
CREATE TABLE IF NOT EXISTS global_events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT NOT NULL,        -- "inbox_request" | "inbox_card_sent"
    task_id    TEXT NOT NULL,
    payload    BLOB NOT NULL,        -- bincoded EventPayload
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS global_event_cursors (
    consumer   TEXT PRIMARY KEY,     -- "inbox_consumer" | "tg_outgoing"
    last_seq   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS inbox_card_deliveries (
    event_seq  INTEGER PRIMARY KEY REFERENCES global_events(seq),
    chat_id    INTEGER NOT NULL,
    msg_id     INTEGER NOT NULL,
    delivered_at TEXT NOT NULL
);
```

The two cursor consumers (`inbox_consumer`, `tg_outgoing`) advance
independently. This is the standard at-least-once-with-cursor pattern.

### 3.10 New `EventPayload` variants

In `surge-core::run_event::EventPayload` (or wherever the canonical
event enum lives — the existing `EventPayload` is shared between
run-scoped and global events):

```rust
pub enum EventPayload {
    // existing variants...

    InboxCardSent {
        task_id: TaskId,
        callback_token: String,
        channels: Vec<String>,
    },
    InboxRunStartRequested {
        task_id: TaskId,
        callback_token: String,
        decided_via: ActionChannel,
    },
    InboxSnoozeRequested {
        task_id: TaskId,
        callback_token: String,
        until: DateTime<Utc>,
        decided_via: ActionChannel,
    },
    InboxSkipRequested {
        task_id: TaskId,
        callback_token: String,
        decided_via: ActionChannel,
    },
    InboxRunStarted {
        task_id: TaskId,
        run_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionChannel {
    Telegram,
    Desktop,
}
```

The existing `InboxDecided` variant from RFC-0010 Plan-C (Task 10.1)
is kept for backwards compatibility and emitted alongside the new
variants for now; future cleanup can remove it.

### 3.11 `InboxCardPayload` change

```rust
// crates/surge-notify/src/messages.rs (modify existing)

pub struct InboxCardPayload {
    pub task_id: TaskId,
    pub source_id: String,
    pub provider: String,
    pub title: String,
    pub summary: String,
    pub priority: Priority,
    pub task_url: String,
    /// Replaces the previous `run_id` field. The token is what callback
    /// data encodes; the actual `RunId` is generated only when the user
    /// taps Start and the engine creates the run.
    pub callback_token: String,
}
```

`format_inbox_card` and `format_inbox_card_desktop` updated to use
`callback_token` instead of `run_id` when building the
`inbox:<action>:<token>` strings. Snapshot tests are updated.

The daemon's `RouterOutput::Triage → InboxCardPayload` construction
site now:
1. Generates a fresh `callback_token` ULID.
2. Inserts (or updates) the `IntakeRow` with state=`InboxNotified`,
   `callback_token = <new token>`, `run_id = NULL`.
3. Writes `InboxCardSent` event to the global event log.
4. Does NOT call `notifier.deliver(...)` directly anymore — the
   `TgInboxBot` outgoing loop and `DesktopDeliverer` watch the event
   log and pull. (For desktop, this is a one-line shim: a tokio task
   that reads `list_undelivered_inbox_cards` and calls the existing
   formatter+deliverer path.)

---

## 4. Data flow: Start tap end-to-end

```
1. User taps ▶ Start in Telegram.
2. teloxide Dispatcher::on_callback fires.
3. Handler parses callback_data="inbox:start:01HKGZ..."
4. storage.intake_lookup_task_by_token("01HKGZ...") → task_id.
5. storage.append_inbox_request(InboxRunStartRequested { ... }).
6. bot.answer_callback_query(q.id).text("Recorded").await?
   ← user sees a brief toast.
7. surge-daemon's InboxActionConsumer ticks (every 500ms).
8. consumer.read_pending_inbox_requests() returns the new event.
9. consumer.handle_start():
   a. Fetch IntakeRow by callback_token.
   b. Verify state ∈ {InboxNotified, Snoozed}.
   c. source.fetch_task(task_id) → TaskDetails.
   d. provision_worktree(run_id) → PathBuf.
   e. bootstrap.build(run_id, prompt, worktree) → Graph.
   f. engine.start_run(run_id, graph, worktree, cfg) → RunHandle.
   g. IntakeRepo updates: state=RunStarted, run_id set, token cleared.
   h. source.post_comment("Surge run #N started — see Telegram for progress.").
   i. tokio::spawn(TicketStateSync::new(task_id, ...).run(handle)).
10. Engine writes RunStarted event in event log.
11. TicketStateSync.run():
    - Receives RunStarted → updates state=Active.
    - Eventually receives Terminal(Completed) → state=Completed,
      posts "✅ Surge run complete." to tracker.
12. consumer.advance_inbox_cursor(seq) — request is processed.
```

Total wall-clock: ~1 s for steps 1-6 (user-visible), ~1-2 s for
steps 7-9 (background), then minutes-to-hours for the run itself.

---

## 5. Error handling

| Scenario | Detection | Response |
|---|---|---|
| Stale callback token (card expired, state already past `InboxNotified`) | `fetch_by_callback_token` returns `None` or row state outside expected set | Bot replies "Card expired"; no event written; consumer no-op. |
| Same callback fires twice (double-tap, retried by Telegram) | Idempotency via state check (already past `InboxNotified`) | Second invocation is a no-op; consumer logs and advances cursor. |
| `source.fetch_task` fails on Start | RPC error from `TaskSource` | Log, append `TrackerCommentPostFailed { reason: "fetch_task" }` event, leave state at `InboxNotified` (user can retry). Consumer cursor still advances — we don't loop on failure. |
| `source.post_comment` fails on Start (after run already started) | RPC error | Log + `TrackerCommentPostFailed` event; state still progresses (run is going). Comment retry on the next FSM transition (Completed/Failed) will overwrite-not-duplicate via prefix detection (already implemented in `TaskSource` impls). |
| `Engine::start_run` fails (e.g. `RunAlreadyActive`) | `EngineError` | Log; ticket_index transitions back to `InboxNotified` so the user can retry; no tracker comment. |
| Telegram bot offline (network down, 5xx) | `teloxide::RequestError` | Outgoing loop logs, retries on next interval. Incoming loop self-recovers via teloxide's built-in long-poll resilience. No event-log writes happen until connectivity returns — events queue naturally. |
| Telegram chat ID not configured | `SURGE_TELEGRAM_CHAT_ID` env var missing AND `surge.toml` `[telegram] chat_id` missing | Log warning at daemon startup; `TgInboxBot` does not spawn. Inbox cards still go to Desktop if configured. |
| Desktop notify-rust fails (e.g. no DBus on Linux server) | `notify_rust::Error` | Log; the `DesktopDeliverer` returns `NotifyError::Transport`. Inbox card delivery on this card is lost; user only gets Telegram. |
| User taps Skip on a snoozed ticket before snooze expires | Valid path: state=Snoozed → state=Skipped is allowed by FSM | Honoured normally. |
| Daemon restart with InboxRunStartRequested event in flight (consumer crashed before advancing cursor) | Cursor still points to seq | On restart consumer re-reads the event. Idempotency via state-check + check that `run_id` already set on `ticket_index`. If `run_id IS NOT NULL` and state=`Active`/`Completed`, we know the run was already started — advance cursor without re-doing the work. |
| Snooze scheduler emits re-card while previous card still un-acked (rare race) | New `InboxCardSent` event before old delivery done | Outgoing loop skips it (the row's current callback_token has already changed; old event references the stale token, so when `intake_lookup_task_by_token` runs against it the lookup will fail or return a row whose token doesn't match). Bot answers "Card expired" if the old card is tapped. |
| `BootstrapGraphBuilder::build` returns `BootstrapBuildError` | e.g. profile registry doesn't have `implementer` | Append `TriageAuthorFailed` event (re-using existing variant for any bootstrap-side failure), state stays `InboxNotified`. Future: add `BootstrapBuildFailed` event variant. |
| Worktree provisioning fails (disk full, permissions) | `surge_git::WorktreeError` | Same as bootstrap-build failure — log + state unchanged + user can retry. |

### 5.1 Idempotency invariants

- **Each `Inbox*Requested` event is processed exactly once.** Cursor
  advance is the commit point; consumer crash before cursor advance
  → re-process on restart.
- **`update_state_validated` rejects invalid transitions.** Replays of
  the same event find the ticket in a terminal-or-progressing state
  and no-op.
- **`source.post_comment` is provider-idempotent for Linear** (idempotency
  key) and **best-effort prefix-deduped for GitHub** (existing impl).
  Re-posts on restart are at most cosmetic dups, not user-visible
  state.

### 5.2 Crash recovery on daemon restart

`InboxActionConsumer::run` resumes from the persisted cursor.
`SnoozeScheduler` is stateless across restarts (it polls
ticket_index every tick). `TicketStateSync` is per-run and lost on
restart — but the engine's M5 resume mechanism will re-broadcast
events when the run is resumed, and we re-spawn `TicketStateSync`
in the resume path.

For runs whose backing ticket has `state=RunStarted` but no
`Active`/terminal yet observed (i.e., daemon crashed between
`start_run` and the first FSM transition), startup recovery scans
`ticket_index WHERE state='RunStarted' AND run_id IS NOT NULL`,
re-subscribes to the run via `engine.resume_run(...)`, and spawns
`TicketStateSync` afresh.

---

## 6. Testing

### 6.1 Unit tests

- **`MinimalBootstrapGraphBuilder`**: produces a structurally valid
  `Graph` (passes `validate_for_m6`) for a sample `BootstrapPrompt`.
  Asserts the prompt text contains title, description, URL.
- **`IntakeRepo::update_state_validated`**: rejects all transitions
  that the existing FSM proptest's `is_valid_transition_from` rejects.
  Hand-coded: `Skipped → Active` Errors; `InboxNotified → RunStarted`
  passes.
- **`IntakeRepo::fetch_by_callback_token` / `set_callback_token` /
  `clear_callback_token`**: roundtrip + uniqueness-violation on
  duplicate token.
- **`IntakeRepo::fetch_due_snoozed`**: returns rows with
  `state='Snoozed' AND snooze_until <= now`; excludes `state='Skipped'`,
  `state='Snoozed' AND snooze_until > now`.

### 6.2 Component tests

- **`InboxActionConsumer::handle_start` (mocked engine)**: feeds an
  `InboxRunStartRequested` event, asserts:
  - `bootstrap.build` was called with the right prompt.
  - `engine.start_run` was called with a fresh `RunId`.
  - `ticket_index` row transitioned to `RunStarted`, `run_id` set,
    `callback_token` cleared.
  - `source.post_comment` was called with the expected body prefix
    "Surge run #".
- **`InboxActionConsumer::handle_snooze`**: asserts `state=Snoozed`,
  `snooze_until` set to `now + 24h ± 1m`.
- **`InboxActionConsumer::handle_skip`**: asserts `state=Skipped`,
  `set_label("surge:skipped", true)` called.
- **`SnoozeScheduler::tick`**: with two due rows + one not-yet-due,
  asserts only the two are re-emitted, with fresh callback tokens
  distinct from the old ones.
- **`TgInboxBot::on_callback`**: feed a synthetic `CallbackQuery`,
  assert the right `Inbox*Requested` payload was appended.

### 6.3 Integration test (`crates/surge-daemon/tests/inbox_callback_e2e.rs`)

```
Setup:
  - In-memory storage (with the new migrations applied)
  - MockTaskSource registered as "mock:t"
  - InMemoryEngineFacade (a thin test double that signals start_run
    success and immediately fires Terminal(Completed))
  - MinimalBootstrapGraphBuilder
  - InboxActionConsumer + SnoozeScheduler running with
    short tick intervals

Scenario A — Start happy path:
  1. Push a TaskEvent into MockTaskSource → router emits InboxCardSent.
  2. Verify ticket_index row exists with state=InboxNotified and
     callback_token set.
  3. Simulate a callback by directly appending
     InboxRunStartRequested with the captured token.
  4. Wait for consumer to process.
  5. Assert:
     - engine.start_run was called.
     - ticket_index state went InboxNotified → RunStarted → Active
       → Completed.
     - source.posted_comments contains exactly two entries:
       "Surge run #... started" and "✅ Surge run complete.".
     - callback_token is NULL on the final ticket_index row.

Scenario B — Snooze + auto re-emit:
  1. Same setup, but simulate InboxSnoozeRequested.
  2. Set snooze_until to now - 1s (in the past) by directly editing
     the row, to skip the 24h wait.
  3. Wait for SnoozeScheduler tick.
  4. Assert: a second InboxCardSent event appeared with a NEW
     callback_token (different from the original).
  5. Old token is no longer resolvable via intake_lookup_task_by_token.

Scenario C — Skip:
  1. Setup, simulate InboxSkipRequested.
  2. Wait for consumer.
  3. Assert: state=Skipped, source.set_labels contains
     ("surge:skipped", true), source.posted_comments contains the
     skip comment.

Scenario D — Stale callback (idempotency):
  1. Start path completes (state=Active).
  2. Simulate the same InboxRunStartRequested event again
     (e.g., daemon restart re-reads pre-cursor events).
  3. Assert: engine.start_run was called only once total. No double
     start. No new comment.

Scenario E — Engine failure rolls back:
  1. Configure mock engine to return RunAlreadyActive on start_run.
  2. Push InboxRunStartRequested.
  3. Assert: state stays at InboxNotified (no transition); no comment
     posted; the consumer cursor still advances (we don't infinite-loop).
```

### 6.4 What is NOT tested in this task

- Real LLM invocation in `MinimalBootstrapGraphBuilder` — requires
  an ACP agent process. Covered by existing engine integration tests
  for `Engine::start_run`.
- Real Telegram API roundtrip — requires a live bot token. The
  callback-handler logic is tested with synthetic `CallbackQuery`
  values; the Bot API surface is teloxide's well-tested code.
- Real Linear / GitHub API roundtrip — `MockTaskSource` covers the
  contract.

### 6.5 Coverage targets

| Component | Target |
|---|---|
| `BootstrapGraphBuilder` + `MinimalBootstrapGraphBuilder` | ≥ 85% |
| `InboxActionConsumer` | ≥ 85% |
| `TicketStateSync` | ≥ 80% |
| `SnoozeScheduler` | ≥ 90% |
| `TgInboxBot::on_callback` | ≥ 75% (parser branches) |
| New `IntakeRepo` methods | ≥ 90% |
| New `Storage` inbox-event helpers | ≥ 85% |

---

## 7. Configuration

`surge.toml` extended:

```toml
[telegram]
# Either set chat_id directly or via $SURGE_TELEGRAM_CHAT_ID env var.
chat_id_env = "SURGE_TELEGRAM_CHAT_ID"
# Optional override for testing.
# chat_id = 123456789
bot_token_env = "SURGE_TELEGRAM_BOT_TOKEN"

[inbox]
# Snooze re-emission scheduler.
snooze_poll_interval_seconds = 300

# Inbox-card delivery channels in priority order. Empty/missing →
# all configured channels deliver.
delivery_channels = ["telegram", "desktop"]
```

If `[telegram]` is absent:
- `TgInboxBot` doesn't spawn.
- Inbox cards still go to Desktop.
- All inbox actions still work via the Desktop receiver.

If `[inbox] delivery_channels` includes a channel that isn't
configured:
- Daemon logs `warn!("inbox channel X listed but not configured")`
  at startup.
- Cards continue to deliver to whatever IS configured.

### 7.1 Why `chat_id_env` not just `chat_id`

Following the existing RFC-0010 pattern for `api_token_env` —
secrets/identifiers stay out of `surge.toml`, which lives in git.
Direct `chat_id` is allowed for tests / local dev where the file is
gitignored.

---

## 8. File changes summary

### Created

```
crates/surge-orchestrator/src/bootstrap.rs                     (~150 LoC)
crates/surge-orchestrator/src/bootstrap/minimal.rs             (~200 LoC)
crates/surge-orchestrator/src/bootstrap/tests.rs               (~150 LoC)
crates/surge-daemon/src/inbox/mod.rs                           (~30 LoC)
crates/surge-daemon/src/inbox/consumer.rs                      (~350 LoC)
crates/surge-daemon/src/inbox/state_sync.rs                    (~120 LoC)
crates/surge-daemon/src/inbox/snooze_scheduler.rs              (~120 LoC)
crates/surge-daemon/src/inbox/tg_bot.rs                        (~250 LoC)
crates/surge-daemon/src/inbox/desktop_listener.rs              (~80 LoC)
crates/surge-daemon/tests/inbox_callback_e2e.rs                (~400 LoC)
crates/surge-persistence/src/runs/migrations/registry/0003_inbox_callback_columns.sql
crates/surge-persistence/src/runs/migrations/registry/0004_global_events.sql
```

### Modified

```
crates/surge-orchestrator/src/lib.rs               (re-export bootstrap)
crates/surge-orchestrator/Cargo.toml               (add surge-intake dep)
crates/surge-core/src/run_event.rs                 (new EventPayload variants, ActionChannel enum)
crates/surge-core/src/config.rs                    (TelegramConfig, InboxConfig)
crates/surge-persistence/src/intake.rs             (new IntakeRepo methods, IntakeRow fields)
crates/surge-persistence/src/runs/storage.rs       (Storage inbox-event helpers, global_events r/w)
crates/surge-notify/src/messages.rs                (InboxCardPayload: run_id → callback_token)
crates/surge-notify/src/telegram.rs                (format_inbox_card uses callback_token, snapshots updated)
crates/surge-notify/src/desktop.rs                 (format_inbox_card_desktop + DesktopDeliverer optional event-writer)
crates/surge-daemon/Cargo.toml                     (add teloxide, notify-rust event-writer feature)
crates/surge-daemon/src/main.rs                    (spawn TgInboxBot, InboxActionConsumer, SnoozeScheduler)
crates/surge-daemon/src/lib.rs                     (pub mod inbox)
```

### Deleted

None.

### Total estimated LoC

- New: ~1,950 LoC (≈1,300 production + 650 tests)
- Modified: ~600 LoC delta

---

## 9. Acceptance criteria

This task is complete when:

1. Tapping `▶ Start` on an inbox card in Telegram causes
   `ticket_index` to transition `InboxNotified → RunStarted →
   Active`, a real `Engine::start_run` to be invoked, and a
   "Surge run #N started" comment to be posted to the originating
   tracker — verified by integration test scenario A.
2. Tapping `⏸ Snooze 24h` sets `state=Snoozed`, `snooze_until = now
   + 24h`. After the snooze expires, the inbox card is re-emitted
   with a fresh callback token — verified by scenario B.
3. Tapping `✕ Skip` sets `state=Skipped`, sets the `surge:skipped`
   label on the tracker, and posts a skip comment — verified by
   scenario C.
4. The same callback fired twice (idempotency) results in exactly
   one run start, no duplicate comments — verified by scenario D.
5. `Engine::start_run` failure leaves `ticket_index` at
   `InboxNotified` (recoverable) — verified by scenario E.
6. `RunOutcome::Completed` posts "✅ Surge run complete." comment
   and transitions `state=Completed`. `RunOutcome::Failed` posts
   "❌ Surge run failed: <reason>" and `state=Failed` — verified
   by scenarios A + an additional fail variant.
7. Desktop action callbacks ("Start", "Snooze 24h", "Skip") write
   the same `Inbox*Requested` events with `decided_via=Desktop` —
   verified by a Desktop-receiver unit test using a mock event
   writer.
8. `cargo build --workspace`, `cargo test --workspace`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo fmt --all -- --check` — all clean.
9. `BootstrapGraphBuilder` trait is exported from
   `surge-orchestrator` and importable from `surge-daemon`. Future
   `StagedBootstrapGraphBuilder` (RFC-0004) can replace
   `MinimalBootstrapGraphBuilder` by changing only the daemon's
   construction site, no other module touched.
10. The bot loop module structure (`crates/surge-daemon/src/inbox/
    tg_bot.rs`) has only `surge-core`, `surge-persistence`,
    `surge-intake`, `surge-notify`, and `teloxide` as its
    dependencies — no `surge-orchestrator` import. Verified
    visually as a precondition for future `surge-tg` extraction
    (the consumer, which DOES import the orchestrator, stays in
    `surge-daemon`).

---

## 10. Future work (explicitly out of scope here, listed for traceability)

| When | What |
|---|---|
| RFC-0004 | `StagedBootstrapGraphBuilder` produces the 6-node prelude with Description Author / Roadmap Planner / Flow Generator profiles. Drop-in via DI. Approval cards on each stage flow through the existing `ApprovalRequested` ↔ `ApprovalDecided` event-log mechanism (RFC-0007), which lives next to but doesn't intersect this task's `Inbox*Requested` events. |
| RFC-0007 | Extract `surge-tg` standalone binary. The `inbox::tg_bot` module's dependencies are pre-curated to be a clean cut. Add `vibe telegram setup` binding-token flow. Add MarkdownV2 escaping + secrets filtering. |
| Future polish | Edit-in-place after action: use `tg_chat_id` + `tg_message_id` (already persisted by this task) to call `editMessageReplyMarkup` removing the keyboard and `editMessageText` adding "✓ accepted by you · 14:32" banner. |
| Future polish | Triage Author LLM dispatch (currently `Priority::Medium` placeholder in `RouterOutput::Triage` handler). Independent of this task — when it lands, the inbox card carries a real priority but the callback flow is unchanged. |
| Future polish | Webhook delivery mode for Telegram (RFC-0014). |
| Future polish | Multi-chat routing (per-source `chat_id_ref`), per-run mute / digest mode. |

---

## 11. Why this design (decision rationale)

1. **Event log over mpsc** — RFC-0007 explicitly specifies SQLite-backed
   communication between the bot service and engine daemons. Adopting
   it from day 1 avoids a refactor when `surge-tg` extracts. Cost:
   500 ms outgoing-poll latency. Benefit: crash safety, audit log,
   cross-process readiness.
2. **`BootstrapGraphBuilder` trait, not `BootstrapDispatcher`** — earlier
   draft considered a dispatcher abstraction. Reading `arch/03-engine.md`
   and RFC-0004 made clear that bootstrap is just a graph, not a
   separate execution mode. The trait collapses to "build a Graph from
   a prompt", which is the actual seam.
3. **`callback_token` separate from `run_id`** — `ticket_index.run_id`
   has FK to `runs(id)`. Pre-creating a fake run row to satisfy the FK
   would mean either dropping the FK (loses referential integrity) or
   creating an orphan row that the engine doesn't know about (audit
   confusion). A separate token column keeps both columns honest.
4. **Bot loop in surge-daemon, not in surge-notify** — `surge-notify`
   is a pure-formatter / pure-deliverer layer; adding a stateful long-
   running bot loop with event-log access would invert its dependency
   direction. The bot loop's natural home is wherever owns `Storage`,
   which is `surge-daemon`. Module boundaries are pre-curated for
   `surge-tg` extraction.
5. **`update_state_validated` over raw `update_state`** — closes the
   gap left by acceptance #10 (FSM proptest covers the predicate;
   nothing yet enforces it at write time). Pure win, low cost.
6. **Idempotency via state-check, not lock-table** — the FSM
   transitions themselves are the natural idempotency primitive.
   A `processing_locks` table or a Redis-style mutex would be
   overengineering for a single-daemon process; if multi-daemon ever
   becomes a real concern (it isn't on the roadmap), advisory locking
   per-`task_id` via SQLite `BEGIN EXCLUSIVE` covers it.

---

## 12. Spec self-review

**Placeholder scan.** No `TBD` / `TODO` markers. The "future polish"
items in §10 are explicitly out of scope, not deferred decisions
within this task.

**Internal consistency.** Bootstrap-graph-builder section (§3.1) and
the consumer's Start-handler (§3.2.1) reference the same trait
signature. `InboxCardPayload` change (§3.11) is reflected in
formatter callsites (§8 Modified). Schema migration (§3.7) matches
the new `IntakeRepo` methods (§3.8). Event variants (§3.10) match
the consumer's match arms (§3.2).

**Scope check.** Single concern: turning inbox-card taps into real
runs + state transitions + tracker comments. Bootstrap stages
(RFC-0004), bot service extraction (RFC-0007), and Triage Author LLM
dispatch are all explicitly excluded with named-RFC traceability.
File-change count (≈12 modified, ≈11 new) is within the
"single-implementation-plan" threshold from
`feedback_spec_scope_discipline`.

**Ambiguity check.**
- "ActionChannel" enum has explicit `Telegram | Desktop` variants;
  no string-typed channel.
- `ticket_index.callback_token` is `TEXT` with `UNIQUE` partial
  index — explicit on collision behaviour.
- `update_state_validated` always errors on invalid; never silently
  no-ops.
- Snooze re-emission generates a new token; the old token is
  rejected on lookup. Behaviour explicit, not "best effort".
