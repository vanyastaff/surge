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
Current count: **5/5 repair dispatches**, with explicit human extensions for repairs4 and5. Home10 remains closed/exhausted separately.
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

### Stage2 marker contract finding — pending third repair release

Current count remains2/3; no production marker fix released. Root's independent
owning-contract review found blocking CONT-S2-MARKER-01: parent353–354 explicitly
requires missing/replaced marker refusal; current inflight Stage2Substep1 stores
namespace/key identities but verifies marker content only. Identical-byte native
replacement is unproved and current owner assessment identifies the gap.
Stage2ownerplan f1e267143979fce52d2a8a5e3bcdd3dc63660979c59141ff272b8b73802ec25d
underspecified this parent obligation without waiving it. Source is951da616 plus
Substep1f441d69f…15-file candidate; fullintegrationcompile was interrupted for
pool-correction, strictboth nowgreen oncurrentcandidate, nofeaturecompletion.

Read-only clarification and current-code RED probes do not release production
fixes. A narrow marker-native-identity amendment is being drafted for independent
critic/security pre-code review. Once accepted and the actual RED is captured,
root will record3/3 HERE BEFORE dispatching the implementation correction. This
is conservatively counted as review-driven repair, not renamed initialauthoring
or a newstage budget. Further automatic repair beyond3 is not authorized.
Original Stage2initialauthoring and builder-owned test/compile inner loops remain
recorded separately; no earlier count reset or hidden retrospective pass is made.

### Repair3 dispatch — CONT-S2-MARKER-01

Recorded BEFORE marker production release to the same continuation_builder.
Shared limit3/3 is now exhausted across stages0–7 and bothreviewphases; no automatic
fourth repair. Home10 remains separate/closed; no counter reset by newstage.
Source/destination: codex/results-planning-history, HEAD951da616, inflight
Stage2Substep1f441d69f…15-file production candidate plus genuine marker tests.
Current-source RED session77418 exit100:3run,1healthycontrolPASS,2FAIL; actual
Unix native dev/inode IDs differ, displaced originals retained, rows/key/private
bytes unchanged and owners settled before assertions. Receipt SHAa85870ae8c367c6dfdd16fa85f6b4a3a23cce37c372c76cb8e875d8620b7bf6e.

Narrow owner amendment004885c6a4add9510b2f8d08bed4e6d5f85a3f6014826fb2de0748db38523f13
accepted by owner plus independent focusedcritic/security. Production correction:
retain/persist/verify exact marker native identity through existing PrivateFiles
owner; atomic write-once markerID with key/digest/Established in uncommitted0035;
complete/decode/native/platform/crash/legacy closure; preserve connection-aware
verification. Windows actual held-handle denial positive plus separatecoldloss/
replacementrefusal. No newprotocol/API/core/seedactivation/sandbox/capability.
Refresh focused/full affected gates, exacttwo pendingRED filters and separate
currentexpectedREDs, then PAUSE for independent Substep1/repairreview and local
commit consideration. No selfacceptance/commit/pushmerge/fullfeaturecompletion.
Amendment draft wording initialshaping/count2 was historical; THIS canonical
record and explicit3/3dispatch supersede it. Any further review-driven production
correction requires explicit extension, with a concrete reviewed proposal.


### Repair4 dispatch — private-unavailable recovery and held-connection closure

Recorded BEFORE implementation dispatch to the same usable continuation_builder.
The human explicitly answered: “Да, разрешаю четвёртый раунд в этом объёме”.
This extends the ONE shared feature budget to4/4; no reset by stage/review/worker.
Reviewed proposal: bootstrap-continuation-fourth-round-proposal.md, SHA256
`d81f9ff85d8d61294aa5d80230afd857a9ef6caf38d77d3c040c0f6e5152fde4`.
Owner, independent critic and security verdicts ACCEPTABLE; receipt:
`stage2/fourth-round-pre-code-review.json`.
Source: HEAD951da616f8de86cd3e1898ecbbfbbb95e5d4dee1 plus paused16-file
stage2/substep1 source digest8cfb8896c695c6e84b750b337e04d6fce53e6b5e8232ddbe28af4cfe823ba1c4.
Blocking evidence: current full gate4199PASS/2FAIL out4201; cold private-input
failure refuses dispatch but leaves attempt Suspended. Additional static held-
transaction verify_lock re-entry sites require actual bounded-pool RED first.
Authorized closure: exact-fenced Attention under original OS guard, separate typed
private-unavailable result, no fake Ready/private claim or permanent mismatch
receipt; identified held-connection verification call closure; RED→GREEN including
all8 fault cases and positive/race controls; current full gates and independent
5a then5b/API reviews; local stage commit only after closure. No push/merge.
Original retained habit tracker, history/all8 tasks/exactgpt-6.1-sol remain required
for later stages, not replaced by this repair. No fourth-round success claimed yet.
Any further review-driven implementation after this dispatch requires explicit
new human mandate; verification/review of repair4 remains allowed.


### Repair4 verification and review boundary

Final21-source digest62572fdb2864e17f7f2b4a2b5399263b4297c1f215e11a7b3e3884fde8ed6cd4.
Current affected eight-package4210/4210PASS (25existingignored+2 separately real
pending RED), workspace strictboth/recovery3/restart2/doctests/docs/fmt/diff
recorded in stage2/repair4/gate-summary.json. Phase5a ACCEPTABLE; SECURITY and
API COMPLETE, DOMAIN initially COMPLETE. Phase5b QUALITY NEEDS WORK with
CONT-S2-QUALITY-01 blocking: if-let pooled-read temporary in private_establishment
open successful arm overlaps existing() second pool borrow. Existing max2/hold1
tests enter open/hydrate before holding external connection; no dynamic actual
reopen reproduction yet. Static exact call/lifetime evidence requires narrow
adjudication/plan, not removal of owner checks. No fifth fix/dispatch authorized.
Source/test/Cargo writer remains PAUSED; local commit HOLD pending quality closure.
Shared count4/4 EXHAUSTED. Evidence phase5a-review.json / phase5b-review.json;
current proposal/review work is read-only/documentary and does not reset count.


### Repair5 dispatch — CONT-S2-QUALITY-01 established-open lifetime

Recorded BEFORE dispatch to the same usable continuation_builder. Human explicit
answer: “Да, разрешаю пятый раунд в этом объёме”. One shared budget extended to5/5;
no reset across stages/reviews. Independently accepted fifth-round proposal SHA256
b234d9d84a3822efbe69511ef318fe1f5efcda372be4bb075897f52c73dce55d.
Baseline HEAD951da616f8de86cd3e1898ecbbfbbb95e5d4dee1 and21-source digest
62572fdb2864e17f7f2b4a2b5399263b4297c1f215e11a7b3e3884fde8ed6cd4 verified unchanged.
Only confirmed if-let481→existing341 redundant pooled checkout lifetime is
authorized: real bounded public reopen/replay/claim/hydration RED first, explicit
connection scope/drop before native helper; preserve all native/stored/guard checks,
healthy/refusal controls, affected full gates and independent5a→focused5b/API.
Withdrawn lazy-boolean461 claim excluded. Local dependency-closed commit only after
review closure; no push/merge. Prior4210PASS remains historical until refreshed.
History/all8 habit tasks/exactgpt-6.1-sol and later feature stages stay required.
No implementation success claimed; any sixth review repair requires new mandate.


