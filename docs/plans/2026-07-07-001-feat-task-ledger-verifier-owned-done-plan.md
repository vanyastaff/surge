---
title: "feat: task ledger + verifier-owned done (product-strategy Phase 1)"
type: feat
status: draft
date: 2026-07-07
---

# feat: task ledger + verifier-owned done (product-strategy Phase 1)

## Summary

Implement Phase 1 of [`docs/product-strategy.md`](../product-strategy.md): turn
the prose roadmap into a **typed, queryable task ledger** (task-granularity
dependencies, `discovered-from` capture, context-budget sizing, `surge ready`)
and make task completion **verifier-owned** (implementers can only report
`ready_for_verification`; a sealed verifier node is the sole write path to a
task's `done` status; flows without verification are flagged at validation
time). Both pillars build on machinery that already exists: `RoadmapArtifact`
(`crates/surge-core/src/roadmap.rs`), the event log + fold, per-run
materialized views, the registry-index pattern (`roadmap_patch_index`), the
`on_outcome` reject pipeline, and the dynamic `report_stage_outcome` enum.

---

## Problem Frame

Two gaps let large projects stall (evidence in `product-strategy.md` §"What
the research established"):

1. **The roadmap is write-only memory.** `RoadmapTask` (roadmap.rs:142) has
   `id/title/status/description/acceptance_criteria` but no task-level
   dependencies (deps are milestone→milestone only, roadmap.rs:174), no
   `discovered-from` edge (work found mid-task is silently dropped), no
   sizing field, and no cross-run queryable store. Nothing can answer "what
   is unblocked right now?".
2. **Completion is agent-claimed.** The only "done" signals are
   `OutcomeReported` (the agent's own `report_stage_outcome` call) and the
   engine's routing `StageCompleted` (run_task.rs:1064). The bundled Verifier
   runs `workspace-write` with read-only-ness enforced by prompt text only
   (verifier-1.0.toml:23-24,58), and no node has more authority than any
   other. Reward-hacking data says agent-claimed completion is *the*
   mechanism behind unfinished projects.

---

## Assumptions

*Agent-authored inferences filling gaps in the strategy text; review before
implementation.*

- **The ledger's source of truth stays artifact + event log, not a new
  mutable store.** `roadmap.toml` (schema v2) remains the git-friendly,
  human-editable definition; runtime status transitions are events folded
  into `RunMemory` and mirrored into a per-run view + a registry index for
  cross-run queries — the same two-tier pattern as `roadmap_patch_index`
  (registry migration 0006) + `RoadmapPatchStore`
  (`crates/surge-persistence/src/roadmap_patches.rs`). No third source of
  truth is introduced.
- **Verifier authority is a profile capability, not a new NodeKind.** The
  `NodeKind` enum is closed (ARCHITECTURE.md §3). Authority is declared in
  the profile (`[verification] authority = true`) and checked by the engine;
  a node without it cannot cause a ledger `done` transition regardless of
  what outcome it reports.
- **Context-budget sizing is a planner contract, not a token counter.**
  Phase 1 adds a required `size` field (`s`/`m`/`l`) plus a planner rule
  ("every task must be completable in one fresh agent session; split
  otherwise") enforced by the existing artifact-contract + `on_outcome`
  reject hooks (roadmap-planner-1.0.toml:60-74 precedent). Token-level
  estimation is deferred — the existing `BudgetLimits` machinery
  (budget.rs) measures spend, not context fit, and conflating them would be
  wrong.
- **`discovered-from` capture is artifact-driven.** Implementer/Verifier
  nodes may produce a `discovered-tasks` artifact (new contract kind); the
  engine folds each entry into the ledger as a `pending` task with a
  `discovered_from` edge. Auto-enqueueing them into execution is *not*
  Phase 1 — they surface via `surge ready --discovered` and future inbox.

---

## Requirements

- R1. `RoadmapArtifact` schema v2: task-level `depends_on`, optional
  `discovered_from`, required `size` (`s`/`m`/`l`), and a `verified: bool`
  marker distinct from `status`; markdown rendering updated; JSON-schema
  snapshot and `docs/conventions/roadmap.md` updated.
- R2. New ledger events (additive, event schema version bump per the
  v1→v2 precedent in `crates/surge-core/src/migrations/mod.rs:21-26`):
  `TaskStatusChanged { task_id, from, to, authority_node }`,
  `TaskDiscovered { task_id, discovered_from, title }`,
  `TaskVerified { task_id, node, evidence: ContentHash }`.
- R3. Fold maintains a `LedgerState` in `RunMemory`; a per-run
  `task_ledger` view is maintained in the same transaction as event append
  (views.rs pattern); a registry `task_ledger_index` table mirrors terminal
  state cross-run (RoadmapPatchStore pattern).
- R4. `surge ready` lists unblocked tasks (all `depends_on` satisfied,
  status `pending`), table + `--json`, modeled on `feature list`
  (`crates/surge-cli/src/commands/feature.rs:218`).
- R5. Implementer profile declares `ready_for_verification` (forward) and
  loses the ability to route straight past verification in bundled flows.
- R6. Verifier profile: `[sandbox] mode = "read-only"` (sealed: read-only +
  no network + no shell, sandbox.rs:42), `[verification] authority = true`,
  and a required `verification-report` produced artifact (new contract
  kind) — the evidence input for the future Run Report.
- R7. Engine enforces authority: a ledger `done`/`TaskVerified` transition
  is only written when the reporting node's resolved profile carries
  verification authority **and** its sandbox mode is `read-only`; a
  mismatch is a stage failure with a clear diagnostic, not a silent
  downgrade.
- R8. Graph validation gains two rules: (a) warn when any path reaches
  `Terminal: success` without traversing a verification-authority node;
  (b) warn when a `Loop` body completes iterations without one. Rendered in
  `surge engine validate` output and the flow-approval card.
- R9. Loop execution binds the current ledger task id into the iteration
  context (`iteration_var_name`, loop_config.rs:7) so stage events and the
  ledger correlate; `on_loop_iteration_done` (loop_stage.rs:213) records
  the task's terminal status via R2 events instead of only
  `LoopIterationCompleted`.
- R10. Roadmap Planner prompt + hooks updated: sizing rule, split rule,
  task-level `depends_on` guidance; `validate_roadmap`
  (artifact_contract/kinds/roadmap.rs:10) upgraded from presence-only to
  field-level checks (unique task ids, acyclic `depends_on`, valid
  `discovered_from` references, `size` present).
- R11. `cargo build --workspace`, `cargo nextest run`, and
  `cargo clippy --workspace --all-targets -- -D warnings` clean; replay
  determinism preserved (fold has no wall-clock/random inputs); docs
  updated (`ARCHITECTURE.md` §3/§4, `workflow.md`, `conventions/`,
  `cli.md`).

---

## Scope Boundaries

- **No parallel dispatch.** `ParallelismMode` stays `Sequential`
  (loop_config.rs); `surge ready` *feeding* concurrent runs is Phase 2+.
  The ledger is designed so dependency-disjoint dispatch becomes a query,
  not a refactor.
- **No Run Report compilation.** The `verification-report` artifact contract
  is defined and required, but assembling reports into a PR-attached bundle
  is the "continuous" strategy line, not Phase 1.
- **No `.surge/memory/`, no inbox, no steering** — Phases 2/3.
- **No token-level context estimation** — `size` is a planner contract
  (see Assumptions).
- **No implementation of `FailurePolicy::Replan`** (currently Abort,
  loop_stage.rs:297-301) — unchanged.
- **No real JSONPath** for `IterableSource` — the dotted-TOML-path resolver
  (loop_stage.rs:164) is extended only as far as R9 needs.
- **No forced migration of user flows/profiles.** Bundled profiles bump
  versions; existing user profiles keep working — they simply carry no
  verification authority, so their runs complete without ledger `done`
  marks (visible in `surge ready` / validation warnings, not fatal).

### Deferred to Follow-Up Work

- Dependency-disjoint parallel dispatch from `surge ready` (needs admission
  + worktree strategy per concurrent task).
- Run Report bundle (evidence compilation, PR attachment).
- Auto-enqueue of discovered tasks behind an approval card.
- A dedicated `Networked` sandbox tier (sandbox.rs:37 note) if a verifier
  ever legitimately needs network fetch — out until proven needed.
- Spec-delta extension of the amendment machinery (Pillar A4).

---

## Context & Research

### Relevant code (verified 2026-07-07)

- `crates/surge-core/src/roadmap.rs` — `RoadmapArtifact` (:21),
  `RoadmapMilestone` (:114), `RoadmapTask` (:142, no deps/size/discovered),
  `RoadmapDependency` (:174, milestone-level only), `RoadmapStatus` (:239),
  `to_markdown()` (:50).
- `crates/surge-core/src/artifact_contract/kinds/roadmap.rs:10` —
  `validate_roadmap()` is presence-only today.
- `crates/surge-core/src/loop_config.rs` — `LoopConfig` (:7),
  `IterableSource` (:23), `ExitCondition` (:38), `FailurePolicy` (:51).
- `crates/surge-orchestrator/src/engine/stage/loop_stage.rs` —
  `execute_loop_entry` (:45), `resolve_iterable` (:103),
  `on_loop_iteration_done` (:213), `exit_condition_met` (:396).
- `crates/surge-acp/src/bridge/tools.rs:77` —
  `build_report_stage_outcome_tool` dynamic outcome enum.
- `crates/surge-orchestrator/src/engine/stage/agent.rs` — declared-outcome
  plumbing (:279-284), `on_outcome` chain runs **before** `OutcomeReported`
  persists (:495-530), `record_outcome_rejection` (:1016), artifact-contract
  rejection (:532-556, :1074).
- `crates/surge-orchestrator/src/engine/run_task.rs` —
  `write_routing_events()` emits `StageCompleted` (:1064);
  `enforce_budget()` (:217) shows the stage-boundary enforcement pattern.
- `crates/surge-core/src/run_event.rs` — `OutcomeReported` (:217),
  `StageCompleted` (:222), loop events (:249-263), `OutcomeRejectedByHook`
  (:314); `crates/surge-core/src/run_state.rs` fold (:187), outcome record
  push (:301-322), StageCompleted no-op (:544-546).
- `crates/surge-core/src/migrations/mod.rs` — `MAX_SUPPORTED_VERSION = 2`
  (:27); additive-bump precedent (:21-26); `writer_emits_max_supported_version`
  test (:189-197).
- `crates/surge-core/src/sandbox.rs` — `SandboxMode::ReadOnly` = read-only +
  no network + no shell (:42); `WorkspaceWrite` has no network (:44).
- `crates/surge-core/bundled/profiles/` — `verifier-1.0.toml`
  (workspace-write :23-24; `passed`/`failed` :29-37; prompt-only read-only
  :58), `implementer-1.0.toml` (`implemented`/`blocked`/`partial` :28-42),
  `roadmap-planner-1.0.toml` (reject hooks :60-74; prose sizing :106,:112),
  `flow-generator-1.0.toml` (roadmap binding :52-54, milestone loop
  :99-104), flows `linear-3-1.0.toml`, `bug-fix-1.0.toml`,
  `multi-milestone-1.0.toml`.
- `crates/surge-persistence/src/runs/views.rs` — `maintain()` in the append
  transaction (:27-28); per-run migration `0002_roadmap_patches.sql`
  precedent for adding a view.
- `crates/surge-persistence/src/roadmap_patches.rs` — `RoadmapPatchStore`
  (:116 upsert, :183 list, :24 filter): the registry-index template.
- `crates/surge-cli/src/commands/feature.rs:218` — `list_command()`: the
  read-only CLI template (Storage::open → typed filter → table/JSON).
- `crates/surge-core/src/validation.rs` — rules 4/5 (:532-596): where the
  new verification-reachability warnings live.

### Design notes

**Ledger state machine** (task granularity, event-sourced):

```text
pending → in_progress → ready_for_verification → done (verified)
                     ↘ failed_verification → in_progress (backtrack)
pending → skipped | aborted
```

`done` is reachable **only** via `TaskVerified`; every other transition is
`TaskStatusChanged`. The fold rejects (ignores + warns) a `TaskVerified`
whose `authority_node` lacks verification authority in the graph snapshot —
defense in depth on replay, mirroring how the engine refuses to emit it.

**Authority check placement.** The engine-side gate lives next to the
existing artifact-contract rejection in the agent stage
(agent.rs:532-556): when a node's outcome maps to a ledger `done`
transition, resolve the node's profile → require
`verification.authority = true` **and** `sandbox.mode == ReadOnly`; on
mismatch, route through `record_outcome_rejection` with a diagnostic naming
the missing capability. This reuses the retry/escalation accounting instead
of inventing a parallel failure path.

**Outcome→ledger mapping.** A node's outcome decl gains an optional
`ledger_effect` (`none` default | `ready_for_verification` | `verified` |
`failed_verification`), so the mapping is graph data — consistent with
"engine is dumb, agents are smart" (routing stays edges; ledger transitions
stay declarations).

---

## Work Breakdown

### M1 — surge-core: ledger types + artifact schema v2

1. Extend `RoadmapTask`: `depends_on: Vec<String>`,
   `discovered_from: Option<String>`, `size: TaskSize` (`S|M|L`),
   `verified: bool` (default false). Extend `RoadmapStatus` with
   `ReadyForVerification` and `FailedVerification` (additive,
   `#[non_exhaustive]` check).
2. Bump `ARTIFACT_SCHEMA_VERSION` for `roadmap`; update `to_markdown()`,
   JSON-schema snapshots, `docs/conventions/roadmap.md`.
3. Upgrade `validate_roadmap()`: unique task ids, acyclic task
   `depends_on`, `discovered_from` referential integrity, `size` required
   at schema v2 (v1 artifacts still parse; absence of v2 fields downgrades
   validation to v1 rules — no forced migration).
4. Add `ledger_effect` to `OutcomeDecl` (node.rs:88), default `none`;
   validation rule: `verified` effect requires the outcome to be declared
   on a node whose profile has verification authority (checked at
   flow-load, see M4).
5. Property tests for the new validation rules (acyclicity, references).

### M2 — events, fold, views, registry index

1. New `EventPayload` variants (R2); bump `MAX_SUPPORTED_VERSION` 2→3
   following the documented additive precedent; extend `discriminant_str`;
   update `writer_emits_max_supported_version`.
2. Fold: `LedgerState` in `RunMemory` (task id → status/verified/authority
   trail); deterministic, no wall-clock. `TaskDiscovered` inserts a pending
   task with the `discovered_from` edge.
3. Per-run migration `0003_task_ledger.sql`: `task_ledger` view
   (task_id, milestone_id, status, verified, size, depends_on_json,
   discovered_from, updated_seq), maintained in `views::maintain()` in the
   same transaction as append.
4. Registry migration `0014_task_ledger_index.sql` + `TaskLedgerStore` in
   surge-persistence (upsert/list/get with a typed filter), mirroring
   terminal task state per run/project — copy the `RoadmapPatchStore`
   shape.
5. Replay test: fold a synthetic event stream containing an unauthorized
   `TaskVerified` and assert it is ignored with a warning; golden replay
   parity for the new events.

### M3 — engine: loop binding, authority gate, discovered capture

1. Loop entry over `roadmap.milestones[*].tasks` binds the current task id
   into iteration context; `on_loop_iteration_done` appends the task's
   `TaskStatusChanged` alongside `LoopIterationCompleted` (R9).
2. Authority gate at the agent-stage outcome boundary (design note above):
   `ledger_effect = verified` ⇒ profile authority + `ReadOnly` sandbox, else
   reject via `record_outcome_rejection`.
3. `discovered-tasks` artifact contract kind + engine fold-in: on stage
   completion with a valid `discovered-tasks` artifact, append
   `TaskDiscovered` per entry (id-collision → reject artifact via the
   existing contract-rejection path).
4. Emit `TaskVerified` with the `verification-report` artifact hash as
   `evidence`.
5. Integration test: linear flow (implement → verify) over a 2-task ledger;
   assert ledger states through the run, unauthorized-verifier failure
   case, and discovered-task capture.

### M4 — profiles + flows + graph validation

1. `implementer-2.0.toml`: replace `implemented` with
   `ready_for_verification` (forward, `ledger_effect =
   "ready_for_verification"`); keep `blocked`/`partial`. Specialized
   implementers re-`extends` the new base.
2. `verifier-2.0.toml`: `[sandbox] mode = "read-only"`,
   `[verification] authority = true`, `passed` gains
   `ledger_effect = "verified"`, `failed` gains
   `ledger_effect = "failed_verification"`; required produced artifact
   `verification-report` (new contract kind: outcome, checks run, evidence
   refs). Shell allowlist moves the `cargo` invocation to read-only-safe
   commands (`cargo check`/`nextest` need target-dir writes — resolve via
   `writable_roots` for the target dir only, or document the delegation
   limit per runtime in `sandbox-matrix.md`; decision recorded during
   implementation).
3. Bundled flows (`linear-3`, `bug-fix`, `multi-milestone`) updated to the
   new profile versions and outcome names.
4. Graph validation R8: reachability pass for verification-authority nodes
   on success paths and loop bodies; render warnings in
   `surge engine validate` and the bootstrap flow-approval card. Warning,
   not error, in Phase 1 (user flows must keep validating).
5. Flow Generator prompt: instruct emitting verifier nodes with the new
   profile and never wiring implement-outcome directly to terminal success.

### M5 — CLI + planner sizing + docs

1. `surge ready` (new `commands/ready.rs`, registered in
   `commands/mod.rs` + `main.rs` dispatch): unblocked-task query against
   `TaskLedgerStore` with `--json`, `--project`, `--discovered`,
   `--all-runs`; table modeled on `print_patch_table`.
2. `surge ledger show <run|project>`: full ledger with dependency and
   discovered-from edges (tree rendering).
3. Roadmap Planner prompt (roadmap-planner-1.0.toml → 2.0): one-session
   sizing rule + split rule + task-level `depends_on`; strengthen the
   existing `on_outcome` reject hooks to run the upgraded validator.
4. Docs: `ARCHITECTURE.md` §3 (ledger_effect on outcomes), §4 (new events),
   §11 (new view/table); `workflow.md` (ledger lifecycle diagram);
   `cli.md`; `conventions/roadmap.md` + new
   `conventions/verification-report.md` + `conventions/discovered-tasks.md`;
   flip the Phase-1 row in `product-strategy.md` sequencing table to
   in-progress/done as milestones land.

---

## Testing & Acceptance

- Unit: roadmap v2 validation (acyclicity, refs, size), fold transitions,
  authority rejection in fold, store filters.
- Integration: M3.5 end-to-end flow; replay-at-seq over a ledger run shows
  correct ledger state at every seq; fork-from-here preserves ledger state
  in the child; crash-recovery resume mid-loop re-derives ledger from fold.
- Golden: artifact-contract goldens for `roadmap` v2,
  `verification-report`, `discovered-tasks`.
- Gates: `cargo build --workspace`, `cargo nextest run`,
  `cargo clippy --workspace --all-targets -- -D warnings`.
- Acceptance: a multi-milestone bootstrap run produces a v2 ledger;
  `surge ready` answers correctly mid-run; a task can only reach `done`
  through a sealed verifier; a flow whose success path lacks verification
  shows the validation warning; discovered work appears in
  `surge ready --discovered` with its `discovered_from` edge.

---

## Risks

- **Sealed verifier vs. real test runs.** `ReadOnly` forbids shell; `cargo
  nextest` writes to `target/`. The M4.2 decision (scoped
  `writable_roots` vs. runtime delegation limits) is the riskiest design
  point — it must not quietly re-open the sandbox. Mitigation: record the
  decision as an ADR; `surge doctor matrix` must report the effective
  verifier confinement per runtime.
- **Event-schema bump ripple.** v3 must keep old logs replayable
  (additive-only, tested by the migration chain suite).
- **Ceremony backlash** (the Kiro lesson): bug-fix archetype keeps a
  1-task ledger with no extra fields required beyond `size` — validation
  scales with archetype.
- **Profile-version churn** for users extending `implementer@1.0` /
  `verifier@1.0`: old versions keep resolving (bundled fallback lanes);
  release notes document the migration.
