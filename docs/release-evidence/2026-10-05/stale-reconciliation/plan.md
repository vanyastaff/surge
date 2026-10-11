# Stale reconciliation investigation and conditional repair

This record reconstructs the accepted plan after a host reboot removed `/tmp`.
Original reviewed revision: `b2b99b1aefd633277830f989687a78d20a14fb7fec9f9f4788bcde45273a6605`.
It is a reconstruction, not a claim that these bytes have that original hash.

## Evidence boundary

macOS CI at 04c4678 passed 3759/3760 tests. The failing
`planned_host_budget_retains_prior_charge_and_aborts_at_frozen_cap` reached a
confirmed PlannedCapacity suspension at control generation 1, snapshot 23, then
observed Attention at the same generation and fence. Its terminal-journal wait
expired after 30 seconds. The 1500-token sibling and unbudgeted parked-wake sibling
passed. This does not establish a timeout or budget-comparison defect.

The assertion omitted the attempt diagnostic. `WorkItemStore::settle` writes its
reason to the attempt, while changing a control to Attention without copying that
reason. Empty journal diagnostics therefore do not establish absence of a reason.
The temporary raw CI log was removed by reboot, but its committed copy survives at
`../windows-ci-04c4678/surge-macos-ci-04c4678.log.gz`: lines 2950–2966 retain this
exact failure. It was re-inspected after reboot. Run 37714922396 used source
`04c467854de7f4dc3a03de0c9acc915d70cf59f5`.

Hypothesis: a Reserved/Launched attempt page is captured before a suspension and
replayed after the original claim is released. Generic reconciliation acquires a
new claim without rereading launch eligibility. Resume correctly rejects a current
Suspended attempt without ContinueReserved, then an unfenced refusal settlement
changes that newer suspension into Attention. This remains a candidate explanation
of the historical Capacity failure until matching evidence is obtained. A late
Launched write is a separate candidate and must not be conflated with this path.

## Accepted investigation

1. Add bounded test failure diagnostics: attempt state/reason, current control,
   registry status, capacity association/cycle and provider operation counts.
   Preserve the existing 30-second deadline and original assertions. Do not dump
   frozen provider configuration, credentials or raw provider logs.
2. Extract the existing per-candidate reconciliation body into a private production
   helper without behavioral changes. A deterministic test captures a genuinely
   Launched candidate, establishes a newer real suspension, releases the old claim,
   then invokes this exact helper. No sleeps, injected timing or substitute launch.
3. First RED may use manual Suspend, writer-sealed journal/snapshot and actual
   `confirm_suspension`, plus valid accepted startup. This establishes the same
   Suspended guard defect; it does not fulfill PlannedCapacity/budget acceptance.
4. Preserve actual raw RED. A compiler error or interrupted build is not runtime
   RED. The pre-reboot build never yielded a runtime result to the investigator.

## Conditional repair after runtime RED

Generic reconciliation must acquire one actual claim, validate it and inspect the
current attempt/control before startup/workspace/admission/dispatch. Carry that same
claim through dispatch; never check then drop/reacquire. A stale candidate is not
launch authority. Superseded Suspended, Attention and terminal states are benign
no-action outcomes. Preserve current Reserved/Launched recovery, SuspendRequested
interruption recovery, explicit Start and authorized Continue/manual/automatic wake.

Manual control updates can happen while an OS claim is held. Generic refusal writes
must therefore use a transaction that validates the retained claim and compares the
complete observed attempt/control authority, including absent-control identity,
operation, generation, state and fence. Superseded must be typed and write nothing.
All generic unfenced paths must retire together: outer reconcile catch-settle,
prelaunch inspection failure, inner startup validation refusal and dispatch refusal.
No inner blind write may precede the conditional outer handler. Genuine current
errors still become Attention with their actual diagnostic. Keep empty-journal
no-automatic-start behavior and the existing pending-owned-flow exception.

## Independent transaction and behavior tests

- No control at eligibility, then a new control inserted before refusal settlement.
- Same generation/operation, changed state/fence before refusal settlement.
- Unchanged current authority positively writes Attention with the exact reason.
- Rejected stale/foreign claim cannot write; test retained claim validation.
- On Superseded compare complete attempt, control and wake before/after.
- Stale Suspended and Attention candidates preserve their newer state and journal.
- Current eligible recovery and authorized Continue dispatch retain behavior.
- Full PlannedCapacity fixture and original 1000/1500-token budget plus unbudgeted
  parked-wake tests remain required; manual-only RED cannot retire that requirement.

## Execution and scope

Daemon `src/work_items.rs`, adjacent tests, quota integration diagnostics; a narrow
persistence refusal fence and its tests if required. No provider/guardian changes.
No test disabling, timeout widening, retry-green, assertion weakening or fake SQL
suspension authority. Explicit writer/task/pool settlement precedes fixture close.

Only one overall Cargo process, coordinated with root, using
`CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`. Save raw output under ignored
`target/ci-repair-evidence`; copy completed receipts into this evidence directory.
Independent security/spec review accepted the conditional design before code; root
retains the runtime-RED gate and coordinates final review/native acceptance.
