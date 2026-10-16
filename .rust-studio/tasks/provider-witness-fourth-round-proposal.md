# Bounded native ACP witness — fourth repair

Current status: authorized round **4/4**, local code verified; independent Phase 5a ACCEPTABLE and all full Phase 5b lenses COMPLETE. Installation remains pending. Human mandate and executed checks are recorded below. The observer has not been installed and no live-provider/native run has occurred. Root independently matched the three final source hashes and the tested helper hash.

## Original pre-authorization proposal (historical)

Status: PROPOSED ONLY; repair budget remains **3/3 exhausted**. This is the same
provider-witness task, not a replacement task or an assumed budget extension.
Explicit human authorization is required before implementation, building,
installing/replacing the fixture binary, configuration changes, or native runs.
This document is the sole authorized write. No Rust or runtime changes were made.

Source inspected: results-planning-history worktree, supplied HEAD `03a7ed20`.
Read-only diagnosis used the existing habit-tracker fixture journal via SQLite
`mode=ro` and metadata captures; no provider transcript, RPC, or auth action.

## Problem and independent product defect

The native AppTester candidate at child run
`run-01M538BSKHWQTW2EHQ2QSHH74A`, event 109, was rejected at event 113 by
`profile-artifact-contract:verification-report`. Its legitimate correction turn
then failed at event 116 with incoming transport closed for `session/prompt`.

`tests/support/live_provider_capture.rs:139-153` permits one prompt only;
`examples/acp_smoke_proxy.rs:84` closes when admission rejects. This is deliberate
for the one-turn smoke test (`exact_selection_permits_one_prompt_on_same_session_only`,
support lines 789-797), but cannot witness ordinary correction turns.

The AppTester report contradiction must be corrected independently and remain
visible: its profile requires a file but tells the agent to submit only inline;
non-authoritative outcomes currently discard that inline report. A witness repair
must neither accept that candidate nor change verification authority, report
validation, sandbox, stage receipts, or ledger effects. See the separately scoped
AppTester evidence-contract task. Native acceptance remains failed/incomplete;
neither proposal establishes completion of both milestones/eight tasks.

## Bounded policy and limit decision

Keep `ProtocolCapture::new` and `with_mode` as default one-prompt constructors.
Add an explicit native policy with fixed cap **6**, selected only by a reviewed
boolean opt-in proxy argument `--native-multi-turn`. Do not expose an arbitrary
CLI maximum or infer native mode from
`--initial-provider`, `--capture-dir`, provider name, or destination path.
Existing positional command/argument forwarding must remain compatible.
Parse only leading proxy options, before the provider program; never consume
provider flags as proxy flags. Reject duplicate/unknown proxy options and malformed
argument layouts before spawning the provider. Default constructors and invocations
without this flag retain the fixed cap 1.

Six means **initial turn plus at most five subsequent turns**, not five repairs
per stage and not evidence that the turns were corrections or reminders. The proxy cannot
authenticate the engine's purpose for a prompt without a new trusted contract;
it must label its evidence as bounded protocol observation only.

Four is not a complete allowance for every default legal engine trace:
`agent.rs:1158-1160` uses `limits.max_retries` (default 3 in
`surge-core/src/agent_config.rs:236`), while `agent.rs:1434-1456,2006` separately
permits two missing-outcome reminders. A trace needing both can require six
prompts. Therefore **4 is insufficient** and the recommendation is the explicit
fixed cap 6, covering initial + three default outcome corrections + two independent
missing-outcome reminders. This remains bounded protocol observation, not a claim
to support every configuration or continuation. Higher configured retries remain
outside the policy and fail truthfully with `prompt_limit_exhausted`; no count reset
or automatic limit increase is permitted.

## Minimal state machine

The existing `Pending` map correlates initialization, new-session and model
selection requests, but prompts currently return before correlation. Extend that
same test-only oracle; do not add a second transport, receipt backend, or daemon
authority surface.

1. Preserve initialization, exact categorical model selection, version-pinned
   legacy metadata consistency, offered-model validation, selection epochs and
   delayed-ack fencing. Initial-provider and explicit-ack modes keep distinct
   selection requirements. No model substring/prefix matching or fallback.
