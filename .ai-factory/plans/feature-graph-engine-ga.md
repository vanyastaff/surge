# Plan: Graph Engine GA

**Branch:** `feature-graph-engine-ga`
**Created:** 2026-05-06
**Refined:** 2026-05-06 (second iteration via `/aif-improve`)
**Mode:** Full
**Source roadmap milestone:** [Graph engine GA](../ROADMAP.md)

## Settings

- **Testing:** yes — every behavioral change ships with a covering test (unit, integration, or proptest as appropriate). New tests must be deterministic and runnable in CI without networked dependencies.
- **Logging:** verbose — DEBUG for hook resolution, fold transitions, IPC frames; INFO for stage entry/exit, run lifecycle, daemon attach/detach; WARN for hook reject, retry exhaustion, schema-version downgrade attempts; ERROR for unsupported migrations and IPC framing failures. Use `tracing::*` macros only — no `println!` / `dbg!` in library crates.
- **Docs:** yes — mandatory documentation checkpoint at completion. Updates routed through `/aif-docs`.

## Roadmap Linkage

- **Milestone:** "Graph engine GA"
- **Rationale:** This plan delivers every remaining sub-bullet of the **Graph engine GA** roadmap milestone — hook execution chain, replay determinism proptest, schema-version migration chain, daemon-attached path completion, archetype example library, integration / criterion gates, and the documentation cut. NodeKind handlers, injected tools, and the M6 validation surface are already complete on `main` (see "Existing State" section); this plan finishes the remaining workstreams that gate `Graph engine GA → done`.

## Existing State (already on `main`)

Confirmed via two rounds of codebase exploration before drafting and refining this plan:

- All seven NodeKind handlers fully wired in `crates/surge-orchestrator/src/engine/stage/{agent,branch,human_gate,loop_stage,notify,subgraph_stage,terminal}.rs`.
- `BridgeCommand` (6 variants) and `BridgeEvent` (9 variants) defined in `crates/surge-acp/src/bridge/{command,event}.rs`. `BridgeCommand::ReplyToTool { session, call_id, payload, ... }` already wired with `ToolResultPayload::{Ok, Error, Unsupported}` (`crates/surge-acp/src/bridge/event.rs:197-212`).
- 33 `EventPayload` variants with `#[non_exhaustive]` and `#[serde(tag = "type", rename_all = "snake_case")]` in `crates/surge-core/src/run_event.rs`. **`OutcomeRejectedByHook` and `HookExecuted` are defined as variants but never emitted in production code today** — only in fold round-trip tests.
- `VersionedEventPayload { schema_version: u32, payload: EventPayload }` wrapper exists; `schema_version` hardcoded to `1`. The events table in `crates/surge-persistence/src/runs/migrations/per_run/0001_initial.sql` has a `schema_version INTEGER NOT NULL DEFAULT 1` column. **No payload-level migration registry exists** — bincode deserializes straight into `EventPayload`.
- `validate_for_m6()` in `crates/surge-orchestrator/src/engine/validate.rs` plus `validate()` in `crates/surge-core/src/validation.rs` cover 14+3 invariants (reachability, terminal reachability, declared-outcome resolution, profile-existence on Agent nodes, edge endpoints, single-edge-per-outcome, loop iterable bounds, subgraph cycles, MCP path safety, Notify-delivered warning). Validation is **purely syntactic** today — no `ReferenceResolver` trait, no profile-existence-in-registry check.
- `report_stage_outcome` builder produces a dynamic per-node enum from declared outcomes (`crates/surge-acp/src/bridge/tools.rs:69-100`); `request_human_input` is recognised in agent stage (`crates/surge-orchestrator/src/engine/stage/agent.rs:419`) with timeout / resolution / `HumanInputResolved` event flow.
- `engine run --watch` and `engine run --daemon --watch` flags exist in `crates/surge-cli/src/commands/engine.rs:101-184`. **`DaemonEngineFacade` is fully implemented**, not a skeleton — `connect()`, `start_run`, `resume_run`, `stop_run`, `resolve_human_input`, `list_runs`, `subscribe_to_run`, `subscribe_global`, `unsubscribe_*` are all wired and tested by `crates/surge-daemon/tests/daemon_e2e_smoke.rs` plus 11 other daemon tests. IPC framing lives in `crates/surge-orchestrator/src/engine/ipc.rs`.
- Hook **types** (`HookTrigger`, `Hook`, `MatcherSpec`, `HookFailureMode`, `HookInheritance`) are defined in `crates/surge-core/src/hooks.rs`. `HookTrigger` is **not** marked `#[non_exhaustive]` (project rule says retrofit). `Profile.role.extends: Option<ProfileKey>` exists but **inheritance resolution is explicitly deferred** to a later milestone (see `crates/surge-core/src/profile.rs:41-42`). The engine **does not invoke hooks** anywhere.
- `examples/` contains exactly two flows: `flow_terminal_only.toml` and `flow_minimal_agent.toml`. Schema observed: `schema_version = 1`, `start = "<node-key>"`, `[metadata]` table, `[nodes.<key>]` flat map, `[[edges]]` array, `Graph` derives only `#[serde(default)]` (no tags / rename_all on the top level). `Graph.subgraphs: BTreeMap<SubgraphKey, Subgraph>` is `#[serde(default)]` and lives at the root.
- Mock ACP support is the **binary** `crates/surge-acp/src/bin/mock_acp_agent.rs` driven by CLI flags / scenarios (`echo`, `report_done`, `report_outcome=K`, `crash_after=N`, `human_input`, `long_streaming`, `frozen`). There is **no** `surge_acp::testing` library module. Tests spawn the binary as a subprocess.
- Workspace has `proptest`, `criterion`, `insta` already in `[workspace.dependencies]`. **`glob` is not** — Task 1.0 below adds it.
- `crates/surge-core/benches/` exists with `fold_events.rs`, `validate_graphs.rs`, `toml_roundtrip.rs`, `bincode_roundtrip.rs` (all `harness = false`). `crates/surge-orchestrator/benches/` does **not** exist yet.
- `.github/workflows/` has `ci.yml`, `release.yml`, `security.yml`. No `CHANGELOG.md` at the repo root.

