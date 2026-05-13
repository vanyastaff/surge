# Implementation Plan: Tracker automation tiers

Branch: unruffled-bell-c7b522 (worktree; no new branch created — work on the existing isolated checkout; the plan file lives at `.ai-factory/plans/tracker-automation-tiers.md`)
Created: 2026-05-13
Refined: 2026-05-13 (RESCUE-mode planner pass — code-verified against the actual `surge-intake`, `surge-orchestrator`, `surge-daemon`, `surge-persistence` surfaces)

## Settings
- Testing: yes (unit per module; integration via `wiremock` mocking GitHub REST + Linear GraphQL, `MockBridge` for engine, `MockTaskSource` for router)
- Logging: verbose (DEBUG / INFO / WARN / ERROR — TRACE is not used in this codebase)
- Docs: yes (mandatory docs checkpoint at the end — `docs/tracker-automation.md` is new, `docs/cli.md` and `docs/workflow.md` updated, `docs/ARCHITECTURE.md` § "Tracker is master" expanded)

## Roadmap Linkage
Milestone: `.ai-factory/ROADMAP.md` § **Tracker automation tiers** (lines 187–203).
Rationale: Tracker source skeletons (✓ 2026-05-06), Inbox + intake pipeline (✓ 2026-05-06), Bootstrap & adaptive flow generation (✓ 2026-05-09) are all complete. Verified end-to-end:
- `surge-intake::TaskRouter` already multiplexes GitHub Issues + Linear streams and applies Tier-1 dedup (`crates/surge-intake/src/router.rs:55-103`).
- `handle_triage_event` already runs the LLM triage author against the canonical task details (`crates/surge-daemon/src/main.rs:387-475`) and produces a `TriageDecision::Enqueued { priority, summary, reasoning }`.
- `InboxActionConsumer::handle_start` already fetches the ticket, assembles a `BootstrapPrompt { title, description, tracker_url, priority, labels }`, calls `BootstrapGraphBuilder::build(run_id, prompt, worktree)` and `engine.start_run(...)` (`crates/surge-daemon/src/inbox/consumer.rs:90-217`). This is the **canonical** start path — 90% of the pipeline is already wired.
- `intake_completion::spawn` already subscribes to `GlobalDaemonEvent::RunFinished`, looks up the ticket via `IntakeRepo::lookup_ticket_by_run_id`, posts a completion comment to the tracker via `TaskSource::post_comment`, and transitions the FSM (`crates/surge-daemon/src/intake_completion.rs:1-171`).

What this milestone wires:
1. A **single `AutomationPolicy` decision step** placed AFTER triage and BEFORE `handle_start` so the same downstream code paths serve L1/L2/L3 — only the prelude differs (bootstrap vs template vs auto-merge follow-up).
2. L2 (`surge:template/<name>`) bypasses bootstrap by routing through `ArchetypeRegistry::resolve` (already present at `crates/surge-orchestrator/src/archetype_registry.rs`) instead of `BootstrapGraphBuilder::build`.
3. L3 (`surge:auto`) reuses the L1 happy path through bootstrap + run, then on `RunFinished { outcome: Completed }` invokes a new merge gate (`AutomationMergeGate`) that checks `all-checks-green AND review-approved` before posting a merge action to the tracker.
4. Tier-aware poll-cadence (L1 5min, L2 2min, L3 1min, +exponential backoff on `Error::RateLimited`) — implemented as a small `CadenceController` wrapping the existing source-poll loop without modifying `TaskSource` itself.
5. `surge intake list` — new CLI that queries `ticket_index` joined against `runs` to render a per-tracker pending/running/completed view.
6. Idempotency dedup key `(tracker_id, ticket_id, event_kind, run_id)` enforced via a new `intake_emit_log` table so retries never duplicate tracker comments.
7. Ticket-as-master integrity: `TaskEventKind::StatusChanged` / `TaskClosed` reflected in the existing `ticket_index` FSM (extend the existing state set; no new states).
8. Telegram cockpit (separate milestone, already planned) consumes the existing `BootstrapApprovalRequested` events emitted by L1 runs — this plan does NOT touch cockpit code; we verify the contract is intact.