2. Native handshake is a lifetime state machine: uninitialized, initialize pending,
   initialized, opening pending, opened. Admit exactly one `initialize` request
   for the entire witness lifetime; reject repeats in every phase, including before
   the first prompt. Its uniquely correlated valid success response is required
   before admitting exactly one `session/new`. Initialization error/malformed
   response terminates observation; a duplicate/delayed initialize response cannot
   replace adapter identity or legacy-model interpretation. Reject opening before
   initialization success, overlapping/repeated openings and prompts before opening
   success. On the opening response, freeze provider-session identifier and opening
   epoch for the witness lifetime. Native mode rejects a subsequent
   `session/new`, `session/load`, `session/resume` or `session/fork` RPC;
   no new opening may reset prompt count or restore admission. Reject repeat
   initialization in every phase as an attempt to replace adapter evidence.
   Before the first prompt, native mode accepts exactly one `session/new` opening;
   it rejects load/resume/fork rather than pretending to prove restored sessions.
   All later opening requests are rejected before forwarding, even with the same
   session ID. See the closed method inventory below; unknown extension methods
   are rejected in native mode, not passed through as supposedly safe openings.
3. For every prompt, require current adapter metadata, same frozen session and
   opening, offered required model, and still-valid current exact selection.
   Explicit mode requires a current acknowledged selection. Initial mode permits
   current initial proof until invalidated; it must not relabel initial evidence
   as an acknowledgement. Same-model updates do not resurrect an invalidated
   selection without a fresh correlated acknowledgement.
4. Validate and reserve every native client request identity before changing state
   or forwarding, including initialize/new/mode/config and prompts. Keep a private
   lifetime reservation table with fixed capacity **64**, bounded IDs (existing
   256-byte maximum), method kind and pending/completed state. Never evict or reuse
   an identity after result/error. Duplicate IDs and reservation exhaustion reject
   before forwarding, with safe enum reasons. This is an observation budget, not
   a claim of arbitrary configuration-churn support. Preserve the existing separate
   pending-request limit 16; reserving an ID never bypasses it. Normal notifications
   without IDs do not consume reservations; malformed request/notification shapes
   fail closed. Provider-request IDs occupy a distinct namespace: client replies
   to them neither consume client reservations nor acknowledge client requests.
   Native mode permits exactly one outstanding prompt. Its bounded pending record
   binds reserved identity, original opening/session and turn ordinal.
5. On a provider response, distinguish requests/notifications before matching
   response IDs. Only a matching prompt response with exactly one of `result` or
   `error` closes that pending RPC. Reject malformed/duplicate prompt responses;
   an uncorrelated response cannot reopen admission. Treat unknown unrelated
   response IDs according to the existing transparent oracle policy, but never
   credit them as prompt completion. A response for any completed client reservation
   (including initialize/new/config) is a duplicate and rejects before forwarding;
   it cannot overwrite proof or accidentally acknowledge a reused prompt ID. Retain
   only the bounded 64-entry lifetime identity table, not response bodies.
6. A correlated error is a completed **RPC response**, not a successful turn.
   Record success/error response counts separately; do not parse/retain the raw
   provider error. A subsequent serial retry can be observed if its model/session
   proof remains valid and the cap permits it. A result may record only a known
   bounded stop-reason enum or an `other` marker, never untrusted free text.
7. Relevant model-option updates and every selection acknowledgement must validate
   the unique model option, its original option identity/category/type, exact current
   value, bounded well-formed offered choices and consistency with any supplied
   version-pinned legacy metadata. Refresh the offered-model set from that validated
   current metadata; never rely on the opening's old offer list after an update.
   Removing the required choice invalidates proof even if currentValue stays exact.
   Missing/malformed/ambiguous offers or inconsistent model-option identity invalidate
   proof rather than preserving a stale selected_model. Unrelated well-formed
   thought-level options do not manufacture or invalidate model proof by themselves;
   an ambiguous model-related update fails closed. A same-model update cannot
   restore invalid proof: restoration requires a fresh uniquely correlated valid
   selection acknowledgement, with current offers. Detect model invalidation during
   a prompt as failure of that turn's exact-model
   proof; terminate native observation rather than allow later re-selection to
   retroactively validate it. Do not change model/session while a prompt is
   outstanding. Preserve strict stale opening/selection acknowledgement fencing.
8. Increment admissions monotonically only after all admission checks pass; record
   correlated response counts separately. Cap exhaustion rejects before forwarding.
   EOF with an outstanding prompt records incomplete observation. Neither EOF,
   response count, nor `stopReason=end_turn` certifies process cleanup, stage
   acceptance, `StageOutcomeCommit`, task verification or overall run completion.

