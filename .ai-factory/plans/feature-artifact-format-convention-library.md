# Implementation Plan: Artifact Format & Convention Library

Branch: feature/artifact-format-convention-library
Created: 2026-05-11
Refined: 2026-05-11

## Settings
- Testing: yes
- Logging: verbose
- Docs: yes

## Roadmap Linkage
Milestone: "Artifact format & convention library"
Rationale: This is the next unchecked roadmap milestone and establishes the canonical artifact contracts that later roadmap amendments, legacy retirement, Telegram UX, and v0.1 schema freeze depend on.

## Scope
Surge should own the shape of role artifacts while agents own how they reason. This plan adds an explicit artifact naming/compatibility matrix, typed contracts and validators, bundled flow templates migrated from legacy spec-template intent, validation hooks that reject bad outputs, updated bundled profile prompts, conventions documentation, and regression coverage.

Non-goals:
- Do not remove `surge-spec`; that belongs to the later Legacy pipeline retirement milestone.
- Do not add new `NodeKind` variants; use profiles, hooks, templates, and existing graph validation.
- Do not make CI depend on live Claude/Codex/Gemini credentials; live cross-agent checks should be opt-in or ignored.
- Do not make `surge-core` depend on `surge-orchestrator`; core validators stay pure and orchestrator/CLI compose higher-level graph checks.

## Commit Plan
- **Commit 1** (after tasks 1-4): "feat(core): define artifact contracts and validators"
- **Commit 2** (after tasks 5-7): "feat(templates): register convention flow templates"
- **Commit 3** (after tasks 8-12): "feat(engine): enforce artifact validation hooks"
- **Commit 4** (after tasks 13-15): "docs: add artifact conventions and regression coverage"

## Tasks

### Phase 0: Contract Decisions
- [x] Task 1: Define the artifact naming and compatibility matrix.
  Deliverable: document the canonical artifact names, primary machine-readable representation, markdown compatibility surface, schema-version ownership, and validator kind for each role output: Description, Requirements, Roadmap, Spec, ADR, Story, Plan, and Flow.
  Expected behavior: implementation has a single source of truth for whether artifacts are `description.md`, `roadmap.toml` plus `roadmap.md`, `spec.toml` plus `spec.md`, `docs/adr/<NNNN>-<slug>.md`, `stories/story-NNN.md`, or `flow.toml`; downstream tasks must not invent alternate names.
  Files: `crates/surge-core/src/artifact_contract.rs`, `crates/surge-core/src/lib.rs`, `docs/conventions/README.md` placeholder if docs directory is introduced early.
  Logging requirements: no runtime logging in `surge-core`; expose stable names and diagnostic codes that callers can log at DEBUG/WARN without parsing prose.
  Dependency notes: blocks tasks 2-7 and 13 because validators, profile prompts, and docs must use the same names.

### Phase 1: Core Artifact Contracts
- [x] Task 2: Add pure artifact contract types and validation diagnostics.
  Deliverable: create `crates/surge-core/src/artifact_contract.rs` and export it from `crates/surge-core/src/lib.rs`. Define `ARTIFACT_SCHEMA_VERSION`, `ArtifactKind`, `ArtifactContractRef`, `ArtifactValidationError`, `ArtifactValidationReport`, diagnostic severity, stable diagnostic codes, and helper APIs from Task 1.
  Expected behavior: validators can report missing sections, invalid schema versions, invalid paths, unsupported artifact kind, and non-machine-readable acceptance criteria without touching I/O.
  Files: `crates/surge-core/src/artifact_contract.rs`, `crates/surge-core/src/lib.rs`, `crates/surge-core/Cargo.toml` if a workspace dependency is needed.
  Logging requirements: keep core pure; return structured diagnostics with stable codes and enough context for CLI/orchestrator callers to log at DEBUG/INFO/WARN/ERROR.
  Dependency notes: depends on task 1; foundation for tasks 3, 4, 6, 8, and 12.

- [x] Task 3: Implement pure syntax/section validators for canonical role outputs.
  Deliverable: add pure validation functions for Description Author markdown sections, Spec Author markdown compatibility shape, Architect ADR TOML frontmatter, story-file naming/content, plan markdown sections, and `flow.toml` schema-version parsing. Do not call `surge-orchestrator::engine::validate` from `surge-core`.
  Expected behavior: validation accepts minimal valid fixtures and rejects missing required sections, malformed frontmatter, invalid story paths, missing acceptance criteria, and unsupported `schema_version`.
  Files: `crates/surge-core/src/artifact_contract.rs`, `crates/surge-core/tests/artifact_contract_test.rs` or inline `#[cfg(test)]` modules.
  Logging requirements: no runtime logging in core; diagnostics must include artifact kind, field/section when known, and a short safe message suitable for hook stderr.
  Dependency notes: depends on tasks 1-2; orchestrator-level graph validation is added in task 8.

