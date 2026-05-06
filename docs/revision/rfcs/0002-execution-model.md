# RFC-0002 · Execution Model

## Overview

The execution model is **event-sourced**. The state of any run at any moment is computed by folding over an append-only sequence of events. This is the architectural foundation that enables time-travel debugging, replay, fork-from-here, durable persistence across app restarts, and unambiguous audit trail.

This document specifies:
- Event types and their semantics
- The state machine that consumes events to produce run state
- Run lifecycle from start to terminal
- How concurrent operations are serialized
- Crash recovery

## Why event sourcing

Alternatives considered and rejected:

**Plain CRUD** (current_status column, updated in place). Cannot replay. Cannot fork. Cannot debug "why did the run go this way at 14:32" — that information is gone the moment status updates. Common choice in similar tools (surge Kanban, Conductor) — and they all suffer from this limitation.

**Snapshots + diffs**. Periodically dump full state to disk, store diffs between snapshots. Less storage than full event log but loses fine-grained history. Time-travel only works to snapshot boundaries.

**Event sourcing** wins because:
- Every meaningful state transition becomes a queryable record
- Replay is `events.fold(initial_state, apply)` — trivially correct
- Fork = "branch the event stream from event N, append new events"
- Crash recovery = "load events for run, fold to get current state"
- Implementation cost is roughly equal to CRUD if SQLite is used (one append-only table)
- Debugging is dramatically easier ("why is X like this?" → grep events)

The cost is storage growth (mitigated by archival policy) and slightly more complex queries (mitigated by materialized views).

## Event taxonomy

All events implement a base structure:

```rust
struct Event {
    run_id: RunId,
    seq: u64,                  // monotonically increasing within a run
    timestamp: DateTime<Utc>,
    payload: EventPayload,     // the typed body
}
```

`seq` is per-run and starts at 1. It is the canonical ordering. Wall clock time may have skew but `seq` is absolute.

### Event payload variants

#### Run lifecycle

- **`RunStarted`** — first event of any run. Carries pipeline ID, initial inputs, project context, run policy (agent sandbox intent, retry limits).
- **`RunCompleted`** — terminal event. Carries final state and which Terminal node was reached.
- **`RunFailed`** — terminal event for unrecoverable failures. Carries error chain.
- **`RunAborted`** — terminal event for user-initiated cancellation.

#### Bootstrap stages

- **`BootstrapStageStarted`** — entering Description / Roadmap / Flow stage.
- **`BootstrapArtifactProduced`** — agent emitted `description.md`, `roadmap.md`, or `flow.toml`. Carries content hash and storage path.
- **`BootstrapApprovalRequested`** — Telegram card sent, awaiting decision.
- **`BootstrapApprovalDecided`** — user tapped approve/edit/reject. Carries decision and free-text comment.

#### Pipeline construction

- **`PipelineMaterialized`** — Flow Generator's output `flow.toml` is parsed and validated, becomes the canonical graph for this run. After this event, the graph is frozen (immutable for remainder of run).

#### Stage execution

- **`StageEntered`** — pipeline reached a node. Carries node ID and attempt number (≥1).
- **`StageInputsResolved`** — all bindings (artifact references, prompt variables) have been computed and the agent is about to be invoked.
- **`SessionOpened`** — ACP session created for this stage attempt. Carries session ID, provider, launch mode, and applied sandbox mode.
- **`ToolCalled`** — agent invoked a tool. Carries tool name, arguments (redacted if sensitive), session ID.
- **`ToolResultReceived`** — tool returned. Carries success/failure and result hash.
- **`ArtifactProduced`** — stage created or modified an artifact (file, document). Carries path and content hash.
- **`OutcomeReported`** — agent called `report_stage_outcome` tool. Carries declared outcome ID and summary.
- **`StageCompleted`** — stage finished successfully. Carries the matched outcome.
- **`StageFailed`** — stage failed (timeout, hook rejection, agent error). Carries failure reason.
- **`SessionClosed`** — ACP session for this stage ended. Carries session ID and disposition.

#### Routing

- **`EdgeTraversed`** — engine resolved which next node to enter based on outcome. Carries edge ID, source/target node IDs.
- **`LoopIterationStarted`** — entered iteration N of a Loop body. Carries loop ID, item being processed, index.
- **`LoopIterationCompleted`** — iteration finished. Carries final outcome.
- **`LoopCompleted`** — loop exhausted iterations or hit exit condition.

#### Human interaction

- **`ApprovalRequested`** — HumanGate node activated. Carries gate node ID, channel (telegram/ui), payload sent.
- **`ApprovalDecided`** — user responded. Carries decision and channel.

#### Sandbox

- **`SandboxElevationRequested`** — provider or agent requested capability outside current sandbox intent. Carries requested capability.
- **`SandboxElevationDecided`** — user approved/denied (or remembered for template).

#### Hooks

- **`HookExecuted`** — engine ran a configured hook (PreToolUse, PostToolUse, OnOutcome, OnError). Carries hook ID and exit status.

#### Cost & telemetry

- **`TokensConsumed`** — incremental token usage event. Carries prompt/output tokens for a single agent turn.

