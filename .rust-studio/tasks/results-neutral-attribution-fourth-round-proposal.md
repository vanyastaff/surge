# Results: neutral styling for unavailable planning attribution

Status: proposed narrow fourth repair; no implementation dispatch or authorization.
Results dispatch budget remains 3/3. Home/Inbox's separately authorized sixth
round does not extend Results. Source remains the reviewed six-file closure
`c7193d6ac803123f235b7b11ec87d94a221edb2c79ee8828e0a05b46b1375a1f`
against `c2001306`. No source, Cargo, provider or runtime mutation in this proposal.

## Evidence and disposition

Independent 5a ACCEPTABLE; generic 5b COMPLETE. Domain 5b COMPLETE with advisory
RPH-R3-DOM-A1; security 5b NEEDS WORK with blocking RPH-R3-SEC-1, P2. Both identify
the same actual mapping. Root accepts RPH-R3-SEC-1 as blocking RPC-03's neutral
attribution requirement, retaining the domain severity disagreement explicitly.

`planning_execution` calls `stage_step(stage, None, ...)`, which defaults to
Lane::Build. When graph/node attribution is unavailable, it appends a caveat but
keeps that lane. `lane_color` maps Build to Semantic::Agent; step_chip uses that
color for its rendered border/tint. theme.rs documents Agent as an agent doing
the work, and External as including unknown. Label, tooltip, state and caveat
remain factual; no execution authority, completion or approval is granted.
Nevertheless, this known semantic color claims a role the graph cannot establish.
Existing gap/broken-predecessor tests explicitly assert the Build fallback.

## Bounded selected change

Keep existing shared `stage_step` default behavior for implementation untouched.
Within planning composition, use the existing neutral Lane::Result fallback,
which the renderer already maps to Semantic::External. Authenticated graph-node
classification may then replace it with the existing Plan/Approve/Result mapping.
No new lane, theme role, classifier, public API, graph acceptance or role heuristic.
Do not infer agent role from stage names or absent session metadata. No change to
the graph lineage, entry identity, labels, state, sessions, caveats or observations.

The existing live styling branch bypasses lane_color: Working uses accent and
Waiting uses warning. Therefore Lane::Result alone is insufficient. Preserve the
existing graph-attribution result explicitly in the internal PlanningStep
presentation record (a small named evidence enum or equivalent explicit field,
derived from that same graph/node lookup, never a second classifier). The planning
renderer passes unavailable attribution into the actual shared chip style owner.
For that case, final border/background and any role-colored activity cue use
Semantic::External even while the exact planning observation is live. Live state,
indicator presence, exact identity and factual text remain unchanged. Authenticated
planning Agent/HumanGate and ordinary implementation use the existing style
branch unchanged. Do not infer unavailable attribution from Lane::Result alone,
which also represents authenticated external nodes.

Use one small production style decision consumed by step_chip for final border,
background and applicable indicator color. Its planning-only override carries
known/unavailable provenance; ordinary implementation callers retain their exact
existing behavior. This internal presentation-record/caller migration is part of
the repair closure, not a global lane or theme change. No new public external API.

Write zone: mission.rs plus its adjacent tests; run_mission.rs and existing Results
tests only for a necessary renderer regression. No app.rs/Home/Inbox, scheduler,
receipts, persistence authority, provider observer, native fixture or runtime.

## RED and verification

Before the production fallback change, preserve and strengthen the existing
missing-config, gap and invalid-predecessor controls to expect the neutral lane.
Each must reach the actual planning projection and fail on the current Build
fallback. Keep exact occurrence identity, state, caveat and all original assertions;
replace the two assertions that currently encode the defect with required neutral
expectations, explicitly recording that reason.

Add a complete-prefix missing-node stage with no agent session and an exact live
parent observation, alongside the offline/gap/broken-predecessor cases. The actual
planning Step must retain Working and valid current activity while its role is
unavailable; neutral color must not be achieved by changing it to Unknown/offline.

Add a renderer-boundary check using the actual projected planning Step and the
same final style owner consumed by step_chip, for available=false and true,
including Working/Waiting branches. Its independent oracle is Semantic::External's
documented final border/background color, with an explicit inequality against
Semantic::Agent. Assert any role-colored indicator follows the same neutral
policy while preserving indicator presence. Do not add
an unused test-only mapping or accessibility claim that the actual style ignores.
Prefer an existing GPUI rendered-style observation when available; otherwise
trace and assert the exact shared color value passed to the real border/tint
builder and retain native rendered-chip identity/state/caveat assertions. Review
must confirm the test exercises the production style path, not a duplicated oracle.

Authenticated Agent and HumanGate positive controls must retain their existing
styles and factual labels. Ordinary implementation fallback remains unchanged.
Refresh affected mission/Results/native UI tests, strict UI default/all-feature
all-target Clippy, owned-source formatting and diff checks. The initial 330 UI +5
integration/25 inspection receipts remain historical until affected gates refresh;
unchanged persistence tests need not repeat absent a new affected seam.

Then independent focused 5a followed by affected generic/domain/security 5b. Keep
the original full repair evidence, findings and current count; do not restart a
broad review or reset the task identity. No Windows/live-provider/product claim.
Local commit only after all blockers/current gates close; no push, PR or remote
merge is included.

Root maintainer pre-code recommendation: ACCEPTABLE pending independent focused
critic/security review. Existing UI owners and neutral role are reused, no new
dependency/unsafe/allocation or public surface. The smallest correct closure is
planning-only fallback plus actual style regression. Explicit user fourth-round
authorization is required before author dispatch; no automatic fifth.

## Focused pre-code reshape

First focused critic/security review: RESHAPE NEEDED, RPH-R4-PLAN-1 and
RPH-R4-PLAN-SEC-1. Both identified the live border/background branch bypassing lane
color. The selected design above now explicitly includes planning-only attribution
provenance and the final actual style branch, with complete-prefix/exact-live
missing-node coverage. No code or tests have been changed. Independent re-review
and user authorization remain required; Results stays3/3.

## Independent revised-plan verdicts

Focused critic and security re-review: ACCEPTABLE. RPH-R4-PLAN-1 and
RPH-R4-PLAN-SEC-1 resolved at design level. Actual final style branch, complete
missing-node/exact-live negative and all authenticated/implementation positive
controls remain binding. Root requested explicit one-round Results extension;
response pending. No implementation dispatch, Results remains3/3. Home/Inbox
sixth-round implementation may proceed in its independent source zone meanwhile.

## Explicit revised mandate and fourth dispatch

User answered: "Да, разрешаю четвёртый раунд Results". Root records dispatch
count **4/4** before author handoff, extending the same Results task by exactly
one round. No automatic fifth, no borrowing from Home/Inbox, no publication.
Implement only the reviewed neutral-attribution/style closure above. Current
source is the frozen third-round six-file closure c7193d6a against c2001306 plus
documentation records. Root will bind the actual handoff to the next local
documentation checkpoint. All original findings, severity disagreement and
successful/failed evidence remain retained. Actual sixth Home author has not
been handed off and waits for this narrow Results closure.
