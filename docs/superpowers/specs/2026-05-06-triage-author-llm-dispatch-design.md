# Triage Author LLM Dispatch — Design

**Status:** Design (drafted 2026-05-06)
**Predecessor:** RFC-0010 Plan-C-polish (5/6 items shipped, this is item 6)
**Successor:** Layer 2 — Triage Author as graph node (separate RFC, deferred)
**Owner:** vanya · `feat(orchestrator): Triage Author LLM dispatch via ACP`

> **Scope summary.** Replace the `Priority::Medium` placeholder in
> `surge-daemon/src/main.rs` with a real ACP-driven Triage Author call.
> When `RouterOutput::Triage { event }` arrives, the daemon assembles
> a `TriageInput`, opens an ACP session against the bundled
> `_bootstrap/triage-author@1.0` profile, awaits a `triage_decision.json`
> artifact, parses via `TriageJson::into_decision`, and routes the
> resulting `TriageDecision` to one of four destinations: real
> `InboxCardPayload`, duplicate-comment-on-tracker, out-of-scope-comment,
> or HumanGate notification.
>
> Closes RFC-0010 acceptance criterion #3 ("priority becomes
> LLM-derived") and unblocks #11 ("clippy clean across the full LLM path").

---

## 1. Goals and non-goals

### 1.1 In scope

- **`dispatch_triage(...)` async function** in
  `crates/surge-orchestrator/src/triage.rs` that opens an ACP session,
  sends a structured input, awaits the agent's decision, parses it,
  and returns a typed `TriageDecision`.
- **File-artifact return path** — agent writes
  `triage_decision.json` and `inbox_summary.md` to its session
  scratch directory. The dispatcher reads these after
  `BridgeEvent::OutcomeReported` fires. This matches the established
  bootstrap-stage pattern (see Description Author in
  `docs/revision/components/profiles.md`) and RFC-0010 acceptance
  criterion #8 (SIGKILL recovery via persisted artifacts).
- **Retry semantics** — three attempts per call with three distinct
  failure modes (timeout, agent crash, malformed JSON). Final fallback
  is `TriageDecision::Unclear { question: "<diagnostic>" }` so the
  daemon can always make forward progress.
- **Daemon mapping** — the existing `RouterOutput::Triage { event }`
  arm in `crates/surge-daemon/src/main.rs` routes to one of four
  destinations based on the parsed decision.
- **Candidate-set assembly** — small helper in
  `surge-intake::candidates` that calls `source.list_open_tasks()`
  and reduces via the existing `top_by_keyword_overlap` to top-15.
- **Active-runs snapshot** — `Storage::snapshot_active_runs()`
  accessor (added if absent) that lists currently active runs as
  `ActiveRunSummary` for inclusion in `TriageInput`.
- **Explicit handling when Claude binary is missing** —
  `surge_acp::discovery` finds the `claude` binary; if unavailable,
  the dispatcher returns
  `Ok(TriageDecision::Unclear { question: "Claude binary not configured (set SURGE_CLAUDE_BINARY or install claude); install to enable LLM-driven triage" })`
  on the first attempt without spinning up an ACP session. This
  surfaces the misconfiguration to the user via the standard Unclear
  notification path. **No silent fallback to a Mock agent in
  production code paths** — Mock is reserved for tests.
- **Feature-gated end-to-end LLM test** — under `--features
  _bootstrap_llm_test`, run real Claude Haiku against the three
  existing fixtures (`enqueue_001.toml`, `duplicate_001.toml`,
  `out_of_scope_001.toml`). Decision must match exactly; priority
  ±1 tolerance.

### 1.2 Out of scope (deferred to Layer 2 RFC)

- **Triage Author as `NodeKind::Agent` inside the engine state
  machine.** This requires extending `RunState::Bootstrapping`,
  introducing a "shadow run" or "intake run" lifecycle, persisting
  triage as event-log entries, and wiring crash recovery through the
  engine's `recover_runs` path. That work is a separate RFC of
  comparable size to RFC-0010 itself.
- **Embedding-based candidate selection.** RFC-0014 deliverable.
- **Persistent triage event log.** Layer 1 keeps triage outside the
  event-sourced run history. Layer 2 promotes it.
- **Web-search MCP for triage.** Profile sandbox stays read-only;
  triage works strictly from supplied inputs.
- **HumanGate-as-graph-node for `Unclear`.** Layer 1 surfaces
  `Unclear` via a one-shot `RenderedNotification` on the desktop
  channel; full inbox-cycle handling is a separate concern.

