# Plan: Legacy Pipeline Retirement

**Branch:** `intelligent-hugle-061097` (existing worktree — no new branch)
**Created:** 2026-05-13
**Plan file:** `.ai-factory/plans/legacy-pipeline-retirement.md`

## Settings

- **Testing:** yes — port every legacy e2e to a graph-executor equivalent before deletion; add unit + e2e tests for `surge migrate-spec`.
- **Logging:** verbose — `tracing::{trace, debug, info, warn, error}`; new instrumentation goes in via `info_span!` / `tracing::instrument`. No `println!` / `dbg!` in library crates.
- **Docs:** yes — mandatory docs checkpoint at completion; remove surge-spec from every documentation page that mentions it.
- **Estimate calibration:** +50% buffer on raw estimates (project rule).
- **Half-implementations:** forbidden — every task is decide-or-defer; defers are written into the plan as explicit out-of-scope notes.

## Roadmap Linkage

- **Milestone:** `Legacy pipeline retirement`
- **Rationale:** Direct execution of the milestone bulleted in `.ai-factory/ROADMAP.md:119-137`. Closes the legacy/engine duality before the v0.1 freeze.

## Scope Summary

In:

- Delete `crates/surge-spec/` (6 source files, ~56 KB) and remove it from the workspace graph.
- Delete 14 legacy modules in `crates/surge-orchestrator/src/` (~5,540 lines) plus their `lib.rs` exports.
- Remove `surge spec` subcommand tree (`create`, `list`, `show`, `validate`) and the spec-keyed top-level shortcuts (`surge run`, `status`, `logs`, `plan`, `skip`, `diff`, `merge`, `discard`).
- Retarget `surge-daemon` and remaining `surge-cli` surfaces to the graph engine.
- Implement `surge migrate-spec <path>` translator (linear cases deterministic; ambiguous cases surfaced as commented TODOs).
- Write `docs/migrate-spec-to-flow.md` migration guide; replace `docs/conventions/spec.md` with a deprecation banner + historical detail collapsed under `<details>`.
- Port the 4 legacy orchestrator e2e tests onto the graph executor; strip `surge_spec` from test helpers.
- Cleanup 13 docs files with surge-spec mentions.
- Final acceptance: `cargo tree | Select-String surge-spec` empty; `cargo build --workspace` clean; `cargo clippy --workspace -- -D warnings` clean; smoke `surge init --default && surge project describe && surge engine run examples/flow_minimal_agent.toml`.

Out:

- Renaming `surge engine run` → `surge run` (cosmetic; ship in a follow-up).
- Bumping `schema_version` on `flow.toml`, events, or registries.
- Performance tuning of the engine.
- New engine features (this milestone is pure retirement + migration).
- Retrofitting `#[non_exhaustive]` beyond surge-spec scope (separate cleanup track per recent commits).

## Phases

### Phase 1 — Parity Verification & Deprecation

#### Task 1.1: Parity Checklist Document ✅

- **Deliverable:** `docs/legacy-parity-checklist.md` — 14-row table: legacy file → engine replacement → location → verifying test → status.
- **Files (create):** `docs/legacy-parity-checklist.md`.
- **Source mapping (from exploration):**
  - `pipeline.rs` (865) → `engine/engine.rs::Engine::start_run` + `Engine::resume_run` — verified by `engine_e2e_linear_pipeline.rs`.
  - `qa.rs` (905) → `engine/hooks/mod.rs::on_outcome` + Verifier profile — verified by `on_outcome_retry_test.rs`.
  - `gates.rs` (746) → `engine/stage/human_gate.rs` — verified by `engine_human_input_*.rs`.
  - `planner.rs` (674) → `engine/bootstrap.rs` + planner profiles — verified by `bootstrap_*.rs`.
  - `executor.rs` (338) → `engine/run_task.rs` + `engine/stage/agent.rs` — verified by `engine_agent_stage_unit.rs`.
  - `context.rs` (336) → `engine/frames.rs` + `engine/stage/bindings.rs` — verified by `engine_agent_artifact_emission_test.rs`.
  - `retry.rs` (310) → `engine/hooks/mod.rs::on_error` (retry policy) — verified by `engine_m6_loop_retry.rs`.
  - `circuit_breaker.rs` (301) → `engine/hooks/mod.rs::on_error` (suppress policy) — verified by `on_error_suppress_test.rs`.
  - `project.rs` (284) → daemon-driven engine loop — verified by ad-hoc daemon smoke (Task 6.1).
  - `budget.rs` (224) → `engine/frames.rs::RunMemory` token tracking — verified by `engine_snapshot_unit.rs`.
  - `conflict.rs` (185) → `engine/routing.rs::resolve_edge` — verified by `engine_m7_routing_dispatcher.rs`.
  - `schedule.rs` (178) → `engine/stage/loop_stage.rs` batching — verified by `engine_m6_static_loop.rs`.
  - `parallel.rs` (165) → `engine/stage/loop_stage.rs` iteration — verified by `engine_m6_iterable_loop.rs`.
  - `phases.rs` (29) → `engine/stage/mod.rs::NodeKind` dispatch — verified by `engine_start_run_smoke.rs`.
