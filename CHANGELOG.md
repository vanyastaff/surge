# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — Graph engine GA (in progress)

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

### Fixed

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
