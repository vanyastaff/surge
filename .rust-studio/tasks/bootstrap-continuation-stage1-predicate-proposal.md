# Stage 1 predicate provenance refusal — owning API proposal

Status: FINAL PROPOSAL; DOC ONLY. Owner verdict: **ACCEPTABLE for independent
pre-code critic/security review**. Root and the same phase 5a reviewer have settled
the scope as predicate API plus actual Branch caller only. No implementation is
dispatched here.

Baseline: `39673ff5172adbb391691d68487cf0352c060180` plus the current uncommitted
stage 1 closure and concurrently owned repair 1. This survey ran no Cargo/build,
Rust tests, provider/runtime action, commit or source edit. Reported local compiler
1.98.1 and declared MSRV 1.96 were not independently verified. The design uses
existing workspace Rust/Result facilities and no new ecosystem dependency.

The canonical counter is the **Canonical review-repair record** in
`bootstrap-continuation-stage1-plan.md`: **1/3**, shared across stages 0–7 and
phases 5a/5b. This proposal does not reset it. Only root may record/authorize the
next repair dispatch (which would be 2/3), after independent acceptance. Repair 1
owns loop/subgraph source and gates; its reported 20 behavioral RED cases and
GREEN 11/11 are not predicate coverage or independent acceptance.

## Problem, source closure and ownership

Accepted design already includes branches: parent proposal lines 183/224 carries
outcome semantics for branches; lines 258–264 requires qualified ordering and
foreign-source refusal. Stage 1 plan section D, line 314, explicitly names
`predicates.rs`. This is remaining CONT-S1-SPEC-01 closure, not a new feature.

| Current source | Actual behavior / consequence |
|---|---|
| `crates/surge-orchestrator/src/engine/predicates.rs:17–23` | `outcome_of` takes `recs.last()` and exposes its key without validating that record's `seq`. An invalid inherited record can decide OutcomeMatches. |
| Same file `:25–31` | `artifact_size` does metadata I/O immediately after map lookup. Unrecorded/foreign/out-of-bound artifact refs can influence ArtifactSize; metadata failure hides invalid provenance as ordinary absence. |
| `crates/surge-core/src/predicate.rs:19–36,44–56` | Public trait uses Option for semantic data; evaluator returns bool. None means a false leaf, which Not can invert and arm fallthrough can convert into the default outcome. Thus returning None on invalid provenance is unsound. |
| `crates/surge-orchestrator/src/engine/stage/branch.rs:31–64` | First true arm immediately appends OutcomeReported; after all false arms it appends default OutcomeReported. Failure must leave both sites unreachable. |
| `crates/surge-orchestrator/src/engine/run_task.rs:1513–1522,2278–2299` | Dispatch forwards StageResult to ordinary result resolution. No predicate-specific alternate execution owner is needed. |
| Same file `:3165–3180,4053–4083` | `run_on_error_hooks_owned` explicitly returns no records/outcome unless the node is Agent; existing `non_agent_node_skips_hooks` tests Branch. A Branch provenance error cannot currently be hook-suppressed. |

Initial suspicion that every branch error could reach suppression was disproved
by that actual callee guard. Retain and test the guard; do not introduce a generic
error-policy change on this mistaken premise. Failure diagnostics are separate
from a branch decision: the outer engine may record StageFailed/RunFailed and
existing failure observations. It must not record branch OutcomeReported,
EdgeTraversed, consumed route/checkpoint or dispatch a downstream provider because
of the invalid predicate. Direct execute_branch_stage failure appends no event.

Core owns pure predicate/combinator semantics and its public trait. Orchestrator
owns memory lookup, structural lineage checks and filesystem metadata. Core must
not import RunMemory, filesystem metadata, persistence or StageError. No new crate,
dependency, seed hydration, scheduler, verifier receipt or sandbox is proposed.

## Reuse survey and alternatives

The current stage 1 sibling primitives are sufficient:

- `execution_recovery/semantic_position.rs`: PositionError (Clone/Eq/thiserror),
  SemanticPosition::validate, SeedLineage; validation is allocation-free and local
  positions remain valid without lineage. Foreign seed, source run, missing
  lineage and beyond-selected-boundary all return ForeignLineage.
- `run_state.rs:653`: ArtifactRef::recorded_position returns Unrecorded for None.
  Keep byte-store handles usable without making them predicate evidence.
- `stage/mod.rs:40`: existing StageError::SemanticPosition has From<PositionError>.
  `stage/bindings.rs:87–102`, current loop/subgraph repair and feedback consumer
  already reuse these validators. Do not create another provenance checker.
