# Bootstrap-owned continuation — stage 1 pre-code plan

Status: PROPOSED revised plan; maintainer design verdict ACCEPTABLE, subject to
independent focused critic/security re-review and root handoff. V1 security
RESHAPE NEEDED (CONT-S1-SEC-01) is addressed below; critic was ACCEPTABLE with
three narrow watchpoints. This is a pre-code refinement, not a repair-count reset. This document authorizes no
implementation dispatch. Parent retains the accepted full design and stage order.

Baseline: `39673ff5172adbb391691d68487cf0352c060180`, plus the separately owned
stage 0 daemon-test fixture diff. Accepted full design body SHA:
`f4cb0605fc46606d43c1d296cae75beb69cd618f2ede886c4d7eafb0c0db84c9`.
Root reports authentic outer RED in
`.rust-studio/evidence/bootstrap-owned-continuation/stage0/outer-red.log`:
positive persisted preconditions/cleanup passed, missing continuation returned
EOF. This plan does not claim independently executed RED or tests.

Feature repair count remains 0/3, shared across all stages/review phases. Home10
is closed; its budget is not reset. Only this proposal was written here.

## Outcome and strict boundary

Stage 1 establishes ONE shared cursor/frame wire format and source-qualified
semantic positions, migrating all existing ordering/serialization/read consumers
in the same stage. It defines the public seed-startup marker, but produces no
private seed, accepted continuation, workspace claim, endpoint or executable
seeded run. The authentic daemon continuation test remains deliberately RED,
visible in receipts, never ignored or relabelled a passing feature test.

Stages 2–7 remain required: private establishment/pins, shared workspace exclusion,
authenticated capture/one-time fold/recovery, atomic bridge, native joined history,
real same-habit/all-eight/two-milestone/follow-up/exact Sol61/platform acceptance.
V1 completed-work refusal remains an intermediate limitation, not full completion.

## Source survey and minimal sibling reuse

Current execution/ownership seams are unchanged from the accepted design base.
Reuse core validating-serde patterns from `work_item/flow_inputs.rs` and existing
`BootstrapContinuation` raw/try_from decoding. Reuse `ContentHash`, `RunId`,
`WorkItemBinding`, `OwnedFlowObjectRef`, `HmacSha256Tag`, and TaskOccurrenceId;
do not create a new identity framework, crate, dependency or unsafe block.

Existing `engine/snapshot.rs` has SerializableCursor/Frame/LoopFrame/
SubgraphFrame/SubgraphInput and SnapshotError; EngineSnapshot itself includes
surge-git WorkspaceCheckpoint and must remain in orchestrator. `RunMemory` has
34 fields and currently stores local u64 sequence positions. Two real exceptions
must be preserved explicitly: `ArtifactStore::put` and fork artifact copying
construct seq0 placeholders before any production event; LedgerTask::default
also uses updated_seq0 for an untouched row. None is a valid SourceEventRef.

## A. Shared wire records: exact move, no second format

Create `surge-core/src/execution_recovery/snapshot.rs` and export it through the
existing execution_recovery module. Move SerializableCursor, SerializableFrame,
SerializableLoopFrame, SerializableSubgraphFrame, SerializableSubgraphInput and
SnapshotError there with unchanged names, fields, serde tags/defaults and wire
values. Move From<&Cursor>, SerializableCursor::into_cursor and the shared error
there. SnapshotError contains pure key/JSON/schema errors and needs no engine/Git
reference. EngineSnapshot, PendingHumanInputSnapshot, version dispatch and local
checkpoint counters stay in orchestrator. Existing snapshot module publicly
re-exports the moved names, preserving downstream import paths.

Frame runtime conversions and JSON/TOML conversion helpers stay in orchestrator:
From<local Frame> for core SerializableFrame and TryFrom<core SerializableFrame>
for local Frame remain legal because Frame is local. Do not move those impls into
core, which would reverse dependencies, or leave duplicate wire definitions.
Keep schema v1→v2 upgrade/default behavior and v2 JSON representation unchanged.
Wire records remain transport DTOs: decoding alone does not certify graph/frame
consistency or grant execution. Full graph-bound frame validation belongs to
stage 4, not a public executable constructor in stage 1.

## B. Qualified positions and exact compatibility policy

Add one core module `execution_recovery/semantic_position.rs`:

- `SourceEventRef`: private RunId/nonzero u64 fields, validating constructor,
  getters, Eq/Hash as needed; serde Raw+try_from, deny unknown fields.
- `ContinuationSeedId`: a distinct semantic manifest identity wrapping the
  existing ContentHash; a digest is correlation, never a hydration capability.