Out of scope (defer with rationale in § Out of Scope): webhook intake (polling only), Tier-1 dedup by ticket *content* (semantic dedup — only ID-based is wired), per-tenant rate-limit policies beyond global retry-after, `surge:disabled` label-removal handling on already-active runs, Discord/Jira adapters.

## Architectural Decisions Locked Before Implementation

These are decisions taken at planning time so /aif-implement does not re-litigate them per task. Verified against actual code at planning time.

1. **`AutomationPolicy` lives in `surge-intake`, not `surge-core`.** Decision is purely about tracker-label semantics — it never reaches the engine or affects the run state machine. `surge-core` has zero knowledge of tracker labels (verified: `crates/surge-core/src/lib.rs` exports `SurgeConfig`, `AgentConfig`, `Graph` types, none reference tracker labels; the only "label" concept in core is `surge_core::edge::EdgePolicy`). Adding policy to core would invert the dependency: core does not depend on intake. New module `crates/surge-intake/src/policy.rs` defines:

```rust
pub enum AutomationPolicy {
    Disabled,                              // L0
    Standard,                              // L1: bootstrap + approval
    Template { name: String },             // L2: skip bootstrap
    Auto { merge_when_clean: bool },       // L3: full automation
}
```

Plus `pub fn resolve_policy(labels: &[String]) -> AutomationPolicy` with explicit precedence: `surge:disabled` wins, then `surge:auto`, then `surge:template/<name>`, then `surge:enabled` defaults to Standard, otherwise `Disabled` (label-absent = L0 per ROADMAP line 188). The precedence rules become a single `match` table tested via proptest on label sets.

2. **Policy decision happens in `handle_triage_event` AFTER `dispatch_triage` returns `Enqueued`, BEFORE the inbox card is enqueued.** Triage Author still decides *whether* (Enqueued/Duplicate/OOS/Unclear); policy decides *how* (L1/L2/L3). For L0 the policy check runs even earlier — directly on the raw `TaskDetails.labels` to short-circuit before paying the LLM cost. Insertion points:
   - L0 short-circuit: in `handle_triage_event` between `source.fetch_task` (`main.rs:408`) and `build_for_task` (`main.rs:418`) — if labels resolve to `Disabled`, log INFO `target: "intake::policy"` and return without dispatching triage. Saves the LLM call entirely.
   - L1/L2/L3 split: in `dispatch_triage_decision` (`main.rs:479+`) inside the `Enqueued` arm. Existing path produces an inbox card for human approval (L1 today). New code branches on `AutomationPolicy`:
     - `Standard` produces the existing inbox card path (no behaviour change).
     - `Template { name }` enqueues a synthesized `InboxActionRow { kind: Start, .. }` directly (skipping inbox-card-and-approval), with a marker field `policy_hint: Option<String>` carrying the template name (Decision 4).
     - `Auto { .. }` is identical to Standard for the bootstrap leg (operator still gets a card for visibility) but every bootstrap HumanGate auto-approves (Decision 13).

3. **`InboxActionConsumer::handle_start` factored into a `launch_ticket_run` helper.** Today the function is 128 lines (`consumer.rs:90-217`). Extract two helpers into a new module `crates/surge-daemon/src/inbox/ticket_run_launcher.rs`:
   - `pub async fn fetch_ticket_for_start(repo, sources, callback_token)` returns `TicketStart { ticket_row, source, details, task_id }`.
   - `pub async fn launch_ticket_run(start, opts, engine, builder, archetypes, repo, ...)` does the worktree+graph+start_run+state transition.
   - `LaunchOpts` carries the `AutomationPolicy` and `policy_hint` so the L2 path branches inside the same launcher and the L3 path can set `auto_approve_bootstrap` and the merge flag on `EngineRunConfig`.
   - The CLI bootstrap entrypoint (`crates/surge-cli/src/commands/bootstrap.rs:122-147`) and the L2 path both call `launch_ticket_run` to share the **one** spot that knows how to seed `project_context` + `bootstrap_parent`. This is the explicit answer to the brief "factor out the BootstrapGraphBuilder::build logic into a shared helper" requirement.

