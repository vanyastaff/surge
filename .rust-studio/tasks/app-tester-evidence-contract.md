# AppTester evidence contract

Status: repair 1/3 BUILD ready for affected 5a, then focused 5b; full review mode. Source base `03a7ed20`.
Repair count: 1/3; SEC-APP-001 documentation repair dispatched to app_evidence_builder. This new product defect is
independent of the exhausted provider-witness repair budget (3/3), which is not
reset or extended by this task. No publishing authority is inferred.

## Actual failure and cause

Native acceptance root:
`/Users/vanyastafford/Develop/surge-acceptance/habit-tracker-sol61-20261016`.
Planning run `run-01M538BSKHNKR64K4C8TC3X3Z6` completed at seq113.
Implementation `run-01M538BSKHWQTW2EHQ2QSHH74A` completed milestone_spec and
the m1-t1 Build, then AppTester submitted an inline report at seq109.
Seq113 rejected it with `profile-artifact-contract:verification-report`.
The subsequent correction turn met the existing one-prompt witness guard;
the run failed at seq117 with incoming transport closed. The old run remains
failed; no journal edits or synthetic success/resume are allowed.

The bundled AppTester declares a required `verification-report.toml` file, but
its prompt prohibits writing that file and promises inline host sealing.
`agent.rs` repeats inline-only instructions whenever verification input exists,
including non-authority task stages. Acceptance correctly seals inline reports
only for ledger verification effects; AppTester has none. Its required file is
therefore missing. The native candidate also put later-task skipped checks in a
passed report; the current validator correctly rejects such a report.

## Selected design and maintainer verdict

Maintainer pre-code verdict: ACCEPTABLE for independent plan review.

Restore the existing evidence-file contract for AppTester. Keep its role
without verification authority and its outcomes without Verified ledger effects.
Reuse the existing core report schema, file validator, contained artifact capture
and host-binding rejection. An unbound AppTester report is evidence for the
downstream verifier; it is never TaskVerified proof. Do not borrow the sealed
verifier path, accept missing files, relax validation, or add a receipt backend.

Crate ownership: surge-core owns bundled role guidance and the shared artifact
contract vocabulary; surge-orchestrator owns emitted session guidance. Persistence,
ACP transport, scheduler and authority types are unchanged. No new dependencies,
public API, unsafe code, migration, or new ecosystem behavior is needed. This is
a cold prompt-construction path; no performance claim is made.

Update AppTester to write its declared relative report and list it in
artifacts_produced, without binding. Current-scope checks must satisfy the existing
passed-report rules. Later-task omissions belong in narrative evidence notes,
not non-passing checks in a passed report. Retained report/screenshots use relative
workspace paths rather than ephemeral external /tmp paths. Preserve source-edit
prohibition and the distinction between evidence and verification.

Make shared injected guidance follow declared verification effects. Sealed
verifier nodes retain inline-only/no-file/no-agent-binding instructions. Other
task stages receive current accepted criterion definitions and are told to follow
their declared profile artifacts and outcomes, without an invented inline report
requirement or forced passed outcome. Reuse the existing verification-effect
predicate/owner; do not duplicate selection by profile-name string or expose a
behavior-selecting boolean API. Keep verification input observation/sealing and
all outcome acceptance rules unchanged.

Clarify existing report type documentation if necessary: unbound audit/evidence
files do not carry authoritative verification; only the existing sealed verifier
path can produce TaskVerified evidence. Do not introduce a compatibility shim.

## Acceptance and RED evidence

Native seq109 -> rejection113 is genuine outer failure evidence. Before behavior
edits, add and execute a focused test against the actual emitted AppTester prompt
that detects the conflicting inline/no-file instructions while keeping host
criterion IDs available. Keep an actual sealed-verifier prompt control case.
Do not claim already-supported valid file acceptance as a newly failing test.

Use existing engine stage/ledger fixtures to demonstrate retained valid AppTester
passed and failed files, no TaskVerified events, and rejection of inline-only
missing files, malformed reports, forged bindings and a passed report containing
skipped checks. Verify containment and source/artifact identity through existing
seams. No test should implement a second acceptance algorithm or weaken oracles.

Run scoped core/orchestrator tests including real stage/ledger integration,
fresh required mock helpers if that integration needs them, strict all-target
Clippy with default and all features, formatting and diff check. Broaden only for
affected boundaries or findings. Independent 5a must precede full 5b review with
quality/domain/security lenses. Record actual commands and failures honestly in
durable ignored evidence, not /tmp. Local commit after reviews only.

## Out of scope and remaining goal