### Repair5 ongoing-build mechanical fixture closure

Before5a/5b, current fullgate4212/4213PASS exposed existing child readiness marker
publication race (empty vs busy). All3 new bounded-public pool regressions passed.
Unchangedfocused1PASS does not clear fullfailure. Owning architect and independent
security accepted test-only protocol correction; root records this as mechanical
failure inside ongoingrepair5, not a sixthreviewrepair or countreset. Only test
marker complete atomic publication and bounded liveness/complete-token observation
may change; exact busy/held,10sdeadline, native exclusion/deathreacquire and actual
child joins preserved. No prod/pool/API/schema/skip/retry change. Details:
stage2/repair5/mechanical-readiness-scope-decision.json. Fresh fullgates/review
remainrequired, no success claimed. Shared count5/5 remains exhausted.


### Stage2 Substep1 closure after repair5

Current21-source digest e1f08dce2310a96953db25ed7e00876a75e53ac2dd50d3129557bdebf612c069.
Phase5a ACCEPTABLE; focused5b QUALITY/SECURITY/DOMAIN/API COMPLETE, no blockers;
CONT-S2-QUALITY-01 resolved. Prior wholeSubstep1 unchanged acceptance carries.
Current affected eight-package4213/4213PASS,27skips=25existingignored+2 separate
expectedRED. Fullworkspace strictboth, focused30, ownership5 independentprocesses,
sameprocess3, ignoredrecovery3/restart2, doctestsboth, normaldocsboth/fmt/diff PASS.
Normaldocs58exactbaselinewarnings NOT strictclean; APItoolsabsent, manualAPIgate.
MacOS-only receipts; noWindows/Linux/MSRV/nativeprovideracceptance. Evidence
stage2/repair5/{builder-handoff.md,gate-summary.json,phase5a-review.json,phase5b-review.json}.
Root may commit entire dependency-closed Substep1 only; no push/merge. FullStage2
Substeps2/3 seedstorage/pins/deletion and laterstages remainOPEN, two genuine
pendingREDs retain that boundary. Source/test/Cargo lease PAUSED. Shared5/5 stays
exhausted acrossfeature stages/reviews, no reset or sixthrepair dispatch.


### Planned Stage2 Substeps2/3 — conservative legacy deletion scope

Root explicitly accepted positive physical deletion only for NEW runs coordinated
before first namespace creation. Legacy history/reads/ordinary execution/continuation/
pins remain; Legacy physical delete returns DeletionCoordinationUnavailable without
filesystem/SQL mutation. Writer reopen cannot upgrade Legacy; missing/replaced
NewControlled origin/control cannot downgrade. No extra database-FD lock bridge.
See Stage2 appended criterion record and bootstrap-continuation-stage2-seed-storage-amendment.md
for exact owner/reader/deletion closure and independent pre-code review gates.
Human NO new UI: Stage6 new native UI deferred, existing history support retained;
same habit/all8/codex-acp/exact gpt-6.1-sol evidence remains OPEN and mandatory.
This is initial planned feature design, not implementation release. Shared5/5 stays
EXHAUSTED across stages/reviews; no sixth dispatch/reset.

### Initial implementation release — Stage2 Substeps2/3

Root verified final amendment SHA52701bf84d8d55f71dbe2838bc3924fb3f1d22cf055242e72ca0a5c2799c5797
and API draft a406200881a547707378a91cd07b67591b2c2587733bc078ba4d9dd6652b2fa5.
Owner/critic/security ACCEPTABLE; API ACCEPTABLE with exact final contention erratum
verified by root. Initial planned feature implementation released to existing builder
from committed77c5f128. Complete storage/reservation/seal/read-back/private pins/
controlled deletion closure together; no intermediate completed slice/commit.
Genuine RED then GREEN and full applicable gates/review remain mandatory.
No new UI, no original habit runtime/SQL/provider changes yet, no push/merge.
This release is not a review repair; shared repair count remains5/5 EXHAUSTED.
No later review-blocker repair may be dispatched automatically without a revised mandate.
Evidence: stage2/substeps2-3-pre-code-review.json. Outer same-history/all8/exactSol
acceptance remains OPEN.

### Initial Stage2 policy fork — human decision pending

Persistence preflight632run/631PASS/1FAIL: static_event_pages_read_latest_live_wal_and_keep_cold_files_unchanged.
New retained run-read admission performs registry READ_ONLY origin/control queries;
cold SQLite registry coordination creates WAL/SHM paths forbidden by the prior test.
No safe existing no-create helper was found; immutable/nolock/cleanup shortcuts rejected.
Default and all-features workspace/all-target strict clippy PASS on source-pre-policy
45-path digest34e5d083bec44938bf44ea32a44a89b4eedfa96809eb64438185e15b35ee4af0.
Source/assertion behavior PAUSED. Root presented explicit contract choice to human.
Proposal bootstrap-continuation-registry-readonly-policy-proposal.md SHA
9cc9cf434e5865ad5e6155ebc4f51ef08863288870595f9c7fc48b44140ab64d,132lines.
Owner/critic ACCEPTABLE as proposal; security CONT-S2-POLICY-COMPANIONS-01 resolved
by exact no-follow identity/security checks and nonvacuous cold ordering.
Proposal/review is NOT permission. Await explicit response before any policy/test edit.
Independent already-authorized docs/fmt/compile checks may finish. No source runtime/
provider or original habit change. Same8/exactSol/outer backend remains OPEN.
Shared review-repair count remains5/5 EXHAUSTED; no sixth dispatch/reset.

Pre-policy independent checks terminal: strict workspace/all-target default and
all-features PASS; docs both modes exit0 with58 baseline warning/file multiset
(no added/removed, line shifts allowed); doctests both modes5PASS/1existingignored;
fmt/diffPASS. Root independently checked all45 source hashes unchanged against
source-pre-policy.json. Behavioral631/632 +1FAIL remains; no full-nextest,5a/5b,
stage completion or provider claim. Sole builder Rust/test/Cargo lease PAUSED;
all commands settled, pending explicit human policy choice. No push/merge/cleanup.

### Human approval received — registry coordination sidecars only