- **Logging:** n/a (docs).
- **Acceptance:** every legacy module has a named replacement and a test reference that already passes. Any row without `Status: ✅` blocks Phase 6.

#### Task 1.2: Add `#[deprecated]` to surge-spec public API ✅

- **Deliverable:** every `pub` item in `crates/surge-spec/src/{lib,parser,builder,graph,validation,templates}.rs` carries `#[deprecated(since = "0.1.0-pre", note = "use surge-orchestrator::engine + flow.toml; surge-spec is retiring")]`.
- **Files (modify):** all of `crates/surge-spec/src/*.rs`.
- **Constraints:**
  - Decorate items, not modules — attribute order matters for re-exports.
  - Keep internal helpers untouched (no compiler noise).
- **Logging:** n/a.
- **Acceptance:** `cargo build -p surge-cli -p surge-orchestrator` succeeds; deprecation warnings count > 0 in both crates; no errors.

#### Task 1.3: Path-Exercised Telemetry ✅

- **Deliverable:** structured `tracing::info!` event at every legacy entry point and at the engine entry point, with field `path = "legacy" | "engine"` under target `surge.path.exercised`. Used to assert in CI that no surviving test exercises legacy after Phase 6.
- **Files (modify):**
  - `crates/surge-orchestrator/src/pipeline.rs::Orchestrator::execute` — start of fn.
  - `crates/surge-orchestrator/src/engine/engine.rs::Engine::start_run` and `Engine::resume_run` — start of each fn.
- **Logging:** `info!(target: "surge.path.exercised", path = "legacy", spec_id, task_id, "entered legacy pipeline path")` and the engine variants (with `kind = "start"` or `kind = "resume"`). No metrics crate dependency.
- **Acceptance:** running any flow under `RUST_LOG=surge.path.exercised=info` emits exactly one path event per run.

**Phase exit:** parity checklist all-green; deprecation warnings visible; telemetry observable.

---

### Phase 2 — `surge migrate-spec` CLI

#### Task 2.1: Spec→Flow Mapping Rules

- **Deliverable:** `crates/surge-cli/src/commands/migrate_spec/mapping.rs` — pure functions converting `surge_spec::SpecFile` → `toml_edit::Document` shaped as flow.toml.
- **Mapping rules (decided):**
  - `SpecFile.title` → `[meta].title`.
  - `SpecFile.description` → `[meta].description`.
  - `SpecFile.subtasks[]` → one `[[nodes]]` entry per subtask with `kind = "Agent"`, `id = "<subtask_id>"`, `profile = "<subtask.profile or 'implementer'>"`.
  - `Subtask.depends_on[]` → emit one `[[edges]]` `from = <dep_id>, to = <subtask_id>, outcome = "success"` per dependency.
  - `Subtask.acceptance_criteria[]` → embedded as `node.success_criteria = [...]` array.
  - Always append a synthetic `Terminal` node (`id = "done"`) wired from every leaf subtask.
