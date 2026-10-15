# Project lifecycle goal

Status: active; no end-to-end completion claim. Baseline: `6265b59` (PR #149).

## User outcome

Surge should guide both a person starting from an idea and a person continuing an
existing repository through the whole project. Developers and non-developers can
understand the flow, planned tasks, current agent work, completed stages, results,
and problems. Real provider testing uses `codex-acp` with `gpt-6.1-sol`.

`docs/product-strategy.md` supplies the existing product decisions. Runtime owners,
accepted outcomes, route consumption, settlement and verification retain their
existing independent contracts.

## Completion ledger

| Requirement | Evidence required | Current evidence / status |
|---|---|---|
| New idea reaches a usable project | Actual desktop journey through planning, approval, execution and running the result; inspect generated project and tests | Unverified |
| Existing repository can continue from its current stage | Actual desktop journey on an isolated existing repository; conventions retained, worktree changes and usable improvement inspected | Unverified |
| Whole path is understandable | Native screenshots and interaction assertions showing authored stages, planned/conditional next steps, current work and past stages together | Baseline opens Tasks; Home missing |
| Current agents and their work are visible | Actual live session bound to the exact task/run; expandable feed and accurate offline state | Per-run surfaces exist; project-wide integration unverified |
| Results and completed stages remain inspectable | Multiple operations/revisions and cold recovery preserve historical tasks, artifacts, evidence and exact destinations | PR #149 supplies TaskExecutionView; full project history unverified |
| Problems and required decisions are actionable | Failure, unavailable history, disconnection and decision scenarios open the exact owner and preserve uncertainty | Existing separate screens; Home integration missing |
| Both audiences understand the interface | Plain-language native labels, technical details available, actual navigation exercised | Partial existing UI; full journey unverified |
| Real Sol 6.1 provider test | Same ACP session advertises exact ID, acknowledges mandatory selection, executes bounded daemon/MCP scenario and settles; metadata binds durable session | Cached adapter and model metadata found; actual invocation pending |

## Next implementation slices

1. **Project Home.** Pure typed projection, native landing screen and shared task
   refresh. Reuse daemon-owned task cache for both Requirements and Flow origins.
   Include pagination, generations, ownership, availability and history coverage.
   Supplement legacy history through read-only inspection, with exact repository
   identity. Keep accepted planning distinct from draft artifacts and project files.
   Reuse TaskExecutionView; actions navigate existing owners.
2. **Portable exact-model live harness.** Explicit executable/JSON arguments/model,
   graph runtime model selection, test-only protocol metadata capture, same-session
   offered/acknowledged selection before prompt, bounded child settlement. Keep paid
   provider invocation opt-in and ordinary CI controlled.
3. **Prove the complete journeys.** Run both starting points through the actual
   desktop, fix failures found, inspect usable results and recovery. Home and a
   transport smoke test alone do not retire the full outcome requirements.

### Actual journey fixtures

Use disposable, isolated runtime homes and repositories; never launch test agents
against the maintainer's working copy. Keep generated demonstration recordings and
provider captures outside the repository.

- **New idea:** a small local habit tracker, with at least two authored stages:
  creating/checking habits, then persistence and filtering. Exercise planning,
  approval, execution and inspect the runnable interface. Check behavior using a
  fixed independent acceptance list, not the agent's own summary. Add a follow-up
  idea after the first result and confirm earlier stages remain inspectable.
- **Existing folder:** a small existing application with explicit conventions,
  baseline tests and a reproducible missing behavior. Open and scan it, implement
  the requested improvement through Surge, inspect the worktree diff and run its
  acceptance tests. Check that the original checkout and conventions survive.
- **Recovery and problems:** use controlled fixtures for a required decision,
  failed attempt, offline state and cold restart. Check exact navigation and
  retained stage/task history; a live paid provider need not be deliberately
  crashed to reproduce every deterministic failure mode.

Capture actual provider-reported Sol selection, task/run identities, accepted
requirements, routing, settlement and result evidence for the real journeys.
Successful transport negotiation alone does not satisfy these scenarios.

### Discovered desktop startup gap

`surge-daemon/src/main.rs` currently enables bootstrap only when startup cwd or
an ancestor contains a valid `surge.toml`. Desktop `start_daemon` inherits cwd,
while `BootstrapRuntime::for_project` can resolve the project's agent catalog only
after a bootstrap runtime already exists. Consequently, opening a configured
project after an ordinary application launch does not establish planning
availability. Prove and repair this with a focused regression before claiming
the ordinary desktop journeys work. A configured test-launch cwd may help
diagnose transport separately, but does not retire this product requirement.

The desktop currently exposes default-agent selection but no bootstrap node model
selection. Bundled planning profiles name Claude explicitly; changing default
agent does not retarget them. Disk profile overrides can select the Codex runtime,
but `recommended_model` remains advisory. The installed modern adapter documents
`CODEX_CONFIG` and derives its reported initial model from the provider's
thread-start response. Real planning journeys can use this supported adapter
configuration plus same-session observation, without changing production profile
recommendation semantics. Keep the strict harness's mandatory node-override proof
separate from initial provider selection, with explicit capture provenance.

## Current checks

- Behavioral RED: both actual desktop project-entry paths reach Tasks rather than
  Home. Isolated child fixtures finish setup and fail at the screen assertion.
- Behavioral RED: registry summaries with tied timestamps lack deterministic ID
  ordering, which is required for safe keyset continuation.
- New harness RED: no acknowledgement / wrong acknowledgement cannot authorize
  prompt; exact selected model must be recorded. Additional RED reproduces stale
  authorization after a later model-changing attempt.
- Native baseline screenshots were captured from a fresh build and inspected.
  Source implementation, GREEN checks, independent final reviews and real
  provider/native journeys are still in progress.

## Verification contract

Write externally observable failing tests before behavior changes. Home coverage
includes project-open navigation, older history alongside modern Flow work,
colliding IDs, exact decision destinations, stale project-switch responses,
pagination, offline/corrupt history, filesystem read-only opening and TaskSplit
cold recovery. Provider coverage includes absent model, wrong acknowledgement,
cross-session acknowledgement, recommendation-only selection, secret exclusion
and cancellation with surviving descendants.

Use independent pre-code plan review, spec review and code quality review. Run
repository formatting, strict Clippy, nextest and doctests with actual feature
sets. Record commands and their outcomes; do not infer full product completion
from targeted passes. Local build profiles may omit debug symbols to fit disk;
this changes neither feature selection nor test scope.

## Baseline evidence

Fresh native app built from `6265b59`; screenshots kept outside Git under
`/tmp/surge-project-journey-audit/`. Project opens Tasks with explicit offline
warnings. Plan and Results are separate destinations. Source inspection finds
newest-only legacy roadmap selection, so older operation history requires an
explicit shared projection. Missing runtime ownership is not proof of project
membership.

No new executor, scheduler change, verification sandbox or receipt authority is
part of these slices. Local commits are authorized; publishing a new PR or merging
new work requires the user's authorization for that work.
