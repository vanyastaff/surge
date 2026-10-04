# SuperPlane-informed improvements

Accepted direction (2026-09-30): strengthen Surge's issue → ACP agent → verified
change → PR → feedback → policy-governed merge path while retaining local-first
execution, ACP and event replay. The app is the primary operator surface;
Telegram duplicates decisions when connected. Existing ledger,
verification, reports, inbox and capacity mechanisms are the starting point.

## Product progress — 2026-10-04

The remaining scope is not a list of untouched features. Implementation, focused
verification and full acceptance have different statuses. This table is the
current summary; dated discovery and test notes below retain their historical
state. The active objective still includes every interview requirement.

| Product area | Already delivered or implemented | What remains |
|---|---|---|
| Reliable completion and evidence | Phases 1 and 2 accepted: durable completion delivery, shared waiting reasons, verification tied to code and criteria revisions | Integrate later task-level result and readiness surfaces |
| Persistent task | 3A accepted: immutable requirements, task discussion/history, ordered attempts, PR association and retained workspace ownership | Full lifecycle integration with resumable execution and cleanup choices |
| Stop, Continue and provider limits | ACP session recovery, durable controls/decisions, candidate cycles and A→B fallback implemented with focused actual mock-ACP wire and restart tests; configured task-owned pre-dispatch accepted in `95c5295` | Full phase acceptance; ordinary Flow ownership, complete child-process settlement, remaining quota/accounting and recovery cases |
| Desktop task experience | Create/discuss/edit and acknowledgement/currentness fixes implemented; isolated native Computer Use checks recorded on 2026-10-01 | Current-build end-to-end Flow/worktree controls, resume/archive/cleanup and full UX verification |
| Ordinary Flow recovery — current worktree | Wake refusal enters lasting Attention; original lease survives caller exit; actual event-before-ACK host crash reuses the existing event without duplicate work. Missing-journal recovery after actual test-host runtime consumption passes its paired fault test. Checkpoint `b622bd4` passes workspace strict clippy and formatting; the five-crate foundation suite passes 1830 tests with 8 inherited skips | Remaining fault/control cases, full workspace tests and independent final acceptance. The source checkpoint is local WIP; the increment is not released or fully accepted |
| Orchestrator and PR review loop | Existing workflow and GitHub merge surfaces provide foundations | Full interview-defined stages/policies, multi-provider planning U01, durable human/bot inline repair and re-review, CI/spec/evidence-based merge decisions |
| Result and notifications | Existing run reports, inbox and delivery mechanisms provide foundations | One task-level spec/evidence/readiness result; app and connected Telegram decisions throughout the full path |

Current recovery evidence is scoped to actual local test processes and mock ACP
providers. It is not native Linux/macOS/Windows child-domain acceptance, live
provider interoperability or complete desktop-product acceptance. Native backend
planning is underway; implementation and actual platform positives remain open.

The current Flow recovery/ownership implementation is preserved in local WIP
source commit `b622bd4`; full workspace tests and remaining fault/review acceptance
are the next milestone. Then execution settlement and quota recovery must
reach full acceptance before the remaining orchestrator/PR loop and product flows
can be called complete. Documentation and raw-test evidence commits are not
substitutes for that source milestone.

## Sequence and acceptance

Configured task-owned pre-dispatch and actual-worktree Start preparation were
accepted on 2026-10-03 and committed in `95c5295`. The final canonical test run
passed 3451 tests with 36 existing skips; independent specification, quality,
architecture and security/unsafe reviews accepted the frozen candidate. Detailed
scope, historical failures, two retained advisories and review evidence are in
`.autopilot/competitive-waves/tickets/12-capacity-scheduling.md`. Ordinary Flow
ownership normalization is the next accepted plan; real ACP spending, session
restoration and the remaining product stages are still open.

### Accepted multi-provider planning extension (2026-10-03)

For a difficult question or planning decision, the orchestrator may construct
several independent analysis nodes using different available providers, followed
by a synthesis node and one implementation owner. This is represented directly
in the generated Flow rather than requiring the user to select a separate mode.

The orchestrator chooses the number and ordering of these nodes from the question
and planning dependencies. Independent analyses may run in parallel; analyses
that need an earlier result run in sequence. Provider availability constrains
the generated graph, and synthesis waits for the required analysis outputs.

Each analysis node receives the same accepted input revision and evidence and
remains read-only. Synthesis records agreements, unresolved disagreements,
selected decisions and their rationale; it cannot replace verification or retire
requirements. Stage acceptance follows the existing configurable policy. When
different providers are unavailable, use the accepted fresh-context same-provider
review fallback or preserve a paused decision, recording reduced diversity.

Acceptance must verify persisted analysis outputs and synthesis after restart,
no writes from reviewers, no execution before required stage acceptance, and
invalidation of analysis/synthesis when their input revision changes. This is an
accepted extension of phase 4, not an implemented or accepted-complete feature.

| Phase | Scope | Acceptance | Status |
|---|---|---|---|
| 1 · P0 | Reliable completion reconciliation and durable outbound delivery | Lost events and daemon restarts cannot strand finished tickets; tracker delivery retries without starting work again | Complete: local reconciliation, durable terminal comments and shared waiting reasons verified |
| 2 · P0 | Verification pinned to code revision/checkpoint and criteria version | Changing the result invalidates old evidence; cancelled/skipped required checks cannot pass | Complete: revision/criteria binding, shared invalidation and current-proof surfaces verified |
| 3 · P1 | Persistent task across runs | Retries retain accepted requirements, PR, history and total cost | In progress: 3A accepted; recoverable execution and task UI remain required |
| 4 · P1 | Persistent CI/review repair loop | Feedback is SHA-scoped; one editor owns a PR; repeated events and restarts cannot produce duplicate repairs; unresolved feedback remains actionable until accepted or an explicit orchestrator decision | Planned |
| 5 · P1 | One result card and task-level report | The user can identify readiness, evidence, cost and the required decision without reading transcripts | Planned |
| 6 · P2 | Explainable admission readiness | Missing scope, context, runnable environment or acceptance checks produce actionable clarification/blockers | Planned |

Implement phases 1 → 2 → 3 → 4 → 5; develop readiness after task and verification
contracts settle. Establish baseline acceptance rate, human interventions,
accepted-task cost, waiting time and false completions before setting targets.

## Historical acceptance checkpoint (2026-10-01)

This checkpoint records the state on that date. The product progress table above
supersedes its remaining-work descriptions, including the old quota fallback RED.
Detailed notes below are chronological evidence, not additional completion claims.

| Requirement | Current evidence | Remaining acceptance |
|---|---|---|
| Native task create/discuss/edit | Updated native screenshots 22 and 23 independently viewed: discussion accepted, composer cleared, immediate edit accepted with task/draft revision 2; ACK/in-flight and lost-reply focused tests pass | Full agent workflow and remaining task mutation/control scenarios |
| Native workflow admission | Screenshot 16: terminal-only task Completed, All 1 / Finished 1 | Agent execution, stop/resume and full end-to-end result |
| HumanGate same-run pause/restart | Native screenshot 25 independently viewed: attempt 1 Suspended with Continue; actual cold-host paused-answer socket GREEN 1/1 in 0.27s; UI had an actual RED pending0→1 after suspend, fixed and task_ui_tests module GREEN 8/8 | Updated native screenshot 27 independently viewed: original gate appeared with Approve/Reject; clicked original question, task UI now shows Completed and Attempt 1 · Completed; read-only journal confirms one request, one resolution, one Continue, one RunCompleted on same run. Concurrent/stale/expired response guards |
| UI stale detail | Actual admitted Suspend GREEN 1/1 in 1.07s; delayed publication/target/version regression GREEN 1/1 in 0.96s; current native build exercised | Full-history session and pending-decision rehydration checks |
| Quota recovery | Storage foundation 13/13; actual A429→B fallback remains RED | Latest-cycle admission, actual provider handoff/coordinator and complete writer-domain containment |
| Original HumanGate decision across Suspend/Continue | Native Computer Use displayed and approved the original decision; actual journal has exactly one request/resolution/Continue/RunCompleted on the same run; suspended-answer socket GREEN | Timeout/conflicting/stale/superseded response negatives and broader cold-restart gates |
| Full provider-session history pagination | Actual RunsScreen opens a 261-session trusted SQLite journal and reaches oldest seq2/original internal ID after 13 real Older clicks; `/tmp/surge-3c-session-pagination-attempt7.log` 1/1 GREEN | Fresh native build/UI check and unavailable-history refusal |
| Task title in inbox mission | Stored-title regression GREEN 1/1; fresh isolated app Computer Use shows `Computer Use: остановка и продолжение` in failure card and detail instead of accepted-requirements hash prefix; backend prompt remains unchanged | More project-ownership cases and full native review flow |
| PR review/CI/merge | GitHub read-only check: no open PR | Publish reviewable implementation, address all inline findings, required CI and full lifecycle acceptance |

No row establishes completion of phases 3–6 or of the full requested product.

## First reliability increment

The daemon reconciles tracker tickets in `Active`/`RunStarted` against their
currently assigned run's existing journal on startup, every minute and after a
lagged global completion subscription. The reader is non-mutating; missing,
corrupt, invalid and nonterminal histories preserve ticket state. Completed,
failed and aborted journal outcomes use the same policy as live completion.
Parked runs remain active and are not labelled failed or aborted.

Run-scoped atomic updates prevent delayed old-run events from affecting a
reassigned ticket. Active promotion cannot resurrect a terminal ticket.
Concurrent global/per-run/sweep terminal delivery completes the ticket and
inserts an immutable comment intent in one SQLite transaction. An enqueue error
rolls back the state change. Per-run subscriptions continue on lag and inspect
the journal when their sender closes.

### Durable terminal comments

Completed, failed and aborted comments persist source, task, run, terminal kind
and exact body in `terminal_comment_outbox` (registry migration 0020). A visible
run identifier makes equal-looking outcomes from different runs distinct. The
existing intake emission key is reused for successful delivery acknowledgment.
Ticket reassignment cannot alter a queued payload.

A separate serial worker claims the oldest due comment and posts it with a
five-second timeout. Claims have a fresh token and a 30-second lease. Startup
reclaims expired leases and drains queued comments even when their tickets are
already terminal. Failed delivery records a diagnostic and retries with
exponential backoff from five seconds to five minutes. Missing sources remain
queued with a request to restore configuration. Invalid queued payloads are
retained with diagnostics and delayed so healthy jobs can proceed.

Acknowledgment atomically settles the outbox and records the emission key.
Both acknowledgment and retry require the current unexpired lease token;
replacement claimants cannot be affected by a stale worker. No database guard
crosses network I/O. Shutdown performs a bounded final drain (at most 32 jobs,
six-second budget); unfinished jobs remain recoverable after lease expiry.
Completion ingestion and local reconciliation continue while delivery is hung.

Reconciliation starts without TaskRouter sources and never rebroadcasts recovered
terminal events or triggers auto-merge. Parked comments remain bounded best
effort; parking does not complete a ticket or enqueue a terminal comment.

Acceptance coverage includes lost startup completion/failure/abort events,
RunStarted catch-up, repeated terminal delivery, stale run assignment and Active
promotion, lagged subscribers, terminal-to-Parked prevention, unchanged
missing/corrupt/nonterminal journals, atomic enqueue rollback, immutable payload,
concurrent claims, expired/stale lease settlement, reopen recovery, migration
upgrade, successful acknowledgment deduplication, missing source retention,
provider failure retry after restart and completion ingestion during hung delivery.

### Delivery limits

The protocol cannot promise exactly-once external delivery after a provider
success followed by a crash before acknowledgment, or an uncertain timeout.
GitHub currently checks only the first comment page for the exact body; Linear's
implementation sends no idempotency key. Stable run-specific bodies support
best-effort provider deduplication, but uncertain success can still duplicate.

Upgrade does not backfill historical terminal tickets: their previous
best-effort comments may already have been sent. Pre-upgrade terminal comments
that were omitted cannot be reconstructed safely by this migration. Existing
successful intake emission records are preserved and suppress enqueue while
still allowing local Active/RunStarted state repair.

### Shared waiting reasons

CLI Inbox, Telegram status replies/cards and desktop Runs share a pure core
projection of durable run state: human input, capacity, and operator recovery.
Human input takes precedence over capacity; capacity includes the recorded wake
and whether it came from an observed reset or policy backoff. A wake time is not
a guarantee that dispatch succeeds.

Registry-confirmed daemon loss is recovery requiring inspection, not completed
work. Durable terminal evidence overrides stale crash labels. Missing, unreadable
or discontinuous history stays unconfirmed; it never implies running or automatic
recovery. Recovery and unconfirmed entries remain visible outside capped Done
history. Desktop Inbox opens the run for inspection and preserves the existing
request identities and decision controls. A disconnected stream alone is not
crash evidence; a gap needs full-history rehydration before its display is trusted.

Acceptance covers fixed reason/action text, human-over-capacity precedence,
recorded wake basis, aborted terminal evidence, stale crash labels, per-run journal
failure isolation and desktop parked terminal/sequence-gap handling. A trusted
history begins at sequence 1 with `RunStarted`; a later start cannot repair an
untrusted origin. Terminal registry labels without readable durable history stay
unconfirmed across all surfaces.

Phase 1 is complete. Independent specification and code reviews accepted the
increment. Full CLI, core, orchestrator, persistence and Telegram suites passed
(104, 790, 401, 386 and 102 unit tests respectively, plus executed integration and
doc tests). Native desktop passed 195 unit/smoke and 5 integration tests; strict
all-target/all-feature clippy passed for all six affected crates. Existing ignored
tests remain excluded. One original skill-binding fixture transiently hung during
the first broad run; isolated and bounded full retries passed without harness or
production engine changes. No protocol fix is claimed for that transient.

## Revision-bound verification

The host observes the whole working tree (including dirty and untracked code),
pins its repository identity and immutable checkpoint, and supplies accepted
criterion IDs. Reports travel inline with their authenticated outcome candidate;
the host preserves the accepted criteria epoch while code and semantic criteria
remain unchanged, and rotates it after an observed invalidation. It stores
canonical TOML outside the checked worktree before publishing proof. Exact candidate retries retain
association; another report cannot replace the candidate's receipt.

All required IDs need passing checks. Cancelled, skipped, missing, mismatched or
agent-bound reports cannot prove success. Live replay, reports and normalized
SQL share the claim gate and invalidation model. Later code observations invalidate
all proofs; accepted roadmap amendments invalidate criteria. Returning to an old
criteria version cannot revive an old report. Observed code A → B → A and task
status transitions clear accepted context; OnError hook mutations are observed
after the hooks run. Completed stale tasks remain visible
for re-verification; no automatic re-verification workflow is introduced.

Host-facing ledger, inbox and report queries compare current code with the bound
identity and validate stored bytes. Missing worktrees, unreadable observations or
missing/corrupt reports are Unknown. Pure report compilation describes recorded
history and does not claim current freshness. Legacy unbound events remain readable
but do not count as proof; registry migration 0021 clears legacy verified bits
without requiring a new event, and per-run migration 0004 adds normalized context.

Limitations: no-Git workspaces cannot produce current proof. Git clean-filter or
mode normalization that differs from the read-only fingerprint fails closed.
Artifact storage must be outside the checked worktree for bound proof sealing. Pre/post code
comparison detects net changes, alongside the existing read-only sandbox; it does
not attest that files were never changed and restored between observations.
External edits restored before an observation have the same content identity.
Existing task verifier profiles must adopt inline reports and supplied IDs.

A generic verifier outside an accepted task can still route its workflow with a
valid unbound audit report. Bundled profiles submit that report inline; historical
file audits still undergo their declared artifact contract. A Passed audit requires
actual passing checks; a valid Failed audit follows its declared failure/backtrack
route. Inline agents cannot supply host binding. Legacy file metadata never counts
as proof, even if it contains binding fields. A generic workflow may be Completed,
but no `TaskVerified` or current task proof is created. No whole-run
synthetic task or criteria are invented.


Phase 2 is complete. Independent specification, API/storage and implementation
reviews accepted the change. The bounded core/Git/persistence/ACP/orchestrator/CLI
run passed 2,720 tests with 34 existing ignored tests; strict all-target/all-feature
clippy passed for those six crates. Full daemon and Telegram suites passed, as did
native desktop's 195 unit/smoke and 5 integration tests and strict clippy for
those three crates. Independent checks reran the Git fingerprint oracle (1/1)
and cross-surface verification regressions (12/12). Formatting and diff checks
passed before the final documentation update.

GUI smoke used an isolated profile: Welcome, opening the project, Decisions and
Results, and a terminal-only Completed run whose checks correctly remained
unverified without a saved report. This does not claim a paid-provider or live
issue → implementation → PR → CI end-to-end run. A disk-capacity failure while
compiling the extra suites was resolved by cleaning regenerable Cargo artifacts;
the successful rerun required no source change.

## Product interview decisions (2026-09-30)

These are accepted product requirements, not claims of implemented behavior.
They refine phases 3–6 and replace the earlier assumption that every merge needs
a human decision.

- One input accepts a new-project goal or an existing-project feature, fix or
  issue. The orchestrator selects skills and follows Goal → Describe → Plan →
  Flow → Slices → Build with project context. Each stage can require human
  approval, delegate acceptance to the orchestrator, or require agent review.
- Reviewers have read-only access and always return findings; the stage executor
  makes corrections. Review and correction repeat until acceptance. Prefer a
  separate provider when configured; if unavailable, a fresh-context reviewer
  session from the same provider can use review skills.
- The daemon hosts the orchestrator, which assigns dependencies, priorities and
  ordering, and runs independent tasks in parallel. It evaluates review findings
  and can perform justified refactoring within the task and PR, or propose a
  separate debt task. It requests human input when stuck or a decision is needed.
- A persistent task owns its accepted requirements, task-specific discussion,
  attempt history, worktree, flow progress, agent session IDs and the same PR
  throughout corrections. The app is the primary interface; connected Telegram
  duplicates notifications and decision requests against the same identities.
- Users can directly edit a task or discuss changes in its chat; the orchestrator
  can update it through MCP. Changes to an agreed spec require a question with
  options and consequences. If a running attempt must stop and restart, retain
  existing code and uncommitted changes and adapt them, rather than discard work.
  Reassess affected plan steps and evidence against the updated requirements.
- Expose worktree changes and flow progress with stop, continue and archive
  controls. Archive removes a task from active work while retaining its history
  and worktree; it is not destructive deletion. Stop also retains recoverable
  execution state. Resume uses the saved ACP session ID when supported; power
  loss and quota exhaustion must not lose the task's work or progress.
- Archiving a task and cleaning its worktree are separate decisions. Before
  proposing cleanup, inspect whether a PR exists, whether it is open/merged/
  closed, its recorded checks/review state, uncommitted files (including
  untracked files), and local commits not preserved remotely. Offer applicable
  options with their consequences: archive and retain the worktree, commit
  selected work and preserve the branch, or remove the worktree when the user
  explicitly chooses cleanup. Unknown or failed inspection is not evidence that
  work is safely preserved. Do not automatically commit all files or infer
  permission to discard changes from a request to archive a task. A merged PR
  does not by itself prove that later local changes are preserved.
- Subscription quotas currently govern availability; there is no user monetary
  cap in this usage scenario. Save sessions and progress on quota exhaustion.
  The orchestrator can choose another available provider/model or follow a
  configured fallback order. If none is available, pause affected work, check
  availability when supported and resume when quota returns. Do not imply that
  an unchecked quota has recovered or that every provider supports ACP resume.
- Address human and bot inline PR feedback in the same process. Reply to each
  finding with agreement or a reasoned disagreement, the correction and commit
  when applicable, then request another review. Silence or a review bot's quota
  exhaustion need not block merge indefinitely: the orchestrator can assess
  known findings and decide readiness, subject to repository merge rules.
- Ready means every spec item within the relevant sprint, task or subtask scope
  is satisfied, evidence is current, known PR findings are addressed, and CI and
  repository rules permit merging. CI alone does not establish spec completion.
  Automatic merge is the default; offer a setting for mandatory human approval.

Phase 3 should first establish durable task identity, requirement history,
attempt/PR/worktree/session associations and non-destructive lifecycle controls.
Later phases add review/repair orchestration and the complete task interface;
their interview requirements are not implicitly delivered by task persistence.
Unresolved operational details include the bounded policy for requesting another
review and handling silence, and provider-specific resume/recovery behavior.

### Implementation sequence and outstanding acceptance

The active objective is to implement all interview requirements. The increments
below describe dependencies, not reduced completion criteria. Every item remains
outstanding until implementation and matching acceptance evidence are recorded.

| Increment | Required observable result | Evidence/status |
|---|---|---|
| 3A · Persistent task and execution association | Accepted requirement history, discussion, same PR, retained workspace and ordered attempts survive reopen; real dispatch uses the accepted revision; late attempts cannot replace current ownership | Accepted: connected daemon/CLI/MCP tests, migration/history/usage, crash and ownership regressions; independent spec and both quality reviews |
| 3B · Recoverable execution | Stop retains work and progress; continue restores the saved ACP session when supported; quota exhaustion preserves sessions and selects configured/automatic fallback or pauses | Real mock ACP resume/load traffic, process restart, cancellation races, unsupported capability, dirty/untracked preservation and quota recovery tests |
| 3C · Task interface and lifecycle | App task detail exposes worktree changes, flow progress, editable requirements and task discussion; MCP edits share history; archive and explicitly chosen cleanup inspect actual preservation state | Native interaction, shared decision identity, active-task archive, PR/unpushed/untracked inspection and cleanup choice tests |
| 4 · Orchestrator and review/repair | Skill-based Goal → Describe → Plan → Flow → Slices → Build; configurable stage acceptance; read-only independent review cycles; priority/dependency-based parallel work; same-PR inline replies, re-review and merge policy | Full stage-policy and provider fallback tests, review permission checks, same-PR feedback/commit association, writer ownership and restart tests; actual merge readiness gates |
| 5–6 · Result and readiness | Task-level current spec/evidence/readiness/result and configured app/Telegram requests; spec amendments offer choices and update dependent work | Cross-surface tests, requirement revision invalidation, notification/decision identity, actionable clarification and full-path acceptance |

The pre-3B baseline discovery found that `Engine::stop_run` aborts irreversibly and
the bridge kept the provider ACP session ID only in memory. Those baseline paths
do not satisfy recoverable Stop/Continue, even though run snapshots and capacity
parking already exist. Task suspension must preserve its own semantics; legacy
Abort must not be presented as a resumable pause.

The baseline bridge handshake in `crates/surge-acp/src/bridge/worker.rs`
initialized ACP v1 and unconditionally called `new_session`; it stored the
returned provider ID in the in-memory session. 3A cold-run recovery through the
mock bridge therefore cannot stand in for 3B provider-session recovery. 3B must
persist provider identity and capabilities, select supported load/resume using
the same working directory, and demonstrate the saved ID on actual mock ACP
wire traffic. An unsupported capability needs an explicit recorded recovery
decision rather than silently claiming the original session continued.
Before 3B, `EventPayload::SessionOpened` recorded Surge's own `SessionId` and
runtime/profile identity without the provider's returned ACP string.
The two identifiers must stay distinct in persistence and recovery, so a saved
internal ID cannot be accidentally sent as a provider session ID.

### Market comparison of the interview choices

Checked against official documentation on 2026-09-30. These are product
trade-offs and hypotheses, not verified claims of superiority.

