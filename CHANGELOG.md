# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed — Artifact contract acceptance-criteria validation

- **Stricter acceptance-criteria checks** — Spec (Markdown and TOML) and Story
  artifacts now reject placeholder, empty-checkbox, or too-short acceptance
  criteria (e.g. `TBD`, `- [ ]`, `?`). The new
  `empty_acceptance_criteria` diagnostic code points to the offending criterion
  by index (`Acceptance Criteria[N]` for Markdown,
  `spec.subtasks[i].acceptance_criteria[j]` for TOML). Each criterion must be a
  non-placeholder string of at least 8 characters after trimming.

### Added — Bootstrap & adaptive flow generation

- **`surge bootstrap` CLI** — runs the bundled Description Author →
  Roadmap Planner → Flow Generator bootstrap graph from a free-form prompt,
  supports console approve/edit/reject gates, resumes completed bootstrap
  runs via `surge bootstrap resume <run_id>`, and starts the materialized
  follow-up graph with inherited bootstrap artifacts.
- **Bundled flow registry and templates** — `surge-core::BundledFlows`
  embeds `bootstrap`, `linear-3`, `linear-with-review`, `multi-milestone`,
  `bug-fix`, `refactor`, `spike`, and `single-task`; `surge engine run
  --template <name>` resolves bundled or `${SURGE_HOME}/templates/*.toml`
  user templates and skips bootstrap.
- **Bootstrap runtime semantics** — bootstrap HumanGates emit structured
  approval/edit events, `EdgeKind::Backtrack` re-enters authoring agents
  with fresh `edit_feedback`, Flow Generator output retries on parse or
  graph-validation failures, and edit-loop caps emit `EscalationRequested`
  before failing clearly.
- **Content-addressed bootstrap artifacts** — agent-produced artifacts are
  copied into the per-run artifact store by content hash, and follow-up runs
  can inherit `description`, `roadmap`, and `flow` artifacts from their
  bootstrap parent via `EngineRunConfig::bootstrap_parent`.
- **Bootstrap telemetry and e2e coverage** — successful bootstrap runs append
  `BootstrapTelemetry` with stage durations, edit counts, and archetype
  metadata; mock-agent tests cover `linear-3`, `multi-milestone`,
  `bug-fix`, `refactor`, `spike`, validation retry, and edit-loop cap paths.
- **Documentation** — added [`docs/bootstrap.md`](docs/bootstrap.md) and
  refreshed workflow, CLI, getting-started, and archetype docs for bootstrap
  and template-skip usage.

### Added — Profile registry & bundled roles

- **`surge-core::profile::registry`** — pure inheritance + merge resolver.
  `ResolvedProfile`, `Provenance` (`Versioned` / `Latest` / `Bundled`),
  `merge_chain`, `merge_pair`, and a `collect_chain` walker with
  cycle detection and `MAX_EXTENDS_DEPTH = 8` guard. Shallow merge
  semantics codified per field shape: `default_mcp` / `default_skills` /
  `default_shell_allowlist` (each replaced when child non-empty),
  `bindings.expected` (merged by `name` field), `hooks.entries` (union
  dedup by `Hook::id` with WARN on collision), `prompt.system`
  (child wins when non-empty), `inspector_ui.fields`, `sandbox`,
  `approvals`. 32 unit tests + 8 property/snapshot tests.
- **`surge-core::profile::bundled`** — `BundledRegistry` with 17
  first-party profiles compiled in via `include_str!`: 3 bootstrap
  (Description Author, Roadmap Planner, Flow Generator), 7 execution
  (Spec Author, Architect, Implementer, Test Author, Verifier,
  Reviewer, PR Composer), 4 specialized via `extends`
  (Bug-Fix / Refactor Implementer, Security Reviewer, Migration
  Implementer), 2 project-level (Project Context Author, Feature
  Planner), and `mock@1.0` for tests.
- **`surge-core::profile::keyref`** — `ProfileKeyRef { name, version }`
  parser for `name@MAJOR.MINOR[.PATCH]` references. Partial versions
  zero-fill; double `@` and unparseable versions reject.
