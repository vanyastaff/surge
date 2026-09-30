# Factory product model and implications for Surge

Research date: 2026-09-28. Primary-documentation research, not a Factory runtime
evaluation. This expands the initial Factory reading in
[Vibe Coding Harness](vibe-coding-harness.md) and informs
[product strategy](product-strategy.md). It does not declare feature parity,
superiority, or completion of Surge's roadmap.

## What was examined

Five questions guided the research: what context survives; how work is validated
and recovered; who authorizes automation; how a repository becomes ready; and how
models and spending are controlled. Discovery used focused searches and the
[official documentation index](https://docs.factory.ai/llms.txt), followed by full
page extraction of the 15 core sources below. Additional SDK lifecycle sections,
the Sessions API schema, and the public Droid Action README supplied cross-checks.
The SDK extraction was capped; it is not claimed as a complete SDK audit.

All Factory behaviors below are **vendor-documented contracts**. Separate pages
corroborate several concepts, but they are not independent empirical evidence.
No Factory installation, fault injection, cost benchmark, or user study was run.
The public action README establishes a published integration design, not proof
that every described safety property holds in its implementation. Earlier
homepage/Missions overview/planning/App reading is background rather than the
main evidence for this expansion.

## The coherent product model

Factory combines a coding session, an execution environment, an extensible agent
harness, and workflows that supervise work across sessions. The following is an
analytical model of its documented surfaces, not a claim about internal storage.

| Entity | Responsibility and boundary | Evidence |
|---|---|---|
| Project and execution checkout | A selected repository supplies context; a separate worktree can hold task changes. Setup, retention, and cleanup are explicit concerns. | [Worktrees](https://docs.factory.ai/factory-app/worktrees) |
| Session and turn | A session carries history/settings/location; a turn has a distinct outcome. SDK resume reattaches to saved context, while runtime handlers and session MCP servers need reattachment. | [Python SDK](https://docs.factory.ai/sdk/python) |
| Worker | A delegated agent has a context boundary, model, and tool policy. Its report is an input to supervision, not inherently application acceptance. | [Subagents](https://docs.factory.ai/harness/subagents) |
| Mission | Planned work coordinates execution and validation; operators can intervene rather than assume unattended success. | [Mission reference](https://docs.factory.ai/missions/reference), [Running in CLI](https://docs.factory.ai/missions/running-cli) |
| Validation run | QA executes user flows and records evidence; code review separately examines changes for actionable defects. | [QA](https://docs.factory.ai/software-factory/automated-qa), [Review](https://docs.factory.ai/software-factory/code-review) |
| Automation | A repeatable trigger is associated with instructions, identity, execution target, visibility, and run history. It is more than a stored prompt. | [Automations](https://docs.factory.ai/software-factory/automations) |

The practical lifecycle is prepare an environment, establish scope and authority,
execute, collect evidence, review, then continue or recover. This is a synthesis:
the sources describe these pieces, but do not establish one universal atomic
transaction spanning them. In particular, a resumed conversation is not evidence
that a half-created branch, repeated external write, or interrupted admission is
reconciled exactly once.

## Evidence and testable implications

Each implication is a proposed Surge acceptance test or design constraint, not a
statement that Surge currently lacks every underlying primitive.

| Official source | Documented behavior that matters | Testable implication for Surge |
|---|---|---|
| [1. AGENTS.md](https://docs.factory.ai/harness/agents-md) | Durable repository guidance is distinct from task notes and skills. Nested guidance refines project guidance; instruction loading consumes finite context. | Show which instructions and project snapshot seeded a run. A nested-rule fixture should receive the correct applicable rules without duplicating every file. |
| [2. Skills](https://docs.factory.ai/harness/skills) | Names/descriptions are discovered before bodies are loaded. Effective, disabled, invalid, and overridden versions are visible. `allowed-tools` is descriptive metadata, not a runtime sandbox. | Expose the effective workflow source and resolution reason. A read-only label must not authorize tools; test restrictions at the execution boundary. |
| [3. Subagents](https://docs.factory.ai/harness/subagents) | Workers use separate contexts and explicit tool/model controls. They are noninteractive and report blockers upward. Background work has identifiable outputs and stop operations. | A worker blocked on human input must enter an observable owner-managed state. Collect every started worker's outcome; no orphan work when the parent stops. |
| [4. Hooks](https://docs.factory.ai/harness/hooks) | Lifecycle hooks run shell commands with timeouts. Pre-tool enforcement differs from post-tool feedback. Hook code inherits local environment/credentials; compaction and session-start hooks can preserve/inject context. | Record hook version, exit, timeout, and enforcement phase. A failed post-action check cannot pretend the action never occurred; cancellation must bypass repair loops that could erase interruption. |
| [5. Droid Exec](https://docs.factory.ai/droid-exec/overview) | Headless execution offers structured results, continuation/forking, tool restrictions and nonzero failure exits. Mutations require explicit autonomy; headless Missions require high autonomy or the unsafe bypass. | Test real CLI completion/failure, absent approval input, interrupted work, and structured results. Multi-agent mode should not automatically grant deployment authority in Surge. |
| [6. Running Missions in CLI](https://docs.factory.ai/missions/running-cli) | Operators monitor workers and pause/redirect when work freezes or plans change. Factory explicitly describes this as an intervention-oriented workflow. | Make the cause of a stop actionable. Test restart during a pending decision, stale replies, safe steering, and resumption without duplicate work. |
| [7. Mission reference](https://docs.factory.ai/missions/reference) | Missions inherit harness configuration, expose separate worker/validator models, and allow scrutiny or user-testing validation to be skipped. | Persist the effective validation policy and configuration provenance. Skipped required checks must remain visibly unverified, including after restart and model changes. |
| [8. Automated QA](https://docs.factory.ai/software-factory/automated-qa) | Setup detects apps and asks targeted questions; generated per-app workflows use diff routing. Reports distinguish pass/fail/blocked and attach browser, terminal, or API evidence. Failure learning may be proposed or committed. | A runnable acceptance fixture must exercise the actual app surface. Preserve blocked checks. Treat agent-proposed oracle changes as reviewable amendments, not permission to weaken acceptance. |
| [9. Local review](https://docs.factory.ai/software-factory/code-review) | Review scopes include working changes, commits, and branch diffs. Findings should identify concrete affected code, severity, and a reproducible reason. | Tie findings to a reviewed revision. After repairs, re-review the changed evidence; do not equate an unstructured positive review message with verified completion. |
| [10. Readiness report](https://docs.factory.ai/agent-readiness/readiness-report) | Repository/app criteria produce a persisted report. Remediation reads the latest report and makes reviewable fixes; rerunning evaluation checks improvement. The documented command requires Git and an origin remote. | Separate discovery from successful startup/testing. Test an unavailable agent, missing service, and app that builds but cannot start. Surge's local project flow should not need a remote solely to assess local readiness. |
| [11. App quickstart](https://docs.factory.ai/factory-app/quickstart) | Onboarding chooses a working directory/model, maps the codebase, and starts with a small reviewable change before broader automation. | Prove one complete useful journey before adding dashboards: enter request, see real admission, inspect changes/checks, request revision. Preserve the request when setup fails. |
| [12. Worktrees](https://docs.factory.ai/factory-app/worktrees) | Sessions can select a base branch, setup profile, and retention lifecycle. Ignored files require explicit inclusion. Cleanup accounts for session use and local changes, and checkout removal is distinct from branch removal. | Persist original repository, execution path, base revision, and setup state separately. Test failed setup/restart and retention of uncommitted work; do not copy secrets automatically. |
| [13. Connectors](https://docs.factory.ai/harness/connectors) | Managed connections are user/organization scoped; tool discovery is deferred. Authentication, availability, and action approval are separate; connector execution occurs in a remote service. | Show identity and destination before external writes. Revoked access must fail at execution even if an old UI/tool catalog still displays it. Do not describe all extensions as local merely because Surge is local-first. |
| [14. Automations](https://docs.factory.ai/software-factory/automations) | Scheduled, Slack, and GitHub triggers have owners, run targets, models, visibility, controls, and histories. Stable service identities/environments are recommended for shared work. | Before expanding intake, make trigger deduplication, persisted intent, execution identity, cancellation, and outcome inspectable. Test retries independently from recurring new work. |
| [15. Cost and productivity](https://docs.factory.ai/agent-effectiveness/cost-and-productivity) | Cost guidance combines model policy, scoped usage, and measurement. It distinguishes exported activity metrics from token/cost estimates in the Analytics API. | Show actual versus estimated versus unavailable cost. Aggregate parent/child/retry usage against one operation budget; count accepted outcomes and intervention time, not lines changed alone. |

## Context, memory, and recovery are different contracts

Repository instructions, reusable skills, working conversation, and learned
project facts should not become one undifferentiated memory store. Factory's
instruction/skill split supports selective loading; its worker isolation keeps
focused work out of the parent context. These are context-management patterns,
not proof of factual memory correctness. [AGENTS.md](https://docs.factory.ai/harness/agents-md),
[Skills](https://docs.factory.ai/harness/skills),
[Subagents](https://docs.factory.ai/harness/subagents).

The SDK documents compaction as summarization into a successor session, with
ownership transfer; rewind separately handles file restoration and reports
failures. It distinguishes interrupted outcomes from success and machinery
errors. Cancellation sends a best-effort interrupt. These qualifications matter:
none establishes durable completion of a larger multi-run operation.
[Python SDK](https://docs.factory.ai/sdk/python).

For Surge, keep accepted requirements, decisions, run identity, artifact hashes,
and terminal evidence outside lossy conversation summaries. A summary can help
an agent resume; it must not approve a pending request or set a task to done.
Recovery should reconcile saved intent with the actual checkout and persisted
events before continuing. These are recommendations consistent with Surge's
event-sourced ownership, not claims that Factory implements or lacks an
equivalent mechanism internally.

## Authority and evidence need visible provenance

Factory's extension design draws a consequential distinction between instruction
metadata and enforceable tools. Hook outputs also distinguish blocking a future
action from reporting on an action already performed. Copying skill files or a
permission dropdown would not reproduce these boundaries.
[Skills](https://docs.factory.ai/harness/skills),
[Hooks](https://docs.factory.ai/harness/hooks).

The public Droid Action README offers an additional concrete pattern: CI repair
policy is read from the default branch; diagnosis and repair capabilities differ;
retry/fix/lifetime-run limits have separate scopes. Its stated trust boundary
excludes fork PRs from privileged CI Steward execution. This is a published
design worth testing in Surge's future intake work, not an independently audited
guarantee. [Droid Action](https://github.com/Factory-AI/droid-action).

Surge should expose who supplied a rule, which revision was reviewed, which
commands actually ran, and what authority permitted side effects. Acceptance
changes require their own recorded decision. A worker's claim, a reviewer finding,
an executable check, and a user's authorization answer different questions.
The UI should retain those distinctions rather than collapse them into a green
checkmark.

## Model and budget conclusions

Factory Router claims adaptive model selection, failover, cost savings, and high
request reliability. Those performance numbers are vendor evaluations; this
research does not adopt them as comparative evidence. The SDK also separates
selected routing policy from the actual model reported on a response.
[Factory Router](https://docs.factory.ai/model-independence/factory-router),
[Python SDK](https://docs.factory.ai/sdk/python).

The reviewed cost guidance does not establish a hard, crash-safe budget for an
entire Mission and all its retries. Absence from these pages is not proof that
Factory lacks such controls. For Surge, record chosen policy and actual runtime,
preserve the user's choice during temporary failover, and never reset the total
budget at the planning-to-implementation handoff. Evaluate efficiency using cost
per independently accepted outcome, elapsed time, and human interventions.

## Recommended sequence for Surge

1. **Finish ownership before increasing autonomy.** Complete the durable
   bootstrap-to-implementation operation, isolated checkout, idempotent admission,
   whole-operation budget, and crash/cancellation recovery. The current honest
   planning UI is one slice, not the complete app-building journey.
2. **Make readiness concrete.** Verify the selected runtime can create a session
   and perform a bounded task; discover startup/check commands and services.
   Missing prerequisites should preserve the draft and offer a specific repair.
3. **Deliver a result with executable evidence.** Preview, changes, and checks
   must refer to the same output revision. Use a real representative application
   journey, negative tests, and explicit blocked results.
4. **Make interruption understandable.** Persist exact outstanding decisions,
   steering, and recovery reasons. Present the next useful action without forcing
   users to interpret worker transcripts.
5. **Then expand extensibility and automation.** Add inspectable instruction and
   skill provenance, enforceable capability policy, and recurring intake on top
   of reliable run ownership. Preserve the ACP boundary; Factory SDK/RPC surfaces
   are references, not a reason to create a proprietary transport dependency.

These priorities build on existing Surge primitives; they are not a request to
replace them with a new framework. The existing product notes contain dated
code-state observations (including the old fake wizard); those should be checked
against the active completion plan before treating them as current defects.
This research file deliberately does not rewrite that implementation status.

Avoid copying a fixed long questionnaire, treating a readiness score as working
application evidence, granting high autonomy merely to enable orchestration,
learning by silently changing the acceptance oracle, or calling a resumed chat
durable work recovery. Also avoid claims that local-first or ACP alone make Surge
better: the same-task comparison still needs pinned versions, comparable model
access, a shared spending ceiling, independent checks, and unsuccessful attempts
included in the results.

## Installed Factory App inspection — September 29, 2026

The user installed Factory and requested direct product inspection. The installed
`/Applications/Factory.app` reports **0.186.0** in its bundle metadata. The
following observations add native-window evidence to the earlier documentation
research; they do not establish execution reliability or a performance comparison.

| Step | Observed in the installed app | Product implication for Surge |
|---|---|---|
| Session entry | A single request composer with execution target, folder, model, autonomy, interaction mode, attachments, connectors and skills. The installation had no displayed sessions. | Start with an outcome and expose detailed choices progressively. Task shape and authority are different controls. |
| Mission Control | Mission list with All/Running/Paused/Completed filters; New Mission explains goal, approved plan, feature execution and milestone validation. The mission list was empty. | Missions should have a durable, identifiable lifecycle and an obvious entry point. Empty-state inspection does not prove the populated monitoring view. |
| Mission setup | Continuing the introduction opens the same composer in Mission Mode. No request was submitted. | Keep entry coherent across task sizes; tell the user why a larger task needs planning and validation. |
| Mission skills | The skill browser exposes mission-planning and define-mission-skills. Their visible instructions describe iterative investigation, worker procedures, dependency/validation readiness, milestone boundaries, and evidence handoffs. | Procedures can be inspected and grounded in an actual task. Reading these instructions is not proof the engine follows them. |
| Mission defaults | Separate model/reasoning choices for orchestrator, worker and validator; Skip scrutiny and Skip user testing switches were both off. | Validation policy should be explicit and recorded. A setting being enabled is not evidence that its checks ran. |
| Worktree settings | Managed worktree directory, retention limit, per-project local/shared setup profiles and a managed-worktree list. No managed worktrees were present. | Isolated execution needs a visible setup and lifecycle, not only branch creation. |

The application displayed a no-paid-subscription banner with upgrade and own-API-key
options. No purchase, credential change, provider request, worktree creation or
mission execution was performed. Account configuration was not audited. The app
was returned to its ordinary empty Normal Mode composer, with Auto Model and
Autonomy Off. Settings and skills were inspected without saving changes. Native
screens were viewed inline; no persistent screenshot set was created, so this is
an inspection record rather than a completed screenshot audit.

### What the shipped mission instructions add

The inspected worker-design skill distinguishes a worker's implementation procedure
from automatic milestone validation. It describes scrutiny and user-testing
validators, evidence-bearing handoffs, and re-running failed validation after
repairs. The planning skill requires executable dependency and validation readiness
and treats a milestone as a coherent, testable vertical slice. These are bundled
instructions, not observed completed runs.

Current [Mission Control documentation](https://docs.factory.ai/missions/running-app)
describes feature criteria and commits, worker inspection, progress and usage,
role-specific models, and pause/replan/resume.
[Planning and Validation](https://docs.factory.ai/missions/planning) emphasizes
reproducible application startup and driving the real user surface.
[Custom droids](https://docs.factory.ai/harness/subagents) documents a fresh context
window with a dedicated prompt, model and tool policy for each invocation.

### Comparison with the installed Agentlas

The inspected Agentlas One surface organizes the entry experience around a
standing team and ordinary-language delegation; Agentlas Work exposes its toolbox
and automation setup. Factory's inspected entry and mission setup organize work
around coding sessions and milestone delivery. This is an interface comparison,
not proof that either product is more capable, cheaper or more reliable.

For Surge, combine understandable outcome-first entry with explicit delivery
checkpoints, parent-qualified work occurrences, scoped worker context, inspectable
validation and steering. Adapt planning effort to task size. The roadmap/milestone/
task/subtask flow explorer and changes to future work remain concrete acceptance
requirements; these competitor observations do not retire them.
