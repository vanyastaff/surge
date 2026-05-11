# Implementation Plan: Roadmap Amendments via `surge feature`

Branch: feature/roadmap-amendments-surge-feature
Created: 2026-05-11
Refined: 2026-05-11

## Settings
- Testing: yes
- Logging: verbose
- Docs: yes

## Roadmap Linkage
Milestone: "Roadmap amendments via `surge feature`"
Rationale: This is the next unchecked roadmap milestone after the artifact convention library and depends on the newly merged profile artifact contracts.

## Scope
Add a typed amendment lifecycle for follow-up feature requests: `surge feature describe <prompt>` runs the bundled Feature Planner profile, produces and validates a `roadmap-patch.toml`, routes it through human approval, applies approved changes to the active or completed roadmap, and either updates an active roadmap-flow run or starts a follow-up run from the appended work.

Non-goals:
- Do not add new `NodeKind` variants; use existing Agent, HumanGate, Loop, Subgraph, Notify, and Terminal semantics.
- Do not mutate completed run history. Completed artifacts stay replayable; approved amendments are appended as new events and/or follow-up run inputs.
- Do not make tracker status changes part of this milestone; tracker automation tiers remain a later milestone.
- Do not build the final Telegram cockpit UX here; add structured notification payloads and minimal delivery hooks that the later Telegram milestone can render richly.
- Do not retire `surge-spec`; legacy cleanup remains a separate roadmap milestone.

## Commit Plan
- **Commit 1** (after tasks 1-5): "feat(core): define roadmap patch contracts"
- **Commit 2** (after tasks 6-9): "feat(feature): draft and store roadmap patches"
- **Commit 3** (after tasks 10-15): "feat(engine): apply roadmap amendments"
- **Commit 4** (after tasks 16-19): "feat(cli): expose feature amendment workflow"
- **Commit 5** (after tasks 20-22): "docs: document roadmap amendment lifecycle"

## Tasks

### Phase 1: Core Contract And Validation
- [x] Task 1: Define typed `RoadmapPatch` structures in `surge-core`.
  Deliverable: add a pure domain model for `RoadmapPatch`, `RoadmapPatchId`, `RoadmapPatchTarget`, `InsertionPoint`, patch operations, dependencies, rationale, conflict metadata, patch status, and schema version.
  Expected behavior: the model can represent inserting new milestones, inserting tasks into existing milestones, replacing draft-only items, dependency edges, conflict metadata, operator conflict choices, and a stable content hash for idempotency.
  Files: `crates/surge-core/src/roadmap_patch.rs`, `crates/surge-core/src/lib.rs`, `crates/surge-core/src/roadmap.rs`.
  Logging requirements: no runtime logging in `surge-core`; expose stable IDs, content hashes, and validation codes so callers can log patch lifecycle at DEBUG/INFO/WARN without parsing prose.
  Dependency notes: foundational for all later tasks; keep `surge-core` pure with no filesystem, `tokio`, `tracing`, or database access.

- [x] Task 2: Register RoadmapPatch as a first-class artifact contract.
  Deliverable: extend `ArtifactKind`, parser aliases, the contract matrix, diagnostics, and CLI kind handling so `roadmap-patch.toml` is a first-class artifact rather than an ad hoc TOML file.
  Expected behavior: `surge artifact validate --kind roadmap-patch roadmap-patch.toml` accepts minimal valid patches and rejects malformed TOML, missing insertion point, empty item sets, unsupported schema versions, and dependency references that cannot be resolved against a supplied roadmap context when context is available.
  Files: `crates/surge-core/src/artifact_contract.rs`, `crates/surge-cli/src/commands/artifact.rs`, `crates/surge-core/tests/` or inline tests, `crates/surge-orchestrator/tests/fixtures/artifacts/`.
  Logging requirements: CLI/orchestrator callers log validation start/end at DEBUG, invalid patch summaries at WARN, and never log the full patch body unless it is a synthetic test fixture.
  Dependency notes: depends on task 1; reuse existing artifact validation style from the artifact convention milestone.