- `SemanticPosition`: private validated representation, Local(nonzero seq) or
  Inherited(seed ID, SourceEventRef). Legacy numeric JSON decodes as Local only;
  local values serialize numerically to preserve existing serialized handoffs.
  Inherited values use an explicit versioned/tagged object, never a bare number.
  Zero/negative/overflow/ambiguous representations reject. No Ord/PartialOrd,
  Deref, blanket u64 conversion, implicit source→EventSeq cast or magic offset.
- `SeedLineage`: private validated seed/source-run/source-through identity used
  solely for contextual comparison. It is pure data, not authenticated I/O.
  `compare(a,b)` returns Result<Ordering, PositionError>: accepted inherited source
  positions sort in source order before local positions; unknown seed, foreign
  source run or source position beyond the selected boundary errors. Local-only
  comparison needs no lineage. No global ordering across unrelated runs.

Expose a small context-bound ordering API, plus explicit local-position access
returning Result/Option rather than coercion. Do not introduce a separate
comparator for feedback, artifacts and amendments. Construct validated lineage
once per loaded semantic context; comparisons allocate nothing.

Field policy in this stage:

| Existing field | Stage 1 representation and invariant |
|---|---|
| ArtifactRef.produced_at_seq | Option<SemanticPosition>; None is an explicitly unrecorded content handle, replacing the two real seq0 placeholders. Recorded artifacts require Some; inherited source seq0 always rejects. |
| OutcomeRecord.seq | SemanticPosition; accepted semantic occurrence, never a source stage commit ID. |
| BacktrackFeedback.edge_seq | SemanticPosition; same lineage as outcomes/artifacts used for selection. Escalations use this same record type. |
| RequirementRevision.seq | SemanticPosition; stable inherited/local audit ordering; task applicability and text semantics unchanged. |
| LedgerTask.updated_seq | Retain successor-local u64 (0 only untouched default); add optional typed inherited provenance carrying seed ID+SourceEventRef. Imported row later points at the actual local seed-binding event, not source seq87. A subsequent local transition clears/replaces inherited last-update provenance. |
| RoadmapPatchMemory.updated_seq / GraphRevisionMemory.updated_seq | Stay local-only runtime metadata; add separate serializable settled-history DTOs with SemanticPosition. Source history is not inserted into runtime amendment maps/counters. |
| applied/processed graph revision seq, stage boundary, snapshot at_seq, occurrence/commit/gate/session/control sequences | Remain local-only. Do not broaden migration into executable authority. |

Add `RunMemory.semantic_lineage: Option<SeedLineage>` as structural comparison
context, default None. Update exhaustive construction/destructuring/policy tests:
this new field is fresh None on ordinary runs, populated only from validated
seed data in stage 4, not imported as raw RunMemory. It is not a launch capability.
All current apply_event producers construct Local positions from the actual
RunEvent, never inspect paths or guess source ownership. A malformed event seq0
must not panic. Section B1 specifies the checked PRE-MUTATION boundary and full
fallibility propagation; the existing APIs are insufficient without that change.

Unrecorded artifact handling is explicit, not a shim: storage byte-addressing
operations may use None; event-folded history, BootstrapContinuation encoding,
feedback, inherited seed artifacts and published materializations reject missing
production position. ArtifactStore put/copy APIs continue to return usable byte
handles without pretending a journal event occurred. BootstrapContinuation's
existing parent-planning handoff remains local-only: its adapter accepts legacy
positive numeric positions, rejects None/inherited positions in this contract,
and preserves its old JSON shape. The later semantic seed is a distinct contract.

### Artifact wire clarification before production: three distinct contracts

Verified actual capture path: ArtifactStore::put returns ArtifactRef with a
pre-event seq0 placeholder (artifacts.rs:100–141). capture_artifacts extracts
ONLY hash/path and emits flat ArtifactProduced { node, artifact, path, name,
source_path } (outcome_acceptance.rs:572–591; run_event.rs:362–374). Its payload
has NO produced_at_seq and NO embedded ArtifactRef. Both full and memory folds
construct the recorded ArtifactRef from the actual positive RunEvent.seq
(run_state.rs:790–795/1841–1846). Fork creates another transient seq0 handle
from the flat event solely for copying bytes (fork.rs:466–481).

Do not claim old ArtifactProduced payloads contain produced_at_seq0, add that
field, or invent a legacy fixture with it. ArtifactRef itself currently has no
generic Serialize/Deserialize impl; BootstrapContinuation owns its existing
StoredArtifact adapter. Preserve each actual wire boundary explicitly:

1. Unrecorded byte handle: ArtifactRef.produced_at_seq=None in memory replaces
   the legitimate pre-event0. There is no generic ArtifactRef serde/wire contract
   to preserve or extend; do NOT add one or introduce a 0↔None field adapter.
   ArtifactStore/fork byte access remains independent of production provenance.
2. Approved BootstrapContinuation history: its existing StoredArtifact adapter
   still rejects unrecorded None/0 and inherited positions; positive Local values
   retain the exact numeric JSON representation. Byte storage success cannot
   promote a handle into approved production history. The later semantic seed
   uses a separate source-qualified contract and likewise requires recorded
   positions.
3. ArtifactRecord SQL/query wire: RunWriter::store_artifact associates the current
   local seq, possibly0 before any event (run_writer.rs:227–242). Keep its numeric
   local materialization index distinct from positive production/source position.
   Its existing EventSeq0 value is explicitly not proof of a production event;
   preserve its existing JSON shape and nullable provenance rule, without casting
   it into SemanticPosition/SourceEventRef or guessing an occurrence.

SemanticPosition::Local and SourceEventRef constructors/serde continue rejecting
zero universally. No byte-handle or local SQL-index exception permits inherited
seq0, null source positions, ambiguous tagged data or approved-history gaps.

Exact regressions: use an old flat ArtifactProduced JSON fixture without a
production-position field; decode/refold under a positive envelope seq and assert
both fold paths produce Some(Local(envelope seq)), regardless of its original
put/fork handle having None. Assert the payload's JSON field set stays unchanged.
Test actual put→capture and fork copy byte access with unrecorded handles;
BootstrapContinuation rejects None/0/inherited and still accepts exact legacy
positive local handoffs. Preserve query ArtifactRecord local-index0 JSON at its
real pre-event storage seam and prove it cannot become production provenance.

## B1. CONT-S1-SEC-01: checked pre-mutation application and atomic batches

Original protected input: nonzero event positions must be validated BEFORE any
recovery observer, verification invalidation, ledger, MCP, requirement, artifact
or outcome mutation. Existing run_state::apply is fallible but first mutates
recovery/verification and does not validate seq0; RunMemory::apply_event and
apply_non_recovery_event return (). Engine execution_state::fold currently only
propagates timestamp conversion errors. These are concrete required API changes,
not an assumption that an existing fallible outer function already checks input.

Choose fallible public memory application with a private checked-input token;
do not add a public generic normalization framework:

- `RunMemory::apply_event(&mut self, &RunEvent) -> Result<(), FoldError>` delegates
  to the same checked preparation as batch application. Add an explicit invalid
  event-position error and SeedUnavailable/startup-marker error as needed to the
  owning core error enums; preserve existing diagnostic variants.
- `RunMemory::apply_events(&mut self, &[RunEvent]) -> Result<(), FoldError>` first
  checks EVERY event and shared marker constraints, collecting private borrowed
  checked inputs/nonzero Local positions. Only after the entire batch succeeds
  does it apply them. A valid earlier event followed by invalid seq0 leaves the
  accumulator byte-for-byte/equality unchanged, including recovery observations.
  Stage 1 checks are input-only; no private payload read is hidden in this API.
- Private `CheckedMemoryEvent<'a>` cannot be constructed/deserialized outside
  its core owner. `apply_non_recovery_event` accepts this checked input; the
  mutation phase is infallible and uses its already validated Local position.
  Public apply_event does recovery exactly once then non-recovery mutation. No
  unwrap/expect, skip, default sequence, post-mutation error or ignored Result.
- `run_state::apply(state, event) -> Result<RunState, FoldError>` runs the SAME
  preflight at its very top, before Bootstrapping/Pipeline recovery and
  verification observation. Its full-state transitions pass the checked input
  into non-recovery mutation so recovery is not counted twice. Public fold
  retains its contiguous-sequence check and propagates apply errors. No new
  whole-state clone per event; checked inputs contain borrowed data/validated seq.
- The new seed marker is unavailable without hydrated input and fails BEFORE
  mutation in these ordinary APIs. Stage 4 extends the one shared preparation
  seam with authenticated seed data/lineage, rather than bypassing checks or
  introducing independent runtime/report algorithms.

Exact production caller propagation:

| Actual caller | Required disposition |
|---|---|
| core run_state::fold / apply / private non-recovery call sites at1111/1141/1162 | Common preflight before all observation; propagate typed errors; no duplicate recovery. |
| engine execution_state::fold at423–434 (load and load_memory) | Propagate memory FoldError as a typed ExecutionStateError variant, distinct from timestamp/storage errors. |
| engine run_task route preparation at2878 | Propagate preparation error before snapshot or commit_stage_route; durable prefix/cursor/owner remain unchanged. |
| engine run_task::apply_read_events at3306–3321 | Convert ALL timestamps, then call atomic apply_events once; map error to explicit storage/fold corruption error. No earlier recovery/verification/ledger mutation survives a bad later event. Graph revisions are returned only after success. |
| core task_execution/compile.rs Compiler::observe at155–159 and compile loop | Return/propagate typed projection error; preflight before cloned verification observation or any task/view mutation. Preflight complete input prefix before publishing a view. |
| persistence runs/query.rs217 and inspection.rs1186 | Propagate core apply failure through existing read/inspection diagnostics; no empty reconstructed memory or inferred success. |
| orchestrator operator/pending/decisions.rs412 | Existing fallible apply handling preserves unavailable decisions; never turn error into an executable human gate. |
| UI run_stream.rs observe_display/core apply | Shared incremental preflight before display/prefix/session/ownership mutation; unavailable invalidates trust and pending decisions, retains diagnostic and true independently recorded terminal facts. |

Refresh the full rg caller inventory before edits. Test callers in recovery tests,
fold_hook_events, daemon bootstrap fixture and engine initial-prompt/profile/
project-context/skill-binding/verification suites must explicitly handle Result;
no `let _ = apply_event(...)`, new lint suppression or compatibility infallible
wrapper is permitted. Public API break is part of section F's API gate.

Actual behavioral REDs available BEFORE new types exist: use current public
run_state::apply with a valid Pipeline state and seq0 RequirementRevised or
ArtifactProduced and assert Err (currently accepts); use current direct
RunMemory::apply_event with seq0 and assert memory equals its pre-call snapshot
(currently mutates). For existing apply_read_events, build a batch with valid
recovery/verification/ledger-affecting events followed by seq0, assert Err and
unchanged caller-owned memory. These tests compile on current types and fail at
behavior assertions. Tests for a future Result signature/token alone do not
count as RED. Keep the existing corrupt-timestamp atomicity regression unchanged.
After the API change assert the specific error too, and test all three mutation
families, full-state apply/fold, engine fold/load_memory and task projection.

## C. Seed-startup schema owner, with activation deferred

Define a validating pure public `ContinuationSeedManifest` under existing core
work_item input ownership. Reuse existing opaque private-object/tag types. Its
versioned, bounded allowlisted fields bind successor operation, RunId and exact
WorkItemBinding, original workspace association identity, source operation/run,
selected source boundary/checkpoint identity, accepted graph hash, SeedId and
opaque private seed-object reference/authentication tag. Values are pure metadata;
no paths/private body/raw memory/session/usage/control/gate authority are exposed.
Use exact raw/try_from decoding; duplicate/unknown fields, zero source positions,
mismatched own run/binding and same source/successor reject structurally. Stage 2
owns actual HMAC/envelope/namespace establishment; stage 4 owns full source proof
and startup normalization. Do not invent public constructors for host capability.

Add `EventPayload::ContinuationSeedBound { manifest: Box<...> }`, stable kind tag,
and event schema v23 through the existing core migrations registry (baseline max22).
Old payloads still decode unchanged; v22-max readers must reject v23 cleanly.
No production writer emits it in this stage. Consumers must not silently treat an
observed marker as a fresh empty run: add one shared pure startup-marker scan
with duplicate/late/mismatch checks and typed SeedUnavailable where no hydrated
seed is supplied. Reuse this guard at engine load/load_memory/startup validation,
read-only report/task projection and trusted desktop history boundaries. It only
recognizes/blocks unsupported seeded input now; it must not implement a second
semantic import/fold. Stage 4 extends this same seam with authenticated seed data.
RunMemory::apply_event alone performs no seed import or execution authorization.

Plain RunReport::compile retains its established API and independently observed
terminal facts, but gains explicit semantic-history availability in its result
(default Available for legacy serialized reports). A TaskExecutionView failure
currently adds a caveat and continues independently computing Completed and
evidence_backed (run_report/mod.rs:798–827); a caveat alone is NOT sufficient.
The shared marker guard must gate semantic task/evidence derivation and
`evidence_backed`: unavailable means None/unavailable, never Some(true), vacuous
success or a false claim that the recorded RunCompleted did not happen. JSON,
Markdown and HTML render semantic history unavailable explicitly while keeping
the actual local terminal claim distinguishable. TaskExecutionView::compile
returns its typed unavailable/error result; engine load/start refuses typed
unavailable. Desktop must preserve inspection/unavailable truth, not fabricate
an empty successful semantic projection.