- **Ambiguous cases (surface, do not silently lose):**
  - Non-linear dependency graphs with merge/diamond → emit `# TODO[migrate-spec]: non-linear deps detected; verify edge fan-in is intended` next to affected edges.
  - Subtasks with no profile → choose `implementer` + `# TODO[migrate-spec]: profile defaulted; pick a more specific role if needed`.
  - Cyclic deps → return error (already invalid in surge-spec; preserve invariant).
- **Files (create):** `crates/surge-cli/src/commands/migrate_spec/{mod,mapping}.rs`.
- **Logging:** `debug!(subtask = %id, "mapped subtask")` per subtask; `warn!(subtask = %id, reason = ?, "ambiguous mapping")` per fallback.
- **Acceptance:** module compiles standalone with `cargo test -p surge-cli migrate_spec::mapping::tests` ≥ 6 cases (single, linear chain, fan-out, fan-in, diamond, no-profile).

#### Task 2.2: `surge migrate-spec <path>` Subcommand

- **Deliverable:** clap subcommand `surge migrate-spec <input.spec.toml> [--output <path>] [--allow-warnings]`.
- **Behavior (decided):**
  - Default: read input via `surge_spec::SpecFile::load` (the last surviving use of surge-spec, deleted in Phase 7).
  - Apply mapping (Task 2.1).
  - Write flow.toml to stdout (or `--output <path>`).
  - Exit code: `0` clean; `2` produced with warnings (unless `--allow-warnings`); `1` on parse/IO failure.
- **Files (create):** `crates/surge-cli/src/commands/migrate_spec/handler.rs`.
- **Files (modify):**
  - `crates/surge-cli/src/main.rs` — register variant `Commands::MigrateSpec(MigrateSpecArgs)` in the clap derive tree.
  - `crates/surge-cli/src/commands/mod.rs` — export `pub mod migrate_spec;`.
- **Logging:** `info!(input = %path, output = ?dst, "migrate-spec starting")`, `debug!(...)` per step, `info!(warnings = n, "migrate-spec finished")`.
- **Acceptance:** `surge migrate-spec --help` shows new command; running against the fixtures in Task 2.3 produces valid flow.toml that loads via `surge-orchestrator` flow loader.

#### Task 2.3: Tests for migrate-spec

- **Deliverable:**
  - Unit tests for mapping rules in `migrate_spec/mapping.rs` (6+ cases per Task 2.1).
  - Integration test `crates/surge-cli/tests/migrate_spec_e2e.rs` running the binary handler against fixture files and asserting flow.toml parses + matches snapshot via `insta`.
- **Fixtures (create):** `crates/surge-cli/tests/fixtures/legacy/{linear_chain,fan_out,diamond,no_profile,single}.spec.toml` — minimum 5 fixtures.
- **Files (create):** `crates/surge-cli/tests/migrate_spec_e2e.rs`, `crates/surge-cli/tests/fixtures/legacy/*.spec.toml`, `crates/surge-cli/tests/snapshots/...` (insta).
- **Logging:** test-only via `tracing-test`.
- **Acceptance:** `cargo test -p surge-cli migrate_spec` green; `cargo insta accept` runs clean once and snapshots are committed.

**Phase exit:** `surge migrate-spec` exists, tested, and emits flow.toml that loads through the engine.

---

### Phase 3 — Migration Guide Documentation

#### Task 3.1: `docs/migrate-spec-to-flow.md`

- **Deliverable:** Standalone guide.
- **Sections:**
  1. Why migrate (declarative graph vs. imperative spec; one execution path going forward).
  2. Quick start: `surge migrate-spec old.spec.toml > new.flow.toml`.
  3. Mapping reference table (mirrors Task 2.1 rules; one row per legacy field).
  4. Manual edit guidance for ambiguous cases (non-linear deps, profile fallbacks).
  5. After migration: `surge engine run new.flow.toml`.
- **Files (create):** `docs/migrate-spec-to-flow.md`.
- **Links from:** `docs/cli.md`, `docs/README.md`, `docs/conventions/spec.md` deprecation banner.
- **Logging:** n/a.
- **Acceptance:** all referenced files link back to the guide; the guide compiles without broken links (manual review).

