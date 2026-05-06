# RFC-0010 · Issue-tracker integration (Linear + GitHub Issues)

| | |
|---|---|
| Status | drafted |
| Updated | 2026-05-06 |
| Depends on | RFC-0001..0008, engine M1–M7 |
| Crate | `surge-intake` (new) + extensions to `surge-daemon`, `surge-persistence`, `surge-notify`, `surge-core` |
| Note | `surge-persistence` is the existing storage crate (uses `rusqlite`, not `sqlx`) |

## Summary

Surge integrates with external task trackers (Linear, GitHub Issues; future Discord/Jira/Slack/Notion) as a generalised **input axis** of the system. New tickets in trackers are detected via continuous polling, run through a triage pipeline (Tier-1 SQLite dedup → Triage Author LLM → priority assignment), surfaced to the user as **inbox cards** in Telegram and Desktop notifications, and (on user approval) flow into the existing vibe-flow bootstrap (Description Author → Roadmap Planner → Flow Generator → execution → PR). The tracker is the source of truth for ticket status; Surge writes only labels (`surge-priority/<level>`, `surge:skipped`) and comments back, never status changes.

## Motivation

The vibe-flow vision is `describe → walk away → return to a PR`. Without integration, the user types descriptions into the CLI or Telegram. With tracker integration, descriptions arrive automatically from the user's existing ticketing workflow — the same place they already write task descriptions for themselves and their team.