Human explicitly answered: «Да, разрешить только эти SQLite sidecars (рекомендую)».
Approved exact132-line proposal SHA9cc9cf434e5865ad5e6155ebc4f51ef08863288870595f9c7fc48b44140ab64d.
Release only the reviewed narrow policy/tests/docs and safe exec observer; preserve
all existing durable bytes/native identities/security/control checks and live WAL
freshness. New WAL has no frames; only exact checked registry companions allowed.
No new UI, production VFS/locking/cleanup bypass, push or merge. Required full gates,
independent5a/5b and local completed-stage commit remain part of authorization.
This resolves the initial feature contract fork, not a sixth review repair; shared
5/5 EXHAUSTED remains unchanged. Earlier failing receipt is retained. Same habit
history/all8/exact codex-acp/gpt-6.1-sol and later backend stages remain OPEN.


### Initial Stage2 definitive gate — typed unpublished-opening closure

Frozen48 source digest a616325e49a6d21ae4bd97586def56999d121958bbdb81191a89a854e2498f3d
full8 gate:4261run/4260PASS/1FAIL/26skipped, exit100; failure archive retained at
stage2/substeps2-3/gate-v1-failure/. Only failure: daemon owned_flow_mcp_recovery::
manifest_binding_refusal_must_persist_attention_after_actual_wake. Real producer
reserves latest Opening with NULL database identity, then creates DB and binds
identity through exact opening/epoch/state guarded UPDATE. Reader currently decodes
NULL as Vec, laundering a truthful intermediate state into a generic SQL error.
Owner/API/security independently ACCEPTABLE the bounded initial-build correction:
validate original control/current route/latest version/state/route first, both Unix
and Windows. Only genuine latest Opening+NULL returns OpeningNotReady{run_id}
before run SQLite open. Missing/malformed/unsupported/foreign/nonOpening NULL must
remain permanent refusals; no older-row fallback or synthetic Busy/success.
Use crate-private typed admitted-open seam retaining all existing checks. Inspection
read_event_range/read_folded propagate StorageError::RunOpening(OpenError); manual
From<OpenError> preserves only genuine Sqlite(e) as old StorageError::Sqlite(e).
Public exhaustive error variants are MAJOR-on-paper under accepted active-dev policy;
no wire/schema/public signature/capability change. Readiness test polls only exact
same-run typed transition plus its existing genuine missing-events schema window,
under unchanged deadline. Deterministic paused-producer RED before fix, bound read
GREEN, malformed/latest-row negatives, conversion and both caller proof required.
Root releases only this corrective closure to sole existing builder, then fresh full
gates/source freeze and independent5a then5b. No new UI/provider/user runtime/push/
merge. Current work is ongoing initial feature build, not post-review repair6;
shared review repair count5/5 EXHAUSTED unchanged. All8/same history/exactSol and
original outer acceptance remain OPEN; failed gate cannot be reported green.


### Stage2 Substeps2/3 current implementation — independent5a dispatched

Root independently verified49source hashes at gate-source-v2 digest
2c44436fc185ebe824aacfe45136630bff23605dc4836a2c6676a10394252b16 againstHEAD77c5f128.
Builder current-handoff-index.json records4264/4264fullPASS,26skips=25existingignored+
1exactplannedouterREDfilter; strictworkspace/alltargets bothPASS, ignoredrecovery3/
restart2PASS, docs both58baseline warnings, doctestsboth5PASS/1existingignored,
fmt/diffPASS. APItoolsabsent/manualinventoryonly. AuthenticouterRED remainsOPEN at
missingendpoint; no real provider/appcompletion/Windows/Linux/MSRVclaim.
Sole source/test/Cargo writer PAUSED; all command handles settled. Independent
continuation_stage1_spec reviews whole dependency-closed Substeps2/3 vs77c5f128,
including new untracked files and approved scope changes. Phase5b NOT started.
Builder native Windows reader watchpoint (artifacts-missing fallback may create)
forwarded as evidence to5a, not yet a finding or authorized repair. Shared5/5
EXHAUSTED remains; no postreview repair dispatch without revisedmandate. No local
completedstagecommit/push/merge yet; samehabit/history/all8/exactSol remainsOPEN.


### Phase5a evidence correction and confirmed native blocker — no repair release

Independent5a verified listed49hashes but found omitted tracked modified daemon
owned_flow_mcp_recovery.rs. Builder corrected metadata ONLY: current-source-complete
accounts50paths/digest5a53f4c609e45c35f41b4386bb13405d68020e6d8506c6c7c38d124e29424e03.
Original49receipt/copy retained. Omitted fixture92231bytes/SHA
aa7e25e4548a3792d5a8e3534995c8c4299542877b2dc48df178523380991f2e reconstructed
from retainedbeforefix+single recordedreplacement, exactcurrentmatch. There was NO
contemporaneous complete50-source freeze; do not claim it. Tool chronology is the
limited evidence for the fixture, not invented hash timestamps. Current index is
corrected; no Rust/test/Cargo writes or rerun occurred. Final5a pending.
Independent5a confirmed Windows reader creates missing artifacts through existing
RunRoot::run(run,false) fallback before latestopening auth. This violates exact
no-other-path-mutation contract; not advisory/platformunverified-only. Root requests
read-only bounded correction PLAN, not implementation. Shared5/5EXHAUSTED; no sixth
repair dispatched. Need explicit revisedmandate before repair. 5b held, no completed
stagecommit/push/merge; realhabit/all8/exactSol remainsOPEN.


### Phase5a final NEEDS WORK — sixth proposal reviewed, awaiting human

Final independent5a: ONE blocker CONT-S2-SPEC-01(P2), Windows refused read creates
missingartifacts before native/latest-row authentication. No other concrete spec
blocker found in whole Stage2 Substeps2/3. Evidence omission accounted as limitation,
not fabricated fullgate-time50proof. Phase5b remainsPENDING, no completedstagecommit.
Bounded sixth proposal bootstrap-continuation-readonly-opening-sixth-round-proposal.md
145lines/SHAa96bb70661d582dacdb836408e181e4dcaf8e6dbabcea8cb77838a45ee815376:
private Windows existing_run(existing_childONLY), readcaller migration, writable
creator unchanged; nativepublicinspection RED→GREEN, positivecreate/readcontrols,
fullcomplete-source/gates/focusedreviews. Owner/critic/security ACCEPTABLE finalplan.
CurrenthostmacOS/no evidencednativeWindowsrunner; implementation requires human6
AND nativeexecutionpath, no WindowsRED/PASS inferred. Localexisting ci.yml supports
workflow_dispatch/windows-latest. Root will explicitly ask optional publication of
ONEdiagnosticcodexbranch+existingCI dispatch fornativeproof; NO currentpublication
permission, no newworkflow/PR/merge, eachcheckpointreviewablefirst. KnownouterRED
separate fromfocusednativepass, neveroverallgreenCIclaim. Approval/execution still
PENDING; solebuilder remainsPAUSED, no repair/source/test/build dispatch occurred.
Shared5/5EXHAUSTED unchanged. Samehabit/history/all8/exactSol/newUIexclusion remain.


### Human sixth-round authorization received — counted before dispatch

