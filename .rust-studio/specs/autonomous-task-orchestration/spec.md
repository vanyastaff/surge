# Spec: Autonomous task orchestration v1

- **Status:** Approved
- **Slug:** `autonomous-task-orchestration`   ·   **Date:** `2026-09-18`   ·   **Owner:** `chief-architect`
- **Governing ADR:** `docs/adr/0020-project-queue-in-registry-tasks-are-runs.md` (to be written via `/adr` on approval)

## Problem

Carried over from `intent.md`. A user runs `surge bootstrap "простая игра"`, approves the
description, the roadmap and one flow — and from that moment the whole project runs through
**one** flow chosen once for the whole run. A bug that arrives from GitHub during the run lands
in a run of its own through `surge-intake`, but nothing places it into the roadmap or decides
when it runs relative to the planned tasks; the static loop node walks milestones in file order.
Every agent node in every flow is the same bundled profile with the same skill set regardless
of the task, and a profile written for this project has nowhere to live inside the repo — it
goes to `SURGE_HOME` and does not travel with the code. Changing priorities means editing
`roadmap.toml` by hand and restarting.

**Fixed looks like** (intent): after approving the plan the user watches a task list execute on
its own: each task gets its own flow built from the project's agents, external tasks from
GitHub/MCP/user appear in the same list in the right place, and the user can pause,
reprioritise or edit a flow without stopping the run. The agents and flows the system composed
are files under `.surge/` in the repo.

## Goals / Non-goals

**Goals**
- G-A A project-local `.surge/` layer for profiles, flows and skills, with precedence
  project → `SURGE_HOME` → bundled, loaded per run (not engine-wide), trusted on first use.
- G-B One dependency-aware, priority-ordered task queue over `.surge/roadmap.toml`, dispatched
  by the daemon, one ordinary run per task, editable while running.
- G-C External tasks (GitHub / Linear / MCP / user) enter the queue only through the
  feature-planner → roadmap-patch path, never directly.
- G-D A **flow catalog**: each task runs a template chosen by fit from `.surge/flows/` (then
  home, then bundled archetypes); the planner creates a new template into the catalog only
  when none fits. Human gates are `human_gate` nodes inside the template, placed by the
  project's autonomy level. The sealed-verifier rule is an *error* for task flows.
- G-E The planner may compose new profiles into `.surge/profiles/_generated/`, behind the
  ADR-0015 gate; per-node prompt / skill / MCP overrides already exist and are reused.
- Steering: pause and priority changes take effect at the next dispatch tick; flow and
  profile edits apply from the next task.

**Non-goals (explicitly out of scope, separate specs)**
- Benchmark-driven model routing per role, 429 fallback, capacity awareness.
- Memory stack (retrieval per task, write-back).
- Per-task token budget with auto-splitting.
- Parallel task execution (v1 dispatches sequentially; the design must not preclude N-wide).
- Any `surge-ui` screen.
- A skill / MCP *fetch-from-registry* installer (v1 composes from what is on disk; the gate
  and the `ComposedArtifactInstalled` event are designed so a fetch step slots in later).

## Approach

**Decomposition (decided; both review lenses converged, evidence in §Alternatives): the queue
is cross-run state and lives in the registry next to `runtime_capacity` / `roadmap_patch_index`;
the ordering policy is a pure function in `surge-orchestrator`; the daemon owns only the
dispatch tick; every task is an ordinary run with its own worktree, log, admission slot and
recovery.** There is no "project run" and no mid-run graph mutation.

### Data flow

```
.surge/roadmap.toml ──(hash)──▶ registry task_queue (mirror: priority, depends_on, size, age)
        ▲                               │  QueuePolicy::next()  (pure, surge-orchestrator)
        │ RoadmapPatch (feature-planner  ▼
        │  planning run + approval)   daemon TaskScheduler tick ─▶ TicketRunLauncher::launch
        │                                                             │
GitHub/Linear/MCP/user ── intake ──▶ planning run                     ▼
                                                     task run: pick/compose flow ─▶ [gate] ─▶ flow ─▶ verifier ─▶ merge
                                                               (FlowCatalog + ProfileRegistry, per run, .surge > home > bundled)
```

### Crate placement