- [x] Task 3: Wire Feature Planner profile output declarations and sandbox mode.
  Deliverable: update `feature-planner-1.0.toml` so `patched` declares `roadmap-patch.toml` as a produced artifact with the RoadmapPatch contract, plus a reject-mode validator hook where applicable. Change the profile sandbox from `read-only` to the narrowest write-capable mode required to create the patch artifact.
  Expected behavior: the engine rejects malformed Feature Planner output before treating the outcome as usable, `out_of_scope` remains a non-patch outcome, and the profile can actually write its declared artifact inside the worktree.
  Files: `crates/surge-core/bundled/profiles/feature-planner-1.0.toml`, `crates/surge-core/src/profile/bundled.rs`, `crates/surge-orchestrator/tests/profile_registry_e2e.rs`.
  Logging requirements: resolved profile artifact contracts log at DEBUG with profile id, outcome id, artifact kind, path, and schema version; rejection logs stay concise.
  Dependency notes: depends on task 2 and the existing profile artifact validation infrastructure.

- [x] Task 4: Add roadmap amendment event types.
  Deliverable: extend `EventPayload` with `RoadmapPatchDrafted`, `RoadmapPatchApprovalRequested`, `RoadmapPatchApprovalDecided`, `RoadmapPatchApplied`, and `RoadmapUpdated` or an equivalent compact event set that captures the same lifecycle.
  Expected behavior: event payloads round-trip through the existing JSON/bincode facade, expose stable discriminants, keep replay deterministic, and do not embed full patch text when a content hash/reference is enough.
  Files: `crates/surge-core/src/run_event.rs`, `crates/surge-core/src/run_state.rs`, migration tests if needed.
  Logging requirements: event writers log patch id, run id, target, decision, and hash at INFO; validation/conflict failures log at WARN with stable reason codes.
  Dependency notes: depends on task 1. Preserve backward compatibility for old event logs.

- [x] Task 5: Update event fold, materialized views, and replay readers for amendment events.
  Deliverable: decide which new roadmap amendment events are fold-visible, which update materialized SQL views, and which are explicit replay no-ops. Add reader/view APIs where CLI and notifications need patch lifecycle queries.
  Expected behavior: `RunMemory` captures patch/applied roadmap state only when needed for deterministic execution; `views::maintain` handles or explicitly ignores each new event; event-log rebuilds reproduce the same patch registry/read model after restart.
  Files: `crates/surge-core/src/run_state.rs`, `crates/surge-persistence/src/runs/views.rs`, `crates/surge-persistence/src/runs/reader_views.rs`, `crates/surge-persistence/src/runs/types.rs`, `crates/surge-persistence/src/runs/mod.rs`, persistence tests.
  Logging requirements: storage layer logs migrations/view rebuild failures at ERROR through existing surfaces; amendment-specific writers log patch id and lifecycle transition at DEBUG/INFO above the persistence layer.
  Dependency notes: depends on task 4 and blocks CLI `list/show/reject` reliability.

### Phase 2: Patch Drafting And Approval Flow
- [x] Task 6: Build a reusable Feature Planner execution driver through existing agent-stage semantics.
  Deliverable: add an orchestrator helper that runs the Feature Planner as an agent stage or ephemeral graph so produced artifact declarations, profile hooks, sandbox intent, and outcome validation are reused rather than reimplemented.
  Expected behavior: the driver works with the mock ACP bridge in tests, binds `request` and `roadmap`, collects `roadmap-patch.toml`, validates it through the same artifact contract path as normal agent stages, retries on validator rejection, and returns a typed patch or `out_of_scope`.
  Files: `crates/surge-orchestrator/src/feature_driver.rs` or `crates/surge-orchestrator/src/roadmap_amendment.rs`, `crates/surge-orchestrator/src/lib.rs`, `crates/surge-orchestrator/tests/feature_planner_driver_test.rs`.
  Logging requirements: DEBUG for profile resolution, session open/close, binding paths, and artifact discovery; INFO for drafted patch id/hash; WARN for out-of-scope or invalid output; ERROR for ACP/session failures.
  Dependency notes: depends on tasks 1-3; follow patterns from `project_context.rs`, `bootstrap_driver.rs`, and agent-stage artifact reading.