- [x] Task 4: Add typed roadmap/spec artifact wrappers without breaking existing types.
  Deliverable: add typed `RoadmapArtifact` and `SpecArtifact` wrappers with `schema_version`, conversion to/from existing `Timeline` / `Spec` where sensible, and markdown compatibility guidance. Preserve existing `Timeline`, `RoadmapItem`, and `Spec` callers.
  Expected behavior: Roadmap Planner and Spec Author can target typed structures while legacy code and tests using `Timeline` and `Spec` keep compiling unchanged.
  Files: `crates/surge-core/src/roadmap.rs`, `crates/surge-core/src/spec.rs`, `crates/surge-core/src/lib.rs`, related tests.
  Logging requirements: no runtime logging in core; parse/validation errors must include milestone/task/subtask identity so callers can log precise failures.
  Dependency notes: depends on tasks 1-2; feeds tasks 6, 7, 13, and 14.

### Phase 2: Bundled Templates And Profile Contracts
- [x] Task 5: Migrate legacy spec-template intent into bundled flow templates with aliases.
  Deliverable: add or align first-party `flow.toml` assets for `feature`, `bug-fix`, `refactor`, `performance`, `security`, `docs`, and `migration` based on the intent currently encoded in `crates/surge-spec/src/templates.rs`. Add alias resolution for legacy names: `bugfix`/`fix` -> `bug-fix`, `perf` -> `performance`, `sec` -> `security`, `doc` -> `docs`, `migrate` -> `migration`.
  Expected behavior: `surge engine run --template=<name>` resolves every canonical and legacy alias; user templates still shadow bundled templates; existing archetype names keep working.
  Files: `crates/surge-core/bundled/flows/*.toml`, `crates/surge-core/src/bundled_flows.rs`, `crates/surge-orchestrator/src/archetype_registry.rs`, relevant validation tests.
  Logging requirements: keep existing `flow::bundled` TRACE logging and `archetype::registry` DEBUG logging; include requested name, canonical name, version, and provenance when aliases resolve.
  Dependency notes: depends on task 1. Do not delete or rewrite `surge-spec`.

- [x] Task 6: Add profile-level artifact schema declarations.
  Deliverable: extend `ProfileOutcome` or profile metadata with optional artifact contract references that bind required artifacts to an `ArtifactKind`, canonical artifact name, and schema version.
  Expected behavior: bundled profiles can declare that outcome `drafted` produces `description.md` with the Description contract, `spec.md` or typed spec artifact with the Spec contract, `roadmap` with the Roadmap contract, or `adr.md` with the ADR contract; old profiles without declarations continue to parse.
  Files: `crates/surge-core/src/profile.rs`, `crates/surge-core/src/agent_config.rs` if node overrides need schema metadata, bundled profile TOML files, profile registry tests.
  Logging requirements: when contracts are loaded by orchestrator code, log resolved contract refs at DEBUG with profile id, outcome id, artifact name, artifact kind, and schema version.
  Dependency notes: depends on tasks 1-4 and supports tasks 10-12.

- [x] Task 7: Port legacy planner prompt conventions into bundled profile prompts.
  Deliverable: update the bundled Description Author, Roadmap Planner, Spec Author, Architect, Implementer, Verifier, Reviewer, and PR Composer profiles so their prompts reference the canonical artifact contracts instead of duplicating drifting format rules.
  Expected behavior: profile prompts clearly state required artifact names, required sections, schema version expectations, story-file convention, ADR path convention, and acceptance criteria format from the matrix in task 1.
  Files: `crates/surge-core/bundled/profiles/*.toml`, especially `description-author-1.0.toml`, `roadmap-planner-1.0.toml`, `spec-author-1.0.toml`, `architect-1.0.toml`, `implementer-1.0.toml`, `verifier-1.0.toml`, `reviewer-1.0.toml`, `pr-composer-1.0.toml`.
  Logging requirements: no logging inside profile TOML; profile outcomes, artifacts, and future hook ids must be named clearly so engine logs identify which contract is being enforced.
  Dependency notes: depends on tasks 1, 4, and 6.