Human explicitly answered: «Да: шестой раунд, диагностическая ветка и Windows CI»
to exact145line proposal SHAa96bb70661d582dacdb836408e181e4dcaf8e6dbabcea8cb77838a45ee815376.
Repair6 is now dispatched/counted6/6, ONLY CONT-S2-SPEC-01 and required regressions,
fullchecks/independentreview/localcommit. NativeWindows path is existing GitHub CI:
user also authorizes publication of ONEdiagnosticcodexbranch+existingworkflowdispatch,
noPR/merge/newworkflow/newUI/userhabit/runtime/providerchanges. NativeWindowsRED
must be actualobserved before productionfix; locallyreviewable test-only diagnostic
checkpoint may be committed/pushed to obtain that evidence, not a completedstage.
Root checked gh auth/remote readonly; successfulrunner/dispatch notyetclaimed.
Priorfull50sourceclosure and incomplete49gateprovenance retained. Samehistory/all8/
codex-acp/exactgpt-6.1-sol remain goal afternecessarybackendstages. No repair7 or
permissionreset inferred. Solebuilder receives test-only firstlease; root ownsGit/CI.


### Repair6 diagnostic native SETUP failures — mechanical compile-only lease

Diagnostic70ad449c published ONEauthorizedbranch, workflow39149097408 exactSHA.
WindowsClippy117738899222 nativecompile failed beforebehavior: missing explicit
Windows-core-error→existing native/OpenError conversion at namespace.rs1443;
Windows-only unused lease at storage.rs456; cfg(test) observe visibilityE0603;
new reparse-test Result payload mismatchE0308. Rawnativejoblog retained. These
are SETUP, NOT behavioralRED; reader creating getter/caller remain unchanged.
Root releases bounded mechanical compile correction only, preserving native
errors/security/creator semantics/testassertions, no new publicFrom/errorAPI or
lint suppression. Same ongoingrepair6 includes these compilation failures;
count6/6 unchanged. Re-freeze and independent diagnostic-checkpoint review before
newcommit/push SAMEbranch/dispatch. No production read-admission fix before actual
WindowsbehavioralRED. No PR/merge/newworkflow/UI/habit/provider action.


### Repair6 diagnostic v2 — native compile advanced to strict lints

Compile-only e048e90a published SAMEbranch, workflow39149740756. WindowsClippy
117740971447 advanced past priorrustcerrors; fourstrictlints nowfail: Windows
_namespace usedbinding, WindowsUTF16 %2/chunks_exact, native testmodule before
newproductionitems. Rawlogci-windows-clippy-diagnostic-v2 retained. This is still
SETUP/strictgatefailure, NOTbehavioralRED. WindowsTestSuite117740971470 continues
independently; root will not cancel/re-publish solely for these lints before its
behavioral result. Bounded mechanicallease only: cfgcorrect namespacefieldname,
existing UTF16 evenlength validation preserved via exactchunks/remainder, move
unchanged testmodule afterproductionitems. No lintallow/newAPI/securitychange or
readgetterfix. Sameongoinground6/6; sourcefreeze/reviewrequired before anypublication.


### Repair6 native v2 target tests — observer SETUP, root-anchor correction

WindowsTestSuite117740971470 on e048e90a completed4540run/4486PASS/54FAIL/53skip;
rawlogci-windows-tests-diagnostic-v2.log preserved. Sevennewtargettests compiled;
reparsePASS, other6fail beforetargetassertions in snapshot_worker owner validation.
This is SETUP, NOTbehavioralRED; productionreadgetter remainsHELD. Other54failures
are not automatically scope-authorized. Source-bound diagnosis confirmed bybuilder
andsecurity: collect firstobserves statehome, but helper classifies outertempfile
parentasProtected. Realproducer protectsONLYstate; actualproduction outerisAncestor.
Rawlog lacks path/SID; exactnativebasename/owner is not claimedobserved.
Rootmaintainer ACCEPTABLE +independentsecurity ACCEPTABLE bounded cfg(test) fix:
captureNamespace::existing_parent(home) ONCE, holdoriginalprotectedhome throughcollect,
rootfactsfromSAMEhandle, descendantsnormalrelativecomponentsONLY/existing_childONLY,
retainoriginalroot+parent verification before/after, existing leafnofollow/type/nlink/
ACL/owner/nameID and existingSqliteSidefilepolicyUNCHANGED. No newAdmin/defaultowner
exception, no arbitrarypath/parentescape/productionguard/API/Unixchange. Add bounded
path/phase errors only(noSIDs/privatecontent). Existingnative targetassertions remain.
ReleaseSOLEbuilder testobserver-only correction, localchecks/fullderivedmanifest,
independentdiagnosticreview before SAMEbranchpublish+existingWindowsCI. Reviewed
unpublished3lintfixes e93b21… maysharecheckpoint. No actualgetterfix untilgenuine
WindowsbehaviorRED. Ongoingrepair6/6 includesrequiredtestsetup, no reset/repair7.


### Repair6 native v3 — exact producer directory roles and direct effect oracle

Workflow39153752152 on5577f7b0 completed. WindowsTestSuite117752562787:
4540run/4485PASS/55FAIL/53skip; target six fail BEFORE inspection at
state/work-items leaf-security-pre, reparsePASS. This is SETUP, not targetRED.
Source proves producer intentionally requires Protected{inheritable:false} there,
while cfgtest observer hardcodes true for every directory. No production ACL defect
is established. Owner/security independent bounded plan verdicts ACCEPTABLE.
Exact producer-backed false relative paths ONLY: daemon, work-items, work-items/locks,
work-items/preparation-locks, work-items/private-flow-inputs, run-controls. Sources:
windows/mod.rs54–60, namespace.rs1037/1039, private_objects.rs20–32/60/175.
Other paths retain true expectation; root retains independently captured owner.
Apply same exact mapping to normal-component cumulative parent traversal and leaf
directory validation; no prefix/suffix inference, try-both ACL, owner/ACE/protected/
rights/nofollow/type/nlink/name-ID relaxation, native creator or Unix change.
Add ONE direct public inspect_existing_run effect regression independent of Snapshot:
real producer; settle actual writer/run and registry owners; remove empty artifacts;
require exact metadata NotFound before; inspect; capture result/metadata before any
verifying SQL; settle/cleanup; require typed native refusal and exact NotFound after.
All existing seven native tests/assertions remain unchanged and required. This direct
oracle proves only missing-artifacts effect, not whole-home mutation preservation.
Release sole builder for these cfgtest changes ONLY. Actualgetter/caller remains HELD
until actual native behavioral RED. Re-freeze/local checks and independent diagnostic
reviews before root commits/pushes SAME diagnostic branch and existing workflow.
Same ongoingrepair6/6, no reset/repair7/newUI/PR/merge/habit/provider changes. Other
55 native failures and six Windows UI UnixListener compile errors remain outside
this bounded repair; no global green claim.


### Repair6 native v4 — genuine behavior RED; release bounded production correction