#### Task 3.2: Deprecation Banner on `docs/conventions/spec.md`

- **Deliverable:** keep file (preserve backlinks); prepend deprecation banner; collapse original content under `<details><summary>Historical reference</summary>…</details>`.
- **Files (modify):** `docs/conventions/spec.md`.
- **Banner text:** `> **DEPRECATED.** The structured-spec format is retiring. Use [`flow.toml`](./flow.md) for new work and [`migrate-spec-to-flow.md`](../migrate-spec-to-flow.md) to convert existing `.spec.toml` files.`
- **Logging:** n/a.
- **Acceptance:** banner present at top; original content preserved inside `<details>`.

**Phase exit:** migration story is documented before code disappears.

---

### Phase 4 — Port Legacy Tests to Graph Executor

#### Task 4.1: Port `e2e_pipeline.rs` → `engine_e2e_spec_parity.rs`

- **Deliverable:** new test that exercises the same scenario as the legacy file via flow.toml + mock ACP agent.
- **Files:**
  - delete (after port) `crates/surge-orchestrator/tests/e2e_pipeline.rs`.
  - create `crates/surge-orchestrator/tests/engine_e2e_spec_parity.rs`.
- **Approach:** load `examples/flow_linear_3.toml`, drive via mock agent, assert same artifacts + RunCompleted outcome that legacy expected.
- **Logging:** `debug` per stage transition.
- **Acceptance:** runs in default CI lane (not `#[ignore]`) when mock agent suffices; falls back to `#[ignore]` only if it needs a real CLI agent — explicitly noted in test comment.

#### Task 4.2: Port `gate_approval_e2e.rs` → engine HumanGate test

- **Deliverable:** verify approve/reject paths through `engine/stage/human_gate.rs`.
- **Files:**
  - delete `crates/surge-orchestrator/tests/gate_approval_e2e.rs`.
  - extend `engine_human_input_*.rs` if coverage matches; otherwise create `engine_human_gate_approval.rs`.
- **Logging:** `debug` per approval lifecycle event.
- **Acceptance:** equivalent approval/rejection coverage; failure modes (timeout, double-decide) covered.

#### Task 4.3: Port `gate_persistence_e2e.rs` → engine resume test

- **Deliverable:** verify gate state survives engine restart via snapshot/replay.
- **Files:**
  - inspect existing `engine_m6_resume_*.rs` and `engine_resume_after_crash.rs` for coverage parity.
  - if covered → just delete `gate_persistence_e2e.rs`.
  - if a gap remains → extend an existing resume test rather than add a new file.
- **Logging:** `debug` for snapshot/replay phases.
- **Acceptance:** persistence cycle verified by an engine-path test before legacy file is deleted.

#### Task 4.4: Port `circuit_breaker_e2e.rs` → `on_error` hook test

- **Deliverable:** verify retry/suppress directives via `on_error` hook chain.
- **Files:**
  - inspect existing `on_error_suppress_test.rs` and `on_outcome_retry_test.rs` for parity.
  - delete `circuit_breaker_e2e.rs`.
  - extend the on_error tests if gaps remain (e.g., max-attempts cap, exponential backoff coverage).
- **Logging:** `debug` per retry decision.
- **Acceptance:** retry + breaker semantics verified through engine path.

#### Task 4.5: Strip `surge_spec` from `tests/helpers.rs`

- **Deliverable:** `crates/surge-orchestrator/tests/helpers.rs` does not import from `surge_spec`.
- **Files:** `crates/surge-orchestrator/tests/helpers.rs`.
- **Approach:** replace `SpecFile`-based helpers with `flow.toml` fixture loaders.
- **Logging:** test-only.
- **Acceptance:** `Select-String "surge_spec" crates/surge-orchestrator/tests/` empty.

**Phase exit:** every legacy test deleted, with an engine-side equivalent verified to pass.

---

### Phase 5 — Retire CLI Surfaces

#### Task 5.1: Remove `surge spec` Subcommand Tree