This plan does **not** touch already-working code paths except where wiring requires it (e.g., agent stage to call hook chain, persistence read site to call the migration registry).

## Research Context

> Source: roadmap milestone "Graph engine GA" plus dependent milestones that consume its primitives.

### Active Summary

- **Topic:** Graph engine GA — finalize end-to-end execution of every NodeKind through `flow.toml`.
- **Constraints:** All work flows downward through the dependency layers in `.ai-factory/ARCHITECTURE.md`. Validation rules belong in `surge-core`, executed by `surge-orchestrator`. New enum variants get `#[non_exhaustive]` retrofits where missing. `surge-core` stays leaf (no `tokio`, no I/O).
- **Cross-milestone consumers:** `Artifact format & convention library` requires the `on_outcome` reject hook as a primitive. `Bootstrap & adaptive flow generation` requires deterministic fold for replayable bootstrap stages. `Crash recovery` requires the schema-version migration chain.
- **Out of scope (deferred):** `Profile registry` (separate milestone — covers `extends` chain resolution and bundled roles), `Bootstrap & adaptive flow generation`, MCP server lifecycle hardening, Telegram cockpit, Tracker automation tiers, Crash recovery, v0.1 release.

## Tasks

> 20 tasks across 6 phases. Commit checkpoints after each phase. File paths are relative to the workspace root.

### Phase 1 — Hook execution chain

#### [x] Task 1.0 — Preflight retrofits and workspace dependencies

- **Deliverable:** Three small but project-rule-mandated edits before the rest of Phase 1:
  - Add `#[non_exhaustive]` to `HookTrigger` in `crates/surge-core/src/hooks.rs:104` (project rule: retrofit `#[non_exhaustive]` on closed enums that may grow).
  - Add `glob = "0.3"` to `[workspace.dependencies]` in the workspace `Cargo.toml`; depend on it from `crates/surge-core/Cargo.toml` for `MatcherSpec::file_glob` evaluation in Task 1.1.
  - Add `surge_core::error::SurgeError::SchemaTooOld { found: u32, min: u32 }` and `SchemaTooNew { found: u32, max: u32 }` variants (the existing example referenced these but they were never added to `crates/surge-core/src/error.rs`). Task 2.2 then uses them.
- **Files:** `crates/surge-core/src/hooks.rs`, `Cargo.toml` (workspace), `crates/surge-core/Cargo.toml`, `crates/surge-core/src/error.rs`.
- **Tests:**
  - Compile-check that all crates still build after the retrofit.
  - Unit: `crates/surge-core/src/error.rs::tests` — round-trip the two new variants through `Display`.
- **Logging:** N/A (no runtime change).
- **Notes:** This is the only task in this plan that touches workspace `Cargo.toml`; keep the diff minimal and place `glob` adjacent to other small utility crates.

#### [x] Task 1.1 — Hook resolver and executor scaffolding

- **Deliverable:** A `HookExecutor` that resolves a single profile's hooks by trigger and matcher, executes them sequentially, and returns a `HookOutcome { Proceed | Reject { reason } | Suppress { outcome } }` decision to the caller. `extends` chain resolution is **out of scope** here and deferred to the `Profile registry & bundled roles` milestone — accept a single resolved `Profile` and operate on `profile.hooks.entries` directly.
- **Files:**
  - `crates/surge-core/src/hooks.rs` — add `Hook::matches(trigger: HookTrigger, ctx: &MatchContext) -> bool` and `MatcherSpec::matches(ctx: &MatchContext) -> bool`. `MatchContext` exposes `tool_name`, `outcome`, `node`, `tool_args_json`, `file_paths_touched`. File-glob match uses `glob::Pattern::matches_path`. Pure functions — no I/O.
  - `crates/surge-core/src/profile.rs` — add `Profile::hooks_for_trigger(&self, trigger: HookTrigger) -> impl Iterator<Item = &Hook>` (no inheritance — direct iteration over `self.hooks.entries`).
  - `crates/surge-orchestrator/src/engine/hooks/mod.rs` — new module. Define `HookContext { node, session, tool, outcome, last_error, run_state }`, `HookOutcome`, `HookExecutor::execute_chain(profile: &Profile, trigger: HookTrigger, ctx: HookContext) -> HookOutcome`. Spawn each hook command via `tokio::process::Command` with `timeout_seconds`; capture stdout/stderr; map exit codes per `HookFailureMode` (Reject → `HookOutcome::Reject`; Warn → log and continue; Ignore → silent continue).
- **Tests:**
  - Unit: `crates/surge-core/src/hooks.rs::tests` — matcher coverage (file glob via `glob::Pattern`, tool exact, outcome exact, tool-arg substring on `tool_args_json`).
  - Unit: `crates/surge-orchestrator/src/engine/hooks/mod.rs::tests` — chain execution against an in-memory mock that records spawned commands; verifies short-circuit on Reject when `HookFailureMode::Reject`.