Do not simply replace `prompts != 0` with `prompts >= max`: that would allow
overlapping prompts, recycled IDs and uncorrelated response-based retries without
proving the original session or a still-current selection.

### Closed native client-method inventory

Actual bridge wrappers are `surge-acp/src/sdk_v1.rs:187-263`; the wire-method
assertions are at lines 594-599. Opening dispatch in `bridge/worker.rs:479-496`
uses `session/new`, or continuation via `session/resume` with `session/load`
fallback. Fork is not emitted by those wrappers, but the pinned ACP 2.2.0 request
mapping includes `session/fork` (`src/schema/client_to_agent/requests.rs:55`), as
well as new/load/resume at lines 34,41,56-60. Do not confuse unsupported-by-bridge
fork with an opening the proxy may transparently permit.

Native client requests with a method are restricted to `initialize`,
`session/new`, `session/set_mode`, `session/set_config_option`, `session/prompt`
and the `session/cancel` notification, with the phase/fencing rules above.
`session/load`, `session/resume`, `session/fork` are explicitly rejected before
forwarding in every phase of this fresh-session-only policy. Legacy model methods
`session/set_model`, `session/setModel`, `_session/set_model` remain invalidating
and rejected in native mode; they cannot supply an acknowledged selection.
Any other client method, including underscore extensions and unknown session
methods, gets `unsupported_client_method` without forwarding or printing its name.
Client replies to provider-initiated requests have no method and continue through
unchanged; provider request IDs use their separate namespace and never acknowledge
client prompts. Preserve provider notifications/tool requests and normal ACP replies.

`session/cancel` must refer to the pinned session and does not itself close the
pending prompt or certify cleanup; the matching response or existing shutdown
owner settles it. `session/set_mode` before a prompt conservatively invalidates
model selection and requires a subsequent exact correlated model proof; during an
outstanding prompt it is rejected. Model-changing config requests follow the same
invalidation rules; no same-model update may manufacture fresh authority. Unknown
client methods fail closed, so this policy deliberately makes no promise about
future bridge/adapter extensions. Default one-prompt smoke forwarding compatibility
must remain unchanged. Pure tests enumerate this closed native inventory.

## Metadata and safe failure publication

Extend the existing bounded `Capture` snapshot with backward-compatible defaults:
policy kind/limit, prompt admissions, correlated successful/error responses,
pending boolean, and a safe enumerated rejection code plus direction/turn ordinal.
The existing `prompts` field retains its admission-count meaning. Pin/epoch and
bounded lifetime request-ID reservations stay private state; do not publish raw IDs or
frames just to aid debugging. Existing bounded model/session metadata is retained.
Accepted adapter/model/session metadata identifiers are intentionally retained;
privacy does not promise that an alphabet-valid sentinel submitted in an allowed
metadata field is absent. Raw prompt/output/error/tool data, request IDs and
credentials are excluded from persisted diagnostics; no untrusted data is copied
into safe reason enums. Tests use provenance-specific sentinels to check this boundary.

Safe reasons include limit exhausted, overlapping prompt, repeated request,
wrong session/opening, model proof unavailable/invalidated, stale acknowledgement,
malformed prompt response, repeated opening and incomplete prompt on EOF. Error
strings, arguments, prompt text, tool data, provider output/stderr, paths and
credentials must never populate reason fields.

On admission/provider rejection, write the safe snapshot through the already
opened `EvidenceFile` before breaking and closing transport. If publication fails,
fail closed and explicitly classify diagnostics as unavailable; never forward the
rejected frame or falsely claim a durable reason. Preserve cleanup even when
diagnostic publication fails. A fixed safe stderr enum may supplement an existing
stderr consumer, but must not replace durable capture and must not print raw errors.
Do not emit unsolicited diagnostic frames on ACP stdout or manufacture provider
responses. Retain existing directory ownership, no-follow/create-new behavior,
Windows retained-control-file checks, frame bounds and bounded readers. Enforce
the snapshot size limit before write on every platform, using bounded fields.

The snapshot is observation, not a crash-proof append-only journal: current Unix
publication rewinds/truncates the file. Do not claim atomic durable publication
under arbitrary process termination. Reader failure remains diagnostic unavailable.

## Actual files and consumers

