# Bootstrap implementation continuation through WorkItem ownership

Status: PROPOSED, reshaped after independent review findings CONT-ARCH-01,
CONT-ARCH-02, CSEC-1 and CSEC-2; owner pre-code verdict ACCEPTABLE for focused
re-review. Independent critic/security acceptance remains pending. No
implementation dispatch is authorized by this file.
Task repair count: 0/3 for this new feature. This does not reset Home 5/5 or
provider-witness accounting. The human separately authorized witness round 4/4;
that mandate and its active source author are independent of this proposal.
No push, PR or merge authority is inferred.

Analysis originally used source `03a7ed20`; the proposal was written at
`2923e96cac2774b74bd9e58bcf03152fb891f0de` after the independent AppTester fix.
The concurrent witness working diff is outside this proposal. No ecosystem API
assumptions, new dependency, new crate or unsafe implementation are proposed.

## Problem and full acceptance scope

Actual durable native acceptance root:
`/Users/vanyastafford/Develop/surge-acceptance/habit-tracker-sol61-20261016`.
Planning `run-01M538BSKHNKR64K4C8TC3X3Z6` completed at seq113; implementation
`run-01M538BSKHWQTW2EHQ2QSHH74A` failed at seq117 after an accepted Build and
a rejected AppTester candidate. There are no WorkItem associations for these
legacy runs. Bootstrap Retry accepts NeedsAttention, not terminal Failed.
Starting again from the clean original checkout discards the partial app.

The new operation must continue this same partial implementation with its
approved graph and truthful history. It must not raw-resume the terminal run,
rewrite journal history, manually copy the partial app into another project,
or replace the original request with an easier fixture.

The full user goal remains open: two complete habit-tracker milestones and all
eight tasks, a useful runnable result, a follow-up preserving history, existing
project improvement, real cold recovery, truthful native UI and current platform
acceptance including Windows. Actual provider execution must retain codex-acp
and exact gpt-6.1-sol. This feature and its v1 refusal policy are intermediate
work, not a new definition of project completion.

## Decision, alternatives and module ownership

Selected: one host operation bridges the failed legacy Bootstrap implementation
into one persistent WorkItem and its first real attempt. A fresh successor run
starts with its own binding and an authenticated semantic execution seed. The
existing partial workspace keeps its original Git owner and actual current files.

Rejected: Bootstrap-only successor lifecycle. It would duplicate reservation,
workspace exclusivity, cancellation, cold reconciliation, usage and controls.
Another failure, follow-up requirements and quota continuation would deepen that
duplication. Reuse WorkItem attempt ownership instead.

Rejected: generic fork/copy-prefix plus Resume. `engine/fork.rs:153-226` copies
the old RunStarted/session/usage prefix and publishes lineage across two logs.
Its isolated checkpoint restoration can discard later partial files. That is
incompatible with a truthful new accepted WorkItem startup. Generic fork's
existing callers are not broadened or silently reinterpreted.

Rejected: new clean bootstrap or arbitrary existing-directory adoption. Neither
preserves this execution's accepted progress and original ownership.

The interface has one submit operation and ordinary receipt/task inspection.
Preparation, provenance validation, seed capture, ownership transfer and recovery
stay behind that interface. Do not expose separate client calls for adopting a
workspace, injecting a seed or selecting a resume cursor.

Core owns pure provenance, manifest, semantic-memory seed and shared observation
contracts, plus the serializable cursor/frame records reused from the current
snapshot representation. Move/re-export those records rather than define a
second frame wire format; existing EngineSnapshot remains in orchestrator.
Orchestrator owns source capture, executable seed/frame validation, runtime
conversion and cursor/checkpoint selection. Persistence owns authenticated
private objects, operation/attempt acceptance and durable identity. A shared pure
core normalization/fold accepts validated seed data supplied through persistence
read capabilities; persistence never depends on orchestrator. Git owns original
worktree authentication. Daemon coordinates these owners and normal dispatch.
Desktop/reports consume read observations, never launch capabilities.

## Mandatory reuse survey

| Existing seam | Reuse and limitation |
|---|---|
| `surge-core/src/work_item.rs:89-237` | Accepted revision, attempt binding/generation, retained workspace and lifecycle; no synthetic legacy attempt. |
| `surge-core/src/work_item/origin.rs:25-160` | Exact accepted graph and raw prompt, validating construction and domain-separated identity. |
| `surge-persistence/src/work_items/owned_flow/preparation.rs:18-54, 211-418` | Provisional operation, stable identities, frozen inputs, retained launch guard and short atomic acceptance. Extend this owner rather than duplicate its FSM. |
| `surge-daemon/src/owned_flows.rs:29-119` | Blocking preparation worker retains capabilities; cancellation before acceptance abandons; accepted launch reconciles. |
| `surge-git/src/task_workspace.rs:59-80` | Reconcile the original registered owner using AfterExecution, preserving dirty/untracked files; existing plan_at_path does not authorize adoption. |
| `engine/execution_state.rs:76-119, 235` | Derive graph, cursor, frames and memory from a bounded authenticated source prefix; consumed route requires matching checkpoint. |
| `engine/snapshot.rs:13-49, 125-182` | Reuse versioned frame representation and validation; do not serialize raw RunMemory. |
| `engine/admission/startup.rs:56-81` and `engine/work_items.rs:47-145` | One real successor RunStarted, binding, graph and frozen-input manifest with strict cold validation. |
| `surge-daemon/src/bootstrap_continuation.rs:109-140` | Reload the approved handoff by identity instead of rereading mutable flow.toml. |
| `surge-core/src/task_execution.rs:16-26` | Reuse TaskOccurrenceId for source task occurrence identity. |
| `owned_flow/private_inputs.rs:16-103` and `private_files.rs` | Retained private namespace, established host key and domain-separated authentication; public manifest alone conveys no capability. |
| `engine/writer_coverage.rs:131-223`, `surge-acp/src/process_evidence.rs:64-146` | Existing precise process/container probes and ADR-0021 cleanup distinctions; no stronger containment claim. |

New types are limited to concepts not represented by these primitives: source
event position, validated seed manifest, semantic seed, typed continuation
refusals and its authenticated host capability. Reuse existing errors/receipts
where they convey the same meaning; do not wrap every reused type in a new API.

## Host operation and durable association

Conceptual request:
`ContinueBootstrapImplementation { operation, bootstrap, expected_revision, expected_failed_run }`.
Client inputs contain no path, graph, prefix, cursor, private input or authority.
Normalize and bind exact request identity before source/configuration I/O.