- **Deliverable:** `surge spec ...` no longer exists in the CLI; `SpecCommands` enum and module gone.
- **Files:**
  - delete `crates/surge-cli/src/commands/spec.rs`.
  - modify `crates/surge-cli/src/commands/mod.rs` — drop `pub mod spec;` and remove `load_spec_by_id()` + any spec-loading helpers.
  - modify `crates/surge-cli/src/main.rs` — remove `SpecCommands` derive, remove the match arm dispatching to `commands::spec::run` (~line 491).
- **Logging:** n/a.
- **Acceptance:** `cargo run -p surge-cli -- spec` → clap "no such subcommand"; `cargo build -p surge-cli` clean.

#### Task 5.2: Remove / Retarget Spec-Keyed Top-Level Commands

- **Decision (decide-or-defer):** REMOVE `surge run`, `surge status`, `surge logs`, `surge plan`, `surge skip`, `surge diff`, `surge merge`, `surge discard`. Engine equivalents (`surge engine run|status|logs|stop|resume`) already cover the supported behaviours. Renaming `surge engine` → `surge` is deferred to a follow-up release note.
- **Files:**
  - modify `crates/surge-cli/src/main.rs` — remove the affected `Commands::*` variants and their dispatch arms.
  - delete `crates/surge-cli/src/commands/pipeline.rs`.
  - delete `crates/surge-cli/src/commands/project.rs` only after splitting out anything still needed by `surge project describe` (which uses `surge-orchestrator::project_context` — keep that path alive in its own command file if it lives here today).
  - audit `crates/surge-cli/src/commands/feature.rs` — `surge feature` is the post-amendment milestone surface; preserve it and migrate its internals off `ProjectExecutor` if it still uses it.
- **Decision (decide-or-defer):** if a removed command has no engine equivalent (e.g., `surge plan`), drop it with a CHANGELOG note in the commit message. The engine exposes equivalent semantics via flow.toml planning nodes.
- **Logging:** n/a.
- **Acceptance:** `surge --help` lists only `init`, `project`, `profile`, `engine`, `feature`, `doctor`, `mcp`, `migrate-spec` (and any preserved subgroup); `cargo build -p surge-cli` clean.

#### Task 5.3: Update CLI Integration Tests

- **Deliverable:** CLI integration tests stop invoking removed commands.
- **Files:** audit `crates/surge-cli/tests/*.rs`; remove or migrate tests that reference removed subcommands.
- **Logging:** test-only.
- **Acceptance:** `cargo test -p surge-cli` green.

**Phase exit:** the only CLI surface that still touches surge-spec is the soon-to-be-deleted `surge migrate-spec` shim (which is intentional — it depends on surge-spec for one last read).

---

### Phase 6 — Retire Legacy Orchestrator Modules

#### Task 6.1: Migrate `surge-daemon` Server to Engine

- **Deliverable:** `surge-daemon/src/server.rs` runs flows via `Engine::run`, not `pipeline::Orchestrator::execute`.
- **Files:**
  - `crates/surge-daemon/src/server.rs` — swap entry point.
  - any daemon endpoints accepting `spec_id` → take a flow descriptor (path or registered template name) and a `run_id`.
- **Decision (decide-or-defer):** preserve daemon socket protocol on its current schema_version; do not bump in this milestone — backwards-compat is deferred until v0.1 schema freeze.
- **Logging:** `info!` on daemon startup with engine config; `debug!` per event observed.
- **Acceptance:** daemon starts; `surge engine run --daemon examples/flow_minimal_agent.toml` succeeds end-to-end against the running daemon.

#### Task 6.2: Delete Legacy Modules from surge-orchestrator