#### Forking

- **`ForkCreated`** — new run was forked from this run at given seq. Carries the new run's ID. (The forked run gets its own event log starting from `RunStarted` that references this fork point.)

### Event design rules

1. Events are **append-only**. Never edit, never delete (archival deletes whole runs but never edits).
2. Events are **complete**. The `payload` must contain everything needed to recompute state — no hidden references to mutable external state.
3. Events are **deterministic**. Replaying the same event sequence on the same code must produce the same result. This means: no wall-clock dependencies in fold logic, no random IDs generated during fold (all IDs come from events), no environment lookups.
4. Large content is **hashed**, not embedded. Artifacts go to filesystem under `runs/<run_id>/artifacts/`, events carry content hash. This keeps event log compact.
5. Sensitive data is **redacted**. Tool arguments containing secrets get redacted in the event log (stored as `<REDACTED:tool_call_id>` with separate keychain).

## State machine

The engine is conceptually a single-threaded state machine per run. Concurrency between runs is supported (multiple runs in parallel) but within a single run, events are strictly ordered.

### State representation

```rust
enum RunState {
    NotStarted,
    Bootstrapping {
        stage: BootstrapStage,  // Description | Roadmap | Flow
        substate: BootstrapSubstate,
    },
    Pipeline {
        graph: FrozenGraph,
        cursor: Cursor,         // current node + attempt
        memory: RunMemory,      // accumulated artifacts, costs, etc.
    },
    Terminal {
        kind: TerminalKind,     // Completed | Failed | Aborted
        reason: String,
    },
}

enum BootstrapSubstate {
    AgentRunning { session: SessionId, started: Seq },
    AwaitingApproval { artifact: ArtifactRef, requested: Seq },
}
```

### Fold function

```rust
fn apply(state: RunState, event: &Event) -> RunState {
    match (state, &event.payload) {
        (NotStarted, RunStarted { .. }) => Bootstrapping { stage: Description, substate: ... },
        (Bootstrapping { stage, .. }, BootstrapApprovalDecided { decision: Approve, .. }) => 
            advance_bootstrap_stage(stage),
        (Bootstrapping { stage: Flow, .. }, PipelineMaterialized { graph }) => 
            Pipeline { graph, cursor: graph.start, memory: empty() },
        (Pipeline { graph, cursor, memory }, StageEntered { node }) => 
            Pipeline { graph, cursor: cursor.enter(node), memory },
        // ... etc
    }
}
```

The full match is exhaustive. Invalid transitions (e.g., `OutcomeReported` while in `NotStarted`) produce a recoverable error logged separately, not a panic.

### State derivation rules

- **Current node** at any seq N: fold events 1..=N, look at `cursor`.
- **Available artifacts** at any seq N: scan `ArtifactProduced` events 1..=N.
- **Cumulative cost** at any seq N: sum `TokensConsumed` events 1..=N, multiply by model rates.
- **Pending approvals**: find `ApprovalRequested` without matching `ApprovalDecided`.
- **Active session**: find `SessionOpened` without matching `SessionClosed`.

## Run lifecycle

### Phase 1: Initialization

User invokes `surge run "<description>"` from a project directory.

1. Engine generates `RunId` (UUIDv7).
2. Engine creates worktree branch `surge/run-<short_id>` from current `HEAD`.
3. Engine creates run directory `~/.surge/runs/<run_id>/`.
4. Engine writes initial event `RunStarted` to event log.
5. Engine forks subprocess to handle this run; main process returns control.
6. Daemon mode: engine continues even after CLI exits (run persists across terminal closures).

### Phase 2: Bootstrap

Three sub-stages, each with its own agent + approval cycle.

For each sub-stage:
1. `BootstrapStageStarted` event written.
2. Engine constructs prompt for that stage's agent (Description/Roadmap/Flow).
3. ACP session opened, agent invoked.
4. Tool calls and results recorded as events.
5. Agent emits artifact (description.md, roadmap.md, or flow.toml) → `BootstrapArtifactProduced`.
6. `BootstrapApprovalRequested` written, Telegram card sent.
7. Engine pauses. State persists to event log. Run continues to exist as a daemon.
8. User responds. Telegram bot writes `BootstrapApprovalDecided`.
9. If approve: next sub-stage. If edit: re-run sub-stage with feedback. If reject: `RunAborted`.

After Flow is approved:
- `flow.toml` content is parsed and validated.
- `PipelineMaterialized` event written with the frozen graph.

### Phase 3: Pipeline execution

The frozen graph is a directed graph with declared start node. Engine executes:

1. Read current cursor position.
2. Resolve inputs for the current node (from previous artifacts).
3. Execute node based on `NodeKind`:
   - **Agent**: open ACP session, run agent, receive `OutcomeReported`.
   - **HumanGate**: write `ApprovalRequested`, send to channel, pause.
   - **Branch**: evaluate predicate against memory, no agent involved.
   - **Terminal**: write `RunCompleted`/`RunFailed`/`RunAborted`, exit.
   - **Notify**: send notification, no pause.
   - **Loop**: enter iteration over collection.
   - **Subgraph**: enter inner graph at its start node.