Diagnostic2f4e14621ae443c597765892b09e8d5cd54fd2e8, workflow39157692686,
WindowsTestSuite117764222765 completed4541run/4508PASS/33FAIL/53skip.
Direct publicinspection test reaches READONLY ARTIFACTS EFFECT line355: before
OS2/NotFound, inspect Some(Ok(RunInspection)), after Ok(directory Metadata).
This is actual native behavior RED after actual producer/owner settlement and
cleanup, not compiler/setup failure. Three missing-artifacts full matrices also
reach final assertions; healthy writer+64readers, foreignrun, foreigndatabase and
reparse controls PASS. Full raw log+classification retained.
Release solebuilder EXACT previously accepted production correction: Windows
crate-private RunRoot::existing_run verifies original root, opens run/artifacts
via existing_child ONLY, builds same retained RunRoute then verifies it. Switch
ONLY RunUseOwner::database Windows caller to existing_run. Writable run(false),
creation/ownership/errors/ACL/coordination/schema/API/Unix behavior unchanged.
All eight regression tests and observer assertions unchanged. Full local gates,
native Windows GREEN and independent focused5a/then5b remain required. Same
ongoingrepair6/6, no reset/repair7. Root owns diagnosticbranch/CI, noPR/merge/UI
or habit/provider actions. Other29native failures and existing WindowsUIsix
compile failures remain outside this bounded correction, no globalgreen claim.


### Repair6 final scoped disposition — CONT-S2-SPEC-01 RESOLVED; Stage2 NEEDS WORK

Code checkpoint fbaed8fb210d7f5a24f3d22fdf4598e2139b0aad / ALL52 source digest
42fc0a36998e7aa5acd3a35c983093f065920a6c5d98b3dc302e08a740a11d7a.
Native v4 genuineRED retained; v5 workflow39160468869/job117772518526 ALL8
UNCHANGED target tests PASS, including full missing-bound/NULL/malformed matrices,
direct creation-effect oracle, foreignrun/database, reparse, writer+64readers.
Independent focused spec/security/owner re-review: CONT-S2-SPEC-01 RESOLVED.
Local full4264/4264PASS,26skip=25existingignored+exactfutureouter; strictworkspace
default/allfeatures PASS, history11, ignoredrecovery3/restart2 PASS, docs58baseline
warnings eachmode, doctests5PASS+1existingignored eachmode, fmt/diff PASS. All52
source hashes stayed identical through gates; all owned verification tasks settled.

WholeStage2 Phase5a remains NEEDS WORK; Phase5b BLOCKED/not started. V5 Windows
4541run/4497PASS/44FAIL/53skip; v4→v5 has29common,4targetresolved,15newlyobserved.
No blanket baseline, unrelated-failure or getter-causality claim is justified.
WindowsClippy still six UI UnixListener E0433. Standard-user probe also fails
SETUP: stale exactfilter accepted_public_flow_replay_precedes_unavailable_private_storage
selects0tests; do not call standard-user evidence green. FreshmacOS4553/4554 and
Linux4947/4948 only fail authentic futureouter MISSING CONTINUATION; two priorLinux
UIsettlement cases passed unchanged, not proof their underlying flakiness was fixed.

Caller-map correction: changed directcall is control::database, ALSO reached by
writable startup storage::upgrade_existing_run and verify_empty_initialized.
Actual creators run(true)/existing_parts run(false) remain unchanged and establish
artifacts before databaseowner validation. No source-level getter regression found;
remaining nativecaller gate attribution stays open. Source/test assertions unweakened.

Sharedrepair6/6 exhausted; no new repair or wider permission inferred. NoPR/merge,
newUI/provider/user-runtime/history mutations. Same retained habit publicTOML hashes
confirmed, all8tasks/2milestones and7exact codex-acp/gpt-6.1-sol bindings preserved;
real continuation NOT executed. Full receipts/classifications and independent verdicts:
sixth-round/readonly-getter-sixth-round-final-disposition.json and referenced logs.
This documentation checkpoint records bounded resolution and failed gates, not
wholeStage2 completion or approval to start Phase5b/furtherrepairs.


### Human extension to repair7 — remaining mandatory Windows gates only

Human explicitly authorizes: «Разрешаю следующий раунд: закрой оставшиеся
обязательные Windows-гейты и review без новых улучшений UI. Затем продолжи тот
же habit tracker через codex-acp на gpt-6.1-sol, сохранив историю и все восемь задач.»
This extends shared limit to7, does NOT reset count or authorize unlimitedrepairs.
Completed6/6 remains recorded; repair7 implementation NOT YET dispatched.
Read-only owner/builder/spec diagnosis now scopes actual44nativeFAIL,6UIWindows
UnixListener compileerrors and standard-user stale exactfilter0tests on tested
fbaed8fb; sourceHEADfc9b5b2c adds documentationonly. ExistingCONT-S2-SPEC-01
resolved8/8nativeGREEN remains intact. New mandatory findings will receive stable
IDs, source-backed ownership/RED plans and independentpre-codecritique+security
before solebuilder writelease; canonical7/7 must be recorded BEFORE thatdispatch.
Mechanical compile/setup corrections within the same bounded attempt retain7/7,
no reset. Existing single diagnosticcodexbranch+existingci.yml remain the native
verification route for authorized mandatorygatework; no newworkflow/PR/merge or
UXimprovement is inferred. Fix necessary platform testcompile without changing UI
behavior. Preserve allnative security/identity/ownership refusals, actualcoldtests,
fullassertions/model/session evidence, source pins and separatefutureouterRED.
Unknowncauses require observation before classification/fix; no blanketbaseline
exception or timeoutincrease/testskip/lintallow to manufacturegreen.
After necessarygates/reviews and previously accepted continuationstages, resume
same retainedhabit project/all8tasks/history through exactcodex-acp/gpt-6.1-sol.
No userruntime/providercalls or history rewrite during gatediagnosticrepair.


### Repair7 dispatch — mandatory Windows gate attempt 7/7

Recorded BEFORE solebuilder release. Full staged plan mandatory-windows-gates-seventh-round.md
has independent harsh-critic and domain/security ACCEPTABLE verdicts. Owner IPCB
and empty-inventory component decisions ACCEPTABLE, actual native causal RED still
required before either production correction. Firstlease: source-backed testfixture/
Unix-only-test-cfg corrections plus bounded native diagnostic/RED anchors, preserving
original assertions/refusals and resolved eight readonly cases. Private exacttyped
result and remaining recovery phases must be observed before any semantic correction.
Root owns Git/CI, builder owns Rust/tests/scripts; sourcefreeze/readinessreview before
samebranch existingci.yml native dispatch. This is ongoingrepair7/7; mechanical
compile/setup issues in thisattempt do not reset count or imply repair8.
Full required Windows gates and independent5a→5b remain completion conditions;
no newUI/provider/userhistoryactions/PR/merge. Buildreserve2GiB retained.


### Repair7 first native Clippy — mechanical test correction released