### 1.3 Why two layers

Promoting Triage Author to a real graph node is the architecturally
correct end state — see RFC-0010 §"Bootstrap-as-graph-nodes" and
RFC-0004 §"Bootstrap stages as graph nodes". But it requires:

1. New `RunState` variant covering pre-Description triage.
2. Run-creation-on-incoming-ticket logic in the daemon.
3. Engine-level dispatch of `_bootstrap/triage-author` profile via
   the existing `execute_agent_stage` machinery.
4. New `EventPayload` variants for triage decisions.
5. Crash recovery covering the new state.
6. Acceptance-criterion #8 requires the artifact-recovery path.

That work is a separate RFC. **Layer 1 deliberately picks a surface
that does not paint Layer 2 into a corner**: file artifacts, standard
`report_stage_outcome` flow, and `BridgeFacade` usage are all
compatible with the future migration. When Layer 2 lands,
`dispatch_triage` either becomes a thin test wrapper around the
engine's executor or its body is inlined into the Triage stage's
profile-handler. No API of `TriageInput` / `TriageJson` /
`TriageDecision` changes.

---

## 2. Architecture decision

### 2.1 The three rejected alternatives

| Alternative | Why rejected |
|---|---|
| **A. JSON-into-`summary` field** of `report_stage_outcome` | `summary` is contractually a 1-3 sentence rationale (see `crates/surge-acp/src/bridge/tools.rs:97-99`). Repurposing it abuses the contract, requires retry-with-prompt-feedback when the agent ignores the convention, and forces every future bootstrap stage to invent its own JSON convention. |
| **B. Custom tool `submit_triage_decision`** with `TriageJson` schema | Type-safe at the ACP wire level, but introduces a tool category specific to one stage. Description / Roadmap / Flow Authors would then need their own custom tools for their own structured outputs, fragmenting the bootstrap stage protocol. |
| **C-naive. Replace `[sandbox] mode = "read-only"` with writable** | Profile semantics matter: read-only declares intent ("don't touch the project source"). Changing it solves nothing — agents can already write to their own working directory regardless of project sandbox mode. |

### 2.2 The chosen approach: artifact files via session scratch directory

**Triage Author writes `triage_decision.json` and `inbox_summary.md`
to its session working directory using ACP-native filesystem tools.
After `report_stage_outcome` fires, the dispatcher reads the files,
parses the JSON, and returns a typed `TriageDecision`.**

Why this matches the long-term architecture:

1. **Uniform with Description / Roadmap / Flow Author.** All four
   bootstrap stages produce file artifacts (`description.md`,
   `roadmap.md`, `flow.toml`, `triage_decision.json`). One pattern,
   four instances.
2. **Read-only sandbox is consistent.** The sandbox's `read-only`
   mode protects the *project source*, not the agent's own working
   directory. Description Author already writes `description.md`
   under `default_mode = "read-only"` (see
   `docs/revision/components/profiles.md`).
3. **Crash recovery is intrinsic.** Acceptance criterion #8 of
   RFC-0010 asks: "Daemon SIGKILL during Triage Author is recovered
   on restart; re-running Triage is skipped if `triage_decision.json`
   is already in the run's artifacts." The file-artifact path
   delivers this for free in Layer 2 (Layer 1 keeps triage outside
   run history, so crash-recovery defers).
4. **Layer 2 migration is a no-op for the public surface.** The
   scratch directory becomes `$WORKTREE/.vibe/runs/$RUN_ID/artifacts/`
   without any change to `dispatch_triage`'s caller-visible types.

### 2.3 Where the dispatcher lives

`crates/surge-orchestrator/src/triage.rs` — same module as
`TriageInput` / `TriageJson`. The orchestrator crate already depends
on `surge-acp` (for `BridgeFacade`) and `surge-intake` (for the
typed decision and provider trait). Daemon stays a thin wiring
layer; the actual LLM call lives in the orchestrator alongside the
typed contract.

### 2.4 Session lifetime

**Short-lived per call.** Triage runs are infrequent (≥60s between
successive calls per `task_source` due to polling intervals;
duplicates are filtered by Tier-1 dedup before reaching Triage).
Session-pool overhead (idle timeouts, bindings churn, connection
reuse) buys nothing here — subprocess spawn is dwarfed by LLM
latency. Each call: open → send → await outcome → close.

---

## 3. Components

### 3.1 `triage::dispatch_triage`