4. **`InboxActionRow.policy_hint: Option<String>` is a new column on the existing `inbox_action_queue` table.** Migration `0007_inbox_action_policy_hint.sql` adds the column with `DEFAULT NULL`. The L2 path writes the template name; the L1/L3 paths leave it NULL. `InboxActionConsumer::handle_start` reads it on dispatch — if `Some(name)`, the launcher calls `ArchetypeRegistry::resolve(name)` (`archetype_registry.rs:46-67`) instead of `BootstrapGraphBuilder::build`. Unknown name degrades to Standard with a WARN log (Decision 14).

5. **Idempotency dedup key = `(source_id, task_id, event_kind, run_id)`.** New table `intake_emit_log` records every outbound side-effect (tracker comment, label change) keyed by this quad. `event_kind` is one of: `triage_decision`, `run_started`, `run_completed`, `run_failed`, `run_aborted`, `merge_proposed`, `merge_blocked`. Insert is `INSERT OR IGNORE`; the existing emit code wraps its call with a precheck via `intake_emit_log.has(...)` and skips the side-effect if the row already exists. This is in addition to the per-source idempotency the comment-poster already performs (GitHub exact-body match at `crates/surge-intake/src/github/source.rs:365-391`, Linear idempotency keys).

6. **No new `TicketState` variants.** The existing FSM (`crates/surge-persistence/src/intake.rs:39-73`) already covers every state we need. The gap for L0 short-circuit is filled by re-using `Skipped` plus a `triage_decision = "L0Skipped"` discriminator on the row. Add `pub const TRIAGE_DECISION_L0: &str = "L0Skipped"` to `surge_intake::policy` so the discriminator is stable. The same row makes `surge intake list` render this ticket as "L0 (disabled)" without storing extra state.