The registry stores an immutable bridge association linking source Bootstrap,
source implementation, source workspace owner, successor item/run/binding,
source terminal and selected prefix identities, seed manifest and launch receipt.
The original implementation is a unique continuation source. Exact replay returns
the original operation/receipt; a different body or competing operation refuses.
Abandoned preparation is recoverable by its original operation without allocating
a second item or successor. Reuse existing provisional and accepted owner states.

The old operation and run remain Failed. Their terminal claim, accepted prefix
and selected failure-tail facts are immutable; the journal need not forever stop
growing. A later harmless ForkCreated lineage append does not invalidate captured
facts. They are linked source history, not a fabricated WorkItem ordinal-zero
attempt. Further attempts use the same normal WorkItem owner and workspace.

## CONT-ARCH-01: seed application once and local checkpoint precedence

One normalization rule is shared by engine load, load_memory, initial start,
inspection/materialized projections and cold read/recovery. Decode/authenticate
one seed manifest against the successor's unique startup binding. Structural
validation of a deserialized seed is not host launch authorization.

The pure normalization receives authenticated seed data plus local envelopes;
it does not open private files itself. Inspection supplies a read capability;
execution admission supplies the additional real launch/workspace capabilities.
Normalization validates the seed appears exactly once in the allowed startup
position before any local executable occurrence. A report missing read access
shows seed/history unavailable and cannot fall back to an apparently empty fresh
run. Existing non-seeded runs keep their current pure folding path.

The memory fold initializes only permitted semantic fields from that seed once,
then folds successor-local events in sequence order. A local checkpoint does not
reinitialize memory. Loading from the beginning or incrementally must produce
the same result; the seed cannot be applied again over a previously folded tail.
The seed-binding event is an initialization marker, not a repeatable mutation.
Duplicate, late, mismatched or missing required seed binding refuses. Successful
read/cold inspection cannot manufacture a consumed route or stage occurrence.

Cursor/frame selection is separate from memory initialization. Before the first
successor-local checkpoint, use seed cursor/frames/counters. Once a validated
local checkpoint exists at or before the requested position, use that local
checkpoint exclusively. All local consumed routes require their matching local
checkpoint under the existing rule. A missing/contradictory local checkpoint
refuses rather than falling back to the older seed. No source route or sequence
is inserted into the successor's consumed-route, pending-stage or stage-occurrence
sets. Snapshot memory-applied positions are successor-local; the seed has its own
qualified source position, never an artificial successor seq87.

For read-only normalization persistence returns an authenticated read capability
containing validated pure seed data. Engine launch additionally requires the
matching WorkItemLaunchClaim and authenticated execution capability. The read
capability cannot start a run or pass effect fences. Core pure fold/report code
consumes data, not I/O/launch authority. Do not introduce separate report and
runtime import algorithms, or an orchestrator dependency in persistence.

Required regression: successor performs a backtrack, a new TaskSplit, ledger
changes and another accepted route, then closes/reopens twice. Both memory and
cursor/frames retain that local progress and newly spliced order; they never reset
to the inherited boundary. Check load and load_memory at positions before/after
the first local checkpoint, cold inspection and live execution through this same
normalization seam. Oracles compare explicit task/order/cursor facts, not another
implementation of the folding algorithm.

## Semantic seed: exact field policy

The private seed is schema-versioned, bounded and validated against source facts.
Its public manifest binds the real successor operation, request identity,
binding, run, original workspace owner, source provenance, version and opaque
private object reference. The accepted Flow contract pins graph and raw prompt.

| State | Carry policy |
|---|---|
| Graph | Exact approved effective graph, matching accepted contract. Pending or partially applied revisions refuse in v1. |
| Cursor | Next cursor/attempt from the last fully consumed route with matching checkpoint. No graph-start fallback after consumption. |
| Frames | Complete loop/subgraph stack, resolved items and bindings, indexes, retries, return nodes and configurations. Validate all references against graph. |
| Traversal bounds | Preserve root/frame traversal counts, node visits and entered-via edges. No retry-budget reset. |
| TaskSplit | Preserve already-spliced frame lists plus source-qualified split/discovery lineage and child order. Never splice again on import/replay. |
| Artifacts | Verified retained bytes, logical name, producer and qualified production position. Retain required closure in successor-owned storage before acceptance. |
| Outcomes | Accepted-and-routed semantic outcome/summary history needed by branches/feedback, qualified by source. Exclude rejected candidates and unconsumed commits. |
| Feedback | Preserve backtrack/escalation semantics and their source positions/artifacts. |
| Requirements and tasks | Accepted requirement revisions, ordered task identities and permitted unfinished ledger states, with source provenance. |
| Verification | Import no current subject, criterion epoch, positive claim or verification authority. Preserve old evidence only as source history. |
| Runtime authority | Exclude provider sessions, writers, establishment intents, stage occurrences, pending commits/gates, MCP calls, control/suspension generations, quota handoffs/plans and active runtime rotations. |
| Spend | Exclude source costs and warn/exceeded flags from successor memory. Account source usage separately. |
| Bootstrap state | Exclude planning approval/edit FSM, edit counters and bootstrap-only control state. |
| Recovery observer | Source history only, never successor recovery observations. |

An inherited semantic fact does not authorize a tool call, route consumption,
provider continuation or old human decision. Prefix selection must refuse a
pending stage/commit, unresolved human decision, unsupported active rotation,
pending graph revision, ambiguous task scope or uncertain consumed route.

## CONT-ARCH-02: exhaustive RunMemory policy

This table covers every current RunMemory field at run_state.rs:279-386.
Carry means an allowlisted semantic seed field with qualified provenance; fresh
means supplied by successor startup or its local tail, never copied. History
means available only through linked source observations. Refuse applies to a
source condition that cannot cross the chosen boundary safely. Adding a field
requires an explicit policy and an exhaustive constructor/destructure test; no
wildcard defaulting may silently import or discard a newly added field.

