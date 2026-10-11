---
title: "feat: verifier rejection ladder (v1 task 1.1)"
type: feat
status: completed
date: 2026-10-05
---

# feat: verifier rejection ladder (v1 task 1.1)

## Summary

When an implementer → verifier loop runs out of attempts, the run should climb
a ladder instead of failing: retry on a different allowed agent, then let a
planner split the task using the verifier's findings, then ask a human. Part of
the [v1 plan](2026-10-05-002-feat-v1-release-plan.md), phase 1.

## Current state (code map, 2026-10-05)

- `ExceededAction::Escalate` (the default) only re-routes through a declared
  `max_traversals_exceeded` outcome edge (`route_after_max_traversal`,
  `surge-orchestrator/src/engine/run_task.rs`). No shipped flow declares one,
  so every exhausted loop in `linear-3`, `bug-fix`, `refactor` and
  `multi-milestone` fails the run.
- A re-entered implementer gets no feedback. Shipped implementer nodes bind only
  `spec`; the verifier's `verification-report` artifact and outcome summary stay
  in the run log. Retries are blind.
- Per-attempt agent override exists only for quota fallback (`quota_opening`,
  frozen candidates) and only for persistent work items.
- `FailurePolicy::Replan` is a stub; roadmap patches can add or replace draft
  items between iterations.
- Gates raised without a declared node exist (`SkillTrust`, agent
  `request_human_input`). `EscalationRequested` is informational and has no
  max-traversal cause.
- Parallelism is per run (one work item attempt per run), so a ladder is per
  run; other work items keep running by construction.

## Steps

| Step | What | Done when |
|---|---|---|
| A | **Done.** **Feedback on re-entry.** A stage entered through a backtrack edge gets a "Feedback from the previous attempt" section before its prompt: the source stage's outcome and summary, and the failed or skipped checks of a `verification-report` it produced. Engine-level, so user flows benefit without new bindings. | Fold and prompt tests; the section is absent on a first entry. |
| B | **Done.** **Human rung as the default end.** `Escalate` with no declared escalation edge reaches a default `HumanGate` (retry / stop; accept-as-is and revise arrive with 1.2) instead of failing. | `engine_escalation_gate_test`: exhausted loop asks; retry runs one more attempt with the verifier's findings; stop fails cleanly; a restarted host reissues the gate; the journal stays trusted. |
| C | **Done.** **Retry on another agent.** Before the human rung, one extra attempt of the loop's agent, on `[escalation] retry_agent` (optional `retry_model`) or on its own agent when unset. | Derived extra-attempt edge and `escalation_exhausted` chain in core and engine; `engine_escalation_gate_test` runs the extra attempt with findings before the gate, also across a restart; config transform and detection unit-tested. |
| D | **Done.** **Planner split.** In loop bodies, after the extra attempt, a split planner replaces the task with smaller tasks in the same loop and run; applied at once (maintainer decision), no roadmap patch. Elsewhere the ladder goes straight to the human rung. | `engine_escalation_gate_test::an_exhausted_task_is_split_and_its_subtasks_run_in_its_place`: ladder to the split, subtasks run in place with fresh budgets, the next original task follows, no gate, journal trusted. |

Steps run in order; each is independently shippable and keeps all current
fail-closed behaviour for `ExceededAction::Fail`.

## Decisions

Step B, 2026-10-05:

- The gate is an ordinary `HumanGate` in a *derived* graph
  (`surge_core::escalation::with_default_escalation_gates`), applied by the
  engine's routing graph, the run-state fold and operator views. The persisted
  graph, its hashes and frozen task graphs are unchanged, and the existing
  gate recovery, Inbox, CLI, MCP and desktop paths work without changes. A
  host-raised gate on the exhausted agent stage was rejected: it would add a
  second outcome to that stage's occurrence and conflict with gate recovery.
- No new event or schema bump: `EdgeTraversed { kind: Escalate }` plus the
  gate's request and answer are the record. `EscalationRequested` stays for
  causes without a gate.
