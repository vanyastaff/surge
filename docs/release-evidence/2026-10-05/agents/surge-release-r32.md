# R32 Independent release reviewer — interim NO-GO

Read-only final judge. Inspected requested brief, release ledger, reports R01/R03/R05/R06/R12/R15/R20–R25/R28/R30, and working-tree status. No Cargo commands, repository edits, publication, or user data operations.

## Current verdict

NO-GO pending integrated evidence. This is an interim readiness assessment, not an assertion that every reported defect remains unfixed. Baseline build completed while source/lock changed and therefore cannot prove final source. Several owners have implementation or tests in progress without executed Rust regressions.

## Required evidence before final verdict

- Frozen final source/revision and same-source locked CLI/daemon release build, fmt, strict default/all-feature lint, workspace nextest/doctest totals, MSRV and relevant ignored mock integration receipts. Explicit executed/pass/fail/ignored/skipped denominators; empty ignored tests and service self-skips do not count.
- Independent regression receipts for old per-run database upgrade/future-schema refusal, memory FTS update/delete repair and atomicity, retained admission wakes for completion and rollback, automation completion reconciliation with merge reservation before uncertain external RPC, MCP symlink/hardlink/retained-descriptor capture, Telegram error URL redaction, and daemon socket boundary findings disposition.
- Extracted final ARM64 macOS archive smoke (CLI plus adjacent daemon), isolated durable main flow/restart scenario and complete package/checksum contract. Native Linux x86_64, macOS Intel and Windows x86_64 remain unverified until actual CI/native receipts exist; fixtures for four layouts prove archive logic only.
- Release workflow syntax and just parsing are validated, but actual required reusable CI and four-native build matrix need external execution before multi-platform publication readiness. No publication permission was given.
- Final fresh dependency audit and deny after final lock. R06 audit exits zero with 17 allowed warnings; report must preserve warnings and distinguish policy acceptance from zero security findings. Final API compatibility of git2/Telegram dependency updates needs compilation/tests.
- Durable release/rollback instructions match forward-only schema and complete runtime/project/worktree snapshot boundary. R30 Python SQLite mechanics prove backup primitives, not Rust daemon restoration or productive recovery.

## Scope and limitations to retain

Release deliverable is v0.1.0 CLI/daemon four native archives. Desktop not packaged; native UI/accessibility evidence separately limited. Telegram/GitHub/Linear/real ACP/provider services remain unverified absent authenticated execution. Existing accepted competitive requirements remain open and cannot be retired by agents. Unsupported Loop Replan and nested-loop/subgraph crash placeholder tests need explicit resolved disposition or honest documented exclusion; do not claim all recovery combinations.

## Final decision rule

GO can be scoped to locally prepared artifacts only if final local gates/regressions pass and missing external/native proof is explicitly excluded. GO for the configured four-platform publishable release requires actual four-platform build/archive receipts and mandatory exact-revision CI. Otherwise final verdict remains NO-GO with precise external blockers, while completed local work is still reported.

Awaiting coordinator final integrated evidence for verdict.

## Independent source review follow-up: R29/R26/R11

Source verdict: ACCEPTABLE pending executed Rust regressions and integrated gates. This is a code-review conclusion, not release GO or runtime/security clearance. Read current diffs and complete socket helper; compared Darwin FFI against locally installed Apple SDK sys/acl.h and existing secure_lock validator. No repository changes or Cargo invocation.

### R29 socket publication and process exit

- Other-UID access boundary now precedes bind: opened parent descriptor must be owner-owned; root/current-UID ancestors with writable modes require sticky protection; descriptor chmod0700, dev/ino validation and macOS ACL deny-only check prevent the original chmod-failure fail-open path.
- Existing regular paths and symlinks are rejected rather than removed. Owned socket cleanup retains device/inode and validates retained parent; replacement preservation follows from inspection. Result retained across spawned task allows normal server startup error to propagate exit1.
- Darwin FFI argument/result types and constants ACL_TYPE_EXTENDED=0x100, ACL_FIRST_ENTRY=0, ACL_NEXT_ENTRY=-1 and DENY=2 match local SDK. ACL owns allocated pointer and frees exactly once; borrowed entry remains within live ACL lifetime. Exhaustion cap fails closed.
- Review limits: chmod/unlink and bind remain pathname operations protected by private parent, not descriptor-relative operations against hostile same-UID concurrent mutations. Do not promise same-UID adversary resistance. Linux POSIX ACL effectiveness is constrained by mode0700; macOS extended grants require explicit validator as implemented. Conservative rejection of every ancestor grant can reject harmless ACL grants; this is fail-closed usability behavior, not verified cross-platform support.
- Unexpected server task panic is joined only after shutdown coordination; panic handling is existing process protected-owner scope and should not be credited to startup-error fix. This review does not establish newly complete supervisor panic semantics.
- Needed GREEN: startup bind failure exit1, unrelated file preservation/parent0700, socket guard tests, extended grant rejection and healthy native startup. Native Windows named-pipe DACL behavior unchanged and unverified.

