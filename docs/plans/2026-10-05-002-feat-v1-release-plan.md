---
title: "feat: first release (v1, macOS desktop) task plan"
type: feat
status: draft
date: 2026-10-05
---

# feat: first release (v1, macOS desktop) task plan

## Summary

Ordered tasks for the v1 cut recorded in
[product strategy](../product-strategy.md#product-decisions--2026-10-05):
a reliable core plus the project dashboard, new ideas during work, the
verifier ladder with loop protection, and agent switching on limits. Engine
work comes before the desktop surfaces that render it; every desktop action is
also exposed through `surge mcp serve`. Release stays NO-GO until phase 4
passes.

Current-state notes come from a read-only code map on 2026-10-05; each task
starts by confirming them, because a mapping pass can miss existing code.

## Phase 0 — unblock the release

| # | Task | Depends on | Done when |
|---|---|---|---|
| 0.1 | **MCP restart recovery** ([ADR-0021](../adr/0021-mcp-restart-recovery.md), [plan](2026-10-05-001-feat-mcp-restart-recovery-plan.md)): group termination, best-effort cleanup record, restart from frozen manifest, outcome-unknown notice for interrupted calls. | — | **Done 2026-10-05** (CI run 37391960503 on `3e5db7d`, macOS and Ubuntu green). |
| 0.2 | **CI scope for the macOS release.** Decided 2026-10-05: Windows Clippy and test jobs are advisory (`continue-on-error`, named "advisory"); Windows compile errors are being fixed as they appear; the ~60 Unix-dependent Windows test failures stay advisory until Windows support after v1. | — | Done in CI; Windows failures stay visible but no longer fail the workflow. |

## Phase 1 — engine foundations (no UI)

| # | Task | Current state | Done when |
|---|---|---|---|
| 1.1 | **Verifier rejection ladder** ([plan](2026-10-05-003-feat-verifier-rejection-ladder-plan.md); steps A (feedback on re-entry) and B (default human gate) done). Implement `ExceededAction::Escalate` behind `EdgePolicy.max_traversals`: retry on a different or stronger allowed model, then a planner split using the verifier's findings, then a human gate. Independent tasks keep running. | `Escalate` declared in `surge-core/src/edge.rs`, runtime semantics reserved. | Engine tests cover each rung, dependency-aware continuation and event replay. |
| 1.2 | **Human override of a rejection.** "Accept as is" (recorded and rendered as accepted by a human, never verified, findings kept) and "revise requirement and re-verify". | Missing. | Ledger, run report and fold distinguish the three outcomes. |
| 1.3 | **Loop protection.** No progress (no new events), repeated identical tool calls or a turn cap counts as a failed attempt and enters 1.1. Defaults in `surge.toml`. | Missing. | Mock-agent tests for each trigger. |
| 1.4 | **Agent switching on limits.** Make capacity `Rotate` reachable: move a parked task to another allowed agent; park only when none fits. Keep the verifier on a different vendor than the implementer where possible, warn otherwise. | Parking and wake complete (ADR-0016); `Rotate` unreachable. | Rate-limit fixture switches agents; no allowed agent parks and wakes. |
| 1.5 | **Model, effort and thinking restrictions.** Disable providers or models, allow specific models, effort levels and thinking modes; enforced by the planner and validated at flow load. | Profile overrides exist; no restriction layer. | Flow load rejects a forbidden assignment with a clear diagnostic. |
| 1.6 | **Clarifying questions before the description.** 3–5 questions with defaults at bootstrap start; methodology question (TDD or not) in the agents-and-flow step; TDD puts a test-author first. | Bootstrap goes straight to the description; triage has `Unclear { question }`. | Bootstrap e2e with answers bound into the description stage. |
| 1.7 | **Verifier test-coverage check.** The verifier checks that tests cover the stage's criteria, not only that they pass. | Verifier owns `verified`. | Fixture where green tests miss a criterion is rejected. |
| 1.8 | **New idea in a live project.** Short path: 1–3 questions, a proposed roadmap placement (task in a stage or new stage) as a roadmap patch, one-step approval; the current stage keeps running. Small bugs skip questions; feedback on a finished stage uses the same path. | Roadmap patches apply between runs; no live-project path. | Idea submitted during a running stage lands as an approved patch without interrupting it. |
| 1.9 | **PR per task.** Open a GitHub PR per verified task; without a GitHub remote, a local review branch. Dependent tasks stack on unmerged PRs; auto-merge as a project setting (default off). Conflicts with the user's commits are resolved by an agent, then re-verified, else the ladder. | Worktrees and a label-driven auto-merge gate exist; no PR creation or stacking. | Mock GitHub test for create, stack, auto-merge and conflict re-verification. |
| 1.10 | **Stop and restart against a revised requirement.** Stopping keeps the worktree; restart continues from it. The same mechanism powers "go back here" on a history step. | Steer/stop exist; restart-from-worktree with a revised requirement not confirmed. | Restart test keeps prior code and binds the revised requirement. |

## Phase 2 — desktop

| # | Task | Depends on | Done when |
|---|---|---|---|
| 2.1 | **Planning wizard:** idea → questions → description → stages → agents and flow → start, with back/edit/next on each step; bootstrap gates move here from the Inbox. | 1.5, 1.6 | Full bootstrap through the wizard on the bundled app. |
| 2.2 | **Project dashboard** as the project start screen: "needs your decision" (questions, PRs waiting to merge, ladder escalations), stages with progress, recent activity, "add an idea", "run the app". | 1.1, 1.8, 1.9 | Real daemon state only; empty and busy states designed. |
| 2.3 | **Work view:** one line per task (who, step, time), expandable live agent feed; stop and restart; run history timeline with "go back here". | 1.10 | Live feed holds up with several parallel agents without input lag (GPUI render cost). |
| 2.4 | **Decision cards:** ladder human step, accept as is / revise requirement, idea placement approval, PR review (local and GitHub). | 1.1, 1.2, 1.8, 1.9 | Each card resolves through the same daemon path as CLI/MCP. |
| 2.5 | **Agent onboarding and selection:** supported agents, copyable install/sign-in commands, auto-advance when `surge doctor` sees an agent ready; assignment editing and restrictions. | 1.5 | First launch on a clean Mac reaches a ready agent without a terminal paste-back. |
| 2.6 | **Notifications and keep-awake:** native macOS notifications from every project; power assertion with a visible indicator while work runs. | — | Notification on a question and on stage end; Mac stays awake during a run and sleeps after. |
| 2.7 | **Projects:** opens the last project, switcher with "needs decision" badges, Fleet scoped to the current project; parallel projects share agents equally (priorities after v1). | — | Two projects run side by side with correct badges. |

## Phase 3 — MCP surface parity

| # | Task | Depends on | Done when |
|---|---|---|---|
| 3.1 | Add tools for ideas, decision cards (ladder, override, placement), PR merges and project switching to `surge mcp serve`; writes stay behind `--allow-write`. | Phase 1 | Each new desktop action has a matching MCP tool with tests. |

## Phase 4 — prove the promise and release

| # | Task | Done when |
|---|---|---|
| 4.1 | Run real multi-stage projects (a new idea and an existing repository) through the bundled desktop app; fix every failure found. | Several projects reach their stages with verified tasks and no manual repair. |
| 4.2 | Release gates: strict Clippy, nextest, MSRV, packaging contract, macOS CI green, fresh release candidate and provenance. | Independent GO verdict recorded in release readiness. |

## After v1

Confirmed by the maintainer on 2026-10-05: master agent proposing viewpoint nodes, judge
agent, skills/MCP Hub, personal memory and code index, three customization
paths, benchmark ratings, cross-project priorities, Telegram in onboarding,
budget UI, Windows and Linux.

## Open questions

- What "run the app" does for an arbitrary stack (a run command recorded by the
  planner per project, or per stage).