Checkpoint c885efee2d8bd6f60a4b9403cda6945885d36229, workflow39167406937,
Windows Clippy job117793126251 completed FAIL. Retained raw/clean logs show
exactly three new cfg(windows) test lints: excessive_nesting in private_inputs.rs
registry-witness wait, unnecessary_debug_formatting for Path diagnostic, and
assert_is_empty in deletion.rs anchor. Release sole builder ONLY equivalent
test refactor/Display/empty-value assertion corrections, without lint allows or
weakened cleanup/assertions. Production IPC/deletion/private mapping remain HELD
until actual native causal RED. Test job117793126362 remains running; do not
push/dispatch and cancel its native evidence. Same ongoing attempt7/7.


### Human publication extension — current branch PR and merge

Human now explicitly directs: «давай доведи до merge», after inspecting the full
GitHub comparison (225 files, +14821/-293, 35 commits vs main6722b495). This
authorizes publishing a PR for codex/continuation-readonly-windows-20261018 and
merging only after fresh final-source mandatory CI, including Windows, and closed
review. Previous no-PR/no-merge restrictions are superseded for this branch.
No new UI improvement or history/provider mutation is inferred during gate work.
Ongoing repair7/7 remains the same attempt, not a budget reset. Existing workflow
concurrency separates workflow_dispatch and pull_request, so draft PR creation
does not cancel the native diagnostic run39167406937. The PR must state actual
open gates and incomplete continuation endpoint; no green or app-completion claim.
Keep same retained habit tracker/all8/history/exact codex-acp/gpt-6.1-sol as the
continuation goal. Git/remote operations remain root-owned, no force push.


### Repair7 native checkpoint — release exact deletion/private corrections

Native c885efee / run39167406937 Windows job117793126362 completed4547run,
4500PASS/47FAIL/53skip. All8 readonly regression cases PASS unchanged. Actual
owned original zero-length events.sqlite.lock inventory fails ReadFile OS33;
nonempty full-hash and locked-nonempty-refusal controls PASS. Actual unavailable
private replay and begin both return StateHome/native Open NTSTATUS(-1073741565),
exact STATUS_NOT_A_DIRECTORY. Independent owner/security preaccepted these exact
conditions. Release sole builder accepted empty-length read avoidance preserving
identity/security+before/after metadata, and ONLY exact SDK NTSTATUS mapping to
PrivateInputsUnsafe. No other error/status/launch/Attention authority changes.

IPC actual Ping anchor is confounded: healthy canonical passes, subsequent
recreated same-name listener pairs all return AccessDenied, including canonical.
Bare/absolute address mismatch and missing-parent acceptance are observed, but
actual causal alias Ping RED is not established. IPC production remains HELD
for focused owner/security-reviewed test correction and fresh native evidence.
Unknown recovery47 includes six new anchors; no blanket baseline/load attribution.
Linux4944/4948 has authentic futureouter plus3 UI settlement failures; macOS
4553/4554 only authentic futureouter. These remain mandatory PR151 blockers.
Same ongoing repair7/7, root Git/CI ownership and user no-new-UI boundary retained.
Full logs and ci-first-native-classification.json retained. No provider/history
actions and no whole Stage2, whole CI or merge-readiness claim.


### Repair7 tactical IPC oracle/probe release after independent review

Owning lead and security independently ACCEPTABLE: correct only the test oracle
to ONE retained actual listener with sequential healthycanonical/ordinary/canonical/
verbatim clients, original2s each and exact Ping87/version. Accept loop waits for
explicit stop (not four successful accepts); failed ordinary connect must not kill
the listener. Label resolve/create/connect/accept/read/write/response errors, retain
later canonical positives, stop and actually join/settle owners before homeclose
and final assertions. Actual native ordinary-alias RED with healthy controls is
still required before IPC production B. No retry/recreation or weakened assertions.

WG7-PROBE exact stalefilter replacement is ACCEPTABLE in this same correction
checkpoint now original native private Open STATUS_NOT_A_DIRECTORY is confirmed
and narrow mapping implemented. Select exact existing private-backed replay test,
retain one actual execution/PASS/name/currentbinaryhash/non-elevatedSID/nonzero
checks; all other cases/security/receipts unchanged. Next native probe must prove
GREEN, this release does not assume it. Sole builder lease extends exactly to
IPC test module and this script filter; no IPC production/recovery/UI behavior.
Same7/7, PR151 fullCI/review/merge authorization, source freeze before dispatch.


### Mandatory PR151 gate plans — isolated native discriminator and test scheduling

Independent spec diagnosis WG7-RECOVERY-OBS01: native deadline cuts occur during
admitted nonterminal progress, not a proven permanent fault. Quota has established
session and durable SessionOpened before first-send gate; MCP cases reach differing
session/usage/closure boundaries; cold mock remains in real startup; split already
commits/routes and starts later session. Post-timeout lease-drop logs are effects.
No production recovery/timeout change released on this evidence.

Owning lead and security accept ONE fixed Windows isolated diagnostic step before
the UNCHANGED full suite, exact five binary/test identities:
- surge-daemon::quota_recovery_route_test / actual_429_after_declared_source_change_is_audit_only_and_primary_is_attempted_again
- surge-daemon::owned_flow_mcp_recovery / explicit_empty_survives_real_cold_wake_and_changed_global_defaults
- surge-daemon::work_item_route_test / cold_daemon_resumes_committed_startup_with_same_run_and_accepted_acp_context
- surge-daemon::work_item_route_test / task_split_recovery::accepted_split_recovers_children_once_with_fresh_retry_budget
- surge-orchestrator / engine::ipc::windows_address_tests::public_resolver_aliases_reach_same_actual_listener_with_ping
Preflight inventory MUST equal exact set5; require exactly5 actual executions.
Same SHA and actual native test/mock binary hashes, test-threads1, no retries or
per-test deadline/assertion changes, inherited120s killbound. Reuse existing receipt/
logging seams, separate focused logs/JUnit from existing bridge evidence and upload
with always immediately after focused step. continue-on-error ONLY allows fullsuite;
final always enforcement checks actual focused AND full step outcomes, fails for
either failure or skipped required execution. No selectable dispatchinputs/new
workflow/framework or reduction of full acceptance. Implementation lease remains
HELD until root receives owning lead exact mechanism confirmation and dispatches.

Mandatory Ubuntu UI gate plans UI-CLOCK-01/UI-HYDRATE-02 have independent owner/
security conditional ACCEPTABLE verdicts and critic source diagnosis. They change
TEST synchronization only, no production UI behavior. First capture deterministic
old-harness RED: forced fast read completion demonstrates5s fakeclock overshooting
original500ms/1s/2s retry boundaries; delayed failed-read publication demonstrates
live/subscribed precedes completed hydration. Then step independently fixed original
retry intervals only after actual same-owner/run settled read; preserve exact1..4
counts (never>=), exhaustion/replacement/navigation assertions. For hydration reuse
controlled_history_reads load/release seam and require same run/token Incomplete +
!is_reading with retained diagnostic/pending before repair/retryclock. Preserve all
pending/session/unconfirmed-fence/eventual-history assertions and existing deadlines.
UI author lease remains HELD for first corrected native checkpoint publication;
root may release these exact test-only RED/repair paths afterward within explicit
user PR151 mandatory-green mandate. No new feature/UI improvement or count reset;
ongoing7/7 remains shared. Source author remains sole builder, root owns Git/CI.