| Concept | Crate | New / reused |
|---|---|---|
| `ProjectLayer { root, profiles_dir, flows_dir, skills_dir }` + `Layer::{Project, Home, Bundled}`; `Provenance::Project` | surge-core | new struct/enum; extends `Provenance` (`crates/surge-core/src/profile/registry.rs:66`) |
| `RoadmapTask.priority: Priority` (default Medium, `#[serde(default)]`), `RoadmapTask.flow: Option<FlowRef>` (pin a template) | surge-core | reuse `Priority` (`roadmap.rs:748`); roadmap schema v2 additive |
| `FlowRef` (`name@MAJOR[.MINOR]`), `FlowCatalog` (scan roots, `[metadata] when_to_use`, `archetype`, `autonomy`) | surge-core | new; same shape as `ProfileKey`/`SkillCatalog` (`skill.rs:133`) |
| `RunOrigin::Task { project_root, task_id, attempt }` on `RunConfig` | surge-core | field, `#[serde(default)]` — no schema bump |
| `EventPayload::ComposedArtifactInstalled { kind, path, hash, gate_node }` | surge-core | **new variant → event schema 8→9** (the only bump) |
| `QueueEntry`, `QueuePolicy::next(&[QueueEntry], now) -> Vec<TaskId>` | surge-orchestrator::scheduler | new, pure, property-tested |
| `ProfileRegistry` per-run: `EngineRunConfig.project_layer: Option<ProjectLayer>`; lookup project → home → bundled | surge-orchestrator::profile_loader | reshape (`registry.rs:33`, `engine/config.rs:24`) |
| Task-run graph builder: pick template (`FlowCatalog`) or run flow-generator stage with `{{flow_catalog}}` + `{{profile_catalog}}` bindings; `run_flow_generator_post_processing` reused | surge-orchestrator::engine::bootstrap | reuse (`bootstrap.rs:125`) |
| Planning-run graph for intake: feature-planner node + `human_gate` → `RoadmapPatchApprovalLoop` (target `ProjectRoadmap`) | surge-orchestrator | reuse (`feature_driver.rs:117`, `roadmap_amendment.rs:135`) |
| `task_queue` table (migration 0017): `project_root, task_id, roadmap_hash, priority, depends_on_json, size, enqueued_at, skipped_dispatches, dispatch_state, run_id`; `project_queue` row: `paused` | surge-persistence | new; CAS `UPDATE … WHERE dispatch_state='queued'` (idempotency as `intake_emit_log`) |
| `TrustStore` for `.surge/`: pinned content hashes per repo under `SURGE_HOME/trust/<repo-id>.toml` | surge-persistence | new; reuse skill hash (`skill.rs:100`); add hash for profiles / flows |
| `TaskScheduler` tick + reconcile; `TickLoop` extracted from `wake_scheduler`/`snooze_scheduler` | surge-daemon | new + extraction (their own doc says third copy → extract) |
| `surge task {list,priority,pause,resume,regenerate-flow,requeue,skip}`, `surge ready` dependency-aware, `surge flow {list,show}` | surge-cli | new subcommands, `ready.rs` fix |

### Queue semantics (G-B)

- **Ready** = status `Pending` ∧ every `depends_on` is `Completed`/`verified`. A dependency in
  `Failed`/`Skipped` blocks the dependent; `surge ready` shows it as `blocked_by_failed` with
  the offending id; `surge task requeue|skip` resolves it. Never silently promoted.
- **Order** = `(effective_priority desc, size asc, enqueued_at asc)`, where
  `effective_priority = priority + floor(skipped_dispatches / aging_threshold)` capped at
  Critical. Aging is the starvation guard the critic demanded: a Low task passed over
  `aging_threshold` times (default 5) rises one level. Config: `[queue] aging_threshold`.
- **Dispatch** is sequential in v1: at most one row in `Dispatched` per project. The policy
  returns a `Vec` so N-wide is a config change, not a rewrite.
- **Mirror invalidation**: the row carries `roadmap_hash`; a tick whose file hash differs
  re-mirrors planning columns (priority, deps, size) and keeps execution columns.
- **Priority edit** = write `.surge/roadmap.toml` (planning truth) and re-mirror; the CLI does
  both. A dispatch that raced the edit is visible: the child's `RunOrigin` carries the
  `roadmap_hash` it was dispatched under.