### Phase 3: Validation Surfaces And Hook Enforcement
- [x] Task 8: Add a CLI validation surface that composes core and orchestrator checks.
  Deliverable: implement `surge artifact validate --kind <kind> <path>` with optional `--format human|json`. Use pure `surge-core` validators for artifact shape and compose orchestrator graph validation for `flow.toml` in the CLI layer, not in `surge-core`.
  Expected behavior: exit code 0 for valid artifacts; non-zero for contract failures; human output is concise enough for hook stderr; JSON output is stable for tests and future Telegram cards.
  Files: `crates/surge-cli/src/main.rs`, `crates/surge-cli/src/commands/artifact.rs`, `crates/surge-cli/src/commands/mod.rs`, `crates/surge-cli/tests/cli_artifact_validate_test.rs`.
  Logging requirements: use `tracing` DEBUG for validation start/end with kind/path; WARN for invalid artifacts with diagnostic count; do not log artifact contents or secrets.
  Dependency notes: depends on tasks 2-4 and is used by tasks 11-12.

- [x] Task 9: Preserve and integrate existing Flow Generator validation retry behavior.
  Deliverable: adapt `crates/surge-orchestrator/src/engine/bootstrap.rs` so the existing Flow Generator post-processing path can reuse the new flow artifact validator while preserving current semantics: `BootstrapEditRequested`, synthetic `validation_failed`, edit-loop cap handling, and `PipelineMaterialized` on success.
  Expected behavior: existing `bootstrap_validation_retry_test` still passes, invalid `flow.toml` still backtracks, and generic artifact validation does not create a second conflicting retry path for Flow Generator.
  Files: `crates/surge-orchestrator/src/engine/bootstrap.rs`, `crates/surge-orchestrator/tests/bootstrap_validation_retry_test.rs`, `crates/surge-orchestrator/src/engine/validate.rs` if helper extraction is needed.
  Logging requirements: keep `engine::bootstrap::validation` DEBUG/WARN logs, adding contract diagnostic codes where available; avoid dumping the full generated flow text.
  Dependency notes: depends on tasks 3 and 8; must land before task 15.

- [x] Task 10: Merge profile hooks with node-level hooks for effective agent execution.
  Deliverable: update agent-stage setup so hooks declared on resolved profiles are combined with `AgentConfig.hooks`, honoring existing hook inheritance semantics where available and preserving node-level overrides.
  Expected behavior: bundled profile `hooks.entries` actually run during `pre_tool_use`, `post_tool_use`, `on_outcome`, and `on_error`; node-level hooks still work; tests cover profile-only hooks, node-only hooks, and combined order.
  Files: `crates/surge-orchestrator/src/engine/stage/agent.rs`, `crates/surge-orchestrator/src/engine/hooks/mod.rs`, `crates/surge-orchestrator/tests/profile_registry_e2e.rs`, `crates/surge-orchestrator/tests/on_outcome_retry_test.rs`.
  Logging requirements: log effective hook count at DEBUG with node/profile id and counts split by profile vs node; log conflict/override decisions at DEBUG or WARN depending on severity.
  Dependency notes: depends on task 6; must land before task 12 because profile-level validators otherwise never run.

- [x] Task 11: Make hook execution worktree-aware and validator-command portable.
  Deliverable: extend `HookContext`, `HookExecutor`, and the production process spawner so hooks can run with the run worktree as current directory and receive safe environment variables such as `SURGE_WORKTREE`, `SURGE_NODE`, `SURGE_OUTCOME`, `SURGE_SESSION`, and a portable `SURGE_BIN` path derived from the current executable when available.
  Expected behavior: an `on_outcome` hook can validate `description.md` or `flow.toml` relative to the agent worktree on Windows, macOS, and Linux, including test/daemon environments where `surge` is not on `PATH`.
  Files: `crates/surge-orchestrator/src/engine/hooks/mod.rs`, `crates/surge-orchestrator/src/engine/stage/agent.rs`, hook tests under `crates/surge-orchestrator/tests/`.
  Logging requirements: log hook cwd/env setup at DEBUG without dumping full environment; log missing worktree/canonicalization failures at ERROR with node and hook id.
  Dependency notes: depends on task 8 and supports task 12.

- [x] Task 12: Wire `on_outcome` artifact validators into bundled profiles and retry flow.
  Deliverable: add reject-mode `on_outcome` hooks to relevant bundled profiles or profile outcomes so invalid artifacts trigger the existing retry path before `OutcomeReported` is persisted.
  Expected behavior: a malformed artifact causes `OutcomeRejectedByHook`, the agent gets another chance within `max_retries`, and exhausted retries fail the stage with the validator summary. Flow Generator keeps the specialized bootstrap retry path from task 9.
  Files: `crates/surge-core/bundled/profiles/*.toml`, `crates/surge-orchestrator/src/engine/stage/agent.rs`, `crates/surge-core/src/run_event.rs` if rejection reason needs an optional field, `crates/surge-core/src/run_state.rs`, `crates/surge-orchestrator/tests/on_outcome_retry_test.rs`.
  Logging requirements: log validation hook rejection at INFO with node/outcome/hook id/attempt and at WARN when retry budget is exhausted; keep validator stderr short and redact artifact content.
  Dependency notes: depends on tasks 6, 8, 10, and 11.

