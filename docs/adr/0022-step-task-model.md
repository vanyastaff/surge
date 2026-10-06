+++
status = "proposed"
deciders = ["vanyastaff"]
date = "2026-10-05"
+++

# ADR 0022 — Steps own their tasks; tasks own agent, session and PR

## Status

Proposed on 2026-10-05, at the maintainer's request after the verifier
rejection ladder; decisions refined in a maintainer interview the same day ([plan](../plans/2026-10-05-003-feat-verifier-rejection-ladder-plan.md))
showed the cost of the current shape. Not implemented. Open questions are listed
at the end and need the maintainer's answers before acceptance.

## Context

The product promise is that large projects reach the end in stages the user can
see ([product strategy](../product-strategy.md)). The v1 dashboard shows stages
with progress, who works on what, and a PR per task. Today that information is
scattered:

| Concept | Where it lives now |
|---|---|
| Stage / milestone | Roadmap artifact (`roadmap.toml`) items, iterated by a `Loop` node (`multi-milestone`: `milestone_loop`) |
| Task | Roadmap task items, iterated by a nested `Loop` (`task_loop`); frozen into `LoopFrame.items` at loop entry |
| Task status | `RunMemory.ledger`, folded from `TaskStatusChanged` / `TaskVerified` / `TaskDiscovered`; the roadmap file's own `status` is never updated during a run |
| Who works on it | `AgentConfig` on each graph node (profile plus runtime override); one config for every task the loop runs |
| Session | `RunMemory.provider_sessions`, keyed by stage invocation, not by task |
| PR | Does not exist (v1 task 1.9) |
| Steps a task goes through | A subgraph (`task_body`: spec → implement → verify) shared by every task |

Consequences seen while building the ladder:

- Splitting a task needed a new `TaskSplit` event that splices loop items,
  because a task is an anonymous loop item, not an entity the run can grow.
- A per-task agent switch needs a derived edge marker and an `entered_via`
  fold, because the agent is a property of the graph node, not of the task.
- Retry budgets were per loop, not per task, until fixed.
- The dashboard would have to join four sources (roadmap artifact, loop
  frames, ledger, sessions) to show one task.

The maintainer proposed:

```
Node {
  metadata: { type: step, ... },
  tasks: [ Task { ... } ],
  agent, session, pr, ...
}
```

## Decision (proposed)

Answers from the maintainer interview of 2026-10-05 are marked *(maintainer)*.

1. **Everything is a task, and tasks form a tree** *(maintainer)*. A project, a
   stage, a milestone and a subtask are the same entity:
   `Task { id, title, description, criteria, status, tasks: [TaskRef], … }`.
   Any task can be split into smaller tasks at any depth. "Step" and
   "milestone" are only names for levels of the tree in the UI.
2. **Each task has its own pipeline** *(maintainer)*: the ordered stages it goes
   through (for example `research → planner → builder → verify → tester`). The
   planner composes it from allowed node kinds when the task is planned; the
   engine validates it at load and the user sees and may edit it before
   approval. A task without its own pipeline uses the project default.
3. **A planner stage creates child tasks of its own task** *(maintainer, JWT
   example)*. When a task's pipeline reaches its planner and the planner breaks
   the work into ten steps, those steps become real child tasks of that task,
   visible with their own status, not text inside a plan. Later stages of the
   parent work through the children in order (a fresh session per child, each
   child marked done on its own). The parent's verifier checks every child and
   the parent's own criteria, so splitting can never drop a requirement. The
   ladder's split rung (`TaskSplit`) is the same operation, triggered by
   repeated rejection instead of by the plan.
4. **A task owns its execution record**: `assignment` (agent, model, effort;
   proposed by the planner, editable before approval, changed by the ladder for
   one attempt), `attempts` (sessions, outcome, findings, agent per attempt) and
   `pr` (one PR per task, as decided in the 2026-10-05 product interview).
5. **Guards on splitting**: a maximum depth, and a split is accepted only when
   the children are smaller than the parent by the planner's size estimate.
6. **One run per root task**; every task attempt opens a fresh session
   (product decision: context inside a task is the runtime's job). Independent
   sibling tasks run sequentially in v1; parallel siblings come after v1
   *(maintainer)*. Dependencies are stored from the start.
7. **Events are task-scoped** (`TaskPlanned { parent, children }`,
   `TaskAttemptStarted { task, agent }`, `TaskSplit`, `TaskVerified`,
   `TaskPrOpened`, `TaskAcceptedByHuman`). The fold builds the task tree; the
   desktop, reports and `surge mcp serve` read the tree, not loop frames.
8. **Flows without a roadmap** (`linear`, `bug-fix`) are a root task with one
   level, so every run has the same shape on the dashboard.
9. **Persisted graphs stay immutable**; engine-added rungs stay in the derived
   routing graph (`surge_core::escalation`).
10. **View: task tree with drill-down** *(maintainer, after reviewing a mockup
    of both views on the JWT example, 2026-10-05)*. The desktop shows the task
    tree with status and progress; selecting a task shows its own pipeline, the
    ladder state, agent, criteria, PR and attempts. The single large graph with
    collapsible task groups was the rejected alternative.

## Migration

- **Phase 1 (with v1 1.9 PR per task):** add the fold view
  `RunMemory.steps` built from existing events (`LoopIterationStarted`, ledger
  events, `TaskSplit`, sessions by stage occurrence inside a task iteration).
  No new storage; the dashboard (2.2) reads it. Task-scoped `TaskPrOpened`
  lands here.
- **Phase 2:** record the assignment and attempts per task
  (`TaskAttemptStarted { task, agent }`); the ladder's agent switch writes the
  task assignment instead of relying on the derived edge marker.
- **Phase 3:** the task loop reads the next task from the view (decision 4);
  `TaskSplit` stops splicing frames and becomes a pure task-view event. New
  ideas placed into a running step use the same path.

Each phase is a schema bump with identity migrations; old journals keep
replaying through the loop-frame path.

## Alternatives

- **Keep tasks as loop items and add more frame events.** Cheapest per feature,
  but every task-level feature (PR, assignment, ideas, dashboard) repeats the
  splice-and-join work.
- **Make every task its own graph node.** Graph size grows with the roadmap,
  every split or idea becomes a graph revision, and frozen graph hashes change
  constantly.
- **One run per task (work items).** Already exists for persistent tasks; it
  isolates sessions well but loses the step-level view and the in-run ladder.

## Open questions

- Size estimate used by the split guard (planner-declared size, or measured
  from history as the product strategy suggests for compaction signals).

## Consequences

- The v1 dashboard, PR per task and new-ideas work build on one view instead
  of four sources.
- Phase 1 is additive and fits v1; phases 2–3 change engine control flow and
  can follow v1 without blocking the release.