| RunMemory field | Exact policy |
|---|---|
| unresolved_mcp_calls | History only; unresolved call/effect after boundary refuses where ordinary admission cannot safely cross. Never replay. |
| execution_writers | History and source reconciliation only; fresh successor records. |
| provider_sessions | History only; fresh provider sessions. |
| session_establishments | History only; unmatched live/uncertain establishment refuses; successor set starts empty. |
| stage_occurrences | Fresh empty; populated only by successor StageEntered. |
| quota_plans | Fresh empty; pending active source plan requiring consumption refuses. |
| gate_recovery_error | Source Some refuses; successor None until a local error. |
| gate_decisions | History only; unresolved/unconsumed source decision refuses; successor empty. |
| committed_stage_outcomes | History only; source consumed commitments must be authenticated; any pending commitment refuses. No source consumption IDs imported. |
| committed_gate_stages | History only; unconsumed effect refuses; successor empty. |
| suspension | Fresh None; active source suspension/control conflict refuses. |
| control_generation | Fresh from current accepted task/control owner; never source generation. |
| bootstrap_edit_loop_cap | Fresh frozen successor configuration, not legacy planning authority. |
| artifacts | Carry retained allowlisted references with qualified production position. |
| artifacts_by_node | Carry same artifact objects/indexing, preserving stable within-node order. |
| outcomes | Carry accepted/routed semantics and qualified positions; never source commit authority. |
| costs | Fresh zero/unavailable state as appropriate; source spend separate. |
| budget_warning_raised | Fresh false, then local events. |
| budget_exceeded_noted | Fresh false, then local events; remaining allowance frozen separately. |
| bootstrap_edit_counts | History only; successor empty. |
| node_visits | Carry semantic traversal counters. |
| backtrack_feedback | Carry semantic references with qualified positions. |
| escalations | Carry semantic references with qualified positions. |
| entered_via | Carry graph-validated latest semantic incoming edges. |
| requirement_revisions | Carry ordered accepted texts/scope with qualified source positions. |
| runtime_rotations | Active source rotation refuses in v1; successor empty/new frozen policy. |
| last_edit_feedback_by_stage | Bootstrap history only; successor empty. |
| roadmap_patches | Complete settled amendment history only with qualified metadata; successor runtime map empty until local amendments. Unsettled lifecycle or missing source closure refuses. |
| latest_graph_revision | History only with qualified metadata; successor None. Effective accepted graph is successor initial graph. |
| ledger | Carry permitted unfinished/discovered task semantics and source lineage; positive verification/human completion refuses; local update index points to actual seed binding. |
| verification | Fresh empty context, then own startup subject and fresh accepted criteria; no old epoch/claim. |
| verification_graph | Fresh accepted effective successor graph. |
| work_item | Fresh current accepted WorkItemContext from real binding. |
| recovery | Fresh successor observer; source observations stay linked history. |

For settled amendments, authenticate the complete patch-to-artifact-to-effective
graph history before freezing the successor initial graph. Preserve that history
in source-qualified observations; do not replay its acceptance events into the
successor. Source applied_graph_revision_seq and processed_graph_revision_seq are
not carried. Successor applied/processed counters start at zero and thereafter
refer only to local GraphRevisionAccepted events/checkpoints. Pending/partially
applied revisions or incomplete amendment artifacts refuse explicitly.

## Qualified ordering and consumer compatibility

Introduce validated `SourceEventRef { run, seq }` with nonzero seq. Reuse
TaskOccurrenceId for task identities. Never treat an old bare seq as successor
local. `RunMemory` itself is not the serializable seed format.

The ordering-bearing fields on ArtifactRef, OutcomeRecord and BacktrackFeedback
need an explicit position representation: current event position or inherited
source position authenticated by this seed. Inherited records preserve source
order and precede successor events. Ordering is valid only in this lineage;
foreign inherited runs or unknown seed identities refuse. Do not implement an
unrestricted global Ord. Source artifact events within the implementation prefix
provide the ordering positions even when their bytes originated in planning.

The sequence-bearing closure also includes LedgerTask.updated_seq
(run_state.rs:503), RequirementRevision.seq (:548),
RoadmapPatchMemory.updated_seq (:579) and GraphRevisionMemory.updated_seq (:589).
Requirements use qualified positions for inherited ordering/audit. Carried ledger
rows index their actual successor seed-binding event and retain a separate
SourceEventRef, never put old seq into EventSeq. Source amendment metadata remains
qualified history; local amendment metadata uses local sequences only. Update
SQLite reader_views.rs:183-202/236-281, ledger/index writers, run_report rendering,
task projections, requirement prompt consumers and serializers accordingly. A
local-only EventSeq interface must expose inherited provenance separately or
return a typed unavailable view, not coerce a source sequence into EventSeq.

This closure includes `stage/agent.rs:153-170` feedback selection and
`run_task.rs:2672-2690` split artifact stage-entry comparisons, all artifact/node
output consumers, serialized BootstrapContinuation artifact adapters, snapshot
compatibility, fixtures and callers. Simple successor materialized indexes may
reference the actual seed-binding event while retaining source provenance
separately. Existing legacy bare-sequence data must decode as local to its own
original journal, never be retroactively assigned successor authority.

## Verification policy and explicit intermediate limitation

V1 permits unfinished states Pending, Running, ReadyForVerification and
FailedVerification. It permits advancing past a split-replaced parent when
authenticated split lineage establishes that replacement and retained children.
It refuses non-replaced completed task iterations, completed ledger entries,
imported TaskVerified authority and human-accepted completion with a typed reason
such as CompletedWorkRequiresRevalidationPlan. A frame advanced past an item
without authenticated replacement or supported task scope also refuses.

This is deliberate: `stage/loop_stage.rs:243-429` advances using frame indexes
and outcomes, not ledger verification. Clearing verified cannot make already
traversed work reverify. Do not silently reset completed tasks, retain stale
positive proof, or skip them while displaying current completion.

The actual source boundary87 remains eligible: both indexes are zero; no task
iteration or task verification completed. Resume app_task with the full original
graph and both frames. Keep current dirty/untracked worktree files, accepted Build
semantics and m1-t1 unfinished/unverified. Rejected AppTester candidate109 is not
accepted evidence. Preserve complete failed tail through117 as source history.

Successor startup records its own freshly captured checkpoint/subject using
existing fingerprint/checkpoint machinery, then normal stage entry accepts fresh
criteria. `stage/verification.rs:96-173` needs a remembered subject for non-verifier
guidance; neither importing an old criterion epoch nor leaving all observations
absent is correct. This is a current input observation, never a passing receipt.
No verifier acceptance, scheduler or sandbox policy is weakened.

Broader continuation after completed work remains OPEN and is required before
claiming general project continuation from any stage. It needs an explicit
revalidation plan connecting historical completion, changed code, loop progress
and new proof. The v1 refusal is not a substitute for that remaining work or a
reason to retire any full-goal requirement.

## Private payload and current runtime configuration

Frame bindings and artifacts can contain private content. Seed payload is always
in retained private objects, not public registry JSON. Reuse the existing private
object owner and host-key authentication with a separate seed domain. Public
metadata exposes only safe identities and opaque references. Missing/replaced key
or object refuses; never recreate a lost established key to accept old payload.