No witness, provider config, provider RPC, original native runtime/project,
scheduler, sandbox, verification receipt, host binding or gate-policy changes.
The witness still cannot support legitimate multi-turn validation; extending it
requires a separately reviewed concrete proposal and explicit human fourth-round
mandate under review-repair.md. Removing this product contradiction does not
repair that limitation or prove full native acceptance.

After fixes, continuing the same project must use a legitimate owner operation
with retained failed history. Terminal Failed is not NeedsAttention and bootstrap
Retry currently refuses it. Do not raw-resume or rewrite the terminal journal;
inspect the supported continuation/attempt path before the next provider launch.
Two complete milestones, follow-up history, cold recovery, existing-project
acceptance, native UI findings and current platform gates remain open.

## Independent plan review

Harsh-critic: ACCEPTABLE, no redesign required. Binding implementation conditions:
keep guidance selection and begin() Verified predicate under the verification
module owner; test actual prompts for AppTester, implementer, Verified verifier
and a renamed/custom profile; fix both inline/file and later-task skipped-check
contradictions; retain cannot_run and honest current-scope failure reporting.
Include bundled profile and affected archetype mock closure, whose broad
artifact superset/always-inline behavior must not mask the real file contract.
Do not modify outcome_acceptance production guards without separate evidence.
Security/domain: ACCEPTABLE. Existing report type reuse is appropriate; correct
its verifier-only documentation rather than adding an unnecessary artifact type.
Preserve cannot_run without a mandatory report, report failed enum semantics,
no authority/binding on evidence, and all validation/capture guards.

Required RED fixture detail from security review: run_stage_steered currently
creates frames only when an inline report is present and starts empty RunMemory.
Non-verifier begin() also needs a remembered source-bound subject. Seed genuine
accepted current criteria and prior subject in the isolated fixture, assert the
criterion IDs actually reach the emitted SendPrompt, and inspect that prompt.
Do not let missing verification input make the RED vacuous. Include implementer,
renamed/custom role and sealed-verifier controls.

Both independent pre-code gates ACCEPTABLE. Implementation is within the active
user goal; no witness or provider launch is authorized by this plan. Initial
product builder dispatch is authorized with repair count still 0/3. Full 5a then
quality/domain/security 5b and affected gates remain mandatory before commit.

## Builder execution chronology (2026-10-16)

Evidence directory: `.rust-studio/evidence/app-tester-evidence-contract/`, ignored
locally by root in shared git info/exclude. No native/provider launch, terminal
journal access, commit, push or memory write by builder.

1. Read complete approved plan, root AGENTS/CLAUDE, project base rules and
   rust-studio dev-task instructions. No scoped AGENTS/CLAUDE below crates found.
2. Added isolated accepted task fixture before behavior edits. Existing default
   fixture remains unchanged: the new mode creates real loop acceptance criteria,
   remembers a fingerprint/checkpoint, and requires IDs to appear in the actual
   BridgeFacade send_message prompt before instruction assertions.
3. `cargo test -p surge-orchestrator --test engine_task_ledger_test
   task_prompt_follows_declared_evidence_contract -- --nocapture`:
   initial `red-prompt.log`: 0/4 passed; tester/implementer/custom showed the
   actual contradiction, verifier had a fixture checkpoint reference collision
   (seed seq0 overlapped begin seq0). This setup error is not product RED.
4. Fixed only isolated seed reference to seq999; repeated same command BEFORE
   production edits. `red-prompt-corrected-fixture.log`: 1/4 passed (sealed
   verifier control), 3/4 failed (tester, implementer, renamed custom tester),
   each with criterion:1 present. This is the authentic emitted-prompt RED.
5. Changed bundled tester file/narrative guidance, existing report documentation,
   verification-owned shared guidance and agent callsite. Extracted/reused exact
   existing Verified declaration predicate; input capture and sealing unchanged.
6. Same focused prompt command after edits, `green-prompt.log`: 4/4 passed.
7. `cargo test -p surge-core --lib profile::bundled -- --nocapture`,
   `core-bundled.log`: 16/16 passed, 941 filtered.
8. `cargo test -p surge-orchestrator --test engine_task_ledger_test -- --nocapture`,
   `ledger.log`: 28/28 passed. New coverage includes valid passed/failed file
   capture with stored/source byte/hash identity and no TaskVerified; six
   rejection cases (inline only, missing, unlisted, malformed, forged binding,
   passed plus skipped); cannot_run without file; sealed failed inline control.
   Valid file acceptance was already supported and is not claimed as new RED.
9. Applied cargo fmt. Full affected unit and selected integration/strict lint
   gates follow; final receipts/hashes will be appended before review handoff.