### R26 atomic configuration save

NamedTempFile exclusively creates a unique same-directory inode, write_all/sync_all operate on its retained descriptor and persist replaces destination. PersistError owns tempfile and drops cleanup on mapped error. This removes predictable-name symlink truncation and writer collisions without claiming parent-entry power-loss durability. Review found no blocking defect. Same-UID hostile parent namespace races and preserving arbitrary destination ACL/mode are not promised; new config uses private0600. Needed GREEN: synthetic legacy symlink victim unchanged, concurrency complete configs/no leaked temp files, failed persist original destination retained, saved0600.

### R11 memory FTS migration

UPDATE deletes indexed old values with FTS5 control command and inserts new values; DELETE deletes old indexed values. Migration replaces triggers and rebuilds all four postings within one transaction, then records version3 and commits. Failure uses transaction rollback; no dynamic user identifier enters SQL (fixed internal names). Review found no blocking defect.

Fixture limits: repair test constructs new-schema database marked v2 and directly introduces stale/orphan postings, then restores dummy legacy trigger names. This is an independent damage oracle and useful proof of repair, but not genuine old-v2-schema fixture evidence. Failure fixture checks schema_version and trigger restoration; it does not assert source-row retention or successful later retry. These denominators should remain explicit; full schema compatibility depends on final tests and migration review. SQL integrity command tests are stronger than JOIN matching alone for orphan posting detection.

## Frozen-source L3/ACP/git2/migration review

Quality/spec verdict: NEEDS WORK due to one P1 in L3 merge error classification. Runtime pass counts relayed by coordinator are targeted evidence only; source review does not independently rerun them.

**P1 — published merge error becomes terminal blocked instead of uncertain.** `automation_merge_gate.rs::attempt_merge` reserves MergeAttempted before merge_pr, but Err(e) calls record_blocked with generic 'merge attempt errored'. If notification/comment/label succeeds, record_blocked persists terminal MergeBlocked. already_emitted and reconciliation candidates then suppress that run before recover_interrupted_merge can classify MergeUncertain. A transport error can occur after provider applied irreversible merge, so the user receives a blocked decision without explicit unknown outcome/manual inspection. Required fix: on any unproven merge_pr error after attempt reservation, durably reserve MergeUncertain and emit explicit manual-inspection text, never generic terminal blocked as the sole decision. Add applied-but-error test requiring one merge call, uncertain receipt/manual notification and zero replay after restart.

Other reviewed areas ACCEPTABLE pending full gates:
- ACP config RPC uses handshake_step with same deadline as initialize/session opening, biased cancellation/reply closure and effect fence. Published options never qualify initialize-only retry. Failure reason passes shared redaction before connection cleanup and child settlement. Tests assert publication then cancel/deadline, rather than only object shape.
- git2 0.21 fallible UTF8 access propagates errors rather than silently omitting tracked/status/remotes/worktree entries. Existing missing reflog still becomes ownership conflict; fallible message parsing now retains actual error. SSH/HTTPS capability retained by explicit root features. UI callers changed consistently. Targeted Git 77-pass/2-ignored reported by coordinator; ignored denominator remains explicit.
- Existing-run migration occurs after both writer guards, before pool/task creation. Read-write-without-create plus existence guard avoids creating missing run DB. Future migration IDs refused before pragmas. Error guards unwind automatically. Genuine six-step fixture exercises public writer, old event/stage retention, new session projection writes, idempotent reopening; future refusal checks journal mode/history and released ownership. Reader intentionally does not migrate, so read-only inspection of old schema remains documented boundary.
- Reconciliation keyset paging64 advances persisted-in-loop cursor and wraps; terminal tickets included, explicit terminal receipts excluded. Completed durable journal required; page work sequentially bounded per completion30s. Saturated large ticket history can increase latency (64x30s worst page service), not falsely immediate30s recovery. No new blocking defect established there.