- **Deliverable:** 14 modules removed.
- **Files (delete):** `crates/surge-orchestrator/src/{pipeline,planner,executor,gates,qa,retry,circuit_breaker,parallel,schedule,phases,budget,context,project,conflict}.rs`.
- **Files (modify):** `crates/surge-orchestrator/src/lib.rs` — drop module declarations and `pub use` re-exports for the deleted modules; preserve `project_context`, `roadmap_amendment`, `roadmap_document`, `roadmap_target`, `prompt`, `archetype_registry`, `bootstrap_driver`, `profile_loader`, and `engine` (full module tree).
- **Pre-flight gate:** Task 1.1 parity checklist all-green AND Task 6.1 daemon migration confirmed clean — otherwise abort.
- **Logging:** n/a (removal).
- **Acceptance:** `cargo build -p surge-orchestrator --all-targets` clean; `cargo test -p surge-orchestrator` green; `Select-String "use surge_spec" crates/surge-orchestrator/src/` empty.

#### Task 6.3: Verify `project_context` Survives Clean

- **Deliverable:** `crates/surge-orchestrator/src/project_context.rs` does not import surge-spec; `surge project describe` still works.
- **Files:** `crates/surge-orchestrator/src/project_context.rs` (modify if needed).
- **Logging:** preserve existing `tracing` instrumentation.
- **Acceptance:** `cargo run -p surge-cli -- project describe` in a temp dir succeeds; output `project.md` matches snapshot.

**Phase exit:** orchestrator carries only engine-path code plus the deliberately preserved root-level surfaces.

---

### Phase 7 — Delete `surge-spec` Crate

#### Task 7.1: Remove Cargo Wiring

- **Deliverable:** surge-spec gone from the workspace graph.
- **Files (modify):**
  - root `Cargo.toml` — remove `"crates/surge-spec"` from `[workspace] members` (line ~7) and `surge-spec = { ... }` from `[workspace.dependencies]` (line ~137).
  - `crates/surge-cli/Cargo.toml` — remove `surge-spec = { workspace = true }` (only `migrate-spec` referenced it; that command is being rewritten in Task 7.2 to read TOML directly).
  - `crates/surge-orchestrator/Cargo.toml` — remove `surge-spec = { workspace = true }`.
- **Logging:** n/a.
- **Acceptance:** `cargo metadata --format-version 1` does not list `surge-spec`.

#### Task 7.2: Rewrite `migrate-spec` to Read TOML Directly

- **Deliverable:** `surge migrate-spec` no longer depends on the `surge-spec` crate — parses the input TOML into local DTOs in `migrate_spec/dto.rs` and applies the mapping.
- **Files:**
  - create `crates/surge-cli/src/commands/migrate_spec/dto.rs` — minimal `serde::Deserialize` types matching the legacy spec.toml schema (one-shot definition; no need to share with the deleted crate).
  - modify `crates/surge-cli/src/commands/migrate_spec/handler.rs` — replace `SpecFile::load` with `toml::from_str` into `dto::LegacySpecFile`.
- **Decision (decide-or-defer):** snapshot the DTO schema in tests so future legacy spec.toml files keep parsing; defer translating any new spec extensions (we do not expect any after retirement).
- **Logging:** preserve existing per-step logs.
- **Acceptance:** `cargo build -p surge-cli` clean without `surge-spec`; all `migrate_spec` tests still green.

#### Task 7.3: Delete `crates/surge-spec/` Directory

- **Deliverable:** directory gone.
- **Files:** delete `crates/surge-spec/` entirely.
- **Logging:** n/a.
- **Acceptance:** `Test-Path crates/surge-spec` → False; `cargo tree --workspace | Select-String surge-spec` empty.

**Phase exit:** the crate is gone; the workspace builds clean; `migrate-spec` keeps working off local DTOs.

---

### Phase 8 — Restructure Remaining Helpers

#### Task 8.1: Audit Surviving Root Modules in surge-orchestrator

- **Deliverable:** decision per file documented inline in `lib.rs` (one-line `//` comment per module declaration).
- **Approach:** list `crates/surge-orchestrator/src/*.rs` after Phase 6 and for each module decide:
  - belongs under `engine/` (only the engine consumes it) → mark for move in Task 8.2.
  - stays at root (cross-cutting surface like `project_context`, `roadmap_*`, `bootstrap_driver`, `archetype_registry`, `prompt`) → annotate why.