- **`surge-core` `RuntimeCfg::agent_id`** — new `String` field with
  `#[serde(default = "default_agent_id")]` returning `"claude-code"`.
  Identifies the agent runtime to launch via `surge_acp::Registry`
  lookup; replaces the M5 string-based fallback.
- **`surge-orchestrator::profile_loader`** — disk-touching half of the
  registry. `surge_home()` / `profiles_dir()` honour `SURGE_HOME` (fall
  back to `dirs::home_dir().join(".surge")`); `DiskProfileSet::scan`
  walks `*.toml` flat and warn-and-skips per-file parse failures;
  `ProfileRegistry::{load, resolve, list}` does the canonical
  versioned → latest → bundled 3-way lookup with version match
  against `Profile.role.version` in the TOML body.
- **`surge-orchestrator::prompt::PromptRenderer`** — Handlebars 6
  wrapper with strict mode (used at `ProfileRegistry::load` to fail
  loudly on broken templates) and lenient mode (used at agent stage
  execution to forgive missing optional bindings). HTML escaping
  disabled. `validate_template` is the load-time fail-fast hook.
- **`Engine::new_full`** constructor + `EngineConfig::profile_registry:
  Option<Arc<ProfileRegistry>>` field. Legacy constructors keep
  working with `profile_registry = None` (mock-only fallback);
  production wiring (CLI / daemon) calls `ProfileRegistry::load()` at
  startup.
- **`AgentStageParams::profile_registry`** — agent stages resolve
  `agent_config.profile` through it to derive `AgentKind` from the
  merged profile's `runtime.agent_id`. The M5 `if profile_str ==
  "mock"` fallback at `agent.rs:126-137` is gone — `mock@1.0` is now
  a bundled profile resolved through the registry like everything
  else. Unknown agent ids surface as `StageError::Internal` rather
  than silently degrading to mock.
- **`surge profile` CLI** — four new subcommands:
  - `surge profile list [--format json]`
  - `surge profile show <name> [--version X.Y.Z] [--raw]`
  - `surge profile validate <path>`
  - `surge profile new <name> [--base BASE]`
- **Documentation.** [`docs/profile-authoring.md`](docs/profile-authoring.md)
  is the new authoring guide (schema, inheritance, Handlebars,
  outcomes, sandbox/approvals/hooks, versioning, CLI).
  [ADR 0001](docs/adr/0001-profile-registry-layout.md) records the
  layout decisions; [ADR 0002](docs/adr/0002-profile-trust-deferred.md)
  defers signature/trust to post-v0.1.

### Changed — Profile registry & bundled roles

- `SurgeError` is now `#[non_exhaustive]` and gains the registry
  error family: `ProfileNotFound`, `ProfileVersionMismatch`,
  `ProfileExtendsCycle`, `ProfileExtendsTooDeep`, `ProfileFieldConflict`,
  `InvalidProfileKey`.
- `surge-orchestrator::engine::stage::bindings::substitute_template` is
  now `#[deprecated]` in favour of `PromptRenderer`. Kept around for
  the two legacy unit tests; new code routes through Handlebars.
- `docs/ARCHITECTURE.md` § 6 (Profiles and roles) and
  `.ai-factory/ARCHITECTURE.md` folder map updated to reflect the
  three-crate layering split.

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
  bench and runs
  `SURGE_STAGE_TRANSITION_BUDGET_CHECK=1 cargo bench -- --quick --save-baseline ci`
  so budget regressions and runtime panics cannot land silently. Full GA
  baseline is local:
  `cargo bench -p surge-orchestrator --bench stage_transition -- --save-baseline ga`.