- **Logging:** DEBUG `[hooks]` on resolution (`hook.resolved=N trigger=...`), DEBUG on each hook entry (`hook.start id=... cmd=...`), DEBUG on exit with exit-status, WARN on reject (`hook.reject id=... reason=...`), ERROR on hook-process spawn failure.
- **Depends on:** 1.0.

#### [x] Task 1.2 — Wire `pre_tool_use` and `post_tool_use` into agent stage

- **Deliverable:** Tool calls in the agent turn loop are bracketed by `pre_tool_use` and `post_tool_use` hook chains. Pre rejection cancels the tool dispatch and emits a synthetic tool-error result back to the ACP session via `BridgeCommand::ReplyToTool { payload: ToolResultPayload::Error { message: <hook reason> } }`. Post rejection logs and proceeds (post-hooks cannot un-run a tool call) but still appends `OutcomeRejectedByHook` for replay/audit. Each invocation appends one `HookExecuted { hook_id, exit_status, on_failure }`.
- **Files:**
  - `crates/surge-orchestrator/src/engine/stage/agent.rs` — at the tool-dispatch site (~line 368 `session_dispatcher.dispatch()`), invoke `HookExecutor::execute_chain(profile, HookTrigger::PreToolUse, ctx)`. After dispatch, invoke `HookTrigger::PostToolUse`. On Pre-Reject, send `BridgeCommand::ReplyToTool` with `ToolResultPayload::Error` and skip the inner dispatcher; on Post-Reject, append `OutcomeRejectedByHook` and continue the loop.
  - `crates/surge-orchestrator/src/engine/hooks/mod.rs` — add `record_hook_executed(writer, ctx, hook, status)` helper to keep `agent.rs` focused.
- **Tests:**
  - Integration: `crates/surge-orchestrator/tests/hooks_pre_post_tool_test.rs` — drives the engine against the existing `mock_acp_agent` binary using the `echo` scenario; profile declares a `pre_tool_use` hook with `HookFailureMode::Reject` matching the `echo` tool name; assert the dispatcher is never reached and the agent receives a `ToolResultPayload::Error`.
  - Integration: same file — `post_tool_use` hook with `Warn` failure mode logs but does not block; assert exactly one `HookExecuted` event is appended per call.
- **Logging:** DEBUG `[hooks][pre]` + `[hooks][post]` on entry; INFO on `HookExecuted` event written; WARN on pre-reject (with tool name).
- **Depends on:** 1.1.

#### [x] Task 1.3 — `on_outcome` hook with retry-on-reject

- **Deliverable:** When an agent reports an outcome, `on_outcome` hooks run **before** `OutcomeReported` is appended. If any hook returns `Reject`, the engine appends `OutcomeRejectedByHook { node, outcome, hook_id }` (no `OutcomeReported` for the rejected attempt), surfaces the rejection to the ACP session via `BridgeCommand::ReplyToTool` against the `report_stage_outcome` call_id with `ToolResultPayload::Error { message }`, increments the attempt counter, and lets the turn loop continue so the agent can pick a different outcome. On `attempts > AgentLimits::max_retries`: append `StageFailed { reason: "on_outcome rejection budget exhausted", retry_available: false }`.
- **Files:**
  - `crates/surge-orchestrator/src/engine/stage/agent.rs` — at the outcome-validation site (~line 325), before recording `OutcomeReported`, run `HookTrigger::OnOutcome`. On reject: append `OutcomeRejectedByHook`, send the rejection back via `ReplyToTool`, do **not** mutate `RunMemory.outcomes` (rejection happens before the would-be `OutcomeReported`, so memory stays consistent without an explicit fold change). Honour `AgentConfig.limits.max_retries` (`crates/surge-core/src/agent_config.rs:87`, default 3).
- **Tests:**
  - Integration: `crates/surge-orchestrator/tests/on_outcome_retry_test.rs` — uses a custom `mock_acp_agent` scenario that reports `pass` first then `fixes_needed` on the next turn (added in Task 5.1 if missing). Profile has an `on_outcome` hook that rejects `pass` once. Assert: one `OutcomeRejectedByHook` for `pass`, one `OutcomeReported` for `fixes_needed`, run terminates per the `fixes_needed` edge.
  - Integration: same file — retries-exhausted scenario with the agent always reporting the rejected outcome; assert `StageFailed` is the terminal stage event after `max_retries + 1` rejections.
- **Logging:** INFO `[hooks][on_outcome]` on each evaluation; INFO on retry initiation with `attempt=N/max`; WARN on retry-budget exhausted with the rejecting `hook_id`.
- **Depends on:** 1.1.

#### [x] Task 1.4 — `on_error` hook wiring

- **Deliverable:** When a stage fails (tool spawn failure, ACP transport error, validation failure, internal error caught by stage), `on_error` hooks run before `StageFailed` is appended. `HookOutcome::Suppress { outcome }` converts the failure into a successful `OutcomeReported { outcome }` (the supplied outcome must be declared on the node — validated at hook-result time; otherwise the suppression is itself rejected with a WARN and the original `StageFailed` is appended). `HookOutcome::Reject` is treated as `Proceed` (cannot reject an error); `Proceed` lets `StageFailed` be recorded as before.
- **Files:**
  - `crates/surge-orchestrator/src/engine/run_task.rs` — central error-classifier: at the catch-site that produces `StageFailed`, wrap with `HookExecutor::execute_chain(..., OnError, ctx_with_error)` and branch on `HookOutcome`.
  - `crates/surge-orchestrator/src/engine/hooks/mod.rs` — add `HookContext::with_error(reason: &str)` builder.