```rust
// crates/surge-orchestrator/src/triage.rs

pub async fn dispatch_triage(
    bridge: Arc<dyn BridgeFacade>,
    input: TriageInput,
    opts: TriageOptions,
) -> Result<TriageDecision, TriageError>;

pub struct TriageOptions {
    /// Resolved Claude (or other ACP-agent) binary path. If absent,
    /// the dispatcher falls back to `AgentKind::Mock` with a warn.
    pub claude_binary: Option<PathBuf>,
    /// Per-attempt timeout. Spec §"Bootstrap stage failures" → 5 min.
    pub attempt_timeout: Duration,        // default: 5 min
    /// Maximum attempts before falling back to `Unclear`.
    pub max_attempts: u32,                // default: 3
    /// Root for per-call scratch directories.
    pub scratch_root: PathBuf,            // default: ~/.surge/intake/triage
    /// Whether to keep scratch on Unclear / failure for debugging.
    pub keep_scratch_on_failure: bool,    // default: true
}

#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    #[error("scratch dir setup failed: {0}")]
    Scratch(#[from] std::io::Error),
    #[error("acp bridge: {0}")]
    Bridge(String),
    #[error("artifact malformed: {path}: {error}")]
    Artifact { path: PathBuf, error: String },
}
```

**`Ok(TriageDecision)` is always returned even on retry exhaustion.**
Exhausted retries materialise as `TriageDecision::Unclear { question:
"Triage failed after N attempts: <last error>" }` so the daemon's
match branches stay total. `TriageError` is reserved for invariant
violations (cannot create scratch dir, bridge facade is dead) — these
shouldn't happen in practice and propagate as errors.

### 3.2 `triage::SUBMIT_TRIAGE_TOOL` (constant) — **dropped**

The brainstorming session considered a custom tool; the final design
uses standard `report_stage_outcome` + file artifacts only. No new
constants needed.

### 3.3 `surge_intake::candidates::build_for_task`

```rust
// crates/surge-intake/src/candidates.rs (existing module, new fn)

pub async fn build_for_task(
    source: &Arc<dyn TaskSource>,
    target: &TaskDetails,
    limit: usize,
) -> Result<Vec<TaskSummary>, Error> {
    let open = source.list_open_tasks().await?;
    let inputs: Vec<CandidateInput> = open
        .iter()
        .map(CandidateInput::from_summary)
        .collect();
    let scored = top_by_keyword_overlap(target, &inputs, limit);
    Ok(scored
        .into_iter()
        .filter_map(|s| open.iter().find(|t| t.task_id.as_str() == s.task_id).cloned())
        .collect())
}
```

Bridges between the existing `top_by_keyword_overlap` (which works
on `CandidateInput`) and the daemon's needed `Vec<TaskSummary>` for
`TriageInput`.

### 3.4 `Storage::snapshot_active_runs` and the type-location wrinkle

`ActiveRunSummary` is defined in `surge-orchestrator/src/triage.rs`,
but `Storage` lives in `surge-persistence`. A direct
`Storage::snapshot_active_runs() -> Vec<ActiveRunSummary>` would
create a circular dependency (orchestrator already depends on
persistence). **Resolution: Storage exposes its own row type;
the dispatcher does the mapping.**

```rust
// crates/surge-persistence/src/runs/active.rs (new fn or method)

pub struct ActiveRunRow {
    pub run_id: String,
    pub task_id: Option<String>,
    pub status: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl Storage {
    pub async fn snapshot_active_runs(&self, limit: usize) -> Result<Vec<ActiveRunRow>, Error>;
}
```

```rust
// crates/surge-orchestrator/src/triage.rs (mapping helper)

impl ActiveRunSummary {
    pub(crate) fn from_row(row: surge_persistence::runs::ActiveRunRow) -> Self {
        Self {
            run_id: row.run_id,
            task_id: row.task_id,
            status: row.status,
            started_at: row.started_at.to_rfc3339(),
        }
    }
}
```

Implementation: `SELECT id, task_id, status, started_at FROM runs
WHERE status IN ('Bootstrapping', 'Running') ORDER BY started_at DESC
LIMIT ?`. The `task_id` column is sourced from `ticket_index` joined
on `run_id` (or directly stored on `runs` if that's where the schema
already keeps it — confirm during T2 implementation).
Bounded by `LIMIT 32` — Triage doesn't need exhaustive enumeration
of every active run, just a representative sample for dedup hints.

If a similar accessor already exists in `surge-persistence`, it is
reused; if not, the new method ships alongside this change
(~30 LOC plus one in-memory unit test).