4. Match outcome to declared outcomes for node, find edge, write `EdgeTraversed`.
5. Move cursor to target node.
6. Repeat from step 1.

### Phase 4: Termination

When a Terminal node is reached or an unrecoverable failure occurs:
1. Active session (if any) closed gracefully.
2. Final terminal event written.
3. If outcome is success and policy says auto-PR: PR Composer node would have run, PR exists.
4. Telegram notification sent.
5. Run subprocess exits.
6. Run remains in storage as completed; can be replayed but not resumed.

## Crash recovery

The engine must survive:
- App restart (CLI process killed)
- Machine restart (full power off)
- ACP agent crash mid-stage
- Network failures during Telegram delivery

### Recovery procedure

On engine startup, scan `~/.surge/runs/` for runs in non-terminal state. For each:

1. Load event log into memory.
2. Fold events to compute current state.
3. Inspect state:
   - **Awaiting approval**: re-send Telegram card if not already delivered (idempotent — message has run-relative ID).
   - **Agent session was active mid-stage**: treat as failed, write `StageFailed { reason: AppCrash }`, retry per node policy.
   - **Loop iteration in progress**: same as agent session — retry the iteration.
4. Resume normal execution from new state.

Idempotency is key: re-running stage logic for an attempt N must not corrupt N-1's results. Events from previous attempts persist; a new `StageEntered` event with `attempt: N+1` is what marks the new attempt.

### What is NOT recoverable

- Agent's mid-thought state (LLM context). Always lost across restarts. Stage retry starts the agent fresh with the same inputs.
- Tool calls in flight at crash time. Outputs that didn't get written to event log are lost; the tool effect (e.g., file modification) may persist on disk and the next attempt sees that state.
- ACP session reconnection. New session = new ID; old session is dead.

## Concurrency

### Within a run

Strictly sequential. One node executes at a time. Future versions may add `Parallel` node type for fan-out, but in v1 the engine processes one event source at a time.

### Across runs

Multiple runs can execute concurrently. Each run is isolated:
- Own SQLite event log (or own table partition)
- Own worktree branch
- Own ACP session pool
- Own subprocess (daemon)

The CLI tracks runs by ID. `surge list` queries all runs, `surge attach <run_id>` connects stdout to a running run for live tailing.

### Locking

A run is single-writer (the engine subprocess for that run). Other processes are read-only — they read events for display (UI, replay, CLI).

SQLite WAL mode is sufficient for this access pattern.

## Time-travel and forking

### Time-travel (read-only)

Given a run ID and target seq N:
1. Load events 1..=N.
2. Fold to get state at seq N.
3. Render UI from this state.

This is what the replay scrubber does. No state mutation, just selective fold.

### Fork (creates new run)

Given a run ID and fork point seq N:
1. Generate new RunId.
2. Copy events 1..=N from source run to new run's log (re-numbered as 1..=N in new run, with `forked_from: { source: source_id, at_seq: N }` metadata in the new run's `RunStarted` event).
3. Snapshot worktree at the corresponding state (using stored commit hash for the artifact at seq N or replaying file changes).
4. New run starts in state derived from those events. Engine resumes execution from there.
5. Source run is unaffected.

The forked run can take a different path because the user can override prompts, profiles, or graph structure at fork time. The engine treats it as a fresh run that just happens to start from a non-trivial state.

## Storage projection

The event log is the source of truth, but materialized views accelerate common queries:

- **`run_summary`** view: one row per run with `id`, `status`, `started_at`, `ended_at`, `total_cost`, `pipeline_template`. Updated on terminal events.
- **`stage_executions`** view: one row per stage attempt with timing, outcome, cost. Updated on `StageCompleted`/`StageFailed`.
- **`pending_approvals`** view: open `ApprovalRequested` without matching `ApprovalDecided`. Used by Telegram bot to know what's awaiting.
- **`artifacts_by_run`** view: artifact metadata per run.

These views are SQLite tables maintained by triggers or by the engine on event write. They are **derivable** — can be rebuilt from event log if corrupted.

## Open questions

- **Event log compaction.** A single 30-minute run can produce 500–2000 events. Across 100 runs, that's 200K events, ~50–200MB. Manageable but grows. Strategy: archive old runs (>90 days, terminal) to compressed format with full event log preserved, but excluded from default queries. Decided in implementation phase.

- **Schema migration.** Event payloads evolve. Strategy: every event has a `schema_version`. Engine has migration chain to read old events as new types. Tested via fixture-based replay test.

- **Distributed runs.** Out of scope for v1.0. Hard-coded assumption: one engine, one machine.

## Acceptance criteria

The execution model is correctly implemented when:

1. A run can be killed at any point (SIGKILL on engine subprocess) and resumed from CLI on next invocation, ending in the same terminal state as if not killed (modulo retry counters incrementing).
2. Replaying any completed run from event log on a fresh database produces a state semantically identical to the live state at run completion.
3. Forking from any seq within a run produces a valid runnable child run.
4. The materialized views are exactly recomputable from the event log alone.
5. End-to-end test: 100 events spanning all variants → fold to state → assertion against expected state. Repeat with random subsets to test composability.