- [x] Task 7: Store patch artifacts and amended artifacts through the artifact store.
  Deliverable: persist `roadmap-patch.toml`, amended `roadmap.toml`/`roadmap.md`, and amended or follow-up `flow.toml` via the existing content-addressed artifact store and emit hash/path references in events.
  Expected behavior: patch lifecycle events refer to stable content hashes; replay and follow-up runs can recover patch and amended artifacts without reading mutable worktree files; no event embeds large artifact bodies.
  Files: `crates/surge-orchestrator/src/roadmap_amendment.rs`, `crates/surge-persistence/src/artifacts.rs`, `crates/surge-orchestrator/tests/roadmap_amendment_artifacts_test.rs`.
  Logging requirements: DEBUG for artifact store paths and content hashes; INFO for stored patch/amended artifact refs; ERROR for store failures with path/hash context but without full artifact contents.
  Dependency notes: depends on tasks 1, 4, and 6; blocks robust restart/replay behavior.

- [x] Task 8: Add a human approval loop for roadmap patches.
  Deliverable: introduce an approval helper that presents patch summary, insertion point, dependencies, and rationale with approve/edit/reject decisions and a bounded redo loop.
  Expected behavior: approve proceeds to apply, edit feeds operator feedback back into the Feature Planner, reject records a terminal rejected patch state, and repeated invalid/edit cycles escalate cleanly.
  Files: `crates/surge-orchestrator/src/roadmap_amendment.rs`, `crates/surge-orchestrator/src/engine/stage/human_gate.rs` if reusable helpers are needed, tests under `crates/surge-orchestrator/tests/`.
  Logging requirements: INFO for approval requested/decided with patch id and decision; DEBUG for redo iteration count; WARN when edit-loop cap is reached or approval times out.
  Dependency notes: depends on tasks 4, 6, and 7; reuse existing `HumanInputRequested` / `resolve_human_input` patterns instead of creating a parallel approval mechanism.

- [x] Task 9: Persist pending patch metadata for list/show/reject commands.
  Deliverable: add a project-level or run-indexed persistence surface for patch records keyed by patch id/content hash with status, target run/project, created time, decision, and content hash. Prefer event-derived read models when the patch belongs to a run; use a project-level SQLite table only for patches that are not yet attached to a run.
  Expected behavior: pending patches survive process restarts, `list` can filter by pending/applied/rejected, duplicate content hashes resolve to the existing patch record, and `reject` works without scanning every run event log.
  Files: `crates/surge-persistence/src/roadmap_patches.rs` if a project-level store is needed, `crates/surge-persistence/src/runs/reader_views.rs`, `crates/surge-persistence/src/lib.rs`, migration files if schema changes are required, persistence tests.
  Logging requirements: DEBUG for store open/query paths, INFO for status transitions, WARN for duplicate/replay conflicts, ERROR for migration or serialization failures.
  Dependency notes: depends on tasks 1, 5, and 7; supports CLI tasks 16-17.

- [x] Task 10: Normalize roadmap target discovery.
  Deliverable: implement a target resolver that can identify whether a prompt should amend a project roadmap file, an active run's roadmap artifact, or a completed bootstrap/follow-up run.
  Expected behavior: resolver returns a typed target with roadmap artifact hash/path, current flow hash/path when available, run status, run worktree when available, last safe amendment point, and whether active pickup is allowed. Ambiguous targets require an explicit CLI selector.
  Files: `crates/surge-orchestrator/src/roadmap_target.rs`, `crates/surge-persistence/src/runs/`, `crates/surge-cli/src/commands/feature.rs`.
  Logging requirements: DEBUG for candidate target scan and selected target; INFO for chosen active/completed/project target; WARN when no roadmap is found or multiple ambiguous targets require explicit CLI selection.
  Dependency notes: depends on tasks 5 and 9 for persisted patch context; use event-log reads and existing project config paths rather than ad hoc global state.