7. **External-state-change reflection (acceptance #11) uses `TaskEventKind::StatusChanged` and `TaskClosed`.** Both variants already exist in `TaskEvent` (`crates/surge-intake/src/types.rs:201-214`). The router today (`router.rs:63-100`) only forwards `NewTask` events into Triage. Extend the router to route `StatusChanged` / `TaskClosed` / `LabelsChanged` events into a new path that updates `ticket_index.state` directly (no LLM):
   - `TaskClosed` or `StatusChanged { to: "closed" }`: if `ticket_index.state == Active`, transition to `Aborted` with reason `"closed externally"` and cancel the run via `EngineFacade::abort_run(run_id)` (Decision 8). Already-terminal states are no-op. `InboxNotified` clears callback token + transitions to `Skipped` with `triage_decision = "ExternallyClosed"`.
   - `LabelsChanged { added, removed }`: `surge:disabled` added mid-run triggers graceful-cancel via `abort_run`; `surge:auto` or `surge:template/*` added to an in-flight Standard run is INFO-logged but does not escalate (operator must restart). Documented in ADR 0014 (Task 2).

## Out of Scope

- Webhook intake (polling only).
- Tier-1 dedup by ticket *content* (semantic dedup — only ID-based is wired).
- Per-tenant rate-limit policies beyond global retry-after.
- `surge:disabled` label-removal handling on already-active runs.
- Discord / Jira adapters.

## Implementation Tasks

Derived from the Decisions section (1–7) and ROADMAP acceptance criteria (lines 187–203). Each task is atomic, testable, and has explicit verification.

- [x] **Task 1 — `AutomationPolicy` module in `surge-intake`** (Decisions 1, 6)
  - New file `crates/surge-intake/src/policy.rs` with the `AutomationPolicy` enum, `resolve_policy(labels: &[String]) -> AutomationPolicy` with explicit precedence (`surge:disabled` > `surge:auto` > `surge:template/<name>` > `surge:enabled` > absent ⇒ `Disabled`), and `pub const TRIAGE_DECISION_L0: &str = "L0Skipped"`.
  - Wire `pub mod policy;` + public re-exports through `surge-intake::lib`.
  - Unit tests per tier; proptest over arbitrary label vectors verifying precedence is total and deterministic.
  - Verbose tracing at `target: "intake::policy"` (DEBUG on input labels, INFO on resolved tier).
  - Verification: `cargo build -p surge-intake && cargo test -p surge-intake policy && cargo clippy -p surge-intake -- -D warnings`.

- [x] **Task 2 — Persistence migrations: `policy_hint` column + `intake_emit_log` table** (Decisions 4, 5)
  - Migration `registry/<next>_inbox_action_policy_hint.sql`: `ALTER TABLE inbox_action_queue ADD COLUMN policy_hint TEXT DEFAULT NULL`.
  - Migration `registry/<next>_intake_emit_log.sql`: `CREATE TABLE intake_emit_log (source_id TEXT NOT NULL, task_id TEXT NOT NULL, event_kind TEXT NOT NULL, run_id TEXT NOT NULL, recorded_at_ms INTEGER NOT NULL, PRIMARY KEY (source_id, task_id, event_kind, run_id))`.
  - Extend `InboxActionRow` with `policy_hint: Option<String>` + repo read/write.
  - New `IntakeEmitLog` repo: `has(source_id, task_id, event_kind, run_id) -> bool` and `record(...)` with `INSERT OR IGNORE`. Event-kind enum: `triage_decision`, `run_started`, `run_completed`, `run_failed`, `run_aborted`, `merge_proposed`, `merge_blocked`.
  - Tests against in-memory SQLite: roundtrip + idempotency property.
  - Verification: `cargo test -p surge-persistence`.

- [x] **Task 3 — Extract `launch_ticket_run` helper** (Decision 3)
  - New module `crates/surge-daemon/src/inbox/ticket_run_launcher.rs` with:
    - `pub async fn fetch_ticket_for_start(repo, sources, callback_token) -> Result<TicketStart, ...>`.
    - `pub async fn launch_ticket_run(start, opts: LaunchOpts, engine, builder, archetypes, repo, ...) -> Result<RunHandle, ...>`.
    - `pub struct LaunchOpts { policy: AutomationPolicy, policy_hint: Option<String>, auto_approve_bootstrap: bool, merge_when_clean: bool }`.
  - Migrate `InboxActionConsumer::handle_start` (`consumer.rs:90-217`) and `crates/surge-cli/src/commands/bootstrap.rs:122-147` to call `launch_ticket_run`. Preserve current L1 behavior — no L2/L3 branching yet (lands in Task 4).
  - Tests: launcher unit tests with `MockBridge`; consumer + CLI integration regression.
  - Verification: `cargo test -p surge-daemon -p surge-cli`.

- [x] **Task 4 — Policy decision wiring: L0 / L1 / L2 / L3** (Decision 2)
  - L0 short-circuit in `handle_triage_event` (`main.rs:407`-ish, between `fetch_task` and `build_for_task`): if `resolve_policy(task_details.labels) == Disabled`, write `ticket_index` row with state `Skipped`, `triage_decision = "L0Skipped"`, record in `intake_emit_log`, return without dispatching triage.
  - L1 / L2 / L3 branching in `dispatch_triage_decision` `Enqueued` arm:
    - `Standard` ⇒ unchanged path (inbox card).
    - `Template { name }` ⇒ synthesize `InboxActionRow { kind: Start, policy_hint: Some(name), .. }` directly (skip inbox card and approval).
    - `Auto { merge_when_clean }` ⇒ identical to `Standard` for the bootstrap leg but tag the resulting run for HumanGate auto-approve + post-completion merge.
  - `InboxActionConsumer::handle_start` reads `policy_hint`; if `Some(name)`, the launcher calls `ArchetypeRegistry::resolve(name)` instead of `BootstrapGraphBuilder::build`. Unknown name ⇒ WARN, degrade to `Standard`.
  - Tests with `MockTaskSource` covering each tier and unknown-template fallback.

- [x] **Task 5 — External state-change reflection** (Decision 7)
  - Extend `TaskRouter` to forward `StatusChanged` / `TaskClosed` / `LabelsChanged` into a new `RouterOutput::ExternalUpdate { event }`.
  - Daemon handler:
    - `TaskClosed` or `StatusChanged { to: "closed" }`: if `state == Active`, `EngineFacade::abort_run(run_id)` + transition `Aborted` (reason `"closed externally"`); if `InboxNotified`, clear callback token + transition `Skipped` (`triage_decision = "ExternallyClosed"`); terminal states are no-op.
    - `LabelsChanged { added: surge:disabled, .. }` mid-run: graceful `abort_run`.
    - `LabelsChanged { added: surge:auto | surge:template/*, .. }` on Standard in-flight: INFO log only.
  - Tests: router routing + handler FSM transitions.

- [x] **Task 6 — `CadenceController` for tier-aware polling** (ROADMAP §198, milestone wires #4)
  - Wrap each source's poll loop in a `CadenceController` that picks the most aggressive tier among active tickets for that source (L1 = 5min, L2 = 2min, L3 = 1min) and applies exponential backoff with jitter on `Error::RateLimited`.
  - Tests: deterministic schedule under `tokio::time::pause`, backoff curve assertion, recovery after rate-limit.

- [x] **Task 7 — `AutomationMergeGate` for L3** (ROADMAP §203, milestone wires #3)
  - Consumer subscribing to `GlobalDaemonEvent::RunFinished`.
  - On `Completed`: lookup policy via `ticket_index`; if `Auto { merge_when_clean: true }` AND PR has all-checks-green AND review-approved, post merge action; otherwise post `merge-blocked` comment with reason.
  - Idempotency via `intake_emit_log` (`merge_proposed` / `merge_blocked`).

- [x] **Task 8 — `surge intake list` CLI** (ROADMAP §201, milestone wires #5)
  - New subcommand reading `ticket_index JOIN runs`, rendering `Tracker | Ticket | Tier | State | Run ID | Started`.
  - `--format json|table` (default table), optional `--tracker <id>`.
  - In-memory DB integration test.

- [x] **Task 9 — ADR 0013 + tracker-automation docs** (mandatory docs checkpoint; renumbered from the plan's placeholder "0014" — next available ADR slot was 0013)
  - `docs/adr/0014-tracker-automation-tiers.md`: tier semantics, label precedence, FSM additions, idempotency contract, interaction with bootstrap and templates.
  - `docs/tracker-automation.md` (new, user-facing): tier table, label conventions, examples, troubleshooting.
  - `docs/cli.md`: add `surge intake list` section.
  - `docs/workflow.md`: add tier flow diagram.
  - `docs/ARCHITECTURE.md`: expand § "Tracker is master" with policy precedence and external-state reflection.

## Commit Checkpoints

- After Task 1: `feat(surge-intake): AutomationPolicy resolver with tier precedence`.
- After Task 2: `feat(surge-persistence): policy_hint column + intake_emit_log table`.
- After Task 3: `refactor(surge-daemon): extract launch_ticket_run helper`.
- After Task 4: `feat(surge-daemon): wire AutomationPolicy into triage and inbox dispatch`.
- After Task 5: `feat(surge-intake): route external state changes into ticket_index FSM`.
- After Task 6: `feat(surge-intake): tier-aware polling cadence controller`.
- After Task 7: `feat(surge-daemon): AutomationMergeGate for L3 auto-merge`.
- After Task 8: `feat(surge-cli): surge intake list`.
- After Task 9: `docs(tracker): ADR 0014 + tracker-automation pages`.