- **Pause** = `project_queue.paused = true`; the tick stops dispatching; a running task run
  finishes its current stage boundary (same semantics as `surge steer`) and is then not
  advanced. Resume clears the flag. No new `RunStatus`.
- **Merge**: after the task run's verifier-owned `verified`, the daemon merges the run branch
  into the project branch with `merge_run_worktree` (`surge-git/src/worktree.rs:745`, first
  production caller) before the next dispatch; the next task's worktree is created from the
  updated project branch (`create_run_worktree(base_branch)`). Merge conflict → row
  `Failed{merge_conflict}` + `EscalationRequested`.

### Intake → roadmap (G-C)

`TicketRunLauncher::launch` (`surge-daemon/src/inbox/ticket_run_launcher.rs:192`) gains a
policy branch `IntakePolicy::PlanIntoRoadmap` (default when `.surge/roadmap.toml` exists):
instead of starting a work run it starts a **planning run** whose graph is
feature-planner → `human_gate` (autonomy ≥ high: auto-approve when no conflict) →
`RoadmapPatchApprovalLoop` with `RoadmapPatchTarget::ProjectRoadmap`. Conflict
`RunningMilestone` in the automatic path defaults to `DeferToNextMilestone`; other conflicts
escalate as today. Applied patch → file → mirror → row. "user" and external tools = a daemon IPC verb `SubmitTask` (used by
`surge task add`); `rmcp` is client-only in this workspace, so an MCP server exposing it is
deferred to the memory/MCP spec (user decision 2026-09-18).

### Flow catalog + composition (G-D, G-E)

- A template is a `flow.toml` with `[metadata] when_to_use`, `archetype`, `autonomy`
  (`auto | milestone_gates | task_gates`), and unbound task-scoped bindings
  (`{{task}}`, `{{acceptance_criteria}}`, `{{project_context}}`) filled at dispatch.
  Bundled archetypes gain `when_to_use` and become the bundled tier of the catalog.
- **Selection**: if `RoadmapTask.flow` is pinned → that template. Else a cheap *classifier
  stage* (one agent turn, read-only, bound to `{{flow_catalog}}` + `{{task}}`) returns
  `use: <FlowRef>` or `compose`. On `compose` the existing flow-generator stage runs with
  `{{flow_catalog}}`, `{{profile_catalog}}` and the task; its output is validated with the
  bootstrap retry loop, **then written to `.surge/flows/<name>-1.0.toml` behind the ADR-0015
  gate** and recorded as `ComposedArtifactInstalled{kind: Flow}`. From then on it is a catalog
  entry the user can read, edit and pin.
- **Validation for task flows** (new `FlowPurpose::Task` argument to `validate_for_m6`):
  `UnverifiedSuccessPath` and `SameRuntimeVerification` become `Severity::Error`; a
  `human_gate` must exist at the level the template's `autonomy` demands; every agent node
  must resolve against the per-run registry (resolver pass moved before the gate, closing the
  "passes retry loop, fails at start" hole). Findings are rendered on the gate card, not only
  `tracing::warn!`.
- **Composed profiles**: the generator may emit `ArtifactKind::Profile` artefacts with
  `category = "_generated"`; post-processing loads each through the strict `ProfileRegistry`
  path, rejects `[verification] authority = true`, rejects a name that shadows a bundled or
  home profile, and installs into `.surge/profiles/_generated/` behind the same gate with
  `ComposedArtifactInstalled{kind: Profile}`.
- **Per-node overrides** reuse `AgentConfig.prompt_overrides` / `tool_overrides`
  (`skills_add/remove`, `mcp_add/remove`, `agent_config.rs:133-139`) — no new fields.
- **`.surge/` load-time trust**: on first load of a project file (profile / flow / skill /
  `surge.toml [[mcp_servers]]` entry) the daemon records its hash in `TrustStore`; a changed or
  new hash on a later load → the run is not started, `EscalationRequested{UntrustedProjectFile}`
  is raised with the diff, `surge trust accept <path>` pins it. The gate that installs a
  composed artefact pins it in the same step, so the system's own output is never re-prompted.
  Shadowing rule: a project profile whose name matches a bundled one is loaded only if pinned
  **and** logged with `Provenance::Project` on every `ProfileResolved` line.