Incremental trusted desktop closure is required as well as batch replay.
`RunStream::apply` and `apply_recorded` both flow through apply_at/observe_display;
run_stream.rs:596–614 owns startup association eligibility. Integrate the SAME
small startup-marker state/checker there BEFORE changing prefix evidence,
display_seq, sessions, decisions or WorkItem trust. Recognize the marker as a
startup record for phase checking, but do not grant semantic/work-item/session
trust from it without seed hydration; invalidate unavailable semantic trust and
suppress executable decisions. Duplicate/late markers diagnose explicitly.
Maintain a per-stream incremental checker with constant work per event; batch
replay uses it over the prefix. Do not rescan the whole journal on every live
event or add a second UI import algorithm. Generation/current-owner fencing from
Home remains intact. Test apply, apply_recorded, pending replay and cold candidate
publication with a live marker, duplicate/late marker and true terminal control. Registry/per-run event writers may store schema-valid marker bytes
for compatibility tests; persistence storage is not admission authority.

## D. Full caller and persistence closure

The following producers/consumers migrate together; author refreshes the exact
rg inventory against the frozen stage 0 handoff before editing and records it.
No remaining bare comparison, max_by_key, source numeric serialization or
unqualified EventSeq conversion is accepted as a temporary compatibility shim.

| Owner/files | Required change |
|---|---|
| core run_state.rs | All artifact/outcome/feedback/requirement construction, ledger last-update clearing and amendment history DTOs; 35-field policy coverage; local-only authority fields unchanged. |
| core bootstrap_continuation.rs | StoredArtifact custom adapter, constructor invariants, legacy positive numeric fixtures and inherited/unrecorded rejection. |
| core execution_recovery/mod.rs, lib.rs, run_event.rs, migrations/mod.rs | Shared wire/position exports, validated manifest/event/kind/schema and unavailable-marker guard. |
| engine snapshot.rs, execution_state.rs plus tests, work_items.rs | Core DTO re-exports/local conversions, unchanged snapshot versions, common marker refusal at load/load_memory/startup; no seeded execution yet. |
| bootstrap/handoff.rs, engine/fork.rs, persistence artifacts.rs | True source-local positions for handoff; explicit unrecorded byte handles for put/fork; retain paths/hashes without inventing production events. |
| stage/agent.rs | previous_attempt_feedback becomes fallible for invalid lineage; contextual outcome/artifact interval selection, no seq0 lower bound; scoped newest-last requirement revisions retain order. |
| engine/run_task.rs | read_split_tasks accepts only a matching successor-local artifact after actual local StageEntered; inherited large seq cannot qualify. Amendment processing counters remain local. |
| stage/bindings.rs, loop_stage.rs, subgraph_stage.rs, predicates.rs | All artifact/node output access uses new positions without numeric coercion or accidental ordering change. |
| stage/verification.rs | Source amendment history never appears as current local revision/criteria; local patch observations remain local. |
| roadmap_target.rs | latest_of_kind currently max_by_key(produced_at_seq): migrate contextual selection/unavailable handling rather than impose Ord. |
| persistence runs/types.rs, reader_views.rs, views.rs, writer.rs, run_writer.rs, storage.rs, migrations.rs; task_ledger.rs | Materialized ArtifactRecord and TaskLedgerRow/index retain actual local row event index plus optional typed source provenance. Writers/readers/upsert/reopen/rebuild agree; numeric SQL ordering never stands for source ordering. |
| core run_report/mod.rs+render.rs, task_execution.rs; UI mission/run_stream/results adapters; notify/render.rs | Source-aware semantic evidence labels/order; occurrence identity remains TaskOccurrenceId with real run/seq, no synthetic inherited StageEntered/session. Unsupported seed reads explicitly unavailable; ordinary renders unchanged. |
| CLI ledger/ready/mcp_serve/report; engine IPC/daemon_facade and desktop callers | Transport/JSON fixtures accept changed record fields, retain local indices separately; inherited provenance cannot become an executable target. |
| artifact_contract/names.rs, affected integration/unit fixtures | Shared ArtifactRef fixture constructors and serialization changes; no relaxed artifact validation. |