- **Files:** `crates/surge-orchestrator/src/lib.rs`.
- **Logging:** n/a (structural).
- **Acceptance:** `lib.rs` enumerates surviving modules with one-line rationale.

#### Task 8.2: Collapse Engine-Only Helpers into `engine/`

- **Deliverable:** modules marked in Task 8.1 moved under `engine/`.
- **Files:** moves per Task 8.1 decisions. Update all imports.
- **Decision (decide-or-defer):** do not invent new sub-modules for borderline files; either it clearly belongs in `engine/` or it stays at root.
- **Logging:** n/a (refactor).
- **Acceptance:** `cargo build --workspace` clean; `cargo test --workspace` green; no orphan root-level modules unused by tests or binaries (`cargo udeps` optional sanity check).

**Phase exit:** orchestrator layout matches the engine-first model documented in `.ai-factory/ARCHITECTURE.md`.

---

### Phase 9 — Documentation Cleanup

#### Task 9.1: Update `docs/ARCHITECTURE.md`

- **Deliverable:** zero surge-spec mentions; "Legacy spec pipeline" section deleted.
- **Files:** `docs/ARCHITECTURE.md`.
- **Decision (decide-or-defer):** preserve a single short paragraph titled "Retired in v0.1: structured-spec pipeline" pointing at `docs/migrate-spec-to-flow.md` — useful for readers landing from older commits; do not preserve any other historical detail.
- **Acceptance:** `Select-String "surge-spec\b|legacy spec pipeline|spec\.toml" docs/ARCHITECTURE.md` returns only the new retirement note.

#### Task 9.2: Update CLI / Workflow / Bootstrap / Getting-Started Docs

- **Files (modify):**
  - `docs/cli.md` (lines 16, 32, 38, 75) — drop "manage legacy specs"; remove the legacy-pipeline row from the execution-surfaces table; remove `surge spec` examples.
  - `docs/workflow.md` (line 32) — drop the "Legacy spec pipeline" row.
  - `docs/bootstrap.md` (lines 5, 54) — rewrite spec→flow references to mention flow.toml directly.
  - `docs/getting-started.md` (line 94) — replace `surge artifact validate --kind spec spec.toml` with the flow.toml equivalent.
  - `docs/README.md` — update CLI section to remove links to the deprecated spec conventions.
- **Acceptance:** `Select-String -Pattern "surge[ -]spec|spec\.toml|surge spec\b" docs/` returns only the deprecation banner in `docs/conventions/spec.md`, the migration guide, and the `legacy-parity-checklist.md` (intentional).

#### Task 9.3: Update Project Context Files

- **Files (modify):**
  - `.ai-factory/DESCRIPTION.md` (lines 76, 78) — remove the `surge-spec` row from the crate-layout table; update the `surge-orchestrator` description to drop "legacy spec pipeline".
  - `.ai-factory/ARCHITECTURE.md` (lines 49, 52) — same.
  - `CLAUDE.md` (lines 26, 35) — remove `surge spec` / `surge run` from the commands list; refresh the design-decision note that mentions specs being TOML.
  - root `README.md` (line 21) — update the surge-orchestrator description.
- **Acceptance:** `Select-String "surge-spec\b|\bsurge spec\b" .ai-factory/*.md CLAUDE.md README.md` empty.

#### Task 9.4: Mark Milestone Complete in ROADMAP

- **Deliverable:** roadmap reflects completion.
- **Files:** `.ai-factory/ROADMAP.md` (lines 119, 270).
- **Approach:** flip `[ ]` → `[x]` on the milestone heading; append a row to the "Completed" table with the actual completion date.
- **Acceptance:** ROADMAP renders 15 completed / 6 remaining.

**Phase exit:** every documentation surface that mentioned surge-spec either points to the migration guide or no longer mentions it.

---

### Phase 10 — Final Acceptance

#### Task 10.1: Clean Workspace Verification

- **Commands (PowerShell):**
  - `cargo build --workspace --all-targets`
  - `cargo test --workspace --no-fail-fast`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- **Acceptance:** all green; zero deprecation warnings (deprecated items deleted); zero unused-import warnings.

#### Task 10.2: Dependency Graph Verification