## Repair 1 independent re-review

Prior P1 published-merge-error classification is RESOLVED. `attempt_merge` now routes every merge_pr Err to classify_uncertain_merge. Helper durably reserves MergeUncertain before best-effort notifications, uses explicit unknown external outcome/manual inspection/no retry wording, and does not publish provider error text. Existing MergeAttempted remains replay fence if classification persistence fails. Recovery uses same helper.

Read `/tmp/surge-release-r25-repair1-green.log`: 17 passed, 0 failed, 0 ignored, 0 filtered; includes published_merge_error_is_durable_uncertainty_and_never_replayed, interrupted recovery and rejected storage reservation tests. Future-incompatibility warning proc-macro-error2 v2.0.1 remains separate advisory. No Cargo invoked by reviewer.

Final scoped source quality/spec verdict: ACCEPTABLE. No remaining specific blocking source defect from this review. Whole gates, final build/archive and native platform evidence are still required for release verdict.

## Integrated gate evidence checkpoint

Independently inspected final default Clippy log and latest all-features pass6 log: both end in Finished dev profile and enumerate desktop plus CLI/daemon/dependent crates; no Rust lint failure shown. Coordinator reports both strict workspace/all-target command exit0. Future-compat warnings block0.1.6 and proc-macro-error2 2.0.1 remain and are not erased by strict Clippy success.

Final deny log ends 'advisories ok, bans ok, licenses ok, sources ok'. Final audit log retains '17 allowed warnings found', including rand0.7.3/0.8.5 unsound and yanked chacha20/core2/spin; no zero-security-findings claim justified. Coordinator reports both exit0.

Final packaging suite log: 5 tests, OK, 20.683s; deliberate duplicate ZIP fixture emits expected UserWarning. These are packaging-contract fixtures, not four native executed binaries.

fmt/actionlint exit0 are coordinator command receipts; no standalone corresponding logs discovered in /tmp, so not independently log-verified by this role. R28 prior actionlint receipts recorded separately. Nextest currently running; no final test/build/MSRV/mocks/performance/archive readiness conclusion made.

## Architecture decision: populated MCP cold recovery blocker

Independent review confirms test is an intentionally unmet acceptance oracle, not a stale assertion. Test comments forbid invented closure/coverage upgrades; RESUME repeatedly records genuine populated cold writer-proof RED. Current engine records MCP intents local_effects=false and containers GroupOnly. Observer provides only before_child/child_started, no domain settlement proof. GroupOnly empty process group remains Unknown because escaped descendants/external effects are not covered. Direct leader EOF/reap is insufficient for ExecutionWriterClosed.

Decision: do NOT manufacture Closed, upgrade coverage, ignore/remove test or reinterpret refusal-only success as productive completion. Preserve accepted requirement and failing productive oracle. Narrow honest release disposition is explicit NO-GO on productive cold writer-domain proof while completing independent checks. Productive fix requires continuous descendant/effect containment with retained authority and typed journal closure; helper callback alone cannot supply missing evidence.

Additional safety investigation required: R15 actual journal shows suspension cleanup_confirmed=true followed new cold MCP birth despite unresolved GroupOnly intents. rg found no WriterLiveness/ExecutionWriterClosed consumer in daemon/orchestrator production paths. If wake treats direct settlement as domain cleanup, minimum safe fix is fail-closed Attention/refusal before any new effect on unresolved domain. This would improve shipped safety but keep productive oracle and accepted requirement open; not green readiness. Await R15 source trace of seal authority for specific fix boundary.

### Pre-code decision: truthful MCP suspension plus cold guard

Expanded R15 plan ACCEPTABLE: after registry shutdown and before seal, replay fresh authoritative journal (memory can miss out-of-band intents); unresolved/conflicted/missing/unknown/readerror domain evidence sets pending fence cleanup_confirmed=false with structured diagnostic. Independently checked control.rs: false maps control and attempt to Attention. Cold resume separately checks authoritative prior writer evidence before registry mutation or new effects, protecting historical true fences.