### 3.5 Daemon mapping

The existing arm at `crates/surge-daemon/src/main.rs:311` is
replaced. The new shape:

```rust
RouterOutput::Triage { event } => {
    let Some(source) = source_map_for_consumer.get(&event.source_id) else {
        warn!(source_id = %event.source_id, "no source registered; dropping triage");
        continue;
    };

    // Fetch full task details (raw_payload alone is insufficient for Triage).
    let task_details = match source.fetch_task(&event.task_id).await {
        Ok(td) => td,
        Err(e) => {
            warn!(error = %e, task_id = %event.task_id, "fetch_task failed; surfacing fallback inbox");
            // Fall through to a Medium-priority placeholder InboxCard
            // (preserves Plan-C MVP behaviour for unrecoverable provider errors).
            deliver_fallback_inbox(&notifier, &event).await;
            continue;
        }
    };

    let candidates = surge_intake::candidates::build_for_task(source, &task_details, 15)
        .await
        .unwrap_or_default();
    let active_runs = storage.snapshot_active_runs().await.unwrap_or_default();
    let input = TriageInput { task: task_details.clone(), candidates, active_runs };

    let opts = TriageOptions {
        claude_binary: surge_acp::discovery::AgentDiscovery::find_claude().ok(),
        attempt_timeout: Duration::from_secs(300),
        max_attempts: 3,
        scratch_root: surge_runs_dir().join("intake").join("triage"),
        keep_scratch_on_failure: true,
    };

    match surge_orchestrator::triage::dispatch_triage(bridge.clone(), input, opts).await {
        Err(e) => {
            warn!(error = %e, task_id = %event.task_id, "triage invariant failure; falling back");
            deliver_fallback_inbox(&notifier, &event).await;
        }
        Ok(TriageDecision::Enqueued { priority, summary, .. }) => {
            // Build InboxCardPayload using LLM-derived priority + summary
            // (replaces existing Priority::Medium placeholder).
        }
        Ok(TriageDecision::Duplicate { of, reasoning }) => {
            let body = format!("Surge: detected duplicate of {of}. {reasoning}");
            if let Err(e) = source.post_comment(&event.task_id, &body).await {
                warn!(error = %e, "duplicate comment post failed");
            }
            // No InboxCard.
        }
        Ok(TriageDecision::OutOfScope { reasoning }) => {
            let body = format!("Surge: out of scope. {reasoning}");
            if let Err(e) = source.post_comment(&event.task_id, &body).await {
                warn!(error = %e, "out_of_scope comment post failed");
            }
            // No InboxCard.
        }
        Ok(TriageDecision::Unclear { question }) => {
            let rendered = surge_notify::RenderedNotification {
                severity: surge_core::notify_config::NotifySeverity::Warn,
                title: format!("Triage unclear · {}", event.task_id.as_str()),
                body: question,
                artifact_paths: vec![],
            };
            // Deliver via Desktop channel (mirror existing InboxCard wiring).
        }
    }
}
```

`deliver_fallback_inbox` extracts the existing placeholder logic
(MVP `Priority::Medium` payload) into a helper so two error paths
can call it without duplication.

---

## 4. Data flow

```
                                    ┌──────────────────┐
                                    │ RouterOutput::    │
                                    │  Triage { event } │
                                    └─────────┬────────┘
                                              │
        ┌─────────────────────────────────────┴──────────────────────────────────┐
        │                                                                         │
        │ 1. Daemon: source.fetch_task(event.task_id) → TaskDetails              │
        │ 2. Daemon: candidates::build_for_task(source, &task_details, 15)       │
        │ 3. Daemon: storage.snapshot_active_runs() → Vec<ActiveRunSummary>      │
        │ 4. Daemon: TriageInput { task, candidates, active_runs }                │
        │                                                                         │
        └─────────────────────────────────────┬──────────────────────────────────┘
                                              │
                                              ▼
                              ┌─────────────────────────────┐
                              │  dispatch_triage(...)       │
                              │                             │
                              │  per attempt (1..=3):       │
                              │  ┌────────────────────────┐ │
                              │  │ mkdir scratch_dir      │ │
                              │  │ open ACP session       │ │
                              │  │   working_dir=scratch  │ │
                              │  │   sandbox=ReadOnly     │ │
                              │  │ send_message(prompt    │ │
                              │  │   + JSON(input))       │ │
                              │  │ tokio::time::timeout(  │ │
                              │  │   5min, event_loop)    │ │
                              │  │   ↓                    │ │
                              │  │ OutcomeReported →      │ │
                              │  │   close_session →      │ │
                              │  │   read decision.json → │ │
                              │  │   read summary.md →    │ │
                              │  │   into_decision()      │ │
                              │  └────────────────────────┘ │
                              │  retry on:                  │
                              │   - timeout                 │
                              │   - AgentCrashed            │
                              │   - bad json                │
                              │  exhaust → Unclear fallback │
                              └────────────┬────────────────┘
                                           │
                                           ▼
                       ┌──────────────────────────────────────┐
                       │ TriageDecision (always Ok)            │
                       └────────────┬──────────────────────────┘
                                    │
        ┌───────────────────────────┼──────────────────────────────┐
        │                           │                              │
        ▼                           ▼                              ▼
  Enqueued                    Duplicate / OOS              Unclear
  → InboxCardPayload          → source.post_comment()      → Warn-severity
    with real priority,       → no InboxCard                 RenderedNotification
    summary from              → ticket_index → Skipped       via Desktop
    inbox_summary.md
```

