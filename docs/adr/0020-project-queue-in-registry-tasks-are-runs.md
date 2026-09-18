+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-09-18"
+++

# ADR 0020 — The project queue lives in the registry; tasks are runs

## Status

Accepted.

## Context

The product goal recorded in `docs/product-strategy.md` (revision 2026-09-18)
is a task list that executes on its own: the user approves a roadmap, then
Surge dispatches each task with its own flow, folds in work that arrives from
GitHub, MCP or the user, and lets the operator pause, reprioritise or edit a
flow without restarting. Today one flow is chosen once per run and a static
loop node walks the roadmap in file order; intake starts runs of its own that
never touch the roadmap; the only cross-run view of tasks is the ledger index,
which is mirrored after a run finishes.

The question this ADR settles is **where the queue lives and what a task is
at runtime.** Four shapes were weighed against the code on
`feat/ui-consumer-readiness` @ 6905d00; the spec
(`.rust-studio/specs/autonomous-task-orchestration/spec.md`) records the full
comparison.

1. **A long-lived project run that parents one child run per task.** Reuses
   run machinery on paper; in the tree it does not survive. Daemon admission
   is FIFO with a default of eight slots and no preemption
   (`crates/surge-daemon/src/main.rs:27`), so a parent holding a slot while
   its child waits for one deadlocks. Recovery rule 6 flags a run idle for
   more than 24 h as stuck (`recovery.rs:195`), which a parent waiting on a
   long child always is. `merge_run_worktree` had no production caller, and
   the only way to stop a run was `stop_run` → `Aborted`. A queue inside one
   run's log also puts project-scoped truth into run-scoped storage: two
   parents on one repo are two queues.
2. **One run whose loop body is regenerated per iteration.** Graph revisions
   exist (`GraphRevisionAccepted`), but they are applied only when the frame
   stack is empty (`run_task.rs:2009`), never inside a loop; the whole
   project would share one worktree against the isolation rule; and the
   engine is single-threaded per run, which would make parallel tasks
   impossible for good.
3. **A daemon-level scheduler over registry state, one ordinary run per
   task.** The queue is cross-run state, and the registry already holds
   cross-run state that is not event-sourced (`runtime_capacity` in
   migration 0015, `runs.wake_at` in 0016, `roadmap_patch_index` in 0006).
   Every seam it needs exists: `TicketRunLauncher`, `AdmissionController`,
   `recover_on_startup`, `bootstrap_parent` seeding, the ADR-0015 gate.
4. **A per-project event journal** with the registry table as its fold. The
   principled end state, but a second log kind with its own writer and fold
   is the wrong first step.

Pre-code maintainer verdict (chief-architect, harsh-critic, 2026-09-18):
RESHAPE NEEDED on shape 1 as briefed; ACCEPTABLE on shape 3 after the reshapes
now in the spec (registry per run, index as a hash-keyed mirror, load-time
trust for `.surge/`, verifier rule as an error for task flows).

## Decision

**The project task queue is registry state that mirrors `.surge/roadmap.toml`;
the ordering policy is a pure function in `surge-orchestrator`; the daemon owns
only the dispatch tick; every task is an ordinary run. There is no project run
and no mid-run graph mutation.**

- `.surge/roadmap.toml` in the repository is the planning truth: `depends_on`,
  `size`, `priority`, an optional pinned flow. Git is its history.
- The registry `task_queue` table is a mirror keyed by the roadmap file's
  hash plus the execution truth the file cannot hold: dispatch state, run id,
  attempt, enqueue time, skipped-dispatch count. A changed hash re-mirrors the
  planning columns; a compare-and-set on `dispatch_state` makes a double tick
  harmless.
- `surge_orchestrator::scheduler::QueuePolicy::next(&[QueueEntry], now)` is
  the only place that orders tasks: dependencies first, then priority with
  aging, then size, then age. The daemon never sorts.
- The daemon's `TaskScheduler` tick dispatches through `TicketRunLauncher`; a
  task run carries `RunOrigin::Task { project_root, task_id, attempt,
  roadmap_hash }` in its `RunConfig` so it knows its origin without the
  daemon. After the verifier owns `verified`, the daemon merges the run
  branch into the project branch before the next dispatch.
- Pause is a flag on the project's queue row checked at every tick; a running
  task finishes its current stage boundary, as `surge steer` already does.

## Rationale

1. **A task run is a run.** Isolation, admission, crash recovery, replay, the
   run report and the inbox all work per run today; making a task a run
   inherits every one of them at zero new machinery, and makes "N tasks in
   parallel" a change in how many rows the policy returns.
2. **Project truth in project storage.** The queue is a fact about a
   repository, not about a run. The registry is where Surge already keeps
   facts that outlive a run; the roadmap file is where the user already
   edits the plan. Neither place is new.
3. **A pure policy is a testable policy.** Ordering, aging and blocked-by-
   failed semantics are property-tested without a daemon, the same way
   `recovery::decide_action` is tested with an injected clock.
4. **Nothing forbidden is touched.** The `NodeKind` enum stays closed, the
   graph stays immutable inside a run, and the one new event variant
   (`ComposedArtifactInstalled`) takes the schema bump the rule demands.

## Consequences

- `ProfileRegistry` becomes per-run (`EngineRunConfig.project_layer`), so a
  daemon serving two repositories never leaks one's `.surge/profiles/` into
  the other. `EngineConfig.profile_registry` stops being the engine-wide
  source.
- `merge_run_worktree` gains its first production caller; a merge conflict is
  a task failure with an escalation, never an automatic resolution.
- `surge ready` becomes dependency-aware and stops reporting the worktree as
  the project path; the `task_queue` row records the origin repository.
- Priority edits are a file write plus a re-mirror. A dispatch that raced an
  edit is visible after the fact through the `roadmap_hash` on the run's
  origin, not prevented.
- The queue is not event-sourced. `surge task` edits leave no log line of
  their own; the audit trail is the git history of `.surge/roadmap.toml` and
  the `RunOrigin` of each dispatched run.

### Accepted costs and mitigations

| Cost | Mitigation |
|---|---|
| Queue edits have no event log | `.surge/roadmap.toml` is in git; `RunOrigin` pins the hash each run was dispatched under; the table is kept narrow so a per-project journal (shape 4) can back it later |
| A third tick loop in the daemon | `TickLoop` is extracted and shared with `wake_scheduler` / `snooze_scheduler`, as their own module comment asks on the third copy |
| Sequential dispatch in v1 | `QueuePolicy::next` returns a `Vec`; N-wide dispatch is a config change once worktree merges are proven |
| Trust-on-first-use prompts on every fresh clone of a repo with `.surge/` | A file in git was never installed; the prompt is the honest answer, and artefacts Surge composes itself are pinned by the gate that installs them |

## Out of scope

- Model routing per role and rate-limit fallback (separate spec).
- Memory retrieval per task and write-back (separate spec).
- Per-task token budget with auto-splitting (separate spec).
- Parallel task dispatch; the design permits it, v1 does not enable it.
- A per-project event journal; recorded as the successor to this ADR's
  registry table, not as part of it.

## Revisit conditions

- Operators need an audit trail of queue edits that git history and
  `RunOrigin` cannot answer — then shape 4 (the per-project journal) replaces
  the table, with the table becoming its fold.
- Parallel dispatch shows that merge-after-verify into one project branch
  serialises tasks in practice — then the merge strategy, not this decision,
  is revisited.
- Admission grows aging or preemption — that removes one of the three
  reasons shape 1 was rejected, but not the other two.