### Phase 4: Documentation, Fixtures, And Regression Coverage
- [x] Task 13: Add the conventions documentation library.
  Deliverable: create `docs/conventions/` with pages for description, requirements, roadmap, spec, story file, ADR, plan, and flow formats, plus an index linked from `docs/README.md`.
  Expected behavior: each page has a minimal valid example, a checklist, schema version notes, artifact naming from task 1, and guidance for profile authors; story files use `stories/story-NNN.md`; ADRs use `docs/adr/<NNNN>-<slug>.md` with TOML frontmatter.
  Files: `docs/conventions/README.md`, `docs/conventions/description.md`, `docs/conventions/requirements.md`, `docs/conventions/roadmap.md`, `docs/conventions/spec.md`, `docs/conventions/story.md`, `docs/conventions/adr.md`, `docs/conventions/plan.md`, `docs/conventions/flow.md`, `docs/README.md`.
  Logging requirements: docs do not log; include examples that tell implementers which runtime components log validation failures and which diagnostics users will see.
  Dependency notes: depends on tasks 1, 4, 6, and 7.

- [x] Task 14: Add golden fixtures for format equivalence and validator failures.
  Deliverable: create fixture artifacts for valid and invalid description, roadmap, spec, ADR, story, plan, and flow outputs; add golden tests that normalize agent outputs and compare contract-equivalent results independent of prose differences.
  Expected behavior: mock-agent output can be checked in CI; real Claude/Codex/Gemini equivalence tests are ignored or feature-gated unless credentials and runtimes are available.
  Files: `crates/surge-orchestrator/tests/fixtures/artifacts/`, `crates/surge-orchestrator/tests/artifact_contract_golden_test.rs`, `crates/surge-core/tests/artifact_contract_test.rs`, possibly `crates/surge-acp` mock-agent fixtures.
  Logging requirements: tests should assert validator diagnostics include stable codes; test helpers may print fixture names only on failure and must not print full artifact contents unless the fixture is explicitly synthetic.
  Dependency notes: depends on tasks 2-4 and 8; supports task 15.

- [x] Task 15: Update end-to-end validation and user-facing guidance.
  Deliverable: add integration tests for `surge engine run --template=<name>` across canonical and legacy alias templates, profile validation for all bundled profiles, validation hook retry behavior, Flow Generator retry parity, and docs references from CLI help or getting-started material where useful.
  Expected behavior: `cargo test --workspace --exclude surge-ui`, `cargo clippy --workspace --all-targets --all-features`, and `cargo fmt --check` remain green; no live external agent is required for default CI.
  Files: `crates/surge-cli/tests/examples_smoke.rs`, `crates/surge-orchestrator/tests/fixtures_validation.rs`, `crates/surge-orchestrator/tests/profile_registry_e2e.rs`, `crates/surge-orchestrator/tests/bootstrap_validation_retry_test.rs`, `docs/getting-started.md`, `docs/cli.md`.
  Logging requirements: integration tests should assert key validation failures are logged or surfaced as structured diagnostics; runtime code should use DEBUG for successful validator passes and WARN/ERROR for failures.
  Dependency notes: final verification task; depends on tasks 5, 8, 9, 10, 11, 12, and 13.

## Verification Plan
- Run `cargo fmt --all`.
- Run `cargo test -p surge-core artifact_contract`.
- Run `cargo test -p surge-cli --test cli_artifact_validate_test`.
- Run `cargo test -p surge-orchestrator --test artifact_contract_golden_test`.
- Run `cargo test -p surge-orchestrator --test on_outcome_retry_test`.
- Run `cargo test -p surge-orchestrator --test bootstrap_validation_retry_test`.
- Run `cargo test -p surge-cli --test examples_smoke`.
- Run `cargo test --workspace --exclude surge-ui`.
- Run `cargo clippy --workspace --all-targets --all-features`.
- Run `cargo fmt --check`.
- Run `git diff --check`.

## Implementation Notes
- Keep `surge-core` pure: no filesystem, network, process, `tokio`, `tracing`, or `surge-orchestrator` dependencies in validators.
- Prefer structured parsers (`toml`, `serde`, existing graph/spec types) over ad hoc string checks where a typed parser exists.
- Preserve backward compatibility for old profiles by defaulting new optional profile schema fields.
- Preserve existing Flow Generator bootstrap retry semantics unless a parity test proves the replacement identical.
- Make validator hooks portable across CLI, daemon, test binaries, and Windows runners; do not assume `surge` is available on `PATH`.
- Keep validation diagnostics short, stable, and safe to show in future Telegram cards.
- Do not remove or rewrite `surge-spec`; only copy its template intent into graph-template assets.