### 4.1 Initial message contents

The system prompt is the profile's `[prompt].system` section,
followed by a `# Inputs` block containing the JSON-encoded
`TriageInput`. The agent has read-only project access (sandbox
mode) but the `working_dir` it sees is the scratch directory, not
the project root, so practically it works from supplied data
only — exactly what the spec intends.

The full message body has the following shape (rendered via
`format!`, no template engine):

~~~text
<system prompt from profile>

# Inputs

The triage input is encoded as JSON. The shape is:
- task: TaskDetails  (the new ticket: title, description, labels, url, ...)
- candidates: TaskSummary[]  (top-15 similar open tickets + recent specs)
- active_runs: ActiveRunSummary[]  (currently active Surge runs)

The literal JSON follows:

<serialized JSON of TriageInput>

# Task

Decide whether this ticket is a duplicate, out-of-scope, unclear,
or should be enqueued. Then, in your working directory:

1. Write your structured decision to `triage_decision.json`
   (schema below).
2. Write a 3-5 line markdown blurb to `inbox_summary.md`
   (used as the body of the inbox card on `enqueued`; safe to
   omit otherwise).
3. Call `report_stage_outcome` with the matching `outcome` and
   `artifacts_produced = ["triage_decision.json", "inbox_summary.md"]`.

triage_decision.json schema:
- decision: "enqueued" | "duplicate" | "out_of_scope" | "unclear"
- duplicate_of: string (task id, e.g. "github_issues:foo/bar#42")
                or null
- priority: "urgent" | "high" | "medium" | "low"
- priority_reasoning: one sentence explaining priority
- summary: one sentence high-level description of the task
- question: string (only when decision = "unclear")
~~~

The schema is described in plain prose rather than as nested
fenced code blocks for two reasons: (1) markdown rendering of
nested triple-backticks is unreliable across viewers; (2)
LLMs handle prose schemas as well as fenced JSON in this
context, and we already have `serde_json::from_slice` enforcing
the actual structure.

### 4.2 SessionConfig

When `opts.claude_binary` is `None` the dispatcher short-circuits
before reaching this point (see §1.1 — returns `Unclear` with a
configuration-hint message). The session is only built when a real
agent binary is resolved:

```rust
SessionConfig {
    agent_kind: AgentKind::ClaudeCode {
        binary: opts.claude_binary.clone().expect("checked above"),
        extra_args: vec![],
    },
    working_dir: scratch_dir.clone(),
    system_prompt: prompt_text,
    declared_outcomes: vec![
        OutcomeKey::try_from("enqueued")?,
        OutcomeKey::try_from("duplicate")?,
        OutcomeKey::try_from("out_of_scope")?,
        OutcomeKey::try_from("unclear")?,
    ],
    allows_escalation: false,
    tools: vec![],                        // ACP-native fs tools are enough
    sandbox: delegated_sandbox(),         // see note below
    permission_policy: PermissionPolicy::default(),
    bindings: BTreeMap::from([
        ("intake.task_id".into(), task_id_str),
        ("intake.attempt".into(), attempt.to_string()),
    ]),
}
```

For unit tests, the test harness constructs `SessionConfig` with
`AgentKind::Mock { args: ... }` directly — Mock is a test
mechanism, not a production fallback path.

