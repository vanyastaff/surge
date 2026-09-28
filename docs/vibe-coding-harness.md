# Surge as a developer Vibe Coding Harness

Status: product target and source audit, 2026-09-27 (America/Chicago).
Implements the user's direction: an understandable, convenient and effective
tool for creating applications, with Agentlas-OS and Factory as competitive references.
This supplements [product strategy](product-strategy.md); it does not retire
roadmap requirements or declare Surge finished.

## The outcome

A developer describes an application or a change, reviews the important
assumptions, and receives a working result with inspectable changes and evidence.
Surge owns the work across agent sessions and interruptions. The interface makes
the next useful action apparent without requiring knowledge of graphs, ACP,
materialized views, or individual orchestration stages.

The primary journey is:

1. Create an application or open a repository.
2. Describe the desired result; attach relevant context.
3. Review scope, acceptance checks, chosen agent and budget. Ask only questions
   whose answers would change the implementation; preserve answered decisions.
4. Start work and see the current activity, completed requirements and decisions
   that need attention. Steering must survive disconnects and restarts.
5. Open the result, inspect changes, and see exactly which checks passed, failed,
   or were not run. For a web application, a working preview is part of this step.
6. Request a revision or prepare a PR. Publishing is a distinct explicit action.

## What was inspected in Agentlas

Source snapshots: Agentlas-OS `4ca8fd28d5ad9a92cbe33b39d05bcff8febe7448`;
Agentlas Desktop `ac487b729224084420b8d0f75ba88f2a3e3e80db`.
Files were read at these revisions. Neither application was installed or executed
for this comparison. Source-level contracts and test cases are evidence of design
and implementation, not evidence that every shipped workflow passes.
Search found third-party listings that repeat the README, but no independent
usability benchmark suitable for claiming either product is better.