- PredicateContext already uses borrowed outcome keys and dyn dispatch. Preserve
  both; no boxed error, hidden RefCell/error latch, cloned RunMemory, checked
  context wrapper or parallel generic validation framework is warranted.

| Approach | Encoding, failure modes and future ownership | Decision |
|---|---|---|
| Fallible semantic lookups and evaluator, using existing PositionError | Result keeps refusal distinct from legitimate None and false. Recursive `?` carries refusal through Not/And/Or without changing ordinary short circuit. Core owns interpretation; engine validates only reached semantic reads. Future predicate variants can follow the same explicit contract; new unrelated error domains require a later owning API decision. | **Choose.** Smallest complete current boundary. |
| Engine preflight before unchanged boolean evaluator | A separate scan must replicate recursive demand/short-circuit/arm selection or reject unused invalid inputs. A global memory scan adds unrelated refusal and cost; a checked wrapper must prevent bypass and maintain a second traversal contract. Future variants create two synchronized interpreters. | Reject: duplication and altered valid control flow. |
| Generic associated context error / new PredicateError wrapper | Can support unrelated future runtime failures but expands the public trait, dyn annotation and error conversions now, without any existing second error family. Does not remove need for reached-leaf validation or branch propagation. Three future hypothetical domains are insufficient reason for a framework today. | Reject for this repair; revisit only with a real second domain. |

## Chosen API and exact behavioral contract

Change only the two semantic PredicateContext methods and evaluate:

```rust
fn outcome_of(&self, node: &NodeKey) -> Result<Option<&OutcomeKey>, PositionError>;
fn artifact_size(&self, name: &str) -> Result<Option<u64>, PositionError>;
pub fn evaluate(predicate: &Predicate, ctx: &dyn PredicateContext)
    -> Result<bool, PositionError>;
```

`env_var -> Option<String>` and `file_exists -> bool` stay unchanged. The trait
remains dyn-compatible with no associated type or generic method. Use the
existing core PositionError directly; no duplicate PredicateError taxonomy.
Result itself exposes refusal structurally. No boolean compatibility wrapper or
`unwrap_or(false)`/`.ok()` around a semantic Result is allowed.

Engine outcome lookup: missing map entry or empty history is Ok(None). Select
the same last record as before, validate its seq against memory.semantic_lineage,
then return Ok(Some(&record.outcome)). Do not sort, cast inherited seq to local,
scan unrelated histories, or change the existing stable newest-last contract.
This API consumes the selected record, not every earlier record. Seed validation
owns whole imported history consistency in stage 4; this check is not hydration.

Engine artifact lookup: missing name is Ok(None). For an existing ref, call
recorded_position()?.validate(lineage)? **before metadata**, even if its path
does not exist. Only afterward preserve existing metadata `.ok().map(len)`
behavior as Ok(Option<u64>). Valid recorded artifact plus absent/unreadable bytes
remains ordinary missing data for this predicate; missing production provenance
is Err(Unrecorded), never a metadata fallback. Metadata path/size semantics and
hash verification policy are unchanged; this patch grants no new path authority.

Evaluate reached semantic leaf via `?`, then compare its Some value or use false
for None. File/environment leaves wrap the existing bool in Ok. Implement And/Or
as straightforward ordered fallible loops: return Ok(false) on first false And,
Ok(true) on first true Or; otherwise preserve empty And=true and empty Or=false.
Not returns Ok(!evaluate(child, ctx)?). An Err always propagates; Not never
negates it. An unreached leaf is not consumed and is not validated. Preserve arm
order: a valid matching earlier arm skips all later arms, including invalid data
they would have accessed. Do not eagerly prevalidate all predicates.

Branch calls `evaluate(&arm.condition, &ctx)?`; existing From maps PositionError
to StageError::SemanticPosition. Both append sites stay after successful relevant
evaluation. All-false reached arms may publish default only if each returned
Ok(false). Earlier false arms write nothing, so a later refusal is atomic with
respect to branch publication without a new transaction/batch. There is no await
inside predicate evaluation, borrowed RunMemory is immutable, and no preemptive
OutcomeReported is introduced. Storage errors retain their existing behavior.

## Exact production, test, example and API closure

Required predicate edits:

1. `crates/surge-core/src/predicate.rs`: public trait/evaluate, module and item
   rustdoc distinguish normal absence from typed refusal; MockCtx returns Ok;
   every current bool assertion unwraps/asserts the Result; add faulting context
   controls proving propagation and actual call counts (ordinary existing tests
   named short-circuit do not count calls).