| File | Proposed change or invariant |
| --- | --- |
| `crates/surge-daemon/tests/support/live_provider_capture.rs` | Explicit policy, serial prompt correlation/fencing, bounded metadata/reasons and deterministic tests; preserve ownership/security helpers. |
| `crates/surge-daemon/examples/acp_smoke_proxy.rs` | Opt-in argument, policy construction, safe failure snapshot before existing teardown; retain framing, forwarding and cleanup. |
| `crates/surge-daemon/tests/live_provider_smoke.rs` | Native synthetic transport coverage and capture schema compatibility; keep default smoke assertions at exactly one prompt. |

The support module is included by both the example (line 2) and integration test
(lines 4-5). Existing smoke launch arguments at lines 394-400 do not opt in.
`project_capture` at lines 193-211 and `check_provider_capture` at lines 514-540
require one-prompt explicit-ack proof: both consumers must additionally require
explicit strict policy and cap 1, including parent assessment around lines 227-229.
A native-policy capture with prompts=1 must fail strict smoke assessment. Old
captures deserialize with explicit serde defaults strict policy/cap 1; mixed or
inconsistent policy/limit combinations fail validation. Do not relax smoke checks
to accept native captures. Fixtures constructing `Capture` through the default snapshot around line
1504 need schema-default coverage. Windows compiles the same support into both
consumers; run strict clippy for both paths without temporary suppressions.

Persistent habit-tracker instrumentation/configuration is outside implementation
file scope. After approval and successful gates, replacing its binary/arguments
requires an explicit recorded fixture update and hash/source identity; no silent
substitution of a different observer or regeneration of accepted run evidence.

## RED then GREEN proof plan (not executed)

- RED: extend the existing pure `ProtocolCapture` tests to demonstrate that a
  correlated first result followed by a distinct same-session exact-model prompt
  is rejected by the current implementation. Keep the existing default second-
  prompt rejection test green throughout.
- GREEN: native initial-provider and explicit-ack tests admit exactly six serial
  prompts and responses; seventh is rejected before forwarding; counts do not reset.
  Initial + three corrections + two reminders is supplied explicitly, without
  inferring semantic correctness from the protocol observer.
- Negatives: overlap; lifetime reused ID after result/error, including completed
  initialize/new/config ID reused by prompt and completed prompt ID reused by config;
  delayed duplicate handshake responses; 64-reservation exhaustion before forwarding;
  collision with pending config
  ID; wrong session; repeated new and load/resume/fork in every phase; unknown
  client methods/underscore extensions; repeat initialize in every phase, opening
  before initialize acknowledgement, stale initialize metadata; mode changes during
  prompt; mode change without renewed exact proof; delayed opening/selection reply;
  uncorrelated/duplicate/malformed response; result+error; response notification
  with a matching ID; stale or wrong model, removed offered model, unsupported or
  conflicting legacy metadata; same currentValue with removed required offered choice,
  malformed/ambiguous offers or changed option identity in updates AND selection ACKs;
  no proof resurrection by same-model updates; model update during pending prompt; cap overflow;
  EOF mid-RPC. Provider-to-client request IDs must not complete client prompts.
- Privacy: inject separate sentinels in raw prompt/output/error/credential/request-ID
  fields and assert they never appear in persisted capture or diagnostics. Valid
  bounded adapter/model/session metadata is intentionally retained; malformed
  metadata is rejected, not mirrored into errors. Verify bounded
  metadata, accepted identifiers, malformed/oversized capture and publication
  failure handling. Assert structured reason precedes observed closure when
  publication succeeds, without claiming ordering after a failed write.
- Wire: extend existing `--fixture-wire` and lifecycle-test seams only if needed,
  using a bounded provider marker/count independent of oracle counters. Prove
  six admitted frames arrive unchanged, rejected seventh/overlap/replay never
  arrives, RPC responses stay intact and closure records a safe reason. These
  wire negatives include handshake-to-prompt and prompt-to-config ID reuse,
  delayed duplicate handshake responses, repeat/out-of-phase initialize/opening,
  and model-update/selection-ACK removed-choice, malformed/ambiguous/identity-change
  cases; prove rejected frames do not reach the opposite endpoint. These
  fixture changes are part of the fourth repair, not a free new harness round.
- Default compatibility: unchanged one-prompt smoke proof/arguments, strict mode
  never downgraded, initial mode exact proof and version-pinned legacy checks,
  old-capture strict/cap1 serde defaults, rejection of native-policy capture even
  with one prompt by BOTH strict consumers, inconsistent-policy rejection,
  capture read compatibility and all existing adversarial tests.