Allow only genuine consistent explicit closure or fully covered Gone+local_effects=true; current GroupOnly/false does not qualify. Conflicts always refuse. No fabricated Closed/coverage. Add independent actual negative false-fence Attention/no new spawn/provider effects test, retain original productive positive RED and accepted requirement open. This is safety correction, not productive completion or final release GO.

### Final MCP safety source re-review

R15 production diff ACCEPTABLE for scoped false-authority safety fix. Verified execute shuts registry down then replays fresh storage before sealing; refused/read-error observation changes fence to false and emits diagnostic. Engine guard runs after claim/active checks and before clear_parked/open writer/wake effects. Validator conflicts first, requires established container even for explicit closure; otherwise Gone and local_effects=true mandatory. Current GroupOnly/false refuses; no closure emitter or coverage upgrade added.

New actual negative test requires GroupOnly journal, false suspension fence, unchanged MCP/provider physical records after cold refusal, and durable Attention. Original productive positive is unchanged. JSONL helper only consumes newline-complete records and still fails on complete corrupt records; test confirms both. Runtime GREEN awaited. This review resolves scoped unsafe authorization design, not the open productive-cold requirement or release gate failures.

### Typed suspension refinement supersedes false-fence proposal

Runtime exposed seal_suspension invariant: writer.rs517 refuses cleanup_confirmed=false. Prior source review missed this downstream contract. Revised plan ACCEPTABLE: on unresolved/read-error domain, skip suspension seal entirely and set outcome through existing recovery_required, preserving typed cleanup-true invariant. Helper appends RunRecoveryRequired and returns RecoveryRequired; existing work_items supervisor maps Attention. Helper can return outcome after failed append with diagnostic, so durable-event claim requires actual successful journal assertion, not source promise.

Negative oracle must observe committed RunRecoveryRequired and Attention, no true cleanup fence, no new MCP/provider effects on maximum-time cold scheduler retry. Original productive oracle remains RED. Consumer historical guard retained. Earlier false-fence endorsement is superseded; no invariant weakening approved.

### MCP typed refinement final source and GREEN receipt

Independently read current run_task producer: unknown/readerror sets outcome=recovery_required and skips seal; only clean branch seals original cleanup-confirmed fence. Writer closes afterward. No typed invariant relaxation.

Independently read `/tmp/surge-release-final-mcp-domain-green3.log`: unresolved_mcp_domain_requires_attention_and_blocks_actual_cold_dispatch PASS; 1 passed, 0 failed, 0 ignored, 21 filtered, 1.02s. Test asserts genuine GroupOnly establishment, committed RunRecoveryRequired, absence of any confirmed RunSuspended, unchanged physical MCP/provider records after cold refusal, final Attention. Scoped safety verdict ACCEPTABLE, backed by this one actual negative. Previous false-fence compile/runtime failures remain historical failures; not credited as pass. Full suite and productive positives remain outstanding and cannot be inferred from 1/22 selected test.

## Controlled MCP/approval smoke and full-suite receipt

Controlled fixture final diff ACCEPTABLE: Registry::for_run(config) supplies runtime availability, catalog baseline uses render_with env failures and fresh empty capacity. Inherited artifact bindings cleared only for self-contained system-prompt smoke. Complete checkout HEAD/diff/status oracle unchanged.

Independently read `/tmp/surge-release-controlled-green.log`: child controlled_daemon_mcp_smoke 1 passed/0 failed/0 ignored/3 filtered; parent reports same selected test, not two distinct scenarios. Durable SessionOpened1, HumanInputRequested1, HumanInputResolved1, StageToolReceipt2, OutcomeReported1, RunCompleted1, RunFailed0. This proves controlled mock transport plus human input scenario, not authenticated provider service or productive cold recovery.

Independently read final-nextest-integrated3 log: 3840 executed, 3830 passed, 10 failed, 36 skipped; process reports test run failed. All ten failures in owned_flow_mcp_recovery; eight are ~20s fixture setup waits and two direct policy/resume failures. Safety refusal can make previously expected confirmed-park fixtures fail, but failures remain failures and accepted requirements stay open. Current full release verdict necessarily NO-GO regardless passing controlled smoke.

# R32 FINAL INDEPENDENT VERDICT — NO-GO

