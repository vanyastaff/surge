# Legacy Pipeline Parity Checklist

> Gate document for the **Legacy pipeline retirement** milestone (see `.ai-factory/ROADMAP.md`).
>
> Every row in the parity table must be `Status: ✅ verified` before Phase 6 (`Retire Legacy Orchestrator Modules`) is allowed to delete the source file. A row in any other state blocks deletion of that module.

## Purpose

Surge currently carries two orchestration paths side-by-side:

- The **legacy spec pipeline** in `crates/surge-orchestrator/src/` (14 root modules plus `surge-spec` for I/O).
- The **graph executor** under `crates/surge-orchestrator/src/engine/`.

This checklist enumerates every legacy module, names its replacement in the engine path, points at the verifying test, and tracks the verified-or-not status. It is the parity gate referenced by Phase 6.

## Parity Table

| # | Legacy file (`crates/surge-orchestrator/src/`) | Lines | Replacement | Verifying test | Status |
|---|---|---|---|---|---|
| 1 | `pipeline.rs` | 865 | `engine/engine.rs::Engine::start_run` (new) / `Engine::resume_run` (replay) | `tests/engine_e2e_linear_pipeline.rs` | ✅ verified |
| 2 | `qa.rs` | 905 | `engine/hooks/mod.rs` (`on_outcome`) + Verifier profile | `tests/on_outcome_retry_test.rs` | ✅ verified |
| 3 | `gates.rs` | 746 | `engine/stage/human_gate.rs` | `tests/engine_human_input_resolved.rs`, `engine_human_input_timeout.rs`, `engine_human_input_unit.rs` | ✅ verified |
| 4 | `planner.rs` | 674 | `engine/bootstrap.rs` + planner profiles | `tests/bootstrap_archetypes_test.rs`, `bootstrap_linear_3_test.rs`, `bootstrap_validation_retry_test.rs` | ✅ verified |
| 5 | `executor.rs` | 338 | `engine/run_task.rs` + `engine/stage/agent.rs` | `tests/engine_agent_stage_unit.rs` | ✅ verified |
| 6 | `context.rs` | 336 | `engine/frames.rs` + `engine/stage/bindings.rs` | `tests/engine_agent_artifact_emission_test.rs` | ✅ verified |
| 7 | `retry.rs` | 310 | `engine/hooks/mod.rs` (`on_error` retry policy) | `tests/engine_m6_loop_retry.rs` | ✅ verified |
| 8 | `circuit_breaker.rs` | 301 | `engine/hooks/mod.rs` (`on_error` suppress policy) | `tests/on_error_suppress_test.rs` | ✅ verified |
| 9 | `project.rs` | 284 | Daemon-driven engine loop (`surge-daemon::server` consuming `EngineFacade`) | `crates/surge-daemon/tests/daemon_e2e_smoke.rs` | ✅ verified (daemon already on `EngineFacade`) |
| 10 | `budget.rs` | 224 | `engine/frames.rs::RunMemory` token tracking | `tests/engine_snapshot_unit.rs` | ✅ verified |
| 11 | `conflict.rs` | 185 | `engine/routing.rs::resolve_edge` | `tests/engine_m7_routing_dispatcher.rs` | ✅ verified |
| 12 | `schedule.rs` | 178 | `engine/stage/loop_stage.rs` batched iteration | `tests/engine_m6_static_loop.rs` | ✅ verified |
| 13 | `parallel.rs` | 165 | `engine/stage/loop_stage.rs` iteration | `tests/engine_m6_iterable_loop.rs` | ✅ verified |
| 14 | `phases.rs` | 29 | `engine/stage/mod.rs::NodeKind` dispatch | `tests/engine_start_run_smoke.rs` | ✅ verified |

**Totals:** 5,540 legacy lines mapped. 14/14 modules verified by an existing engine-path test before deletion. The `project.rs` row was originally expected to be verified by a Phase 6.1 daemon migration smoke; inspection during execution showed the daemon was already on `EngineFacade`, so `daemon_e2e_smoke.rs` served as the verifying test instead.