- **Commands:** `cargo tree --workspace | Select-String surge-spec` — must return nothing.
- **Acceptance:** empty.

#### Task 10.3: End-to-End Smoke

- **Commands (in a fresh temp dir, RUST_LOG=surge=debug):**
  - `cargo run -p surge-cli -- init --default`
  - `cargo run -p surge-cli -- project describe`
  - `cargo run -p surge-cli -- engine run examples/flow_minimal_agent.toml`
- **Acceptance:** all three succeed; resulting `project.md` and run completion logs look normal.

#### Task 10.4: Path-Exercised Telemetry Assertion

- **Deliverable:** a CI assertion (or local script documented in `docs/legacy-parity-checklist.md`) that searches the workspace test logs for `path = "legacy"` events and fails if any are observed.
- **Acceptance:** running `cargo test --workspace -- --nocapture | Select-String 'path.exercised' | Select-String 'legacy'` returns nothing.

**Phase exit:** milestone is complete; commit + push.

---

## Commit Plan

Checkpoints follow phase boundaries; each commit is independently buildable.

- **C1** (after Phase 1): `feat(orchestrator): deprecate surge-spec API + add path-exercised telemetry`
- **C2** (after Phase 2): `feat(cli): add surge migrate-spec command`
- **C3** (after Phase 3): `docs: spec→flow migration guide; deprecation banner on conventions/spec.md`
- **C4** (after Phase 4): `test(orchestrator): port legacy e2e tests onto graph executor`
- **C5** (after Phase 5): `refactor(cli): remove surge spec subcommands and spec-keyed shortcuts`
- **C6** (after Phase 6): `refactor(orchestrator): retire legacy pipeline modules; daemon uses Engine`
- **C7** (after Phase 7): `chore: remove surge-spec crate from workspace; migrate-spec reads TOML directly`
- **C8** (after Phase 8): `refactor(orchestrator): collapse engine-only helpers under engine/`
- **C9** (after Phase 9): `docs: drop surge-spec mentions across documentation`
- **C10** (after Phase 10): `chore: mark legacy pipeline retirement milestone complete`

---

## Estimates (with +50% buffer)

| Phase | Raw | Calibrated |
|---|---|---|
| 1 — Parity + deprecation | 1 d | 1.5 d |
| 2 — migrate-spec CLI | 2 d | 3 d |
| 3 — Migration guide docs | 0.5 d | 1 d |
| 4 — Port legacy tests | 2 d | 3 d |
| 5 — Retire CLI surfaces | 1.5 d | 2.5 d |
| 6 — Retire orchestrator modules | 2 d | 3 d |
| 7 — Delete surge-spec crate | 0.5 d | 1 d |
| 8 — Restructure helpers | 1 d | 1.5 d |
| 9 — Docs cleanup | 0.5 d | 1 d |
| 10 — Final acceptance | 0.5 d | 1 d |

**Total: ~18.5 days of focused work** (≈3-4 weeks at normal cadence).

---

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Engine doesn't cover a legacy behaviour | Phase 1 parity checklist is a hard gate before Phase 6 deletes anything. |
| `surge migrate-spec` mis-translates real specs | Scope is deterministic linear cases + comment-marked TODOs for ambiguous; insta snapshots keep behaviour stable. |
| Daemon regression after switching to engine | Task 6.1 lands before Task 6.2; legacy modules stay in tree until daemon migration test passes. |
| Legacy tests deleted before equivalents land | Phase 4 fully completes before Phase 6; each Phase-4 task pairs delete-with-port. |
| Hidden non-engine consumers of legacy modules | `cargo build --workspace` runs at every phase boundary; `Select-String surge_spec` audits at Phase 6 and Phase 9. |
| Engine path-exercised counter regression in CI | Task 10.4 wires the assertion into the verification commands; failure surfaces immediately. |

---

## Next Steps

After review:

```
/aif-implement
```

To view tasks during implementation:

```
/tasks
```

Use `/aif-plan --list` and `/aif-plan --cleanup <branch>` only after finishing the milestone if the worktree is no longer needed.
