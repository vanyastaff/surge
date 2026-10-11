# R15 security boundary audit

Status: NEEDS WORK. Read-only review; no cargo builds/tests run (coordinator requested build serialization). Dependency advisories belong to R06.

## HIGH: MCP diagnostic capture follows attacker-controlled links

`crates/surge-mcp/src/connection.rs:849-862` chooses `<cwd>/.surge/mcp-stderr/<server>.log`, or predictable `temp_dir()/surge-mcp-stderr/<server>.log` for daemon probes. `stderr_forwarder`, lines 875-911, follows directories via create_dir_all/set_permissions and opens existing leaf with create+truncate+write and no O_NOFOLLOW. `StderrRecords::publish` line 815 and final suppression write line 942 reopen pathname. A malicious repository containing .surge/mcp-stderr symlink/leaf can cause truncation of any file writable by launching user; predictable shared Linux /tmp fallback can be precreated by another local user. File chmod follows symlink too. Merely mode 0600 at creation does not prevent following existing links or changing target permissions. Same-user process pathname replacement is also relevant to retained writer ownership.

Concrete isolated system-call semantic reproduction performed with Python tempfile: create victim containing valuable bytes, symlink capture.log to victim, os.open(capture.log, O_CREAT|O_TRUNC|O_WRONLY, 0600), chmod(capture.log,0600). Output:
```
Equivalent OpenOptions(create,truncate,write,mode=0600) followed symlink: True
Target length after open: 0
Target mode after chmod: 0o600
```
This verifies OS primitive exploitability, not production-path integration (still required).

Existing tests at connection.rs:1174/1181 assert capture path scope only. No symlink/hardlink safety test found for stderr capture. Strong descriptor-relative protection already exists in persistence owned_flow/private_files.rs and should inform fix.

Minimum fix: acquire capture file once through retained descriptor-relative O_NOFOLLOW directory chain. Validate private capture directory owner/mode; use per-user fallback and reject unsafe preexisting directories. Open leaf without truncate, with O_NOFOLLOW|O_NONBLOCK; validate regular type/current owner/nlink=1 before descriptor chmod and set_len. Retain descriptor for every write, no pathname reopen. Regression tests independently construct file and directory symlinks/hardlinks, assert target bytes unchanged; swap leaf after acquire and assert replacement gets no writes. R16 reviewed this plan and returned ACCEPTABLE.

## MEDIUM: daemon socket authorization setup fails open

server.rs:190-215 binds first, then chmod0600. chmod failure logs warning and keeps listener serving despite code documenting socket as only authz boundary. Metadata failure silently skips chmod. No peer UID validation found. Under permissive umask/shared socket parent, clients can connect between bind and chmod and existing established connections remain valid. Typical user-owned 0700 daemon runtime parent reduces exposure, but configurable path can remove it.

Minimum fix: validate private socket parent before bind, enforce owner-only bind permissions atomically where practical; fail startup if socket permission enforcement fails; authenticate peer UID where supported. Coordinate with R14 authorizer before overlapping server changes.

## Existing defenses reviewed

IPC read_frame_bytes_capped enforces 8MiB while consuming chunks before allocating remainder; it avoids the basic unbounded-line allocation defect. MCP minimal_child_env clears broad inherited credentials and keeps explicit runtime allowlist. Persistence private object Unix path handling uses descriptor-relative nofollow validation and includes malicious links tests.

No claim of whole-workspace security clearance. Public parser fuzzing, Windows DACL verification, dependency audit, and final regression gate remain delegated/unverified here. No durable memory exception/waiver discovered.

## Implementation checkpoint

Coordinator authorized direct implementation under R15 (fixed 32-role team). R16 pre-code plan verdict: ACCEPTABLE. Changed `connection.rs`, added private `stderr_capture.rs`, added existing workspace nix dependency to MCP Unix target. Capture now opens descriptor once, checks nofollow directory ancestry after trusted cwd/temp base canonicalization, owner/non-writable runtime ancestor, private capture directory, regular leaf/euid/nlink, descriptor 0600 before truncate. All subsequent writes target retained descriptor. Daemon temp capture uses per-euid namespace. No process/stdio protocol modifications. On non-Unix disk capture is fail-closed optional diagnostic functionality; stderr continues draining with safe trace categories and explicit warning (documented limitation to include in release docs). Rustfmt succeeded; Rust builds/tests remain unverified pending coordinator. Added leaf symlink, hardlink, directory symlink victim-preservation tests and path-replacement retention test. R23 independent diff review requested.