**Sandbox-delegated convention.** `delegated_sandbox()` is a helper
that returns `Box::new(AlwaysAllowSandbox)`. The bridge-level
`Sandbox` trait field is required by `SessionConfig`'s shape but
the no-op `AlwaysAllowSandbox` implementation means **the bridge
applies no filtering**: tool-list and per-call decisions pass
through to the agent. This matches `docs/revision/VISION-2026.md`
§"Sandbox-delegated":

> Each agent has its own native sandbox (Codex CLI sandbox modes,
> Claude Code Skills isolation, etc.). Surge configures it via ACP,
> the agent enforces it.

The profile's `[sandbox] mode = "read-only"` field is a **semantic
marker** read by the agent itself (or by a future Layer 2 profile
loader that translates it to ACP-level capability flags). Surge's
bridge does not enforce it — the agent does. Globalising this
convention (e.g., a `delegate_sandbox_to_agent: bool` toggle in
`SurgeConfig` and removing `DenyListSandbox` surface) is the
subject of the RFC-0006 refactor (Tier 3+4 deprecation) called out
in Vision-2026; it is out of scope here.

`bindings` carry the task id and attempt number for correlation in
`BridgeEvent::SessionEstablished` — useful for the future tracing
work that will surface intake telemetry.

### 4.3 Event loop

Mirrors `execute_agent_stage` but stripped to just outcome / crash
/ timeout handling. No tool dispatch (the agent only uses ACP-native
filesystem tools served by the bridge's built-in `Client` impl). No
human-input handling (Triage profile sets `allows_escalation =
false`).

Event filter: only events for our `session_id`. Discarded:
`AgentMessage`, `TokenUsage`, `ToolCall` with `meta.injected==true`,
`ToolResult`, `Error` (logged at warn).

Terminating events:
- `OutcomeReported { outcome, .. }` → break and proceed to artifact
  read.
- `SessionEnded { reason: AgentCrashed { exit_code, stderr_tail } }`
  → bail with retry-eligible error.

---

## 5. Error handling and retry

### 5.1 Per-attempt failure modes

| Failure | Retry decision | Feedback to next attempt |
|---|---|---|
| `tokio::time::timeout` exceeded (5 min) | retry up to `max_attempts` | none — fresh attempt |
| `BridgeEvent::SessionEnded { reason: AgentCrashed }` | retry up to `max_attempts` | none — fresh attempt |
| `triage_decision.json` missing after `OutcomeReported` | retry up to `max_attempts` | feedback message: "previous attempt did not produce triage_decision.json; you must write the file before calling report_stage_outcome" |
| `triage_decision.json` exists but `serde_json::from_slice` fails | retry up to `max_attempts` | feedback message: "previous triage_decision.json was malformed: \<error\>" |
| `TriageJson::into_decision` returns `Err` (e.g. unknown priority) | retry up to `max_attempts` | feedback message: "previous decision was rejected: \<error\>" |
| `BridgeError::OpenSessionError` / `SendMessageError` | bail as `TriageError::Bridge` (no retry) | n/a |

Retry feedback is delivered as a follow-up `MessageContent::Text` on
the same session if the session is still alive, otherwise on a fresh
session at the start of the next attempt.

### 5.2 Final fallback

After `max_attempts` exhaustion the dispatcher returns:

```rust
Ok(TriageDecision::Unclear {
    question: format!(
        "Triage failed after {} attempts: {}",
        opts.max_attempts, last_error
    ),
})
```

Daemon surfaces this as a Warn-severity desktop notification. The
`ticket_index` row stays in `Triaging` state — the ticket is not
acted upon until a human resolves it (Layer 2 will introduce a
proper inbox-cycle for Unclear).

### 5.3 Scratch dir lifecycle

- Created at the start of each attempt (top-level call): `mkdir -p
  ~/.surge/intake/triage/{ulid}/`. Each retry within a single call
  reuses the same directory (so prior decision.json drafts can be
  overwritten by the agent).
- On success (Enqueued / Duplicate / OutOfScope): scratch dir is
  removed (`std::fs::remove_dir_all`). Best-effort — error is
  warn-logged and not propagated.
- On Unclear or `TriageError`: scratch dir is **kept** if
  `keep_scratch_on_failure` is true (default). This preserves
  artifacts for post-mortem.

### 5.4 What does *not* fall under retry

- Daemon-side errors before invoking the dispatcher
  (`source.fetch_task`, `candidates::build_for_task`,
  `storage.snapshot_active_runs`) — daemon delivers a fallback
  Medium-priority InboxCard and logs at warn. This preserves
  pre-existing Plan-C MVP behaviour for unrecoverable provider
  errors.
- `TriageError::Bridge` for facade-level dead bridge — propagates
  as Err to the daemon, which logs and falls back to InboxCard
  placeholder.

---

## 6. Testing strategy

### 6.1 Unit tests (mock BridgeFacade)

`crates/surge-orchestrator/tests/triage_dispatch.rs` — six
scenarios using a mock `BridgeFacade`. Mock pattern: existing test
double already used by `agent.rs` integration tests.

| Test | Mock behaviour | Expected result |
|---|---|---|
| `enqueued_happy_path` | Mock writes `triage_decision.json` with `decision="enqueued", priority="high"`, fires `OutcomeReported` | `Ok(Enqueued { priority: High, .. })` |
| `duplicate_happy_path` | Mock writes `decision="duplicate", duplicate_of="...#42"` | `Ok(Duplicate { of: "...#42", .. })` |
| `out_of_scope_happy_path` | Mock writes `decision="out_of_scope", priority="low"` | `Ok(OutOfScope { .. })` |
| `unclear_happy_path` | Mock writes `decision="unclear", question="..."` | `Ok(Unclear { question })` |
| `bad_json_then_recovers` | Attempt 1: malformed JSON. Attempt 2: valid JSON. | `Ok(Enqueued { .. })` after retry |
| `exhaust_retries_yields_unclear` | All three attempts produce malformed JSON | `Ok(Unclear { question: contains "Triage failed after 3 attempts" })` |

### 6.2 Snapshot test of the rendered initial message

`crates/surge-orchestrator/src/triage.rs` — `#[cfg(test)] mod
prompt_render_tests`. Asserts the constructed prompt is byte-stable
given a fixed `TriageInput` so prompt regressions are caught at PR
review time. Using `insta` to match repo convention.

### 6.3 Feature-gated end-to-end LLM test

`crates/surge-orchestrator/tests/triage_llm.rs` — under feature
flag `_bootstrap_llm_test`. Reuses the existing fixtures
(`enqueue_001.toml`, `duplicate_001.toml`, `out_of_scope_001.toml`)
to produce three real ACP calls against Claude Haiku at
`temperature=0`. Asserts:

- decision string matches exactly
- priority is within ±1 step of the fixture's expectation
- on `duplicate`, `duplicate_of` matches exactly

Requires `ANTHROPIC_TEST_KEY` env. Default workspace test does not
activate this feature, so CI cost stays unchanged.

### 6.4 Daemon-side smoke

`crates/surge-daemon/tests/triage_wiring.rs` — compile-and-init
test. Constructs the daemon's task-source consumer (with a mock
`BridgeFacade` that returns Enqueued), drives one `TaskEvent`
through the pipeline, asserts that the resulting `InboxCardPayload`
carries a non-`Medium` priority. Validates the wiring without an
LLM round-trip.