### Phase 3: Applying Patches To Roadmaps And Flows
- [x] Task 11: Implement pure roadmap patch application.
  Deliverable: add pure functions that apply a `RoadmapPatch` to a `RoadmapArtifact` and markdown `roadmap.md` representation while preserving completed history and producing a patch result summary.
  Expected behavior: approved patches append or insert only where allowed, keep original completed items intact, produce deterministic ordering, and reject references to missing or currently-running milestones with typed conflict errors.
  Files: `crates/surge-core/src/roadmap_patch.rs`, `crates/surge-core/src/roadmap.rs`, core tests and fixtures.
  Logging requirements: core remains log-free; callers log apply result with inserted milestone/task ids and conflict codes.
  Dependency notes: depends on tasks 1 and 10. Prefer typed parsing and `toml_edit`/structured helpers over ad hoc string replacement.

- [x] Task 12: Implement flow amendment generation and validation.
  Deliverable: add an orchestrator helper that translates approved roadmap patch results into changes for the active `flow.toml`, inserting new milestone/task nodes and rewiring edges through existing graph primitives.
  Expected behavior: generated graphs pass `validate_for_m6`; rollback leaves the previous graph untouched on validation failure; follow-up-only targets can produce a minimal appended flow instead of mutating active flow.
  Files: `crates/surge-orchestrator/src/flow_amendment.rs`, `crates/surge-core/src/graph.rs` if helper APIs are needed, `crates/surge-orchestrator/tests/flow_amendment_test.rs`.
  Logging requirements: DEBUG for graph node/edge insertions and validation start/end; INFO for successful graph amendment with old/new graph hash; WARN for validation rollback with diagnostic summary.
  Dependency notes: depends on task 11. Do not add a new `NodeKind`.

- [x] Task 13: Define graph revision and replay semantics for active amendments.
  Deliverable: decide how an amended graph is represented after the original `PipelineMaterialized` frozen graph: either append a new graph-revision event, treat `RoadmapUpdated` as a deferred loop-input event, or explicitly constrain active amendments to pending loop items without mutating `RunState::Pipeline.graph`.
  Expected behavior: replay from the event log reconstructs the same graph/loop state after an amendment; snapshots can serialize and restore the new state; old runs without amendments replay unchanged.
  Files: `crates/surge-core/src/run_event.rs`, `crates/surge-core/src/run_state.rs`, `crates/surge-orchestrator/src/engine/replay.rs`, `crates/surge-orchestrator/src/engine/snapshot.rs`, engine replay/snapshot tests.
  Logging requirements: INFO when a new graph revision or deferred update is accepted; DEBUG for replay/snapshot reconstruction details; WARN when an attempted active mutation cannot be represented safely.
  Dependency notes: depends on tasks 4, 5, and 12; blocks active-run pickup.

- [x] Task 14: Add active-run pickup semantics at safe loop boundaries.
  Deliverable: teach the engine/daemon-side runner to observe `RoadmapUpdated` for eligible active roadmap-loop runs and include new pending items only at a safe outer milestone loop boundary.
  Expected behavior: if the current milestone is already running, the update is deferred to the next safe loop boundary; already-resolved `LoopFrame.items` are not silently mutated mid-iteration; if the target is terminal or active pickup is disabled, a follow-up run path is selected instead.
  Files: `crates/surge-orchestrator/src/engine/run_task.rs`, `crates/surge-orchestrator/src/engine/routing.rs`, `crates/surge-orchestrator/src/engine/stage/loop_stage.rs`, `crates/surge-daemon/src/`, engine tests for active and terminal run cases.
  Logging requirements: INFO for update observed/picked-up/deferred/follow-up-created; DEBUG for loop-boundary checks; WARN for conflicts that require operator choice.
  Dependency notes: depends on tasks 10, 12, and 13; this is the riskiest part, so land only after pure apply, graph revision, and flow validation tests pass.