- **Tests:**
  - Integration: `crates/surge-orchestrator/tests/on_error_suppress_test.rs` — uses `mock_acp_agent --scenario crash_after=1`; profile `on_error` hook returns `Suppress("retry_later")`; assert the run records `OutcomeReported { outcome: "retry_later" }` (which is declared on the node) instead of `StageFailed`, and routes correctly.
  - Integration: same file — without the `on_error` hook the same crash produces `StageFailed`, proving the hook genuinely changed behaviour.
  - Integration: same file — suppression with an undeclared outcome falls through to `StageFailed` and emits a WARN log.
- **Logging:** WARN `[stage][error]` on raw error captured; INFO `[hooks][on_error]` on resolution; INFO on suppression with the substituted outcome key; WARN on suppression-with-undeclared-outcome.
- **Depends on:** 1.1.

#### [x] Task 1.5 — Fold pass-through guard for hook events

- **Deliverable:** Confirm via test that `RunState::apply()` treats `HookExecuted` and `OutcomeRejectedByHook` as deterministic pass-throughs (no state mutation). `discriminant_str()` covers both. A golden insta snapshot of a synthetic event sequence containing every hook-related event guards against regressions if a future change accidentally mutates state on these events.
- **Files:**
  - `crates/surge-core/src/run_state.rs` — verify the existing exhaustive `match` in `apply()` covers both events as no-op pass-throughs (current code at ~line 391 already does this for unhandled-state events; add explicit arms with comments to make intent visible to future readers).
  - `crates/surge-core/src/run_event.rs::discriminant_str` — confirm both variants present (existing M6 code).
  - `crates/surge-core/tests/fold_hook_events.rs` (new) — synthetic 12-event sequence including `HookExecuted` and `OutcomeRejectedByHook` between `StageEntered` and `OutcomeReported`. `insta::assert_yaml_snapshot!` of the resulting `RunState` to lock the pass-through behaviour.
- **Tests:** the golden snapshot test itself.
- **Logging:** N/A (test fixtures).
- **Notes:** Per refinement: `RunMemory` has no `last_outcome` field — outcomes are stored as `outcomes: BTreeMap<NodeKey, Vec<OutcomeRecord>>`. Because `on_outcome` rejection in Task 1.3 happens **before** `OutcomeReported` is appended, no fold-side mutation of `RunMemory` is required. This task documents and enforces that invariant.
- **Depends on:** 1.3.

### Phase 2 — Determinism and schema versioning

#### [x] Task 2.1 — Replay determinism property test

- **Deliverable:** Property test asserting that for any well-formed event sequence `E`, `fold(&E[..N])` is idempotent (running twice produces the same `RunState`) and incremental fold (`apply()` step by step) equals one-shot `fold()` byte-for-byte at every prefix `N ∈ [0, |E|]`. Custom strategy generates valid graphs and event sequences honouring the engine state machine (`NotStarted → Bootstrapping → Pipeline → Terminal`).
- **Files:**
  - `crates/surge-core/tests/fold_determinism_proptest.rs` (new) — proptest strategy: build a small random graph (1–5 nodes), generate the linear sequence of events that a deterministic engine would produce (with fixed timestamps from a seeded counter, no wall-clock reads), fold and assert determinism.
  - `crates/surge-core/Cargo.toml` — `proptest` already in `[dev-dependencies]` (verified during refinement; no edit needed unless absent at implementation time).
- **Tests:** the proptest itself; `cargo test -p surge-core --test fold_determinism_proptest` passes with default 256 cases.
- **Logging:** N/A.
- **Notes:** Strategy must avoid wall-clock reads; use a deterministic timestamp generator inside the strategy.

#### [x] Task 2.2 — Schema-version migration registry

- **Deliverable:** `surge-core` exposes an `EventMigrator` infrastructure: a `MigrationChain` that maps `(schema_version, payload_bytes) → EventPayload`. Registry currently has the identity migration for v1. The migration entry point — `migrate_payload(version: u32, bytes: &[u8]) -> Result<EventPayload, SurgeError>` — is invoked by **persistence on read** (the existing bincode read path in `surge-persistence`), not from inside `VersionedEventPayload::deserialize` (which routes through serde and would create a layering issue). Unsupported versions return `SurgeError::SchemaTooOld { found, min }` or `SchemaTooNew { found, max }` (added in Task 1.0).
- **Files:**
  - `crates/surge-core/src/migrations/mod.rs` (new) — define `Migration` trait, `MigrationChain`, `migrate_payload(version: u32, bytes: &[u8]) -> Result<EventPayload, SurgeError>`. Register `IdentityV1` migration. Use `bincode` (already in workspace deps) for the v1 payload deserialization.
  - `crates/surge-core/src/lib.rs` — re-export `migrate_payload` and the chain types.
  - `crates/surge-persistence/src/runs/<event read site>` — replace direct `bincode::deserialize::<EventPayload>(bytes)` (or wherever payload bytes are turned into `EventPayload`) with `surge_core::migrate_payload(row.schema_version, &row.payload)`. Verify by file:line during implementation.
- **Tests:**
  - Unit: `crates/surge-core/tests/migrations_v1_roundtrip.rs` — round-trip a v1 payload through the chain produces an equal `EventPayload`.
  - Unit: same file — synthetic v0 payload (mocked via direct bincode encode of a valid struct then version-tag override) returns `SchemaTooOld`.
  - Unit: same file — synthetic v999 payload returns `SchemaTooNew`.
  - Integration: `crates/surge-persistence/tests/migration_payload_v1.rs` (new) — write events via `RunWriter::append_event`, read them back, assert the read path now traverses `migrate_payload` (verify via a `tracing-test` capture or a counter on a test-only migrator hook).