2. `crates/surge-orchestrator/src/engine/predicates.rs`: both implementations,
   current direct outcome/evaluate test callers and provenance/missing controls.
3. `crates/surge-orchestrator/src/engine/stage/branch.rs`: fallible evaluation and
   actual writer-backed no-publication tests, preserving current matching/default
   positives and explicit FixtureHome/writer/storage cleanup.
4. `crates/surge-orchestrator/src/engine/run_task_semantic_tests.rs`: actual
   dispatch/result-resolution closure regressions using private run-task access
   where authentic ordinary journal input cannot represent an inherited record
   yet. Reuse existing fixtures/owners; do not expose a production injection API.

Inspected unchanged callers: production `run_task.rs:1513` uses the unchanged
BranchStageParams/StageResult surface; `benches/stage_transition.rs:115` likewise
uses execute_branch_stage and must compile. `run_task.rs` non-Agent hook guard
and error-resolution pipeline must be exercised, not assumed. Workspace search
found exactly two PredicateContext implementations (MockCtx, EnginePredicateContext)
and direct evaluate callers only in core tests, engine predicate tests and branch.
`surge-core/src/lib.rs:66` and `surge-orchestrator/src/engine/mod.rs:52` expose the
modules. There are currently no executable predicate doctests; the benchmark
contains text command examples, not evaluator Rust examples. Refresh a full
workspace Rust/Markdown symbol inventory before edits; migrate any newly found
external/integration/example/doctest caller in the same dispatch.

This is a deliberately breaking public Rust API change for both trait implementors
and evaluator callers: Option/bool become Result, so external callers must handle
or propagate refusal and distinguish Ok(None). Active development permits this
complete migration, not an infallible alias or delayed caller. Record API-review
findings against the final diff; classify a normal published stable API release
as breaking/major on paper. No release/version bump/publication is authorized.
No wire/event/schema/BranchConfig change is required.

## Authentic RED strategy and independent oracles

Before behavior edits, use current stage 1 types and the existing fallible
execute_branch_stage to add tests that compile now. Assert `.is_err()` and exact
unchanged journal suffix/bytes, not a nonexistent future Result signature. Capture
actual failing assertion output before any predicate production edit. New trait
signature compile failures and faulting mock tests enabled afterward are setup,
not behavioral RED evidence.

Fixed data: local event 7, source event 87, selected through 100, distinct fixed
seed/run identities, and source 101 for beyond-boundary. Outcome and artifact
cases cover: missing lineage, different seed, different source run, beyond boundary;
artifact additionally None production. Keep local and same-seed/run/through-valid
inherited cases as separately reported positives. Source zero is constructor/serde
coverage already owned elsewhere, not an impossible fabricated predicate fixture.

- Writer-backed branch RED: invalid selected outcome that raw-matches selects an
  arm today; invalid nonmatching outcome under Not selects an arm today; invalid
  matching outcome under Not falls through to default today. Each must instead
  Err with no OutcomeReported for selected OR default, and no journal append by
  execute_branch_stage. Include two arms (first valid false, second invalid) to
  prove an earlier evaluation cannot publish partial outcome.
- Artifact RED: existing bytes with invalid provenance must refuse instead of
  selecting by size; missing path with invalid provenance must refuse before the
  existing metadata fallback. Use direct ArtifactSize, Not, and default-fallthrough
  forms so treating invalid as None cannot accidentally pass. Artifact None must
  produce Unrecorded, lineage failures ForeignLineage after API migration.
- Actual caller control: construct branch RunExecutionState with the invalid
  semantic record, dispatch through dispatch_node_stage and resolve_stage_result
  using real writer and graph edge to a downstream trap. Assert failure/no route,
  unchanged cursor/frames/semantic memory, no branch OutcomeReported/EdgeTraversed/
  consumed route/checkpoint or downstream bridge call. Execute normal run-loop
  handling if fixture access supports it; do not claim public cold seeded-start
  acceptance before stage 4. Diagnostic StageEntered/StageFailed/RunFailed and
  existing failure observations are allowed and asserted separately.
- Branch hook control: exercise the actual non-Agent hook guard and assert zero
  HookExecuted/suppress outcome/downstream effects. BranchConfig has no hook
  list; do not fake an Agent node to claim branch hook suppression is reachable.
  Existing ordinary Agent suppression regression remains a positive behavior
  control. No Agent hook-policy trap or policy change is required by this repair.