### Repair7 fixed native observation mechanism — sole-builder lease released

Owner final ACCEPTABLE confirms exactfive identities above. Use a tiny logging-only
wg7-windows-observation nextest profile (same inherited120s timeout, fail-fastfalse,
retries0, separate JUnit directory) and one fixed Windows step in existingci.yml.
Preflight JSON inventory exact(binaryID,testname) set equality, nonignored, and
postrun exactfive actual executions; capture currentSHA and actual selectedtest/
mockbinary hashes, verify samebinaries after fullsuite. Capture nativeexitcode,
early always upload separate logs/JUnit/inventory/identity evidence. Focused
continue-on-error is for sequencing only; preserve fullsuite execution even when
focused/evidence step fails and finalalways enforcement checks REAL outcomes
(including evidence failures), never maskedconclusions. Full original suite/
commands/requiredplatforms remain mandatory. No dispatchinputs/newworkflow/general
diagnosticframework. Reuse existing receipt helpers where possible; a bounded
fixed wrapper or inline workflow implementation may implement exactly this seam,
with meaningful checker checks and independent readiness review before publishing.
Release source lease for this mechanism alongside already released two production
corrections, IPC TESToracle and exactprobefilter. UItest authoring remains HELD
until first corrected native checkpoint is frozen/reviewed/published, so Windows
evidence can run while later approved test-only deterministicRED proceeds.
IPC production and unknownrecovery semantics remain HELD until causal evidence.
Ongoing7/7, root-only Git/CI, no newUI/provider/history actions.


### Existing recovery diagnostic scope — later macOS corruption refusal cut

Initial PR151 macOS c885efee also fails transplanted_objects_and_changed_manifest_
refuse_real_cold_writers after successful park/corruption/coldrestart, childphase
refuse, original10s wait_attention. Reconciliation reports acceptedassociationchanged;
ManifestBinding is only a candidate, dedicatedmanifestbindingcase PASSED same run.
Do not change production classification/guard/expectedAttention on this evidence.
Extend existing bounded TEST observation lease only: emit exact corruption label,
run/control state, bounded lastjournal and duewake/reservation/refusal phase before
cleanup at existing wait_attention seam; reuse retained stores, never rawliveDBFD,
no private payloads. Preserve original10s/all no-effect+Attention assertions, owners
and actual settlement. This is existing WG7-RECOVERY observation scope, not a sixth
focused case/new discriminator/repair8. Exact isolatedset5 remains unchanged.


### Fixed observation implementation closure accepted

Sole builder proposed exact4file closure before extrahelper edits: existing
.github/workflows/ci.yml, .config/nextest.toml, fixed scripts/wg7_windows_observation.py
and its stdlib unittest file. Root accepts this bounded implementation of the
independently approved seam. Reuse bridge_portability run_command/revision_errors/
read_junit, strengthen exact(binaryID,name) matching, actual5execution/zeroignore/
skip/duplicate checks, currentSHA and actualtest/mockbinaryhash before/afterfull.
Capture/verify phases only, no configurableselectors/dispatchinputs/overrides.
Native list/run shares fullsuite Cargo flags; logging-only profile+serialfocused
execution is the sole runtime difference. Checker branches must cover mismatches,
empty/extra/duplicate/ignored lists, failed/skipped/error/wrongidentityJUnit, source/
binarydrift and masked/skipped outcomes. Actualnextest CLI/artifacts defineformat.
Workflow finalalways enforcement checks checker/focus/full/verify/bothuploads REAL
outcomes; all diagnostic step failures sequence fullsuite without erasing failure.
Root keeps sourcefreeze/readiness/native dispatch and final fullgate obligations.
Same7/7, no new workflow/framework/product/UI/provider/history authority.

### Corrected checkpoint published; approved test-scheduling lease released

Root committed and pushed `8e9e935eb94cea97a7a9e50009c9a2c7fad96f99` to PR151.
Frozen checkpoint gates: persistence643/643, strict workspace all-targets default
and all-features, checker7, fmt/diff; all terminal exit0. Spec/security/critic
accepted diagnostic readiness only. Native corrected checkpoint remains pending
in run39172594448; whole-feature Phase5a/5b and continuation outer remain open.

Sole builder may now implement only reviewed UI-CLOCK-01/UI-HYDRATE-02 test
scheduling corrections in the two existing UI test files. Production UI and IPC
remain held. Independent critic accepts deterministic CLOCK counterexample by
splitting the same old5s window at independent500ms/1s/2s boundaries, settling real
current-owner reads and consuming remaining1.5s: record actual1→2→3→4, require4,
then retain old expected2 assertion. This establishes an old TEST oracle defect,
not a production regression. No counter/token/state/timer mutation. GREEN advances
one independent original interval per actual settled read, preserving original
exact counts, exhaustion/navigation assertions and deadlines. HYDRATE uses actual
controlled failed-read publication and same-token Incomplete+!is_reading barrier.
No new UI behavior, production seams, timeout increases or skipped assertions.
Same ongoing7/7; no new repair dispatch or budget reset.

### Native retained-listener causal RED — release accepted IPC plan B

Fixed native5 in run39172594448 actually executed5: four recovery cases PASS,
IPC alias case FAIL. Clean synthetic merge65308188 has identical Git tree to
published8e9e935e (46a2c6e5a2d4dff6705a28bc2ee3a141cfc0ab7f), actual source and
five binary identities unchanged before/after. Early artifact retained under
seventh-round/focused-native-39172594448; root verified actual JUnit/phase output.
ONE retained listener: healthy canonical PASS, ordinary client-connect OS2
(file not found), canonical after ordinary PASS, verbatim PASS. Actual server
accept/read/write phases and explicit stop/join complete. Old recreation confound
is absent. This is sufficient native causal RED for previously reviewed plan B.

Sole builder may now edit only existing IPC resolver/tests/rustdoc, its inaccurate
daemon_facade caller comment and existing getting-started IPC lifecycle docs.
Windows resolver validates original UTF8 filename; empty relative parent resolves
current directory; canonicalize EXISTING parent and require directory; join exact
filename, then unchanged pure windows_pipe_name. Propagate errors, create nothing.
No lexical namespace stripping, fallback old hash, new FFI/dependency/public API
or namespace/auth/adoption authority. Preserve distinct homes, missing/nondirectory
refusal, real original-listener alias Ping, foreign-PID and native security fences.
Document mixed-version paired daemon stop/restart using matching old binary.
Local meaningful tests, strict affected Clippy both modes/fmt/diff and frozen
reviewable checkpoint required; actual native GREEN remains pending publication.
Current full Windows same-SHA suite must finish for discriminator comparison;
root holds remote push until then. No recovery production/scheduling change yet.
Same ongoing7/7, no repair reset, no UI/provider/user-history actions.