### 6.5 Coverage targets

Per RFC-0010 §"Coverage targets" the Triage Author rendering and
parser must hit ≥85%. The tests above plus the existing parser
tests in `triage.rs` cover ≥90% of the new code paths.

---

## 7. Forward-compatibility (Layer 2)

When Triage Author is promoted to a real graph node, the migration
proceeds as follows. **No types in this design change.**

### 7.1 RunState extension

Add `RunState::IntakeTriage { task_id, scratch_dir, attempt }`
ahead of `Bootstrapping`. The engine's main loop dispatches this
state via a new `advance_intake_triage` method that wraps the same
inner logic as `dispatch_triage`. Crash recovery checks for an
existing `triage_decision.json` in `scratch_dir` and short-circuits
to outcome routing if present — directly satisfying RFC-0010
acceptance criterion #8.

### 7.2 Event log

New `EventPayload` variants (already drafted in the Plan-C plan
under Task 10.1):

- `TicketDetected { task_id, source_id, provider }` — emitted by
  the daemon when `RouterOutput::Triage` arrives.
- `Tier1DedupDecided { decision, duplicate_run_id }` — already
  implicit in Tier-1; now externalised.
- `TriageDecided { decision, priority, duplicate_of, reasoning }`
  — emitted on `OutcomeReported`.

### 7.3 dispatch_triage role in Layer 2

Two options, picked at Layer-2 design time:

- **Option α — keep as test wrapper.** Engine integration calls
  the same logic via the standard agent stage path; `dispatch_triage`
  is moved into `#[cfg(test)]` for unit-testing the input/output
  shape without spinning up the engine.
- **Option β — extract as library.** Engine's stage handler for
  `_bootstrap/triage-author` profile delegates to `dispatch_triage`
  internally; `dispatch_triage` continues to be public.