Independent final source review R23: ACCEPTABLE (before final awaited-I/O refinement); added runtime-ancestor symlink and positive permission tests per review. Publication now uses tokio::fs::File converted from retained descriptor with awaited rewind/write/flush/truncate; no path reopen, avoiding per-record blocking filesystem work. Setup descriptor open remains synchronous once per connection. No cargo evidence available yet. Regression evidence must still be captured by coordinator.

## Additional P1 Telegram revoked chat disclosure

R13 confirmed outgoing cockpit cards used fixed configured admin chat without current pairing admission. Coordinator assigned R15. Precode independent root verdict ACCEPTABLE: private generic wrapper in cockpit production boundary rechecks PairingsAdmission immediately before each send/edit network delegation. Shared instance covers emitter/startup/lag reconcile/snooze; pairing replies stay raw bot exemptions. Existing PairingsAdmission query errors must propagate Persistence instead of unwrap_or(false). Already-authorized in-flight request is not retroactively canceled by later revoke.

Three actual Storage+fake API regressions authored: send/edit pair→revoke→repair; reconcile denied after revoke preserves card pending then reparing edits/closes; registry lookup failure denies with Persistence. Production wrapper composed, intentionally pass-through until RED captured. Coordinator's serialized RED attempt blocked by unrelated surge-git/git2 compile failures before test binary. Log `/tmp/surge-release-r15-red.log`; this is NOT behavior RED evidence. Implementation gate still pending tests compilation.

Telegram RED retry compiled test binary successfully but fixture setup failed `SingleThreadedRuntime` before behavior (`/tmp/surge-release-r15-red-retry.log`). Corrected all three tests to multi_thread worker_threads=2 matching production Storage contract. Setup failures are not accepted as behavioral RED. Retrying via R26 exclusive cargo queue.

Telegram behavioral RED confirmed by R26: 0 passed, 3 failed, exit101 `/tmp/surge-release-r15-red-behavior.log`. Failures reached actual unpaired transport delegation, revoked reconcile outgoing edit, and swallowed registry lookup errors. Guard now applied: current allowlist immediately before send/edit, Auth denial; persistence query errors propagate Persistence. Production same wrapped instance covers card emitter/reconcile/snooze. No public API changed. Pairing route replies remain exempt. GREEN requested from R26.

Telegram card guard GREEN executed by R26: 3/3 passed `/tmp/surge-release-r15-green.log`. Independent R23 final source verdict: ACCEPTABLE for sensitive CARD admission; emitter/reconcile/snooze composition confirmed. R23 additionally identified command-reply admission check occurs before awaited status/list reads then raw send_reply may send after concurrent revocation. Minimal second plan to recheck regular replies and reserve raw replies for /pair submitted coordinator; not yet implemented/verified.

Reply extension precode independently ACCEPTABLE by root and R23. Added exact production private reply seam with local wiremock Bot endpoint tests. Meaningful RED executed R26:1pass1fail (`/tmp/surge-release-r15-reply-red.log`), revoked command response reached unchecked send. Implemented current-admission check immediately before ordinary send network request; `/pair` success/error explicitly use private pairing-only raw helper. Registry lookup errors failclosed Persistence; test strengthened to assert no extra HTTP request after dropping pairing table. Synthetic unpaired pairing result remains usable. GREEN requested R26; no live Telegram used. Checked teloxide failure privacy: existing TelegramCockpitError::From<RequestError> discards source payload and reports generic Bot API failure; relevant production/callback logging already uses this safe conversion. Existing regression covers token marker non-disclosure.

## Final targeted evidence