| Choice | Documented comparison | Expected benefit and cost for Surge |
|---|---|---|
| One task/PR through feedback and CI | SuperPlane already tracks tasks through implementation and verification, returning failed checks and PR feedback for another attempt. [Factory lifecycle](https://docs.superplane.com/get-started/how-superplane-works/) | Necessary baseline, not unique positioning. Surge must prove preservation of local edits, sessions and accepted spec across restart. |
| Orchestrator decides refactoring and merge | SuperPlane describes workflow-controlled agents and human responsibility for uncertain work and final merge. [Control model](https://docs.superplane.com/get-started/how-superplane-works/) | More delegated judgment may reduce user interventions, but makes accurate spec/evidence gates and recorded reasons essential. |
| Local retained worktree and subscription-aware recovery | SuperPlane's Claude component documents a provisioned sandbox that clones, commits, pushes and opens or updates a PR. [Claude component](https://docs.superplane.com/components/claude/) | The proposed local continuity is useful for work already in progress; power loss, local workspace ownership and provider resume support become Surge's responsibility. |
| Configurable automation and review | Factory documents configurable GitHub-triggered and scheduled automations. [Custom automations](https://docs.factory.com/software-factory/automations) | Automation alone is not differentiating. The proposed task-specific conversation, stage acceptance and recoverable sessions need usable interfaces and end-to-end proof. |

No absence of a feature in these pages is treated as proof that a competitor
cannot implement it. Measure accepted-task completion, interventions, recovery
success and stale-evidence failures before claiming the design is better.

### 3A design for pre-implementation review

Ownership: add core `WorkItemId` and pure domain records; registry persistence
owns work-item history. Keep legacy FSM `TaskId`, external tracker locators and
run-local `RoadmapTaskId` distinct. Execution journals and verification remain
run-scoped. Reuse the existing registry pool, migrations, artifact storage,
read-only run inspection, engine startup batching and Git repository identity.

Add registry migration 0022 with project identities (unique canonical Git common
directory), work items, immutable accepted revisions, discussion/proposals,
attempts, a single immutable PR locator per item, and idempotent operations.
Project records retain the original checkout separately from the task workspace.
An item stores its accepted revision, row version, retained workspace/original
ownership locator, archived timestamp and active run/generation. Each attempt
stores unique run ID, item-local ordinal, accepted revision/hash, frozen execution
inputs, reservation status, diagnostic and terminal observation. Unique indexes
and transactions enforce one active attempt per item. No automatic migration
association of historical runs, tracker tickets or similarly named roadmap tasks.

Mutations carry an operation ID and expected item version; accepting requirements
also carries expected revision. Same operation/body replay returns its original
result; reuse with a changed body conflicts. Requirements are never overwritten.
User edit acceptance is explicit; MCP proposals remain proposals until an explicit
decision accepts them. Active items reject archive and accepted-spec changes in
3A; discussion remains available. 3B introduces recoverable suspension before an
active revision can change. Archived items reject dispatch. Archive deletes no
files or branches.

Start validates graph and repository/workspace identity, then reserves run,
ordinal and generation transactionally before engine launch. The first attempt
provisions a retained task workspace through a new Git helper; retries reuse it
sequentially without replacing dirty/untracked files. Missing or mismatched
retained workspaces require attention rather than silent recreation. Do not edit
the pre-existing user changes in `surge-git/src/run_worktree.rs`.

The daemon supplies a host-owned optional attempt association; arbitrary generic
StartRun requests cannot attach themselves to a task. Seed immutable accepted
requirements as an artifact and initiating context, and inject their revision/
hash/text into the actual agent prompt. Add `WorkItemAttemptBound` to the same
durable startup event batch as `RunStarted` and `PipelineMaterialized`, before
dispatch. No agent-authored binding or mutable file can replace this context.

Registry and run journals are separate databases. Reconcile these crash windows:

- Reserved without a journal: keep reservation/frozen inputs; explicit retry of
  the same operation starts the same reserved run, not a new attempt.
- An empty initialized journal: recover only after checking emptiness and exact
  reservation ownership; do not delete it to restart.
- A complete trusted startup batch with matching binding: mark launched and use
  run recovery/admission; do not dispatch another run.
- Missing/mismatched binding, corrupt or discontinuous history: retain work and
  report Attention; do not guess that execution never happened.
- Durable terminal history: settle that attempt; clear current ownership only
  with matching run/generation. Old completion can update only its own history.
- A confirmed engine rejection before durable startup: retain history/diagnostic
  and workspace and transactionally release the active reservation.

The operator projection distinguishes attempts on old accepted revisions from
current requirements; historical proof cannot establish current task readiness.
Aggregate usage once per unique associated run from durable events/projections,
with known cost and unknown/incomplete counts separate. Repeated polling or
resume is not a new chargeable attempt. PR attachment preserves one normalized
provider/repository/number identity and validated display URL; it does not yet
create, repair or merge PRs.

Route connected controls through daemon service and shared operator adapters:
CLI `surge task create/show/list/edit/discuss/start/archive/attach-pr`, plus MCP
create/show/list/propose/discuss/start/archive/attach-pr. MCP writes retain its
existing mutation guard and audit. Later native UI controls use these same
identities/services; no disconnected second task model.

Required failing-first tests cover real engine dispatch and the exact first ACP
prompt; concurrent starts and stale edit/archive/completion; operation replay
and conflicting body; restart at each reservation/startup/settlement boundary;
revision/retry retaining PR, discussion and dirty/untracked workspace; historical
proof freshness; unknown usage and deduplication; actual CLI/MCP daemon routing;
and additive upgrade preserving existing registry records. Full affected suites,
strict all-target/all-feature clippy, formatting and diff checks follow. Cargo
runs are serialized with incremental compilation and dev/test debug disabled.

Independent plan review requires the following protocol refinements before code:

- A rejected start operation is terminal and replays its rejection even after
  ownership is released; a new attempt needs a new operation ID. An unresolved
  reservation retains ownership and reuses its reserved run. Operation replay
  precedes current version checks; changed bodies always conflict.
- Launch has an exclusive inter-process OS lock plus a transactional claimant
  token tied to reservation generation. Hold the lock through inspection,
  workspace preparation, startup commit and settlement. Concurrent same-operation
  replay never provisions or launches twice. An absent journal cannot override a
  live lock; process death releases the lock, after which recovery acquires a
  fresh claim and re-inspects. Prefer standard-library file locking supported by
  the project MSRV, without adding a dependency or copying leaked-lock machinery.
- Persist exact workspace creation intent (pinned base commit, canonical common
  directory, path, branch, creation/ownership identity) before Git mutation.
  Reconcile creation-before-acknowledgment from that intent without reset/adoption
  of unrelated directories. Verify canonical path, common directory and registered
  linked-worktree/branch ownership; substituting a normal checkout is rejected.
- Reserved task run IDs cannot bypass ownership through generic start, resume,
  fork or admission queue-drain paths. The service supplies an unforgeable host
  claim; engine association comes from validated reservation data, not a public
  configuration field. Durable reservation inputs survive loss of the in-memory
  admission queue.
- Resume validates and hydrates binding, immutable requirements artifact/hash,
  graph and frozen inputs. Every ACP stage prompt, including resume, bootstrap
  and subgraph stages, includes the host-pinned accepted context. An engine Err
  alone cannot release ownership: re-inspect trusted durable evidence while
  holding launch ownership, since errors can occur after startup commit.
- Lists, discussion, revisions and attempts have stable cursor pagination and
  enforced maximum page sizes. Task summary usage comes from indexed per-run
  registry projections with durable event cursors, not unbounded journal scans.
  Sync reconciles each unique run incrementally and preserves unknown values;
  detailed evidence inspection is explicit and bounded. Test beyond page limits.

Both independent plan lenses accepted these refinements on 2026-09-30. Launch
locks use a stable file identity across processes and match the SQL generation;
no database transaction crosses an async provisioning or launch wait. Post-build
repair dispatch count for 3A is 2; pre-code reshaping does
not consume or reset that later shared review-repair count.

Initial behavioral RED: `work_item_route_test` sent a task-create request through
the actual daemon socket and failed because the daemon closed the unrecognized
request (0 passed, 1 failed, exit 101). This confirms the missing route only;
restart, real execution, bound prompts and the remaining protocol acceptance
still require implementation and expanded tests before 3A can be accepted.

Ownership behavioral RED: `work_item_ownership_test` reserved an attempt in the
real registry and demonstrated that generic `Engine::start_run` could still run
that reserved ID without a host claim (0 passed, 1 failed, exit 101). The engine
ownership guard subsequently passed that focused test (1 passed, 0 failed).
The positive host-claim path and wider protocol acceptance remain under
implementation; this single negative-case GREEN is not 3A completion evidence.

Expanded connected acceptance subsequently passed: the daemon closes, registry
storage reopens under a fresh engine/daemon, the saved task starts, its durable
startup includes accepted binding, and the bridge prompt carries the exact
accepted text, criterion, revision and hash. Replaying the start returns the same
run. That binary passed one task scenario and four shared mock-bridge fixture
tests (5 total); this is not live-provider ACP transport or whole-goal evidence.
Boundary tests, affected suites, strict gates and independent reviews are pending.

Crash-boundary expansion found two genuine failures (six tests passed, two
failed): concurrent replay could return a stale launch claim when terminal
settlement raced it, and cold recovery of committed startup failed before agent
dispatch because the required `InitialPrompt` artifact was absent. These are
implementation gaps, not accepted exceptions. Their regression tests must pass
before specification review. Startup inspection now reads complete history in
bounded SQL pages rather than rejecting legitimate runs after 10,000 events;
the paged continuity and immutable-startup validation still require verification.

The corrected crash suite subsequently passed eight tests (four task scenarios
and four shared fixture tests). A production-shaped startup with `user_prompt`
bytes/event resumes the original run and supplies accepted text, revision and
hash to the mock bridge; concurrent retry reuses the original attempt. Startup
validation now checks the prompt event and bytes. A separate missing-prompt
Attention regression has been added but its next run, full affected gates and
independent reviews remain pending. This is not proof of live ACP session resume.

The full persistence unit binary passed 395 tests, including a 10,025-event
trusted fold with late-gap rejection, 205-entry cursor history, migration
upgrade and usage deduplication. A compiled CLI subprocess against an isolated
real daemon passed create/start/replay/show for the same persistent task using a
terminal flow. The connected MCP test passed create/discussion and confirmed
that amendments remain proposals and unsupported direct edits are rejected.
Remaining command coverage, cross-crate gates and independent review are still
required; these results do not prove session resume or the complete product flow.

The expanded compiled CLI lifecycle passed its connected scenario: discussion,
proposal acceptance, edits, revision/attempt history, list, PR attachment and
archive all reach the daemon. A second attempt retains the same PR and untracked
feedback file; the earlier attempt projects `Superseded` after accepted
requirements change. Historical run verification and task-level current readiness
must remain distinguishable; task-level result/readiness is still pending.

The expanded crash binary passed eleven tests (seven task scenarios plus four
shared fixture tests): missing prompt or binding becomes Attention without
dispatch, disconnected callers retain the durable attempt, and terminal
settlement verifies frozen requirement integrity before releasing ownership.
The actual-Git historical-proof regression also passed: a prior run's proof
retains its historical meaning while its attempt becomes Superseded after a
requirement edit. Full affected tests, strict lint and independent reviews remain
pending before this increment can be accepted.

At source freeze the affected six-crate run completed without timeout: 2,602
passed, three fixture expectations failed and 28 were ignored. The two schema
expectations and the debug snapshot were updated explicitly; a full core retry
passed 856 tests, yielding 2,605 passes with the other already-passed suites.
Final six-crate strict all-target/all-feature clippy, formatting and diff checks
passed. The root independently reran the daemon crash/control binary after freeze
and confirmed 11/11. Specification review is in progress; quality review has not
yet started, and 3A is not accepted until both independent review gates pass.

Specification review returned NEEDS WORK with two blocking findings. Repair
dispatch 1 addresses both: `3A-S1` must validate the actual materialized graph
payload against its declared hash and frozen reservation; `3A-S2` must require
all mandatory owned startup facts before any dispatch/execution or terminal
result. Matching binding or artifacts appended later cannot retroactively make
an untrusted start valid. Corrupt graph and late-binding regressions must retain
ownership in Attention without dispatch or terminal release. Specification
review must recheck these fixes before quality review begins.

Repair 1 captured five genuine cold-daemon RED cases: altered graph retaining
its declared hash, late binding before/after terminal and each mandatory
artifact arriving after execution began. All five failed before the fix.
The corrected crash/control binary initially passed 16/16. Startup validation
now hashes the graph payload against declared and frozen identities; startup
facts are taken only from the prefix before execution/lifecycle events while
the entire journal still folds in bounded pages. The strengthened crash/control
binary passed 16/16, including late binding on both sides of an actual terminal
result. Full persistence passed 435 tests with two ignored; CLI lifecycle and
ownership guards each passed their acceptance test. Six-crate strict
all-target/all-feature clippy passed. Source is frozen for independent
specification re-review; these build results do not predeclare acceptance.

Specification re-review accepted both repaired findings. The root independently
reran the frozen crash/control binary and confirmed 16/16. Independent
implementation and API/storage quality reviews are now in progress. The shared
repair count remains one; 3A awaits both quality verdicts.

Quality review returned NEEDS WORK. Shared repair dispatch 2 addresses
`3A-Q1` (a second live engine host can resume after the launch lock is released),
`3A-Q2` (conflicting definitive terminal events can release task ownership), and
`WI-API-01` (PR attachment does not establish the task project's GitHub identity).
Execution ownership must survive the startup boundary and release on process
death; ambiguous terminal evidence must retain Attention and ownership; PR
association requires host-validated repository identity before insertion.
Concurrency/death recovery, live/restart terminal ambiguity and SSH/HTTPS remote
identity regressions are required before both quality lenses re-review.

The first two-host probe passed before the fix because the run-writer lock
already prevented a second agent dispatch. The strengthened probe then failed
on an actual state change: the second host displaced a healthy live attempt from
Launched to Attention. This is the demonstrated ownership defect; duplicate
dispatch was not observed. Execution-lifetime claim retention must prevent the
contender from rewriting healthy ownership as well as preserve process-death
recovery.

Repair 2 also captured actual PR-association RED cases (unknown remote and a
foreign repository were accepted) and terminal-history RED (Completed followed
by Failed was trusted and released ownership). These regressions exercise the
daemon boundary with real isolated Git repositories. Process-death recovery and
live completion observation remain required in addition to cold recovery.

Repair 2's first focused crash/control run passed 23/23 (including four shared
mock fixtures and a child-process helper). Substantive scenarios verify that a
second live host leaves the original attempt Launched, killing a separate owner
process releases the OS claim and recovers the same run, live tracking errors
and restarted conflicting terminal evidence retain Attention/ownership, and
foreign or unknown GitHub identities are rejected. Upstream/remap replay cases,
affected full gates and both independent quality re-reviews are still pending.

Repair 2 retains the stable OS/SQL launch claim in the completion observer through
tracking confirmation and settlement. Unconfirmed tracking becomes Attention;
read-only owned replay rejects duplicate as well as conflicting definitive terminal
events. A killed execution host releases the OS lock and the same reserved run
can recover; this exercises mock ACP cold recovery, not provider-session resume.

Fresh PR attachment checks the pinned canonical Git common directory and checkout,
then matches the target against explicitly configured GitHub fetch remotes. HTTPS,
SSH and scp-style URLs normalize to repository identity; origin and upstream/fork
remotes are equally eligible. Unsupported or absent mappings fail closed rather
than infer a target. An already successful operation replays its original result
even if those remotes subsequently change. This does not authorize GitHub writes.
MCP work-item IDs intentionally address the shared local Surge home, consistent
with existing registry/read tools; its configured project root seeds creation,
not a per-project authorization boundary. Existing mutation guards still apply.

Repair 2 focused daemon acceptance is green: 23 tests, including a real killed
child execution process, live-host contention, live tracking failure, restarted
terminal ambiguity and PR mapping/replay. Its bounded six-crate full test gate
completed with 2617 passed, zero failed and 28 ignored (all binaries and doctests).
The demonstrated RED for contention was displacement to Attention; the pre-existing
run-writer lock already prevented a second mock-agent turn. Provider ACP session
continuation remains 3B. Both independent quality re-reviews accepted repair 2.
The root independently repeated the frozen daemon target and confirmed 23/23.
The shared post-build repair count is two; no review finding remains open for 3A.

### 3A acceptance and build gates

The six owning crates (`surge-core`, `surge-git`, `surge-persistence`,
`surge-orchestrator`, `surge-daemon`, `surge-cli`) completed their bounded
`cargo test --no-fail-fast` run, including all binaries and doctests. The initial
run had 2602 passes, three failures and 28 ignored tests; the failures were two
explicit current-schema expectations and the new `work_item: None` memory
snapshot field. Those exact fixtures were updated. The full core retry passed
856 tests with no failures; combined coverage is 2605 passes and 28 ignored tests.
No timeout or production fix was needed for these fixture failures.

The same six crates pass `cargo clippy --all-targets --all-features -- -D
warnings`. Registry migration upgrade, the schema-11 binding golden/version gate,
real inter-process launch locking, complete read-only history beyond 10,000
events, immutable revisions, bounded pagination and cumulative usage all passed.
Specification review and both implementation/API quality lenses accepted 3A
after two repair dispatches with failing-test-first evidence. Final six-crate
tests passed 2617 with zero failures and 28 ignored; final strict lint,
formatting and diff checks passed. Phase 3A is complete as a dependency increment;
the full product objective remains active. Actual provider
session continuation, suspension, desktop task controls and later phases remain
required. Tests use isolated homes, temporary repositories and mock agents.

Pre-code maintainer self-check: ACCEPTABLE direction, with independent adversarial
implementation-plan reviews required before writes. New task operations are cold
operator paths; bounded database reads and indexed ownership updates need no new
runtime, dependency or speculative optimization. Principal rejection risks are
cross-database crash ambiguity, lost worktree ownership, forgeable associations,
and readiness derived from stale requirements. 3B/3C and later phases remain
mandatory to satisfy the full objective; 3A alone cannot earn overall completion.

## 3B discovery and acceptance draft

3A is accepted; 3B design is under discovery and has not authorized a particular
implementation yet. Existing `stop_run` signals cancellation, which is folded
to `RunAborted`; agent cancellation shares this path. Introduce recoverable
suspension through an explicit durable intent and distinct outcome rather than
relabeling an aborted run. The task retains the same attempt/run, accepted
revision, PR, workspace and cumulative usage. Suspension acknowledges only
after the execution owner has stopped dispatch and durably preserved recovery
state; Continue cannot overlap that owner or silently create another attempt.

Observable acceptance for the complete 3B increment:

- Suspend an active task after an agent writes tracked and untracked changes:
  preserve both, flow position and every provider session identifier; no
  definitive abort/completion is recorded for the suspension.
- Close the daemon and reopen it: the explicitly suspended task stays suspended
  until Continue; Continue reuses its original run/workspace and validates the
  accepted context and execution ownership established in 3A.
- Actual mock ACP transport advertises resume or load: reconnect using the saved
  provider ID, same canonical cwd and fresh authenticated MCP endpoint; record
  which mode succeeded. Replayed load history cannot execute historical tools
  or accept historical outcomes as new stage reports.
- Missing capability, unknown session, changed runtime or invalid recovery data
  yields an explicit recoverable decision/Attention with files and identifiers
  retained. A new session is an explicit recovery choice, never mislabeled as
  continuation. Crash before the provider ID is durably recorded is handled as
  uncertain establishment rather than invented session recovery.
- Suspend/Continue/replay concurrent requests, cancellation during handshake,
  terminal-result races and owner-process death cannot dispatch twice, release
  another owner's reservation or discard uncommitted work.
- Subscription exhaustion preserves all session records and pending progress.
  Configured fallback or automatic availability selection actually dispatches
  an eligible provider while retaining the prior session for later recovery;
  with no eligible provider the task pauses. Known reset times may schedule
  recovery; unsupported quota checks remain explicitly unknown. Probe/retry
  policies must avoid both permanent stale-capacity parking and retry storms.
- CLI and MCP controls reach the daemon and expose meaningful suspend/continue
  and quota/recovery status; later 3C app controls use the same durable protocol.

Maintain MSRV/ACP v1 and reuse the installed SDK's session load/resume APIs.
Protocol behavior was checked against the [official ACP session setup
document](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v1/session-setup.mdx)
and locally installed `agent-client-protocol` 2.2.0 / schema 1.9.1 source.
Current capacity dispatch explicitly disables rotation, so existing parking is
not acceptance evidence for the required provider fallback. Exact types, edit
sites, migration and cancellation/ownership protocol need independent plan
review before code changes. 3B post-build review repair count starts at zero;
the accepted 3A history and its two repairs remain recorded above.

### 3B proposed protocol for independent pre-code review

Maintainer self-verdict: ACCEPTABLE direction, pending two independent plan
lenses. Full cross-crate review mode remains required. No production writes,
tests, dependencies or SDK upgrades have been made for 3B.

1. Add task Suspend/Continue controls with operation/body replay, expected
   version and existing attempt/run ownership. Registry migration 0023 stores
   suspend intent, control generation and confirmed suspension independently
   of accepted requirement revisions. Persist intent before engine signalling;
   a detached host confirms cleanup and durable suspension before acknowledging
   success. Retain active-attempt uniqueness. Continue reacquires the existing
   claim only after its execution owner exits; terminal races preserve the
   genuine terminal result. Reconcile crash-after-intent from journal/control
   state. Manual suspension does not auto-wake; legacy Abort remains Abort.
2. Separate Abort/Suspend interruption and add Suspended outcome plus durable
   RunSuspended/RunContinued events, schema 12 migration/golden gates and explicit
   replay/view/query/display handling. Quiesce agent and authenticated endpoint,
   persist post-cleanup verification observation and snapshot/cursor/pending
   stage identity before suspension. Unconfirmed cleanup becomes Attention and
   retains ownership; Pause must not manufacture failure, success or terminal
   abort. Continue preserves dirty/untracked files and original Git owner.
3. Represent the provider ID distinctly from Surge's SessionId. Persist immutable
   per-stage-invocation session descriptors (provider ID, canonical cwd, runtime
   and launch identity, observed capabilities, internal ID), not one latest
   session slot. Opening mode is typed New or Continue(descriptor, preference).
   Bridge opening metadata must reach persistence before the first prompt.
   Persist establishment intent before handshake; acceptance-before-ID-commit
   crashes remain uncertain recovery. Old provider-less records stay readable
   and explicitly non-resumable.
4. Capture fresh initialize capabilities, prefer supported resume then load,
   and send the saved provider ID/same cwd/current authenticated MCP endpoint.
   During load, historical notifications cannot execute tools or submit current
   stage outcomes; readiness begins only after restore response. Capability
   failure, unknown ID or changed identity yields actionable Attention without
   silently starting a new session. An explicit `Continue --new-session`
   recovery choice creates a recorded replacement in the same attempt and stage;
   it is never described as original-session continuation.
5. Enable real runtime routing: configured fallback order or automatic eligible
   registered runtime selection changes launch runtime while preserving graph
   role, sandbox, tools, outcomes and accepted context. Persist selected runtime
   and prior-session association before dispatch. Exclude exhausted, disabled,
   incompatible candidates and try each at most once per recovery cycle.
   No candidate leaves a recoverable capacity pause with all sessions retained.
   Optional availability probes distinguish supported observations, unsupported
   checks and failure; ACP provides no generic quota check. Known resets and
   bounded blind-backoff retries use the owned task wake route; manual pause is
   inert. Unknown availability is not asserted healthy, and forced cleanup must
   preserve typed rate-limit evidence.

Owning edits: core task commands/state/events/replay and runtime policy; ACP
session config/facade/worker/lifecycle/mock agent; orchestrator engine control,
snapshot/replay, run driver and agent stage; persistence task registry/run
materialization/inspection; daemon task owner/tracking/wake scheduler; CLI task
and MCP controls. Public bridge shape (descriptor query versus opened-result
metadata), per-invocation identity and control-generation protocol need explicit
plan verdicts. Reuse existing sibling crate responsibilities and standard APIs;
never move Git I/O into core or infer provider IDs from internal IDs.

Before production changes, the builder must capture an actual daemon + real mock
ACP subprocess outer RED. Wire assertions require saved provider ID, canonical
cwd, fresh MCP endpoint and no second new_session on supported Continue.
The acceptance matrix above covers both capabilities, rejected restoration,
historical replay, uncertain handshake commit, dirty work, process death,
concurrent controls, actual fallback dispatch and unsupported quota probes.
Affected suites and exact feature-set strict lint/fmt/diff gates precede source
freeze, specification review and two independent quality lenses. The shared 3B
post-build repair count is zero; accepted 3A repair history is unchanged.

### 3B mandatory pre-code refinements

The first independent plan verdict is RESHAPE NEEDED. These contracts become
part of the proposed design before implementation:

- RunSuspended is a durable journal fence containing control generation, pending
  stage invocation, snapshot/recovery sequence and confirmed cleanup. Registry
  suspension is confirmed only from the matching fence, never intent or socket
  closure. Unresolved cleanup forbids Continue.
- Operation replay precedes CAS. Terminal completion wins a pending suspension;
  an obsolete Suspend cannot affect a later Continue generation. Continue
  reserves its recovery generation durably before restore and holds ownership
  through handshake, execution and cleanup. Test each journal/registry boundary.
- Return typed OpenedSession metadata atomically with the internal ID, provider
  ID, invocation, launch/runtime, cwd, capabilities and actual opening mode.
  Constructor and deserialization validate the descriptor. Host persistence
  precedes prompting; persistence failure quiesces the opened session and records
  uncertain recovery. A mutable post-open latest-session query is rejected.
- Fence replay in the bridge and maintain authenticated invocation/candidate
  identity checks after restore completes. A historical notification arriving
  late cannot become current simply because load has returned. Test delayed
  notifications crossing restore-response boundaries.
- Separate recorded new-session quota rotation from provider-session restoration
  and explicit replacement permission. Restore failure cannot silently route to
  a fresh session as fallback. Persist recovery-cycle identity and attempted
  candidates so restart does not reset the bounded candidate set.
- Manual suspension cancels queued wake/fallback dispatch and dominates capacity
  recovery until explicit Continue. Availability observations record time and
  bounded expiry; unsupported or failed probes never clear exhaustion as healthy.
- Save execution phase, distinguishing an interrupted agent stage from committed
  outcome/effects awaiting edge routing. Existing snapshots after routing alone
  are insufficient for suspension at that boundary; Continue must not replay
  already committed effects. Exact phase/fence protocol needs the execution
  plan lens before code.

Both initial plan lenses returned RESHAPE NEEDED. Final proposed transition
protocol and additional execution-lens requirements:

| Current state / command | Durable order | Recovery rule |
|---|---|---|
| Executing / Suspend | Replay operation first; CAS item/run/attempt/control generation and persist SuspendRequested; signal current owner | Old generations cannot signal or confirm a later owner; true terminal result wins |
| Owner acknowledges SuspendRequested | Fence new dispatch; quiesce bridge/tools/endpoint; confirm cleanup; observe files; persist recovery snapshot plus matching RunSuspended fence; confirm registry suspension; release claim | Before journal fence: unresolved pause/Attention, no Continue; after fence before registry ack: reconcile matching fence; manual pause stays inert |
| Suspended / Continue | Replay first; CAS and durably reserve new control/recovery generation; acquire exclusive execution claim; validate fence/frozen context/files; restore saved phase/session; publish RunContinued; dispatch | Claim busy leaves original owner unchanged; death during restore preserves uncertain intent; exact operation replay never allocates new run/attempt |
| Suspended because of capacity / due wake | CAS observed recovery cycle and manual-control generation; persist selected candidate and attempted set; acquire same claim before restore/new routed session | Manual Suspend invalidates old queued wake; exhausted candidates remain attempted across restart; no candidate stays capacity-paused |
| Any active control / definitive terminal | Validate unique trusted terminal journal; settle exact run/attempt generation | Terminal wins concurrent pause; ambiguous evidence retains Attention/ownership |

Recovery snapshots use an explicit pending-stage phase. An interrupted stage
retains its invocation and saved descriptor for restoration. A validated outcome
with committed ledger/receipts is represented as CommittedOutcomeAwaitingRoute;
Continue performs the uncommitted route once instead of reopening the agent or
reapplying effects. Snapshot sequence identifies the trusted committed prefix.
Journal transactions/CAS must bind that prefix to the suspension fence; do not
assume a last routed-node snapshot proves pending effects absent.

Pending human/permission requests retain durable decision identity and recorded
resolution. Suspension cancels transport waiters, not the decision history;
Continue reattaches unresolved decisions with a fresh transport handle and does
not replay an already acknowledged tool receipt. Pending steers retain their
IDs/order and cancellation state. Stale permission handles, old MCP endpoints
and receipts from another invocation cannot authorize new execution. Uncertain
external tool effects are surfaced as recovery decisions, not blindly reissued.

Bridge ingress tags historical restoration traffic before publishing it. Both
queued history and delayed old-invocation traffic remain fenced after restore
response; current MCP invocation/receipt identity remains authoritative.
The logical stage invocation has a stable StageInvocationId across restoration;
each authenticated MCP connection receives a fresh StageGenerationId. Neither
is the work-item attempt's ownership generation. Continuing a logical stage
must not reuse the previous connection's tool authority.
Restrict the current two-attempt handshake timeout policy: initialization that
has not issued a session operation may safely retry, but timeout after
session/new/load/resume publication persists uncertain establishment/restoration
and never silently repeats an unclassified session operation.

Persist per-invocation runtime override and recovery-cycle generation, attempted
canonical runtime/account candidates, and next probe/wake without altering 3A
frozen graph/config. Deduplicate exhausted aliases/accounts using available
registered identity; unknown account identity is not invented. Eligibility uses
actual runtime/capabilities and configured policy. Add outcome-to-edge pause,
pending decision recovery, uncertain handshake timeout, queued late-history,
stale wake/control and forced-cleanup quota cases to executable acceptance.

Both independent plan re-reviews accepted this corrected design on 2026-09-30.
Implementation is covered by the existing user authorization to implement all
requirements. The next foreground phase is builder-owned outer acceptance RED,
followed by the complete 3B implementation and gates. The builder has been
dispatched under the existing authorization and owns the serialized Cargo lane.
No 3B implementation or acceptance pass is claimed from these design verdicts.

Initial 3B outer RED is captured through actual daemon and ACP subprocess:
the mock establishes a session and submits an authenticated report, but the
journal has no provider continuation ID (one failed test). Separately, the raw
task Suspend request after an actual controlled prompt receives an empty
response because the daemon has no route (one failed test). Earlier fixture
compile errors and incidental AgentMessage timeouts are not behavior evidence.
Full GREEN must additionally prove metadata durability before prompt and actual
restart/Continue wire identity; neither initial RED proves those later contracts.

The ACP library check currently passes with typed opening results and observed
capabilities. A separate real-wire RED showed automatic repeated session/new
after an uncertain timeout. The worker now limits automatic retry to safe
initialization and implements saved-ID load/resume without silent New fallback;
actual restore-wire tests and caller/control integration are still in progress.

The actual subprocess provider-identity test now passes (one test). SessionOpened
carries optional typed opening metadata for old-event compatibility, and the
agent stage writes it before prompt submission. This initial GREEN observes the
completed journal; the full transport/control acceptance must still prove the
durable-before-prompt crash boundary, historical replay fencing and same-session
Suspend/restart/Continue. Library checks alone do not establish those contracts.

Initial actual ACP restoration tests passed two tests: separate provider
processes restore the same independently generated ID and cwd, prefer resume
when both capabilities are present, use load when only load is supported, and
never create a second session when restore is unsupported. Wire logs contain
exactly one New and the matching saved-ID restore request. These bridge-only
tests have no stage MCP endpoint and do not yet prove the full daemon suspension,
fresh endpoint, historical notification fence or quota fallback acceptance.

The focused persistence control-intent test passes (one test), and the current
persistence/ACP/orchestrator library check passes. Run-writer suspension now
commits its snapshot and RunSuspended together against an unchanged exact journal
prefix and rejects unconfirmed cleanup. Agent opening records establishment
intent before handshake. These are build checkpoints only; daemon controls,
phase recovery, cleanup coverage and actual quota routing remain under build.

The Engine now has a separate suspension signal and a nonterminal Suspended
outcome, with interrupted execution distinguished from a committed result awaiting
routing. This is an implementation checkpoint, not a completed control protocol:
daemon confirmation must require the matching durable cleanup fence, including
when MCP teardown times out or sealing the suspension fails. Continue routing,
cold-owner coverage, historical ingress and quota-cycle integration remain under
build; no full 3B acceptance or review verdict is claimed.

The actual daemon/ACP Suspend anchor now passes one test: the durable request
interrupts the Engine, and the monitor verifies the journal/snapshot before
registry confirmation. Explicit matching RunSuspended cleanup-fence and absence
of RunAborted assertions were subsequently added and await the next run. This
checkpoint does not yet prove restart Continue, writer coverage, historical
notification handling or quota fallback.

The stronger Suspend/restart/Continue anchor first failed before restart:
the persisted attempt was Attention rather than Suspended because the tracking
confirmation rejected the new nonterminal outcome (`OutcomeMismatch`). Tracking
now recognizes confirmed suspension, clears it on RunContinued and preserves
definitive terminal precedence. A subsequent run exposed a stale launch claim:
claim acquisition allowed Suspended but validation did not. Validation now accepts
that state with the existing run/generation/token/active-assignment predicate;
the Engine additionally requires ContinueReserved before resuming it.

The expanded actual-wire test now passes one test
(`/tmp/surge-3b-suspend-continue.log`): confirmed Suspend without Abort, an isolated
host restart with manual suspension remaining inert, then same-provider-ID resume,
exactly one New, the same attempt binding and preserved untracked content. The
positive fixture explicitly advertises both restore capabilities. Its earlier
default unsupported capability also exposed ordinary terminal failure routing;
that case must instead retain recoverable Attention and an explicit replacement
decision, and remains under build. The daemon library check passes
(`/tmp/surge-3b-controls-check.log`). This happy path does not establish historical
ingress, cold surviving-writer coverage, quota fallback or full 3B acceptance.

The unsupported-capability outer test now passes one test
(`/tmp/surge-3b-restore-attention.log`): provider capability policy changes between
isolated processes with the same launch contract; Continue retains the active
attempt in Attention, produces no terminal Failed/Aborted and sends neither a
second New nor a prompt. A strengthened metadata-ordering test also passes one
test (`/tmp/surge-3b-provider-before-prompt.log`), checking durable identity at the
fixture's actual send_message forwarding boundary and establishment before
SessionOpened before the first authenticated receipt. Explicit replacement,
historical ingress, cold surviving-writer coverage and quota cycles remain
separate mandatory acceptance work.

### 3B owner-death cleanup evidence refinement

OS claim release proves the owning host stopped, not that its provider/tool
process tree stopped. SIGKILL skips Rust Drop and kill_on_drop. Cold Continue
must therefore require confirmed prior cleanup or read-only evidence that the
recorded old process identity/owned process group is gone before restoring an
agent. Persist bounded local process identity with opening metadata before prompt;
absence of that durable identity after uncertain establishment remains Attention.

PID alone is insufficient: process reuse, another boot, permission denied,
unsupported probes and probe errors must be distinguished from confirmed Gone.
Never treat an unreadable process as dead or kill an unrelated reused PID.
The existing Unix bridge creates a process group; surviving descendants matter
even when the provider leader has exited. Reuse project dependencies/OS helpers
where adequate; `ProcessTracker::is_running` alone is not confirmation because
its boolean signal-zero check loses permission and identity distinctions.

Known old-boot death or confirmed cleanup may permit automatic restoration;
Alive/Unknown preserves Attention and exposes a recovery decision. This does not
replace automatic power-loss continuation with mandatory approval when process
absence is provable. Add actual killed-host/surviving-provider tests followed by
confirmed process/group disappearance and same-session recovery, plus unknown
identity/probe behavior. Independent plan lenses must review this evidence
refinement before its implementation.

Independent probe refinement verdicts require explicit coverage of provider and
host-owned tool writers: current shell_exec starts processes outside the ACP
group. Persist local execution ownership/coverage before tool dispatch and record
process/group identities when established. Missing/incomplete coverage, including
uncontained descendants or uncertain external effects, is Unknown/Attention even
if the provider group is empty. A group probe never proves an unrestricted
process tree stopped. Known trustworthy old-boot disappearance can cover all
recorded old-boot writers; missing boot identities cannot establish this proof.

Probe result is typed Alive/Gone/Unknown. Validate stored platform/PID/start/boot/
group-or-job metadata on construction and deserialization. Gone requires complete
evidence for the recorded ownership domain, distinguishing permission errors from
absence. Cleanup fences bind exact invocation/control generation and tools/MCP
endpoints. The cold probe remains read-only and never signals a reused or
unverified PID/group. Test both surviving provider and surviving host-tool writer:
Continue sends no restore request until complete cleanup/Gone is established,
then restores the original provider session automatically.

Both probe plan lenses now accept the refined evidence contract, including the
execution lens's explicit host-tool coverage correction. Implementation may
proceed within the existing authorized 3B scope; these plan verdicts do not prove
process cleanup or restoration behavior.

The first process-evidence compile checkpoint passes for the ACP library
(`/tmp/surge-3b-process-check.log`). New read-only Linux/macOS helpers bind machine,
boot and precise process creation identity, retain probe failures as Unknown, and
do not infer complete disappearance from an empty GroupOnly container. Durable
pre-dispatch integration and killed-host provider/tool acceptance are still under
build. The macOS FFI uses unsafe blocks with safety notes and must receive its
independent soundness review in addition to the full spec and quality gates.

Primary-source check found that macOS kern.boottime cannot establish reboot
identity: XNU's calendar-clock setter adjusts clock_boottime when wall time is
changed, and the boottime sysctl returns that adjusted value. Use a trustworthy
immutable boot-session identity such as kern.bootsessionuuid, with unavailable
identity remaining Unknown; never infer old-boot writer death from a clock
adjustment. Sources: [Apple clock implementation](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/clock.c),
[Apple sysctl definitions](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sysctl.c).
The helper now reads bounded, NUL-checked kern.bootsessionuuid with no boottime
fallback, and core validation requires UUID shape for macOS/Linux boot identities;
old numeric boot records fail closed. This implements the correction within the
accepted trustworthy-boot requirement; probe acceptance is still pending.

Provider writer metadata now compiles through the daemon
(`/tmp/surge-3b-writer-metadata-check.log`). The same all-writers contract also
covers shell hooks: `crates/surge-orchestrator/src/engine/hooks/mod.rs` launches
shell commands separately from the ACP process group. Their pre-dispatch intent,
establishment and cleanup cannot be inferred from provider cleanup, just as for
shell_exec and MCP children. This is part of the accepted coverage requirement;
cold dispatch and actual killed-host acceptance remain under build.

Read-only MCP lifecycle discovery identifies additional owning seams:
`connection.rs::spawn_and_serve` starts arbitrary children before initialization;
connection retries and health-monitor respawns each need their own pre-dispatch
intent. Tool-call effects require receipts/uncertainty independently of child
lifetime. Registry construction must receive the run-scoped append-only recorder.
Connection shutdown currently returns unit and can rely on Drop or log a cancel
error; registry shutdown likewise logs child timeout/panic. Outer shutdown success
therefore does not establish every child's cleanup. Propagate typed per-child
evidence and retain process identity independently of RunningService ownership.
Fresh stage-endpoint authority and settlement remain separate from child/tool
death; endpoint timeout stays Unknown and credentials must not enter the journal.
This inventory is discovery only, not a code-review or acceptance verdict.

The real hook ownership regression is RED (zero passed, one failed,
`/tmp/surge-3b-hook-writer-red.log`): the shell creates its effect file while the
durable journal contains zero writer intents. This proves missing ownership
evidence after execution, not yet the pre-side-effect timing claimed by the test
name. Full acceptance must additionally observe committed intent before dispatch
and prove journal rejection prevents the shell effect. Fixture compilation
failures were corrected and are not behavior RED evidence.

The hook intent regression now passes one test
(`/tmp/surge-3b-hook-writer-green.log`), and a separate rejected-journal regression
passes one test (`/tmp/surge-3b-hook-rejected-journal.log`): a closed journal
prevents the real shell effect. Hook process observations remain GroupOnly rather
than claiming unrestricted descendant containment. OnError hook contexts now
carry recorder authority as well. Independent positive timing evidence,
shell_exec/MCP integration and killed-host coverage remain under build.

Host-hook integration compiles through the orchestrator
(`/tmp/surge-3b-host-hook-check.log`). Shared pre-dispatch ownership lives in
`engine/writer_coverage.rs`; real tool contexts receive recorder authority and a
stable invocation, while fixtures are explicitly unowned. The real shell_exec
closed-journal regression is RED (one failed test,
`/tmp/surge-3b-shell-writer-red.log`): the command still runs despite unavailable
durable ownership recording. Fix this dispatch boundary before claiming complete
host-tool coverage; no full cold-recovery verdict follows from hook-only GREEN.

The shell_exec refusal regression now passes one test
(`/tmp/surge-3b-shell-writer-green.log`): a closed ownership journal prevents the
real command effect. Dispatch records intent before spawn, isolates its group,
uses kill_on_drop and records process observation; observation-commit failure
kills/reaps the owned direct child. GroupOnly records do not establish complete
descendant/external-effect cleanup. Cold recovery must still treat their
unconfirmed coverage as Unknown until complete disappearance is proven.

The hook timing oracle is now strengthened: the real child reads the event
database using Python's standard SQLite reader and requires a committed ownership
intent before creating its effect file. This replaces the after-return-only
timing assumption; the strengthened run passes one test
(`/tmp/surge-3b-hook-before-effect.log`). The
immutable macOS boot identity regression passes one test
(`/tmp/surge-3b-boot-identity.log`), including constructor/JSON rejection of numeric
wall-clock boot tokens. These focused checks do not replace cold-owner acceptance.

The writer fold still needs correction before fence acceptance: SessionClosed is
an audit event, not independent confirmation of complete writer cleanup. Replace
that inference with explicit validated writer evidence and check every outstanding
boundary, including GroupOnly/external effects. The existing positive wire oracle
must remain, but cannot substitute for this all-writers coverage proof.

MCP child ownership now compiles through the orchestrator
(`/tmp/surge-3b-mcp-writer-check.log`). An injected HostWriterObserver preserves
crate dependency direction; each lazy child launch/respawn records a distinct
intent, dedicated group and actual PID before serve/init. Start/resume construct
independent run-owned registries instead of reusing an unowned live registry.
Per-call effects, typed cleanup and complete fence/cold-owner acceptance remain
under build. This library check is not their behavior evidence.

Typed MCP cleanup propagation now compiles through the daemon
(`/tmp/surge-3b-mcp-cleanup-check.log`): connection shutdown returns outstanding
handle/join errors, registry aggregates child timeout/worker failures, and the run
task refuses suspension sealing on inner errors as well as outer timeout. Field
documentation warnings in that compile were subsequently corrected; strict lint
and behavior gates remain pending. Successful transport cancellation is explicitly
distinct from complete writer/process/effect disappearance.

Actual MCP regressions now pass independently: rejected host intent prevents the
real child effect (one test, `/tmp/surge-3b-mcp-rejected-intent.log`); an actual
mock service holds an echo call behind a release-file barrier, shutdown reports
OutstandingHandle, and that original call later completes (one test,
`/tmp/surge-3b-mcp-live-cleanup.log`). This proves transport refusal/uncertainty,
not unrestricted descendant containment.

Focused independent design clarification found no host-established CoveredDomain
producer in the workspace. Group cleanup, leader reaping and endpoint settlement
cannot establish escaped-writer death. A concrete containment refinement is now
required before explicit all-writer cleanup can honestly acknowledge normal
same-boot Suspend/Continue or complete-Gone cold recovery. Candidate direction:
host-enforced fully-owned-write mode with inherited OS restrictions and authenticated
host mutation tools; existing runtime sandbox declarations are insufficient.
Protect checkout/common Git state, durable run authority and saved provider state,
and test aliases/escaping children plus genuine automatic positive paths. The
builder is preparing a maintainer-grade plan for both independent pre-code lenses.
This is unfinished 3B work, not a smaller completed substitute or a post-build
repair dispatch; the shared review repair count remains zero.

### Containment refinement awaiting default-mode choice

The builder's maintainer self-check is RESHAPE NEEDED. Seatbelt checkout denial
alone cannot protect a mutable provider resume store from escaped descendants,
and chmod/cache copying cannot revoke existing writable descriptors. No complete
process-domain mechanism or OCI runtime is currently available in this workspace/
host; a CoveredDomain enum value is not an implementation.

The concrete candidate uses an owned complete process domain: delegated Linux
cgroup-v2 with race-free pre-exec placement and validated empty-domain evidence;
Windows non-breakaway Job assigned before the child resumes; a supported OCI/VM
domain on macOS. Each attempt has durable domain identity and generation-fenced
launch admission. Saved provider state uses a persistent logical home/volume,
with exact saved provider ID, cwd, image/launch contract and volume association
retained through Continue. Credentials remain transient and never enter journal
records. Domain teardown must finish before snapshot/fence/claim release.

Untrusted children do not receive writable checkout, common Git, journal or
accepted-artifact mounts. Mutations go through hardened authenticated host tools
with pre-effect intent, atomic replacement and durable receipt. Check/build/cache
outputs are generation-owned staging; promotion to current code/evidence is an
explicit authenticated host action. Unknown remote effects remain unresolved
independently of local domain emptiness. Provider availability and authentication
must be checked in the actual chosen execution environment, not inferred from a
host-only executable or account probe.

Required executable evidence includes setsid/double-fork containment, killed-host
live-domain refusal with zero restore traffic, verified domain stop followed by
same-session restore, open-FD/hardlink/symlink protection, stale staging rejection,
domain/permission/capability mismatch and all existing wire/hook/MCP controls.
Both independent pre-code lenses must accept the refined plan before containment
implementation; existing phase-routing/history/quota work can continue separately.

One product preference question is pending: managed isolation as the macOS default
with native mode optional, or native default with explicit isolation configuration
and truthful Unknown/Attention where cleanup cannot be established. No runtime has
been provisioned, no choice is inferred from elapsed time, and no requirement is
retired by this question. This is clarification of a concrete setup/behavior
tradeoff, not a claim that recoverable execution is finished.

Both independent containment plan lenses return RESHAPE NEEDED. Select one
concrete supported backend/API and trusted owner first; persist inspectable domain
creation intent before launch and fence immutable domain/runtime-instance/boot/
volume identity across restart and name reuse. An enum or provider declaration
cannot produce CoveredDomain. Every executable writer must use that actual domain;
revoke launch/mutation admission and old endpoints, drain effects/receipts and
verify emptiness before publishing a suspension fence. Prevent child migration,
runtime-control-socket access and other actual routes outside the selected domain.
Keep unsupported paths truthful, without replacing all recovery with approval.

Host mutations need generation/revision/effect/base validation under the same
admission lock, root-relative alias/FD protection and durable idempotent receipts.
Pinned read-only check subjects and staging promotion must preserve existing
verification freshness. The provider-state volume has exactly one domain writer;
no replacement attaches it before prior complete-Gone proof. Authentication is
observed inside the chosen environment without broad host-home mounts. Add actual
backend launch-before-identity-commit, runtime restart/name reuse, mutation races
and all previously required escape/cold/same-session tests. These are pre-code
plan refinements, not consumption of the post-build repair budget.

Independent ACP uncertainty work progresses while this choice is pending: the
real handshake regression passes one test
(`/tmp/surge-3b-handshake-green.log`), never repeating uncertain session/new. The
positive retry fixture stalls initialize only. This does not complete historical
ingress or the remaining quota/phase/control acceptance.

The full isolated ACP control-lifecycle suite also passes 13 tests
(`/tmp/surge-3b-acp-control-lifecycle.log`), covering safe initialize retry,
uncertain session operation, cancellation/reaping, permission handling and
reserved-notification authority. These tests do not establish loaded-history
ingress or production complete-domain containment. Managed domain recovery must
validate its actual VM/runtime boot and immutable instance identity; host macOS
boot change alone cannot prove a persisted/restored VM domain disappeared.

## Next implementation discoveries

### Task interface and lifecycle

The current native task home in `crates/surge-ui/src/screens/fleet.rs` constructs
each `WorkTask` from a `UiRun`, its prompt and run-level pending decisions.
It does not yet expose the durable WorkItem requirement/discussion/attempt model.
The 3C interface must attach to that existing durable task identity and its shared
control protocol so multiple attempts stay within one task. Its current fallback
status label maps otherwise unmatched run states to Queued; suspended and recovery
states need truthful explicit presentation when the controls are connected.

Read-only 3C mapping identifies the data boundary: Fleet selection / rows should
use WorkItemId and daemon WorkItemDetail, with RunId retained for selected
historical attempts. AppState needs project-scoped durable task loading, errors,
pagination and reconnect refresh. Reuse Attempts / Revisions / Discussion shared
commands through DaemonEngineFacade; GPUI must not read task SQLite directly.
Task discussion uses Discuss rather than the existing run-scoped steer action.
Edit / AcceptProposal carry the displayed version and revision. Suspend /
Continue / Archive retain operation IDs across retries and display authoritative
pending or confirmed states, not success inferred from a button acknowledgment.

The connected API already exists in
`surge-orchestrator/src/engine/daemon_facade.rs` as DaemonEngineFacade::work_item.
WorkItemRecord.project is immutable, but the current List command accepts only
cursor and limit. Project-scoped UI pagination therefore needs a daemon-filtered
listing contract or complete correctly paginated filtering; filtering just the
first global page can falsely display an empty project. Keep archived task
history accessible without mixing current project identities.

3C acceptance must show two attempts under one task after restart; retained
requirements / discussion / PR / workspace; stale-version protection; reconnect
and project-switch pagination without lost tasks; active archive interruption
choices with files retained; and one shared app / Telegram decision identity.
This map is discovery, not a completed UI implementation or review verdict.

The current registry rejects Edit, AcceptProposal and Archive while an attempt
owns the task, while allowing discussion. The 3C lifecycle must offer the agreed
active-task edit/archive workflow through explicit interruption and preservation
choices, rather than treating that foundation guard as a finished interaction.
Archiving currently sets a timestamp and does not remove the checkout; any later
cleanup choice still requires actual PR, dirty/untracked and unpushed inspection.

For the later stage-policy increment, `crates/surge-core/src/config/pipeline.rs`
currently models GateConfig as boolean human gates after spec, plan, each subtask
and QA. Those switches do not prove the requested per-stage human/orchestrator/
independent-review policy or fresh-context same-provider fallback. The legacy QA
default of ten iterations is likewise not evidence of the accepted persistent
same-PR feedback loop; preserve that requirement in the phase-4 implementation.

### Concrete managed-backend discovery (not an accepted implementation plan)

Podman is a candidate for one initial managed backend, independent of the pending
native-versus-managed default choice. Its documented `create --cidfile` and exact
container-ID inspection permit separating creation from start. The creation
intent must precede even `create`; an uncertain result needs reconciliation
without starting another domain. The cidfile is not the durable ownership record.
The adapter must inspect the actual immutable ID and containment configuration.
See [Podman create](https://docs.podman.io/en/latest/markdown/podman-create.1.html)
and [container inspect](https://docs.podman.io/en/latest/markdown/podman-container-inspect.1.html).

On macOS, the remote client cannot use `--conmon-pidfile`; host PID evidence must
not be invented from VM process metadata. Machine inspection timestamps do not
establish an immutable guest boot identity. The documented
[machine SSH command](https://docs.podman.io/en/latest/markdown/podman-machine-ssh.1.html)
offers a possible controller-side read-only guest observation path, which still
needs a pinned, validated helper contract and real tests. Container `exited` or
`Pid: 0` alone is not the reviewed complete-writer proof. A replacement VM or
unavailable controller must not become inferred Gone from the macOS host boot.
No runtime has been installed or enabled by this discovery; both technical plan
lenses remain RESHAPE NEEDED.

Independent concrete discovery narrows the candidate to a host Podman adapter
plus a trusted guest controller, not remote inspect alone. Persist creation
intent, reconcile exact domain identity/configuration, and commit the full CID
before [attached non-TTY start](https://docs.podman.io/en/latest/markdown/podman-start.1.html).
The guest helper reports actual guest boot, rootless storage/controller identity,
PID namespace and owned cgroup identity. Revoke launch/exec admission before
stopping the exact CID; require identity-bound complete kernel emptiness, not
just CLI completion or namespace-init death. Inaccessible/replaced evidence is
Unknown. No runtime has been provisioned or tested.

All arbitrary executable writers must run inside that domain; protected host
checkout/Git/journal are not writable mounts. Persistent provider state belongs
to an exclusive guest volume with stable logical cwd/home and image contract.
A pinned Linux helper plus authenticated generation-scoped SSH/stdio relay is
needed for ACP/MCP, since the existing macOS Unix endpoint is not automatically
reachable from the VM. Host mutations still require separate intent/receipt
fences. These are concrete design seams, not an accepted complete containment
plan or positive backend evidence.

### Loaded-history authority and atomic outcome checkpoint

Actual ACP restoration now rejects a replayed permission request during
`session/load`, even with the permissive fixture policy. The focused recovery
suite passes 3/3 and control lifecycle passes 13/13 in
`/tmp/surge-3b-ingress-lifecycle.log`. This proves the startup ingress boundary;
delayed old-generation MCP receipts and complete containment remain separate
requirements. The strengthened fresh-prompt assertion now also passes: historical
permission is Cancelled, while the new prompt receives Selected(allow). The real
ACP provider-recovery suite passes 3/3 in 0.33 seconds in
`/tmp/surge-3b-history-current-permission-green.log`.

The next transactional oracle has reached a behavioral RED: a real SQLite trigger
rejects the required TaskStatusChanged append, but OutcomeReported remains in the
journal (`/tmp/surge-3b-outcome-atomic-red.log`, 0 passed / 1 failed). Accepted
outcome and its required ledger effects must commit together; route events and
their post-route snapshot likewise need one prefix-checked transaction. This is
build discovery, not a post-build repair; full 3B repair count remains zero.

The first fix now passes the full engine task-ledger suite, 16/16 in 0.64 seconds
(`/tmp/surge-3b-outcome-atomic-green.log`). Accepted outcome and required ledger /
discovered-task effects use one existing AppendBatch transaction; the rejecting
SQLite trigger leaves no OutcomeReported. Logical-invocation commit identity and
pending-phase recovery remain unfinished; atomic routing/snapshot publication is
verified separately below.

The routing oracle now reaches a genuine behavioral RED after correcting its
startup fixture: a real Engine accepts an outcome, then SQLite rejects the
post-route graph snapshot while EdgeTraversed / StageCompleted remain committed.
`/tmp/surge-3b-route-atomic-red.log` reports 4 passed / 1 failed in 0.15 seconds.
The fix must prepare graph/cursor/traversal-counter changes on a separate state,
commit routing events and snapshot under one expected-prefix transaction, and
publish the prepared in-memory state only after success. The earlier subscriber
timeout and compile failure were fixture failures, not behavioral evidence.

The owning routing rollback oracle now passes (one boundary test plus four
shared-fixture tests, `/tmp/surge-3b-route-atomic-green.log`, 5/5 in 0.14 seconds).
RunWriter commits EdgeTraversed, StageCompleted and the snapshot in one SQLite
transaction with an expected-prefix check; the engine prepares a clone and
publishes its cursor/counters after commit. The read-boundary adjustment was
subsequently rerun successfully below. State must match the event fold, and
forced-restart recovery must still prove one routing commit and zero provider
execution after an already committed outcome. This checkpoint does not prove
the complete pending-phase recovery contract.

Snapshot inspection clarifies that RunMemory is reconstructed from the journal,
not serialized inside EngineSnapshot; a post-route node-visits fold therefore
does not by itself prove lost snapshot memory. The remaining concrete window is
a fallible event read after successful route commit, which can report failure
despite a durable transition. Preparing the exact routing fold before commit
allows successful persistence to be followed only by in-memory publication.

That adjustment has now been rerun: the owning route suite passes 5/5 and the
task-ledger suite passes 16/16 in `/tmp/surge-3b-stage-commits-green.log` (0.14 and
0.70 seconds). Exact projected routing events are folded before serialization;
after a successful transaction the engine only publishes its prepared state.
The durable invocation marker and cold committed-outcome recovery oracle remain
the next mandatory boundary; these passing suites do not substitute for them.

The strengthened first-route rejection fixture also passes 5/5 in 0.12 seconds
(`/tmp/surge-3b-route-publish-green.log`): neither route events nor a graph snapshot
are published. Its baseline intentionally has no prior graph snapshot, so this
does not prove preservation of an existing cursor/counter snapshot. Current
execute_inner handles committed outcomes through an existing suspension fence;
cold death before that fence must additionally recover from a trusted matching
logical-invocation commit marker, never from a legacy bare OutcomeReported.

The producer checkpoint now has a failing real-stage test:
`/tmp/surge-3b-marker-red.log`, 0 passed / 1 failed in 0.07 seconds. Successful
execute_agent_stage leaves no StageOutcomeCommitted marker. This presence test
only establishes missing producer metadata; it is not a functional cold-recovery
oracle. Acceptance additionally requires validated owner/effects binding,
rejection of forged or nonadjacent records, and actual recovery with zero
re-execution of the committed invocation.

The producer implementation now appends StageOutcomeCommitted last in the same
AppendBatch as its outcome and required effects. Its private validated contract
uses constructor-backed deserialization, non-nil run/session/transport-generation/
logical-invocation IDs, a bounded effects count, and the ordered payload digest.
Compilation and the producer suite now pass: 17/17 in 0.69 seconds in
`/tmp/surge-3b-marker-producer-green.log`. The new producer test only checks marker
presence. Trusted adjacency/session inspection and cold recovery are not yet
connected, so this result does not establish their acceptance.

Binding inspection found three distinct session identities: the host-generated
MCP authority nonce created before open_session, the internal provider connection
handle returned by the bridge, and the provider's actual saved ACP session ID.
The marker must retain the authenticated context and a separate connection
handle, matching SessionOpened by that handle, node and stable invocation. It
must not assume the context nonce equals the bridge handle. Historical authority
never authorizes reuse of the old endpoint on Continue; restoration still creates
a fresh transport generation. Distinct-value fixtures must exercise this seam.

Trusted-history validation has reached a behavioral RED:
`/tmp/surge-3b-marker-inspection-red.log`, 0 passed / 1 failed in 0.06 seconds.
The persisted history reader accepts a marker whose adjacent effects digest is
forged. Initial fixture compilation errors were not counted as behavioral RED.
The corrected negative test must be paired with an otherwise identical valid
history and a specific corruption diagnostic so GREEN proves marker validation,
rather than rejection of unrelated startup metadata.

The history reader uses 256-event pages, whereas a validated commit can bind up
to 4096 preceding effects. Its bounded rolling validation window must therefore
survive page boundaries. Acceptance must include an actual effects/marker batch
straddling a page boundary, plus repeated-node invocations whose older marker
must not authorize a newer stage. Retain the existing complete-history and
contiguous-origin checks; do not restore a whole-journal buffer to validate this.

The first trusted-inspection fix passes its owning test, 1 passed / 398 filtered
in 0.18 seconds (`/tmp/surge-3b-marker-inspection-green.log`). It checks adjacent
count/digest, accepted outcome, allowed ledger effects, run and opened-provider
identity; the valid twin is accepted and the forged twin fails with the specific
digest-mismatch diagnostic. The rolling 4096-event window is outside the paging
loop. Repeated-stage fencing, page-boundary execution and cold route-once
acceptance remain incomplete. Conflicting reuse of an internal connection handle
must not overwrite trusted identity evidence silently.

The authority adjustment has rerun its first owning test successfully
(`/tmp/surge-3b-marker-authority-green.log`, 1 passed / 398 filtered, 0.10 seconds).
Source now clears prior stage bindings on StageEntered. Dedicated repeated-stage
and page-straddling fixtures are still required; the unchanged focused test does
not prove those new branches.

Independent read-only discovery confirms the cold-recovery connection: replay
must expose an authenticated outstanding commit before enter_stage, and the
route transaction must identify which commit it consumes. On restart after that
transaction but before registry acknowledgment, reconcile the existing route
rather than traverse again. The outer oracle uses a terminal successor and an
actual ACP outcome with owner death at both boundaries, asserting zero repeated
provider traffic and one effect batch / route. This discovery is not a 5a or 5b
acceptance verdict.

The page-straddling fixture now passes its valid/forged twins with an actual
281-effect batch crossing the 256-event read boundary
(`/tmp/surge-3b-marker-page-green.log`, 1 passed / 398 filtered, 0.12 seconds).
SessionEstablishmentRequested now persists the host-issued MCP authority before
provider RPC; inspection requires that exact context and the current stage node.
Stage entry clears active authority/connection bindings, and conflicting
historical connection reuse or duplicate logical-invocation commit is rejected.
Those branches still need their dedicated negative fixtures; the page twin
establishes paging/digest behavior, not the full cold recovery contract.

The cold-recovery reducer is being connected through StageRouteCommitted,
referencing the logical invocation and the exact outcome commit sequence.
Recovery must reject orphan, mismatched or duplicate consumption records;
silently ignoring an unmatched route is not evidence of safe recovery. The route
consumption record belongs in the same atomic transaction as edge/completion
events and the post-route snapshot. Source scaffolding is not yet a passing
restart oracle.

The per-run consumption projection must rebuild from the authoritative journal.
Existing views::rebuild deletes graph_snapshots despite their host-authored
frame/counter/checkpoint state not being derivable from ledger events. The agreed
fix preserves these durable snapshots when rebuilding computed projections and
validates their anchor/schema/route identity during replay. Missing or corrupt
consumed-route snapshots require RecoveryRequired / Attention, never graph.start
or repeated execution. The preservation change is implemented and its asserted
rebuild test passes, as recorded below.

The first real-ACP cold fixture now holds the host immediately after accepted
batch persistence, kills that host, then reopens the same task storage. Root
inspection found a weak oracle: stopping on Attention and checking only zero
extra provider traffic can pass without any route. The functional acceptance
must positively assert one matching consumed-commit / edge / completion and the
expected successor state, besides retained binding/workspace and zero extra
provider operations. Explicit fixture-provider cleanup is not production
containment proof, and uncertain writer cleanup must not be bypassed to force
this oracle green.

The corrected real-ACP cold oracle now reaches a genuine behavioral RED:
`/tmp/surge-3b-cold-commit-red.log`, 0 passed / 1 failed / 27 filtered in 0.41
seconds. After accepted-result persistence and owner death, reopening produces
two provider prompts instead of one. This demonstrates actual repeated execution
across restart, independently of the marker-presence tests. Positive one-route /
expected-successor assertions remain mandatory before claiming the eventual
GREEN; explicit fixture cleanup still does not establish production containment.

The cold fixture uses the checked-in minimal graph: one impl_1 agent stage whose
done edge goes directly to Terminal::Success. Its second prompt cannot be a
legitimate successor-agent request, so the observed RED does identify repeated
execution of the committed source stage.

Cold route preparation must respect the replay watermark: initial recovery
already folds the full trusted journal into RunMemory, while the live route
helper folds a stage-tail range. Reapplying that range after replay can repeat
usage/ledger/visit effects even when no provider prompt is repeated. The owning
restart oracle must also preserve exactly-once usage and ledger effects, with
route preparation starting after the already applied prefix.

The owning real-ACP cold test now passes: `/tmp/surge-3b-cold-commit-green.log`,
1 passed / 27 filtered in 0.35 seconds. After accepted-result persistence and
owner death, the same attempt reaches Completed with exactly one accepted
commit, edge, stage completion, route commit and RunCompleted. Wire evidence
shows one original New/prompt and no additional Load/Resume. A replay-prefix
watermark prevents route preparation from folding accepted effects again.
The fixture explicitly tears down its provider; this proves stage-phase recovery,
not production complete-writer containment. Death after route commit before
registry acknowledgment, orphan-route rejection, rebuild/checkpoint retention
and wider affected-suite gates remain unverified.

The orphan-route regression needs recapture. Its initial append failure hit the
writer projection guard; the later 0/1 reader failure used a bare EventPayload
instead of the production VersionedEventPayload envelope and therefore did not
prove the claimed reader defect. The corrected exact-encoding fixture is being
rerun against the pre-guard reader before claiming RED/GREEN. Intentional writer
Internal rejection is currently mapped to WriterTaskDied even without actor
death; new route/prefix rejection needs a distinguishable typed error.

The corrected production-envelope regression has now been recaptured with only
the new reader guard removed: 0/1 in 0.06 seconds at the intended orphan-route
assertion. Restoring the guard passes both owning inspection tests
(`/tmp/surge-3b-route-inspection-green.log`, 2 passed / 398 filtered, 0.16 seconds),
including the valid/forged effects twins. Current-stage ownership, missing/corrupt
snapshot recovery and after-route-before-ack restart coverage remain separate
requirements; this guard result is not their acceptance evidence.

With current-stage route ownership and typed OperationRejected added, the real
completed-outcome cold test still passes (`/tmp/surge-3b-cold-route-guards-green.log`,
1 passed / 27 filtered, 0.40 seconds). Replay now refuses consumed routes lacking
a checkpoint at or after the consumed sequence. Dedicated missing/corrupt
checkpoint tests must still prove task Attention / RecoveryRequired and zero
provider traffic; the normal cold test does not establish this failure path.

The extended real cold test also passes after rebuilding projections:
`/tmp/surge-3b-cold-rebuild-green.log`, 1 passed / 27 filtered, 0.44 seconds.
It asserts the authoritative checkpoint remains byte-identical and the rebuilt
accepted / consumed invocation counts remain (1, 1). This is new positive
rebuild evidence, not a retroactive pre-fix RED and not complete containment.

The complementary terminal registry-ack crash test passes in the current cold
suite (`/tmp/surge-3b-cold-route-ack-green.log`, 4 passed / 25 filtered, 0.44
seconds). Its isolated SQLite registry barrier permits the run journal to reach
RunCompleted before owner death; recovery reconciles the terminal task with one
route and unchanged provider traffic. Two tests in this count are existing cold
startup cases. This proves terminal acknowledgment recovery, not the distinct
ContinueReserved → routed snapshot → RunContinued/control-ack crash interval;
that control-phase variant remains required.

The suspended committed phase currently records a journal prefix without stable
invocation identity. Its pending-phase contract must instead reference the actual
accepted marker invocation and commit sequence, separately from the suspension
snapshot cut. A matching already-consumed route resumes from its validated
post-route cursor and reconciles Continue acknowledgment; it never routes again.
Unbound or ambiguous historical phases require explicit recovery attention.

The invocation/marker binding and consumed-route skip are now present in source.
The initial daemon check passes (`/tmp/surge-3b-control-phase-check.log`, 18.89
seconds), but later non-provider/fail-closed edits await the focused run. Root
inspection found invalid-binding branches still calling terminal failed(); these
have since been changed to a shared nonterminal RecoveryRequired helper.
Dedicated owning tests must still prove retained-attempt Attention and zero
provider traffic. A consumed sequence alone cannot replace validation of the
matching post-route checkpoint/cursor.

The existing four cold tests pass again after this adjustment
(`/tmp/surge-3b-control-phase-cold-green.log`, 4 passed / 25 filtered, 0.43 seconds).
The filename does not imply control-ack coverage: these remain the prior terminal
and startup cases. Exact route-owned checkpoint/target validation has also been
written but awaits rerun; missing/corrupt checkpoint and Continue-ack crash
variants remain mandatory.

## Full-path checkpoint

The strengthened pre-fix Continue experiment contradicts the earlier defect
claim: `/tmp/surge-3b-nonterminal-continue-ack-red.log` was overwritten by the
recapture and actually reports 1 passed / 34 filtered, 2.22 seconds, with the
added launch reconciliation removed. The earlier immediate status failures
were observation races and must not justify a fix or count as behavioral RED.
Any retained reconciliation change needs separate demonstrated necessity.

Independent source inspection instead finds a pending-decision recovery gap:
execute_human_gate_node allocates a fresh GateRequestId on dispatch, while
execute_human_gate_stage always appends HumanInputRequested. The current >=1
request oracle permits replacement / duplicate decisions. Required acceptance
captures original node / call_id / requested_seq, restarts, requires unchanged
identity with one actionable request, and resolves using the original ID. A
resolution-before-owner-death case must avoid another request or duplicate route.
The owning actual host-death test now reproduces it:
`/tmp/surge-3b-decision-identity-red.log`, 0 passed / 1 failed, 2.13 seconds,
at the intended count assertion (two HumanInputRequested events instead of one).
The original request exists before owner death; cold recovery registers the
actual run and then creates a replacement. Rehydration and GREEN remain pending.

Pending-request rehydration now passes the same owning test:
`/tmp/surge-3b-decision-identity-green.log`, 1 passed / 34 filtered,
2.43 seconds. RunMemory replay retains the original request and dispatch attaches
a fresh receiver using its ID; the test requires one request and resolves through
the original node / ID without extra provider traffic. This closes the observed
pending-request duplication only. Durable-answer-before-success, resolved gate
completion / route crash intervals, contradictory occurrence handling, bootstrap
effects and full affected-suite / independent review gates remain mandatory.

Recovery must connect to the Engine's actual RunMemory replay, not only the
separate RunState pending-input projection. Keep request node / ID / sequence
and stage-occurrence identity, original options / deadline, accepted response
and consumed route identity. The pending projection clears on resolution and
therefore cannot alone restore an answered gate before routing.

Operator-answer durability is another required boundary: resolve_gate_input
currently reports success after an in-memory oneshot send; the stage records
HumanInputResolved later. Persist acceptance under the exact request / occurrence
and current authority before reporting success. Identical replay is safe;
conflicting or stale replies cannot replace the decision. Bootstrap decision /
edit / outcome effects must commit or reconcile once, so replay cannot increment
edit counters or emit another approval. These are source-backed implementation
requirements, not yet owning crash-test acceptance evidence.

Fresh and recovered responses need the same validation against the original
request's allowed options / schema. OutcomeKey syntax alone is not acceptance
authorization. Current fresh waiter delivery does not enforce the restored
branch's allowed-option check; resolve this before durable success. A graph
amendment must not silently substitute new options for a retained old request.
Preserve the existing explicit free-text outcome policy: allow_freetext gates
emit a string schema without enum and accept valid outcome keys; listed-option
gates enforce their enum. Missing enum is not by itself corrupt authority when
the original authenticated request deliberately enables free outcomes.
Use the existing owned run writer for acceptance; opening another independent
writer from the resolver would introduce a second mutation authority.

The application-visible acceptance boundary is the daemon ResolveGateInput RPC:
server.rs maps resolver success directly to ResolveHumanInputOk, and the connected
DaemonEngineFacade exposes that call. Its owning negative test should reject
HumanInputResolved storage, require an error rather than successful RPC reply,
then verify restart retains the original unresolved decision. Receipt replay is
keyed by the durable gate occurrence / ID and exact response, not the transient
RPC request ID.

The socket-level storage-rejection test now reproduces premature success:
`/tmp/surge-3b-gate-durable-answer-red.log`, 0 passed / 1 failed, 0.42 seconds.
With HumanInputResolved inserts rejected, actual ResolveGateInput still replies
resolve_human_input_ok. The failure is at that intended response assertion,
not setup or timeout. The fix must report rejection, preserve the original
actionable request for retry and return success only after its exact accepted
answer is durable; GREEN and crash-after-acceptance proof remain pending.

The owning daemon socket oracle now passes:
`/tmp/surge-3b-gate-durable-answer-green.log`, 1 passed / 35 filtered,
0.30 seconds. ResolveGateInput uses the existing writer capability to commit
acceptance before removing the waiter; storage rejection returns an error and
the same original ID succeeds after the trigger is removed. Successful RPC
already sees one HumanInputResolved in the journal. Stage delivery verifies that
exact committed sequence rather than appending another resolution. This proves
the rejected-write / successful-retry boundary, not all post-acceptance crash,
bootstrap effects, skill-trust, timeout or receipt replay cases.

The actual daemon receipt-retry oracle now fails at the intended completed-run
boundary: `/tmp/surge-3b-gate-receipt-red.log`, 0 passed / 1 failed,
0.32 seconds. Identical retry of the accepted answer returns an error after
active-run exit instead of recovering its successful receipt. Restart and
conflicting-body assertions occur later and have not yet been reached. The fix
must read exact trusted request / answer history without creating a new answer
or depending on the former in-memory waiter.

The first receipt implementation regressed live acceptance because pure
RunState::apply initialized Pipeline memory without its graph, whereas the
Engine's RunMemory replay retained PipelineMaterialized. Purpose therefore read
as Unbound in trusted inspection. Initial Pipeline graph-memory alignment is now
written. The latest rerun still fails completed-answer retry
(`/tmp/surge-3b-gate-receipt-green.log`, 0 passed / 1 failed, 1.45 seconds),
so receipt replay is not accepted and the filename must not imply GREEN.

The corrected receipt oracle now passes:
`/tmp/surge-3b-gate-receipt-green.log`, 1 passed / 36 filtered, 0.32 seconds.
It exercises actual daemon socket storage rejection / original-answer retry,
waits for task completion, replays the identical answer after active-run exit,
rejects a changed response body, and replays through a new Engine owner with
exactly one HumanInputResolved. Inspection preserves original listed-option or
explicit free-outcome policy. This does not prove death between accepted answer
and waiter delivery / bootstrap completion / route; those crash cases remain.

After original request configuration / timeout preservation, the two daemon
socket answer tests pass together (`/tmp/surge-3b-gate-combined.log`, 2 passed /
35 filtered, 0.38 seconds). Both owning fixtures use free-text outcome policy;
this count is not independent coverage of listed-option enforcement.

Occurrence and orphan-response guards are now present in the recovery reducer:
requests bind to their StageEntered sequence, responses require that same node /
occurrence, and restoring a request avoids another StageEntered. These guards
still need owning regression coverage. Completion removes the outstanding
record, so accepted-answer retry receipts must remain recoverable from durable
history after route completion / active-run removal, independently of a waiter.

The same PendingGate transport also serves skill-binding trust requests. Those
decisions approve / reject specific skill content; they are not HumanGate route
outcomes. Recovery must distinguish their purpose and consumption authority:
an approved skill followed by an agent's success outcome must not become a gate
conflict or suppress a later legitimate StageEntered. Cover skill binding through
actual stage completion and revisit, in addition to the human-gate tests.

The owning Engine skill-binding regression now reproduces this collision:
`/tmp/surge-3b-skill-purpose-red.log`, 0 passed / 1 failed, 0.73 seconds.
An actual approved unpinned skill and successful agent StageCompleted leave a
conflicting outstanding HumanGate record because approval is compared to the
agent's route outcome. Purpose separation and GREEN are pending. Skill trust
must retain its content-hash authority; schema text alone cannot authenticate
the type of decision.

Purpose separation now passes the full owning skill integration file:
`/tmp/surge-3b-skill-purpose-green.log`, 8 passed / 0 failed, 0.91 seconds.
The regression approves an actual unpinned skill, waits for Engine completion,
and replays its journal with no outstanding HumanGate record. Purpose derives
from the accepted graph node; matching SkillBound consumes the host-stamped
name / provider / content-hash approval rather than comparing it to an agent
route outcome. This does not establish interrupted skill-approval recovery,
all gate receipt cases or full 3B verification.

The nonterminal oracle waits for recovered Engine ownership before inspecting
control authorization. It also passes with the experimental reconciliation
(`/tmp/surge-3b-nonterminal-continue-ack-green.log`, 1 passed / 34 filtered,
2.11 seconds). Passing with and without that change establishes this fixture's
positive behavior, not the change's necessity. Earlier immediate assertions and
the misleadingly named 2/3 green log are superseded observation-race results.
They must not count toward failing-test-first acceptance.

The two actual Continue-owner death intervals now pass with a terminal successor:
`/tmp/surge-3b-continue-owner-death.log`, 2 passed / 32 filtered, 2.17 seconds.
One kills the owned host after route commit before journal RunContinued; the
other after journal RunContinued before registry acknowledgment. They preserve
original-operation replay / generation and assert one route and acknowledgment
without repeating provider traffic. Initial timeouts were fixture setup failures
around the old isolated socket, not behavioral RED. The nonterminal positive
additionally checks matching-generation Executing before terminal completion
and original operation replay without a new generation or another RunContinued.
Exact pending-decision reuse remains separately unverified.

The distinct rejected Continue acknowledgment oracle passes with transactional
invocation / node / outcome identity checks:
`/tmp/surge-3b-continue-route-identity-green.log`, 1 passed / 31 filtered,
0.44 seconds. It suspends after an actual accepted provider result, records an
invocation-bound pending phase, rejects RunContinued after the route commit,
observes Attention without terminal events, then continues to completion.
Accepted outcome, route, edge, stage completion, acknowledgment and run completion
each occur exactly once; actual wire traffic remains one New and one prompt,
without Load or Resume. This is a rejected-ack recovery test, not process death
in the Continue control-ack interval. The projection identity query uses the bare
ULID storage representation, not the prefixed Display form.

The owning consumed-route checkpoint cases now pass:
`/tmp/surge-3b-checkpoint-attention.log`, 3 passed / 28 filtered, 0.43 seconds.
Missing and corrupt snapshots retain the same attempt in Attention, produce no
terminal event and make no additional provider New / Load / Resume / prompt.
The third case is the terminal registry-ack positive. The corrupt fixture uses
the actual graph_snapshots.snapshot column; its earlier SQL column error was
fixture failure, not a behavioral RED. These cases do not establish complete
writer containment; separate Continue crash evidence is recorded above.

Before expanding integrations, prove GitHub issue → ACP implementation →
verification → PR → failing CI → repair → fresh verification → merge decision,
including a forced daemon restart during the workflow. Each phase must keep its
own independently checkable acceptance evidence.

### Historical 3B acceptance snapshot (before 2026-10-03 quota integration)

Focused tests establish individual boundaries; they are not full phase approval.

| Required boundary | Current evidence / remaining work |
|---|---|
| Actual provider session continuation | Owning Suspend / Continue wire tests; broader recovery matrix still required |
| Accepted outcome and route exactly once | Atomic commit, cold route and consumed-checkpoint cases pass |
| Pending human decision retains identity | Actual owner-death RED → GREEN; wider occurrence / timeout regressions required |
| Answer durable before API success | Socket storage-rejection RED → GREEN, receipt replay and accepted-answer owner-death case pass; broader gate matrix remains |
| Bootstrap decision effects exactly once | Required-outcome rollback RED → GREEN and HumanGate unit slice pass; request-bound effects / route crash cases remain |
| Historical tools and replies lack current authority | Ingress cases pass; late authenticated MCP receipt / generation cases remain |
| Complete writer cleanup before continuation | No production CoveredDomain producer established; concrete backend and owning containment tests remain |
| Quota fallback and automatic owned wake | Rotation remains inert in current source; durable candidate cycle / override / probe / task wake required |
| Full phase verification | Affected suites, strict lint / formatting, spec review and independent quality / unsafe review remain |

### Remaining 3B quota integration seams

The quota storage foundation's initial precode review required reshaping. The
revised plan passed both independent lenses before implementation. Persist wake
basis as ObservedReset or PolicyBackoff; unsupported
or unknown probes may schedule bounded retries without becoming Available.
Cycle rollover must preserve prior attempted history and CAS the current control
and revision, so concurrent wakes cannot create competing retry cycles. Manual
Suspend invalidates both wake eligibility and control rebinding. Canonical
runtime / account keys remain opaque; persistence does not resolve aliases or
infer account independence. Owning reservation / restart / conflicting-retry /
stale-claim tests are required; raw duplicate placeholder SQL is insufficient.
Registry 0023's existing placeholder rows must remain nonactionable audit through
the new migration, without fabricated dispatch or wake authority.

The revised storage plan now has an acceptable independent spec verdict: wake
consumption / cycle rollover is one exact-claim / control / revision transaction,
and reservation replay returns a receipt without a second dispatch grant.
Technical final acceptance also passed; storage implementation is in progress.
The UI plan has both
independent precode acceptances; surge-ui-only implementation is authorized with
real-facade and native gates, while active edit / archive and bootstrap-to-task
backend connections remain mandatory before 3C completion.

Source discovery confirms that RotationPolicy is intentionally inert in
`surge-core/src/capacity.rs`; the engine's Rotate branch logs and dispatches the
original runtime. This does not satisfy accepted fallback behavior. Existing
canonical runtime capacity storage and typed RateLimited handling are useful
foundations, but ACP health's in-memory guessed reset is not a quota probe.

Generic parked-run wake does not own task-control recovery. Task quota wake must
reserve the matching task/control generation and durable cycle before dispatch,
and manual Suspend must invalidate it. Persist attempted canonical candidates,
known account association, selected stage runtime override, probe evidence /
expiry and bounded next wake; do not change the frozen graph/config. A volatile
one-resume precheck bypass cannot enforce once-per-cycle across restart. No
generic ACP quota endpoint has been found, so unsupported/unknown probes must
remain explicit rather than treating binary health as recovered capacity.

### Accepted-answer crash and parallel implementation checkpoint

The owning accepted-answer crash case passes in
`/tmp/surge-3b-gate-answer-crash.log`: 1 passed / 37 filtered, 0.33 seconds.
The test receives successful ResolveGateInput over the isolated daemon socket,
checks the committed answer while the human-gate outcome transaction is delayed,
kills its owned daemon host, and cold-recovers the same stored attempt. It asserts
one original request and answer, two total agent-plus-gate outcomes, completions
and edges, one run completion, and successful identical receipt replay after
restart. Actual mock ACP traffic remains one New and one prompt, with no Load or
Resume. Initial import and wire-field fixture mistakes are not behavioral RED.
This is recovery evidence, not proof of full writer containment or atomic
bootstrap decision effects.

The UI outer regression has a genuine behavioral RED in
`/tmp/surge-3c-zero-run-red.log`: 0 passed / 1 failed, 0.98 seconds after successful
compilation. A real DaemonEngineFacade socket response supplies a durable task
with no runs; the rendered Fleet still shows its empty state and lacks the task
selector. This establishes the missing row behavior through transport and a GPUI
frame, not real-daemon stored-task mutation or full native interaction acceptance.

The quota migration and owning storage API are being implemented separately from
the UI and main recovery code. The integration must account for automatic
Capacity Suspend changing control generation: only an authoritative matching
capacity fence may rebind a cycle and retain its history. Manual Suspend of an
already capacity-suspended task must durably disable its automatic wake rather
than simply rejecting the request. These behaviors remain required and unproven.

The first bootstrap atomic-outcome test invocation failed to compile an
affected ACP test fixture (`engine_budget_test.rs` returns SessionId where the
updated bridge requires OpenedSession). This is migration debt, not behavioral
RED. A targeted owning unit regression can establish the atomicity defect only
after it executes; the affected integration fixture and full suites must still
be repaired and checked before phase approval. The subsequent `--lib` invocation
executes the owning regression and exposes the actual defect:
`/tmp/surge-3b-bootstrap-atomic-red.log`, 0 passed / 1 failed, 0.13 seconds.
When SQLite rejects the required OutcomeReported, BootstrapApprovalDecided and
BootstrapEditRequested remain committed while the accepted HumanInputResolved
also remains durable. The required change is to preserve the accepted answer but
commit decision effects and the required outcome together. Implementation and
accepted-effects crash / repeat-routing proof are still pending.

Quota evidence must validate deserialization as well as construction. Policy
backoff is capped, but a valid provider-reported reset farther than one day must
remain representable as observed evidence rather than being silently discarded.
The storage author confirmed both constraints for the implementation; executable
evidence is still pending.

Quota storage's owning RED was executed in session 94381: 0 passed / 1 failed,
0.08 seconds, after begin and candidate reservation reach the unimplemented
observation write. The author's extracted transcript is preserved in
`/tmp/surge-3b-quota-red.log`; it is an evidence summary rather than the original
complete Cargo output. The earlier missing test import in session 71089 was a
compile prerequisite, not behavioral RED. Restart / receipt replay / next
candidate assertions after this failure have not yet executed successfully.
This storage regression does not establish actual fallback dispatch or task wake.

Draft source inspection identified a quota ordering case for the author: a newer
observed Available result clears exhaustion, so comparing subsequent timestamps
only against exhaustion would allow an older Exhausted reply to overwrite it.
Unknown replies must also preserve the last actual observation ordering. Owning
out-of-order cases and authoritative control rebinding remain required before
the foundation is accepted. The next-cycle candidate history / eligibility
contract is being reconciled with the main integration owner.

The desktop draft now retains cache contents on daemon disconnect, marks them
stale with an error and increments its request generation so an older reply
cannot restore freshness. Root inspected both the transition wiring and model
regression source; execution of that regression and native real-daemon proof
remain pending. These changes do not close the wider task editing / start /
bootstrap and active-preservation requirements.

The initial quota owning test passed in the first invocation recorded at
`/tmp/surge-3b-quota-green.log`, 1 passed / 400 filtered, 0.07 seconds.
Root inspected the original Cargo output and source assertions: after an Unknown
observation write and reopening storage, an identical candidate gets the original
receipt with Replayed disposition, and another candidate can be Reserved. The
test currently does not read the observation back, so it does not yet prove that
observation's durable contents. Additional assertions and wider gates remain.

The integration owner clarified cycle semantics: same-cycle Suspend / Continue
rebinding preserves attempted candidates and known-account exclusions. A new
due-wake cycle retains prior history for audit but permits one new eligible retry
per candidate, so exhaustion can recover. Permanent exclusion across all cycles
would contradict automatic recovery. Likewise the draft lifetime limit of 16
probes is being removed: retries retain a checked audit count and positive bounded
policy delay, without making a task permanently dormant after an arbitrary
number of checks. This is a corrected implementation contract, not yet full
fallback or wake acceptance.

Root inspected another bootstrap handoff boundary:
`bootstrap_continuation::apply_edits_from_events` currently selects the latest
HumanInputResolved for node `flow_gate` without binding that response to its
accepted request occurrence / decision outcome. Graph validation and the final
digest do not themselves establish which response authorized those edits. The
main implementation owner must check orphan / conflicting / rejected later
responses and bind child edits to the approved occurrence before closing
bootstrap-to-task acceptance. No exploit or passing regression is claimed yet.

The quota author subsequently overwrote that green log path with an expanded
test invocation that fails compilation on the wrong MockClock import. The
earlier output was inspected, but the current file no longer proves that result.
Future invocations must retain separate immutable logs. Expanded test execution
and the final current-source gates remain unproven.

Root confirmed the task-error protocol gap: the daemon emits all WorkItem errors
as EngineError plus text, and the facade loses typed definitive rejection versus
transport uncertainty / pending control. The UI must retain the original
operation on ambiguous errors; it cannot safely issue a new operation by parsing
message strings. Typed rejection / pending outcomes and their owning retry tests
are required for a reliable conflict-resolution interaction.

The lifetime probe ceiling is now absent from migration 0024; the retained count
is nonnegative. This source change alone does not prove ongoing bounded wakes.
The user-requested interface illustration is an exploratory concept, with
illustrative PR numbers and capacity values; it is not approval to substitute
those values or that layout for the implementation requirements.

The user explicitly said the illustrated interface appears to miss part of their
intent and is not the present priority. Finish the Superplane improvement plan
and accepted functional requirements; defer discretionary redesign rather than
treating the illustration as approved.

The expanded quota foundation tests now pass in the separately retained raw log
`/tmp/surge-3b-quota-green-extended-81202.log`: 8 passed / 400 filtered,
0.24 seconds. They cover validated deserialization, nonactionable placeholder
migration, known-account deduplication, stale ownership / revision / manual
control rejection, out-of-order observation handling, observed-reset evidence,
receipt replay and policy-wake rollover with retained history. The reopen test
now reads back Unknown observation explicitly. These are foundation tests, not
actual provider rotation / capacity-control rebinding / automatic daemon wake,
nor full affected suites, strict gates or independent post-build acceptance.

The next foundation log is
`/tmp/surge-3b-quota-green-final-10b.log`: 10 passed / 400 filtered, 0.37 seconds.
It adds concurrent wake consumption producing one next cycle and confirmed
capacity-control rebind preserving receipts while rejecting manual fences. Root
inspected the raw output. Full persistence suites / strict gates, 5a then 5b,
actual fallback and automatic daemon wake remain required; this is not full 3B
acceptance. Invocation identifier canonicalization is being checked before the
foundation freezes.

The revised task-outcome protocol plan has independent SPEC and API ACCEPTABLE
verdicts before backend edits. Classification comes from the authoritative
operation phase: pre-admission refusal is rejected; a durable Start / Control
receipt remains accepted with current Pending / Attention diagnostics after
restore failure or deadline; unavailable receipt lookup remains uncertain.
Immutable receipt identity and the evolving current-state projection must stay
distinct. The UI keeps the original operation and drafts on uncertainty and may
offer a fresh explicit mutation only after definitive rejection. Implementation
and replay / lost-ack / pending-control tests are still required.

The bootstrap required-outcome rollback fix now has a green owning unit slice:
`/tmp/surge-3b-bootstrap-atomic-green.log`, 13 passed / 394 filtered,
0.68 seconds. Root inspected raw output and the one AppendBatch production call
for bootstrap audit effects plus OutcomeReported. Rejected outcome writes retain
the independently accepted human answer and roll back decision/edit effects.
Existing approve, edit, reject, cap and cancellation cases also pass. This does
not establish a crash after committed gate effects before routing or approved
flow-edit occurrence binding; those boundaries and full phase gates remain.

The first UI green invocation did not execute its test: moved screen modules
missed StyledExt and had a dangling documentation comment. That compile-only
failure is retained at `/tmp/surge-3c-zero-run-green.log`, not counted as GREEN.
The author corrected those errors and reserved a new log for the next invocation.
Root confirmed 237 MiB free, with 20 GiB of Cargo dependency artifacts; the main
builder coordinates standard package cleanup and serialized builds. This is a
recoverable build-resource limitation, not evidence of completed UI behavior.
The coordinator subsequently completed standard Cargo cleanup for orchestrator,
daemon, CLI and persistence (4202 files / 10.4 GiB); root confirmed 10 GiB free.
The UI's bounded retry slot was explicitly renewed. Source and worktree contents
were preserved; current-source UI acceptance still waits for executed results.

Quota invocation canonicalization and a read-only historical reservation query
were added after the 10-test gate. Current-source foundation acceptance therefore
still needs the new 11-test slice, full persistence suites and strict gates; the
earlier ten passing tests do not validate the later delta.

The second UI invocation also failed compilation on Div versus Stateful<Div>;
it is preserved at `/tmp/surge-3c-zero-run-green2.log`. After that mechanical
correction, the original outer row regression passes at
`/tmp/surge-3c-zero-run-green3.log`: 1 passed / 202 filtered, 0.71 seconds.
Root inspected the raw output. It proves that a durable task with no runs,
received through a real DaemonEngineFacade socket fixture, appears in the GPUI
Fleet frame. The filtered integration binary ran zero tests and contributes no
additional evidence. Full task model / mutation gates, native real-daemon
interaction, active preservation and bootstrap handoff remain required.

The UI model slice passes at `/tmp/surge-3c-model-green.log`:
7 passed / 196 filtered, 0.01 seconds. Root inspected raw output and test source.
It covers project/request freshness, disconnect/cache retention, traversal past
a globally paginated nonmatching page through the facade, task-bound validated
Start command and immutable retry value. The retry assertion only compares a
stored command clone; it does not drive accepted-mutation / lost-response /
Retry through Fleet. That owning transport/mutation test, typed conflict draft
handling and real-daemon native interaction remain required. The filtered
integration binary again executed zero tests. Persisted human discussion alone
does not establish automatic orchestrator replies or bootstrap handoff.

The full persistence gate is not yet green:
`/tmp/surge-3b-quota-persistence-all-20260930.log`, 411 passed / 1 failed,
5.30 seconds. All twelve quota tests pass, but the verification-upgrade fixture
chooses its legacy schema by `REGISTRY_MIGRATIONS.len() - 2`. Adding registry 0024
moves that slice past the proof-downgrade migration before the fixture inserts
its legacy verified row. Root inspected the exact source and authorized a
fixture-only correction to select the explicit migration boundary, retaining
the original downgrade and reapplication assertions. The rerun and strict gates
remain required; this is not permission to weaken production migration behavior.

The corrected full persistence invocation now passes at
`/tmp/surge-3b-quota-persistence-all-fixed-20260930.log`: 446 passed,
0 failed, 1 ignored across all test binaries. Root inspected every result line
and the explicit `registry-0021-verification-binding` fixture boundary plus the
assertion that it has not run before legacy data is inserted. The strict
all-targets / all-features Clippy invocation currently stops on six new core
RunMemory nesting / collapsible-if findings, so persistence itself has not yet
been fully linted. The core owner is repairing these without suppressions; final
strict lint and independent foundation 5a / 5b remain required.

The next unsuppressed strict invocation reaches persistence after the core
nesting fixes, but still fails on seven findings in `runs/inspection.rs`:
items-after-statements, redundant closures and test conversion/default clarity.
Root inspected `/tmp/surge-3b-quota-persistence-clippy-fixed-20260930.log`.
The main recovery owner is fixing that zone. No findings were reported in the
quota module, but the failing overall command is not a strict pass. Standard
formatting also requires the main control module to be formatted; module-only
format checks cannot substitute for that affected gate.

After those fixes and formatting, the next strict invocation still fails on
three skill-trust purpose nesting findings in core (`run_state.rs` 1122 / 1131 /
1138), recorded at
`/tmp/surge-3b-quota-persistence-clippy-final-20260930.log`. Core / inspection /
quota source is being held stable during the next gate after the helper repair;
daemon fixture authoring may continue separately. Foundation review has not
started and no strict success is claimed. These are pre-freeze verification
corrections, not a reset of the full 3B independent-review repair budget.

The dependency-frozen unsuppressed strict invocation now passes:
`/tmp/surge-3b-quota-persistence-clippy-frozen-20260930.log`, finished in
10.26 seconds. Root inspected original output. The author is rerunning the full
tests on the same frozen dependencies after helper extraction; foundation 5a /
5b and actual engine fallback / task wake remain outstanding. During source
inspection root also asked the author to check public expected-cycle invocation
alias handling, since canonicalized reads and raw update predicates must not
silently disagree; no final review verdict is asserted yet.

The author confirmed that alias concern as a real pre-freeze defect: a public
expected-cycle snapshot using the bare invocation alias can be read canonically,
but finish updates zero rows with the raw alias and still returns success. Public
read / begin may accept valid aliases; mutation snapshots must use the canonical
identity or normalize all predicates consistently. Root requested an owning
behavioral RED before the correction, including stored state assertions, then
new full / strict gates. The earlier strict pass does not validate this next fix.

Alias regression calibration reproduces the exact defect:
`/tmp/surge-3b-quota-alias-snapshot-red-20260930.log`, 0 passed / 1 failed,
0.05 seconds. Only the two new expected-snapshot canonical guards were removed;
the enhanced test observed successful finish with the canonical row still open.
This is reconstructed pre-fix evidence after the initial fix, not original
test-first chronology. Both guards were restored, and the new full invocation
`/tmp/surge-3b-quota-persistence-final-after-alias-red-20260930.log` passes
446 tests, 0 failed, 1 ignored. Root inspected the failing assertion and every
passing result line. Final strict / formatting gates and independent review
still must validate this source before integration acceptance.

The current frozen foundation now has all verification gates:
446 all-targets / all-features tests passed (1 ignored), 4 doc-tests passed
(1 ignored), unsuppressed strict Clippy passed in 3.62 seconds at
`/tmp/surge-3b-quota-persistence-strict-final-after-alias-20260930.log`, and
recursive affected formatting / diff checks passed. Root inspected the raw
strict and doc-test output. The Cargo lane was released to the main builder.
An independent read-only Phase 5a reviewer is now checking the frozen source,
schema and observable acceptance evidence. Phase 5b has not started; neither
foundation approval nor full 3B completion is claimed.

Independent Phase 5a returned SPEC ACCEPTABLE for the frozen quota storage
foundation, with no blocking specification mismatch and no review repair round
consumed. The reviewer checked source, migration, owning row-state / reopen /
concurrency evidence and retained gates. Advisory test improvements are explicit
malformed RecoveryWake JSON and unchanged-row / revision assertions on dedup
refusals. The registry-fence rebind fixture is synthetic and cannot stand in for
real suspension journal / snapshot confirmation or daemon wake execution.
Two independent Phase 5b quality lenses are now read-only over the same frozen
source. Full provider dispatch / task wake / cleanup preservation / containment
and overall 3B acceptance remain outstanding.

Both independent Phase 5b lenses now return QUALITY ACCEPTABLE for the frozen
storage foundation: API/security and correctness/concurrency. No blocking
finding or review repair dispatch was required. Historical Replayed receipts
do not grant current dispatch authority; an armed retry likewise does not assert
current provider availability. Advisory malformed-wake JSON coverage and replay
documentation remain recorded. This technical foundation acceptance does not
erase the disclosed reconstructed alias-test chronology or approve full 3B.

The same storage author is now preparing a read-only full execution / automatic
task-wake integration plan with exact write ownership and owning ACP / daemon
regressions. It must pass independent technical and spec lenses before source
writes. Main implementation continues the typed task-operation phase boundary;
the UI author resumes functional 3C and real-daemon lost-ack proof. Runtime
containment and the full issue-to-merge path remain mandatory.

### Full quota execution integration draft (precode review pending)

#### Concrete quota handoff transition refinement (awaiting both lenses)

The existing general recovery-cycle mutation fence remains Executing-only.
Automatic recovery uses a separate store-issued capability, never a caller flag
or a deserialized authority. Provider session history is a Vec of OpenedSession
records per invocation; preserve every record and select an exact candidate
descriptor rather than the last session of the original provider.

Proposed registry migration 0025 adds two normalized records:

- Capacity-control association: monotonic sequence, run/task/attempt generation,
  logical stage invocation, cycle generation/revision, old/new control generation,
  selected reservation and wake identity. Creation is atomic with the internal
  Capacity Suspend request, before the owner stops. This makes the generation
  transition discoverable even before journal confirmation or cycle rebind.
- Quota handoff: operation/receipt identity, origin (same-cycle rotation or due
  automatic wake), immutable source cycle/control/wake, next cycle/control,
  selected provider invocation/runtime/model/account evidence/launch hash, opening
  mode and optional exact saved provider descriptor, state, and journal evidence
  sequence. Unique source-wake and handoff/open-epoch identities prevent competing
  ticks from allocating separate continuations. Prior candidate/session rows are
  retained. State is Reserved, OpeningUnknown, Established, Executing, Invalidated
  or Attention; none of these alone asserts quota availability.

The operation row for automatic Continue is an internal tagged body/hash, not a
public WorkItemCommand that a client can construct. The existing control foreign
key and idempotent operation result remain intact. Frozen graph/configuration,
accepted requirements and workspace identity are immutable inputs to the receipt.

Provider invocation and dispatch authority are distinct identities. New fallback
creates a new provider invocation; Resume/Load must retain the exact invocation
in its saved descriptor because the worker validates descriptor/config equality.
A new handoff operation ID is the opening epoch in both cases. Proposed unique
keys are (run, source logical invocation, source cycle, source control, wake ID)
for automatic wake, (run, logical invocation, target cycle, candidate key) for
one candidate reservation, and handoff operation ID for one opening admission.
Do not impose a global unique provider-invocation constraint on handoffs: that
would reject legitimate later restoration of the same saved session.

The handoff binds saved provider ID/invocation/runtime/launch contract/open mode;
Established additionally binds the newly returned internal SessionId and durable
SessionOpened sequence. Prompt/tools/results require the exact current handoff
ID, control generation, selected provider invocation, internal SessionId and
fresh authenticated MCP endpoint generation. A delayed prior opening/history
message cannot acquire authority merely because the provider invocation is the
same. Core journal opening facts must carry this handoff/epoch association so
cold confirmation never chooses an older SessionOpened from the retained Vec.
OpeningUnknown remains spent even for Resume/Load; no second ambiguous restore
or fresh New is inferred. Add crash/replay tests for two legitimate restores of
one saved provider invocation with distinct handoffs/internal sessions, old-epoch
late ingress rejection, and uncertain Resume without a second Resume or New.
This exact identity refinement awaits technical and spec delta acceptance before
handoff schema implementation.

| API / transition | Exact authority and durable result |
|---|---|
| request_capacity_suspend(claim, expected cycle, reason/wake) | One IMMEDIATE transaction checks live owner and current Executing generation/revision, inserts the internal Suspend operation/control and capacity association. Cycle attempts remain unchanged. |
| reconcile_capacity_control(claim, association) | Inspect the authoritative journal/snapshot and confirmed writer cleanup, then transactionally confirm the exact Capacity fence and rebind its associated cycle. Already confirmed/rebound returns the original result. Manual reason, terminal/stale assignment or conflicting invocation rejects. |
| reserve_automatic_wake(claim, expected cycle, wake ID, now, selected candidate/launch contract) | One IMMEDIATE transaction checks exact current confirmed Capacity Suspended control, claim/task/run/attempt, cycle/revision, due immutable wake and eligibility evidence/policy. Consume the wake, close prior cycle, create next cycle, reserve candidate, insert internal Continue operation and next ContinueReserved control, and insert Reserved handoff together. Exact replay returns inspection only; conflicting body rejects. |
| reserve_quota_rotation(claim, expected cycle, candidate/launch contract) | Under current Executing and confirmed prior-writer cleanup, reserve the next candidate and its distinct provider invocation with a Reserved handoff. Same-cycle attempts/account exclusions are preserved. |
| admit_provider_open(claim, receipt) | Re-read exact receipt/assignment/control; allow only matching automatic ContinueReserved or ordinary Executing origin. CAS Reserved to OpeningUnknown before external opening; return one private non-Clone/non-Deserialize OpenPermit. Replay cannot return a second permit. |
| confirm_provider_open(claim, receipt, opened journal sequence) | Inspect durable SessionOpened and exact selected invocation/runtime/launch hash/open mode/saved descriptor. CAS OpeningUnknown to Established under unchanged ownership/control. Former-provider responses cannot establish the selected candidate. |
| authorize_prompt(claim, receipt, continued journal sequence) | Automatic origin requires matching durable RunContinued and synchronous registry Executing acknowledgement; update control/cycle binding and handoff Executing together before prompt. Ordinary rotation checks current Executing. Authenticated marker/tool/result ingress checks the selected receipt/invocation and control generation throughout execution. |

Manual or terminal control winning before admission prevents opening. Winning
after admission invalidates the handoff, cancels the admitted writer and waits
for confirmed cleanup; it cannot erase an already admitted external operation.
No prompt/results may acquire current authority afterward. Unknown cleanup stays
Attention, not confirmed suspension. Manual Suspend of a capacity-paused task
also creates a new control generation and invalidates queued automatic receipts.

OpeningUnknown deliberately includes crash-before-send and ambiguous RPC return:
absence of SessionOpened does not prove the operation was unsent. Cold recovery
must not repeat New, Resume or Load from that state. A matching durable opened
record can reconcile to Established; otherwise preserve uncertainty/Attention and
require an explicit recovery choice. A proven pre-session failure with confirmed
writer cleanup may be recorded definitively, but does not turn quota Unknown
into Available or restore the spent reservation.

Cold task reconciliation pages active capacity-control associations and handoffs
by monotonic cursor with a captured high-water mark and a fixed page bound,
independently of due_recovery_wakes. For each entry acquire its execution claim,
inspect the trusted journal/phase/cleanup and complete the idempotent transition.
Do not skip a generation-before-rebind record merely because the due query cannot
see its old cycle. Only afterward discover due wakes. Manual/terminal/current
assignment checks occur again inside every mutating transaction.

Crash-cut oracles: (1) after Capacity control reservation/before journal fence,
(2) after journal fence/before registry acknowledgement/rebind, (3) after atomic
wake/Continue/candidate reservation, (4) after OpeningUnknown/before RPC,
(5) RPC response lost, (6) after SessionOpened/before Established,
(7) after RunContinued/before registry Executing, and (8) after Executing/before
prompt. Assert one stored continuation/candidate, no second uncertain opening,
no prompt before current Executing, preserved session/workspace history, and
manual/terminal victory without current late results. Actual A-to-B ACP traffic
and all-exhausted task-owned timer recovery remain mandatory outer acceptance.

Author zones remain new quota coordinator/provider-observation modules plus
approved persistence extension/migration. Main owns core/config/replay, engine
stage/run-task glue and daemon controls/reconciliation/scheduler. Neither author
edits UI. Both independent lenses must approve this concrete refinement before
source implementation. Proven containment remains a separate unresolved
dependency; GroupOnly/Unknown is never promoted to complete writer cleanup.

The integration plan connects actual alternate ACP dispatch and task-owned wakes
to the accepted storage foundation. Candidate resolution uses configured
canonical runtimes and explicit order or deterministic automatic order. Record
the original exhausted candidate and reserve every alternate before side effects;
Replayed never grants another opening. Persist the selected runtime/model and
its launch contract, retain prior sessions and workspace, and do not modify the
frozen graph/config. Logical-stage cycles and provider dispatch invocations need
an explicit durable association so late former-provider results cannot acquire
current authority.

Persist typed RateLimited evidence before cleanup. Unknown cleanup retains the
quota cause, blocks rotation and enters Attention. After confirmed cleanup, try
each eligible candidate once per cycle. With none available, persist Capacity
pause plus observed reset evidence or explicit Unknown/Unsupported policy retry;
readiness and binary presence do not assert quota availability. No lifetime
retry or monetary cap is added.

The plan must resolve two concrete ordering gaps: Capacity Suspend advances
control generation before cycle rebind, requiring bounded cold reconciliation
even when old cycles are absent from due queries; and current RunContinued occurs
after provider opening while candidate writes require Executing. A narrowly
authenticated automatic-wake handoff must atomically reserve exact claim/control/
cycle/revision/wake and candidate before opening, then confirm journal and registry
Executing before prompt dispatch. Uncertain opening cannot issue another New.
Manual Suspend, including an already capacity-paused task, invalidates automatic
wake and rebind without losing explicit Continue.

Write ownership draft: quota author owns a new coordinator / provider-observation
module and agreed capacity routing APIs; main owner retains shared run_task,
stage agent, core replay, daemon controls and typed outcomes; UI remains separate.
Shared seam edits require exact coordination, not competing writers. Independent
technical and spec precode verdicts are required before implementation.

Outer RED must run an actual task-owned daemon / ACP A-quota→B-success flow,
preserving task/run/workspace. Additional owning cases cover all exhausted→pause→
timer→retry, unknown probes, reset expiry/order, crashes around reservation/open/
suspend/rebind/handoff, concurrent/manual/terminal races, saved Resume/Load,
unsupported or uncertain restoration, dirty/untracked work and cleanup failure.
Affected multi-crate full suites, strict gates and independent reviews remain.
GroupOnly/Unknown cannot prove CoveredDomain; complete automatic cold recovery
also requires proven writer containment. No managed runtime installation is
authorized by this draft.

The initial actual socket task-outcome regressions both fail:
`/tmp/surge-3b-typed-admission-red.log` (0 / 1, 0.13 seconds) returns generic
engine_error for a stale-version pre-admission conflict;
`/tmp/surge-3b-typed-pending-red.log` (0 / 1, 0.10 seconds) returns generic
engine_error / run-not-found for an already requested Suspend with no local actor.
Root inspected both raw outputs. In those first fixtures the receipt/control
assertions came after the response failure and did not execute, so their stored
state is not established by those logs. The owner moved those assertions before
the failure and will recapture the unchanged daemon behavior before the fix.
No typed-outcome GREEN is claimed.

The quota execution draft has TECH ACCEPTABLE with exact store-issued handoff
invariants, but SPEC returned RESHAPE NEEDED before implementation: the draft
still leaves the ordering transitions implicit. The author must specify one
transaction for old control/cycle/wake CAS, next control generation, candidate
reservation and private receipt; allowed pre-open state; persisted uncertain
opening; synchronous trusted journal / registry confirmation before prompt; and
cold active-control reconciliation independent of due visibility. Manual or
terminal CAS winning must revoke dispatch eligibility. Both lenses must approve
the revised executable protocol before integration source writes.

Root also flagged the host replay ordering requirement: an accepted historical
Continue receipt must not dispatch or alter current task state after a newer
manual control wins. Read the current generation before actor action, preserve
the original immutable receipt, and show the current projection separately.
The main owner is checking this in the typed-outcome integration; no exploit or
passing regression is asserted solely from source ordering.

The strengthened original-behavior socket regressions now execute stored-state
assertions before failing on the response:
`/tmp/surge-3b-typed-admission-strong-red.log` (0 / 1, 0.14 seconds) confirms no
operation receipt / item version change before the missing typed-conflict error;
`/tmp/surge-3b-typed-pending-strong-red.log` (0 / 1, 0.10 seconds) confirms the
original SuspendRequested operation with no cleanup fence before generic
run-not-found is returned. Root inspected source assertion order and raw outputs.
These supersede the weaker initial logs for admission-state evidence. Typed
outcome implementation and GREEN remain pending.

Subsequent owning socket tests passed: typed pre-admission conflict preserves no
receipt / unchanged item version (`/tmp/surge-3b-typed-admission-green.log`,
1 / 1, 0.13 seconds); admitted Suspend returns the original pending operation
(`/tmp/surge-3b-typed-pending-green.log`, 1 / 1, 0.18 seconds). The diagnostic
extension also passed (`/tmp/surge-3b-typed-pending-diagnostic-green.log`,
1 / 1, 0.13 seconds), retaining the matching control's failure without inventing
a cleanup fence. These are focused proofs, not the full 3B gate. Historical
control replay and broader affected checks remain required.

The concrete quota handoff refinement now has independent SPEC ACCEPTABLE and
TECH ACCEPTABLE verdicts. Implementation of the agreed disjoint zones is
authorized, with eligibility / policy checked inside the owning transaction and
immutable launch contract / open mode. Actual alternate-provider ACP dispatch,
all-exhausted wakeup and the documented crash-cut matrix remain unproved.

The real-daemon UI lost-ack fixture attempts currently fail on test runtime /
scheduler compatibility rather than the semantic oracle. Distinct attempt logs
are retained; no passing native interaction or no-duplicate effect is claimed.

The legitimate Agent-stage revisit regression passes after correcting its oracle
to count only the actual `implement` node rather than all stage entries including
the terminal successor (`/tmp/surge-3b-skill-revisit-corrected.log`, 1 / 1,
0.72 seconds). The original 3-versus-2 failure was a fixture counting error,
not evidence of a production defect. The test checks distinct approval request
identities and that completed SkillTrust approvals leave no outstanding HumanGate
recovery record. This mock-bridge proof does not establish cold provider recovery.

The corrected real-daemon UI lost-response fixture now passes
(`/tmp/surge-3c-real-daemon-lost-ack-attempt5.log`, 1 / 1, 0.81 seconds).
Root inspected the raw result and oracle: the proxy drops an actual accepted
`WorkItemOk`, SQLite already contains the original operation receipt and one
discussion before retry, the GPUI controller repeats the full identical command,
and SQLite still contains exactly one message with the original whitespace.
Attempts 1–4 remain runtime / scheduler setup failures, not behavioral REDs.
This proves this owning discussion mutation through the controller and real
daemon; it does not prove a manual native walkthrough, active-task edits,
bootstrap association, or the full 3C gate.

Current-source rerun of the nonterminal control / operator-gate crash fixture
failed waiting for its initial close-barrier file
(`/tmp/surge-3b-decision-current-regression.log`, 0 / 1, 8.15 seconds,
`committed_continue_fixture` before the Suspend request). The test's child host
was still alive after the parent panic; the owner is inspecting fixture-owned
processes and the missing barrier before classifying this as a semantic recovery
regression. Earlier passing crash logs do not establish this current source
combination. No timeout extension or recovery pass is claimed.

The diagnostic rerun (`/tmp/surge-3b-decision-barrier-diagnostic.log`, 0 / 1,
8.16 seconds) identifies the missing barrier's prerequisite failure: the actual
journal records `StageFailed` / `RunFailed` from ACP `new_session` reporting
`MCP helper spawn failed`, before any SessionOpened. Thus this invocation never
reached the recovery cut and is fixture setup evidence, not a behavioral RED for
gate recovery. The owner is verifying the required helper executable and build
prerequisites, then must rerun the same recovery oracle.

The required helper rebuild exposed a real compile integration gap
(`/tmp/surge-3b-stage-helper-build.log`): `surge-cli` doctor still passed the
new `OpenedSession` descriptor to three APIs requiring its `SessionId`.
The main owner is repairing this ACP API ripple and rebuilding the helper;
the cold recovery test remains unproved on current source until rerun.

Source inspection also found that a disconnected task view could render a
previously cached run stream as currently working. The UI owner has authored
a GPUI controller regression that first establishes a running stream, then
disconnects and requires an unconfirmed state while preserving task data.
No behavioral RED / GREEN for this new stale-stream case is claimed yet.

The ACP doctor ripple is now repaired: the configuration pins the actual registry
entry id, and send / close operate on `OpenedSession.session`. Root inspected the
diff and the required helper rebuild passed
(`/tmp/surge-3b-stage-helper-build2.log`, dev build 27.73 seconds).
This restores the missing fixture executable prerequisite; the crash oracle and
broader CLI checks remain separate obligations.

With the helper rebuilt, the same nonterminal-control crash oracle passes on
current source (`/tmp/surge-3b-decision-current-with-helper.log`, 1 / 1,
3.87 seconds). Root inspected the raw result. The preceding timeout invocations
remain setup failures before the crash cut. This focused current-source result
does not prove the still-missing committed HumanGate effects-before-route cut.

The saved-provider identity refinement has separate SPEC ACCEPTABLE and TECH
ACCEPTABLE verdicts: Resume/Load retain the saved provider invocation while a
fresh spent handoff epoch, internal session and authenticated MCP generation
establish current authority. Schema implementation is authorized in the agreed
zones. Actual two-restores, late-old-ingress and uncertain-Resume/no-New tests
remain required; no complete quota integration is asserted.

The historical-launch projection now has an actual behavioral RED
(`/tmp/surge-3c-historical-launch-red3.log`, 0 / 1, 0.01 seconds): a retained
Launched attempt without current control projects Running instead of Status
unavailable. Earlier red / red2 logs are unrelated test-harness compile failures.
The separate GPUI cached-stream disconnect case is still awaiting its owning
result; the projection RED alone does not prove the renderer defect or its fix.

The separate owning GPUI cache/controller regression also reaches behavioral RED
(`/tmp/surge-3c-cached-working-red.log`, 0 / 1, 0.68 seconds). Its recorded-event
fixture establishes a Running stream, retains that stream and task identity after
cache disconnect, then the actual Fleet status still returns Running instead of
Run state is unconfirmed. Root inspected the prerequisite assertions and raw
failure. This is a controller/cache proof using synthetic recorded events, not
an actual-daemon disconnect walkthrough. The UI owner is now applying the fix;
no GREEN for either new stale-state oracle is claimed yet.


### Gate stage commit protocol draft (precode lenses pending)

This is an owning completion of 3B decision recovery, not provider authority or a
new workflow. No gate commit implementation is authorized until both lenses
accept this contract.

`GateStageCommit` is a constructor-validated and validating-deserialized core
record with: `node`, `GateRequestId`, `stage_entry_seq`, `requested_seq`,
`resolved_seq`, accepted `response_hash`, `OutcomeKey`, `GateCommitDisposition`,
`effects_count` (1..=4096), and `effects_hash`. The disposition is `Route`,
`BootstrapRejected`, or `BootstrapEscalated`; the last two replay the existing
typed stage error, rather than appending bootstrap audit effects again. There is
no provider SessionId, provider connection, or conversion from gate ID to a
provider invocation. Event envelope sequence is the actual commit sequence,
never a predicted writer prefix. A legitimate loop occurrence has a new request
and stage-entry sequence.

The response hash is SHA-256 of serde-json serialized accepted response under a
versioned `surge-gate-response-v1` domain. Effects hash uses the same ordered
VersionedEventPayload serialization as provider commits but a distinct
`surge-gate-effects-v1` domain; it includes the schema wrapper and count. Host
producer prepares the exact batch already used by human_gate: bootstrap
approval/edit/escalation effects as applicable, then exactly one OutcomeReported,
then GateStageOutcomeCommitted. One append transaction commits every effect and
the marker, or none. The independently durable HumanInputResolved precedes this
batch and remains available after rejection of the effect batch.

The owning reader folds the original accepted graph, stage entry, exact request
and original schema/options/config before accepting resolution. It requires
requested < resolved < first effect <= commit, original node and occurrence,
HumanGate purpose (never SkillTrust), exact accepted response hash/outcome and
bootstrap disposition consistent with the original mode and response. The
ordered preceding effects must be contiguous, bounded, all belong to this gate,
and equal the semantic effect shape for its original bootstrap mode (including
original edit count and feedback); one matching OutcomeReported is last. No
orphan response, conflicting answer, repeated commit for one request, stale
occurrence, unexpected effect, or digest/sequence gap is normalized as trusted.
A rolling window spans inspection pages. Valid and forged identical-history
twins exercise the actual guard. Legacy answers remain readable; an old unbound
outcome/effect already present cannot be replayed as fresh accepted gate effects.

Per-run migration 0006 adds a gate_stage_commits projection keyed by exact request
and stage occurrence, with immutable commit sequence/body and nullable consumed
route identity. It is derived only from validated journal events; rebuild resets
and replays this projection while preserving host-authored graph snapshots.
GateStageRouteCommitted binds node/request/occurrence, exact gate commit sequence
and outcome. The existing route helper selects an explicit Provider or Gate
commit origin. Gate routing prepares cloned graph/cursor/counters/memory, folds
its projected events, and atomically commits edge + StageCompleted + owning gate
route marker + exact post-route snapshot under writer-prefix CAS and unconsumed
commit CAS. Rejected transaction publishes no memory/counter changes. Orphan,
duplicate, mismatched or superseded route rolls back the whole batch with typed
OperationRejected; the reader also rejects corrupt raw historical rows.

Cold replay exposes the validated outstanding gate commit before enter_stage,
question dispatch or response effects. Route disposition consumes it exactly
once without bridge traffic or another question. Rejection/escalation replays
the existing typed stage error without new bootstrap audit effects. Already
consumed route requires its exact valid post-route snapshot, then continues from
that cursor; missing/corrupt checkpoint or contradictory joins yield retained
Attention/RecoveryRequired. RunMemory gate completion clears only the matching
occurrence; durable accepted-answer receipt remains replayable after completion.
Bootstrap continuation selects approval/edit inputs through this exact trusted
commit/request binding, never latest HumanInputResolved by node alone.

These new EventPayload variants require schema 12 -> 13 identity migration,
old/new golden and SchemaTooNew tests, plus explicit views/reports/trace/UI event
consumer updates. Producer, reader and projection tests cover rejection rollback,
page crossing, forged valid twins and legitimate loop revisit. Outer real-daemon
socket oracles cut after accepted answer + committed effects before route, and
after committed route before acknowledgment, then kill/restart owned fixture
host: same request/answer, exactly one bootstrap edit/audit/route, original reply
receipt, no provider reexecution. Existing accepted-answer-before-effects and
pending-decision positives remain. Confirmed writer cleanup is a separate
containment obligation; this protocol never upgrades GroupOnly to CoveredDomain.

Both concrete precode lenses now accept this gate protocol. SPEC checked existing
bootstrap rejection/escalation: the current producer already reports the actual
decision outcome as audit, then returns the typed error; non-Route dispositions
must preserve that behavior without traversing a success edge. TECH requires the
reader hash each original schema wrapper rather than rewrap historical events as
schema 13, with old provider-commit golden regressions. Implementation is
authorized; outer crash cuts and full affected checks remain unproved.

The stale task-status fixes pass both owning regressions
(`/tmp/surge-3c-stale-status-green.log`, 2 / 2, 0.75 seconds). Historical Launched
alone is unconfirmed, and the Fleet controller requires fresh task cache,
connected facade and live stream before presenting Working as current. Root
inspected the source guards and raw result. Actual daemon disconnect/reconnect,
typed conflict draft preservation and full 3C checks remain separate obligations.

The owning manual-control wire regression reaches behavioral RED
(`/tmp/surge-3b-manual-dominance-red.log`, 0 / 1, 0.41 seconds): after actual ACP
suspension and a durable undispatched Continue reservation, the real daemon
rejects Manual Suspend as already pending instead of creating the newer control.
The subsequent historical-replay assertions were not reached. Root also found
that the replay host fixture omitted wire-log flags, weakening its no-dispatch
oracle; the owner must retain provider instrumentation on that host before
claiming GREEN. New control authority cannot inherit an old cleanup fence as
proof of cleanup for the new generation.

The enhanced manual-control fixture passes
(`/tmp/surge-3b-manual-dominance-green.log`, 1 / 1, 0.45 seconds): the newer
manual Suspend is admitted, the historical Continue receipt remains original,
the entire latest control stays unchanged on replay, workspace content survives,
and instrumented provider wire is unchanged. Root inspected those assertions.
One remaining oracle limitation must be closed: the fixture retains a launch
claim across replay, which could suppress an erroneous dispatch attempt before
any wire traffic. The owner must independently observe launch admission or
remove this protective condition and demonstrate that removing the generation
guard fails the regression. The current pass proves admission supersession;
complete absence of historical dispatch is not yet established.

The first real-daemon typed-conflict / retained-draft fixture invocation is not
GREEN (`/tmp/surge-3c-typed-conflict-draft-attempt1.log`, 0 / 1, 1.95 seconds).
It ends in GPUI teardown reporting leaked textarea entity handles. Root inspected
the code's actual conflict, uncertain retry and draft assertions; a teardown
failure cannot be represented as a passing owning UI test. The owner must repair
entity / window lifecycle without disabling the leak checker, then rerun.

Attempt 2 reaches its explicit semantic-completion marker for typed refusal,
uncertain retry, identical command and retained drafts, but still fails teardown
(`/tmp/surge-3c-typed-conflict-draft-attempt2.log`, 0 / 1, 2.32 seconds): a Tokio
timer is created while its runtime is shutting down. Its forced draft clearing
is diagnostic only and must be removed for final owner-lifecycle proof. This
invocation is not GREEN; runtime / pending UI task teardown remains to repair.

The unlocked historical-control invocation did not reach its oracle
(`/tmp/surge-3b-manual-unlocked-green.log`): compilation reports two undefined
`after_effects` references in the newly extended gate crash fixture in the same
integration test binary. The owner must repair the fixture helper / entry points
and rerun with a new log. The prior lock-protected pass is not upgraded by this
compile attempt; guard sensitivity remains unproved.

The repaired unlocked invocation passes
(`/tmp/surge-3b-manual-unlocked-green2.log`, 1 / 1, 0.49 seconds). Root inspected
the raw result and current fixture: the retained launch claim is dropped before
historical replay, original receipt and complete newer control remain unchanged,
and the same provider instrumentation remains enabled. Sensitivity to removing
the generation guard is still pending; this pass does not replace the requested
counterexample calibration or broader control/crash checks.

Controlled guard calibration now demonstrates sensitivity
(`/tmp/surge-3b-manual-guard-calibration.log`, 0 / 1, 0.45 seconds): disabling
only the latest-generation guard causes historical Continue replay to alter the
newer manual control from SuspendRequested to Attention. Root inspected the raw
control comparison and disabled expression. This is a deliberate mutant, not a
new production RED. The owner must restore the exact production guard before
other gates and verify a positive rerun; broader concurrent-control and quota
handoff checks remain required.

Root has now verified the exact generation guard is restored in the daemon host.
The next actual-daemon crash-cut regression reaches behavioral RED
(`/tmp/surge-3b-gate-effects-cold-red.log`, 0 / 1, 0.45 seconds): the accepted
answer and gate outcome are durable before owner death, but restart produces
three total OutcomeReported events instead of the expected earlier agent outcome
plus one gate outcome. Original request / answer count checks reach one each.
This is the generic approval gate, not a bootstrap-edit proof. Root requested
post-kill/pre-restart count assertions to exclude a raced route commit from the
cut. The authorized gate-commit implementation must eliminate the repeated
outcome and satisfy the wider bootstrap / forged-history / route-ack oracles.

The validated gate identity / answer / effect-marker module is now authored at
`surge-core/src/execution_recovery/gate_commit.rs`, preserving distinct response
and effects hash domains and validating deserialization. Root inspected this
initial source. It is not yet registered or wired through producer, authoritative
reader, projection and route transaction, and no passing implementation test is
asserted for it. Public scalar constructors do not themselves establish graph,
request or response authority; those joins remain the reader's obligation.

UI fixture teardown diagnosis is now tied to primary GPUI source:
`TestAppContext::run_until_parked` schedules tasks but does not flush App effects;
`App::finish_update` flushes effects and releases dropped entities. The corrected
fixture performs an App update after dropping its owner and requires weak handles
to the original Fleet view and all four original inputs to stop upgrading. No
forced draft clearing or leak-check suppression is used. Root inspected the
primary source; the corrected invocation still needs its passing owning result.

The strengthened crash-cut invocation confirms the same behavioral RED
(`/tmp/surge-3b-gate-effects-exact-cut-red.log`, 0 / 1, 0.36 seconds).
After the owned host is killed and waited, before restart, SQL asserts exactly
two outcomes and only the earlier agent's StageCompleted. Thus the gate outcome
is committed and its route is not. Restart still produces a third outcome.
Root inspected the post-kill assertion order and raw failure; this supersedes
the earlier pre-kill-only cut evidence for the generic-gate duplication defect.

The first actual ACP quota-route invocation is not yet a fallback behavioral RED
(`/tmp/surge-3b-quota-wire-red-initial-20261001.log`, 0 / 1, 5.17 seconds).
It fails the prerequisite that A emitted one typed RateLimited response: the
observed count is zero, so the missing-B oracle has not executed. The owner is
capturing the pre-cleanup journal and both provider wire logs to identify why
the controlled A quota response was not observed. No rotation implementation
or availability claim is inferred from this failure.

The diagnostic quota probe (`/tmp/surge-3b-quota-wire-red-diagnostic-20261001.log`,
0 / 1, 5.16 seconds) has no run database / registry row or provider wire. Source
inspection identifies the fixture prerequisite: it manually provisions and dirties
the worktree before the host records workspace preparation, so the real Start
still selects BeforeExecution. The corrected fixture must let the host prepare
and open A, pause at its actual first send, write tracked/untracked work into the
actual cwd, then release the underlying ACP quota RPC. No fabricated preparation
flag, deleted user files or synthetic RateLimited response is permitted.

The precise UI owner-lifetime attempt is still not GREEN
(`/tmp/surge-3c-typed-conflict-draft-attempt4.log`, 0 / 1, 1.65 seconds).
Its semantic marker is reached and the original Fleet weak owner no longer
upgrades after App-update GC, but weak handles to the original draft inputs
still upgrade. Thus App-update GC alone is insufficient; the remaining input
retention must be reproduced and traced independently. The earlier source-based
GC diagnosis was a necessary cleanup step, not a complete leak explanation.
The leak checker and original retained-draft assertions remain enabled.

The real-daemon current-revision projection test reaches behavioral RED
(`/tmp/surge-3c-superseded-current-revision-red.log`, 0 / 1, 0.89 seconds).
Create → terminal Start → actual assignment release → stored Edit leaves one
Completed historical attempt whose AcceptedRevisionRelation is Superseded and
accepted revision is 2. Those independent storage assertions pass before the
Fleet controller incorrectly shows Completed rather than Needs execution for
current revision. The UI owner is correcting the current-task projection while
retaining the historical Completed attempt; no GREEN is asserted yet.

### Current-revision and visible-task UI corrections verified

The strengthened zero-run facade/window probe independently expects `All  1`
for its one visible durable task. It reaches behavioral RED in
`/tmp/surge-3c-zero-run-count-red.log` (0 / 1, 0.70 seconds): the row exists but
the badge says `All  0`. The shared durable-task filter/count correction passes
the same owning probe in `/tmp/surge-3c-zero-run-count-green.log`
(1 / 1, 0.72 seconds). Independent multiple-attempt and unrelated legacy-run
count coverage remains pending; this focused pass does not establish total
counts across unloaded pages.

The actual stored Edit/Superseded regression now passes in
`/tmp/surge-3c-superseded-current-revision-green.log` (1 / 1, 0.45 seconds):
the historical attempt remains Completed while the current revision needs
execution. The disconnected-stream probe also passes in
`/tmp/surge-3c-reconnect-freshness-green.log` (1 / 1, 0.37 seconds): fresh task
cache does not make an old disconnected Working stream current. Root inspected
all three raw result logs. These are focused functional corrections, not a full
UI gate or approval of the earlier interface illustration. The retained-input
lifetime failure and the complete issue-to-merged-PR scenario remain open.

### Actual quota fallback reaches behavioral RED

The corrected owning daemon/ACP probe in
`/tmp/surge-3b-quota-wire-red-lifecycle-20261001.log` fails 0 / 1 in 5.29 seconds
at the missing fallback open: B has zero actual `new_session` wire operations
instead of one. Before that assertion, the actual host workspace-prepared check,
typed A RateLimited count of one, and A wire counts of one New and one Prompt
all pass. The fixture pauses at A's actual first send, writes tracked and
untracked changes into the host-prepared worktree, then releases the underlying
ACP RPC. Root inspected source ordering and the raw terminal log. This is a
behavioral missing-fallback RED; the previous two prerequisite failures are not.
Same-workspace preservation, both retained provider descriptors and final
completion are still later assertions, not yet proven by this failing run.

Both precode lenses accept a separate manual-Continue recovery-cycle rollover
only after trusted RunContinued tied to current Executing control generation,
runtime invocation/handoff and retained claim. Closing the old audit cycle and
creating the fresh bounded cycle must use one IMMEDIATE CAS transaction against
the old revision and exact control, with idempotent replay returning the original
new cycle. Old observations/reservations/history remain immutable and old wake
authority is invalidated. Missing or contradictory evidence cannot roll over;
cycle creation alone never grants provider dispatch authority. Implementation
and owning verification of this transition remain pending.

### Fleet ownership pagination audit remains open

Current source inspection finds Fleet legacy-run suppression derives its set
from loaded task records' active runs and loaded attempt-history pages. A durable
task with no loaded history therefore needs an independent oracle alongside an
existing terminal project run: loading its details must not be required to learn
that the run belongs to the same task. The UI owner is checking this case and
multiple attempts plus unrelated legacy runs using fixed expected rows/counts.
This is a source-identified coverage gap, not yet a reproduced product failure.
Any correction must use recorded run-to-task ownership and preserve truthful
pagination, rather than infer ownership from identifiers or titles.

The gate-commit integration now registers the validated `gate_commit` core
module, adds `GateStageOutcomeCommitted` / `GateStageRouteCommitted` event
variants, and registers schema 13 / IdentityV13. SessionOpened also carries an
optional, backward-compatible handoff operation identity for exact opening-epoch
association. Root inspected these current sources; producer, trusted reader,
projection and exhaustive consumer integration are still in progress, so no
compile or gate-recovery GREEN follows from this source milestone.

A subsequent audit of retained wake evidence distinguishes historical `wake`
JSON from actionable `wake_at_ms`. Current manual invalidation preserves the
former and clears the latter. The quota owner must verify that wake-consuming
transactions require the actionable schedule as well as the current control
fence: refreshing an inspection object must never turn retained audit evidence
back into automatic dispatch authority. This is a verification obligation, not
yet a demonstrated bypass.

The human-gate source also has a separate timeout path: it durably appends
HumanInputTimedOut before returning the typed rejection or missing-default
error. That path has no accepted human answer and therefore cannot be represented
by fabricating a GateCommitAnswer. Full recovery verification must include a
crash after this timeout event and before terminal failure persistence, preserving
the original request and typed result without repeating the timeout or asking a
new question. This cut remains an explicit open verification item alongside the
accepted-answer/effects/route cuts.

The UI ownership correction now has a source-level observer for a unique
WorkItemAttemptBound in contiguous, successfully folded startup history matching
RunStarted's exact prompt. It retains the item identity through terminal history
and clears it on history reset, gaps, failed fold, late or repeated binding.
Root inspected the observer and reset paths. The independent late-hydration /
unloaded-attempt-page row-count oracle is still pending; absent ownership must
remain explicitly unconfirmed rather than inferred. A minimal real-window view
owning four Textareas is also authored to isolate retained-input lifetime from
Fleet, daemon and command behavior. Neither source change is a test pass yet.

The authoritative gate reader is now being connected: current source checks the
original request occurrence and accepted response, adjacent original-versioned
effect digest, declared outcome and required bootstrap effect shapes. Its route
validator requires the exact accepted commit, adjacent edge/completion, Route
disposition and single consumption. Root inspected these checks. An independent
source review identified a remaining bootstrap-cap validation obligation: a
self-consistent hash and escalation/edit shape alone cannot prove that the
frozen edit-loop cap permitted that disposition. Validation must use the frozen
cap and the pre-effect edit count, including cap zero and exact boundary twins;
the replay accumulator has already observed adjacent effects at this point.
The author is addressing this before claiming reader authority or recovery GREEN.

The concrete cap-authority refinement has passed both precode lenses: freeze
`bootstrap_edit_loop_cap: Option<u32>` in the immutable RunStarted configuration
from the actual engine startup policy. Some(0) means unlimited; legacy None
remains readable but cannot authorize a new bootstrap gate marker. Cold restore
must not substitute today's settings. Recover the pre-effect count with checked
subtraction only for the exact adjacent BootstrapEditRequested; escalation leaves
the count unchanged. Startup configuration fixtures and legitimate/forged cap
boundary twins are required. Root SPEC and independent UI-owner TECH accepted
these constraints; implementation and behavioral evidence remain pending.

Current producer inspection confirms that human-gate effects now append the
GateStageOutcomeCommitted marker in the same batch, binding the actual accepted
response sequence/value and original stage occurrence. RunStarted's core config
also now has the optional frozen cap, and the actual host start sets Some(engine
policy cap). Cold restoration, task startup expected-config equality, writer
route consumption and remaining consumers are still being integrated. No new
build or crash-recovery pass is claimed from this partial source checkpoint.

The reader now checks the frozen bootstrap cap and reconstructs the pre-effect
count with checked subtraction. Root inspected that source correction; boundary
tests remain pending. Route review also requires the actual edge destination,
kind and identity to match the original graph edge selected by the accepted
outcome. Matching only source node/outcome and adjacent StageCompleted is too
weak: pure state folding does not itself validate a declared graph edge.
The author is checking this authority seam and a forged-destination twin before
claiming the new route reader is sufficient for cold continuation.

The per-run gate migration is now registered, and root inspected the new writer
and projection seams: route commits require the original request/stage-entry /
accepted-commit sequence, Route disposition, matching node/outcome and an
unconsumed record. The existing journal-prefix check, event insertion, projection
consumption and graph snapshot share one SQLite transaction. These source checks
do not replace the pending graph-edge authority check, rollback/crash probes or
coherent affected-crate build; no new verification pass is claimed here.

Declared-route validation must use the trusted active graph scope: the actual
router selects Loop/Subgraph body edges while inside a frame, and enforces
traversal counters before committing a route. Root inspected that router and
sent the owning author the nested-gate/revisit obligation. Matching a root edge
alone would reject legitimate nested routes or accept an unrelated route; cold
restoration must also avoid spending the traversal allowance twice. These cases
remain part of the complete recovery matrix, not a new scope reduction.

The first integrated `cargo check -p surge-orchestrator --lib`
(`/tmp/surge-3b-gate-wiring-check1.log`) stops at compile errors: a misplaced
configuration field in query.rs and two EdgeTraversed patterns requesting a
nonexistent outcome field. Root inspected the raw log; this is not a behavioral
RED or a passed build. The writer/projection and cold-consumption integration
are present but still under repair. Declared edge outcome comes from the graph
edge identity; legitimate max-traversal escalation selects the policy's synthetic
edge while StageCompleted retains the original outcome, so its validator needs
the pre-route policy/counter evidence rather than naive outcome equality.

The repaired library build passes in
`/tmp/surge-3b-gate-wiring-check2.log` (8.20 seconds). The owning crash regression
then fails in `/tmp/surge-3b-gate-effects-current1.log` (0 / 1, 8.39 seconds):
its exact post-kill OutcomeReported=2 / StageCompleted=1 assertions pass, but
the restored task never reaches Completed before the bounded wait at line 2094.
Root inspected log and source ordering. This is not the old duplicate-outcome
failure and is not GREEN; the next action is to capture restored control/journal
validation diagnostics and repair the cold continuation, without increasing the
timeout or weakening the final exactly-once assertions.

The same owning exact-cut oracle passes after repairing the startup expected
configuration to match the actual frozen cap:
`/tmp/surge-3b-gate-effects-current2.log` (1 / 1, 0.36 seconds; build 17.87
seconds). Root inspected raw results and the source assertions: actual owner
death after committed effects is followed by Completed with one request/answer,
two outcomes/completions/edges, one RunCompleted and no additional provider
New/Prompt/Resume/Load. This proves the focused generic-gate cut, not bootstrap
edit/rejection/escalation, timeout, nested routes, forged twins or full 3B.

Startup equality still needs explicit legacy coverage: legitimate task journals
with the old absent frozen-cap field must remain readable without treating None
as today's cap. Blanket expected-Some equality would reject even generic stages.
Any compatibility path must retain unknown bootstrap authority and distinguish
genuine old origin from dropping a required frozen cap in new-format history.

The retained-wake cancellation probe's first run
(`/tmp/surge-3b-quota-cancelled-wake-red-20261001.log`) stops at a lib-test
constructor missing SessionOpened.handoff in inspection.rs. Root inspected the
raw compiler output; no behavioral assertion ran and the wake-consumption guard
is unchanged. The library-only compile pass did not cover these test
constructors. Repair the mechanical fixture and rerun before claiming a
behavioral RED or adding the actionable-schedule guard.

The next ready run fails in fixture startup with SingleThreadedRuntime, not a
behavioral assertion. With the fixture's required multi-thread runtime restored,
`/tmp/surge-3b-quota-cancelled-wake-red-runtime-20261001.log` reaches behavioral
RED (0 / 1, 0.06 seconds): retained audit JSON and empty due-wake query checks
pass, but consume returns a newly allocated generation-2 cycle. Root inspected
raw output and source ordering. This isolates the owning invalidation helper's
schedule authority, not the complete real-daemon Manual-control lifecycle.
The author is adding an exact actionable-schedule guard before rerunning.

UI input retention has a concrete source-backed harness diagnosis: the old
fixture constructs and discards render_task_draft outside a real window draw.
GPUI AnyElement allocations then use its fallback thread-local arena rather
than the App frame arena. Root inspected the primary allocation and arena code.
The correction initializes the same draft entities without building abandoned
elements; actual rendering stays inside a real frame. The independent four-input
window-lifetime probe and full semantic fixture rerun must confirm this diagnosis;
no forced draft clearing or leak-check suppression is permitted.

The same cancelled-schedule foundation oracle passes after the exact guard:
`/tmp/surge-3b-quota-cancelled-wake-green-20261001.log` (1 / 1, 0.06 seconds;
build 6.27 seconds). Root inspected log and implementation: the consuming
IMMEDIATE transaction requires SQL wake_at_ms to equal the immutable wake's
due time, alongside the existing identity/due/control/revision/claim checks.
Retained audit JSON with no actionable schedule cannot allocate a new cycle.
Positive legitimate-wake and Manual-control regression coverage, full automatic
handoff/coordinator wiring, and actual A-to-B fallback remain open.

### Native Computer Use walkthrough started

Root launched the existing surge-ui binary with isolated SURGE_HOME and packaged
a temporary macOS app so Computer Use could bind its window. This binary predates
current source changes and is exploratory evidence only. Computer Use captured
and root inspected the exact saved screenshots:

1. Start screen opens successfully: `/tmp/surge-computer-audit-BlKn9k/01-start.png`.
2. Project Tasks screen is visible but daemon offline:
   `/tmp/surge-computer-audit-BlKn9k/02-project.png`.
3. New task opens its form:
   `/tmp/surge-computer-audit-BlKn9k/03-new-task.png`.

The project picker changed externally during capture and the refreshed state
showed the real Surge repository; no task was submitted there. The form visibly
uses placeholder-only field labels, has no visible cancel control and repeats
offline messages. These findings must be rechecked on the fresh native build;
no mutation flow, Stop/Continue or complete native audit is claimed yet.

The independent four-Textarea real-frame probe passes (1 / 1, 0.03 seconds,
`/tmp/surge-3c-four-textarea-real-frame-probe2.log`). The corrected original
typed-conflict fixture also passes (1 / 1, 0.91 seconds,
`/tmp/surge-3c-typed-conflict-draft-attempt5.log`), retaining semantic/draft and
all four weak-release assertions. Root inspected both raw logs. The source-backed
arena diagnosis is now supported by these scoped runtime checks; full UI gates
and the actual native workflow remain pending.

The fresh native UI build passes (`/tmp/surge-3c-current-native-build.log`,
13.34 seconds). Root replaced only the temporary app executable and used
Computer Use to select the separate empty `/tmp/surge-computer-audit-BlKn9k/project`
folder. The app initialized and opened it successfully. The first daemon-launch
failure came from the temporary package lacking `surge` in PATH; after supplying
the development binary directory, the same UI action reaches Live.

Actual current-build native checks now show:

1. Offline Create retains all three input values and the original retry action
   (`05-current-offline-draft.png`).
2. The sidebar reaches Live after daemon startup (`06-daemon-connection.png`).
3. Retry creates exactly one visible task / All1 (`07-created-task.png`);
   a read-only registry query independently confirms work_items count 1.
4. Detail actions can be reached by scrolling (`08-task-detail-scroll.png`).
5. Posting discussion commits one discussion and a second operation in SQLite,
   but the captured UI still shows the filled draft without visible acceptance
   or history confirmation (`09-discussion-result.png`, `10-discussion-history.png`).
   Visible acknowledgment/refresh remains under investigation.

These files are in `/tmp/surge-computer-audit-BlKn9k/`. Current screenshots
also retain the placeholder-only field labels and duplicate offline guidance.
No provider execution, Stop/Continue, PR review or merge was performed by this
walkthrough. The real Surge repository was not used for task mutations.

### Native Computer Use: discussion and requirement edit (2026-10-01)

The isolated SurgeAudit task shows the accepted discussion below the entire composer/workflow form; screenshot `/tmp/surge-computer-audit-BlKn9k/11-discussion-bottom.png` was independently viewed. This corrects the earlier viewport-level uncertainty: discussion persistence succeeds, but composer success feedback is absent.

Editing requirements immediately after discussion produced a genuine stale-item-version rejection. The original draft survived. Through native UI, dismissed the definitively rejected operation, refreshed tasks, selected Use current task version for this draft, and accepted edited requirements. Screenshot `14-edited-requirements.png` shows revision 2 and observed task version 2; draft still says revision 1, so acknowledgement/draft alignment remains a UX issue. No execution, stop/resume, PR or merge acceptance is inferred from this check.

Legacy cap owning regression: `/tmp/surge-3b-legacy-cap-green.log` independently inspected, 1 passed in 0.41s. Covers legitimate original-version-12 missing-cap generic-task resume and original-version-13 missing-cap refusal. Bootstrap frozen capture and nested routing remain separate pending checks.

### Native Computer Use: terminal workflow launch (2026-10-01)

Pasted a reviewed terminal-only graph into the isolated task using native input. First Start after own requirement edit was rejected for stale item version; dismissed definitive rejection, refreshed, explicitly selected Use current task version, then Start. Screenshot `/tmp/surge-computer-audit-BlKn9k/16-terminal-completed.png`, independently viewed, shows All 1 / Finished 1 / Completed revision 2 / observed task version 3. This proves native admission and terminal completion only, not ACP execution, stop/resume, verification or PR. AX still exposes Suspend alongside terminal status; per-detail freshness review owns this potential stale-action defect.

### Native Computer Use: HumanGate Suspend contradicts continuation (2026-10-01)

Fresh daemon/CLI/UI build exited 0 in 57.06s (`/tmp/surge-native-current-build-20261001.log`). Restarted only the isolated audit daemon after confirming pid 11720 and its audit socket; preserved user daemon. Created a second task and launched a valid generic HumanGate→terminal workflow through native UI. Real pending decision appeared. Clicking visible Suspend produced RunAborted with reason `stop_run requested`; screenshot `/tmp/surge-computer-audit-BlKn9k/19-gate-stopped.png` independently viewed. Read-only registry confirms run `run-01M3V3DNNNAKGBK0SHM2RV5VFB` attempt aborted, task version 4 and active_run NULL. Continue is unavailable. This is a real unmet same-run continuation requirement, not an ACP fixture limitation.

Source diagnosis: run_task interruption handling looks for SessionOpened invocation after Cancelled; generic HumanGate has no provider invocation, so no pending suspension is established and cancellation falls through to abort. Builder owns the real socket regression and typed non-provider recovery repair; original unanswered decision ID must survive without a fabricated provider invocation. The earlier terminal Suspend button was transient and disappeared once details caught up; retained-detail freshness still requires a guard during that interval.

### Native Computer Use: terminal decision inbox (2026-10-01)

After the erroneous HumanGate abort, opened Decisions natively. Screenshot `/tmp/surge-computer-audit-BlKn9k/20-aborted-decision-inbox.png` independently viewed: one failure card, Stopped with an error, Open cockpit and Dismiss; no actionable Approve. Badge 1 WAITING represents an undismissed failure, not a retained actionable approval. The card exposes the generated requirement-prompt prefix/hash as mission instead of the human task title; fix should use trusted durable run→task binding and preserve backend prompt contract, never parse the prefix to infer ownership. This does not replace the pending same-run HumanGate suspension repair.

### Gate regression readiness and PR state (2026-10-01)

Reviewed actual socket oracle `pure_human_gate_suspend_is_not_abort_and_reuses_original_decision`: starts a real generic gate, waits for original HumanInputRequested, sends Suspend, expects no RunAborted and durable Suspended, recreates host, sends Continue and resolves original request ID, expects Completed, exactly one request, zero SessionOpened. First log `/tmp/surge-3b-native-human-gate-suspend-red.log` stopped at an unrelated pooled-connection compiler error in quota opening.rs; this is not behavioral RED. Author corrected that mechanical error; owning behavior run remains pending.

Read-only GitHub state: `gh pr list --state open` returned empty. Latest PRs 88/87/86 are merged. Current uncommitted implementation therefore has no open review/CI acceptance; no claim that requested PR review lifecycle is complete.

### HumanGate owning behavioral RED confirmed (2026-10-01)

Independently inspected `/tmp/surge-3b-native-human-gate-suspend-red2.log`: compiled successfully, actual socket test failed 0/1 in 0.20s because RunAborted count was 1 instead of 0 after Suspend. This is the genuine behavioral RED corresponding to native screenshot19; earlier compiler-only logs remain excluded. Reviewed storage acknowledgement: equality of snapshot.pending_stage and journal fence is insufficient by itself for a new WaitingHumanGate phase. Read-only original-journal validation must join exact node/request ID/request sequence/stage occurrence and reject forged cross-node or obsolete request identities even when fence and snapshot agree. Producer/reader repair and cold restart GREEN remain pending.

### Gate occurrence review: repeated review loop (2026-10-01)

Independently read new WaitingHumanGate source and existing restoration selectors. `restored_gate_entry` picks the first historical HumanGate decision for the same cursor node, while execute_human_gate_node collects all historical same-node decisions and rejects length >1. This requires behavioral verification for operator Edit/backtrack revisiting the same gate: only the current unresolved/accepted-but-unconsumed occurrence may restore its original request; a fresh visit must create a fresh request while retaining old journal history. Requested owning two-visits oracle and exact active StageEntered/routed-commit joins from builder. Single pause/resume GREEN would not prove the repeated review-loop requirement.

### HumanGate suspend/cold-continue focused GREEN (2026-10-01)

Independently inspected `/tmp/surge-3b-native-human-gate-suspend-green3.log`: 1 passed in 0.38s after build 24.79s. Actual socket oracle reaches durable Suspended with no RunAborted, recreates daemon host, Continues same attempt, resolves original request ID, completes, observes one HumanInputRequested and zero SessionOpened. Reader now checks WaitingHumanGate exact request/node/purpose/stage-entry/requested sequence/active occurrence against original journal. This establishes the focused non-provider pause/restart scenario; does not prove concurrent answer/suspend races, forged phase negative, repeat gate visits or full native UI repair. UI stale-detail owning suite follows in serialized Cargo lane; later fresh native build is still required.

### Current UI and handoff oracle review (2026-10-01)

`/tmp/surge-3c-current-detail-red2.log` currently fails compilation because the new MissingBootstrapPolicy attention variant is not covered in UI rendering; excluded from behavioral RED. UI author notified to render an explicit recovery/new-capture explanation without guessing policy.

Reviewed new opening store oracles: consumed admission persists OpeningUnknown and denies another permit; exact Resume retains provider session ID with fresh internal session and idempotent confirmation; wrong epoch/reused internal connection/wrong node/new control refuse establishment. Fixtures explicitly seed a Reserved storage contract and do not execute ACP RPC or establish real writer cleanup. Requested latest-cycle/source-revision negative and honest boundary labels; actual fallback wire acceptance remains pending.

### UI stale-detail behavioral RED and native retained workspaces (2026-10-01)

Independently inspected `/tmp/surge-3c-current-detail-red3.log` and owning source: compiled, actual case fails 0/1 in 2.28s, Running instead of Run state is unconfirmed after real admitted Suspend. The fixture first establishes a genuine old Executing detail and recorded Working journal prefix; fresh item/control versions are higher. No invented Executing prerequisite. UI fix owns both publication fencing and mutating command authority, while exact pending retries remain available.

Read-only native registry check confirms both audit task workspace paths still exist after terminal completion/abort; paths are task-retained workspaces, separate from attempt run IDs. This proves directory retention only; no dirty-file preservation or ACP resume acceptance inferred.

### UI publication review: delayed list after acknowledgement (2026-10-01)

Reviewed new per-item detail request generation and monotonic detail publication source. Detail matching joins item/project/version/accepted revision/attempt generation/active run/archive state and control item/run/attempt. Found a remaining publication seam: apply_page currently clears current records and accepts a lower-version item even if a newer acknowledged detail arrived after that list request began. Request-scope freshness alone does not prove response data freshness. UI owner notified to preserve monotonically newer item evidence for the same project/item and cover delayed-list publication; immutable retry bodies and retained history remain unaffected.

### Current-detail focused GREEN (2026-10-01)

Independently inspected `/tmp/surge-3c-current-detail-green.log`: 1 passed in 1.07s, build 44.48s. Same actual admitted-Suspend oracle now refuses old Executing detail as current and includes refusal to enqueue a new mutation from stale details. Source adds per-item Show generation and monotonic version/assignment/control publication, plus late-list merge preserving newer item evidence. Exact pending retries remain separate. Further adversarial delayed-publication checks, submission captured-item identity and native fresh rebuild remain pending; this focused result is not full UI acceptance.

### Correction: completed gate projection lifecycle (2026-10-01)

Broader source inspection found RunMemory StageCompleted removes the matching current HumanGate decision from the active recovery map before GateStageRouteCommitted, while the original journal retains history. The earlier selector-only suspicion therefore does not establish a repeat-visit defect. Two-visit actual-socket oracle remains necessary; no selector patch should be made solely from that suspicion if existing behavior passes. Corrected prior evidence wording accordingly.

### Composer acknowledgement source review (2026-10-01)

New source shows per-task visible accepted feedback, deferred clearing of only the unchanged submitted comment in the owning Window render, and own Edit draft rebasing only when submitted text/criteria/version/revision still match. Production behavior tests and native repeat are pending. Discussion acknowledgement currently advances no draft version, so the observed native Discuss→Edit stale-version seam still needs an exact own-ACK revision join; author notified without allowing silent external-edit rebasing.

Two-visit gate baseline first run stopped at a fixture local named request shadowing the socket helper; exclude compile failure from behavioral findings. Mechanical rename requested.

### Repeat gate visit baseline verified (2026-10-01)

Independently inspected `/tmp/surge-3b-human-gate-two-visits-baseline2.log`: actual socket edit/backtrack then approve scenario passes 1/1 in 0.19s. Two request IDs and sequences differ; two requests/responses precede Completed. Existing StageCompleted active-decision removal is sufficient in this scenario; no selector product patch was made. First compiler failure remains excluded. Forged waiting phase test initially called raw inspect_run rather than trusted inspect_folded_run used by daemon; corrected oracle is running separately, no guard claim until its result.

### Forged waiting phase: reader refusal verified, cold control unresolved (2026-10-01)

`/tmp/surge-3b-human-gate-forged-waiting3.log` uses trusted inspect_folded_run and passes the specific forged-request refusal assertion. The full case still fails 0/1 in 8.22s waiting for attempt Attention after actual Continue. This establishes reader refusal only; cold command response/control/retained attempt need inspection before classifying missing Attention or changing the oracle. No RunContinued or complete recovery claim is inferred.

Reviewed extended actual-daemon UI ACK fixture: unchanged own accepted Edit rebases revision, acknowledged Discussion clears only original body and advances version joined to unchanged accepted revision/hash, edited-in-flight text remains with original base. Source oracle includes actual discussion row count and original lifetime checks. Test execution remains pending.

### Cold Continue refusal diagnostic identifies a real state gap (2026-10-01)

Independently inspected `/tmp/surge-3b-human-gate-forged-waiting-diagnostic.log`: daemon returns work_item_ok with control ContinueReserved generation 2 and the exact trusted-reader rejection diagnostic; the attempt remains Suspended without diagnostic. No provider launches, but the persistent command still presents an in-flight reservation with no actor able to fulfill it. Builder notified to settle current-generation recovery Attention atomically/with generation fencing, preserve assignment/worktree/original evidence and reject superseded-control overwrites. This differs from a mere test waiting mistake and remains pending owning GREEN.

### Forged cold Continue focused GREEN (2026-10-01)

Independently inspected `/tmp/surge-3b-human-gate-forged-waiting-green.log`: 1 passed in 0.23s after build 24.98s. Actual response and persisted control are Attention generation 2 with the original-reader refusal diagnostic; attempt Attention carries the same diagnostic and retains assignment. Test also requires no RunContinued. Recovery refusal transaction checks lock/claim/active assignment and exact latest Continue generation/operation before changing attempt/control together. Superseded-control negative and native current-state rendering remain separate pending coverage. This repairs the previous ContinueReserved-with-no-actor gap for this exact corrupted gate history.

### Recovery refusal final focused verification (2026-10-01)

Independently inspected `/tmp/surge-3b-human-gate-forged-waiting-final.log`: 1/1 passed in 0.22s, build 25.09s. Refusal now joins expected/current item and attempt generation and increments task version atomically with control/attempt Attention, making the post-admission state transition observable to UI freshness checks. Test asserts actual returned Control Attention rather than diagnostic printing. Full affected suite/strict gate and native UI remain pending.

All-provider-session history cannot be inferred from the latest participant or capped 240-row run log. SessionOpened records must retain every original/reopened/fallback connection identity; SessionClosed disposition requires original journal evidence and does not prove complete writer-domain quiescence. UI history slice is being designed separately from cleanup authority.

### UI currentness publication and captured command verification (2026-10-01)

Independently inspected `/tmp/surge-3c-current-detail-publication-green.log`: 1/1 passed in 0.96s, build 42.32s. Source oracle uses actual old/fresh daemon item details, refuses old recorded Executing status and new mutations, applies older Show responses in both obsolete request order and later-request/lower-version form, applies lower-version List after fresh detail, and preserves latest record/control generation. New commands with different target or stale expected version cannot enter task_submissions. These are UI boundary publication assertions over actual daemon records; they do not simulate every network scheduling interleaving. ACK/in-flight real-daemon fixture follows separately.

### ACK fixture first execution does not yet establish success (2026-10-01)

`/tmp/surge-3c-ack-draft-feedback-green.log` independently inspected: 0/1 in 0.42s, expected typed daemon rejection flag was absent at the earlier conflict prerequisite, so new ACK assertions were not reached. Need inspect actual local task_error/request boundary and cache freshness before classifying this as production ACK failure; do not bypass currentness checks merely to satisfy the fixture. Author notified.

Prepared acceptance requirement for operator answer while Suspended: validate original durable request/purpose/options, persist one accepted answer without dispatch/routing, preserve identical replay receipt and refuse conflicting replacement, then Continue consumes the original answer in the same run. Actual socket RED remains pending; no support claim from the existing live resolver.

### Composer and lost reply focused GREEN (2026-10-01)

Independently inspected `/tmp/surge-3c-ack-draft-feedback-green2.log`: 1/1 passed in 1.42s after actual original-version hydration before unseen external Edit. Covers typed conflict, uncertain exact immutable retry, retained drafts, real own Edit rebasing, own Discuss acknowledgement/version advance, unchanged comment clearing in owning window, modified-in-flight text preservation and original input lifetime checks. First failed prerequisite remains preserved, no production freshness bypass.

`/tmp/surge-3c-lost-ack-currentness-regression.log`: 1/1 passed in 0.45s; lost accepted daemon reply retry preserves original command and one actual durable discussion entry. Updated native UI/daemon build and Computer Use repeat are next; these focused tests are not full UI or product acceptance.

### Native Computer Use follow-up: discussion, revision, gate resume (2026-10-01)

Fresh audited UI artifact mtime 02:19:29 and independently built daemon were exercised in the isolated `/tmp/surge-computer-audit-BlKn9k` application. Screenshot `22-current-discussion-feedback.png` was independently viewed: the original message appears once in discussion, “Discussion accepted” is visible, and the composer is empty. Screenshot `23-current-edit-after-discussion.png` was independently viewed: saving changed requirements immediately after discussion updates both task revision and draft basis to revision 2 without manually rebasing.

The real HumanGate workflow then reached its operator question. Actual visible Suspend produced attempt 1 Suspended, retained discussion/revisions, and Continue saved session (`25-current-gate-suspended.png`, independently viewed). Clicking Continue restored attempt 1 Launched and task Waiting for human input. This is partial evidence, not successful completion of the resume scenario: Decisions only displays an unrelated older aborted failure and omits the resumed question. Read-only registry identifies run `run-01M3V5RXSQRHCGFJFCKTFMCM8Y`, ordinal/generation 1, launched. Its journal has original request at seq8, matching WaitingHumanGate suspension at seq9, and RunContinued at seq10; no subsequent events and `pending_approvals` is empty. Original request is `gate-01M3V5RXTZZR6FJNQQGS50QE9Z`, with a one-hour timeout. The workspace name uses a different preallocated identifier; it must not be mistaken for the execution run ID. Backend owner received this evidence to investigate actual question registration on restore. Full native pause→continue→answer→completion acceptance remains open.

Follow-up source correction and backend evidence: `pending_approvals` projects ApprovalRequested/ApprovalDecided, not HumanInputRequested, so its empty rows do not establish a persistence defect. The original seq8 question is authoritative. Source inspection instead finds `RunStreamState::apply_at` clearing pending decisions on every EngineRunEvent::Terminal, including suspended stream outcomes; replay ignores seq <= last_seq for the legacy pending fold while rebuilding canonical display. Backend and UI owners independently identified this mismatch. Proposed repair preserves original request identity across nonterminal stream closure and rebuilds pending decisions from the trusted full prefix; a genuine failing UI regression is still required before implementation acceptance.

Independently inspected `/tmp/surge-3b-human-gate-paused-answer-green1.log`: actual daemon socket integration `suspended_original_gate_answer_is_durable_before_continue` passes 1/1 in 0.27s after a 43.61s build. Source fixture cold-starts the host after suspension, submits the original gate identity, asserts one durable HumanInputResolved before Continue, then continues and completes with one HumanInputRequested and zero SessionOpened. This verifies a pure host gate, not ACP session resume or native question rendering. Negative stale controls, invalid/expired responses, archived ownership and conflicting replays remain to be checked.

### Phase 4 implementation inventory: existing GitHub merge surface

Read-only current source inspection finds `GitHubTaskSource::check_merge_readiness` in `crates/surge-intake/src/github/source.rs` resolving a PR by the issue number, explicitly assuming PR# == issue#. This must be replaced by the persistent task's explicit PR association; separate GitHub issue and PR numbers do not establish a valid mapping. Existing readiness captures the head SHA, checks open/draft/mergeable state and paginates reviews, and merge submission pins that SHA. Preserve those protections.

The current `evaluate_reviews` keeps latest non-comment review per author and requires at least one approval. It does not establish current-head review coverage or disposition of inline threads; commented bot reviews are discarded from that verdict. No durable per-thread repair/re-review coordinator was found in the searched daemon/orchestrator/intake surfaces. This inventory is not a full defect audit. Phase 4 must model exact PR/repository/head identity, human and bot inline findings and their dispositions, actual commit/re-review evidence, CI statuses and policy decisions, with a single editor and restart-safe receipts. The accepted policy permits a reasoned merge decision when CI/spec/evidence meet the gates and bots are silent or quota exhausted; this requires explicit policy and recorded reasoning, rather than the current unconditional approval requirement or silently ignoring comments. No implementation or acceptance is claimed by this inventory.

### Route reconstruction guard: forged edge-kind journal (2026-10-01)

Registered `RouteEvidence` in the authoritative folded-run inspector. The bounded
journal scanner now reconstructs active loop/subgraph scope and traversal
counters, then resolves each recorded edge against the accepted graph before
trusting route identity. `/tmp/surge-3b-route-evidence-check2.log` passes
`cargo check -p surge-persistence` after aligning the event boundary with
`EdgeTraversed { edge, .. }`.

The actual completed HumanGate journal twin is valid before mutation; changing
only its recorded edge kind to Backtrack remains self-consistent with the prior
identity checks. `/tmp/surge-3b-gate-route-kind-red.log` is the owning behavioral
RED: the forged event was accepted when only the new route guard hook was
removed. Restoring the exact guard makes the same test pass in
`/tmp/surge-3b-gate-route-kind-green.log` (1/1). The engine now also passes the
valid empty-loop and nested-subgraph/max-traversal journals in
`/tmp/surge-3b-route-evidence-empty-nested-green2.log` (9/9 for the owning
target, including actual three HumanGate request/response pairs). Reverting
only the parent-frame lookup makes the same valid nested journal fail with
`outer subgraph node nested_call missing or wrong kind` in
`/tmp/surge-3b-nested-parent-scope-red2.log`; restoring it returns the owning
target to GREEN. Broader persistence checks remain pending.

The initial fixture attempt removed its required `completed` return edge and
was rejected before inspection; that was a fixture error. A valid graph then
exposed the engine bug: `current_subgraph_outputs` looked up the nested outer
node only in the root graph, even though it belongs to its parent loop-body
frame. The final valid fixture places the bounded synthetic edge on a real
HumanGate route inside the nested graph and proves two Forward traversals,
then one Escalate, with three actual decision request/response pairs.

### ACP opening identity test fixtures restored to the current contract

The ACP facade now returns `OpenedSession`, which couples the bridge-local
`SessionId` with the exact provider descriptor and operation mode. A full
`cargo check --workspace --all-targets` exposed legacy test doubles and
integration fixtures still returning or storing only `SessionId`; updated those
fixtures to construct valid descriptors or explicitly extract the local ID.
Updated the legacy `SessionOpened` fixture with `handoff: None` for its
backward-compatible primary-open case.

Verification: `cargo check --workspace --all-targets` passes. `cargo test
-p surge-acp` passes 275 unit tests and all enabled integrations; two explicit
opt-in tests remain ignored (real rate-limit wire probe and live Ollama smoke).
`cargo test -p surge-orchestrator --test engine_acp_permission --test
archetypes_mock_test` passes 19 tests, and the affected daemon facade suites
(`daemon_parity_test`, `daemon_resume_stream`, `triage_wiring`) pass 8 tests.
These checks validate the updated opening-identity API and its test doubles;
they do not close the still-open 3B quota fallback or full phase acceptance.

### Recovery executor decomposition and strict library gates (2026-10-03)

Split run restoration into suspended-phase validation, committed-route replay,
and a bounded stage-step loop. Stage interruption/suspension evidence collection
now has separate helpers, preserving the existing journal ordering and
provider-session recovery semantics. Split durable task command mutation/control
from daemon completion supervision so each lifecycle boundary is independently
readable. Boxed large suspension and ACP session-opening payloads where their
enum layout otherwise inflated executor futures.

Verification: `cargo check --workspace --all-targets` passes. The two selected
orchestrator suites pass 19 tests; daemon parity/resume/triage suites pass 8.
Strict `cargo clippy -p surge-acp -p surge-orchestrator -p surge-daemon --lib
--bins -- -D warnings` passes. Adding `--all-targets` currently reaches 16
existing style findings in `crates/surge-daemon/tests/work_item_route_test.rs`
(excessive nesting/collapsible conditions); it is not a passing gate and remains
open. These structural/verification changes do not complete quota fallback,
automatic quota wake, the desktop lifecycle, or the overall phases.

### Quota integration current-state boundary rechecked (2026-10-03)

The coordinator draft in `crates/surge-orchestrator/src/engine/quota_recovery.rs`
is not declared by `engine/mod.rs`; its functions have no production callers.
Current `dispatch_agent_node_with_capacity_gate` still sends a rate-limited
error through the generic runtime capacity ledger and parks the whole run.
`EngineRunConfig` still has no frozen per-node quota routing policy. To preserve
the recovery-cycle owner fence, `WorkItemLaunchClaim` is now cloneable through a
shared `Arc<File>` lock: the daemon supervisor and engine task retain the same
open lock description, run, binding and token; the run task revalidates this
authority immediately before each persistent-task ACP dispatch. This enables
future task-owned cycle mutations without minting a second claimant. The
one-shot `QuotaOpenPermit` must still be spent immediately before opening the
selected candidate. No fallback or wake behavior is claimed by the coordinator
draft alone.

Verification: the focused last-clone lock-lifetime test passes, the existing
cold-daemon resume test passes, workspace all-target check passes, and strict
clippy for persistence/orchestrator/daemon libraries and binaries passes.

### Frozen quota policy carrier (2026-10-03)

Added the validated `FrozenQuotaPolicy` as a backward-compatible
`EngineRunConfig` field and validate its stage references at work-item freeze,
run startup and task resume. Each configured stage must reference an Agent node;
when the profile registry can resolve its primary runtime, the first candidate
must match that runtime. Old serialized engine configs default to an empty
policy. `cargo check -p surge-orchestrator --lib`, strict orchestrator library
clippy, and the targeted legacy-deserialization unit test pass.

This is only the immutable carrier/validation boundary: current
`WorkItemCommand::Start` now accepts optional quota-policy JSON. CLI
`surge task start --quota-recovery <policy.json>` and MCP task control pass the
same immutable body; the daemon validates it before reservation and stores the
typed policy in the attempt's frozen `EngineRunConfig`. A daemon integration
test asserts the policy survives reservation. The orchestrator has not yet
connected quota events to the recovery-cycle/handoff APIs or provider opening
permit. Automatic/configured candidate resolution, fallback execution, and
durable wake consumption remain open.

### Quota opening epoch producer (2026-10-03)

Added `WorkItemStore::reserve_quota_open` as the production-owned producer for
an opening epoch after a candidate reservation. In one registry transaction it
revalidates the task claim, active cycle/revision/control generation, exact
candidate receipt and body, and the stage's frozen candidate allowlist before
writing the immutable operation receipt and `reserved` handoff. Repeating the
same reservation and launch returns the original epoch; a changed launch body
conflicts. Replayed candidate receipts cannot create a new opening epoch, and
`admit_provider_open` still spends the resulting permit once.

The persistence opening module passes 11/11 focused tests, including creation
through this production API after deleting the test-seeded handoff, exact replay,
and one-shot permit admission. Strict persistence library clippy and diff checks
pass. The ACP stage does not yet call this producer, record selected typed quota
origins through it, or execute the next frozen candidate. The actual daemon A429
→ B test still fails with no B opening; fallback, restart reconciliation and
automatic wake remain unimplemented and 3B remains open.

### Task-owned typed quota origin capture (2026-10-03)

When a task-owned ACP `send_message` returns the typed `RateLimited` variant,
the agent stage now records a host-sealed quota origin against the primary
reservation before session cleanup. The record binds the real opening sequence,
logical provider invocation, internal session ID, raw details, provider retry
delay, frozen observation lifetime and exhausted state. Persistence validates
the active claim/cycle and journal opening; if it cannot durably record the
origin, the stage returns `RecoveryRequired` instead of losing the evidence and
letting generic parking pretend task recovery was captured. Generic runs and
non-quota errors retain their existing behavior.

Verification: orchestrator library strict clippy passes; the ACP stage rate-limit
suite passes 10/10, including typed mapping and cleanup-failure preservation.
This still does not reserve/open an alternate candidate or schedule a task-owned
wake; real A429 → B remains unimplemented and 3B remains incomplete.

### Primary task quota-cycle initialization (2026-10-03)

Persistent task agent stages now receive the host-owned task claim and the
matching immutable per-node `FrozenQuotaStage`. After an actual New ACP session
is durably recorded, and before its first prompt is sent, the stage binds the
actual provider descriptor to that frozen policy, begins/reuses the logical
invocation's cycle at the current control generation, and reserves its primary
candidate. Initialization failure closes the session and authenticated stage
endpoint and produces `RecoveryRequired`; ordinary runs and provider
continuations do not enter this path. This establishes the durable primary
cycle that typed quota handling needs.

Verification: orchestrator library check and strict library clippy pass;
`engine_acp_permission` passes 18/18. The wider orchestrator test-target check
hit `No space left on device` while linking a proc-macro build script, then
regenerable orchestrator artifacts were cleaned. ACP fallback still does not
consume this cycle, record its typed exhaustion, or open the next candidate;
the daemon A429 → B acceptance remains failing and 3B remains incomplete.

### Next-candidate reservation gate (2026-10-03)

Added `reserve_next_candidate_after_exhaustion`. It verifies the caller's exact
cycle revision/control fence against the stage policy already bound to the
trusted provider opening, requires every existing reservation to carry a
sealed typed exhaustion marker, then selects the first unattempted candidate
in frozen policy order and reserves it through the existing one-shot registry
path. If another writer advances the cycle, the CAS conflicts; if a selected
candidate lacks a definitive origin (for example after an uncertain open), the
method refuses to skip ahead. Disabled policy and exhausted candidate lists do
not manufacture a candidate.

The regression test records a journal-bound primary 429, reserves B once in
configured order, and proves an unconfirmed B attempt blocks another grant.
The opening/handoff persistence slice passes 12/12 and strict persistence
library clippy passes. This does not yet create B's launch contract, opening
epoch, ACP call or automatic wake; runtime A → B remains unverified and 3B is
still open.

### Quota fallback ACP dispatch (2026-10-03)

The agent stage can now consume an admitted quota-opening permit. Before the
provider RPC it resolves the frozen candidate through the normal agent registry,
checks the launch fingerprint, applies a candidate-pinned model when present,
and opens a fresh ACP session. The `SessionOpened` event carries the handoff
epoch; persistence confirms that exact journal event before the stage sends its
prompt. Failed confirmation closes both the stage endpoint and provider session
and returns `RecoveryRequired`.

On a task-owned typed 429, the stage retains the updated durable cycle, reserves
the next unattempted candidate in frozen order, writes and admits its immutable
handoff, and retries the same node with the same prompt inputs and workspace.
Each retried opening gets a new provider invocation and internal session while
remaining attached to the original logical quota cycle. The daemon acceptance
fixture now freezes explicit A→B candidates and their launch fingerprints.

Verification: orchestrator strict library clippy, the targeted ACP rate-limit
test-target check, and the daemon quota acceptance test-target check pass. The
daemon A→B end-to-end test also passes against two real mock ACP processes: A
returns a typed 429, B opens and receives the same task prompt, the run completes,
and both tracked and untracked workspace edits survive.

Generic due-parked task attempts now resume through their durable launch claim;
task reconciliation skips parked rows so it cannot bypass `wake_at`, and only a
confirmed `Launched` attempt is eligible for automatic task wake. The daemon
library check, all eight wake-scheduler tests, and strict daemon library clippy
pass after this change. A durable task fixture also proves a due parked attempt
in `Attention` is neither resumed generically nor silently reactivated; its
state remains available for explicit recovery.

Persistence now exposes `WorkItemStore::reserve_automatic_wake`: one immediate
transaction verifies the exact due schedule and current confirmed Capacity
fence, closes the source cycle, inserts the next cycle and candidate receipt,
reserves Continue control, and records its opening handoff. A repeated exact
request returns that same receipt; provider opening remains behind the existing
one-shot permit. Persistence tests pass with a real `RunSuspended` journal
fence, replay and second-permit refusal, plus a negative case proving a due timer
without confirmed Capacity control leaves the source cycle untouched. Strict
persistence library clippy and both focused acceptance cases pass.

Prompt admission now has a second durable boundary: after the provider opening
is confirmed, `authorize_automatic_wake_prompt` requires the exact latest
`RunContinued` journal sequence, moves both the task control and handoff to
`Executing`, and makes exact replay idempotent. The persistence library suite
passes (430 tests) with a real opened-session event in this continuation test.

The daemon does not yet call this API, and the stage resume path does not yet
consume an automatic-wake handoff. End-to-end quota-cycle wake, cold-start
reconciliation and 3B acceptance remain open.