| Area | Source evidence | Implication for Surge |
|---|---|---|
| Start work | Desktop `createWorkStart` persists a project/chat/task intent transactionally, validates the selected runtime, and uses an intent digest and claim token to prevent conflicting reuse. Actual execution admission is a separate settlement. [Source](https://github.com/agentlas-ai/agentlas-desktop/blob/ac487b729224084420b8d0f75ba88f2a3e3e80db/electron/work-start.ts) | Starting twice or reconnecting must not create duplicate work. “Queued,” “running,” and “complete” must describe different observed states. |
| Onboarding | Desktop has runtime installation/login/detection and visible progress/error handling in its first-run component. This audit did not exercise these operations. [Source](https://github.com/agentlas-ai/agentlas-desktop/blob/ac487b729224084420b8d0f75ba88f2a3e3e80db/renderer/components/WorkFirstRunOnboarding.tsx) | Setup should detect available agents, explain missing prerequisites and offer the next action. A folder picker alone is not application creation. |
| Clarification | The builder contract records assumptions and requires host-observed interview evidence. It also prescribes 8–12 initial questions and repeated clarity scoring. [Source](https://github.com/agentlas-ai/Agentlas-OS/blob/4ca8fd28d5ad9a92cbe33b39d05bcff8febe7448/contracts/builder-interview-research-gate.md) | Keep durable decisions and distinguish human answers from assumptions. Test whether a shorter, adaptive interview reduces effort for ordinary coding changes; do not copy a fixed questionnaire. |
| Results | The receipt schema separates outcome from verification, including `unverified`, verifier identity, evidence references, duration, tokens and retries. The schema alone does not prove independent verification. [Source](https://github.com/agentlas-ai/Agentlas-OS/blob/4ca8fd28d5ad9a92cbe33b39d05bcff8febe7448/schemas/run-receipt.schema.json) | A green agent message is insufficient. Show requirement coverage, actual commands, exit results and missing evidence next to the deliverable. |
| Application lifecycle | A Desktop smoke script scaffolds a service-app fixture, executes its smoke test, prepares preview files, and checks archive/restore and reusable-tool registration. It uses a supplied manifest; it is not an end-to-end natural-language generation benchmark. [Source](https://github.com/agentlas-ai/agentlas-desktop/blob/ac487b729224084420b8d0f75ba88f2a3e3e80db/scripts/smoke-app-factory.cjs) | Acceptance should reach a runnable app and a reversible lifecycle, beyond producing a plan or code files. |
| Runtime recovery | A contract test checks quota cooldown and that temporary fallback does not overwrite the saved runtime choice; some assertions inspect source text. [Source](https://github.com/agentlas-ai/agentlas-desktop/blob/ac487b729224084420b8d0f75ba88f2a3e3e80db/scripts/runtime-fallback-does-not-change-choice-contract.cjs) | Show the reason for waiting or switching, preserve the chosen policy, and bound retries. Validate with real protocol fixtures rather than only source matching. |
| Honest UI feedback | A Playwright audit injects failures into mutating bridge operations, then searches for newly displayed success text. Its own comments identify it as an audit, not a release gate. [Source](https://github.com/agentlas-ai/agentlas-desktop/blob/ac487b729224084420b8d0f75ba88f2a3e3e80db/scripts/qa-false-success.cjs) | Test rejected submissions, failed saves and unavailable services through the visible UI. Preserve input and show recovery instead of success. |

## Factory: direct application-development reference

The user added [Factory](https://factory.com/) to the competitive set during this
work. The following is a primary-documentation audit, accessed 2026-09-27 local
time. Factory was not executed, and no comparative performance claim follows.

| Area | Documented behavior | Implication for Surge |
|---|---|---|
| Multi-feature work | Missions organize features into milestones, coordinate workers, and validate progress. The documentation also acknowledges unresolved correctness, parallelism and cost trade-offs. [Missions](https://docs.factory.ai/missions/overview) | Treat a project as persisted requirements and milestones. More parallel agents is not itself a measure of efficiency. |
| Control surface | Mission Control exposes workers, feature criteria/commits, time/credits, separate model selection and pause/replan/resume. [Factory App](https://docs.factory.ai/missions/running-app) | A developer should inspect any requirement's evidence and steer work from one surface. Prefer outcome and attention summaries before raw worker transcripts. |
| Validation prerequisites | Factory describes repeatable application startup, logs and programmatic user interaction as inputs to reliable QA. Milestone validation affects cost and duration. [Planning and Validation](https://docs.factory.ai/missions/planning) | Detect the actual startup and test commands, missing services and unavailable checks before unattended work. Make repairable setup gaps actionable. |
| Task mode versus authority | Normal, Spec and Mission modes are separate from the autonomy level governing permitted actions. [Interaction Modes](https://docs.factory.ai/autonomy-and-safety/specification-mode) | Separate what the developer wants done from permissions and spending. Avoid an ambiguous single “autopilot” switch. |
| Repository preparation | Agent Readiness evaluates repository/application criteria and offers remediation. Its level is a product-specific score. [Agent Readiness](https://docs.factory.ai/agent-readiness/overview) | Present concrete verified prerequisites and fixes. Do not substitute a high aggregate score for evidence that this app starts and its required behavior can be tested. |

Surge's proposed differentiation is a short path into an existing repository,
local ownership of durable work, cross-runtime ACP execution, exact outstanding
decisions, and a result that can be inspected and reproduced. These are targets;
they are not established advantages over Factory. Include Factory alongside
Agentlas in the same-task comparison described below.

Before an autonomous application task starts, the interface should show a compact
readiness result: selected agent usable, repository/workspace available, startup
command known, validation commands known, and required services available. Show
“not checked” separately from failure. Offer to repair missing setup, preserve
the request, and never present unexecuted checks as passed. The complete product
must exercise the running application's agreed user flow, not just compile it.

## Current Surge gaps observed in code

- `screens/spec_wizard.rs` renders the description as a `Div`, seeds a fixed plan
  and generic criteria, and advances through an apparent analysis step without
  analysis. It cannot serve as an honest creation workflow.
- `app.rs` handles that wizard's Create event by pushing an in-memory draft.
  Restart durability and execution do not follow from this operation.
- Welcome's Init New Project currently performs the same directory selection as
  Open Project. It does not implement the promised initialization journey.
- Existing Backlog/Fleet submission already calls the daemon via `dispatch_run`.
  Reuse that boundary; do not host a second engine in the desktop application.
  Inspect bootstrap follow-up and worktree ownership before claiming this path
  creates an isolated application end to end.
- CLI local execution and gate cancellation needed repairs before work ownership
  and terminal exit codes were dependable. The active completion plan records
  tests and remaining review findings.
- Live ACP checks did not yet prove application creation: Claude returned a usage
  limit; Codex stalled during session creation. Setup must surface these states
  usefully instead of treating binary discovery as readiness.

## Interface direction

### Runtime evidence from the September 28 acceptance run

The source-gap list above records the September 27 audit. Subsequent live checks
confirmed durable bootstrap submission, isolated planning and automatic creation
of an implementation run after flow approval. Desktop Inbox displayed the actual
document and delivered an approval; a requested roadmap revision preserved the
application requirements. New supervised runs appeared in the already-open UI.

Operation `01M3MC9ZYW75AY10QP5HKKSPSF` completed a live Codex-only countdown-timer
delivery: approved planning, implementation, an accepted verification report, and
durable `RunCompleted`. Rejected specification and report artifacts were repaired
through bounded ACP feedback. The implementer submitted the actual outcome tool
call, which advanced execution to the verifier.

Independent browser checks observed countdown completion, pause/resume/reset,
keyboard controls, validation feedback, and a fitting 390×844 layout with no
console errors. Six Node tests passed. Recorded source hashes still matched after
verification. The verifier ran tests and inspected source/specification evidence;
browser evidence came from the implementer and operator, not its own browser run.

This establishes one working app-creation slice. It does not establish competitive
superiority, recovery correctness, or all application-creation acceptance rows.
Exact evidence, run identifiers, and remaining failures are in the completion plan.

Use three primary work areas: Projects, Work, and Decisions. Agent configuration,
memory, graph details and logs remain accessible as supporting tools.

The Work view places the requested outcome and current state above the activity
stream. Result tabs expose Preview, Changes and Checks. The default view shows
what changed, what is running, and what needs the developer; detailed event logs
remain available for diagnosis. Never show invented subtasks, simulated progress
or unsupported completion claims as live work.

Each blocked state has a named cause and an action: reconnect the agent, revise
scope, answer a question, resume from saved state, or stop. Unknown cost stays
unknown; it must not render as zero. Progress uses completed acceptance items and
current activities rather than a fabricated percentage.

Keyboard operation, visible focus, readable contrast, retained drafts and clear
empty states are acceptance requirements. A redesign must preserve access to
existing capabilities even when their navigation changes.

## Acceptance and comparison

“Better” is a hypothesis to measure, not a claim justified by feature count.
Use the same repository, request, model access and spending ceiling when comparing.
Record product revisions, model identifiers, elapsed time, cost when available,
manual interventions and independent acceptance results. Publish unsuccessful
attempts too. No comparative measurements have been collected yet.

| Scenario | Required observation |
|---|---|
| New app from a plain-language request | An editable request reaches durable execution; the app starts and its agreed user flow passes an independent check. |
| Change an existing repository | Changes remain scoped, existing checks pass, and the developer can inspect the diff before integration. |
| Restart during work | The same work is recovered; requirements and decisions survive; no duplicate run or historical approval is dispatched. |
| Agent unavailable or quota exhausted | The UI states the actual cause, preserves the request and offers retry or a policy-permitted alternative. No endless spinner or false success. |
| Approval from desktop or Telegram | The decision applies to the exact outstanding request once; stale/cross-run replies cannot authorize another action. |
| Stop during execution or approval | Owned work settles, the persisted result reflects interruption, and the interface does not label it complete. |
| Verification fails | The failed requirement and evidence are visible, with a bounded repair path; final completion stays false. |
| Fast repeated submission or disconnect during admission | One durable intent maps to at most one admitted execution; the client can recover its identity. |
| Unprepared existing application | Startup/test/service gaps are visible before unattended execution; repairing setup is a separate observable action, and missing QA remains unverified. |
| Keyboard-only journey | A developer can describe, submit, answer, inspect and stop without inaccessible controls or lost focus. |

Deliver in vertical slices: reliable execution and installation; honest editable
submission; durable isolated bootstrap through implementation; decisions and
recovery; preview/changes/checks; then comparative efficiency measurements.
Each slice needs visible end-to-end evidence. The tracked queue is
[project completion](../.ai-factory/plans/project-completion.md).