Either way, the Layer 1 implementation does not need changes to
support the migration. This is the test of "did we pick the right
surface".

### 7.4 Profile evolution

The current `triage-author-1.0.toml` is minimal (id, version,
declared_outcomes, prompt, sandbox). When Description Author's
richer schema (`[role]`, `[runtime]`, `[approvals]`, `[hooks]`,
`[bindings]`) is generalised across all bootstrap profiles, Triage
Author's profile gets an upgrade in the same change. Layer 1 reads
the minimal version; Layer 2 reads whatever the registry returns —
both work with the same `BOOTSTRAP_TRIAGE_AUTHOR_TOML` constant
because field defaults are permissive.

---

## 8. Implementation order

Suggested TDD task breakdown (writing-plans skill will produce the
final plan):

1. **T1.** `surge-intake::candidates::build_for_task` — small helper
   with three unit tests against `MockTaskSource`.
2. **T2.** `Storage::snapshot_active_runs` — accessor + one unit
   test (in-memory DB, two active runs and one terminal).
3. **T3.** `triage::TriageOptions`, `triage::TriageError` types —
   compilation only.
4. **T4.** `dispatch_triage` happy path: Enqueued via mock bridge
   + scratch dir read. One test.
5. **T5.** `dispatch_triage` for Duplicate / OutOfScope / Unclear
   variants. Three more tests.
6. **T6.** Retry mechanics: bad JSON → retry → success; three
   bad-JSON-attempts → Unclear fallback. Two tests.
7. **T7.** Timeout and AgentCrashed retry handling. Two tests.
8. **T8.** Initial-message snapshot test.
9. **T9.** Daemon wiring replacement: extract `deliver_fallback_inbox`
   helper, route four decision arms.
10. **T10.** Daemon smoke test (`triage_wiring.rs`).
11. **T11.** Feature-gated LLM E2E test (`triage_llm.rs` body
    behind `_bootstrap_llm_test`).
12. **T12.** Roadmap update — strike "Triage Author LLM dispatch via
    ACP" from `docs/03-ROADMAP.md` Plan-C-polish remaining list.

Estimated effort calibration: 1.5 sessions for T1–T11 (Plan-C-polish
items #2–#5 took ~0.5 session each; this is 4× larger so 1.5–2× the
unit). A second pass for clippy / fmt / final QA.

---

## 9. Open questions / explicit deferrals

- **Q1.** Should the dispatcher emit any tracing events to be
  surfaced via `surge engine watch` once Triage is event-sourced?
  **Defer to Layer 2.** Layer 1 uses `tracing::info!` /
  `tracing::warn!` only — fine for daemon log inspection.
- **Q2.** Should Unclear notifications be deduped if the same
  ticket triages-as-Unclear multiple times within a polling
  window? **Defer.** Tier-1 dedup already prevents the LLM call
  on a ticket already marked `Triaging`; Layer 2's FSM transition
  rules will resolve any residual flapping.
- **Q3.** What happens on `TriageDecision::Duplicate { of:
  task_id }` when `of` does not exist in the source's active
  set? Layer 1 still posts the comment using the agent's claim
  (the agent has been told not to invent IDs in the system
  prompt). Layer 2 will validate against `ticket_index` and
  reject malformed `duplicate_of` claims at engine-event level.
- **Q4.** Sandbox tier-3 (network) for triage in case the agent
  needs to follow a link in the ticket. **Out of scope, defer.**
  The profile stays read-only; agents that need web context will
  ask via clarifying question in Layer 2.

---

## 10. Acceptance

This spec is implemented when:

1. `cargo build --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. All existing tests pass.
4. New unit tests in §6.1 pass.
5. New snapshot test in §6.2 passes (with committed snapshot).
6. New daemon smoke test in §6.4 passes.
7. `--features _bootstrap_llm_test` LLM E2E (§6.3) passes against
   real Claude Haiku for all three fixtures (gated behind
   `ANTHROPIC_TEST_KEY`; not enforced in CI).
8. `crates/surge-daemon/src/main.rs` no longer constructs
   `Priority::Medium` as a primary path on `RouterOutput::Triage`
   (it remains as a fallback only on provider-side errors).
9. RFC-0010 acceptance criterion #3 fully passes (priority is
   LLM-derived).
10. `docs/03-ROADMAP.md` Plan-C-polish list shows "Triage Author
    LLM dispatch via ACP" as completed; remaining items =
    explicitly Layer 2 / future RFC.