Hydration returns a nonserializable AuthenticatedExecutionSeed host capability,
passed through a serde-skipped engine field. Validate binding/run/workspace at
runtime use. Successor frozen MCP objects are newly bound using owned-Flow input
preparation; do not transplant a source run's manifest or raw private blob.

## CSEC-2: private seed establishment without MCP

A continuation always requires a private preparation capability even with empty
MCP selection. Its exact request identity uses the existing HMAC form; private
requirement is not inferred solely from explicit MCP entries. Keep MCP selection
semantics separate from private seed storage. Retain a durable private-required
seed reservation before returning a preparation identity, through provisional,
frozen, abandoned and accepted states.

Extend private_key_creation_allowed (currently preparation.rs:204-208) and the
registry integrity checks to account for all those seed reservations/references
as well as HMAC requests and MCP manifests. No established private dependency can
become eligible for new key creation merely because no private MCP manifest
exists. Count provisional seed requirements even before its final manifest exists.

Persist a write-once private-namespace establishment identity in the trusted
registry, using actual retained namespace identity and established key identity.
PrivateFiles supplies descriptor-checked identity from its existing platform
owner; do not invent path-string authentication. Existing-only reopen compares
that identity before replay, seed publication or hydration. Once established,
missing namespace, replaced directory, missing/replaced key or marker yields
PrivateInputsRecoveryRequired/Corrupt and never creation/rekey. This must survive
deletion/replacement of the ENTIRE private-flow-inputs directory, including both
key and marker, across restart. The establishment dependency cannot live only
inside the directory whose loss it must detect. First-time creation remains under
the existing exclusive private capability; publish establishment before exposing
a preparation identity or seed reference. On ambiguous first publication fail
closed and reconcile, not mint another accepted identity.

The seed envelope authenticates domain/version, operation/request identity,
successor WorkItem binding/run, original workspace owner/association, source
provenance/prefix identity and opaque object reference together. A valid object
from another run, operation, binding, source or namespace cannot be transplanted.
The existing MCP envelope does not by itself authenticate a new seed object.

The fresh attempt can use the current corrected runtime/profile implementation;
effective configuration is validated and frozen before acceptance and reused
after restart. Exact graph and explicit codex-acp/gpt-6.1-sol declarations remain
unchanged. Never reread mutable source configuration on accepted replay.

## Ownership transfer, atomicity and cancellation

Use a fixed acquisition order: bridge/source reservation, source opening
protection, original workspace authentication, preparation/private objects,
successor launch owner. Keep capabilities in the preparation worker through real
external work. Revalidate identities before acceptance. A registry uniqueness
constraint prevents another bridge owning the same original implementation.

## CSEC-1: shared cross-run workspace exclusion and deletion fences

An operation/item lock alone is insufficient. Admission currently checks the
requested RunId and worktree existence (admission/mod.rs:450-520), so an unrelated
fresh run ID could execute the transferred workspace. Add one shared workspace
exclusion owner used by every execution admission, not a continuation-only check.
Its identity is authenticated original Git common directory, registered worktree
owner and actual retained workspace identity; canonical path alone is insufficient.
Use the existing descriptor-checked lock owner for the new workspace key rather
than another locking implementation.

Fresh unclaimed execution acquires/checks the same exclusion before source/config
effects. A workspace associated with a provisional or accepted WorkItem requires
that exact WorkItem's current generation and real matching launch claim; inventing
a new RunId, symlink spelling, generic StartRun or generic recovery cannot bypass
it. Unmanaged execution holds a transient workspace exclusion throughout actual
execution so bridge preparation cannot race an existing ordinary writer.
Claimed start/resume/recovery validate the durable workspace association and
generation under the same exclusion. Keep this owner through engine execution
and actual owner settlement, not just startup publication.

The bridge holds source/workspace exclusion while publishing provisional intent,
freezing state and atomically accepting. Transfer that SAME retained owner into
the accepted launch claim and engine; no release/reacquire gap. Cold reconciliation
reacquires the original recorded exclusion identity, authenticates the durable
association and current generation, and refuses substitutions. Cancellation keeps
ownership through real blocking worker/engine settlement; an accepted association
continues to exclude arbitrary fresh execution even while no process is running.

This shared rule changes fresh/claimed/nonterminal recovery admissions. Generic
terminal Resume already returns its existing terminal result (:705-749); do not
describe it as dispatching a terminal run, and do not reject harmless terminal
inspection solely to implement workspace exclusion. Generic fork into a distinct
authenticated workspace remains allowed. A harmless ForkCreated append on the
source journal remains allowed if captured terminal/prefix/tail facts are intact.

Pin required source history and preparation/private objects in the registry for
provisional, frozen and accepted continuation associations. Storage::delete_run
(storage.rs:582-610), deletion tombstones, artifact cleanup and private-object
cleanup must consult these dependencies under the same transactional authority
used to create the pin. Creating a pin cannot race an already-admitted deletion;
deletion cannot remove required source/seed objects after a pin commits. Reject
referenced deletion explicitly. Canceled/abandoned pins may be released only when
no worker/accepted lineage still requires them, by the owning protocol, never an
unscoped cleanup scan. Ordinary source lineage appends do not delete or rewrite
the pinned facts. Durable history dependencies remain separate from executable
capabilities; read-only inspection gains neither workspace nor launch ownership.

Authenticate source writers from the complete terminal history, not merely the
chosen executable prefix. Refuse active execution, conflicting identities,
failed/unknown observations of a required known identity or recorded group, and
unresolved post-boundary effects that ordinary continuation cannot safely cross.
Reuse ordinary admission's retained original opening/workspace ownership,
identity probes and MCP cleanup contract. GroupOnly is not itself a refusal:
an authentically observed empty group may admit ordinary continuation under the
existing best-effort policy, without claiming escaped descendants are covered.
Do not substitute passing-verification-receipt containment requirements for
ordinary execution admission. SessionClosed, a lock or a host-settlement opening
record alone is not writer absence evidence. No new sandbox or OS containment
mechanism is proposed.

The actual source records provider containers at SessionOpened16/51/94, closes
39/84/115 and pre-boundary hook writer intents/containers. Validate their actual
retained identities. Never infer absence from PID alone or signal unrelated
processes. Historical unknown external effects remain visible and are never
replayed through inherited session/tool authority.

The root's actual fixture inspection recorded sixteen known process identities
absent and a scoped open-file check empty. Those observations support the
documented ordinary policy when revalidated under the retained source opening;
they are not a global process inventory or proof of full descendant closure.
Known original ACP groups on macOS are GroupOnly. Preserve that coverage label
and residual uncertainty in provenance/diagnostics. Typed refusal is for a live
known owner/group, identity substitution/conflict, a failed required probe or an
unsupported pending effect, not for the inherent residual risk of GroupOnly.
Thus this fixture can admit if its exact identities, empty groups, original Git
owner and stable source facts pass the existing ordinary reconciliation policy.
No user permission can manufacture missing identity evidence, and successful
continuation cannot upgrade it into a passing verification receipt.

