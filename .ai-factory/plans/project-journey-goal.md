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
