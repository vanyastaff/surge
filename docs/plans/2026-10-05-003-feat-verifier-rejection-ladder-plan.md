---
title: "feat: verifier rejection ladder (v1 task 1.1)"
type: feat
status: in-progress
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
| D | **Planner split.** In roadmap flows, after the agent retry, a planner drafts a roadmap patch that replaces the task with smaller ones, using the findings; the patch follows the normal approval policy. Flows without a roadmap skip to the human rung. | `multi-milestone` fixture: exhausted task becomes an approved split and the loop continues. |

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