Persistence representation is deliberately small: nullable provenance JSON on
per-run artifacts/task_ledger and registry task_ledger_index, decoded through the
same validated provenance type; missing column after supported migration means
legacy Local, malformed/present-incomplete provenance is corruption. Add next
numbered per-run/registry migrations through existing registries; no rewrite of
old events. Existing local index columns stay INTEGER. Source-only settled
amendment history is carried in the later private seed DTO, not fabricated SQL
roadmap_patches rows. Reader rows expose local materialization sequence and source
provenance separately; inherited artifacts/ledger import will index the actual
seed-binding event in stage 4. Stage 1 creates no seeded materialization rows.
No provenance-only public API is allowed to bless an unhydrated manifest as true.

## E. Behavior REDs first, then implementation

Tests that use new contracts may become executable only after the minimal type
skeleton compiles; record compile/setup failures separately, never count them as
behavioral RED. Preserve stage 0 missing-action RED unchanged. For stage 1, write
fixed behavioral expectations before migrating production consumers:

1. Legacy v1/v2 nested loop/subgraph snapshots decode/re-encode the same JSON;
   split children order, indexes, retry counts, return nodes and bindings survive
   conversion. Existing orchestrator import path still compiles.
2. Explicit source87/local5 ordering selects source feedback before local route;
   source report20 lies between source outcomes10 and backtrack25, while an
   unrelated source run/seed or beyond-boundary position refuses. Test actual
   previous_attempt_feedback selection with retained artifact bytes and a fixed
   expected rendered summary, not a second sorting implementation.
3. An inherited discovered-tasks artifact at source900 never satisfies local
   stage-entry3; a fresh local artifact4 does. Use actual read_split_tasks caller
   behavior and fixed child IDs/order. No repeated split import is introduced.
4. Requirement texts for a fixed task include inherited then newer local text;
   unrelated task revisions stay excluded. Ledger imported provenance remains
   separate from local seed-row index; later local update supersedes that origin.
5. Raw JSON zero/negative/overflow/unknown variant/foreign seed/run/ambiguous
   payload cannot bypass private constructors. Valid legacy numeric handoff
   round-trips; unrecorded put/fork handles still copy bytes but cannot serialize
   as approved history or a source event. Include malicious deserialization tests,
   not merely constructor tests.
6. Actual per-run + registry migrated databases close/reopen and rebuild preserving
   local row indices/provenance independently. Legacy rows decode Local, corrupt
   provenance refuses, no cast from source87 into successor EventSeq. Test direct
   SQL facts plus returned records; no implementation-mirroring decoder oracle.
7. Schema23 marker round-trip/kind and schema22 rejection; duplicate/late/wrong
   binding markers refuse. Engine load/load_memory, report/TaskExecutionView and
   desktop trusted projection without seed-read data show unavailable, never
   fresh/empty semantic success. Include independently recorded RunCompleted and
   apparently positive local ledger evidence after an unavailable marker: report
   JSON/Markdown/HTML retain the true terminal fact but never evidence_backed=true
   or complete inherited tasks. Exercise incremental RunStream apply and
   apply_recorded, not only finish_display_history, and prove no pending decision
   or trusted ownership survives unavailable input. Ordinary no-marker fixtures
   remain identical.
8. Amendment source metadata remains qualified history; current runtime maps and
   applied/processed counters receive only local events. Actual report/UI/CLI
   adapters preserve provenance labels and never navigate inherited evidence as
   a successor-local occurrence or human-decision target.

Stage 1 does not claim stage 4 seed-once/two-restart execution REDs are passed;
those remain explicitly open. Exhaustive construction/destructuring protects
future RunMemory field additions; avoid wildcard/default-based coverage claims.

## F. Gates, review and compatibility

Freeze exact stage 0 source/test diff and RED receipt before author changes.
Run affected full core/orchestrator/persistence/Git/daemon/CLI/UI/notify tests and
relevant compatibility/IPC/migration integrations, with the deliberate stage 0
RED reported separately. Strict Clippy default and all-features across affected
crate closure, workspace fmt --check and git diff --check are required. Include
external/integration consumers, serde fixtures and doctests; do not rely on one
crate compilation. Use repo timeout/nextest conventions and actual platform
exclusions. No guessed commands/results or inherited green receipts.

Public compatibility is not purely additive: changing public ArtifactRef/
OutcomeRecord/Feedback/RequirementRevision fields and adding row fields breaks
Rust struct-literal callers; removing unrestricted numeric ordering is deliberate.
DTO relocation with re-export is source-compatible; legacy stored JSON remains
read-compatible; new tagged inherited JSON/new event schema rejects on old
readers. Record API review/semver findings against the exact stage 1 diff before
closure; do not hide a Major-on-paper change behind the development version or
claim existing public examples compile without checking. No release/version or
publication action is authorized here.

