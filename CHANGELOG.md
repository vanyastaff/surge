# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — Graph engine GA

### Added

- **Hook execution chain (Phase 1).** New `engine::hooks` module in
  `surge-orchestrator` exposes `HookExecutor`, `HookContext`, and `HookOutcome`.
  Hooks fire on `pre_tool_use`, `post_tool_use`, `on_outcome`, and `on_error`
  triggers. Pre-tool rejection sends a synthetic `ToolResultPayload::Error`
  reply and skips the dispatcher; `on_outcome` rejection drops the agent's
  outcome attempt and lets it retry until `AgentLimits::max_retries` is
  exhausted, then appends `StageFailed`; `on_error` suppression converts a
  stage failure into a declared outcome via a JSON stdout directive.
- **Schema-version migration registry (`surge-core::migrations`).**
  `migrate_payload(version, bytes) -> Result<EventPayload, SurgeError>` is
  invoked by `surge-persistence` on every read; `MigrationChain` currently
  holds the v1 identity migration. `SchemaTooOld` / `SchemaTooNew` typed
  errors surface unsupported versions.
- **`ReferenceResolver` validation extension.** `surge-core` exposes a new
  `ReferenceResolver` trait and `validate_with_resolver` entry point;
  `surge-orchestrator` adds `validate_for_m6_with_resolver` which surfaces
  `ProfileNotFound`, `TemplateNotFound`, and `NamedAgentNotFound` errors.
  The terminal-only smoke path keeps using the no-resolver `validate_for_m6`.
- **Replay-determinism property test.** `surge-core/tests/fold_determinism_proptest.rs`
  asserts `fold` is idempotent and that incremental `apply()` matches one-shot
  `fold()` byte-for-byte at every prefix.
- **Six new `flow.toml` archetype examples** under `examples/`:
  `flow_linear_3.toml`, `flow_single_loop.toml`, `flow_multi_milestone.toml`,
  `flow_bug_fix.toml`, `flow_refactor.toml`, `flow_spike.toml`. All validate
  through `validate_for_m6_with_resolver` against the bundled
  `implementer@1.0` profile placeholder.
- **Documentation:** [`docs/hooks.md`](docs/hooks.md) (lifecycle, matcher,
  failure-mode matrix, suppression directive, profile authoring example) and
  [`docs/archetypes.md`](docs/archetypes.md) (gallery with mermaid diagrams
  for every archetype). Both linked from the root README and `docs/README.md`.
  `docs/ARCHITECTURE.md` § 4 cross-links the Hooks section.
- **Criterion bench:** `crates/surge-orchestrator/benches/stage_transition.rs`
  measures `StageEntered → OutcomeReported → EdgeTraversed` for a synchronous
  Branch node. `P95_BUDGET_US` constant carries the per-transition wall-clock
  budget. CI gates the bench via a Linux-only `bench` job that builds the
  bench and runs `cargo bench -- --quick` so a runtime panic in the bench
  cannot land silently. Full GA baseline is local:
  `cargo bench -p surge-orchestrator --bench stage_transition -- --save-baseline ga`.
- **Gated real-ACP smoke test:**
  `crates/surge-orchestrator/tests/real_acp_smoke.rs` opts in via
  `SURGE_REAL_ACP_BIN` and `SURGE_REAL_ACP_PROFILE` env vars; without them
  the test prints a `SKIPPED` banner and exits successfully. See
  [`docs/development.md`](docs/development.md#optional-real-agent-smoke-test).
- **Daemon-attached engine path completion (Phase 3).**
  `BroadcastRegistry::subscribe_eventual` parks a oneshot waiter that is
  resolved atomically by the next `register(run_id)` call — closing the
  race where `spawn_forward_task` could push events before a queued
  subscriber attached. The IPC `Subscribe` handler now uses this path
  when the run is in the FIFO admission queue, so `subscribe_to_run`
  remains valid across the queued→admitted transition without an
  explicit re-subscribe. Covered by
  `crates/surge-daemon/tests/daemon_queued_subscribe_test.rs`.
- **Engine-facade parity test.**
  `crates/surge-daemon/tests/daemon_parity_test.rs` runs
  `flow_terminal_only.toml` through both `LocalEngineFacade` and
  `DaemonEngineFacade`, normalises wall-clock fields, and asserts the
  two event sequences are identical.
- **Mock-bridge archetype smoke suite.**
  `crates/surge-orchestrator/tests/archetypes_mock_test.rs` boots the
  engine against `fixtures::mock_bridge::MockBridge` for every bundled
  `examples/flow_*.toml` archetype; the terminal-only flow is run to
  completion and the rest are asserted to start cleanly. Complements
  `crates/surge-cli/tests/examples_smoke.rs` (parser+validator) and
  `tests/real_acp_smoke.rs` (gated real-agent path).

### Fixed

- **`EngineRunEvent::Terminal` IPC serialisation.** The variant was a
  tuple `Terminal(RunOutcome)` while both `EngineRunEvent` and
  `RunOutcome` use `#[serde(tag = "kind")]`. Internally-tagged tuple
  variants flatten the inner object's fields into the outer enum,
  which produced two `kind` fields on the wire and tripped
  `serde_json` with `duplicate field "kind"` on read. Changed to
  `Terminal { outcome: RunOutcome }` (struct variant) so the inner
  enum's tag nests cleanly. All callers (orchestrator engine, daemon
  facade, CLI watch loop, daemon inbox state-sync, integration tests)
  updated to the new field-style pattern.

- **Replay determinism violation in `RunState::apply`.** The proptest above
  uncovered that `apply()` generated `SessionId::new()` on `RunStarted` and
  inside `advance_bootstrap_stage`, violating the project rule
  "no random IDs introduced inside a fold". Replaced with the new
  `SessionId::nil()` deterministic placeholder so replay produces identical
  state byte-for-byte.

### Changed

- **`HookTrigger` is now `#[non_exhaustive]`** to honour the project rule
  for closed enums that may grow.
- **`MatcherSpec::file_glob`** now matches via `glob::Pattern::matches_path`
  (M1 stub used substring-match).

### Migration notes

- New workspace dependency: `glob = "0.3"` (used by
  `MatcherSpec::file_glob` evaluation).
- New `SurgeError` variants: `SchemaTooOld { found, min }` and
  `SchemaTooNew { found, max }`. The error enum is `#[derive(thiserror::Error)]`,
  so existing consumers compile unchanged.
- Persistence event reader now selects `schema_version` from the events
  table; existing rows continue to round-trip through the v1 identity
  migration.