All Cargo commands use the authorized target and environment:
`CARGO_TARGET_DIR=/Users/vanyastafford/.codex/worktrees/results-planning-history/surge/target/auth-required-diagnostic`,
`CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`,
`CARGO_PROFILE_TEST_STRIP=symbols`, `CARGO_BUILD_JOBS=2`, login shell disabled.

Archetype mock closure inspection: eight bundled examples in the existing
archetype suite contain no AppTester stages. The mock supplies a broad artifact
superset and always-inline audit, so its green result cannot prove AppTester
file behavior. The strict ledger fixture provides that evidence through the
actual stage validator/capture path instead; archetypes remain a regression gate.

10. `cargo test --locked -p surge-core -p surge-orchestrator --lib`,
    `unit.log`: core 957/957 and orchestrator 573/573 passed, no ignored cases.
11. `cargo test --locked -p surge-orchestrator --test engine_task_ledger_test
    --test engine_agent_artifact_emission_test --test archetypes_mock_test`,
    `integration.log`: respectively 28/28, 12/12, 2/2 passed, none ignored.
    All these fixtures use freshly source-built in-process MockBridge; they do
    not consume external mock helper binaries. No helper rebuild is necessary.
12. First strict affected all-targets/all-features Clippy failed on builder's
    added nested test assertion (`clippy-all-features.log`). Extracted a test
    identity assertion helper, without any lint suppression. Clarified cannot_run
    vs failed prompt wording (startup failure vs inability to check a criterion
    after application start). This is initial builder work, not review repair.
13. `cargo clippy --locked -p surge-core -p surge-orchestrator --all-targets
    --all-features -- -D warnings`, `clippy-all-features-final.log`: PASS.
    Default-feature equivalent in progress. Final source-revision tests are
    repeated after the last prompt clarification/helper extraction.

## Terminal builder handoff

14. Default strict lint `cargo clippy --locked -p surge-core -p surge-orchestrator
    --all-targets -- -D warnings`, `clippy-default.log`: PASS.
15. Final immutable source: `cargo test --locked -p surge-core -p
    surge-orchestrator --lib`, `unit-final.log`: core 957/957 and orchestrator
    573/573 PASS, none ignored/filtered. Same three integration targets repeated,
    `integration-final.log`: archetypes 2/2, artifact emission/containment 12/12,
    task ledger 28/28 PASS, none ignored/filtered.
16. `cargo fmt --all --check` (`fmt-check-final.log`) and `git diff --check`
    (`diff-check.log`): PASS. Both strict Clippy configurations passed without
    suppressions. No source edits after these gates and final source hash capture.

Command-session handles: first RED 64426, corrected RED 32420, focused GREEN
22931, first ledger 36097, first unit 9895, final unit 25107, first integration
43258, final integration 42556, first Clippy all-features (failed test nesting)
55936, final all-features 28657, default Clippy 93045. All are terminal.

Source HEAD: `03a7ed20ea0d38c25fefd9918289ea4d9d494fbf`.
Final tracked source diff SHA-256:
`ecfea76312fbdf0a72d16b3d1bd1b2468fd2460e125b327218be034fece14deb`.
Exact six-file hashes are in durable ignored `source-hashes.json`; `source.diff`
contains the reviewable patch. Test receipts are local macOS evidence, not native
provider acceptance or Windows/Linux gates.

BUILD is ready for independent spec compliance 5a. Full quality/domain/security
5b must follow a passing 5a. Overall task is not yet complete: independent reviews
and any required review repairs remain open; local commit remains deferred to
root's post-review decision. Product review-repair dispatch count remains 0/3.
Provider-witness budget remains exhausted 3/3 and untouched. No code in
outcome_acceptance, classifiers, sandbox, ACP, persistence, daemon, native UI,
provider harness/auth/env/configuration or process signals was modified.

## Independent review progression

Phase 5a app_evidence_spec_review: ACCEPTABLE, no blocking or advisory findings.
Reviewer matched current six-file diff ecfea76312fbdf0a72d16b3d1bd1b2468fd2460e125b327218be034fece14deb, all source/log hashes, criteria-bearing actual-prompt corrected RED (1 control pass/3 failures), and final 1530 unit +42 integration gates. No production guard changes or scope creep. Full 5b generic/domain/security dispatched only after this verdict. Repair count remains 0/3. Native full goal and platform CI remain open.

## Review repair 1 — SEC-APP-001

Recorded BEFORE dispatch: shared repair count 1/3. Prior reviewed source base03a7ed20, six-file diff ecfea76312fbdf0a72d16b3d1bd1b2468fd2460e125b327218be034fece14deb. Same source/destination worktree. Generic 5b COMPLETE; domain initial COMPLETE; security NEEDS WORK with SEC-APP-001 blocking: shared VerificationReportOutcome enum/variant docs still claim sealed-verifier-only and automatic ledger transitions (roadmap/verification.rs90–101), contradicting unbound evidence and the approved documentation requirement. Fix docs only; no predicate, schema, serialization, acceptance, authority or native changes. Appropriate formatting/schema checks and new frozen manifest; independent affected 5a then focused 5b required. Witness remains3/3 exhausted; nativefullgoalopen.