- **Logging:** INFO `[migrations]` on each non-identity migration applied; ERROR on unsupported version with both `found` and the supported range.
- **Notes:** Per architecture rule, `surge-core` stays leaf — `bincode` and `serde` are already permitted dependencies. No `tokio` needed in this code path. SQL DDL migrations are a separate concern handled by the existing `crates/surge-persistence/src/runs/migrations/` infrastructure and are not touched here.
- **Depends on:** 1.0.

#### [x] Task 2.3 — Validation extension: profile, template, and named-agent reference resolution

- **Deliverable:** Graph validation accepts a `ReferenceResolver` trait (resolves profile names → existing profiles, template names → registered templates, named-agent IDs → registered agents). Each `Agent` node's `profile` field is resolved; missing profiles produce `ValidationError::ProfileNotFound { node, profile }`. Validation in `surge-orchestrator` injects a real resolver backed by the project's profile registry; tests inject an in-memory resolver.
- **Files:**
  - `crates/surge-core/src/validation.rs` — add `pub trait ReferenceResolver { fn profile_exists(&self, name: &str) -> bool; fn template_exists(&self, name: &str) -> bool; fn named_agent_exists(&self, id: &str) -> bool; }`. Add `validate_with_resolver(&Graph, &dyn ReferenceResolver) -> Result<Vec<ValidationError>, Vec<ValidationError>>`. Keep `validate(&Graph)` (current line 150) as the no-resolver variant for syntactic validation. Add `ValidationError::{ProfileNotFound, TemplateNotFound, NamedAgentNotFound}` variants.
  - `crates/surge-orchestrator/src/engine/validate.rs` — extend `validate_for_m6()` to call `validate_with_resolver` when a registry resolver is available; the in-process and daemon paths inject the same resolver type. The existing terminal-only smoke path may pass a `NoOpResolver` that returns `true` for everything (so `flow_terminal_only.toml` still validates).
- **Tests:**
  - Unit: `crates/surge-core/src/validation.rs::tests` — graph with `profile = "implementer-1.0"` and a resolver that returns `false` produces `ProfileNotFound`. Resolver returning `true` removes the diagnostic.
  - Integration: `crates/surge-orchestrator/tests/validate_with_resolver.rs` — `flow_minimal_agent.toml` validates clean against a resolver that knows `implementer@1.0`; fails when the profile is removed from the resolver.
- **Logging:** DEBUG `[validate][resolver]` on each lookup; WARN on missing reference.

### Phase 3 — Daemon-attached engine path completion

> **Refinement note:** Phase 3 was rescoped after the second-iteration codebase walk found `DaemonEngineFacade`, `engine/ipc.rs` framing, server dispatch, and per-run subscription already implemented and covered by `daemon_e2e_smoke.rs`. The remaining gaps are: queued-run auto-resubscribe, global-event wire forwarding, and a parity test against the in-process facade.

#### [x] Task 3.1 — Queued-run auto-resubscribe on admission

- **Deliverable:** When `engine run --daemon` returns `DaemonResponse::StartRunQueued` (admission queue full, run waiting), the client's `subscribe_to_run` call must remain valid: when `AdmissionController` admits the queued run, the daemon emits `GlobalDaemonEvent::RunAccepted { run_id }` to all global subscribers, and the per-run broadcast channel created at admission time is automatically connected to the client's pre-existing local channel via the request-id correlation table on the client side.
- **Files:**
  - `crates/surge-daemon/src/server.rs` — at the queued-run admission site (currently in the dispatch loop around the queue-drain transition), publish a `GlobalDaemonEvent::RunAccepted` and ensure the new per-run sender is registered in the broadcast registry **before** the engine begins emitting `RunStarted`.
  - `crates/surge-orchestrator/src/engine/daemon_facade.rs` — `start_run` already creates a local channel; ensure the background read loop (lines 76–137) routes `DaemonEvent::PerRun { run_id, event }` to the local channel even if the run was queued at start time. Add a unit-level test for the routing table.
- **Tests:**
  - Integration: `crates/surge-daemon/tests/daemon_queued_subscribe_test.rs` (new) — start two runs against a daemon configured with `max_active = 1, max_queue = 4`; first run admitted, second queued. Subscribe to the queued run's events. Stop the first run (so the second is admitted). Assert the second run's `RunStarted` and subsequent events stream to the subscriber without an explicit re-subscribe.
- **Logging:** INFO `[daemon][admission]` on queued → admitted transition with run-id; DEBUG `[daemon][broadcast]` on per-run channel registration.

#### [x] Task 3.2 — Global event wire forwarding

- **Deliverable:** `GlobalDaemonEvent` values published by `surge-daemon::broadcast::BroadcastRegistry` are forwarded over the wire to clients that called `DaemonRequest::SubscribeGlobal`. Currently the broadcast registry has the events; the wire-forwarding loop is the missing link. Frame format: `DaemonEvent::Global(GlobalDaemonEvent)` (already defined in `crates/surge-orchestrator/src/engine/ipc.rs:283-317`). Backpressure: a slow subscriber that lags more than `1024` global events is dropped with a `SubscriberLagged` `DaemonResponse::Error` rather than blocking the daemon.
- **Files:**
  - `crates/surge-daemon/src/server.rs` — in the per-connection task that handles `DaemonRequest::SubscribeGlobal`, spawn a forwarder loop that reads from the broadcast `tokio::sync::broadcast::Receiver` and writes `DaemonEvent::Global` frames using `engine::ipc::write_frame`. Honour the `1024` lag limit; on lag, send the lag-error frame and close the per-connection global subscription.
  - `crates/surge-orchestrator/src/engine/daemon_facade.rs` — `subscribe_global()` already exists; verify it consumes the `DaemonEvent::Global` frames produced by the new forwarder. Update the read-loop dispatcher (lines 79-114) if the global frame routing has gaps.