Reviewed canonical evidence README, final-results.json, frozen artifact manifest, final nextest, actual archive smoke and command log, archive validation/linkage receipts. Independently recomputed archive SHA256 and checked git diff from frozen source checkpoint. No Cargo, product edits, publication or user-data operations by reviewer.

Frozen input revision: 3136b8268cb33009305acf114ff6cc075bbc3b8d. Working diff from checkpoint at review contains only docs/AGENTS/THIRD_PARTY content; final optimized candidate is ARM64 macOS CLI+daemon. SHA256 recomputed exactly 88569c9a783f26296cb31cd16c5b47f6db53c8d35a74201e2f32d9e659573e6b.

## Blocking reasons

1. Required full workspace nextest FAIL: 3840 executed, 3830 passed, 10 failed, 37 skipped; exit100. Ten enabled owned_flow_mcp_recovery acceptance cases remain failures. Safe RecoveryRequired/Attention containment is proven, productive MCP cold recovery is not. Preserve original productive requirements and downstream oracles; do not retire or call completed.
2. Configured four-native release set and exact-revision remote required CI are unverified. Local execution is macOS27 ARM64 only; minOS15 Mach-O metadata/system-only linkage does not establish macOS15 runtime compatibility. Linux, Intel macOS and Windows execution remain external evidence requirements.
3. Complete third-party license/copyright/NOTICE bundle remains unverified. Permitted-license cargo-deny success is not license-notice collection proof. Actual archive Apache terms/static OpenSSL attribution checked; this does not settle all linked dependencies.

## Verified preparation outcomes

- Strict workspace Clippy default/all-features and fmt pass; MSRV1.96 check passes for workspace excluding UI. UI MSRV is not covered by that receipt.
- Deny passes; audit exits0 with17 allowed warnings retained. Future incompatibility warnings remain; no zero-advisory claim.
- Doctests5pass7ignored, ignored mock ACP2/engine3/MCP3, real daemon restart2, controlled daemon MCP/approval1 selected parent-child scenario pass. Authenticated providers/trackers/Telegram remain unverified.
- Optimized static-OpenSSL ARM64 CLI+daemon build, strip, package contract5tests, archive5members/full gzip/source Apache bytes and extracted system-only linkage/minOS15 checks pass.
- Extracted archive actual isolated E2E passes init/describe/two terminal flows/persisted replay/restart/clean source+unchanged HEAD. Reviewed command log includes actual daemon PID/socket settlement. Prior failed fixtures preserved; corrected context commit/config ignores and stop/start settlement preserve scenario oracles.
- Source fixes independently ACCEPTABLE: durable L3 pre-RPC uncertainty/no replay, ACP bounded shared config handshake, Git fallible API propagation, guarded legacy migration, descriptor config/MCP capture and private daemon socket checks. Targeted scoped evidence does not override failing full gate.

## Performance and limits

Final-source bench log at review completed analysis: transition estimate224.19–233.83 microseconds, Criterion says no statistically detected change (p0.07). Coordinator command exit/budget receipt still to be appended; estimates are not the sampled p95 statistic. No benchmark result can override acceptance failures. Native desktop accessibility/real UI operator E2E and all real external service delivery remain separately limited.

## Release/rollback disposition

Prepared candidate may be retained for local review, not published as release-ready. Follow docs/release-procedure.md once blockers are resolved and publication separately authorized. Rollback is complete quiescent pre-upgrade runtime/project/Git/worktree snapshot restoration with prior binaries, not database down migration; backup drill validates SQLite mechanics only and cannot undo post-snapshot external effects.

Final NO-GO is independent and remains valid even if benchmark subsequently passes. No user requirement retired, no test disabled to produce green, no push/merge/publication inferred from local commit authorization.

## Final performance receipt

Coordinator observed final benchmark command exit0 with SURGE_STAGE_TRANSITION_BUDGET_CHECK=1; canonical final-results/README updated. Source confirms enabled budget samples64 after warmup and enforces p95<=5000us. Together enabled command+successful receipt establishes local budget PASS. Criterion estimate224.19–233.83us is a separate statistic; comparison p0.07 does not establish improvement. Hardware is local ARM64/macOS27 only, not cross-platform performance acceptance. Final NO-GO unchanged for acceptance/native/notice blockers above.