- Retry grants one more attempt and does not reset the exhausted counter, so
  each further rejection asks again rather than silently spending a new
  budget.
- The gate has no practical deadline (`timeout_seconds = u32::MAX`): an
  unattended run waits for the operator instead of failing at night.

Step C, 2026-10-05:

- The extra attempt is a derived `escalate` edge back to the loop's agent,
  capped at one traversal and labelled
  `surge_core::escalation::ALTERNATE_ATTEMPT_LABEL`. When it is spent, routing
  takes `escalation_exhausted` to the gate. `route_selection::resolve_stage_route`
  (replayed by the journal inspector) and the engine climb the same chain.
- The rung is always in the graph; the agent it runs on is configuration. A
  config-dependent skip would make a journal disagree with its own replay.
  Without `retry_agent` the extra attempt runs on the stage's agent with the
  findings, which is still one automatic try before a human is asked.
- The capacity `rotation_profile` is not the default: it means another account
  of the same runtime, not another agent. Picking an installed agent
  automatically belongs to v1 task 1.4.
- The override lives in `EngineConfig` (copied from `SurgeConfig::escalation`,
  unknown agents dropped with a warning), so a resumed run keeps it, and the
  occurrence is recognised from the durable `RunMemory::entered_via` fold.
  Stages under a frozen quota plan keep their planned runtime.

- Feedback is injected by the engine (like steering and the interrupted-call
  notice), not through new flow bindings, so existing and user flows get it.
- A ladder never runs the same rung twice for one exhausted edge; the human
  rung is always reachable.
- Records say what happened ("retried on profile X", "split into N tasks",
  "accepted by a human") and never present a human acceptance as verified.

Step D design, 2026-10-05 (the maintainer chose: a split applies at once, no
approval; the user is notified and the gate still follows if subtasks fail):

- **Per-iteration counters.** Loop traversal counters were created at loop
  entry and shared by every item, so one task inherited another's spent
  retries. They now reset at each `LoopIterationStarted` (engine) and at each
  re-entry of the same loop (journal inspector), so every task gets its own
  ladder.
- **Rungs as an ordered list.** `escalation::ESCALATION_RUNGS` =
  `max_traversals_exceeded`, `escalation_exhausted`, `split_exhausted`. Routing
  tries the next rung whenever the current escalation edge is itself exhausted
  (core `resolve_stage_route` and the engine, identically). In a loop body with
  an agent target the derived ladder is: extra attempt (cap 1) → split planner
  (cap 1) → gate. Outside loops: extra attempt → gate. Non-agent target: gate.
- **Split planner.** A derived agent node (`task-splitter@1.0`, new bundled
  profile) with outcomes `split` (requires `discovered-tasks.toml`, now with
  optional `acceptance_criteria`) and `cannot_split` (→ gate). It sees the task
  item and, through the escalation feedback, the verifier's findings.
- **Splice.** Routing `split` commits, in the same atomic route batch as the
  snapshot, a new `LoopItemsSpliced` event (schema v18) and inserts the new
  task items right after the current item of the innermost loop frame. The
  split node then reaches a success terminal, so the iteration ends without a
  failure and the loop continues with the first subtask. `TaskDiscovered`
  events (already emitted for `discovered-tasks`) link each subtask to the
  replaced task. The roadmap artifact is not rewritten and `RoadmapUpdated` is
  not emitted, so prior verification stays valid.
- **Implementation notes.** The planner's tasks come from the existing
  `discovered-tasks` contract (so `TaskDiscovered` links them to the replaced
  task), now with optional `acceptance_criteria`. The persistence writer accepts
  one leading `TaskSplit` in a stage route batch. A roadmap patch was not used:
  a mid-run patch cannot feed the running loop (it only appends nodes after the
  outer terminal or needs a follow-up run) and `RoadmapUpdated` clears all
  verification.
- **Follow-up.** The maintainer asked (2026-10-05) for a step/task model in which
  a step node owns its tasks and each task its agent, session and PR; an ADR
  follows this plan. `TaskSplit` is named for the domain so it survives that
  change.