- **Tests:**
  - Integration: `crates/surge-daemon/tests/daemon_global_wire_test.rs` (new) — connect, `SubscribeGlobal`, start a run via another connection (or in-process), assert the subscriber sees `RunAccepted` and `RunFinished` global events.
  - Integration: same file — slow consumer scenario simulated by holding the receiver without polling for `1100` events; assert lag eviction with `SubscriberLagged`.
- **Logging:** INFO `[daemon][subscribe-global]` on attach/detach with subscriber-id; WARN on lag eviction; DEBUG on per-event broadcast count.

#### [x] Task 3.3 — In-process / daemon parity test

- **Deliverable:** A regression-guarding parity test runs `flow_terminal_only.toml`, `flow_minimal_agent.toml`, and (after Phase 4) at least one of the new archetype examples through both `LocalEngineFacade` and `DaemonEngineFacade` and asserts the resulting event sequences are identical modulo wall-clock fields (`timestamp`, durations). Diff reported as `expected vs actual` for failures.
- **Files:**
  - `crates/surge-orchestrator/tests/daemon_parity_test.rs` (new) — helper `normalize_event(event) -> NormalizedEvent` strips wall-clock fields and any non-deterministic IDs. Assert `Vec<NormalizedEvent>` equality.
  - `crates/surge-orchestrator/src/test_helpers/<file>` — shared parity helper visible only under `#[cfg(any(test, feature = "test-helpers"))]`.
- **Tests:** the parity test itself, gated to skip when daemon binary cannot be spawned (this should never apply in CI; gating exists only for offline-laptop edge-cases).
- **Logging:** N/A (test fixtures).
- **Depends on:** 3.1, 3.2 (and Phase 4 examples for the third archetype assertion).

### Phase 4 — `flow.toml` archetype examples

> Schema confirmed via existing examples: `schema_version = 1`, `start = "<key>"`, `[metadata]`, `[nodes.<key>]` flat map, `[[edges]]` array. Subgraph-using flows declare `Graph.subgraphs: BTreeMap<SubgraphKey, Subgraph>` at the root. Profile keys use the `<role>@<version>` form (e.g. `implementer@1.0`).

#### [x] Task 4.1 — `linear-3` and `single-loop` archetypes

- **Deliverable:** Two new examples that exercise the smallest-non-trivial graphs.
  - `examples/flow_linear_3.toml` — `Spec → Implement → Verify → Terminal(success)`. Three Agent nodes wired to the bundled mock-friendly profile (`implementer@1.0` placeholder used by `flow_minimal_agent.toml`), declared outcomes `pass` and `fail`, fail-edge to `Terminal(failure)`.
  - `examples/flow_single_loop.toml` — outer `Loop` over a static three-item list; body subgraph (declared in `[subgraphs.<key>]`) `Implement → Verify`; terminal on completion.
- **Files:**
  - `examples/flow_linear_3.toml` (new).
  - `examples/flow_single_loop.toml` (new).
  - `crates/surge-cli/tests/examples_smoke.rs` (new or extend existing) — for each new flow, parse + validate + run against the `mock_acp_agent` binary subprocess (spawned with `--scenario report_done`); assert the run reaches a terminal node.
- **Tests:** smoke per archetype.
- **Logging:** N/A (config files).

#### [x] Task 4.2 — `multi-milestone-loop` and `bug-fix-with-Reproduce` archetypes

- **Deliverable:**
  - `examples/flow_multi_milestone.toml` — outer `Loop` over a 2-item milestone list; per iteration, inner `Loop` over a 2-item task list (declared via the root `subgraphs` block); inner body `Implement → Verify`; outer-loop completion → `Final Review → Terminal(success)`.
  - `examples/flow_bug_fix.toml` — `Reproduce → Implement → Verify → Terminal`. `Verify` declares outcomes `pass` (forward) and `regressed` (`kind = "backtrack"` edge to `Reproduce`). Demonstrates a `Backtrack` edge under a realistic shape.
- **Files:**
  - `examples/flow_multi_milestone.toml` (new).
  - `examples/flow_bug_fix.toml` (new).
  - `crates/surge-cli/tests/examples_smoke.rs` — extend with golden-event-log assertions for each new flow against a scripted `mock_acp_agent` invocation. Use `insta` snapshots normalised for wall-clock fields.
- **Tests:** smoke + golden-file event log per archetype.
- **Logging:** N/A.

#### [x] Task 4.3 — `refactor` and `spike` archetypes

- **Deliverable:**
  - `examples/flow_refactor.toml` — `Behavior Characterization → Implement → Verify → Reviewer → Terminal`. Demonstrates the convention that a refactor must capture behaviour first.
  - `examples/flow_spike.toml` — `Implement → Terminal`. Two-node experiment flow that explicitly skips Architect / Reviewer; declared outcome `findings_recorded` only.
- **Files:**
  - `examples/flow_refactor.toml` (new).
  - `examples/flow_spike.toml` (new).
  - `crates/surge-cli/tests/examples_smoke.rs` — parse + validate + smoke run for each.
- **Tests:** parse + smoke.
- **Logging:** N/A.