Review focused 5a against this stage and accepted design, then required affected
architecture/domain/security/API 5b. Root controls count, freeze and stage commit;
no dispatch or local commit by this planning task. Full dependency closure must
be green before handing stage 2 a type/API; never ship a bare-seq bridge to avoid
migrating a consumer. Stage 1 cannot close the overall continuation feature.

## Maintainer objections resolved and remaining decision

Simpler siblings: existing typed identity/HMAC/serde patterns, snapshot wire DTOs
and existing materialized local indices are reused. Adding Ord to a qualified
position, concatenating journals, copying raw RunMemory, or importing source
amendment authority would look smaller but violate the accepted contract. One
lineage-aware ordering seam avoids per-consumer rules. A validated optional
artifact position preserves existing legitimate pre-event byte storage without
making zero a source event. Nullable provenance avoids remapping all SQL integer
indices. No per-comparison allocation or scan of another run is needed; manifests
are bounded, private payload hydration remains outside core.

Review disposition: CONT-S1-SEC-01 is addressed by B1's fallible preflight,
private checked input and full atomic-batch/caller propagation. Critic watchpoints
are addressed explicitly: serializer/input invariants with real current-type REDs;
RunReport semantic availability gates evidence/rendering beyond a caveat; and
incremental trusted UI startup/ownership checks share the marker guard without
quadratic rescanning. V1 bytes are retained at
`.rust-studio/evidence/bootstrap-owned-continuation/stage1/plan-v1.md`, SHA
`80815bf07fa7ed3abfedcd2fec86190a826d9bdf324b3241c9de671fed57e21f`.

Owner verdict: ACCEPTABLE after this reshape as a tactical realization of the
accepted full design.
No material unresolved product/strategic fork identified. Independent reviews
must check the concrete public/schema/unavailable-consumer closure before code
handoff. Any inability to keep the shared wire/origin migration complete is a
reshape boundary, not permission to defer a known caller behind a numeric shim.


## Actual initial stage 1 handoff

Owner and independent critic/security accepted revised body SHA
`d94ee7f1e2faaaf0270a949fd8d7e6d2ba5482c1745b14535005dd66e0a21829`;
exact accepted bytes are retained in stage1/approved-plan-snapshot.md before
this handoff note. Stage0 readiness/spec and generic/domain/security reviews
are closed, with precise advisory evidence limits in
stage0/quality-readiness-review.json. Its genuine missing-command RED remains
visible and the success provider/report suffix remains a stage5 prerequisite.

The same usable feature author receives the sole Rust/Cargo lease for initial
stage1 implementation at baseline39673ff5 plus frozen stage0 test closure.
Feature repair count remains0/3 shared across every stage and5a/5b. Local stage
commit follows complete dependency closure, required reviews and real gates;
no author commit/push/merge, no raw failed-run Resume or native runtime action.
No full continuation/native all-eight-task/gpt-6.1-sol completion is claimed.


## Pre-production artifact clarification receipt

Root requested a legacy placeholder check while production was held; the
verified source trace disproved a persisted ArtifactRef0 payload premise. Root
explicitly cancelled the generic 0↔None serde adapter request: no such wire
contract exists and none is added. The author has three authentic current-type
RED assertions; this planning task executed none. The
accepted v2 snapshot remains byte-exact at approved-plan-snapshot.md (SHA d94ee7f1
e2faaaf0270a949fd8d7e6d2ba5482c1745b14535005dd66e0a21829). The source trace above
corrects the proposed persisted-payload premise; it preserves the accepted
nonzero source-position invariant without inventing a field or a new public
serialization framework. Owner verdict ACCEPTABLE pending root/focused review
of this narrow clarification; no source/tests/Cargo/runtime actions here and
feature repair count remains0/3. Production hold/release remains root-owned.

## Canonical review-repair record

Feature: bootstrap-owned-continuation; all stages0–7 and phases5a/5b share limit3.
Current count: **2/3 repair dispatches**. Home10 remains closed/exhausted separately.
Baseline39673ff5172adbb391691d68487cf0352c060180; initial90-file freeze digest
864e6d1b5f9b5dc4fac5ca2b55851baa1a08c997026dfc81c3a858bda1b4eec2, manifest
71043550c9bfa334509f3f444776a866adc0e3709700435f5a2c545cc42eab15.