- **Gated real-ACP smoke test:**
  `crates/surge-orchestrator/tests/real_acp_smoke.rs` opts in via
  `SURGE_REAL_ACP_BIN` and `SURGE_REAL_ACP_PROFILE` env vars, runs
  `examples/flow_minimal_agent.toml` through the engine against the real
  ACP child, and asserts both `RunCompleted` and at least one
  `TokensConsumed` event. Optional `SURGE_REAL_ACP_KIND` and
  `SURGE_REAL_ACP_ARGS` override launch inference for custom agents. Without
  the required env vars the test prints a `SKIPPED` banner and exits
  successfully. See
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
- **StopRun on a queued run leaked `BroadcastRegistry::waiters` entries**
  introduced by `subscribe_eventual`. When a client called
  `Subscribe(run_id)` while the run was still in the FIFO admission
  queue and then `StopRun(run_id)` cancelled it before admission, the
  parked oneshot sender stayed in `waiters[run_id]` forever and the
  per-connection `forward_queued_to_client` task hung indefinitely.
  `StopRun`'s queued branch now calls `broadcast.deregister(run_id)`
  to drop the parked senders — each waiter wakes with
  `Err(RecvError::Closed)` and exits cleanly. Regression test:
  `daemon_queued_subscribe_test.rs::subscribe_to_queued_then_stop_does_not_leak_waiter`.
- **Hook executor pipe-buffer deadlock + child-leak on timeout.** The
  `spawn_via_shell` helper called `child.wait().await` and only THEN
  drained stdout/stderr. With piped output, a hook that emitted more
  than the pipe buffer (≈64 KiB Linux, ≈4 KiB Windows) blocked on
  write and never exited, hanging the engine forever in the await.
  And on timeout the future was dropped without killing the child,
  leaving the hook process orphaned. Switched to
  `child.wait_with_output()` (concurrent stdout/stderr drain + wait)
  combined with `Command::kill_on_drop(true)` so the timeout path
  genuinely terminates the hook. Caught in PR #48 review.
- **`on_error` hooks were skipping the `HookExecuted` audit trail.**
  `run_on_error_hooks` discarded `HookOutcome::executed()` and only
  returned the resolved suppression key, contradicting the
  "every hook invocation appends `HookExecuted`" rule honoured by
  the pre/post_tool_use and on_outcome chains. The helper now
  returns `OnErrorResolution { outcome, records }`, and the engine
  call site in `run_task::execute` persists each record via
  `record_hook_executed` before consuming the outcome. Unit tests
  in `run_task::tests` updated to verify the audit invariant.
  Caught in PR #48 review.
- **`validate_for_m6_with_resolver` silently dropped non-resolver
  errors.** The orchestrator-level resolver validator filtered
  `surge_core::validate_with_resolver` findings to only the
  `Profile/Template/NamedAgent NotFound` kinds, ignoring every other
  `Severity::Error` finding (single-edge-per-outcome, reachability,
  terminal reachability, loop iterable, etc.). Graphs that passed
  the engine's own structural checks but violated `surge-core`'s
  broader rules were accepted as valid. The validator now propagates
  every `Severity::Error` finding, prefixing each with
  `[ref]` (resolver) or `[structural]` so callers can tell them
  apart. Caught in PR #48 review and uncovered an unreachable
  `failure` terminal in `examples/flow_linear_3.toml` — fail-edge
  `kind` switched from `escalate` to `forward` so the terminal is
  reachable via forward traversal (semantics unchanged, the
  `escalate` kind is for parallel error-handler flows, not normal
  fail-outcome routing).
- **`StorageError::MigrationFailed` distinguishes schema-migration
  failures from raw I/O / pool faults.** The persistence read path
  was mapping `migrate_payload` errors onto `StorageError::Pool`,
  which surfaced as a misleading "pool error" even though the pool
  was healthy. Added a dedicated variant so log filters, dashboards,
  and tests can discriminate without pattern-matching error message
  strings. Caught in PR #48 review.

### Changed

- **`real_acp_smoke.rs` promoted from env-contract scaffold to real
  opt-in driver.** The test now mutates the minimal-agent example with a
  smoke-specific prompt, swaps the engine's mock fallback for the selected
  real ACP launch kind, waits for run completion, and verifies the persisted
  event log contains `RunCompleted` plus `TokensConsumed`.

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