This delivers Symphony-class developer experience (OpenAI's reported +500% landed PRs in three weeks) while preserving Surge's agent-agnostic and source-agnostic positioning. Symphony is Codex-only and Linear-only; Surge handles any combination.

## Non-goals

- Approval UI inside trackers (approvals stay in Telegram; tracker gets informational comments)
- Surge-side dashboards / web UI (use Linear/GitHub native UI for inspecting tickets)
- Status transitions in trackers (Surge never sets `In Progress` / `Done`)
- Agent routing via labels (workflow / template decides routing — see #13 below)
- CI / PR-bot functions (out of scope; future RFC if needed)
- Cross-tracker sync (Linear ↔ GitHub) — out of scope
- Webhook-based ingestion (deferred to RFC-0014; trait designed to accept webhook implementation later)
- Cloud sandbox provider abstraction (deprecated direction; agents handle their own sandbox)

## Decisions

The following decisions were taken during brainstorming and are load-bearing for the design.

| # | Decision |
|---|---|
| 1 | This RFC is one of three in a ladder (RFC-0010, RFC-0011, RFC-0012) sitting above vibe-flow core (RFC-0001..0008). Sandbox abstraction explicitly excluded. |
| 2 | Issue-tracker integration ships first — it does not require new fundamentals; sits on existing `surge-spec`, `AgentPool`, FSM, daemon. |
| 3 | **Authority model = tracker is master.** Ticket status is the source of truth; Surge reacts to it, never sets it. Surge writes only labels and comments. |
| 4 | Linear and GitHub Issues are **separate `type` values** in the `task_sources` config array, symmetric with how `telegram` / `desktop` / `email` are separate notification channel types in RFC-0006. |
| 5 | **Automation levels via labels:** L0 (no label, ignored) / L1 (`surge:enabled`, full bootstrap with approvals) / L2 (`surge:template/<name>`, skip-bootstrap with template) / L3 (`surge:auto`, full auto incl. auto-merge). |
| 6 | Ticket → input for **Description Author** (existing bootstrap stage 1), not a pre-formed spec. The free-text description of a ticket feeds the same pipeline as CLI `vibe run "..."`. |
| 7 | New crate **`surge-intake`** with `trait TaskSource`, plus implementations `LinearTaskSource`, `GitHubIssuesTaskSource`. Future implementations (Discord, Jira, Slack, Notion) live in their own sub-crates or behind cargo features. |
| 8 | New bootstrap stage **Triage Author** — runs *before* Description Author. Profile lives at `~/.surge/profiles/_bootstrap/triage-author-1.0.toml`. Outcomes: `enqueued` / `duplicate` / `out_of_scope` / `unclear`. |
| 9 | **Continuous watcher is mandatory.** This is not on-demand integration — Surge's daemon polls trackers and reacts. |
| 10 | **Dedup target = full D**: Tier 1 computational (SQLite + embedding similarity) + Tier 2 inferential (Triage Author LLM judgement). Phased rollout: MVP ships Tier 1 step 1 (active-run lookup) only; embedding similarity for B/C lands in RFC-0014. |
| 11 | **Priority assignment = B**: Triage Author sets priority via LLM evaluation of ticket text. Surge does not read the user-set priority field of the tracker. Result is written as `surge-priority/<level>` label on the ticket for visibility. |
| 12 | **Polling-only in MVP**, abstraction-friendly trait (`fn watch_for_tasks() -> impl Stream<TaskEvent>`). Webhook implementation deferred to RFC-0014. |
| 13 | Agent / profile / sandbox selection happens in the workflow (template + Flow Generator), not via labels on tickets. Label namespace stays minimal (3 prefixes: `surge:enabled`, `surge:auto`, `surge:template/`). |
| 14 | **Telegram is universal cockpit-inbox**, not a task source. It sees all inbound from all `TaskSource`s, plus all approvals and status updates (RFC-0007 existing). |
| 15 | Tracker integration is one direction of work intake. Telegram's existing `/run` and CLI's `vibe run` cover ad-hoc inputs and remain unchanged. |
| 16 | Notifications channels (RFC-0006) and task sources (RFC-0010) are **two separate axes**. Do not conflate. |
| 17 | New message type **`InboxCard`** in `surge-notify` — for inbound tickets surfaced from any `TaskSource`. Sent in parallel to all configured notification channels. |
| 18 | Pluggable architecture so future `TaskSource` implementations (Discord, Jira, Slack, Notion, ...) ship as separate adapter crates without core changes. |
| 19 | **Inbox-cycle approval** added before vibe-flow bootstrap: `[▶ Start] [⏸ Snooze] [✕ Skip]`. This is on top of the existing 3 bootstrap approvals from RFC-0004. |
| 20 | Inbox cards delivered to **Telegram and Desktop in parallel** via `surge-notify` channels priority order. |
| 21 | `spec.md` / `adr.md` / `plan.md` are **execution-time artifacts**, produced by `spec-author` / `architect` profiles (not bootstrap stages). Approvals on them are optional via HumanGate nodes that Flow Generator may insert based on archetype (RFC-0004). |
| 22 | Tier-1 dedup reads from **SQLite storage** (event log + new `ticket_index` and `task_embeddings` materialized tables), no API calls in this layer. |
| 23 | After run is created from a ticket, Surge posts a comment to the tracker: "Surge run #N created, awaiting approval — see Telegram". |
| 24 | **Execution sandbox = native agent's sandbox.** Surge passes config (workspace path, allowlists) via ACP; the agent enforces. Tier 3+4 in RFC-0006 deprecated; refactor of RFC-0006 is a separate task. |
| 25 | Roadmap Planner system prompt (RFC-0004 stage 2) gets a **vertical-slice mandate** update: each milestone must be a deliverable end-to-end feature, anti-pattern is layered milestones (separate "all backend" / "all frontend"). Inspired by Matt Pocock's Sandcastle methodology. |
| 26 | **Token-budget guard-rail** lives in Roadmap Planner / Flow Generator (RFC-0004 refactor), not in Triage Author. Tasks inside Task Loops are validated to fit within `~60K` input tokens; if not, auto-split before user approval. Token-isolation per-iteration is already provided by Loop / Subgraph nodes (RFC-0003), so smart-zone problem is handled architecturally rather than by manual decomposition discipline. |

## Architecture

### High-level data flow

```
┌──────────────────────────────────────────────────────────────────────┐
│                    INPUT AXIS — Task Sources                          │
├──────────────────────────────────────────────────────────────────────┤
│                                                                       │
│   ┌─────────────────┐   ┌─────────────────┐                          │
│   │ LinearTaskSrc   │   │ GhIssuesTaskSrc │   in RFC-0010            │
│   └────────┬────────┘   └────────┬────────┘                          │
│            │                     │                                   │
│   ┌─────────────────┐   ┌─────────────────┐   future wrappers        │
│   │ CliTaskSource   │   │ TelegramTaskSrc │   (existing CLI/TG `/run`) │
│   │   (existing)    │   │   (existing)    │   + DiscordTaskSrc, etc.   │
│   └────────┬────────┘   └────────┬────────┘                          │
│            │                     │                                   │
│            └──────┬──────────────┘                                   │
│                   ▼                                                  │
│      ┌────────────────────────┐                                      │
│      │  TaskRouter            │  in surge-daemon                     │
│      │  (multiplex,            │                                      │
│      │   rate-limit, retry)    │                                      │
│      └────────────┬───────────┘                                      │
│                   ▼                                                  │
│      ┌────────────────────────┐                                      │
│      │ Tier-1 PreFilter       │                                      │
│      │   • active-run lookup  │  computational dedup                 │
│      │   • embedding similar. │                                      │
│      │   (cuts to ~15-25      │                                      │
│      │    candidates)         │                                      │
│      └────────────┬───────────┘                                      │
└───────────────────┼──────────────────────────────────────────────────┘
                    │
                    ▼
   ┌──────────────────────────────────────────────────────────────┐
   │  Triage Author (bootstrap stage 0, LLM)                       │
   │  inputs: TaskDetails + candidates + active runs                │
   │  outputs: triage_decision.json + inbox_summary.md              │
   │  outcomes: enqueued / duplicate / out_of_scope / unclear       │
   └──┬───────────────────────────┬──────────────────┬─────────────┘
      │ enqueued                  │ dup / oos        │ unclear
      ▼                           ▼                  ▼
   ┌─────────────────┐    ┌──────────────────┐   ┌─────────────────┐
   │ InboxCard sent  │    │ Notify: comment  │   │ HumanGate:      │
   │ (TG + Desktop)  │    │ "duplicate of"   │   │ ask user        │
   │ comment posted  │    │ → Terminal       │   │ → re-triage     │
   └────┬────────────┘    └──────────────────┘   └─────────────────┘
        │ user taps ▶ Start
        ▼
   ┌─────────────────────────────────────────────────────────────┐
   │  Existing vibe-flow bootstrap (RFC-0004):                    │
   │  Description Author → Roadmap Planner → Flow Generator       │
   │  Each gated by Telegram approval card.                       │
   └────┬────────────────────────────────────────────────────────┘
        │
        ▼
   ┌─────────────────────────────────────────────────────────────┐
   │  Pipeline execution (existing engine M5–M7).                 │
   │  spec.md / adr.md / plan.md produced as artifacts.           │
   │  Optional HumanGates per archetype.                          │
   │  PR composed → completion comment back to tracker.           │
   └─────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────────┐
│              OUTPUT AXIS — Notification Channels (RFC-0006)           │
├──────────────────────────────────────────────────────────────────────┤
│                                                                       │
│   Telegram   Desktop   Email   Slack   Webhook                       │
│   (cockpit-inbox: inbox cards, approvals, status updates)            │
│                                                                       │
└──────────────────────────────────────────────────────────────────────┘
```

### Architectural principles

1. **Tracker is master.** Ticket status is the SoT. Surge writes only labels and comments.
2. **Bootstrap-as-graph-nodes.** Triage Author is an ordinary Agent node visible on canvas, replayable, customisable. Not a magic pre-stage.
3. **Trait over implementations.** `trait TaskSource` defines the contract; `LinearTaskSource` and `GitHubIssuesTaskSource` implement it; future sources slot in without core changes.
4. **Tier-1 + Tier-2 dedup.** Computational pre-filter (no LLM) cuts noise; LLM Triage makes the final judgement. Maps cleanly to Fowler's harness engineering computational/inferential split.
5. **Polling-only in MVP, trait shape ready for webhook.** `watch_for_tasks` returns a `Stream`. Polling fills the stream now; webhook fills it later.
6. **Channels symmetry.** `task_sources[*].type` enumerates source kinds; `channels[*].type` enumerates notification kinds. Two separate config arrays.

### What we do NOT do

- Do not approve via tracker (UX in Telegram only)
- Do not provide a Surge web UI (use trackers' native UI)
- Do not change ticket status (only comments + labels)
- Do not route agents via labels (workflow decides)
- Do not act as a CI / PR-bot
- Do not cross-sync trackers

## Components

### `surge-intake` crate (new)

Workspace member at `crates/surge-intake/`. Contains the trait, implementations, and shared types.

```rust
#[async_trait]
pub trait TaskSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn provider(&self) -> &'static str;

    fn watch_for_tasks(&self) -> impl Stream<Item = Result<TaskEvent>> + '_;

    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails>;
    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>>;

    async fn acknowledge_task(&self, id: &TaskId) -> Result<()>;
    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()>;
    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()>;
    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>>;
}

pub struct TaskEvent {
    pub source_id: String,
    pub task_id: TaskId,
    pub kind: TaskEventKind,
    pub seen_at: DateTime<Utc>,
    pub raw_payload: serde_json::Value,
}

pub enum TaskEventKind {
    NewTask,
    StatusChanged { from: String, to: String },
    LabelsChanged { added: Vec<String>, removed: Vec<String> },
    TaskClosed,
}

pub struct TaskDetails {
    pub task_id: TaskId,
    pub source_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub labels: Vec<String>,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub assignee: Option<String>,
    pub raw_payload: serde_json::Value,
}

pub struct TaskSummary {
    pub task_id: TaskId,
    pub title: String,
    pub status: String,
    pub url: String,
    pub updated_at: DateTime<Utc>,
}
```

What is **not** in the trait: `set_status`, `assign_user`, `change_priority`. These violate the tracker-is-master principle.

### `LinearTaskSource`

GraphQL implementation against Linear's API. Dependencies: `reqwest`, `cynic` (typed GraphQL), `serde`. Polling implementation:

```rust
pub struct LinearTaskSource {
    client: LinearGqlClient,
    workspace_id: String,
    poll_interval: Duration,                   // default 60s
    label_filters: Vec<String>,                // ["surge:enabled", "surge:auto"]
    last_seen_cursor: Mutex<Option<String>>,
}
```

`watch_for_tasks` runs an async loop with `issueSearch` GraphQL query, filters by labels, emits `TaskEvent` for new/changed tickets. `updatedAt > last_seen_cursor` for efficiency.

### `GitHubIssuesTaskSource`

REST implementation via `octocrab`. Polls `/repos/{owner}/{repo}/issues?since=<last_seen>&labels=surge:enabled` (and similar for `surge:auto`).

### `TaskRouter` (in `surge-daemon`)

Multiplexes events from all configured `TaskSource`s into a single intake stream. Lives in the existing `surge-daemon` crate.

```rust
pub struct TaskRouter {
    sources: Vec<Box<dyn TaskSource>>,
    intake_tx: mpsc::Sender<TriageRequest>,
}

impl TaskRouter {
    pub async fn run(self) {
        let mut streams = self.sources.iter()
            .map(|s| Box::pin(s.watch_for_tasks()))
            .collect::<FuturesUnordered<_>>();
        
        while let Some(event) = streams.next().await {
            self.handle_event(event).await;
        }
    }
}
```

### Tier-1 PreFilter

Pure computational, no LLM. Module `surge-intake::dedup`. MVP step:

1. **Active-run lookup** (exact): `SELECT run_id FROM ticket_index WHERE task_id = ? AND state NOT IN ('Completed', 'Aborted', 'Skipped', 'Stale')`. If found → `EarlyDuplicate(run_id)`, no LLM stage.

Steps deferred to RFC-0014:
2. Embedding (cached, `fastembed-rs` or OpenAI) for the new ticket
3. Cosine similarity vs. open ticket embeddings → top-N candidates
4. Cosine similarity vs. recent spec embeddings → top-N candidates
5. Output top 15-25 overall

**Triage Author candidate set in MVP** is assembled by a separate computational step in `surge-intake::candidates` (not part of Tier-1 PreFilter): naive keyword overlap (top-15 by Jaccard similarity on title+description tokens against open tickets and recent specs). RFC-0014 replaces this with embedding-based selection (steps 2-5 above). Tier-1 PreFilter itself remains a single-step lookup in MVP.

### Triage Author profile

Bootstrap stage 0, `~/.surge/profiles/_bootstrap/triage-author-1.0.toml`.

```toml
id = "_bootstrap/triage-author"
display_name = "Triage Author"
version = "1.0"

[prompt]
system = """
You triage incoming tickets from external task sources.

You receive:
- The new ticket (title, body, labels, status, URL)
- List of currently active Surge runs and their associated ticket_ids
- Top 15-25 candidate tickets (similar open tickets + recent specs)
- Project context (cwd, language, top-level files)

Your job:
1. Decide whether this ticket is a duplicate, out-of-scope, unclear, or should be enqueued.
2. Assess priority (urgent/high/medium/low) from the ticket text and labels.
3. Output a structured decision.

Anti-patterns to avoid:
- Do not invent details that aren't in the ticket.
- Do not mark "duplicate" unless similarity is strong (>0.80 confidence).
- Always provide priority — never skip.
- Do not propose implementation; that's downstream.
"""

declared_outcomes = ["enqueued", "duplicate", "out_of_scope", "unclear"]
inputs_expected = ["task_payload.json", "candidates.json", "active_runs.json"]
outputs_produced = ["triage_decision.json", "inbox_summary.md"]

[sandbox]
mode = "read-only"
```

Output schema (`triage_decision.json`):

```json
{
  "decision": "enqueued",
  "duplicate_of": null,
  "priority": "high",
  "priority_reasoning": "Mentions production crash, affects parser core path",
  "summary": "Fix panic in parse_object when nested objects exceed depth 16"
}
```

`inbox_summary.md` is 3-5 lines used as the body of the inbox card.

### Storage extensions (`surge-persistence`)

New tables added to the existing SQLite schema (M2):

```sql
CREATE TABLE ticket_index (
    task_id     TEXT PRIMARY KEY,
    source_id   TEXT NOT NULL,
    provider    TEXT NOT NULL,
    run_id      TEXT,
    triage_decision TEXT,
    duplicate_of    TEXT,
    priority    TEXT,
    state       TEXT NOT NULL,           -- ticket FSM state
    first_seen  TEXT NOT NULL,
    last_seen   TEXT NOT NULL,
    snooze_until TEXT,

    FOREIGN KEY (run_id) REFERENCES runs(id),
    FOREIGN KEY (duplicate_of) REFERENCES ticket_index(task_id)
);

CREATE INDEX idx_ticket_index_source ON ticket_index(source_id);
CREATE INDEX idx_ticket_index_run    ON ticket_index(run_id);
CREATE INDEX idx_ticket_index_state  ON ticket_index(state);

CREATE TABLE task_embeddings (
    task_id     TEXT PRIMARY KEY,
    embedding   BLOB NOT NULL,
    model_id    TEXT NOT NULL,
    computed_at TEXT NOT NULL
);

CREATE TABLE task_source_state (
    source_id        TEXT PRIMARY KEY,
    last_seen_cursor TEXT,
    last_poll_at     TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0
);
```

### `InboxCard` notification (`surge-notify` extension)

```rust
pub enum NotifyMessage {
    // existing variants...
    InboxCard(InboxCardPayload),
}

pub struct InboxCardPayload {
    pub task_id: TaskId,
    pub source_id: String,
    pub provider: String,
    pub title: String,
    pub summary: String,
    pub priority: Priority,
    pub task_url: String,
    pub run_id: RunId,
}
```

Telegram inline keyboard:

```
📋 Task from Linear · ABC-42

Add tracing to auth middleware
priority: high (auto-detected)

[ ▶ Start ] [ ⏸ Snooze 24h ] [ ✕ Skip ]
[ View ticket ↗ ]
```

Desktop notification (`notify-rust`):
- Title: "📋 New Surge task"
- Body: 1-line summary + priority
- Actions: Start / Snooze / Skip

### Configuration in `surge.toml`

```toml
[[task_sources]]
type = "linear"
id = "linear-acme"
workspace_id = "wsp_acme_123"
api_token_env = "LINEAR_API_TOKEN"
poll_interval_seconds = 60
label_filters = ["surge:enabled", "surge:auto"]

[[task_sources]]
type = "github_issues"
id = "github-myapp"
repo = "myuser/myapp"
api_token_env = "GITHUB_TOKEN"
poll_interval_seconds = 60
label_filters = ["surge:enabled"]

[[channels]]
type = "telegram"
chat_id_ref = "$DEFAULT"

[[channels]]
type = "desktop"
duration = "persistent"
```

## Data flow

### Ticket FSM (in `ticket_index`)

```
[TaskEvent: NewTask]
        │
        ▼
    ┌────────┐
    │ Seen   │
    └───┬────┘
        │
┌───────┼──────────────┬────────────────┬────────────────┐
▼       ▼              ▼                ▼                ▼
[Tier1Dup] [Triaged] [TriagedDup] [TriagedOOS] [TriagedUnclear]
            │
            ▼
       ┌─────────────┐
       │ InboxNotified │
       └────┬──────────┘
            │
   ┌────────┼─────────────┬──────────────┐
   ▼        ▼             ▼              ▼
[Snoozed] [Skipped]  [RunStarted]    [Stale]
                          │           (no resp 7d)
                          ▼
                     ┌────────┐
                     │ Active │
                     └───┬────┘
                         │
             ┌───────────┼─────────────┐
             ▼           ▼             ▼
         [Completed] [Failed]      [Aborted]
```

Full state semantics in section 3.1 of the design discussion. Summary: ticket FSM tracks the **integration lifecycle** of a ticket, parallel to but not identical with the run lifecycle. A ticket can exist without a run (Skipped, Snoozed); a run can outlive a ticket being closed (we still finish the PR even if the user marked the ticket Done early).

### Sync rules (ticket-state ↔ run-state)

| Event | Source | Ticket index update | Tracker action |
|-------|--------|--------------------|----------------|
| `TaskEvent::NewTask` | polling | `INSERT` row, state=Seen | — |
| `TaskEvent::StatusChanged{to: Closed}` | polling | if Active → Aborted | — |
| `TaskEvent::LabelsChanged{added: surge:skipped}` | polling | state=Skipped | — |
| User taps Snooze | TG callback | state=Snoozed, snooze_until set | — |
| User taps Skip | TG callback | state=Skipped | label `surge:skipped` set |
| User taps Start | TG callback | state=RunStarted, run created | comment "Surge run #N started" |
| `Run::Started` | engine | state=Active | comment "Bootstrap stage 1/3..." |
| `Run::HumanGateRequested` | engine | (no change) | comment "Approval pending — see Telegram" |
| `Run::Completed` (PR merged) | engine | state=Completed | comment "✅ PR #47 merged" |
| `Run::Failed` | engine | state=Failed | comment "❌ Run failed: <reason>" |
| `Run::Aborted` | engine | state=Aborted | comment "Run aborted by user" |

What Surge does **not** do: change ticket status, assign users, close tickets even after merge.

### New event types

Added to `EventPayload` enum in `surge-core`:

```rust
pub enum EventPayload {
    // existing...

    TicketDetected         { task_id, source_id, provider, raw_payload },
    Tier1DedupDecided      { task_id, decision, candidates },
    TriageDecided          { task_id, decision, priority, duplicate_of, reasoning },
    InboxCardSent          { task_id, run_id, channels },
    InboxDecided           { task_id, run_id, decision, decided_via, decided_at },
    TrackerCommentPosted   { task_id, comment_id, body_summary, purpose },
    TrackerCommentPostFailed { task_id, attempt, error },
    TrackerLabelChanged    { task_id, label, present },
    TrackerLabelSetFailed  { task_id, label, error },
    TaskSourcePollFailed   { source_id, attempt, error, retry_in },
    TaskSourceAuthFailed   { source_id, error },
    TriageAuthorFailed     { task_id, attempt, error },
    TriageStaleRecovery    { task_id, run_id, reason },
    UserMentionReceived    { task_id, comment_id, body },
}
```

All these go into the existing event log (M2). Replay, audit, time-travel work for tracker-related runs identically to other runs.

### Idempotency

- `post_comment`: pre-check existing comments by telltale prefix `Surge run #N: ...` (Linear has idempotency keys; GitHub does not).
- `set_label`: natively idempotent.
- `acknowledge_task`: writes only to local SQLite, no API call.

## Error handling

### Provider availability

| Scenario | Detection | Action |
|----------|-----------|--------|
| Single poll fail | timeout / 5xx | Backoff retry: 60→120→240→480→600s, jitter ±20% |
| 5 consecutive failures | counter | Emit `TaskSourcePollFailed`, desktop alert, slow mode (10 min interval) |
| 401/403 | HTTP status | Stop polling source. Critical alert via all channels. Existing runs continue. |
| 429 (rate-limit) | HTTP status | Sleep until `Retry-After` + 10% jitter |
| Schema change | parse error | Log warning, skip event, continue. |

**Critical principle:** tracker offline ≠ Surge offline. Existing runs continue.

### Mid-run ticket changes

| Change | Action |
|--------|--------|
| Ticket status → Done | Run continues; on completion post PR comment + state=Completed. Don't abort. |
| Ticket status → Cancelled | Pause run, send approval card "Ticket cancelled. Continue? [Continue] [Abort]". |
| `surge:enabled` label removed | No effect (labels gate intake, not lifecycle). |
| `surge:auto` label added | If at HumanReview gate, skip approval; otherwise no effect. |
| Title/description edited | No effect (bootstrap took snapshot). |
| New comment with `@surge` mention | Inject as new HumanGate "User feedback received: <body>. Apply? [Apply] [Ignore] [Pause]" |
| Priority changed manually | No effect. |
| Assignee changed | No effect. |

Rationale: ticket after triage is a read-only snapshot. Live-sync of edited descriptions into running pipeline = chaos. User edits → use Telegram Edit approval, not the tracker.

### Crash recovery

Daemon startup re-reads `ticket_index`, scans for inconsistent states:

- state=Seen without Tier-1 → re-run Tier-1
- state=Triaged without InboxCard sent → send (if not stale > 1h)
- state=InboxNotified without user response → continue waiting
- state=RunStarted without linked active run → log corruption warning, mark Aborted
- state=Active with run.status=Completed → post completion comment (idempotent)

Borderline scenario: crash between Triage approve and run start. ticket_index has state=RunStarted but no materialised run. Recovery: lookup `runs` table; if absent, mark state=TriageStale, send a fresh inbox card "Run did not start due to crash. Retry?".

### Bootstrap stage failures

| Failure | Recovery |
|---------|----------|
| Triage Author returns invalid JSON | Retry up to 3× with feedback. Then outcome=unclear → HumanGate. |
| Triage timeout (> 5 min) | Cancel session, retry once. Then outcome=unclear → HumanGate. |
| Description Author can't proceed | outcome=unclear (RFC-0004 existing) |
| Flow Generator `cannot_generate` | HumanGate (RFC-0004 existing) |
| Triage with no candidates available | Continue text-only; log warning. |

If Triage permanently fails (3× attempts), HumanGate sends "Triage could not decide for this ticket. Force-start? [Force start] [Skip] [View raw]".

### Concurrency

- Tier-1 PreFilter uses `INSERT ... ON CONFLICT DO NOTHING` for idempotency.
- Polling: max 1 active poll per `source_id` (mutex). Different sources poll in parallel.
- Subsequent events for same ticket already in Triage → update last_seen, no new triage.
- After Triage completes, if labels significantly changed (e.g. `surge:auto` added), re-eval with new context.

### Graceful shutdown

On SIGTERM or `surge daemon stop`:
1. Polling cancels in-flight fetches.
2. In-flight comments / labels finish (10s timeout).
3. Triage Author session marks `paused`, persistent (M5 existing).
4. Already-sent inbox cards remain in Telegram; user replies dispatched on next start.
5. Active runs gracefully pause via M5 mechanism.
6. `last_seen_cursor` per source persisted for smooth resume.

## Testing

### Unit testing (`MockTaskSource`)

```rust
pub struct MockTaskSource {
    id: String,
    events: Mutex<VecDeque<TaskEvent>>,
    open_tasks: Mutex<HashMap<TaskId, TaskDetails>>,
    posted_comments: Mutex<Vec<(TaskId, String)>>,
    set_labels: Mutex<Vec<(TaskId, String, bool)>>,
}
```

Tests cover:
- TaskRouter multiplexes correctly
- Tier-1 PreFilter distinguishes Pass / EarlyDuplicate
- ticket_index FSM transitions valid

Coverage target: ≥85% on `surge-intake` core.

### Provider integration tests

- Linear: dedicated test workspace, token in `LINEAR_TEST_API_TOKEN` GitHub Secret. Tests for new-issue detection, label filtering, idempotent comments, pagination.
- GitHub: test repo, PAT in `GITHUB_TEST_PAT`. Symmetric test set.
- Cassette mode (`vcr-rs` or similar) for offline development without secrets. CI nightly runs against real APIs.

### Triage Author deterministic testing

- **Layer 1**: prompt unit tests (no LLM). Snapshot-test rendered prompt; assert all inputs present.
- **Layer 2**: 20+ fixture-based scenarios in `tests/triage_fixtures/*.toml` (handcrafted task + candidates → expected decision/priority). Run against Claude Haiku at `temperature=0`.
- **Layer 3**: tolerance bands. `decision` must match exactly; `priority` ±1 acceptable; `reasoning` keyword presence.

CI: ignored by default; nightly cron runs all three layers with `ANTHROPIC_TEST_KEY`.

### End-to-end mock pipeline

Single comprehensive test: mock GitHub source → mock ACP agent (scripted responses) → mock notify channel (auto-approves all) → assert run reaches Completed in < 30s. Verifies full intake pipeline without external dependencies.

### Crash recovery tests

Persistent SQLite state, simulate SIGKILL after various points (after Triage decided / after inbox sent / after user start / mid-run). Restart daemon, verify state consistency and absence of duplicate work (e.g. Triage doesn't re-run if already decided).

### Polling behaviour tests

`tokio::time::pause()` for deterministic timing. Verify backoff sequence on simulated failures. Verify `Retry-After` honoured on 429.

### Coverage targets

| Component | Target |
|-----------|--------|
| `surge-intake` core | ≥ 85% |
| `LinearTaskSource` (mock client) | ≥ 80% |
| `GitHubIssuesTaskSource` (mock client) | ≥ 80% |
| `Tier1PreFilter` | ≥ 90% |
| Triage Author rendering + parser | ≥ 85% |
| `ticket_index` storage | ≥ 90% |
| End-to-end mock pipeline | 1+ scenario |

### CI matrix

```yaml
unit-tests:                   # all PRs
integration-mock:             # all PRs, no secrets
integration-providers:        # nightly cron
  env:
    LINEAR_TEST_API_TOKEN
    GITHUB_TEST_PAT
    ANTHROPIC_TEST_KEY
```

## Acceptance criteria

The RFC is implemented when:

1. `surge-intake` crate compiles, exports `TaskSource` trait, ships `LinearTaskSource` and `GitHubIssuesTaskSource` impls.
2. Configuring two sources in `surge.toml` (Linear + GitHub) and starting the daemon polls both successfully (verified via test workspace + test repo).
3. Creating a ticket in Linear with label `surge:enabled` triggers (within 90s) a Triage Author run, then an inbox card in Telegram + Desktop.
4. Tapping ▶ Start in Telegram begins the existing vibe-flow bootstrap (Description → Roadmap → Flow), with each stage gated by approval cards.
5. Successful run completion posts a "✅ PR #N ready" comment to the originating ticket. Failure posts "❌ Run failed: <reason>".
6. Triage Author correctly identifies an active-run duplicate (Tier-1) and posts "duplicate of #M" comment without starting a new run.
7. Tracker API outage (simulated 5xx for 5 minutes) does not crash the daemon; existing runs continue; backoff sequence verified in event log.
8. Daemon SIGKILL during Triage Author is recovered on restart; re-running Triage is skipped if `triage_decision.json` is already in the run's artifacts.
9. `surge:auto` label triggers L3 — full automation through to merge without approval cards beyond inbox.
10. Property tests verify `ticket_index` FSM transitions valid for all input sequences.
11. `cargo clippy --workspace -- -D warnings` clean.
12. CI green on Linux/macOS/Windows.

## Open questions

### Q1 — Linear webhook subscription via GraphQL?

Linear supports streaming subscriptions. We chose polling for MVP simplicity and consistency with GitHub (which has webhooks but they require public IP). Should we offer Linear streaming as a higher-fidelity alternative even in MVP? Argument for: very-low-latency. Argument against: complexity, requires stable WebSocket, breaks symmetry with GitHub. Default position: defer to RFC-0014.

### Q2 — Inbox card stale handling

Inbox cards that wait > 7 days without response — auto-Skip or auto-prompt-again? MVP picks auto-mark `Stale`. Could be revisited based on user feedback.

### Q3 — `@surge` mention in tracker comments

Initially designed as HumanGate injection. But: should it be limited to ticket author, or anyone? In a shared GitHub repo, anyone could comment `@surge`. Mitigation: only honour mentions from accounts in `task_sources[*].trusted_users` allowlist (configurable). Default = ticket author only.

### Q4 — Multi-tenant repos

If the user configures one Linear workspace and 3 GitHub repos, all under the same daemon, the inbox can become noisy. Should there be per-source mute / digest mode? Out of scope for RFC-0010; flagged for future RFC-0011 / harness templates work.

## Future work

| RFC | Adds |
|-----|------|
| RFC-0011 | Async subagents — supervisor pattern, parallel ticket runs without blocking the orchestrator. Builds on `AgentPool` + `AdmissionController`. |
| RFC-0012 | Harness templates (Fowler) — guides + sensors as reusable blueprints, bound to ticket archetypes. |
| RFC-0013 | (Reserved for OS-local hardening if real demand emerges. Currently low priority.) |
| RFC-0014 | Webhook ingestion (GitHub Apps, Linear webhooks). Embedding-based Tier-1 dedup steps 2-5. Linear streaming subscription. Optional `DiscordTaskSource`. |
| RFC-0015 | Refactor RFC-0006 to deprecate Tier 3+4 (delegate to native agent sandbox). |