Phase5a reviewer continuation_stage1_spec returned NEEDS WORK: blocking P2
CONT-S1-SPEC-01, artifact provenance omitted in loop/subgraph consumers.
Repair1 dispatched to same builder; all20 invalid cases across5consumer forms
executed as genuine RED, validcontrols5PASS; missing-file precedence alsoRED.
Four existing-validator insertions nowGREEN11/11; affected gates ongoing.
No independent repair acceptance or5b/API verdict yet.

Focused same-reviewer clarification found the explicitly named predicates sibling
also belongs to CONT-S1-SPEC-01: invalid source outcome/artifact can affect branches;
OptionNone cannot express refusal because Not/fallthrough publishes normal outcomes.
Predicates remain untouched under HOLD pending owning API design assessment and
independent pre-code review. A further implementation dispatch will increment the
shared count; it will not be renamed initial authoring or hidden in worker changes.

The pre-dispatch1 counter was durably recorded in ignored evidence JSON, rather
than this existing task note. This annex corrects the canonical record location;
that JSON is retained only as historical receipt, not a second status database.
Future count decisions are recorded here before dispatch. Original accepted plan
bytes remain in approved snapshots and freeze-v1 reviewed-task-snapshots.

### Predicate closure scope adjudication

The same5a reviewer confirms predicates are blocking CONT-S1-SPEC-01.
Agent outer error typing and provider-open order are not additional accepted
Stage1 blockers: consumer Err followed by explicit trusted hook recovery differs
from invalid data becoming false inside predicate evaluation. Root preserves
Agent error policy and records those two limitations as advisory. Predicate API,
Branch publication and actual caller closure proceed to pre-code assessment only;
no predicate implementation dispatch yet, counter stays1/3. Exact adjudication
is retained in stage1/predicate-and-agent-scope-adjudication.json.

### Repair2 dispatch — predicate closure

Recorded before dispatch to same usable builder. Repair1 settled/paused at90-file
digest d48868615448d9a8e8e9f83b55516608b3eb217d1ada27ce9868ee134ddb56e7,
fullOrchestrator1301/1301 plusrelevantignored3/3/strictboth/fmt/diff green.
CONT-S1-SPEC-01 remains blocking for predicates. Owned predicate proposal
8ba2896cd313997d466699b9e3858ec24630a34f7ced30786a290dba75c86dcb received
owner ACCEPTABLE and independent critic/security ACCEPTABLE. Root releases
Repair2/3 for exact predicateAPI/Branchcaller/caller-doc-bench migration plus
authentic RED/GREEN and required affectedgates. No Agent hooks/provider-order
change; those limitations remain advisory. Source/destination tree is same
codex/results-planning-history checkout at39673ff5; no newtask/budgetreset.
No5a/5b/API acceptance/commit/Stage2/push/merge is implied.

### Repair2 settlement — awaiting independent focused5a

Builder Rust/Cargo lease PAUSED; repair count remains2/3, one automatic dispatch
left across all stages0–7 and both review phases. Current94-file source digest
66bd5f7ba9d3922d2274ac1d2ddfa34162bb6d2fa1fffec8cd4680274f3a86d1.
All4180 current affected tests PASS; strictboth, recovery3+2, doctestsboth,
normaldocs58unchangedwarnings, fmt/diff PASS. Outer0/1 expectedRED; strictdocs
baseline failure and absent automatedAPItools remain explicit. Repair2 handoff
and exact receipts are under stage1/repair2. No independent acceptance, Stage2,
commit, native/Sol invocation, push or merge is implied by these gate results.
Root next freezes v2 and returns CONT-S1-SPEC-01 to the same5a reviewer.

### Stage1 bounded closure — independent reviews accepted

Same5a reviewer: ACCEPTABLE — Stage1 Phase5a; CONT-S1-SPEC-01 resolved.
Independent5b generic, DOMAIN and SECURITY: COMPLETE, no blockers. API-GATE:
COMPLETE — MAJOR-on-paper; intentional full caller migration accepted, no
automatedsemverPASS/releaseclaim. All94frozen sourcehashes independently verified.
Detailed receipt: stage1/phase5b-review-v2.json; freeze-v2 manifest SHA
f37c37734cf6e80c6b41c82b9b9a0348bbfe2125cb4dc4aab32b7d16c11fdd0e.
Nonblocking rustdoc advisory: select_canonical #Errors could explicitly mention
Position refusal. No additional implementation repair dispatched; counter2/3.

Stage0 readiness + Stage1 foundation may be committed as a local dependency-closed
checkpoint. This does not close the feature or deliberately failing outer test.
Stages2–7, authenticatedseedhydration, endpoint, nativeUI, same real Sol habit
tracker/history/all8tasks, followup and platform acceptance remain open. No push/merge.