### Repair 1 builder execution

SEC-APP-001 corrected only the `VerificationReportOutcome` enum and variant
Rust doc comments. They now describe observations in application evidence,
audits and sealed reports; the value alone has no ledger effect. TaskVerified
requires host-accepted sealed verification; FailedVerification status requires
the corresponding declared, host-accepted effect. No derives, type shape,
serde attributes, variants, serialization, predicates or behavior changed.
JsonSchema may embed corrected prose as description metadata; no schema shape
or version changed.

`repair-1/doc-only-proof.json` reconstructs the exact prior file from only the
three changed doc blocks and matches its frozen SHA-256, then proves all code
lines and the other five source files unchanged. Old source/log manifests and
receipts remain preserved. No behavioral RED is invented for a doc correction;
the authentic earlier emitted-prompt RED remains the behavior-change evidence.

Repair checks using the same authorized Cargo environment:
- `cargo test --locked -p surge-core --lib`: 957/957 PASS (`repair-1/core.log`,
  command session34007), including report validation/roundtrip coverage.
- `cargo test --locked -p surge-core --test artifact_schema_snapshots --test
  bundled_profile_metadata`: 8/8 +2/2 PASS (`repair-1/schema-profile.log`,
  session44522).
- `cargo fmt --all --check` and `git diff --check`: PASS, repair-specific logs.
- Complete affected strict all-targets lint is repeated in both configurations
  because derives can consume doc descriptions; all-features session23969 is
  running, followed by default features.

Prior orchestrator 573/573 and three integrations 42/42 were executed against
the pre-repair source freeze ecfea763...14deb. They are retained behavior
evidence, not newly rerun tests on the repair hash. Exact doc-only proof limits
this repair's effect to descriptive metadata. Review repair count stays 1/3;
no additional repair dispatch and no witness/native/provider work occurred.

### Repair 1 terminal BUILD handoff

Complete affected strict all-targets/all-features Clippy PASS (session23969),
default-feature equivalent PASS (session7251); both repair-specific logs retained.
Core957/957, schema8/8, profile2/2 and fmt/diff checks PASS. No source edits after
these checks. Old manifests/logs are unchanged; repair-1 contains new source and
log manifests and the cumulative review patch. New cumulative diff SHA-256:
`17a3f7cdc58eab37e08450a111c9a151e54397ad5e14b88e86cc157ecee15988`.
Changed report file SHA-256:
`0aeabbb6e706d00a70339b256c5f605cbd3e3f947f72c5630be945439a830089`.

Cargolease released. Ready for independent affected 5a BEFORE focused 5b. This
builder verdict does not close review: SEC-APP-001 requires independent recheck;
repair count remains1/3. No commit, push, memory, native or provider action.

## Final independent closure and local completion boundary

Repair1 affected5a app_evidence_spec_review ACCEPTABLE before focused5b. Generic app_evidence_quality, domain app_evidence_domain and security app_evidence_security each COMPLETE; SEC-APP-001 resolved, no outstanding blockers/advisories. Domain corrected its original COMPLETE to NEEDS WORK for the same documentation finding before re-review; both verdicts retained. Root independently matched all six final source hashes. Final cumulative source diff17a3f7cdc58eab37e08450a111c9a151e54397ad5e14b88e86cc157ecee15988. Repaircount1/3.

The original behavior freeze passed1530 unit+42 integration tests, both strict feature sets/fmt/diff. Final doc-only repair passed957 core+8 schema+2 profile tests and both complete affected strict feature sets/fmt/diff. Earlier behavioral tests were not rerun or relabelled on the doc-only freeze; schema descriptions may change while shape/serialization/code stay unchanged. Actual emitted-prompt authenticRED and finalGREEN retained. Local product task COMPLETE; stage-sized local commit authorized. No push/newPR/merge inferred.

Full user goal is NOT complete: actual habit tracker stopped after first implementation stage and AppTester failure; both milestones/eight tasks, follow-up, existing-project journey, cold native continuation, UI findings and current platformCI remain open. Terminal original failure is preserved. Current private full snapshot validated four SQLite databases and source file/mode/link inventories outside /tmp. The separate provider witness proposal is reviewed ACCEPTABLE but budget3/3 exhausted, no fourth implementation authority. No sandbox/verificationreceipt task restarted, no memory write.