All runs executed serially by R26 coordinator slot; logs inspected R15:
- MCP secure capture:4/4 passed `/tmp/surge-release-r15-mcp-green.log` (leaf/directory/runtime ancestor symlink rejection; hardlink victim preservation; owner permissions; descriptor after pathname replacement).
- Telegram outgoing cards:behavioral RED0/3 then GREEN3/3 `/tmp/surge-release-r15-red-behavior.log`, `/tmp/surge-release-r15-green.log`.
- Telegram command replies:behavioral RED1/2 then final GREEN2/2 including registry lookup failure `/tmp/surge-release-r15-reply-red.log`, `/tmp/surge-release-r15-reply-green-final.log`.
- Rustfmt passed on all owned modules.

Owned changes: `crates/surge-mcp/Cargo.toml`, `crates/surge-mcp/src/connection.rs`, new private `stderr_capture.rs`; `crates/surge-telegram/src/cockpit/production.rs`, new private `outgoing_admission.rs`, `reply_admission.rs`. No commits, no publishing, no live Telegram. Privacy of SDK failures already protected by existing centralized conversion; no sanitizer rewrite.

Remaining scope limitations: checks sample current pairing immediately before dispatch, cannot retroactively cancel previously authorized requests; optional disk capture unavailable non-Unix with explicit warning but launch/stdio/tracing unchanged; actively hostile same-UID hardlink creation after identity check cannot be excluded by nlink snapshot; final full workspace lint/tests/build belong coordinator. Daemon socket issue referred root-owned authz fix, no R15 edits. Dependency RustSec/deny audit belongs R06. Current changed release defects pass targeted evidence; no broad independent release GO asserted here. Durable memory exceptions: none.

Final independent R23 exact reply source verdict ACCEPTABLE; wiremock GREEN2/2 independently inspected. First strict workspace clippy exposed collapsible_if at MCP publish; applied equivalent Rust2024 let-chain, rustfmt passed. No lint suppression, no behavior change, no extra Cargo run (coordinator owns final gate).

Final strict clippy test module repair: moved PermissionsExt into stderr_capture cfg(test,unix) module imports, removed duplicate function-local imports; production scoped import unchanged. Rustfmt passed; no suppression/no Cargo run.

## Final full-suite investigation / new safety blocker

Retained optional-whitelist child stderr `.tmpehbEmb/cold-host-stderr-ILcFrT` shows child panic at records() fixture JSON parse EOF while concurrent writer was appending a JSONL row. Fixed fixture snapshot reader to parse only newline-committed records and still strictly reject corrupted complete rows; added independent partial→complete→corrupt fixture oracle. No checks disabled; Cargo pending root queue.

Retained transplanted fixture `.tmpNbcVNg` has no original.jsonl; child stderr explicitly reports MCP selected catalog timed out after exact configured275ms then NotRunning with restart_on_crashfalse. Default nextest concurrency also caused other WouldBlock resource failures; low-concurrency rerun required before calling this product failure. No deadline change proposed.

Real safety blocker: populated cold twin `.tmp2LJgn5` journal contains MCP intent seq12/26, GroupOnly establishment seq13/27, local_effectsfalse, suspension cleanup_confirmedtrue seq24, final completed seq41, zero MCP writer closure. Workspace has no production ExecutionWriterClosed emitter; HostWriterObserver has no settlement callback and direct-child reaping explicitly does not prove descendant/effect disappearance. process_evidence::probe returns Unknown for empty GroupOnly group; docs/superplane-improvements1094/1108/1136 explicitly document incomplete domain/effect coverage. Productive cold twin acceptance must stay RED/NO-GO until real containment/effect ownership exists. No fake closure, coverage upgrade, ignored test or oracle weakening permitted (root/R32 agreed).

Minimum safe mitigation plan under review: cold resume guard checks authoritative writer journal before registry mutation/provider/MCPdispatch; truthful final suspension seal sets cleanup_confirmedfalse on unresolved MCP coverage and existing persistence transitions false fence to Attention. The productive cold twin remains intentionally unmet acceptance, but unresolved writers must not receive new dispatch. Root/R32 precode review pending.