- [x] Task 15: Implement follow-up run creation from appended work.
  Deliverable: add helper logic that materializes a follow-up graph from the approved patch when a completed roadmap or non-pickup target is amended.
  Expected behavior: completed history is never mutated; the follow-up run receives the current project context and appended roadmap portion as seed artifacts; CLI output prints the new run id.
  Files: `crates/surge-orchestrator/src/roadmap_amendment.rs`, `crates/surge-cli/src/commands/feature.rs`, tests under `crates/surge-orchestrator/tests/`.
  Logging requirements: INFO for follow-up run creation with parent target, patch id, and run id; DEBUG for artifact seeding; ERROR for failure to materialize or start the follow-up run.
  Dependency notes: depends on tasks 7, 10, 11, and 12 and reuses `bootstrap_driver` materialization patterns where possible.

### Phase 4: CLI And Notification Surfaces
- [x] Task 16: Add `surge feature describe <prompt>`.
  Deliverable: introduce a `Feature` CLI command group with `describe`, target selection flags, worktree options, JSON output, and human-readable progress output.
  Expected behavior: command resolves a roadmap target, runs Feature Planner, records pending patch metadata, prompts for approval when configured for console mode, and applies or stores the patch according to user decision.
  Files: `crates/surge-cli/src/main.rs`, `crates/surge-cli/src/commands/feature.rs`, `crates/surge-cli/src/commands/mod.rs`, CLI tests.
  Logging requirements: DEBUG for args/target resolution and driver steps; INFO for patch drafted/applied/follow-up started; WARN for out-of-scope, ambiguous target, or rejected patch; avoid logging full prompt text when it may contain secrets.
  Dependency notes: depends on tasks 6-10 and tasks 11-15 for apply/start behavior.

- [x] Task 17: Add CLI mirrors for pending patches.
  Deliverable: implement `surge feature list`, `surge feature show <id>`, and `surge feature reject <id>` using the patch metadata persistence surface from task 9.
  Expected behavior: `list` shows patch id, status, target, created time, and short rationale; `show` can emit human or JSON details; `reject` is idempotent and records the decision.
  Files: `crates/surge-cli/src/commands/feature.rs`, `crates/surge-persistence/src/roadmap_patches.rs`, CLI integration tests.
  Logging requirements: DEBUG for query filters; INFO for reject status transition; WARN when a patch id is missing, already applied, or already rejected.
  Dependency notes: depends on task 9 and should work even before active-run pickup is enabled.

- [x] Task 18: Add notification payloads for amendment lifecycle.
  Deliverable: add structured notification messages for patch approval requested, patch applied, runner pickup, follow-up run created, conflict detected, and rejection.
  Expected behavior: notification channels can render useful text today, while Telegram can later upgrade them into rich cards without changing the event schema.
  Files: `crates/surge-notify/src/messages.rs`, `crates/surge-notify/src/multiplexer.rs`, `crates/surge-orchestrator/src/roadmap_amendment.rs`, notification tests.
  Logging requirements: INFO for notification dispatch attempts and outcomes; DEBUG for channel selection; WARN for non-blocking delivery failures unless policy makes them blocking.
  Dependency notes: depends on tasks 4, 8, 11, 14, and 15.

- [x] Task 19: Add conflict resolution options.
  Deliverable: model operator choices for conflicts: defer to next milestone, abort current run, create follow-up run, or reject patch.
  Expected behavior: conflicts referencing an already-running milestone surface clear choices; selected choices are persisted and reflected in the eventual apply/follow-up path.
  Files: `crates/surge-core/src/roadmap_patch.rs`, `crates/surge-orchestrator/src/roadmap_amendment.rs`, `crates/surge-cli/src/commands/feature.rs`, tests.
  Logging requirements: WARN for conflict detection with stable conflict code; INFO for chosen resolution; DEBUG for recalculated target after resolution.
  Dependency notes: depends on tasks 8, 10, 14, and 15.