### Error strategy / concurrency

- Library errors are `thiserror` enums per crate (`SchedulerError`, `FlowCatalogError`,
  `TrustError`); the daemon maps to `EscalationRequested` causes; the CLI uses `anyhow`.
- The scheduler tick is single-flight per project (tokio `Mutex<()>` keyed by project root);
  the CAS on `dispatch_state` makes a double tick harmless. The tick injects `now_ms` like
  `recovery.rs` for testability.
- Daemon death mid-dispatch: row stays `Dispatched{run_id}`; `recover_on_startup` handles the
  run; the tick reconciles rows against a **read-only** run listing (`Storage::list_runs`
  mutates — memory note — so a `list_runs_readonly` is added).

### Alternatives considered

| Option | Trade-off | Why not chosen |
|---|---|---|
| Project run + child runs | Reuses run machinery on paper | Parent holds an admission slot forever (FIFO, 8, no preemption → deadlock); recovery rule 6 flags an idle parent stuck after 24 h; a queue inside one run's log is project truth in run-scoped storage; two parents = split brain. Invariants would be driver discipline, not types. |
| Dynamic subgraph in one run | One log, exact replay by hash | `GraphRevisionAccepted` pickup only at `frames.is_empty()` (`run_task.rs:2009`); one worktree for the whole project violates the isolation rule; single-threaded engine makes parallel tasks impossible forever. |
| Per-project event journal (Option 4) | Principled event sourcing for queue edits | Right end state, wrong first step: a second log kind with its own writer/fold. The `task_queue` table is kept narrow so a journal can back it later; recorded in ADR-0020. |
| Generate a flow per task, every task | Maximal fit | 1–4 agent turns + retries per task, escalation on cap exhaustion; nothing durable for the user to edit. Superseded by the catalog after the user's amendment. |

Each alternative's invariant encoding, abuse cases and forward view are in the architect and
critic reports of 2026-09-18 (conversation record); the chosen approach's are inlined above.

## Public surface & semver impact

Pre-1.0, active development. Additive: `RoadmapTask.priority`/`flow`, `RunConfig.origin`,
`Provenance::Project`, `ArtifactKind::{Profile, Flow}`, `FlowRef`/`FlowCatalog`,
`ProjectLayer`, `EventPayload::ComposedArtifactInstalled` (**event schema 8 → 9**, migration
+ proptest per `run_event.rs:683`). Breaking (accepted): `ProfileRegistry::load` takes a
`ProjectLayer`; `EngineRunConfig` gains fields (constructed with `..Default::default()`
everywhere); `validate_for_m6` takes `FlowPurpose`; registry migration 0017. Not done: no new
`NodeKind`, no second MCP list beside `surge.toml`, no un-bumped variant.

## Pre-code maintainer verdict (recorded)

**RESHAPE NEEDED → reshaped, ACCEPTABLE.** Reshapes applied from the two lenses: no project
run; registry per run; index is a hash-keyed mirror, not a copy; no install path invented —
ADR-0015 gate named; warnings rendered on the gate; verifier rule is an error for task flows;
load-time trust for `.surge/`; aging in the queue; merge has a production caller; pause has a
defined boundary. Owning crates per concept as in the placement table.

## Acceptance criteria

The spec-level outer test: `tests/ato_outer_test.rs` in surge-daemon — a temp repo with
`.surge/roadmap.toml` (3 tasks, `t2 depends_on t1`, `t3` Low), a `.surge/flows/` with one
template, mock ACP agent; drive the scheduler tick with an injected clock.

Traced to intent "task list executes on its own":
- [ ] Given the queue, when the tick runs, then `t1` dispatches first and `t2` is not dispatched until `t1`'s ledger row is `verified`; `t3` dispatches before `t2` only if `t1` failed. (outer test)
- [ ] Given `t1` verified, when the next tick runs, then the project branch contains `t1`'s commits and `t2`'s worktree is based on it. (outer test; first production use of `merge_run_worktree`)
- [ ] Given a merge conflict, then the row is `Failed{merge_conflict}`, an `EscalationRequested` is in the log, and `t2` stays blocked. (unit + outer)
- [ ] Given a dependency in `Failed`, then `surge ready` lists the dependent as `blocked_by_failed(t1)` and does not dispatch it; `surge task requeue t1` unblocks. (CLI test)
- [ ] Given a Low task skipped `aging_threshold` times, then its effective priority rises one level (property test on `QueuePolicy`: order is total, deterministic, deps always dominate, aging is monotone).