- Settlement: rerun existing proxy EOF, partial-frame, overflow, cancellation,
  held descendant-pipe, watchdog and Windows Job/security cases. No changes to
  `OwnedProcess`, cleanup timeouts, retained ownership or watchdog semantics are
  expected; any necessary change expands scope and requires renewed review.

After approval, run focused non-provider tests first, build the example, then
explicit existing ignored fixture/lifecycle tests requiring that binary. Run
`cargo fmt --all -- --check` and `cargo clippy -p surge-daemon --all-targets -- -D warnings`
plus the relevant integration tests. Record exact commands/results and source
revision; include Windows native results for Job/security claims. Run no live
provider smoke or native app acceptance until separately authorized fixture/run
actions are ready. A green synthetic suite alone is not native acceptance.

## Review questions and acceptance boundary

Review disposition (proposal only; independent focused re-review pending):

| Finding | Disposition and concrete design |
| --- | --- |
| PW-P1 | Accepted: lifetime reservation of all native client request IDs in a fixed 64-entry table; fail closed on reuse/exhaustion; completed handshake responses cannot acknowledge new work; provider IDs separate. Pure and wire cases added. |
| PW-P2 | Accepted: exactly one initialize lifetime, valid acknowledgement before exactly one fresh opening; repeated/out-of-phase requests and delayed duplicate replies reject without overwriting adapter proof. |
| PW-P3 | Accepted: project_capture and check_provider_capture explicitly require strict policy/cap1; old-capture serde defaults strict/cap1; native prompts=1 is insufficient. |
| PW-P4 | Accepted: update and selection-ACK validation refresh current offered choices, identity/category/type and consistency; removed choice/malformed/ambiguous metadata invalidates, including during pending prompt; same-model update cannot resurrect. |
| Privacy clarification | Accepted: bounded allowlisted metadata identifiers are intentionally retained; raw protocol bodies, request IDs and credentials are not published. Tests assert this precise provenance boundary. |

Independent harsh/security review must assess the recommended fixed-six policy
and closed method inventory, verify all request-ID namespaces and update invalidation
paths, and check publication failure still reaches existing cleanup. It must also
verify that default smoke is unchanged and no metadata claims stage completion.

Approval, if granted, authorizes one additional **fourth witness repair round**
with this reviewed dependency closure; count stays 3/3 until that explicit grant.
Do not rename the task, reset the counter, bypass the observer, change provider
credentials, loosen exact-model checks, disable validation, or grant AppTester
verification authority. Keep the AppTester root-cause correction independently
tracked and tested. Report native failures transparently even after witness tests
pass; both milestones/eight tasks remain open until their real completion evidence.

## Independent proposal verdicts — implementation not authorized

Initial review of e7fbf0eb8db4af3b53660d05a70d2389702dfa1449ba3aebbe9902eab71a99b2 was RESHAPE NEEDED: PW-P1 all native request-ID lifetime correlation, PW-P2 initialization phase/identity, PW-P3 smoke policy identity, PW-P4 current offered-model proof. Revised proposal body 6a2aa33ace187e9142670adc19aa5fc497b7ce411905d8a33aae976feb167d8e passed focused independent harsh-critic and security re-review: ACCEPTABLE, all four resolved, no introduced plan regression. These verdicts approve only the concrete proposal; witness repair budget remains 3/3 exhausted until an explicit human fourth-round mandate. No Rust, runtime, configuration, build or provider actions were authorized or performed by this plan review.

## Explicit human fourth-round mandate and dispatch

2026-10-16: human answered «Да, разрешаю четвёртый раунд» to the request to implement this reviewed native-six-turn plan, execute tests and update the retained test-project observer. Shared provider-witness repair count recorded BEFORE dispatch: **4/4**, previous3/3 exhaustion retained. This is the same task, not a reset. Source/destination independentworktree codex/results-planning-history at2923e96cac2774b74bd9e58bcf03152fb891f0de; the three witness source files are unchanged from inspected03a7ed20. Reviewed proposal body6a2aa33ace187e9142670adc19aa5fc497b7ce411905d8a33aae976feb167d8e with harsh/security ACCEPTABLE and PW-P1–PW-P4 resolved.

Author scope three listed witness files plus necessary existing deterministic fixtures/consumers and this record/evidence; real dependency expansion requires concrete evidence and renewed plan review. Preserve strictsmoke policy1 and all proof/cleanup gates. Authentic controlled multi-turn RED before production edits, then pure/wire/privacy/cleanup regressions, source-built actual helper freshness, applicable strict default/all-feature lints/fmt and source manifests. Independent affected5a BEFORE full5b. Fixture installation occurs only after successful checks/reviews with exact source/hash/retained recipe identity; root owns native fixture mutation. No live provider run through invalid terminal bootstrap recovery, source journal alteration, credential changes, sandbox/verificationreceipts or Home repairs. No push/PR/merge authorization inferred. Full product goal remains open.