### Phase 5: Regression Coverage And Documentation
- [x] Task 20: Add end-to-end amendment tests.
  Deliverable: cover malformed patch rejection, duplicate patch idempotency, amendment during a running roadmap, amendment after a terminal roadmap, conflict on running milestone, CLI list/show/reject, and follow-up run creation.
  Expected behavior: default CI uses mock agents and synthetic fixtures only; real-agent amendment tests are ignored or feature-gated. Tests cover replay/snapshot behavior for amended runs and event-view rebuild behavior for patch lifecycle queries.
  Files: `crates/surge-orchestrator/tests/roadmap_amendment_e2e.rs`, `crates/surge-cli/tests/feature_cli_test.rs`, `crates/surge-core/tests/` or inline core tests, fixtures under `crates/surge-orchestrator/tests/fixtures/`.
  Logging requirements: tests assert important lifecycle logs or event discriminants where practical; fixture failures print patch ids and diagnostic codes, not full private prompts.
  Dependency notes: depends on tasks 1-19 and should be expanded as each phase lands.

- [x] Task 21: Document the amendment lifecycle.
  Deliverable: update workflow/docs with a lifecycle diagram and command reference for `surge feature describe/list/show/reject`, including active-run vs follow-up behavior.
  Expected behavior: docs explain which artifacts are mutated, which events are appended, how conflict choices work, and how completed history remains replay-safe.
  Files: `docs/workflow.md`, `docs/cli.md`, `docs/conventions/roadmap.md`, `docs/README.md`, `README.md` if command overview needs a short mention.
  Logging requirements: docs mention where users can find INFO/WARN lifecycle logs and how to enable verbose `RUST_LOG` while debugging amendments.
  Dependency notes: docs checkpoint is mandatory because `Docs: yes`.

- [x] Task 22: Final verification and roadmap closeout.
  Deliverable: run focused tests, full workspace verification, update `.ai-factory/ROADMAP.md` only after implementation is actually complete, and prepare PR summary.
  Expected behavior: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass; no roadmap checkbox is marked complete until implemented and verified.
  Files: `.ai-factory/ROADMAP.md`, implementation plan status updates, PR description.
  Logging requirements: no new runtime logging; record verification commands and noteworthy failures in the PR summary.
  Dependency notes: final task; do not treat docs-only or CLI-only work as milestone completion.

## Verification Plan
- Run `cargo fmt --check`.
- Run `cargo test -p surge-core roadmap_patch`.
- Run `cargo test -p surge-core artifact_contract`.
- Run `cargo test -p surge-persistence roadmap_patch`.
- Run `cargo test -p surge-orchestrator --test feature_planner_driver_test`.
- Run `cargo test -p surge-orchestrator --test roadmap_amendment_artifacts_test`.
- Run `cargo test -p surge-orchestrator --test flow_amendment_test`.
- Run `cargo test -p surge-orchestrator --test engine_roadmap_update_replay_test`.
- Run `cargo test -p surge-orchestrator --test roadmap_amendment_e2e`.
- Run `cargo test -p surge-cli --test feature_cli_test`.
- Run `cargo test -p surge-notify`.
- Run `cargo clippy --workspace --all-targets -- -D warnings`.
- Run `cargo test --workspace`.
- Run `git diff --check`.

## Implementation Notes
- Keep `RoadmapPatch` and pure application logic in `surge-core`; filesystem, ACP, persistence, and notification wiring belong above it.
- Use structured TOML parsing and `toml_edit`/typed serializers where possible; avoid regex-based roadmap mutation.
- Make patch IDs/content hashes deterministic so idempotency survives restarts and replays.
- Apply active-run amendments only at explicit safe points; defer or create follow-up runs when the current milestone is already executing.
- Keep notification payloads structured now so Telegram cards can become richer later without schema churn.
- Preserve existing bootstrap and graph-engine behavior; amendments should compose with the event log rather than bypass it.
- The Feature Planner driver should reuse agent-stage outcome/artifact validation; avoid introducing a second ACP invocation path with divergent validation behavior.
- Treat active-run amendments as explicit event-log graph/input revisions with replay and snapshot tests before enabling pickup.