Construct WorkItemWorkspace from authenticated original Bootstrap capture and
Git registered owner/branch/common-directory/base; reconcile AfterExecution.
Do not reset HEAD/index/files or broaden ordinary Explicit workspace requests
into foreign-directory adoption. Preserve original owner across later attempts.

Atomic registry acceptance commits bridge mapping, item/revision, attempt,
workspace association, seed identity and receipt. Only afterward publish the
successor journal under the retained launch claim. Startup is one matching own
RunStarted/binding/graph/manifest/seed batch with strict cold validation. Accepted
but missing startup is recoverable; conflicting or duplicate seed/startup refuses.
Do not require a parent-journal back-reference transaction for correctness.

Cancellation before acceptance abandons preparation and preserves files/history.
After acceptance, use durable task cancellation/control semantics, including
accepted-but-not-started. Worker cancellation does not release ownership before
the actual worker exits. Cold replay uses successor-retained objects and artifacts
without mutable source rereads. Captured source terminal, accepted-prefix and
selected failure-tail facts stay immutable; harmless later lineage appends are
permitted and never consumed as successor execution facts.

## Accounting and native history

Successor usage starts empty and uses existing per-attempt sequence accounting
(`work_items.rs:1213-1341`). Source observations are never copied into successor
events. Retain immutable external lineage usage separately, deduplicated by source
run and terminal/prefix identity; display it separately from real WorkItem
attempts. A finite inherited operation budget subtracts authenticated prior spend
once; unknown evidence cannot become a fabricated remaining allowance.

Report/desktop join source TaskExecutionView under original run/occurrence IDs.
Planning and Build show inherited evidence with source navigation. Successor
AppTester gets a new stage/session occurrence and fresh provider invocation.
Preserve split lineage/order and the source failure; do not synthesize successor
StageCompleted, TaskVerified, SessionOpened or usage events for inherited history.

## Complete implementation closure and gates

Expected closure, not permission to implement:

- Core: pure source/seed-memory/manifest contracts, shared serializable frame
  records, qualified positions, event schema/migrations, common one-time seed
  normalization/fold and shared task/report observations.
- Orchestrator: executable source-capture/seed validation; execution_state, snapshot,
  admission/startup, work_items and config; all ordered artifact/outcome/feedback
  consumers, local-checkpoint precedence and Bootstrap handoff validation.
  Existing snapshot wire records are moved/re-exported, not duplicated. Generic
  fork into a separate authenticated workspace stays intact.
- Persistence: bridge association schema/immutable triggers; owned-Flow
  preparation/private objects/launch/reconciliation; authenticated hydration;
  read-vs-execution capability distinction; shared workspace exclusion and
  required-history/object deletion pins; private namespace establishment/rekey
  fences; run projection/inspection compatibility and separate lineage usage.
- Git: original Bootstrap workspace authentication/transfer through retained
  ownership, preserving current files and refusing arbitrary adoption.
- Daemon: host operation, preparation ownership, cancellation/reconciliation,
  IPC request/receipt routing and ordinary task launch.
- Desktop/report: failed Bootstrap action, immutable retry body, successor
  navigation, source-qualified inherited history and truthful refusal state.
- Compatibility: BootstrapContinuation serialized artifact refs, event versions,
  snapshots, existing report/task fixtures and affected transport callers.

Independent critic and security pre-code reviews must accept this contract before
implementation dispatch. Then record implementation acceptance, real RED before
behavior edits, stage-sized local commits and exact verification outputs. Full
mode applies to the cross-crate trust/ownership change. No waiver based on local
targeted tests; source-bound platform CI and native acceptance remain required.

Required outer tests, not a second implementation-shaped oracle:

1. Actual-shaped completed planning/failed implementation resumes AppTester,
   does not rerun spec/Build, retains eight original tasks and dirty/untracked
   bytes, with failed source and new successor history both reachable.
2. Committed TaskSplit children and occurrence/source identities retain exact
   fixture order once on import and across two cold restarts; no split RPC replay.
3. Completed/verified/human-accepted unsupported work refuses explicitly without
   dispatch or source mutation. Split replacement is separately allowed.
4. Crash before acceptance, after acceptance, during startup and after first
   successor route. Recovery creates no duplicate item/run/seed or tool effect.
5. Two operations race for one source; exact replay and changed-body conflict;
   replaced operation/launch/source ownership objects and lost source workspace.
6. Cancellation at each preparation/acceptance/dispatch transition, including
   a blocking worker and accepted-but-not-started attempt.
7. Source journal/Git/worktree substitution, stale graph/artifact/seed hashes,
   missing private object/key, forged manifests and foreign qualified positions.
8. Live/unknown/conflicting writer identities and unresolved post-boundary
   effects; best-effort group cleanup never becomes confirmed closure.
9. Repeated import/reconciliation/cold inspection charges source and successor
   observations once, preserving unknown usage and finite budget exhaustion.
10. Native action and cold desktop history show source failure, inherited Build,
    new AppTester criteria/session and remaining original milestones accurately.
11. Seed once: before/after first local checkpoint, successor backtrack, new
    TaskSplit and ledger updates survive two cold restarts and load/load_memory/
    inspection agreement. Missing/contradictory local consumed-route checkpoint
    refuses instead of restoring the seed. Duplicate/late seed refuses.
12. Exhaustive field and sequence compatibility: reject newly unclassified
    RunMemory fields; inherited requirement/ledger/amendment positions are never
    local EventSeq. Settled amendment graph becomes successor initial graph with
    zero applied/processed source counters; incomplete/pending amendment refuses.
13. Race bridge preparation/acceptance against arbitrary fresh new-RunId execution
    of the SAME workspace through direct engine and daemon surfaces. Exactly one
    owns execution. Cold restart, alias paths, cancellation and delayed worker
    settlement cannot open a gap. Different isolated fork workspace remains valid;
    terminal read/settlement and harmless source ForkCreated append remain valid.
14. Race source/seed deletion against provisional pin creation and acceptance;
    referenced history/private objects survive and deletion refuses. Corrupt pin,
    preexisting tombstone and replacement exclusion identity refuse safely.
15. Empty-MCP seed: first preparation requires private ownership; whole established
    namespace removed/replaced after provisional freeze and after acceptance,
    followed by cold restart and new/old operation requests, must refuse without
    rekey. Test missing key+marker together, foreign valid namespace and transplanted
    seed envelope, not only missing single files. Existing legacy no-seed flows
    retain their documented key-creation behavior.