After the API migration, assert the exact typed errors, plus faulting context tests:
Not(Err) -> Err; And(true,Err)/Or(false,Err) -> Err; And(false,Err) and Or(true,Err)
must never call the second leaf. Test empty combinators, nesting, valid first
branch arm skipping a later invalid arm, genuine missing outcome/artifact false,
Not(missing)=true and genuine missing-data default. File/env leaves and existing
compare operators retain their established controls. Local and valid inherited
lookups must return the same outcome/size; direct branch positives append exactly
one expected OutcomeReported with the fixed outcome, not merely any success.

## Settled scope adjudication and nonblocking Agent limits

Root and the same phase 5a reviewer settled this scope in
`.rust-studio/evidence/bootstrap-owned-continuation/stage1/predicate-and-agent-scope-adjudication.json`
(reviewer `/root/continuation_stage1_spec`, recorded 2026-10-18T01:07:15.760953Z).
The authorized repair is **predicate API plus actual Branch caller only**. Reuse
its existing non-Agent hook guard; no run_task production/error-policy change,
Agent hook-policy change or provider-opening rearrangement belongs in this repair.

The adjudication finds no additional stage 1 blocker from the adjacent Agent
facts. Stage 1 plan D312/314 requires consumer refusal as Err, without requiring
an exact outer Agent StageError variant or zero prior provider opens. B1's stronger
pre-mutation requirements apply to its named memory/fold/route preparation/
projection/inspection/UI boundaries, not every Agent setup step. Explicit failure
received by trusted Agent on_error recovery differs from erasing invalid predicate
data into false/None, where Not/default can silently publish a normal decision.

Retain these two nonblocking evidence limits; this proposal does not claim to
change either:

1. Exact provenance-error typing is not guaranteed at the outer Agent StageError
   boundary: agent.rs:649–651 converts BindingError::Position to Internal text.
   Existing trusted Agent on_error behavior remains within its documented recovery
   policy; predicate refusal cannot reach it because Branch is non-Agent.
2. Feedback/requirement refusal does not guarantee zero prior provider opens:
   their lineage checks are invoked at agent.rs:1374–1383 after session opening.
   No pre-open guarantee is inferred from predicate validation-before-metadata.

Current loop/subgraph entry errors already go to terminal StageDispatch::Failed
(run_task.rs:2220/2253), without hooks and before success cursor/frame updates.
Their final diagnostic stringification does not become a successful semantic route.
These observations account for the inspected siblings; none creates an open scope
fork or a new condition on this proposal's owning pre-code verdict.

## Gates and maintainer handoff

Routine branch evaluation is allocation-sensitive CPU work plus existing metadata
I/O. Selected-record validation is O(1), has no allocation/source scan, and happens
before metadata. Fallible loops retain O(reached predicate nodes) work/stack depth
and current short circuit; no clone/collection/lock/async layer is introduced.
No latency improvement or MSRV execution pass is claimed; existing transition
benchmark should compile, without inventing a new benchmark project.

Before dispatch: independent harsh critic and security owner review this exact
proposal, particularly genuine absence/Not/default behavior, partial publication,
caller-hook guard, API closure and the settled scope above. Root records the
accepted proposal and count before releasing the exclusive builder lease.

Builder records authentic before-fix predicate RED, then exact GREEN and controls;
runs affected core/orchestrator unit/integration tests including ordinary error
suppression, all-target benchmark compilation, doctests, strict default and
all-features clippy across the affected closure, workspace fmt --check and diff
--check under repo timeout conventions. Full stage 1 dependency/platform gates
remain root-owned; this narrow proposal does not replace them or close stage 0's
deliberate continuation RED. No lint waiver, ignored test or schema relaxation.

Owner verdict: **ACCEPTABLE** for the selected predicate API shape and actual
Branch caller closure. Reuse existing types/guards, migrate every public caller,
and preserve short circuit. A strict maintainer should reject boolean swallowing,
an error latch, eager whole-memory validation, a second evaluator, hidden API
shim, unsupported hook trap, or a claim of stronger Agent error typing/provider
opening guarantees than the accepted contract supplies. Root scope adjudication is
complete; no further Agent decision blocks this proposal. Independent pre-code
critic/security acceptance remains required before predicate implementation. Full
stage 1 acceptance and later feature stages remain open until their own reviews
and gates pass. The shared repair counter remains **1/3**; the next authorized
implementation dispatch would be **2/3**, after pre-code acceptance.