### Remaining WG7 recovery observations — pre-code plan, authoring held

Full Windows run39172594448 executed4547:4534PASS/13FAIL; readonly8 and all48
standard-user executions PASS. Exact source/binaries remained unchanged across
serial and full runs. The four selected recovery controls PASS in BOTH schedules;
this supplies no basis for changing their scheduling or runtime semantics. IPC
planB plus approved UI harness fixes are published as665cb261. Native after-fix
validation is pending; bootstrap readiness/IPC failures are not yet called fixed.

Six original WG7 caller failures need narrower evidence. Owning read-only analysis
finds four populated-MCP cases cut INITIAL parking at20s, before corruption/replay:
cycles0, Launched, no control; real provider/session progression is still observed.
Empty-MCP success omits populated catalog startup and is not an equivalent budget
control. Quota killed-host case cuts the FIRST readiness wait12s, before kill/wake;
ColdHost stderr filtering discards existing safe quota phase lines. Legacy-cap
case cuts VERSION12 prompt progression at3s after VERSION13 Attention succeeded;
its listener is already connected. None is proven caused by IPC or private-auth
classification. Teardown errors after assertion do not establish the cause.

Proposed diagnostic-only closure, pending independent critic/security acceptance:
- Existing owned_flow_mcp_recovery test fixture: bounded timestamp/result-kind
  observations around CatalogBridge open/send/close and fixed mock MCP/provider
  request/response-kind counters. Capture current reservation/cycle/control and
  bounded journal-kind tails before original assertion and orderly settlement.
  No message/prompt, private MCP argument, token, environment or object payloads.
- Existing ColdHost test support: retain only explicitly whitelisted WG7-safe
  phase lines in its already bounded stderr excerpt; preserve existing filters
  and bounds. Quota fixture records readiness phase/timestamps and existing safe
  retained-state summary before actual durable ready publication. Do not change
  readiness, actual owner settlement or12s wait.
- Existing legacy-cap test: add version/run and bounded retained journal/control/
  active-run/bridge-state/socket-method diagnostics at the unchanged3s cut before
  cleanup. Record the already-live corruption connection without inventing cause.
  Keep13→Attention and12→real prompt, all counts and assertions.
- Keep every original fixed-five native case, and extend the SAME fixed checker
  mechanism to exactly six by adding the existing healthy populated cold twin.
  This corrects the now-demonstrated empty/populated observation gap. Exact binary/
  testcase inventory/JUnit identities and nonzero/pass/skip checks must require
  all SIX, with tested missing/extra/duplicate/failure branches. Same profile,
  serial execution, unchanged deadlines/retries, source/binary identity fences,
  early artifact, unchanged full-suite command and final actual-outcome gate.
  No dispatch input, second workflow, general framework or reduced full gate.

Source lease remains HELD until this refinement passes independent pre-code review.
No production recovery, scheduling, timeout, expected outcome or writer/namespace
change is proposed. This remains the same ongoing7/7 WG7-RECOVERY attempt, not a
new post-repair finding queue or budget reset. Root owns Git/CI; user runtime,
habit history, all8 tasks and exact provider/model remain untouched during gates.

### WG7 recovery diagnostic refinement accepted — sole-builder release

Owning attribution, independent harsh critic and security/domain accept the above
bounded observation refinement. Same ongoing repair 7/7; no counter reset and no
production correction authorized. Root now releases only the four existing daemon
test/support files and existing fixed-checker script/unit test plus the workflow
step's five→six description. Keep the existing profile and all gate commands.

Counters mark actual invocation and completion after the actual await; absent
completion means pending/unknown, never failed/settled. No diagnostic guard across
await, new observer task, await point, retry or authority. Capture bounded state
before actual cleanup, preserve cleanup ordering and original assertion/failure if
observation itself fails. Whitelist fixed producer markers AND fields; no generic
WG7 substring forwarding Debug, configuration, mock params or environment. Fixed
mock counters expose method/response kinds only. Extend cardinality and malformed
inventory/JUnit self-tests together for six exact cases and zero skips. Preserve
original twelve/twenty/three-second waits and all readiness/corruption/transport/
prompt/Attention assertions. Root owns Git/remote; builder freezes complete source
and publication closure with appropriate local gates before independent diagnostic
review. Current native CI665 continues; publication timing remains root-owned.


## Original WG7 native worktree-registration oracle correction

This remains the original authorized seventh Windows gate attempt, not a new repair
round or an expansion to UI. Published ec6468a0 preserves the bounded six-case
observation and full-suite comparison; its native result remains pending.

The completed 665cb261 run39175364865 had Windows4527/4547 passing and20 failing.
All three native resolver regressions, eight read-only opening regressions and48
standard-user executions passed. Fixed-five serial5/5 passed with identical actual
source and binary identities before/after serial/full execution; empty MCP, cold
startup and TaskSplit then failed in the full suite. This supports a causal
comparison, not a proven contention or semantic diagnosis. The original recovery
caller phases remain under observation; deadlines and assertions remain unchanged.

The authentic Windows continuation outer reached fixture430 after completed
planning, accepted source prefix, common Git directory and expected branch proofs.
Its raw log records failure of Git porcelain text containing Rust canonical path
text; exact spelling difference is inferred because the two rendered values were
not logged. Unix outers still reach the genuine missing-continuation endpoint.

Owning builder, independent critic and security pre-code verdicts: ACCEPTABLE for
ONE test-only correction in bootstrap_implementation_continuation.rs. Replace the
substring oracle with actual `git worktree list --porcelain -z` strict NUL-record
parsing and lossless path handling. Require exactly one existing canonical-path
match, with captured original HEAD and exact refs/heads/run branch on THAT record.
Missing/malformed fields, duplicate matches, wrong directory/HEAD/branch refuse.
No textual prefix stripping/case repair, lossy decode or production authority.
Keep every independent common-dir/branch/.git/sentinel/dirty/source/history/eight-
task assertion unchanged. Cover actual registered worktree success and negative
controls, spaces/Unicode and sibling-prefix distinction, malformed/duplicate rows.

Release the sole builder for that fixture only. Native failure above is the actual
platform RED; local correction must exercise actual Git and retain the authentic
outer's expected missing-endpoint RED with real cleanup, not claim outer GREEN.
Record literal command denominators, bounded build resource ownership, fresh strict
workspace default/all-feature/fmt/diff results and frozen hashes; independent review
before checkpoint publication. No production, runtime scheduler, nextest scheduling,
timeout, retry, skipped assertion or new UI change is authorized by this slice.
Root owns canonical plan, all Git/CI/evidence metadata and next push. Preserve the
in-flight ec6468a0 native serial/full comparison before another push.