## Review finding dispositions in this revision

CONT-ARCH-01: addressed by one shared pure seed normalization/fold, authenticated
read/execute separation, one-time semantic initialization, strict local-tail
folding and first-local-checkpoint precedence. New tests 11 enforce progress
after seed, not merely restart at the inherited boundary.

CONT-ARCH-02: addressed by the exhaustive current RunMemory table, full listed
sequence consumers and explicit amendment history policy. Old graph counters
never enter successor counter space. New test 12 checks structural coverage and
backward compatibility.

CSEC-1: addressed by a shared cross-run workspace owner in ALL execution admission
paths, durable original-workspace association, generation/claim matching,
unbroken preparation-to-execution ownership and required-source/object deletion
pins. Tests 13-14 include bypass/race, cold recovery, cancellation and allowed
harmless lineage/terminal operations. This is not an extra OS sandbox.

CSEC-2: addressed by private-required continuation identity/capability even with
empty MCP, all-stage seed dependency accounting, trusted-registry namespace/key
establishment outside the removable private namespace, full envelope binding and
existing-only reopen. Test 15 requires whole-namespace loss/replacement refusal
without rekey. Existing authentication primitives remain the owner.

These are proposed dispositions pending focused independent re-review, not claims
that code/tests already implement them. Feature pre-code count stays 0/3.

General completed-work revalidation, any other typed unsupported continuation
state, full app/follow-up/existing-project acceptance and Windows/current-source
gates stay open until independently satisfied. Passing this feature's tests does
not close the full project goal.

## Independent pre-code closure (implementation not yet dispatched)

Initial82d689ba review was RESHAPE NEEDED for CONT-ARCH-01 (seed-once fold/checkpoint precedence), CONT-ARCH-02 (complete field/sequence/amendment policy), CSEC-1 (all-run workspace exclusion/deletion pins), CSEC-2 (empty-MCP private seed/established-key namespace). Revised bodyf4cb0605fc46606d43c1d296cae75beb69cd618f2ede886c4d7eafb0c0db84c9 passed focused continuation_plan_critic and continuation_plan_security: ACCEPTABLE. All four addressed on the plan; execution tests have not proved them yet.

Binding implementation conditions: one compatible nonblocking lock acquisition order, never block on another owner while holding a registry transaction; terminal/prefix/tail facts independently validated despite later harmless lineage appends; actual adversarial whole-key-namespace replacement, fresh-admission/deletion races and delayed-worker cancellation tests; complete shared fold/read/execute/admission/deletion consumer closure. No partial interface migration. Feature review repair count remains0/3; no Rust author dispatch. Existing user broad goal authorizes internal project-continuation development within these preserved semantics; no new publication authority. Witness4/4 author currently owns Cargo and all Rust writes; continuation and Results code wait for its terminal freeze/lease release. Full generic completed-work continuation and native original app/follow-up/existing-project/platform requirements remain open.

## Sequential delivery plan (documentation only)

This appendix schedules the accepted contract above; it does not revise it or
claim any implementation/test has run. The focused critic/security acceptance
of body `f4cb0605fc46606d43c1d296cae75beb69cd618f2ede886c4d7eafb0c0db84c9`
remains the design prerequisite. Status of every stage below: NOT STARTED.
No Rust author is dispatched by this document. Feature repair count remains
**0/3 shared across every stage, Phase 5a and Phase 5b**; a stage name, commit,
worker replacement or new context cannot reset it. Findings requiring authorized
repair consume that same counter. Initial planned implementation is not itself a
repair dispatch. Each stage must receive independent spec compliance before its
full quality/domain/security lenses; corrective work rechecks affected gates.

### Execution discipline and evidence ownership

One source author owns Rust writes and Cargo at a time. The witness fourth-round
author currently owns both: wait for its terminal freeze, record its actual HEAD
and scoped diff, and obtain the released lease before stage 0. Do not reset its
files, borrow its test target concurrently, or infer readiness from a status file.
The approved Results third repair is separately queued at existing count 2/3;
it must retain its own accounting and source ownership. Sequence that repair
before a conflicting continuation stage, or explicitly schedule it afterward;
never include its fixes under the new feature's counter. Recheck overlapping
projection tests after both changes.

For each implementation stage, record before/after source identity, complete
file closure, authentic RED for changed behavior, commands with exit statuses,
independent verdicts and unresolved outer requirements in this same task. Commit
a reviewable complete dependency closure locally when its own gates pass. An
internal stage checkpoint is not feature completion: the top-level outer test
may remain deliberately red while prerequisites are being built. Record that
fact; do not ignore, delete, loosen or relabel the acceptance test to create a
green workspace claim. No publication/merge authorization follows from local
commits. Any contract reshape returns to pre-code review before authoring.

New fixtures use existing retained runtime-home/process settlement support and
actual storage/daemon/engine boundaries. Extend an existing fixture where it
represents the condition; create only a bounded test fixture when its shape
cannot express the failed implementation. Do not add a second controller, FSM,
seed importer, storage database or production wrapper for test convenience.
Fixture cleanup must settle actual tasks, pools and owners before removal;
assertion failures must retain diagnostics without leaking provider processes.

### Stage 0 — executable outer RED before foundation edits

**Owner/write zone:** daemon integration tests and their existing fixtures only;
no production code. Reuse `tests/owned_flow_ipc.rs` raw JSON socket request seam
and `tests/fixtures/bootstrap_gate_cold.rs` real host/storage/approved-handoff
fixture discipline. A synthetic runs-only host that cannot supervise Bootstrap
is not sufficient. Use a controlled ACP/mock script to produce completed planning,
accepted Build and terminal failed AppTester on the original two-milestone,
eight-task graph. Make dirty tracked and untracked sentinels before failure.
Assert those preconditions from persisted facts, original Git ownership and
actual settled writer evidence before the missing-action assertion.

Send the proposed continuation wire body through the running real daemon's
existing line-framed local socket, using `serde_json::Value`, not a future enum
variant. First assert Ping/BootstrapStatus readiness and exact source revision,
planning Completed and implementation Failed. Then send the continuation request
and assert the intended accepted receipt/new WorkItem association. Today an
unknown request fails `read_request_frame` and closes that connection
(`server.rs:305-371`); represent actual EOF as an observed response value so the
assertion reports missing continuation. Check daemon still answers Ping on a new
connection and both source journals/files are unchanged. A decode/compile error,
readiness timeout, fixture panic, missing binary or unobserved hung connection is
SETUP FAILURE, not RED. Capture the assertion and current source SHA before any
foundation production edits. Do not first add the production endpoint to obtain
a convenient red test.