R32 independent expanded producer+consumer plan verdict ACCEPTABLE. Existing productive cold twin preserved without weakening. New independent actual negative test `unresolved_mcp_domain_seals_false_fence_and_blocks_actual_cold_dispatch` authored first; uses actual GroupOnly MCP launch, requires false suspension fence, tests cold scheduler restart emits no new MCP/provider records and attention state. Current production unchanged pending root serialized RED. JSONL fixture oracle test source now ready. R32 explicitly notes this safety mitigation does not establish productive containment or release GO.

MCP safety negative RED confirmed root: exit101,0passed1failed `/tmp/surge-release-final-mcp-domain-red.log`, asserted actual direct transport settlement falsely authorized whole writer domain. Implemented minimum producer+consumer safety mitigation in `engine/writer_coverage.rs`, `engine/engine.rs`, `engine/run_task.rs`: fresh authoritative writer replay after registry shutdown before seal sets false fence on missing/conflicting/unconfirmed MCP/effect coverage or readerror; before cold resume registry/writer/wake mutation same evidence is checked and existing WorkItemRejected denies unresolved writers. Existing persistence false-fence Attention mapping retained. Explicit consistent established genuine closure remains trusted; otherwise actual full-domain Gone plus local-effect coverage required. No new closure emitter, no GroupOnly upgrade, no transport exit masquerading as domain proof. Rustfmt passed, no Cargo executed R15. R32 final exact diff scoped safety verdict ACCEPTABLE. Actual negative GREEN pending root; original productive cold twin remains unchanged/open NO-GO requirement.

First mitigation GREEN attempt compile blocked E0599: Storage.open_run_reader requires &Arc<Storage>; corrected private inspect helper receiver to match both actual callers. Rustfmt passed. This compile failure is not behavior GREEN. Root requested source freeze after correction; subsequent checks root-owned.

Typed refinement supersedes false-fence sealing: production persistence `runs/writer.rs517` correctly prohibits sealing cleanupfalse. GREEN2 showed noSuspended event and fixturetimeout/tracking mismatch; this was not accepted evidence. R32 independently approved refined existing RecoveryRequired path. Now unresolved/readerror MCP branch skips suspension sealing altogether, appends RunRecoveryRequired via existing helper and returns typed RecoveryRequired; existing daemon supervisor projects Attention. No persistence contract weakened and no false/true cleanup fence invented. Negative fixture/test renamed `unresolved_mcp_domain_requires_attention_and_blocks_actual_cold_dispatch`, waits actual durable RecoveryRequired plus Attention, asserts no confirmed fence and no new MCP/provider effect on cold restart. Root GREEN pending; source frozen after rustfmt. Original productive cold requirement stays unchanged/open.

Final fixture lint mechanical repair: extracted shared initial_mcp_evidence_ready predicate and flattened readiness branch using let-chain. Same negative RecoveryRequired+Attention proof, same positive confirmedSuspended+actualcycle proof; no production/test assertions changed, no allow attributes. Rustfmt passed; strict retry coordinator-owned. Root reported full nextest3830passed10failed(all known MCP productive acceptance),36skipped.

Remaining return-selection nesting eliminated using flat bool.then_some(first_cycle).flatten(); conditional body removed entirely. Same None for negative mode, same first cycle for positive modes, no external effects. Rustfmt passed; no Cargo.

## Controlled ignored smoke fixture investigation
Retained root `/private/tmp/surge-controlled-retained-w6iuvkbl` proves repository clean and worktree only `.surge/profile_catalog.md` changed: engine correctly adds ollama unavailable annotation for absent OLLAMA_HOST, fixture baseline had omitted runtime-environment availability. Durable immutable SQLite StageFailed seq5 and RunFailed seq6 independently show required user_prompt missing from inherited example graph bindings. Fixture-only correction seeds catalog using the same Registry::for_run(config) and agent_env resolution, and clears example-specific bindings in the selfcontained transport smoke. Full HEAD/diff/status oracle unchanged. R32 independently ACCEPTABLE. Rustfmt + git diff --check PASS; controlled runtime rerun pending coordinator. No product changes, no Cargo by R15.