### Phase 5 — Integration and performance gates

#### [x] Task 5.1 — Mock-ACP archetype suite

- **Deliverable:** A single integration test file that loads each of the six new archetype examples (4.1, 4.2, 4.3) plus the two existing examples, drives each through the existing `mock_acp_agent` binary subprocess (`crates/surge-acp/src/bin/mock_acp_agent.rs`) with a per-archetype scripted scenario list, and asserts the event log shape is acceptable (no `StageFailed` unless explicitly expected by the archetype, every run terminates, no panics). If the existing scenarios (`echo`, `report_done`, `report_outcome=K`, `crash_after=N`, `human_input`, `long_streaming`, `frozen`) cannot drive an archetype (e.g., bug-fix needs a `regressed` outcome on the first turn then `pass` on the second), extend `mock_acp_agent.rs` with a new scenario such as `report_sequence=K1,K2,...` that emits the listed outcomes in order.
- **Files:**
  - `crates/surge-orchestrator/tests/archetypes_mock_test.rs` (new) — parameterised over `examples/*.toml`. Skip `flow_terminal_only.toml` (no agent expected) and run the rest with the mock-agent subprocess wired through the existing test pattern (`tokio::test(flavor = "multi_thread", worker_threads = 2)` per the project convention; spawn the mock binary, configure the engine to launch it as the ACP child).
  - `crates/surge-acp/src/bin/mock_acp_agent.rs` — extend `Scenario` enum with `ReportSequence(Vec<String>)` if needed by an archetype. Honour the existing CLI-flag conventions.
- **Tests:** the suite itself.
- **Logging:** INFO `[test][archetype]` per archetype start; DEBUG on event count.
- **Depends on:** 4.1, 4.2, 4.3.

#### [x] Task 5.2 — Real-ACP smoke test (gated)

- **Deliverable:** A real-agent smoke test that runs `flow_minimal_agent.toml` against an actual ACP-conformant agent binary (Claude Code or Codex CLI), gated by env vars (`SURGE_REAL_ACP_BIN` and `SURGE_REAL_ACP_PROFILE`). When the env is absent, the test is skipped with a clear message — CI green path does not require a real agent. When env is present, the test asserts the run reaches `RunCompleted` and at least one `TokensConsumed` event is recorded.
- **Files:**
  - `crates/surge-orchestrator/tests/real_acp_smoke.rs` (new).
  - `docs/development.md` — section "Optional: real-agent smoke test" describing how to opt in locally.
- **Tests:** the smoke test itself.
- **Logging:** INFO on agent binding selection.
- **Notes:** Per `decide-or-defer` rule: this test is genuine, opt-in, and complete — it is not a stub. The roadmap's "real agent" verification is satisfied by enabling this test in a developer's local environment. The deterministic CI path is covered by the `mock_acp_agent` subprocess in 5.1.

#### [x] Task 5.3 — Criterion bench: stage-transition p95 with CI regression gate

- **Deliverable:** A criterion bench measuring the transition latency `StageEntered → OutcomeReported → EdgeTraversed` for a `Branch` node (synchronous, no agent). p95 budget: documented as a numeric value derived from a baseline run (decision: take the p95 of the first clean baseline plus 25% headroom, encoded as `P95_BUDGET_US` constant in the bench source). CI runs `cargo bench --bench stage_transition -- --save-baseline ci` and a small Rust harness compares the latest run against the baseline; regression > 25% fails the job.
- **Files:**
  - `crates/surge-orchestrator/benches/stage_transition.rs` (new) — criterion bench. `harness = false` per project convention (matches the four existing benches in `crates/surge-core/benches/`).
  - `crates/surge-orchestrator/Cargo.toml` — add `[[bench]] name = "stage_transition" harness = false` entry and `criterion = { workspace = true }` to `[dev-dependencies]`.
  - `.github/workflows/ci.yml` — add a `bench` job (Linux only) that runs the bench and compares against `target/criterion/`-stored baseline. Keep the existing CI matrix (Ubuntu/Windows/macOS) untouched; the bench job runs on Linux only.
- **Tests:** the bench itself + CI gate (manual smoke locally during this task).
- **Logging:** N/A.

### Phase 6 — Documentation and GA cut

#### [x] Task 6.1 — Documentation: hooks authoring guide and archetype gallery

- **Deliverable:**
  - `docs/hooks.md` (new) — hook lifecycle (when each trigger fires), `MatcherSpec` semantics, `HookFailureMode` matrix, profile authoring example, retry semantics for `on_outcome`, suppression semantics for `on_error`, deterministic-fold guarantees. Note explicitly that `extends` chain resolution is deferred to the `Profile registry` milestone and current hooks operate on a single resolved profile.
  - `docs/archetypes.md` (new) — gallery of all six new archetype flows (linear-3, single-loop, multi-milestone, bug-fix, refactor, spike) with mermaid diagrams and a one-paragraph description of when to use each.
  - `docs/ARCHITECTURE.md` § 4 — replace the "Hooks" sentence with a link to `hooks.md`; remove the "_(intent)_" qualifier on the hook execution paragraph.
  - `docs/development.md` — link to the criterion-bench job and the optional real-ACP smoke test.
  - `docs/README.md` (if present) and root `README.md` — update navigation lists with `hooks.md` and `archetypes.md`.
  - `.ai-factory/ROADMAP.md` — flip `[ ] Graph engine GA` to `[x]` only after Task 6.2 acceptance check passes; add a row to the Completed table with the build date.