## Out-of-Tree Consumers

Modules that import or call into the legacy path from outside `surge-orchestrator/src/`:

| Caller | What it uses | Replacement plan |
|---|---|---|
| `surge-cli/src/commands/pipeline.rs` | `pipeline::Orchestrator`, `OrchestratorConfig`, `PipelineResult`, `gates::GateManager`, `phases::Phase` | Module deleted in Phase 5.2 (commands surfaces removed in favour of `surge engine *`). |
| `surge-cli/src/commands/project.rs` | `ProjectExecutor`, `ProjectConfig`, `ProjectResult` | Split: `surge project describe` keeps `surge-orchestrator::project_context` (kept alive — see Task 6.3); legacy `surge project run`-style commands removed in Phase 5.2. |
| `surge-cli/src/commands/feature.rs` | `ProjectExecutor` (legacy path) | Migrated in Phase 5.2 to drive `surge feature` against the engine; ProjectExecutor reference removed. |
| `surge-cli/src/commands/spec.rs` | `surge_spec::*` (parser, builder, graph, validation, templates) | Entire module removed in Phase 5.1. |
| `surge-cli/src/commands/mod.rs` | `load_spec_by_id` helper | Kept intentionally (now routed through `crate::legacy_spec::LegacySpecFile`); the analytics / insights / memory subcommands still resolve historical persistence data by `spec_id`. The helper retires when those queries migrate to `run_id`. |
| `surge-daemon/src/server.rs` | `pipeline::Orchestrator` | Retargeted to `Engine::run` in Phase 6.1; replacement verified by daemon smoke documented there. |
| `surge-orchestrator/tests/helpers.rs` | `surge_spec::SpecFile` | Replaced with `flow.toml` fixture loaders in Phase 4.5. |
| `surge-orchestrator/tests/e2e_pipeline.rs` | `surge_spec::DependencyGraph`, `SpecFile` | Test ported to `tests/engine_e2e_spec_parity.rs` in Phase 4.1; legacy file deleted afterward. |
| `surge-orchestrator/tests/gate_*_e2e.rs` | Legacy gate manager | Coverage already lives in `engine_human_input_*` (Phase 4.2/4.3); legacy files deleted afterward. |
| `surge-orchestrator/tests/circuit_breaker_e2e.rs` | Legacy retry + breaker | Coverage already lives in `engine_m6_loop_retry.rs` + `on_error_suppress_test.rs` (Phase 4.4); legacy file deleted afterward. |

## Phase-Exercised Telemetry

To prove no surviving test exercises the legacy path after Phase 6, Phase 1.3 instruments:

- `Orchestrator::execute` → `info!(target = "surge.path.exercised", path = "legacy", spec_id = %spec_id, task_id = %task_id, …)`
- `Engine::start_run` → `info!(target = "surge.path.exercised", path = "engine", kind = "start", run_id = %run_id, …)`
- `Engine::resume_run` → `info!(target = "surge.path.exercised", path = "engine", kind = "resume", run_id = %run_id, …)`

The final acceptance check (Task 10.4) asserts the test logs contain no `path = "legacy"` events.

## Status Legend

- ✅ **verified** — replacement is implemented and an engine-path test passes against it today.
- ⏳ **pending Phase 6.1** — replacement exists but verification is deferred to the daemon migration task in Phase 6.1.
- 🚧 **in-progress** — replacement is being added inside this milestone (transient state during execution).
- ❌ **missing** — no replacement exists; this row blocks Phase 6 and the milestone cannot proceed until resolved.

## Update Discipline

- When a Phase 4 task ports a test, update the **Verifying test** column to the engine-path file.
- When a Phase 6 task deletes a legacy module, the corresponding row must already be `✅ verified` (or `⏳ pending Phase 6.1` for `project.rs`).
- When Phase 6.1 finishes the daemon migration, flip row #9 to `✅ verified` and link the new smoke.

This file is the single source of truth for whether deletion is safe. Do not delete a legacy module without flipping its row.