The same outer scenario must later assert unchanged Build invocation count one,
AppTester reinvocation with a new session/current criteria, all eight original
task identities/order, both milestones completed through the real graph, source
failure retained and both histories inspectable. Before implementation it fails
at the first missing accepted receipt; later assertions remain the unchanged
end-to-end oracle. Keep an explicit fixed expected task/order list and provider
invocation log, not a second algorithm that computes expected seeded state.
Native action/rendering assertions are added at stage 6, without replacing this
backend anchor. Scripted ACP behavior proves protocol mechanics only; real
codex-acp/gpt-6.1-sol app acceptance remains stage 7/full-goal work.

**Exit:** authentic outer assertion RED, source/fixture preconditions passed,
settlement evidence retained. This is readiness to implement, not a green stage
or feature closure. Record the eventual exact test name/command, rather than
claiming a proposed test already exists.

### Stage 1 — shared wire records and qualified position migration

**Depends on:** stage 0 RED. **Owners:** core contracts; orchestrator conversion;
persistence readers/materializations; report/UI/IPC consumers of changed types.
Move/re-export existing serializable frame records into core with existing wire
compatibility. Add validating source/seed/position contracts and startup seed
binding event schema needed by the accepted design; do not expose an executable
seed constructor to clients. Migrate the complete qualified-ordering closure in
one stage: ArtifactRef, OutcomeRecord, BacktrackFeedback, requirement/ledger/
amendment fields, snapshot adapters, BootstrapContinuation serialization,
feedback selection, split stage-entry artifact comparisons, reader_views/index
writers, report/task projections and prompt consumers. Legacy sequences remain
local to their original run. Do not leave a temporary bare-seq shim, unrestricted
Ord, duplicate frame format or a consumer that casts inherited seq to EventSeq.
Source-only amendment history and actual successor seed-event indexes follow
the policies above. Pure no-seed folds remain behavior-compatible.

**Behavior tests before edits:** legacy serialized fixtures round-trip/localize;
foreign lineage and zero/ambiguous position refuse; explicit inherited/local
ordering selects the right feedback/artifact; new field coverage has exhaustive
construction/destructuring. Include default/all-feature consumers and external
serialization fixtures in the same closure. **Exit/gates:** affected core,
orchestrator, persistence, CLI and UI tests plus relevant compatibility/IPC tests;
strict affected default/all-feature Clippy and fmt. Outer continuation test is
still RED. Stage 1 does not activate seeded execution.

### Stage 2 — private seed owner, establishment and retained dependencies

**Depends on:** stage 1. **Owners:** persistence work_items/owned_flow, private
files/inputs, registry migrations/integrity, run deletion/artifact cleanup owners;
existing platform namespace owner supplies retained identities. Extend the
existing preparation protocol, not a parallel operation lifecycle. Add bridge
provisional dependency records and uniqueness/pins needed for atomic acceptance;
pin creation and deletion/tombstone checks share transactional authority. Keep
source/seed dependencies distinct from executable authority. Private-required
reservation is durable before exposing preparation identity even with empty MCP.
Publish write-once registry namespace/key establishment outside that removable
namespace, fail closed on ambiguous publication, extend all-stage rekey guards,
and bind full seed envelope to request, operation, run/binding, workspace/source,
version/domain/object identity. Expose authenticated read hydration separately
from the nonserializable launch-capable seed. Complete existing-only reopen and
all affected owned-Flow reconciliation/cleanup callers together.

**RED/GREEN:** no-MCP first establishment; loss/replacement of whole directory
with key and marker after provisional freeze and acceptance; old/new operation
requests after cold reopen; foreign valid namespace/object transplantation;
missing/corrupt key/object; pin-vs-deletion race, preexisting tombstone and abandoned
worker dependencies. Prove no rekey/publication and no referenced deletion on
refusal. Existing legacy no-seed/MCP tests retain their behavior. **Exit/gates:**
persistence preparation, launch, recovery, deletion and platform namespace suites,
strict default/all-feature Clippy/fmt and migration compatibility. No endpoint or
source-authority shortcut becomes reachable; outer remains RED.

### Stage 3 — original workspace authority across every execution path

**Depends on:** stage 2 durable association/pins. **Owners:** Git original-owner
reconciliation, existing descriptor-checked lock implementation, persistence
workspace association/launch claim, orchestrator ALL execution admission paths,
daemon fresh/claimed/recovery call sites. Establish one authenticated workspace
exclusion identity shared by unmanaged execution and provisional/accepted tasks.
Use nonblocking compatible acquisition order; never wait for an owner while
holding a registry transaction. Unmanaged execution retains it through actual
settlement. Bridge preparation transfers the same retained owner into accepted
launch/engine without release/reacquire. Claimed start/recovery checks exact
association and generation; cold reopening authenticates the recorded original
Git/common-directory/worktree identity. Preserve dirty/untracked files/index/HEAD.
Do not enable arbitrary Explicit adoption or use path canonicalization as proof.

**RED/GREEN:** direct engine and daemon fresh new-RunId bypass; alias/symlink path;
race ordinary writer against bridge preparation; stale generation/foreign claim;
workspace replacement; cancellation with delayed blocking worker; accepted but
idle association exclusion and restart reacquisition. Negative controls: distinct
isolated fork succeeds, harmless terminal inspection/settlement and source
ForkCreated append remain valid. **Exit/gates:** Git workspace tests, engine
admission/recovery tests and daemon launch/cancellation tests, relevant platform
lock/namespace tests and strict affected Clippy/fmt. No temporary continuation-
only exclusion; all execution callers migrate in this closure. Outer remains RED.

### Stage 4 — executable source capture and one-time seeded execution

**Depends on:** stages 1-3. **Owners:** orchestrator source capture, execution_state,
snapshot, memory load, startup/work_items/config; shared core normalization;
persistence authenticated read/inspection/materialized views. Capture approved
handoff and complete terminal writer/failure-tail facts under retained original
ownership, select consumed-prefix boundary87 with matching checkpoint, retain
artifact bytes in successor storage, validate frames/traversal counters and
exhaustive RunMemory policy. Enforce typed unsupported completed-work/pending
state refusals. Keep GroupOnly ordinary reconciliation semantics honest and
visible; no stronger receipt containment requirement or claim.