- **Files:** as above.
- **Tests:** N/A.
- **Logging:** N/A.
- **Depends on:** 1.5, 2.x, 3.x, 4.x, 5.x.

#### [x] Task 6.2 — GA acceptance check and CHANGELOG entry

- **Deliverable:** Run the full local acceptance suite, document results, and draft the changelog. Then commit and push the milestone branch.
  - `cargo build --workspace --exclude surge-ui` — clean.
  - `cargo test --workspace --exclude surge-ui` — green, including all proptest, criterion bench (smoke run, not gated), and integration tests added by this plan.
  - `cargo clippy --workspace --exclude surge-ui -- -D warnings` — clean against the project's `clippy.toml`.
  - `cargo fmt --check` — clean per `rustfmt.toml`.
  - `cargo bench --bench stage_transition -- --save-baseline ga` — establish baseline; compare against `ci` baseline if present.
  - Cross-reference each of the 14 sub-bullets of the **Graph engine GA** roadmap milestone with the artifacts produced by this plan; record a one-line evidence pointer per sub-bullet.
  - Create `CHANGELOG.md` at the repo root (currently absent) with a section `## [Unreleased] — Graph engine GA` summarising hooks, determinism, daemon path, archetypes, criterion gate, and docs additions. Use Keep-a-Changelog categories.
- **Files:** `CHANGELOG.md` (new), local-only acceptance log captured in PR description.
- **Tests:** the acceptance suite is the test.
- **Logging:** N/A.
- **Depends on:** 6.1.

## Commit Plan

| # | After phase | Suggested commit message |
|---|---|---|
| 1 | Phase 1 (1.0–1.5) | `feat(engine): wire hook execution chain (pre/post-tool, on_outcome retry, on_error suppress) + retrofits` |
| 2 | Phase 2 (2.1–2.3) | `feat(core): replay determinism proptest + schema-version migration registry + reference-resolver validation` |
| 3 | Phase 3 (3.1–3.3) | `feat(daemon): queued-run auto-resubscribe, global-event wire forwarding, parity test` |
| 4 | Phase 4 (4.1–4.3) | `feat(examples): six flow.toml archetypes (linear-3, single-loop, multi-milestone, bug-fix, refactor, spike)` |
| 5 | Phase 5 (5.1–5.3) | `test: archetype suite, gated real-ACP smoke, criterion stage-transition gate` |
| 6 | Phase 6 (6.1–6.2) | `docs: hooks guide + archetype gallery; chore(release): Graph engine GA acceptance` |

Each commit must pass `cargo build --workspace --exclude surge-ui` and `cargo test --workspace --exclude surge-ui` locally before pushing.

## Acceptance Criteria

This plan is complete when **all** of the following hold:

1. Each task above is implemented, its tests pass on Linux and Windows, and `cargo clippy --workspace -- -D warnings` is clean.
2. Every sub-bullet of the **Graph engine GA** roadmap milestone has at least one artifact produced by this plan that demonstrates it. Cross-reference table is included in the PR description.
3. `cargo bench --bench stage_transition` reports p95 stage-transition latency within the documented `P95_BUDGET_US`; the CI bench gate is wired and green.
4. `docs/hooks.md` and `docs/archetypes.md` exist, link from `README.md` and `docs/ARCHITECTURE.md`, and are reviewed via `/aif-docs`.
5. `.ai-factory/ROADMAP.md` flips `Graph engine GA` to `[x]` and the Completed table has a row with the build date.
6. `CHANGELOG.md` exists at the repo root with the `Graph engine GA` section.
7. The branch `feature/graph-engine-ga` merges cleanly into `main` with no `unwrap()` introductions in library code, no `anyhow` imports outside binary crates, and no `tokio` import added to `surge-core`.

## Notes for `/aif-implement`

- **Do not refactor** any of the seven existing NodeKind handlers beyond the minimal edits needed in tasks 1.2–1.4. The handlers are GA-quality already.
- **Daemon path is largely done.** Phase 3 is narrowly scoped to admission-time subscription continuity, global wire forwarding, and a parity test. Do not regenerate `engine/ipc.rs`, `daemon_facade.rs`, or `server.rs` from scratch — extend them.
- **`extends` chain resolution is OUT of scope.** Hook resolver in 1.1 operates on a single resolved profile. Multi-profile inheritance belongs to the `Profile registry & bundled roles` milestone.
- **Test infrastructure pattern:** mock-ACP work uses the existing `mock_acp_agent` **binary** (`crates/surge-acp/src/bin/mock_acp_agent.rs`) spawned as a subprocess with `--scenario <name>` flags. There is no `surge_acp::testing` library module today; do not invent one. Extend the binary's `Scenario` enum if archetypes need a new pattern (e.g. `ReportSequence(Vec<String>)`).
- **Test gating discipline:** new tests must be deterministic and run in CI without external services. Real-ACP smoke (5.2) is the only gated test and only via env vars.
- **Validation:** new validation rules belong in `surge-core/src/validation.rs`; the orchestrator only invokes them. This is a project-level rule. The new `ReferenceResolver` trait is the seam.
- **Migrations:** the v1-only chain is correct for now. Do not invent forward migrations; the chain mechanism is the deliverable. SQL DDL migrations are unrelated and live under `crates/surge-persistence/src/runs/migrations/`.
- **`#[non_exhaustive]` retrofits:** done in Task 1.0 for `HookTrigger`. Add to any new public enum that may grow; not needed if the enum stays internal to orchestrator (e.g. `HookOutcome`).
- **Logging discipline:** library crates use `tracing::*` only; no `println!`, `eprintln!`, or `dbg!`.
