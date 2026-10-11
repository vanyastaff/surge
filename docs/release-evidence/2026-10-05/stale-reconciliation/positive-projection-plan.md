# Conditional positive reconciliation projection fence

Status: independently accepted extension; all three deterministic projection tests
produced runtime assertion RED. The conditional CAS repair is now implemented in
source. The 15-case daemon reconciliation gate and exact mixed429 integration
passed; strict checks and final review remain pending.
`positive-projection-red-receipt.json` records the exact bounded RED evidence.
The earlier error-CAS source freeze and runtime receipts remain a bounded slice.

## Observed source facts

Four positive writes remain shared by explicit and generic launch:
- `prepare_launch`: folded Waiting -> Launched;
- `prepare_launch`: authenticated terminal journal -> terminal attempt;
- `prepare_launch`: already-active engine run -> Launched;
- `dispatch_attempt`: accepted tracked run -> Launched.

All currently call the unfenced `WorkItemStore::settle`. Manual controls can insert a
new SuspendRequested row while the retained run claim exists. A terminal projection
can therefore replace a control inserted after eligibility with Terminal; a Launched
projection can mutate an attempt even though its observed authority was superseded.
The positive contract should compare the same complete authority as error settlement.

Do not overstate the separate fast-suspension hypothesis: in current daemon source,
`confirm_suspension` is called only by `reconcile_terminal`, itself called by
`supervise_completion`. Dispatch spawns that supervisor AFTER its Launched write.
`spawn_tracked_run` observes durable journal/outcome and does not confirm registry
suspension. A fast RunSuspended journal before Launched is possible, but a registry
Suspended-before-Launched production schedule is not yet demonstrated. The earlier
manual RED proves stale-page refusal corruption, not this separate schedule.

## Proposed deterministic RED

Extract a private positive-projection helper from all four actual call sites while
preserving existing blind settlement behavior. It receives the retained claim,
observed attempt/control purpose and closed intended projection.

Using actual Storage and retained claim:
1. Capture eligible Reserved/Launched authority.
2. Submit a real manual Suspend command, which requires no access to the claim.
3. Invoke that same production projection helper using the older observation.
4. Compare complete attempt/control/assignment/wake state. Baseline must fail if
   the projection writes. Include Terminal to expose replacement of the newer
   control; include Launched from Reserved to expose stale attempt projection.

A second registry-boundary case may seal/confirm a real suspension under the same
owner before calling the helper and assert that Launched cannot overwrite it. This
is a controlled projection-order oracle, not a claim that a fast live engine can
currently schedule registry confirmation before the dispatch write.

## Conditional repair after actual RED

Add one narrow persistence method for positive reconciliation projections, sharing
existing error-CAS ownership/comparison mechanics. Input is a closed enum admitting
only Launched, Completed, Failed and Aborted, not arbitrary attempt states. It
accepts the actual retained claim plus complete observed attempt and Option<control>.

In one IMMEDIATE transaction: validate lock, token, full binding; compare complete
attempt/control and eligible state; return typed Superseded without writes if
changed; otherwise perform the exact old positive settlement including terminal
assignment release, item version increment and terminal control transition.

All generic positive call sites use this primitive. Explicit Start/Continue routes
retain existing semantics. A successful provider dispatch is already owned: regardless
of a Superseded projection or a storage error, retain its completion task, move the claim to the existing
supervisor and settle the real outcome normally. Do not cancel the run or release
admission/claim to manufacture a green projection test.

Tests require unchanged-current positive Launched and terminal updates, new control,
same-generation changed state/fence, confirmed suspension, and rejected foreign/stale
claim; compare all authority/wake rows on Superseded. A daemon accepted-dispatch test
must prove ownership/supervision continues when its projection is skipped.

No Cargo until root grants its one-process token. No API extension before this design
is independently accepted; preserve the actual RED before applying the fix. Historical
Capacity failure attribution remains distinct from these deterministic contracts.