Implement the ONE pure normalization/fold used by initial start, engine load,
load_memory, cold inspection and projections. Read capability supplies validated
seed data; execution additionally requires real matching launch/workspace claims.
No-seed legacy path remains intact. Bind seed once in own startup; initialize
allowlisted semantic memory once, then fold local events only. Use seed cursor
before any local checkpoint and validated local cursor exclusively afterward.
Contradictory/missing consumed-route checkpoint, duplicate/late/mismatched seed
or unavailable read capability refuses explicitly. Capture a fresh successor
subject/checkpoint and fresh criteria through existing observation machinery;
never inherit verification/session/tool/commit/route authority or source spend.

**RED/GREEN:** engine/load/load_memory/inspection at explicit before/after local
checkpoint positions; backtrack then new split, ledger updates and accepted route
survive two real close/reopens without seed reset; exact fixed child/order facts;
no duplicate split import/RPC; absent/mismatched checkpoint and seed refusal;
exhaustive memory/sequence/amendment policy including initial graph counters zero;
source writer unknown/live/conflict refusal and authentic empty GroupOnly allowed
without upgraded coverage. **Exit/gates:** full affected core/orchestrator/
persistence suites plus report/inspection/legacy snapshot regressions, strict
Clippy/fmt. Engine-only seeded tests require actual host capabilities produced
by the owner, not fabricated public JSON authority. Outer endpoint remains RED.

### Stage 5 — atomic bridge, daemon operation and ordinary task launch

**Depends on:** stages 2-4 complete. **Owners:** existing owned-Flow preparation/
acceptance/launch protocol and schema, daemon owned_flows/work_items/bootstrap
handoff coordination, engine owned-flow/IPC client and server callers. Implement
one request with no client-supplied path/graph/cursor/seed. Bind request before
source/config effects; atomically accept mapping, item/revision, attempt/workspace,
seed and receipt; then publish one own startup batch under retained launch claim.
Use existing provisional/accepted recovery states. Reconcile accepted-missing-
startup and accepted-before-dispatch through normal WorkItem ownership. Original
Bootstrap/run stay Failed, source history is linked without synthetic attempt0.
Freeze corrected current profile/config and newly bound MCP objects once; replay
must not reread mutable source config. Preserve finite budget/prior unknown usage
rules and separate deduplicated source-history usage from fresh attempt usage.

**RED/GREEN:** stage 0 raw request now returns accepted receipt and unchanged full
outer scenario reaches all remaining-task/milestone assertions; same request
replays original identities, changed body/competing source refuse. Crash/cancel
before acceptance, after acceptance, during startup and after first local route;
actual delayed worker settlement, no duplicate item/run/seed/effect; post-capture
harmless source append allowed and captured fact substitution refused. Repeat
hydration/inspection/reconciliation without duplicated charges. Typed synthetic
runs-only refusal remains intentional. **Exit/gates:** daemon real IPC integration,
engine startup/launch/cold suites, registry transaction/race tests, all affected
transport callers, strict affected Clippy/fmt. Stage 5a requires the original
outer backend test GREEN. This establishes backend continuation, not native
usability or a completed real habit app.

### Stage 6 — native action and truthful joined history

**Depends on:** stage 5; compatible queued Results repair resolved or explicitly
integrated/retested under its own existing budget. **Owners:** UI task/project/run
coordinators and screens, shared core TaskExecutionView/report observations,
persistence inspection/lineage pagination and daemon query surfaces where needed.
Offer Continue implementation on eligible failed Bootstrap state, preserve an
immutable operation body on retries, navigate accepted WorkItem/successor, and
show typed refusals without silent restart. Display inherited Build/spec/source
failure under original run/occurrence IDs; successor AppTester/session/criteria
are new. Keep original planning history navigable and source usage separately
labelled. Missing authenticated seed-read access displays unavailable history,
never empty/fresh success. No UI-owned launch capability or second projection.

**RED/GREEN:** actual UI action/model-to-daemon invocation test before new action;
source Completed planning versus Failed implementation versus current successor
status; initial/replayed operation navigation; fixed task/split order; cold desktop
reload; unavailable/corrupt lineage and meaningful refusal. Recheck N3/N6/N7
existing observations; their Results corrections consume that task's budget,
not a renamed continuation stage. **Exit/gates:** native action/coordinator,
report/TaskExecutionView/inspection tests, strict affected Clippy/fmt; current
native screenshots/AX evidence proves action, lineage and status. Backend and
UI outer tests must both be GREEN. This still does not prove real app usefulness.

### Stage 7 — same project delivery, cold recovery and platform acceptance

**Depends on:** stages 0-6 gates/reviews; witness round 4/4 independently closed
and exact-model multi-turn observation verified before real-provider invocation.
Do not change the witness again under this feature's budget. Revalidate retained
source owners and provenance of the permanent habit-tracker fixture; preserve
its settled backup. Exercise the legitimate native continuation on THAT partial
workspace with approved original graph and codex-acp/exact gpt-6.1-sol. Prove
Build m1-t1 is inherited once, AppTester obtains a new session, remaining original
eight-task/two-milestone graph completes, useful app/meaningful tests pass,
original Failed history remains visible and follow-up retains history. Scripted
outer tests are not substitutes for these real provider/native/app observations.
Stop/cold-reopen at authenticated supported boundaries and verify no duplicate
split/tool dispatch, task loss, seed reset or accounting duplication.

Run full current-source project gates after closure, including workspace nextest,
doctests, strict all/default-feature Clippy/fmt and relevant ignored/native suites;
mirror platform exclusions from current ci.yml, do not infer Windows from macOS
or PR150. Platform CI requires separate publication authorization for this
independent branch; record Windows as pending until actual current-SHA evidence
exists. Retain exact command/output/source receipts and private settled full
runtime/project backup. Existing-project improvement and broader completed-work
continuation/revalidation remain explicit full-goal requirements if still open.

### Critical chain and completion boundaries

Critical chain: **released source/Cargo lease → outer behavioral RED → shared
wire/position closure → private establishment/pins → shared workspace owner →
seed capture/fold/recovery closure → atomic backend/outer GREEN → native joined
history → real same-project delivery/cold/platform acceptance**. Stages are
sequential because their authority/data owners are dependencies; no parallel
partial Rust interfaces. Read-only review may proceed without competing writes.

The next ready phase is stage 0 author/test dispatch after the actual lease
release and accepted delivery-plan handoff. No dispatch occurred here. Internal
foundations can close only their stated stage criteria; backend green closes the
continuation API behavior; native green closes the observable continuation flow;
real app/follow-up/current-platform evidence closes only its corresponding goal
requirements. V1 completed-work refusal remains truthful and tested but OPEN as
a limitation for general continuation. Never label the full project tool complete
while that requirement, existing-project acceptance, useful app delivery or
current Windows evidence remains missing.
