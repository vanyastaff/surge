---
status: accepted
deciders: vanyastaff
date: 2026-05-07
supersedes: none
---

# ADR 0003 — Bootstrap-to-pipeline transition strategy

## Context

The "Bootstrap & adaptive flow generation" milestone introduces a three-stage Description → Roadmap → Flow agent sequence with a HumanGate after each stage. The output of the last stage is a validated `flow.toml` (a typed `Graph` value) that **becomes the pipeline that actually does the work**.

The bootstrap is itself a graph: three Agent nodes interleaved with three HumanGate nodes. The question is how the bootstrap graph "hands off" to the materialized graph it produced.

Three options were considered:

1. **Single mega-graph.** Bootstrap nodes and the materialized pipeline are both nodes in one graph. The graph generator emits a partial extension that the engine splices in mid-run.
2. **In-process graph swap.** The engine emits a second `PipelineMaterialized` event mid-run and the run loop transitions onto the new graph.
3. **Two-graph model with a follow-up run.** Bootstrap is its own complete `Graph`. Its `Terminal` node emits `PipelineMaterialized { graph, graph_hash }`. The driver (CLI or daemon) detects this event after the bootstrap run completes and starts a **new run** with the materialized graph, linking the two via `EngineRunConfig.bootstrap_parent: Option<RunId>`.

## Decision

We pick **(3) two-graph follow-up run**.

- The bootstrap graph is bundled as `crates/surge-core/bundled/flows/bootstrap-1.0.toml` and loaded via `BundledFlows` (mirror of `BundledRegistry` from M7).
- Flow Generator's terminal outcome (`approve`) emits `PipelineMaterialized { graph, graph_hash }` once, in the same write batch as the bootstrap's `RunCompleted`.
- The follow-up run is started by `surge-cli` (or the daemon) reading the most recent `PipelineMaterialized` event from the bootstrap run's event log and calling `Engine::start_run` with `bootstrap_parent: Some(<bootstrap_run_id>)`.
- The follow-up run pre-populates `RunMemory.artifacts` from the parent's content-addressed `ArtifactStore` (Decision 8 in the implementation plan): `description.md`, `roadmap.md`, `flow.toml`.

## Consequences

**What this preserves:**

- The M6 invariant that `RunStarted` and `PipelineMaterialized` are emitted **atomically** in one write batch (`crates/surge-orchestrator/src/engine/engine.rs:202-215`). Each run has exactly one `PipelineMaterialized`.
- Replay determinism: `fold(events[..N])` produces identical state byte-for-byte at every `seq` for both the bootstrap run and the follow-up run, independently. No mid-run graph-swap state to migrate.
- Fork semantics from any seq in either run remain straightforward — there is no "implicit graph swap point" to special-case.

**What this rules out:**

- A single `surge run` log file covering both phases. Operators see two run IDs (linked via the parent reference). The Telegram cockpit, replay UI, and analytics treat them as related-but-distinct runs.

**What this enables:**

- The "outer milestone Loop wrapping inner task Loop" pattern in the materialized graph composes naturally: each milestone iteration is just another run-internal `Loop` body subgraph traversal — no architectural concession needed for bootstrap.
- The `--template=<name>` skip path (Decision 7 in the plan) is symmetric: pre-baked templates skip the bootstrap run entirely; explicit `flow.toml` paths likewise. Both reach `Engine::start_run` directly.

## Alternatives Rejected

**(1) Single mega-graph** is rejected because:

- Graph validation (`validate_for_m6`) runs at run start. A graph that contains both bootstrap stages **and** placeholders for the materialized pipeline cannot validate the pipeline portion — it doesn't exist yet.
- The materialized portion is shaped by the agent's output. Splicing requires either a "wildcard" node kind (rejected by the closed-`NodeKind` invariant per `.ai-factory/ARCHITECTURE.md` § Key Principles 1) or a special-case engine path that erodes the principle "engine is dumb, agents are smart".

**(2) In-process graph swap** is rejected because:

- Mid-run `PipelineMaterialized` re-emission would either need fold-time graph replacement (breaks "events are append-only, folds are deterministic") or a marker event (`PipelineSwapped`) that adds a new variant to the closed event taxonomy for one specific use case.
- Crash recovery semantics get hairy: which graph does the recovered fold use, the original or the post-swap one? The follow-up-run model sidesteps this entirely — each run has one graph from start to terminal.
- Replay UI scrubbers would have to render two graphs in one timeline.

## Implementation Footprint

- **Core (no I/O):** add `EngineRunConfig.bootstrap_parent: Option<RunId>` (`surge-orchestrator/src/engine/run_config.rs` is the existing home — confirmed via `EngineRunConfig` struct at lines 41-73).
- **Orchestrator:** add `bootstrap_driver` module exposing `run_bootstrap_in_worktree(engine, prompt, run_id, worktree_path) -> MaterializedRun` so callers pass the isolated run worktree explicitly (per Task 19 of the implementation plan).
- **Persistence:** `ArtifactStore::open(parent_run_id, hash)` reads parent artifacts during follow-up run start (Task 20).
- **CLI:** `surge bootstrap "<prompt>"` calls `run_bootstrap`, then immediately calls `engine::run` with `bootstrap_parent: Some(...)` (Task 13).

## Out of Scope

- Daemon-mode bootstrap (Telegram approvals, async resume) — handled by future `Telegram cockpit` and `Crash recovery` milestones; the events emitted here (`BootstrapApprovalRequested`, `BootstrapEditRequested`) are the contract those milestones consume.
- Roadmap amendments mutating an active follow-up run — handled by the future `Roadmap amendments via surge feature` milestone.
