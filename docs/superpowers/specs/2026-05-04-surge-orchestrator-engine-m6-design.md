# M6 — `surge-orchestrator` engine — loops, subgraphs, real Notify, CLI

**Status:** Design — DRAFT for human review
**Date:** 2026-05-04
**Predecessor:** [M5 — surge-orchestrator engine](2026-05-03-surge-orchestrator-engine-m5-design.md) (M5.1 merged at `a34a29e`)
**Successor:** M7 (daemon mode + MCP server delegation — see §22.1)
**Aligns with:** `docs/revision/03-engine.md`, `docs/revision/0002-execution-model.md`, `docs/revision/0003-graph-model.md`, `docs/revision/cli.md`

> Scope: milestone M6 — extends the M5 engine from a strict-linear single-cursor
> executor into one that drives `Loop` and `Subgraph` nodes via a frame stack,
> ships real `Notify` channel delivery, and wires the engine into the CLI
> surface. Stays single-threaded **within a run** per revision §03-engine;
> multi-edge parallel fanout remains rejected and is deferred to M7+ via a
> future `NodeKind::Parallel`. Pure-addition strategy preserved: legacy
> `surge-orchestrator::{pipeline, phases, executor, parallel, planner, qa,
> retry, schedule}` modules untouched.
>
> **Scope deliberately split.** An earlier draft (also dated 2026-05-04)
> bundled this milestone with daemon mode and MCP server delegation — total
> ~10 weeks of solo evening work, four independent subsystems. Per
> [M6 review feedback](#) (2026-05-04), that monolith is split: M6 ships
> frame mechanics + Notify + CLI integration in-process; M7 ships daemon
> mode + MCP delegation; M8 picks up the previously-M7 retry / bootstrap /
> HumanGate-channels work that the earlier M5 spec §19 had grouped under
> M7. Each ships as an independently testable user-facing checkpoint.

## 1. Goals & non-goals

### 1.1 Goals

- **Sequential Loop execution.** `NodeKind::Loop` is supported. The body
  subgraph is iterated sequentially over the configured `IterableSource`.
  Exit conditions and iteration-failure policy honoured per `LoopConfig`.
  `EdgePolicy::max_traversals` becomes enforced (read-only in M5).
- **Subgraph execution.** `NodeKind::Subgraph` is supported. A subgraph
  stage runs its referenced inner subgraph (`Graph::subgraphs`) from the
  inner `start`, with `SubgraphConfig::inputs` projected as iteration
  bindings and `SubgraphConfig::outputs` projected back to the outer
  outcome. Subgraphs share the parent worktree.
- **Real Notify delivery.** `NodeKind::Notify` becomes a working stage
  via a pluggable `NotifyDeliverer` trait. M6 ships impls for Desktop,
  Webhook, Slack, Email, Telegram. The M5 "log + advance with
  `delivered`" stub is retired.
- **CLI integration (in-process).** A new `surge engine` subtree wires
  the engine into the CLI: `surge engine run <flow.toml> [--watch]`,
  plus `watch`, `resume`, `stop`, `ls`, `logs`. All commands run
  in-process (no daemon). Legacy `surge run` keeps its current
  FSM-based behaviour and is unchanged. The `--daemon` flag is **not**
  introduced in M6 — M7 retrofits it.
- **Forward-compat hygiene.** Two small `surge-core` amendments land
  as M6 prerequisites: (1) mark `EventPayload`, `EngineRunEvent`,
  `RunOutcome`, `NodeKind` as `#[non_exhaustive]` so future milestones
  can extend without a workspace-wide compile break; (2) add a graph
  validation rule for the Notify outcome contract (§10.4) so
  consumers building their own runners don't miss it.
- **Pure addition.** Same posture as M3 / M5: legacy modules unchanged,
  new functionality lives under `crates/surge-orchestrator/src/engine/`
  and one new crate `crates/surge-notify`. M5's public API
  (`Engine::{new, start_run, resume_run, stop_run,
  resolve_human_input}`) stays source-compatible.

### 1.2 Out of scope (deferred to later milestones)

- **Parallel within a run.** Per revision §0002 §Concurrency:
  *"Within a run: Strictly sequential. One node executes at a time.
  Future versions may add `Parallel` node type for fan-out."* M6 keeps
  this contract intact. Multi-edge fanout from a single
  `(from_node, outcome)` pair remains rejected at run-start with the
  same `RoutingError::MultipleMatches` that M5 surfaces. The future
  vehicle is a new `NodeKind::Parallel` (closed enum, requires core
  edit) — see §22.4. M8+ owns it.
- **Daemon mode.** A long-running `surge daemon` process hosting the
  engine over a local socket is **M7** scope. M6's CLI runs each
  command in-process: `surge engine run` constructs an `Engine`,
  drives the run, exits when the run terminates (or returns the
  run-id immediately if `--watch` is not set, then exits — the run
  itself terminates with the process). M7's daemon makes runs
  survive CLI exit.
- **MCP server delegation.** The M5 three-tool surface
  (`read_file`, `write_file`, `shell_exec`) stays unchanged in M6.
  Agents that need richer tooling get `Unsupported` for unknown
  tools, same as M5. M7 ships `McpRegistry` + `RoutingToolDispatcher`.
- **Retry policies, circuit breakers.** Stage-level retry stays out
  of scope. `AgentConfig::limits::max_retries` and `CbConfig` continue
  to be read but unused. M8 owns retry semantics. There is one
  M6-internal exception: `LoopConfig::on_iteration_failure::Retry
  { max }` retries an *iteration* (not a stage) — see §6.4.
- **Bootstrap stages.** Same gap as M5 §1.2. M8 owns the
  description → roadmap → flow.toml pipeline.
- **`gate_after_each` between loop iterations.** `LoopConfig::gate_after_each`
  is **rejected at validation** in M6 with a clear **M7** pointer.
  Implementing it cleanly requires a new `LoopConfig::gate_channel`
  field plus routing through the run's notification machinery —
  M7's daemon hosts the broadcast registry that makes per-iteration
  pause-and-resume tractable, so the feature lands together with
  daemon mode. See §22.5.
- **`FailurePolicy::Replan`.** Needs the bootstrap pipeline; M6 treats
  as `Abort` with a clear "not implemented" diagnostic. Resolved when
  M8 ships bootstrap.
- **Real sandbox enforcement.** M4 still owns Landlock /
  sandbox-exec / AppContainer impls.
- **HumanGate delivery channels.** M8 wires HumanGate channels through
  the same channel adapters M6 introduces for Notify, but with
  bidirectional request → response semantics.
- **Profile registry.** Same gap as M5 §1.2 / §19.8.
- **Tool result content store.** M5 §19.11 carried over.

## 2. Architectural decisions

### 2.1 Pure addition strategy (mirrors M3 / M5)

Same posture: `crates/surge-orchestrator/src/engine/` gains new
submodules, M5 files are edited in place only where §3.1 lists them,
legacy modules untouched. One new sibling crate: `surge-notify` (one
trait + 5 channel impls + a multiplexer + render).

Verified by acceptance #18 (§21): `git diff --stat
main..claude/m6-engine -- crates/surge-orchestrator/src/{pipeline,
phases, executor, parallel, planner, qa, retry, schedule}.rs` shows
zero insertions/deletions in those files.

### 2.2 Cursor + frame stack — single-threaded extension

The engine remains single-threaded per run. M5 carried one `Cursor
{ node, attempt }`; M6 adds a per-run **frame stack** that holds
nested execution context. The cursor still names "the one node we are
about to execute next"; the frame stack records "what we will do when
the cursor reaches a terminal node inside an inner graph".

```rust
pub struct ExecutionState {
    pub cursor: Cursor,                // unchanged: { node, attempt }
    pub frames: Vec<Frame>,            // pushed on entering Loop/Subgraph
}

pub enum Frame {
    Loop(LoopFrame),
    Subgraph(SubgraphFrame),
}

pub struct LoopFrame {
    pub loop_node: NodeKey,            // the outer Loop node
    pub config: LoopConfig,            // body, exit_condition, …
    pub items: Vec<toml::Value>,       // resolved at frame-push time, capped
    pub current_index: u32,
    pub attempts_remaining: u32,       // for FailurePolicy::Retry
    pub return_to: NodeKey,            // outer cursor on loop exit
    pub traversal_counts: HashMap<EdgeKey, u32>, // body-edge max_traversals enforcement
}

pub struct SubgraphFrame {
    pub outer_node: NodeKey,           // the Subgraph node
    pub inner_subgraph: SubgraphKey,   // referenced from graph.subgraphs
    pub bound_inputs: Vec<ResolvedSubgraphInput>,
    pub return_to: NodeKey,            // outer cursor on subgraph exit
}
```

The cursor advances inside a frame just as it does outside — Branch
predicates, Agent stages, Notify, etc., all work the same. The
**only** difference between "outer" execution and "in-frame"
execution is what happens when the cursor reaches a terminal node:

- **No frame on stack** (outer): the run ends; `RunCompleted` /
  `RunFailed` emitted per `TerminalKind`.
- **`LoopFrame` on top**: the iteration ended. The frame's
  `current_index` advances, the cursor resets to the body subgraph's
  `start`, and `LoopIterationStarted` for the next iteration is
  emitted. Or, if the exit condition is met, the frame pops and the
  cursor resumes at `return_to`.
- **`SubgraphFrame` on top**: the inner subgraph completed. Outputs
  project to the outer outcome, the frame pops, the cursor resumes
  at `return_to`.

This preserves the revision contract — exactly one node executes at
a time, the frame stack just tells the executor *which* "outside" to
resume into when an "inside" finishes.

### 2.3 Single-threaded executor — implementation

The per-run `tokio` task is still a single `loop { execute_one_node;
advance; }`, plus a frame-stack mutator that runs at terminal-node
boundaries. No `JoinSet`, no concurrency primitives within a run.
Multiple runs continue to share one process via `tokio::spawn`-per-run
exactly as M5 ships today (see [engine.rs:26](crates/surge-orchestrator/src/engine/engine.rs:26)).

The legacy M5 `run_task::execute` retains its loop shape; a small
helper (`engine/frames.rs::on_terminal`) handles the
"terminal-inside-frame → pop-or-iterate" transition.

```rust
// Pseudocode for the per-run loop, M6 shape.
loop {
    if cancelled { abort_and_exit; }

    let node = graph.lookup_in_active_frame(cursor.node, &frames)?;

    if let NodeConfig::Terminal(_) = &node.config {
        match frames.last() {
            None => return run_outcome(node);            // outer terminal
            Some(Frame::Loop(_)) => on_loop_iter_done().await?,
            Some(Frame::Subgraph(_)) => on_subgraph_done().await?,
        }
        continue;
    }

    // Non-terminal: execute the stage and route the outcome.
    let outcome = execute_stage(node).await?;
    cursor = next_cursor(graph, cursor.node, outcome, &mut frames)?;
    snapshot_if_boundary(...).await?;
}
```

### 2.4 Items cap for `LoopFrame` — bounded memory

Two caps prevent unbounded memory growth:

1. **`MAX_LOOP_ITEMS_STATIC` = 1000** in core graph validation:
   `IterableSource::Static(items)` with `items.len() > 1000` is
   rejected at TOML load with `ValidationError::LoopStaticTooLarge`.
2. **`MAX_LOOP_ITEMS_RESOLVED` = 1000** in engine at frame-push:
   `IterableSource::Artifact { … jsonpath }` resolved to more than
   1000 elements fails the stage with
   `EngineError::LoopItemsTooLarge { count, max }`.

The same constant value (1000) is used for both — the limit is about
in-memory `LoopFrame::items` size, not the source. Documented as M6
limitation; M8+ may revisit if real workloads need streaming
iteration (which would require a different frame model — frames
currently hold `items: Vec<toml::Value>` whole).

### 2.5 No multi-edge fanout — explicit rejection (preserved from M5)

`validate.rs` continues to reject any `(from_node, outcome)` pair
with more than one outgoing edge:

```rust
fn validate_unique_outgoing(graph: &Graph) -> Result<(), EngineError> {
    let mut seen: HashSet<(NodeKey, OutcomeKey)> = HashSet::new();
    for edge in &graph.edges {
        if !seen.insert((edge.from.node.clone(), edge.from.outcome.clone())) {
            return Err(EngineError::GraphInvalid(format!(
                "multiple edges from ({}, {}) — parallel fanout is M8+ scope (NodeKind::Parallel)",
                edge.from.node, edge.from.outcome
            )));
        }
    }
    // Recursively for graph.subgraphs[*].edges as well.
    Ok(())
}
```

This keeps the single-cursor invariant from §2.3 strictly enforced.

### 2.6 Notify delivery — pluggable channels

Replaces the M5 stub at
[stage/notify.rs](crates/surge-orchestrator/src/engine/stage/notify.rs).
The `NotifyDeliverer` trait lives in a new `surge-notify` crate:

```rust
#[async_trait::async_trait]
pub trait NotifyDeliverer: Send + Sync {
    async fn deliver(
        &self,
        ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError>;
}
```

The default `MultiplexingNotifier` dispatches on `NotifyChannel`
variant:

| Channel | Impl | Crate dep |
|---------|------|-----------|
| `Desktop` | `notify-rust` | `notify-rust = "4"` |
| `Webhook` | `reqwest::Client::post(url).json(payload)` | `reqwest` (workspace) |
| `Slack { channel_ref }` | `chat.postMessage` via Slack Web API | `reqwest` |
| `Email { to_ref }` | `lettre` async SMTP | `lettre = "0.11"` |
| `Telegram { chat_id_ref }` | Bot API `sendMessage` | `reqwest` |

`*_ref` fields are secret-store references; the deliverer resolves
them at send time and fails with `NotifyError::MissingSecret` if
unresolved.

**Difference from M8 HumanGate channels.** Notify is one-way:
render → deliver → fire-and-forget; the run advances regardless of
delivery success. HumanGate is request-response: the channel must
receive a reply and route it back to `Engine::resolve_human_input`.
M8 will likely build its bidirectional adapters on top of M6's
`NotifyDeliverer` infrastructure, but the trait is *not* the same —
HumanGate adapters need long-lived listeners (Telegram bot polling,
SMTP IMAP, Slack interactive-button webhook), where Notify channels
just open one HTTP connection per delivery.

**Outcome contract.** Notify nodes' `declared_outcomes` MUST include
`delivered`. If `on_failure: Fail`, they SHOULD also include
`undeliverable` to give the failure path a routing target. The
contract is enforced at graph load time — see §10.4. Engine emits
`delivered` on success or `on_failure: Continue` post-error; emits
`undeliverable` (if declared) or `StageFailed` (if not) on
`on_failure: Fail` post-error.

### 2.7 CLI integration — new `surge engine` subtree (in-process)

A new top-level subcommand:

```
surge engine run    <flow.toml>     [--watch] [--worktree <path>]
surge engine watch  <run-id>
surge engine resume <run-id>
surge engine stop   <run-id>        [--reason <text>]
surge engine ls
surge engine logs   <run-id>        [--since <seq>] [--follow]
```

All commands run **in-process**. `run` constructs an `Engine`, calls
`start_run`, and either:
- streams events to stderr if `--watch` (returns when run terminates),
- or returns the run id and exits, with the run terminating alongside
  the process (in M6 there is no daemon to keep it alive). Without
  `--watch` is mostly useful for the resulting run-id followed by
  immediate `surge engine watch` invocation, or for pure scripting
  where the script keeps the process alive.

**No `--daemon` flag in M6.** M7 retrofits the flag and, with it,
the ability to detach a run from the CLI process. M6 callers who
need detached execution must wrap the invocation in their own
process-management (`nohup`, `systemd-run`, `screen`, `&`).

`watch`, `resume`, `stop`, `ls`, `logs` operate against the
persistence layer (`~/.surge/runs/<run_id>/`) — they read events
from the on-disk log even if no live engine is attached. `watch`
and `stop` work fully only when a live engine in the current
process owns the run; otherwise they return a clear "no live
engine" error suggesting either `surge engine resume` or M7's
daemon. (This is an explicit M6 limitation; the API surface is
ready for M7 to layer daemon dispatch underneath.)

**Coexistence with legacy `surge run`.** Legacy `surge run <spec_id>`
keeps its current FSM-based behaviour (routes to
[commands::pipeline::run](crates/surge-cli/src/commands/pipeline.rs),
uses M0-era markdown specs). The new subcommand `surge engine run`
takes a TOML flow path. The two paths are parallel:

| Command | Spec format | Engine | Status |
|---------|-------------|--------|--------|
| `surge run` | Markdown / older TOML, FSM stages | Legacy `surge-orchestrator::pipeline` | Stable, M0–M2-era |
| `surge engine run` | flow.toml graph | M6 `engine::Engine` | New in M6 |

We do NOT auto-detect spec format — authors choose their command.
Unification is M9+ when legacy is fully deprecated.

### 2.8 Routing — extended for max_traversals enforcement only

`routing::next_node_after` keeps its M5 signature and "single match"
semantics. The error variants are the same:

- `NoMatchingEdge` (no edge from this `(node, outcome)`)
- `MultipleMatches` (M5/M6: rejected; reserved for future
  `NodeKind::Parallel`)

What's new: per-edge **traversal counters**, scoped to the enclosing
loop frame (or the whole run if no loop frame). Each `EdgeTraversed`
event increments the counter for that edge in the current frame. When
the counter exceeds `EdgePolicy::max_traversals`, the
`EdgePolicy::on_max_exceeded` action wins:
- `Escalate` (default) — synthesise a `max_traversals_exceeded`
  outcome from the source node; routing re-resolves with that
  outcome (typically routes to a HumanGate or Notify); if nothing
  accepts that outcome, the stage fails with `RoutingDeadEnd`.
- `Fail` — halt the run.

This delivers M5 §11's "M6 honours it" promise. The counter lives
in `LoopFrame::traversal_counts` for in-loop edges and in
`ExecutionState::root_traversal_counts: HashMap<EdgeKey, u32>` for
edges outside frames.

### 2.9 Forward-compat hygiene — `#[non_exhaustive]` retrofit

Two surge-core enums and two engine enums get `#[non_exhaustive]`
in M6 prerequisite Phase 1:

```rust
// surge-core
#[non_exhaustive] pub enum EventPayload { … }
#[non_exhaustive] pub enum NodeKind { … }

// surge-orchestrator::engine
#[non_exhaustive] pub enum EngineRunEvent { … }
#[non_exhaustive] pub enum RunOutcome { … }
```

This is a **one-time defensive retrofit**: existing exhaustive
matches inside the workspace gain `_ => { /* … */ }` arms. Forward
benefit: future milestones adding `EventPayload::McpServerSpawned`
(M7) or `NodeKind::Parallel` (M8+) becomes additive, not a
workspace-wide compile break. External consumers of `surge-core`
benefit immediately.

This is the lesson of §4.3 in the previous M6 draft, which proposed
adding many new variants to `EngineRunEvent` and got it wrong:
`EngineRunEvent` is a thin wrapper (`Persisted { seq, payload } |
Terminal(RunOutcome)`), the new event details flow through
`EventPayload` automatically. M6 doesn't add `EngineRunEvent`
variants — but locking the door for future milestones is cheap.

### 2.10 Snapshot v2 — cursor + frames

`EngineSnapshot` evolves to schema_version=2:

```rust
pub struct EngineSnapshot {
    pub schema_version: u32,                // = 2 in M6
    pub cursor: SerializableCursor,         // unchanged shape
    pub frames: Vec<SerializableFrame>,     // NEW: frame stack
    pub root_traversal_counts: HashMap<String /*EdgeKey*/, u32>, // NEW
    pub at_seq: u64,
    pub stage_boundary_seq: u64,
    pub pending_human_input: Option<PendingHumanInputSnapshot>,
}

pub enum SerializableFrame {
    Loop(SerializableLoopFrame),
    Subgraph(SerializableSubgraphFrame),
}
```

**Backward compatibility with M5 v1 snapshots.**
`EngineSnapshot::deserialize` reads `schema_version` first; v1 maps
to v2 with `frames = vec![]` and `root_traversal_counts = HashMap::new()`.
M5 paused-on-input runs deserialise correctly. Documented in §13.

**Frequency.** Snapshot at every "boundary": after any
`StageCompleted`, after any frame push/pop. Per-iteration boundary
inside a Loop is the snapshot point — *not* per inner-stage
completion. For a 1000-iteration loop with 10 inner stages, ~1010
snapshots, each ~1–5 KB.

### 2.11 Bindings, predicates, sandbox factory — unchanged from M5

`AgentConfig::bindings` resolution, `predicate::evaluate`,
`sandbox_factory::build_sandbox` work identically. Inside a frame,
predicates evaluate against the same `RunMemory` (which folds events
across all frames); `outcome_of` returns the most recent outcome at
the named node regardless of which frame produced it. Loop iteration
bindings are resolved at frame-push time and exposed under the
`LoopConfig::iteration_var_name` template variable to inner stages.

## 3. Module layout after M6

### 3.1 `surge-orchestrator/src/engine/` extensions

```
crates/surge-orchestrator/src/
├── engine/
│   ├── mod.rs                     (extended re-exports)
│   ├── error.rs                   (extended: LoopError, SubgraphError, NotifyError, LoopItemsTooLarge)
│   ├── engine.rs                  (M6 backwards-compat — same public methods)
│   ├── handle.rs                  (#[non_exhaustive] retrofit on EngineRunEvent / RunOutcome)
│   ├── config.rs                  (extended: notify_deliverer field on EngineConfig)
│   ├── snapshot.rs                (v2: + frames + traversal counts; v1 reader retained)
│   ├── run_task.rs                (extended: terminal-inside-frame branch)
│   ├── frames.rs                  (NEW: Frame, LoopFrame, SubgraphFrame, on_terminal helper)
│   ├── stage/
│   │   ├── mod.rs                 (extended re-exports)
│   │   ├── agent.rs               (unchanged; same per-stage model)
│   │   ├── branch.rs              (unchanged)
│   │   ├── human_gate.rs          (unchanged)
│   │   ├── terminal.rs            (extended: defers to frames module on inner terminal)
│   │   ├── notify.rs              (REWRITTEN: real delivery via NotifyDeliverer)
│   │   ├── loop_stage.rs          (NEW: execute_loop_entry, iteration boundary)
│   │   └── subgraph_stage.rs     (NEW: execute_subgraph_entry, output projection)
│   ├── tools/
│   │   ├── mod.rs                 (unchanged)
│   │   ├── worktree.rs            (unchanged)
│   │   └── path_guard.rs          (unchanged)
│   ├── routing.rs                 (extended: traversal counter + max_traversals enforcement)
│   ├── predicates.rs              (unchanged)
│   ├── sandbox_factory.rs         (unchanged)
│   ├── replay.rs                  (extended: rebuilds frame stack from new events)
│   └── validate.rs                (relaxed: Loop/Subgraph allowed; subgraph refs validated;
│                                   multi-edge same-port still rejected; gate_after_each rejected)
└── lib.rs                         (no new top-level modules)
```

Total new files: 3. Total edited M5 files: 9.

### 3.2 `crates/surge-notify/` (new crate)

```
crates/surge-notify/
├── Cargo.toml                     (deps: notify-rust, reqwest, lettre, async-trait, …)
└── src/
    ├── lib.rs                     (re-exports)
    ├── deliverer.rs               (NotifyDeliverer trait + NotifyError)
    ├── multiplexer.rs             (MultiplexingNotifier with builder API)
    ├── render.rs                  (template rendering: {{run_id}}, {{node}}, {{artifact:name}})
    ├── desktop.rs                 (notify-rust impl)
    ├── webhook.rs                 (reqwest impl)
    ├── slack.rs                   (Slack chat.postMessage)
    ├── email.rs                   (lettre SMTP)
    └── telegram.rs                (Telegram Bot sendMessage)
```

### 3.3 `surge-core` changes (small, surgical)

Three additions, all additive:

- **New `EventPayload` variants:**
  ```rust
  EventPayload::SubgraphEntered { outer: NodeKey, inner: SubgraphKey }
  EventPayload::SubgraphExited { outer: NodeKey, inner: SubgraphKey, outcome: OutcomeKey }
  EventPayload::NotifyDelivered {
      node: NodeKey,
      channel_kind: NotifyChannelKind,
      success: bool,
      error: Option<String>,
  }
  ```
- **`#[non_exhaustive]` retrofit** on `EventPayload`, `NodeKind` (M6
  prerequisite Phase 1).
- **Extended graph validation** (`crates/surge-core/src/validation.rs`):
  rule N+1 (Notify outcome contract). See §10.4. Implementation:
  ~30 lines + ~5 unit tests.

`VersionedEventPayload::schema_version` stays at `1` per the same
rationale as M5 §2.10.

`Cursor` is unchanged. The frame stack lives in `EngineSnapshot`
only — engine-internal concept, not part of the canonical run state
in `surge-core::run_state`. Reconsider in M8 if `RunState` consumers
(UI, replay) need to materialise frame state from events alone.

### 3.4 `surge-cli` extensions

- `crates/surge-cli/src/commands/engine.rs` (new): `EngineCommands`
  enum, dispatch directly into `surge-orchestrator::engine`.
- [main.rs](crates/surge-cli/src/main.rs)'s `Cli::Commands` gains
  one new variant `Engine { command: EngineCommands }`. All other
  variants unchanged.
- Event printing helper (`commands/engine/print.rs`) renders
  `EngineRunEvent` to stderr in compact form; ANSI colours via
  `owo-colors` if stderr is a TTY.

## 4. Public API surfaces

### 4.1 `Engine` — backwards-compatible

The M5 `Engine::{new, start_run, resume_run, stop_run,
resolve_human_input}` signatures are unchanged. M5 callers compile
against the same surface.

New constructor for the notify deliverer:

```rust
impl Engine {
    /// M6 constructor that wires a real notify deliverer.
    pub fn new_with_notifier(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        notify_deliverer: Arc<dyn NotifyDeliverer>,
        config: EngineConfig,
    ) -> Self;
}
```

The M5 `Engine::new` is kept as a thin wrapper that constructs a
no-op `MultiplexingNotifier::default()` (behaves like the M5 stub:
`deliver` returns `Err(ChannelNotConfigured)`, the `compute_outcome`
helper maps that to `delivered` if `on_failure: Continue`, else
`undeliverable` / `StageFailed`).

### 4.2 `EngineRunConfig` extensions

```rust
pub struct EngineRunConfig {
    pub human_input_timeout: Duration,
    pub stage_timeout_override: Option<Duration>,

    // M6 additions:
    /// Per-iteration timeout cap inside loops. None = inherit from agent's
    /// limits.timeout_seconds.
    pub loop_iteration_timeout: Option<Duration>,
}
```

### 4.3 `EngineRunEvent` — unchanged

`EngineRunEvent` continues to be `Persisted { seq, payload } |
Terminal(RunOutcome)`. The new event payload variants
(`SubgraphEntered`, `SubgraphExited`, `NotifyDelivered`,
`LoopIteration*`, `LoopCompleted`) flow through `Persisted.payload`
and are surfaced to subscribers as before. M5 callers consuming the
broadcast keep working unchanged.

The retrofit `#[non_exhaustive]` on `EngineRunEvent` requires any
exhaustive match to add a `_ => {}` arm — one-time workspace
adjustment, locked in M6 prerequisite Phase 1.

### 4.4 `NotifyDeliverer` trait

(See §2.6.) Lives in `surge-notify::deliverer`. Default
`MultiplexingNotifier::default()` constructs a notifier with all
five channels wired but each returning `ChannelNotConfigured` until
the consumer overrides via builder methods (`with_desktop`,
`with_webhook`, etc.). Production wiring happens in
`surge-cli::commands::engine` (and in M7's daemon).

### 4.5 `EngineError` extensions

```rust
#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    // … M5 variants unchanged …

    // M6:
    #[error("subgraph reference {0} not found in graph.subgraphs")]
    SubgraphMissing(SubgraphKey),

    #[error("loop body reference {0} not found in graph.subgraphs")]
    LoopBodyMissing(SubgraphKey),

    #[error("notify delivery error: {0}")]
    Notify(String),

    #[error("loop iterable resolved to {count} items, exceeds maximum {max}")]
    LoopItemsTooLarge { count: u32, max: u32 },

    #[error("edge {edge} max_traversals exceeded ({count}/{max}) — action: {action}")]
    EdgeMaxTraversals { edge: EdgeKey, count: u32, max: u32, action: String },
}
```

## 5. Run lifecycle

### 5.1 Cold start — same as M5 with relaxed validation

```
1. Validate graph (M6 rules):
   - Loop and Subgraph nodes ALLOWED.
   - Multi-edge from same (from_node, outcome) — STILL REJECTED with M8+ pointer.
   - Subgraph references in Loop.body and Subgraph.inner must resolve.
   - Inner subgraphs themselves validated recursively.
   - LoopConfig::gate_after_each = true REJECTED with M7 pointer.
   - Notify nodes' declared_outcomes must include `delivered`; if
     on_failure: Fail, SHOULD include `undeliverable` (warning).
   - IterableSource::Static(items) with len > 1000 REJECTED.
2. Open RunWriter; verify worktree.
3. Append RunStarted + PipelineMaterialized atomically (same as M5).
4. Compute initial cursor = Cursor { graph.start, 1 }; frame stack empty.
5. Spawn run_task with the cursor + empty frames + bridge + storage + tool dispatcher
   + notify deliverer.
6. Return RunHandle.
```

### 5.2 Warm start — frames reconstructed from events

```
1. Open RunWriter, RunReader.
2. Read latest snapshot. Branch on schema_version:
   - 1 (M5): cursor only, frames empty, root_traversal_counts empty.
   - 2 (M6): full frame stack + counts.
3. Replay post-snapshot events:
   - SubgraphEntered → push SubgraphFrame.
   - SubgraphExited → pop.
   - LoopIterationStarted (index 0) → push LoopFrame.
   - LoopIterationStarted (index N>0) → bump current_index on top frame.
   - LoopCompleted → pop top frame.
   - EdgeTraversed → bump traversal counter for the edge (in top loop frame
     if any, else root counts).
   - HumanInputRequested → set pending_human_input; HumanInputResolved/TimedOut
     → clear.
4. Spawn run_task in resume mode at the recovered (cursor, frames, counts).
```

The graph is recovered from `PipelineMaterialized` (same as M5).

### 5.3 Per-stage / per-frame loop

(Pseudocode from §2.3 expanded.)

`graph.lookup_in_active_frame(&cursor.node, &frames)` resolves the
node against the *innermost frame's* node set: for a `LoopFrame`,
against `graph.subgraphs[frame.config.body].nodes`; for a
`SubgraphFrame`, against `graph.subgraphs[frame.inner_subgraph].nodes`;
outer, against `graph.nodes`. Same for edges in routing.

### 5.4 Stop / completion semantics — same as M5

`Engine::stop_run` flips the cancellation token. The loop checks the
token between iterations; on cancel it:
1. Replies to any in-flight tool call with `Cancelled`.
2. Closes the bridge session (if any).
3. Writes a final snapshot.
4. Emits `RunAborted`.
5. Returns.

Idempotent.

### 5.5 Run completion

A run completes when the cursor reaches a `Terminal` node *with an
empty frame stack*. The terminal kind drives `RunCompleted` /
`RunFailed` per M5 semantics. Inner-frame terminals trigger frame
exit logic (§6) and never directly terminate the run.

## 6. Stage execution detail

### 6.1 Agent / Branch / HumanGate / Terminal — unchanged from M5

Bit-for-bit unchanged in their stage execution helpers. The only
M6 difference is that `Terminal` now consults the frame stack via
`frames::on_terminal` rather than unconditionally ending the run.

### 6.2 Notify — real delivery

```rust
async fn execute_notify_stage(p: NotifyStageParams<'_>) -> StageResult {
    let rendered = render::render(&p.notify_config.template, &p.run_memory)?;
    let resolved = resolve_artifacts(&rendered, &p.notify_config.template, &p.run_memory)?;

    let result = p.notify_deliverer
        .deliver(
            &NotifyDeliveryContext { run_id: p.run_id, node: p.node },
            &p.notify_config.channel,
            &resolved,
        )
        .await;

    let outcome = compute_outcome(&result, &p.notify_config.on_failure, &p.declared_outcomes)?;

    p.writer.append_event(VersionedEventPayload::new(EventPayload::NotifyDelivered {
        node: p.node.clone(),
        channel_kind: p.notify_config.channel.kind(),
        success: result.is_ok(),
        error: result.as_ref().err().map(|e| e.to_string()),
    })).await?;

    p.writer.append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: p.node.clone(),
        outcome: outcome.clone(),
        summary: result.as_ref()
            .map(|_| "delivered".into())
            .unwrap_or_else(|e| format!("delivery error: {e}")),
    })).await?;

    Ok(outcome)
}

fn compute_outcome(
    result: &Result<(), NotifyError>,
    on_failure: &NotifyFailureAction,
    declared: &[OutcomeDecl],
) -> Result<OutcomeKey, StageError> {
    let delivered = OutcomeKey::try_from("delivered").unwrap();
    match (result, on_failure) {
        (Ok(()), _) => Ok(delivered),
        (Err(_), NotifyFailureAction::Continue) => Ok(delivered),
        (Err(e), NotifyFailureAction::Fail) => {
            let undeliverable = OutcomeKey::try_from("undeliverable").unwrap();
            if declared.iter().any(|o| o.id == undeliverable) {
                Ok(undeliverable)
            } else {
                Err(StageError::NotifyDelivery(e.to_string()))
            }
        }
    }
}
```

Outcomes contract documented in `NotifyConfig` rustdoc and tested
via unit tests on `compute_outcome`. Graph-level validation (§10.4)
catches missing `delivered` / `undeliverable` declarations at TOML
load.

### 6.3 Loop entry — push frame, advance cursor

```rust
async fn execute_loop_entry(p: LoopStageParams<'_>) -> StageResult {
    let body_subgraph = p.graph.subgraphs.get(&p.loop_config.body)
        .ok_or(StageError::LoopBodyMissing(p.loop_config.body.clone()))?;

    let items = resolve_iterable(&p.loop_config.iterates_over, &p.run_memory).await?;

    // Cap check: refuse oversized resolved iterables.
    if items.len() > MAX_LOOP_ITEMS_RESOLVED {
        return Err(StageError::LoopItemsTooLarge {
            count: items.len() as u32,
            max: MAX_LOOP_ITEMS_RESOLVED as u32,
        });
    }

    if items.is_empty() {
        let outcome = OutcomeKey::try_from("loop_empty").unwrap();
        p.writer.append_event(VersionedEventPayload::new(EventPayload::LoopCompleted {
            loop_id: p.node.clone(),
            completed_iterations: 0,
            final_outcome: outcome.clone(),
        })).await?;
        return Ok(outcome);
    }

    let return_to = routing::edge_target_after_outcome(&p.graph, &p.node, &OutcomeKey::try_from("completed").unwrap())?;

    p.frames.push(Frame::Loop(LoopFrame {
        loop_node: p.node.clone(),
        config: p.loop_config.clone(),
        items,
        current_index: 0,
        attempts_remaining: match p.loop_config.on_iteration_failure {
            FailurePolicy::Retry { max } => max,
            _ => 0,
        },
        return_to,
        traversal_counts: HashMap::new(),
    }));

    p.writer.append_event(VersionedEventPayload::new(EventPayload::LoopIterationStarted {
        loop_id: p.node.clone(),
        item: items[0].clone(),
        index: 0,
    })).await?;

    Ok(StageEffect::AdvanceTo(body_subgraph.start.clone()))
}
```

### 6.4 Loop iteration boundary — terminal-inside-loop

When the cursor reaches a `Terminal` node and `frames.last() == Loop(_)`:

```rust
async fn on_loop_iteration_done(
    frames: &mut Vec<Frame>,
    cursor: &mut Cursor,
    /* writer, graph, run_memory */
) -> Result<(), StageError> {
    let Frame::Loop(loop_frame) = frames.last_mut().expect("Loop frame on top") else { unreachable!() };

    let just_completed_outcome = current_branch_outcome();   // from RunMemory's last OutcomeReported

    writer.append_event(EventPayload::LoopIterationCompleted {
        loop_id: loop_frame.loop_node.clone(),
        index: loop_frame.current_index,
        outcome: just_completed_outcome.clone(),
    }).await?;

    // 1. Iteration-failure handling.
    if is_failure(&just_completed_outcome) {
        match &loop_frame.config.on_iteration_failure {
            FailurePolicy::Abort => return exit_loop(loop_frame, frames, cursor, "aborted").await,
            FailurePolicy::Skip => { /* fall through to advance index */ }
            FailurePolicy::Retry { .. } if loop_frame.attempts_remaining > 0 => {
                loop_frame.attempts_remaining -= 1;
                cursor.node = body_subgraph_start(graph, loop_frame);
                cursor.attempt += 1;
                writer.append_event(LoopIterationStarted { /* same index */ }).await?;
                return Ok(());
            }
            FailurePolicy::Retry { .. } => {
                return exit_loop(loop_frame, frames, cursor, "aborted").await; // exhausted
            }
            FailurePolicy::Replan => {
                tracing::warn!("FailurePolicy::Replan not implemented in M6 — treating as Abort");
                return exit_loop(loop_frame, frames, cursor, "aborted").await;
            }
        }
    }

    // 2. Exit condition.
    if exit_condition_met(loop_frame, &just_completed_outcome) {
        return exit_loop(loop_frame, frames, cursor, "completed").await;
    }

    // 3. Advance to next iteration.
    loop_frame.current_index += 1;
    if loop_frame.current_index >= loop_frame.items.len() as u32 {
        return exit_loop(loop_frame, frames, cursor, "completed").await;
    }

    cursor.node = body_subgraph_start(graph, loop_frame);
    cursor.attempt = 1;

    writer.append_event(LoopIterationStarted {
        loop_id: loop_frame.loop_node.clone(),
        item: loop_frame.items[loop_frame.current_index as usize].clone(),
        index: loop_frame.current_index,
    }).await?;

    Ok(())
}
```

`exit_condition_met` checks `LoopConfig::exit_condition`:
- `AllItems` — exit when index == len-1.
- `UntilOutcome { from_node, outcome }` — exit when the most recent
  `OutcomeReported` event for `from_node` matches `outcome`.
- `MaxIterations { n }` — exit when index+1 >= n.

`exit_loop` pops the frame, emits `LoopCompleted`, and sets the
cursor to `loop_frame.return_to`.

**`gate_after_each` is rejected at validation** (§5.1). The
implementation hook is reserved but not wired in M6.

### 6.5 Subgraph entry

```rust
async fn execute_subgraph_entry(p: SubgraphStageParams<'_>) -> StageResult {
    let inner = p.graph.subgraphs.get(&p.subgraph_config.inner)
        .ok_or(StageError::SubgraphMissing(p.subgraph_config.inner.clone()))?;

    let bound_inputs = resolve_subgraph_inputs(&p.subgraph_config.inputs, &p.run_memory)?;

    let return_to = routing::edge_target_after_outcome(&p.graph, &p.node, &OutcomeKey::try_from("completed").unwrap())?;

    p.frames.push(Frame::Subgraph(SubgraphFrame {
        outer_node: p.node.clone(),
        inner_subgraph: p.subgraph_config.inner.clone(),
        bound_inputs,
        return_to,
    }));

    p.writer.append_event(VersionedEventPayload::new(EventPayload::SubgraphEntered {
        outer: p.node.clone(),
        inner: p.subgraph_config.inner.clone(),
    })).await?;

    Ok(StageEffect::AdvanceTo(inner.start.clone()))
}
```

### 6.6 Subgraph exit — terminal-inside-subgraph

```rust
async fn on_subgraph_done(
    frames: &mut Vec<Frame>,
    cursor: &mut Cursor,
    /* writer, graph, run_memory */
) -> Result<(), StageError> {
    let Frame::Subgraph(frame) = frames.last().unwrap() else { unreachable!() };

    let outcome = project_outputs(&frame, &graph.subgraphs[&frame.inner_subgraph], &run_memory)?;

    writer.append_event(VersionedEventPayload::new(EventPayload::SubgraphExited {
        outer: frame.outer_node.clone(),
        inner: frame.inner_subgraph.clone(),
        outcome: outcome.clone(),
    })).await?;

    writer.append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: frame.outer_node.clone(),
        outcome: outcome.clone(),
        summary: format!("subgraph {} completed", frame.inner_subgraph),
    })).await?;

    let return_to = frame.return_to.clone();
    frames.pop();

    cursor.node = routing::edge_target_after_outcome(&graph, &frame.outer_node, &outcome)?;
    cursor.attempt = 1;

    Ok(())
}
```

`project_outputs` walks `SubgraphConfig::outputs`: for each entry,
look up the inner artifact in `RunMemory.artifacts`, and if found,
use the entry's `outer_outcome` as the projected outer outcome.
**First match wins** (deterministic; documented). Authors who want
"fan-out on multiple matches" need `NodeKind::Parallel` (M8+); for
now this is a single-outcome contract.

### 6.7 No multi-edge fanout — preserved (§2.5)

(Already covered in §2.5.)

## 7. Routing — extended for traversal counters

```rust
pub fn next_node_after(
    graph: &Graph,
    current: &NodeKey,
    outcome: &OutcomeKey,
    frames: &mut Vec<Frame>,
    root_traversal_counts: &mut HashMap<EdgeKey, u32>,
) -> Result<NodeKey, RoutingError> {
    let edges = active_edge_set(graph, frames);  // body subgraph's edges if inside a loop frame

    let edge = edges.iter()
        .find(|e| e.from.node == *current && e.from.outcome == *outcome)
        .ok_or(RoutingError::NoMatchingEdge { from: current.clone(), outcome: outcome.clone() })?;

    let counts = match frames.last_mut() {
        Some(Frame::Loop(loop_frame)) => &mut loop_frame.traversal_counts,
        _ => root_traversal_counts,
    };
    let count = counts.entry(edge.id.clone()).or_insert(0);
    *count += 1;

    // Note: avoiding let-chains (stable in 1.88+; workspace MSRV is 1.85).
    if let Some(max) = edge.policy.max_traversals {
        if *count > max {
            return match edge.policy.on_max_exceeded {
                ExceededAction::Escalate => Err(RoutingError::ExceededTraversal {
                    edge: edge.id.clone(), action: ExceededAction::Escalate,
                }),
                ExceededAction::Fail => Err(RoutingError::ExceededTraversal {
                    edge: edge.id.clone(), action: ExceededAction::Fail,
                }),
            };
        }
    }

    Ok(edge.to.clone())
}
```

The caller maps `RoutingError::ExceededTraversal { Escalate }` to a
synthetic `max_traversals_exceeded` outcome and re-resolves; if no
edge accepts the synthetic outcome, the stage fails with
`StageError::RoutingDeadEnd`.

## 8. Sandbox factory — unchanged from M5

`build_sandbox` returns `AlwaysAllowSandbox` for every variant. M4
will replace the factory body without engine API change.

## 9. Predicate evaluation — unchanged from M5

`predicate::evaluate` keeps its signature and semantics. Inside a
loop body, `outcome_of` returns the most recent outcome at the named
node from `RunMemory.outcomes` (which folds across all iterations).
Loop iteration variables are resolved at frame-push time and exposed
as bindings to inner stages.

## 10. Notify delivery — full design

### 10.1 Trait + multiplexer

`NotifyDeliverer` trait + `MultiplexingNotifier` dispatching on
`NotifyChannel` variant. Default `MultiplexingNotifier::default()`
constructs a notifier with all five channels wired but each
returning `ChannelNotConfigured` until the consumer overrides.
Production wiring happens in `surge-cli::commands::engine` and (at
M7) the daemon.

### 10.2 Template rendering

`NotifyTemplate::title` and `body` carry mustache-style placeholders:
- `{{run_id}}`
- `{{node}}` (the notify node's id)
- `{{outcome}}` (the most recent outcome of any node in this run)
- `{{artifact:NAME}}` (inserts the path of artifact `NAME`)
- `{{stage_summary}}` (most recent `OutcomeReported.summary`)

Missing placeholders render as empty strings (lenient — alerts
shouldn't fail because a placeholder is unset).

### 10.3 Channel impls

| Channel | Mechanism | Failure modes |
|---------|-----------|---------------|
| `Desktop` | `notify_rust::Notification::new()...show()` | Linux without notification daemon → `Transport`. |
| `Webhook { url }` | `reqwest::Client::post(url).json(payload)` | HTTP error → `Transport`. |
| `Slack { channel_ref }` | resolve secret → bot token → `chat.postMessage` | Token missing → `MissingSecret`. |
| `Email { to_ref }` | resolve recipient + SMTP creds → `lettre::AsyncSmtpTransport` | Creds missing → `MissingSecret`. |
| `Telegram { chat_id_ref }` | resolve chat id → Bot API `sendMessage` | Token missing → `MissingSecret`. |

All HTTP-based channels share one `reqwest::Client` per
`MultiplexingNotifier`.

### 10.4 Outcome contract — enforced in `surge-core::validation`

Added as **graph validation rule N+1** (the "N+1" relative to M1's
existing rule list, exact number TBD when implementing):

> **Notify outcome contract.** A `Notify` node MUST declare the
> `delivered` outcome. If `on_failure: Fail` is used, the node
> SHOULD declare `undeliverable`. Missing `delivered` is a hard
> error; missing `undeliverable` with `Fail` is a warning.

Implementation: ~30 lines in
`crates/surge-core/src/validation.rs::validate_node`, extending the
existing per-NodeKind validation with a `NodeKind::Notify` arm.
Tested via fixture-based round-trip tests.

This places the rule at the canonical validation layer — anyone
loading a graph (engine, editor, future external runners) gets the
check for free.

## 11. (Reserved for M7 — MCP server delegation)

M7 adds `RoutingToolDispatcher` and `McpRegistry`, which layer over
M6's tool dispatch surface without changing the
`ToolDispatcher` trait or the M6 `WorktreeToolDispatcher`. Section
intentionally left as a placeholder so the spec's section numbering
matches across milestones.

## 12. Snapshot strategy v2 (revisited)

### 12.1 Frequency

Snapshot at every "boundary":
- After any `StageCompleted` (outer or inner).
- After any frame push (`SubgraphEntered`, first
  `LoopIterationStarted`).
- After any frame pop (`SubgraphExited`, `LoopCompleted`).

For a 1000-iteration loop with 10 inner stages: ~1010 snapshots,
each ~1–5 KB.

### 12.2 Content

```rust
pub struct EngineSnapshot {
    pub schema_version: u32,                 // = 2
    pub cursor: SerializableCursor,
    pub frames: Vec<SerializableFrame>,
    pub root_traversal_counts: HashMap<String, u32>,
    pub at_seq: u64,
    pub stage_boundary_seq: u64,
    pub pending_human_input: Option<PendingHumanInputSnapshot>,
}
```

### 12.3 Resume — back-compat with v1

```rust
impl EngineSnapshot {
    pub fn deserialize(blob: &[u8]) -> Result<Self, SnapshotError> {
        let value: serde_json::Value = serde_json::from_slice(blob)?;
        match value.get("schema_version").and_then(|v| v.as_u64()) {
            Some(1) => Ok(Self::from_v1(serde_json::from_value(value)?)),
            Some(2) => Ok(serde_json::from_value(value)?),
            other => Err(SnapshotError::UnsupportedSchema(other)),
        }
    }
}
```

## 13. Persistence integration

### 13.1 New event variants

(See §3.3.) `SubgraphEntered`, `SubgraphExited`, `NotifyDelivered`.
All additive — `VersionedEventPayload::schema_version` stays at `1`.

### 13.2 Stage event sequences

Loop stage:
```
StageEntered → LoopIterationStarted(0) → [ inner stages ]
  → LoopIterationCompleted(0) → LoopIterationStarted(1) → [ inner stages ]
  → LoopIterationCompleted(1) → … → LoopCompleted → OutcomeReported → StageCompleted
```

Subgraph stage:
```
StageEntered → SubgraphEntered → [ inner stages ]
  → SubgraphExited → OutcomeReported → StageCompleted
```

Notify stage (M6):
```
StageEntered → NotifyDelivered → OutcomeReported → StageCompleted
```

### 13.3 Snapshot write timing

Right after `StageCompleted` (outer or inner — including the
synthetic ones emitted by `LoopCompleted` and `SubgraphExited`).
Right after `LoopIterationStarted` for index > 0 (iteration boundary).
Right after `SubgraphEntered`.

## 14. Concurrency model

Per revision §03-engine:

- **Within a run** — strictly sequential. One `tokio::spawn`ed task
  per run; the task's loop processes one node at a time. No
  `JoinSet`, no per-stage parallelism.
- **Across runs** — multiple runs run concurrently in the same
  process via `tokio::spawn`-per-run. Same as M5.
- **Daemon admission** — deferred to M7 (when daemon ships). M6's
  CLI runs each invocation in-process.

## 15. Threading model

Same as M5 §16. Per-run task spawned via `tokio::spawn` (Send).
Bridge calls remain Send. `NotifyDeliverer` impls take `&self` and
share an internal `reqwest::Client` (Send + Sync).

## 16. CLI integration — full design

### 16.1 Subcommand tree

```rust
#[derive(Subcommand)]
pub enum EngineCommands {
    Run {
        spec_path: PathBuf,
        #[arg(long)] watch: bool,
        #[arg(long)] worktree: Option<PathBuf>,
    },
    Watch { run_id: String },
    Resume { run_id: String },
    Stop { run_id: String, #[arg(long)] reason: Option<String> },
    Ls,
    Logs { run_id: String, #[arg(long)] since: Option<u64>, #[arg(long)] follow: bool },
}
```

Note: no `--daemon` flag in M6. M7 adds it.

### 16.2 Dispatch flow

```rust
pub async fn run(command: EngineCommands) -> Result<()> {
    let engine = build_local_engine().await?;

    match command {
        EngineCommands::Run { spec_path, watch, worktree } => {
            let run_id = RunId::new();
            let req = build_run_request(&spec_path, run_id, worktree)?;
            let handle = engine.start_run(req.run_id, req.graph, req.worktree, req.config).await?;
            println!("{}", handle.run_id);
            if watch {
                let mut events = handle.subscribe_events();
                while let Ok(event) = events.recv().await { print_event(&event); }
                let outcome = handle.await_completion().await?;
                print_outcome(&outcome);
            }
        }
        EngineCommands::Watch { run_id } => {
            // Read events from the on-disk log, follow new ones.
            // If a live engine in this process owns the run, also
            // subscribe to its broadcast.
            let reader = open_reader_for_run(&run_id)?;
            tail_events(reader, /* follow = */ true).await?;
        }
        EngineCommands::Resume { run_id } => {
            let handle = engine.resume_run(run_id, default_worktree(&run_id)?).await?;
            // Tail until completion.
            let mut events = handle.subscribe_events();
            while let Ok(event) = events.recv().await { print_event(&event); }
        }
        EngineCommands::Stop { run_id, reason } => {
            // In-process only in M6: this can stop runs owned by the
            // current Engine instance. Returns "no live engine" if the
            // run is not active in this process.
            engine.stop_run(run_id, reason.unwrap_or_else(|| "user".into())).await?;
        }
        EngineCommands::Ls => {
            // Read run metadata from ~/.surge/runs/, format as table.
            print_runs_table(list_runs()?);
        }
        EngineCommands::Logs { run_id, since, follow } => {
            tail_events(open_reader_for_run(&run_id)?, follow).await?;
        }
    }
    Ok(())
}
```

### 16.3 Output formatting

Events print to stderr in compact form:
```
[plan_1] StageEntered (attempt 1)
[plan_1] StageCompleted → done
[loop_1] LoopIterationStarted (index 0)
[impl_1] StageEntered (attempt 1)
…
[loop_1] LoopCompleted (3 iterations, final: completed)
Terminal Completed at success_1
```

ANSI colours via `owo-colors` if stderr is a TTY.

## 17. (Reserved for M7 — daemon mode)

Section intentionally left as a placeholder to keep numbering
consistent across the M5/M6/M7/M8 spec series. M7 adds:

- `crates/surge-daemon/` binary crate with IPC over Unix socket /
  named pipe.
- `AdmissionController` (max concurrent runs, priority queue).
- `BroadcastRegistry` (multi-subscriber event fan-out).
- `EngineFacade` trait + `LocalEngineFacade` / `DaemonEngineFacade`
  impls.
- `--daemon` flag retrofit on every `surge engine ...` subcommand.
- Risk doc on M5/M6/M7 daemon-binary mixing (stale daemons reading
  v2 snapshots, etc.).

## 18. Error handling — extensions

Same baseline as M5 §14. New error categories:

- **Notify delivery errors** routed via `NotifyConfig::on_failure`.
- **Loop iteration errors** routed via `LoopConfig::on_iteration_failure`.
- **Subgraph internal failures** project to whichever outer outcome
  the matching `SubgraphConfig::outputs` entry says (typically a
  failure outcome).
- **`EdgeMaxTraversals`** — synthetic outcome routing or stage failure
  per `EdgePolicy::on_max_exceeded`.
- **`LoopItemsTooLarge`** — fail the stage with a clear message
  pointing at the cap (1000) and the resolved item count.

## 19. Testing strategy

### 19.1 Unit tests — frame mechanics

In `engine/frames.rs`:
- Push and pop LoopFrame at iteration boundaries.
- Push and pop SubgraphFrame at entry/exit.
- Frame stack survives snapshot roundtrip via JSON.

Target: ~10 unit tests.

### 19.2 Unit tests — Loop semantics

- Static iteration over `[1, 2, 3]` runs body 3 times.
- `ExitCondition::UntilOutcome` exits early.
- `ExitCondition::MaxIterations { n: 2 }` caps at 2.
- `on_iteration_failure: Skip` continues past failures.
- `on_iteration_failure: Abort` halts the loop.
- `on_iteration_failure: Retry { max: 2 }` retries before aborting.
- `EdgePolicy::max_traversals` enforced inside body.
- `gate_after_each: true` rejected at validation.
- Empty iterable produces `loop_empty` outcome and skips frame push.
- `IterableSource::Static` of size 1001 rejected at TOML load.
- `IterableSource::Artifact` resolving to size 1001 fails the stage.

Target: ~25 unit tests.

### 19.3 Unit tests — Subgraph semantics

- Simple subgraph with one inner stage projects single-output
  outcome.
- Subgraph inputs bound from outer artifacts.
- Inner Branch routing against inner edges (not outer).
- Multiple outputs with first-match semantics.
- Missing subgraph reference fails graph validation.

Target: ~12 unit tests.

### 19.4 Unit tests — Notify delivery

`MockNotifyDeliverer` records calls and returns scripted
success/failure. Verify each channel, render rules, on_failure
behaviours, undeliverable outcome contract. `compute_outcome`
exhaustive matrix.

Target: ~15 unit tests.

### 19.5 Unit tests — graph validation extensions

In `crates/surge-core/src/validation.rs`:
- Notify node missing `delivered` outcome → error.
- Notify node with `on_failure: Fail` missing `undeliverable` → warning.
- Multi-edge from same `(node, outcome)` → error with M8+ pointer.
- `gate_after_each: true` → error with M7+ pointer.
- `IterableSource::Static` len > 1000 → error.

Target: ~10 unit tests.

### 19.6 Integration tests — engine

Located in `crates/surge-orchestrator/tests/engine_m6_*.rs`. Use
`mock_acp_agent` from M3.

| Test | Coverage |
|------|----------|
| `engine_m6_static_loop` | 3-iteration static loop completes in order. |
| `engine_m6_iterable_loop` | Loop over an artifact-derived list; body reads iteration var. |
| `engine_m6_loop_max_traversals` | Loop body with `max_traversals = 2`; `Escalate` action triggers. |
| `engine_m6_loop_skip_failure` | Iteration failure with `Skip` policy continues. |
| `engine_m6_loop_retry` | Iteration failure with `Retry { max: 2 }` retries before aborting. |
| `engine_m6_subgraph_simple` | Subgraph with one inner agent stage; outputs project. |
| `engine_m6_subgraph_with_branch` | Inner Branch routing inside subgraph; outer outcome correct. |
| `engine_m6_notify_webhook` | Notify Webhook channel POSTs to local `tiny_http` server. |
| `engine_m6_resume_with_loop_frame` | Crash mid-loop, resume restores LoopFrame at correct iteration. |
| `engine_m6_resume_with_subgraph_frame` | Crash inside subgraph, resume restores SubgraphFrame. |
| `engine_m6_multi_edge_rejected` | Validation rejects a graph with `(node, outcome)` having 2 edges. |

### 19.7 CLI tests

`surge engine run --watch` end-to-end against an in-process engine.
`surge engine resume <id>` against an interrupted run. `surge engine
ls` lists runs from the on-disk store. Use `assert_cmd` for CLI
invocation.

Target: ~6 CLI tests.

## 20. Documentation requirements

- Rustdoc on every public item in: `engine::frames`,
  `engine::stage::loop_stage`, `engine::stage::subgraph_stage`,
  every public type in `surge-notify`.
- `crates/surge-notify/README.md` documenting per-channel setup,
  required secret-store keys, environment variables, and
  troubleshooting (e.g. Linux `notify-rust` requires a running
  notification daemon; Slack requires a bot token with
  `chat:write` scope; Telegram requires a bot token from
  `@BotFather` plus the chat id).
- Migration note in `docs/03-ROADMAP.md`: M5 → M6 surface changes
  (Notify outcome contract, new `EngineRunConfig::loop_iteration_timeout`,
  `#[non_exhaustive]` retrofit, MAX_LOOP_ITEMS caps).
- Cross-reference `docs/revision/03-engine.md` from the engine
  module's top-level rustdoc.

## 21. Acceptance criteria

The milestone is complete when all of the following pass:

1. `cargo build --workspace` succeeds.
2. `cargo test --workspace --lib --tests` succeeds. M5 tests
   bit-for-bit unchanged where listed in §3.1.
3. `cargo clippy --workspace --all-targets -- -D warnings` clean.
4. `cargo clippy -p surge-orchestrator -- -D clippy::pedantic
   -A clippy::module_name_repetitions
   -A clippy::missing_errors_doc -A clippy::missing_panics_doc`
   clean for new modules (`frames`, `stage::loop_stage`,
   `stage::subgraph_stage`). Allow list matches the existing
   `engine/mod.rs` module-level pragmas verbatim — no new allows.
5. `cargo clippy -p surge-notify -- -D clippy::pedantic
   -A clippy::module_name_repetitions
   -A clippy::missing_errors_doc -A clippy::missing_panics_doc`
   clean. Same allow list as the engine module for consistency.
6. Rustdoc coverage: every public item documented; `cargo doc
   --workspace --no-deps` succeeds with no warnings.
7. Integration test `engine_m6_static_loop` passes.
8. Integration test `engine_m6_iterable_loop` passes.
9. Integration test `engine_m6_loop_max_traversals` passes.
10. Integration test `engine_m6_loop_skip_failure` passes.
11. Integration test `engine_m6_loop_retry` passes.
12. Integration test `engine_m6_subgraph_simple` passes.
13. Integration test `engine_m6_subgraph_with_branch` passes.
14. Integration test `engine_m6_notify_webhook` passes.
15. Integration test `engine_m6_resume_with_loop_frame` passes.
16. Integration test `engine_m6_resume_with_subgraph_frame` passes.
17. Integration test `engine_m6_multi_edge_rejected` confirms
    multi-edge fanout still rejected with the M8+ pointer message.
18. Pure-addition guarantee: legacy `surge-orchestrator::{pipeline,
    phases, executor, parallel, planner, qa, retry, schedule}`
    bytes unchanged.
19. M5 engine API surface unchanged: every M5 caller of
    `Engine::new`, `start_run`, `resume_run`, `stop_run`,
    `resolve_human_input` compiles unchanged. Verified by retaining
    `examples/engine_in_daemon.rs` (M5 acceptance #15) byte-for-byte.
20. `EngineSnapshot` v2 deserialises both v1 and v2 blobs;
    roundtrip via JSON is bit-perfect at v2.
21. `surge engine run /tmp/flow.toml --watch` end-to-end works
    in-process; produces correct event log; exits with 0 on success.
22. `surge engine resume <id>` resumes an interrupted run from its
    snapshot.
23. `#[non_exhaustive]` retrofit applied to `EventPayload`,
    `EngineRunEvent`, `RunOutcome`, `NodeKind`. All workspace
    consumers updated to use `_ => { … }` arms; no compile errors.
24. Notify outcome validation rule lands in
    `crates/surge-core/src/validation.rs` with unit-test coverage.
25. `crates/surge-notify/README.md` ships, documents the five channels
    (Desktop / Webhook / Slack / Email / Telegram), their secret-store
    keys, and one paragraph of troubleshooting per channel.

## 22. Open questions / future work

### 22.1 Daemon mode (M7)

`crates/surge-daemon/` binary, IPC via Unix socket / named pipe,
JSON-RPC 2.0 framing, `AdmissionController` (max concurrent +
priority queue, no aging in M7), `BroadcastRegistry` (multi-subscriber
event fan-out), `EngineFacade` trait with two impls (`LocalEngineFacade`,
`DaemonEngineFacade`), `--daemon` flag retrofit on every `surge
engine ...` subcommand. Risk doc on M5/M6/M7 daemon-binary mixing
(stale daemons reading v2 snapshots, version negotiation on the
socket). Soft-preemption decision: M7 implements basic FIFO admission
without preemption; aging / interactive-yields-to-batch deferred to
M8 if real workloads need it.

### 22.2 MCP server delegation (M7)

`McpRegistry`, `RoutingToolDispatcher`, per-server crash recovery
semantics (in-flight calls return `McpServerCrashed`, engine doesn't
retry, restart applies to next call), agent-config `mcp_servers:
Vec<McpServerRef>`, sandbox interaction at session-open time. MCP
crate selection (`rmcp` vs `mcp-sdk`) — Phase 0 spike.

### 22.3 Retry, bootstrap, HumanGate channels (M8)

The earlier M5 §19 grouped these under M7; M6 splits daemon/MCP into
M7 and bumps these to M8. M8 owns:
- Stage-level retry (`AgentConfig::limits::max_retries` enforcement,
  backoff, circuit breakers).
- Bootstrap stages (description → roadmap → flow.toml).
- HumanGate delivery channels (Telegram, email, Slack interactive
  buttons).

### 22.4 `NodeKind::Parallel` for fan-out (M8+)

Per revision §0002, parallel within a run requires a new
`NodeKind::Parallel`. M8+ adds the variant to the closed enum, the
executor, the validator, and the snapshot model. Open sub-questions:

- How does Parallel interact with frames? (Likely: each branch gets
  its own frame stack.)
- Synchronisation: implicit join at downstream node, or explicit
  `NodeKind::Join`?
- Snapshot v3 with frontier vs. keep cursor + per-branch state in a
  new v3 schema?
- Per-branch budgets / failure isolation.

M6 explicitly does not pre-design this — leaving the decision to M8+
when there's a real driving use case.

### 22.5 `LoopConfig::gate_after_each` channel (M7)

`LoopConfig` doesn't carry a delivery channel for the
`gate_after_each` approval. M6 rejects `gate_after_each: true` at
validation. M7 lands the implementation alongside the daemon work:
add `LoopConfig::gate_channel: Option<NotifyChannel>`, reuse M6's
`NotifyDeliverer` for the per-iteration notification, route the
approval reply through the daemon's broadcast registry +
`Engine::resolve_human_input` (the same plumbing as a HumanGate
node, just synthesised between iterations rather than declared as
a graph node). This works without M8 HumanGate-channel adapters
because the iteration pause needs only "render and deliver a
prompt" + "receive a binary approve/abort decision" — both fit
inside the M7 notify+daemon surface. M8's bidirectional channel
adapters add richer reply parsing on top.

### 22.6 Parallel loop iterations

`LoopConfig::parallelism::Parallel` (variant doesn't yet exist in
core) is M8+. Subsumed by `NodeKind::Parallel` (§22.4) — parallel
iterations may not be a separate concept once Parallel exists.

### 22.7 `FailurePolicy::Replan`

Needs the bootstrap pipeline. M6 treats as `Abort` with a clear
diagnostic. M8 once bootstrap lands.

### 22.8 Subgraph isolation

M6's subgraphs share parent worktree. If a real use case needs
isolated worktrees per subgraph,
`SubgraphConfig::isolation: SubgraphIsolation::{Shared, NewWorktree}`
in M8+.

### 22.9 CLI `surge engine` vs `surge run` unification

Two parallel paths until legacy FSM is deprecated (target: M9?).
Then `surge run` could route to the engine and the FSM is removed.

### 22.10 Tool result content store

M5 §19.11 carried over. M8+ may add a per-run content store for
debugging `surge engine logs --include-tool-results`.

### 22.11 Streaming loop iteration (large iterables)

M6's `MAX_LOOP_ITEMS_RESOLVED = 1000` cap is conservative. Real-world
roadmap-style iteration over 10–50 milestones fits comfortably; CSV
batch processing over 10000 rows does not. Lifting the cap requires
a streaming iteration model where `LoopFrame` doesn't materialise
`items: Vec<toml::Value>` — instead it holds an iterator handle plus
a hash of the source artifact for deterministic resume. Open question
with no urgent driver. M8+ if needed.

## 23. Estimate

Solo evening pace, 11 phases, calibrated against M1/M2/M3 history
(50% over typical):

| Phase | Work | Days |
|-------|------|------|
| 0 — scaffolding | crate skeleton (surge-notify), workspace updates | 1 |
| 1 — core extensions | `#[non_exhaustive]` retrofit, new EventPayload variants, validation rule, AgentConfig untouched | 3 |
| 2 — frame mechanics | Frame, LoopFrame, SubgraphFrame; snapshot v2 + v1 reader | 3 |
| 3 — run_task extension | terminal-inside-frame branch, snapshot timing | 2 |
| 4 — routing + traversal counters | routing.rs extension, max_traversals enforcement | 2 |
| 5 — loop stage | execute_loop_entry, on_loop_iteration_done, items cap, on_iteration_failure | 4 |
| 6 — subgraph stage | execute_subgraph_entry, on_subgraph_done, output projection | 3 |
| 7 — surge-notify crate | trait + multiplexer + 5 channel impls + render | 5 |
| 8 — notify stage rewrite | execute_notify_stage with deliverer + outcome contract | 2 |
| 9 — CLI engine subtree | EngineCommands, dispatch, event printing | 3 |
| 10 — integration tests | 11 engine + 6 CLI tests | 5 |
| 11 — rustdoc + clippy + CI | rustdoc, strict clippy, CI updates | 2 |
| Buffer | discoveries, flake-fixing, secret-store integration tweaks | 4 |

Total: ~39 days raw → 5–6 weeks at 1 ratio, **6–8 weeks calibrated**
(M1/M2/M3 history). This replaces the earlier monolith estimate
(5–7 weeks raw, 7–10 calibrated) with a smaller scope at the cost
of M7 being a separate ~5–7 week milestone afterwards.

Combined M6+M7 calendar time: ~11–15 weeks. Same order as monolith
but with two ship checkpoints and snapshot recovery if life eats
weeks.

## 24. Phasing for plan

The implementation plan (next document, written via writing-plans
skill) sequences ~35–40 tasks across the 11 phases. Tasks chunk
roughly:

- Phase 0: 1 task.
- Phase 1: 4 tasks (one per core change + non_exhaustive retrofit).
- Phase 2: 3 tasks.
- Phase 3: 2 tasks.
- Phase 4: 2 tasks.
- Phase 5: 5 tasks.
- Phase 6: 3 tasks.
- Phase 7: 6 tasks (one per channel + multiplexer + render).
- Phase 8: 2 tasks.
- Phase 9: 3 tasks.
- Phase 10: 6 tasks.
- Phase 11: 2 tasks.

Total: 39 tasks. Each is 2–5 minutes of focused work modulo test
execution and external HTTP timeouts (notify integration tests).

### 24.1 Critical-path risk: Phase 1 exhaustive-match discovery

Phase 1 (surge-core amendments) blocks every subsequent phase. The
`#[non_exhaustive]` retrofit on `EventPayload`, `NodeKind`,
`EngineRunEvent`, and `RunOutcome` may surface more exhaustive
matches in legacy code than the 3-day estimate covers — every
existing pattern-match on those enums needs a `_ => { … }` arm
added. The legacy `surge-orchestrator::{pipeline, phases, executor,
parallel, planner, qa, retry, schedule}` modules are pure-addition-
protected so they shouldn't need changes — but the legacy
`surge-cli::commands::pipeline`, `surge-persistence` views, and
`surge-acp::bridge` event loops may all match exhaustively in places.

If Phase 1 discovers >10 such match sites, expect Phase 1 to slip
from 3 to 5–6 days, sliding the milestone correspondingly. The
buffer (4 days) absorbs ~2 days of slip; deeper slip eats into
later phases.

**Mitigation:** Phase 1's first task is a `cargo check --workspace`
trial-run of the retrofit on a throwaway branch. If the trial
surfaces >10 sites, raise the Phase 1 estimate before continuing,
rather than absorbing silently.

## 25. Review checklist (before locking the spec)

Items the human partner should sign off on before the plan document
is generated:

- [ ] **Scope split confirmed** (M6 = frames + Notify + CLI; M7 =
      daemon + MCP; M8 = retry + bootstrap + HumanGate channels).
- [ ] Single-threaded executor preserved (revision §0002 §Concurrency
      alignment).
- [ ] Frame-stack approach to Loop / Subgraph (vs. flattening into
      cursor).
- [ ] Items cap = 1000 (both static and resolved). Static cap in core
      validation; resolved cap in engine.
- [ ] `gate_after_each: true` rejected at validation in M6, deferred
      to M7.
- [ ] `#[non_exhaustive]` retrofit on `EventPayload`, `EngineRunEvent`,
      `RunOutcome`, `NodeKind` in Phase 1 — accept the one-time
      workspace `_ => {}` arm additions.
- [ ] Notify outcome validation rule placed in
      `surge-core::validation` (not engine-only).
- [ ] Snapshot v2 schema layout (§12.2).
- [ ] Notify channel set (Desktop/Webhook/Slack/Email/Telegram).
- [ ] CLI subcommand naming (`surge engine` subtree, no `--daemon`
      in M6).
- [ ] Subgraph isolation default = Shared (§22.8).
- [ ] Acceptance test count and shape (§19, §21).
- [ ] Calibrated estimate 6–8 weeks (vs. M5 actual ratio).

After sign-off, the implementation plan references this spec by date
and follows superpowers' writing-plans format.