## Fourth-round author BUILD handoff — independent review pending

Source base `2923e96cac2774b74bd9e58bcf03152fb891f0de`; reviewed human mandate remains **4/4**, with no fifth dispatch or scope reset. Local source author has finished; independent Phase5a and full5b are still required before installation/commit.

Implemented only the three reviewed witness files. Native opt-in is fixed6; strict smoke policy stays1, including legacy serde defaults and BOTH smoke consumers. Native guard retains all64 client IDs with method kind/completion state, one initialized/fresh opening, one outstanding prompt, current exact-model/offer/session/epoch fences, correlated result/error counts, safe rejection metadata and EOF/framing failures. Full same-model config updates containing mode/thought/collaboration options remain transparent while a prompt is pending. A standalone current_mode_update conservatively requires new proof; it is not evidence the model changed.

Native wire fixtures independently hash actual received frames, witness the provider identity before first client request and confirm its absence after settlement. Six serial frames remain unchanged; rejected seventh/overlap/replays, pending initialization/opening,64 lifetime/16 pending budgets and invalid ACK/update replies are withheld. Failed rejection publication retains the last successful snapshot and emits only fixed capture_unavailable; cleanup still settles actual provider owners. Existing container/watchdog/retained-home guards remain intact.

Exact already-merged upstream `6722b4952adcff12a532665863606a40093dee25` same-file baseline fixes were verified via `git diff a2ba09c5 origin/main -- crates/surge-daemon/tests/support/live_provider_capture.rs`: Unix-only mutable Command shadow and Windows-only owner drop in an existing fixture. These do not change process semantics or grant a new Home repair round.

Evidence: `.rust-studio/evidence/provider-witness-fourth-round/final-manifest.json` contains exact three source hashes, actual proxy/mock artifact hashes and paths, final command log hashes, provenance and retained failures.

Final executed gates (all exit0): default nextest219/219 and all-feature nextest219/219 (16skipped each), example tests35/35, explicit controlled ignored tests15/15 with live_codex skipped, both default/all-feature daemon all-target strictClippy, workspace fmt-check and git diff-check. Fresh final example and controlled mock builds use actual CARGO_TARGET_DIR/auth-required-diagnostic, not the shared dangling target/debug symlink.

Authentic initial RED: red.log runs native_serial_second_prompt_is_observable_after_correlated_response, 1failed on second serial exact-model prompt after first correlated result, before production edits. Initial exact-filter0 run is not counted as RED. Other setup/lint failures and the real Unix bound failure remain recorded in logs; all assertions retained.

Not performed: live codex-acp/Sol61 call, fixture installation/configuration/runtime/journal changes, raw failed-run resume, Windows execution, native app acceptance, commit/push/PR/merge, memory writes. Habit-tracker full goal remains incomplete. This is BUILD-ready for independent reviews, not a COMPLETE verdict.

## Independent Phase 5a result

witness_fourth_spec_review ACCEPTABLE, no blockers. All final source/helper/gate-log hashes independently matched final-manifest.json. PW-P1–PW-P4 resolved; actual RED1failure, final219/219 in each feature configuration with16skipped, example35/35 and controlled ignored15/15 with paid live Codex excluded. Scope includes only three daemon witness files and already-merged same-file Windows baseline. Root separately matched all three source and tested proxy hashes. Full5b generic/domain/security dispatched only after5a. Repair remains4/4; fixture installation and live native execution remain pending.

## Independent Phase 5b closure

witness_fourth_quality, witness_fourth_domain and witness_fourth_security each COMPLETE after independent5a. No blockers or code advisories. All three independently matched final source/helper/gate-log hashes. Security advisory PW-S-A1 retained as an evidence boundary: local synthetic tests and observed provider settlement do not establish native Windows, global descendant closure, passing verification receipts or full live native acceptance. Those remain requirements of the full goal and are not relabeled complete.

Local code stage COMPLETE, same authorized4/4 count. Root may commit this dependency closure and install the exact reviewed helper at a new retained versioned path with an explicit scoped recipe change. Original helper/recipe, source failed journal and existing paid captures remain preserved. No additional repair or publication authority follows.