Traced to "each task gets its own flow built from the project's agents":
- [ ] Given a task whose classifier answer is `use: bug-fix@1`, then the run's `PipelineMaterialized` graph equals the template with task bindings filled and no generator turn occurred. (outer, mock agent transcript)
- [ ] Given `compose`, then the generator's output is validated with `FlowPurpose::Task`; an output with success reachable without a `verified` gate, or same-runtime verifier, is rejected and retried; on success the file exists in `.surge/flows/` and `ComposedArtifactInstalled{kind: Flow}` is logged after the gate resolves. (unit for validator — negative cases first — + outer)
- [ ] Given a composed profile with `authority = true` or a bundled name, then it is rejected at post-processing with a named error. (unit)
- [ ] Given a `.surge/profiles/x-1.0.toml` in repo and none in home, then the run resolves `x@1` with `Provenance::Project`; with the same name in home, project wins; two daemons/two repos never see each other's profiles. (registry test)

Traced to "external tasks appear in the same list in the right place":
- [ ] Given a GitHub issue candidate, when intake launches, then a planning run (not a work run) starts, and after approval `.surge/roadmap.toml` contains the task at the planner's insertion point and a queue row exists. (daemon test with mock source)
- [ ] Given the patch conflicts with the running milestone, then it is deferred to the next milestone without operator input. (unit on policy)

Traced to "pause, reprioritise or edit a flow without stopping the run":
- [ ] Given `surge task pause`, then no new dispatch occurs, the running run reaches its next stage boundary and halts; `resume` continues. (outer)
- [ ] Given `surge task priority t3 critical` while `t1` runs, then the file and the row change and `t3` dispatches next; the running run's `RunOrigin.roadmap_hash` still shows the old hash. (outer)
- [ ] Given an edit to `.surge/flows/bug-fix-1.0.toml` mid-run, then the current task is unaffected and the next task using it gets the edited content — after the trust prompt if the hash changed. (outer)

Traced to "the agents and flows the system composed are files under `.surge/`":
- [ ] Given a fresh clone with an unpinned `.surge/` profile, then the first run does not start, `EscalationRequested{UntrustedProjectFile}` is logged, and `surge trust accept` pins it. (daemon test)

Cross-cutting:
- [ ] Event schema migration 8→9 has the round-trip proptest and the "adding a variant bumps" test passes. (existing harness)
- [ ] `surge ready` no longer reports the worktree as project path. (CLI test)
- [ ] Daemon restart with a row in `Dispatched` and a live run: reconcile leaves it; with a dead run: row goes `Failed` and the task is re-queued once (`attempt+1`). (recovery test, injected clock)

## Risks & open questions

- The classifier stage is still one agent turn per task (cheap, read-only). If it proves
  flaky, `RoadmapTask.flow` pinning by the roadmap-planner at plan time removes it entirely.
- Trust-on-first-use will prompt on every new clone's `.surge/`; acceptable for v1, and it is
  the only honest answer to "a file in git was never installed".
- v1 is single-runtime unless the user configures several; with routing deferred,
  `SameRuntimeVerification`-as-error may reject every composed flow on a one-runtime setup —
  the validator must downgrade to warning when the per-run registry has one runtime, and say
  so on the gate card.
- Merge-after-verify assumes tasks are small enough to merge cleanly; conflicts escalate,
  they are not auto-resolved.

## Links

- `intent.md` (same slug) — frozen problem + user amendments
- ADR-0020 (to write), ADR-0015 (skill binding trust), ADR-0006 (ACP only), ADR-0013 (intake tiers)
- `docs/plans/2026-07-07-001-feat-task-ledger-verifier-owned-done-plan.md` (Phase 1 ledger)
- `docs/competitive-plan-2026-09.md` Wave 1 (skills), Wave 4 (capacity — the routing spec)
- Memory: `surge-list-runs-not-read-only`, `surge-schema-bump-variant-vs-field`,
  `surge-no-flow-marks-implementer`, `surge-warning-rules-swallowed-in-engine`
