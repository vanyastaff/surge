+++
status = "proposed"
deciders = ["vanyastaff"]
date = "2026-10-05"
+++

# ADR 0022 — Steps own their tasks; tasks own agent, session and PR

## Status

Proposed on 2026-10-05, at the maintainer's request after the verifier
rejection ladder ([plan](../plans/2026-10-05-003-feat-verifier-rejection-ladder-plan.md))
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

1. **A step is a first-class run entity.** A step has an id, title, order,
   status and a list of tasks. Roadmap milestones become steps when a run is
   materialized. The graph keeps describing *how* work is done (the per-task
   pipeline: spec → implement → verify, gates, terminals); steps and tasks
   describe *what* is done.
2. **A task is a first-class run entity owned by a step.** A task has an id,
   title, description, acceptance criteria, status, dependencies, and:
   - `assignment` — the agent (runtime, model, effort) chosen for it, which the
     planner proposes and the user may edit before approval, and which the
     ladder may change for one attempt;
   - `attempts` — each attempt's session(s), outcome, findings and agent;
   - `pr` — the review branch or GitHub PR for its verified result (v1 1.9);
   - `children` — tasks it was split into (the ladder's split rung), with the
     parent marked *replaced*.
3. **Events are task-scoped.** Task lifecycle events carry the task id
   (`TaskStarted`, `TaskAttemptStarted { agent }`, `TaskSplit`, `TaskVerified`,
   `TaskPrOpened`, `TaskAcceptedByHuman`). The fold builds a `Steps { tasks }`
   view; reports, the desktop dashboard and `surge mcp serve` read that view,
   not loop frames.
4. **Loops iterate the step/task view, not a frozen artifact copy.** The task
   loop asks the fold for the next runnable task of the current step, so a
   split, a new idea placed into a step (v1 1.8) or a re-run of a task is a
   task-view change, not a frame splice.
5. **Persisted graphs stay immutable**; the derived routing graph
   (`surge_core::escalation`) stays the mechanism for engine-added rungs.

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

## Open questions for the maintainer

1. Is a step exactly a roadmap milestone, or can the planner group tasks into
   steps differently from milestones?
2. Should the per-task pipeline stay one shared subgraph, or may a task carry
   its own methodology (for example TDD adds a test-author step, v1 1.6)?
3. Is a PR per task or per step when tasks in a step are small?
4. Should several tasks of one step run in parallel when they do not depend on
   each other (the product decisions say independent tasks keep running)?

## Consequences

- The v1 dashboard, PR per task and new-ideas work build on one view instead
  of four sources.
- Phase 1 is additive and fits v1; phases 2–3 change engine control flow and
  can follow v1 without blocking the release.
