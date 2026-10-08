# Release preparation — 2026-10-05

**NO-GO for the final release.** macOS CI passes on `89117a7`; MCP restart
recovery follows the accepted ADR-0021 contract. Remaining v1 phases and a fresh
release candidate with provenance still require completion. The active scope is
macOS only. Commits and pushes to PR #89 are authorized; release publication,
tagging, merge and production changes remain unauthorized. Earlier sections
below preserve historical checkpoints; later dated updates supersede them.

## Scope and revision

Baseline `d102cf6` was clean. Branch `codex/release-readiness-20261005` prepares the
existing v0.1.0 CLI and sibling daemon archives: Linux x86_64 GNU, macOS Intel and
Apple Silicon, Windows x86_64. Desktop is separately assessed and not packaged.
macOS candidates target 15.0 and must have no Homebrew runtime dylib dependency.
Existing competitive-waves accepted requirements remain open; no manifest row was
retired. Source checkpoints so far: `e4797a2` runtime/dependency closure and
`f1bb266` CI/documentation, `0e06bf1` macOS packaging, `cd86349` MCP safety,
`8c4b45a` CLI outcomes and `3136b82` owned fixture closure. Historical native candidate source
is `3136b82`; that candidate precedes the new notice/provenance packaging
contract. Artifact identity is recorded in
[the evidence index](release-evidence/2026-10-05/README.md).

Latest compiled source: `8e3a781cb26666933a314c2e0d3361810ef1a21d`.
The subsequent evidence-only commit records results without changing that
artifact identity. See [continuation evidence](release-evidence/2026-10-05/continuation/README.md).

## Task ledger

| Task | Priority | Owner | Dependencies | Acceptance | State |
|---|---|---|---|---|---|
| Release scope/inventory | P0 | R01/R03 | Existing docs | Deliverables and deferred requirements distinguished | Complete |
| Dependency remediation | P0 | R06 | Lock/API closure | Narrow patched dependencies, strict policy | deny PASS; audit 17 allowed warnings |
| git2 SDK compatibility | P1 | R06/R19 | git2 0.21 | Fallible data errors preserved across Git/UI | Git 77 passed, 2 ignored; workspace lint compiled UI |
| Existing run upgrades | P1 | R12 | Writer ownership | Legacy data preserved; future schema refused before mutation | Upgrade 3/3, registry fixture 1/1 |
| Memory FTS repair | P2 | R11/R12 | Atomic v2→v3 migration | Four-table postings integrity and rollback | Memory 40/40 |
| Private atomic config | P1 | R26 | Exclusive tempfile | No victim clobber/colliding writer; 0600 | RED→GREEN 4/4 |
| Admission wakes | P1 | R26 | Single queue consumer | Notification before poll retained | RED→GREEN 10/10 |
| ACP option deadline | P1 | R25 | Opening budget | Timeout/cancel settles published child without retry | RED→GREEN 2/2; related 18/18 |
| Durable merge reconciliation | P1 | R25/R32 | Registry/journal | Lost completion repaired; uncertain RPC never replayed | 17/17; independent ACCEPTABLE |
| MCP diagnostic capture | P1 | R15/R23 | Descriptor ownership | Symlink/hardlink refusal; retained descriptor | 4/4; independent ACCEPTABLE |
| Telegram authorization/redaction | P1 | R15/R16/R23 | Current pairing | Revocation blocks cards/replies; URL token removed | Cards 3/3, replies 2/2, redaction 1/1 |
| Runtime home isolation | P1 | R27/R11 | Canonical SURGE_HOME | Usage/artifact default roots isolated | Subprocess 2/2 |
| Daemon startup/socket safety | P1 | R29/R32 | Private socket parent | Nonzero failure; unrelated paths preserved; ACL guard | Startup 2/2; socket/ACL 5/5 |
| CLI outcome and abort projection | P1 | R05/R26 | Durable owner/history | Failed/aborted/missing/conflicting outcome is nonzero | Watch units 7/7; lifecycle 6/6; final projection PASS |
| Fixture ownership/readiness | P1 | R26/R25/R15 | Actual owner settlement | Await writer close; stale socket/partial JSONL cannot signal readiness | Scheduler 9/9; route/JSONL final PASS |
| MCP cleanup authority safety | P0 | R15/R32 | Fresh journal | Unknown evidence refuses seal/resume before fresh effects | Actual RED→GREEN 1/1; independent ACCEPTABLE |
| SQLite maintenance settlement | P1 | R25/R26/R32 | Actual DB owner lifetime | Dropped reader workers exit promptly; live pool policies and owners preserved | RED 0/390 exits → GREEN 390/390 (524 connections); final full workspace regression PASS |
| Version launcher cache | P1 | R10/R26/R32 | Invocation identity | Aliases preserve argv[0]; concurrent requests share a probe | Genuine RED 2 failures → GREEN 12/12; full workspace PASS |
| Productive MCP cold recovery | P0 | R15/R32 | Complete containment/effect proof | Original accepted productive and downstream recovery tests pass | OPEN; NO-GO; ten acceptance failures retained |
| Release workflow gates | P0 | R28/R19 | Same revision CI | Strict features/security/tests; native archive linkage | actionlint PASS; remote native execution unverified |
| macOS archive OpenSSL | P1 | R19 | Native release build | Static OpenSSL; no non-system dylib path; target15 | ARM64 build/linkage/archive E2E PASS; other runners unverified |
| Distribution notices | P1 | R06/R19/R32 | Linked dependency inventory | Complete required attribution/NOTICE bundle, not just license selection | Final actual ARM64 source/binary capture PASS; 351 packages / 17,553 published files; other native platforms unverified |
| Notice/provenance tooling | P1 | R05/R06/R19/R20/R28 | Exact graph, runtime terms and source inputs | Six-member archives, paired receipts, corruption refusal and immutable source capture | Final Python 41/41 PASS (75.379s); authenticated native producer/collector/package PASS |
| Release/rollback docs | P0 | R30/R31/R32 | Schema and packaging | Full quiescent snapshot; restore-only rollback | Complete; synthetic and actual same-schema native terminal-only restore drills PASS |
| Final integrated validation | P0 | Coordinator/R20–24 | Frozen source | Honest build/lint/test/smoke denominators | Frozen clippy/fmt/MSRV/build/smoke PASS; nextest NO-GO |
| Independent verdict | P0 | R32 | Final evidence | Reviewable GO/NO-GO | Final independent NO-GO; safety/local ARM64 candidate ACCEPTABLE |

## Verification

Original frozen `3136b82` full workspace nextest (four threads): **3,840 executed: 3,830 passed,
10 failed, 37 skipped**. Every failure belongs to `owned_flow_mcp_recovery`.
The new negative safety test passes: actual journal `RunRecoveryRequired`, attempt
`Attention`, no cleanup-confirmed fence and no new MCP/provider effect after
scheduler restart. Existing productive tests and their requirements are preserved.
Some downstream refusal/manifest tests can no longer reach their prior productive
suspension precondition; this is still an unmet acceptance gate, not a test waiver.

First full run had 3,814 passed/20 failed/36 skipped. Its CLI/writer fixture and
resource errors were repaired. Two later compilations failed for disk exhaustion
before tests; these are retained environment failures, not passing receipts.
[Evidence](release-evidence/2026-10-05/README.md) records final lint, format, MSRV,
doctest, deterministic ignored integration, packaging and extracted archive checks.

Benchmark enforced p95 ≤5ms on 64 samples and passed; Criterion interval was
224–234µs. No historical +25% improvement claim. Dependency audit exits zero with
17 allowed transitive maintenance/unsoundness/yanked warnings; this does not mean
zero advisory findings. Synthetic Python SQLite recovery applied actual 30 registry
and 8 run migrations, checked WAL backup, integrity/FKs and restored artifact bytes;
it does not prove production/provider recovery.

## Continuation: authenticated source closure

Notice provenance stages `dda2a1d`, `68c6943` and `22d3bf9` added reviewed
full upstream/runtime texts, exact correspondence maps and immutable build input
capture. Independent review then found missing registry archive authentication
and an incorrectly pruned real `cc/src/target` directory. The producer now checks
all published registry file bytes against Cargo.lock-authenticated archives before
and after build; the collector and portable consumer independently verify the
complete member ledger. Actual inventory checks 351 packages and 17,553 files.
Python release tests: 41/41 PASS (75.714s); independent corruption checks PASS.
The provisional `22d3bf9` archive is retained as intermediate evidence and must
not be substituted for the new authenticated-source candidate.

The first continuation workspace repeat executed 3,840 tests: 3,828 passed,
12 failed, 37 skipped. Besides the ten known MCP failures, it exposed an IPC
fixture reading before schema creation and an OS35 thread-creation failure in a
skill test. The IPC fixture now waits for durable Completed before one strict
journal inspection; errors are not swallowed. Both targeted tests pass (2/2).
Final frozen workspace repeat and native capture remain pending at this checkpoint.
These pending checks are not covered by earlier passing receipts.

The `da89ba2` full repeat passed 3,829/3,840 with 11 failures and 37 skips:
IPC and the previous skill failure passed, but OS35 thread creation recurred in
another skill test. It is not classified as resolved by an isolated pass.
Independent lifetime review found that the default scheduler drains its future
30-second reaper even after a DB pool loses its last owner, retaining three
threads per expired pool. The new private builder retains separate three-worker
executors and discards pending callbacks only after their last owner is gone.
Live connection limits, pragmas, idle timeout and maximum lifetime are unchanged;
in-flight jobs retain their executor. A permanent shared executor was rejected
because it would accumulate dead periodic jobs. Actual lifecycle regression:
old behavior 0/390 exits within 3 seconds; corrected behavior 390/390 exits,
524 connection acquisitions, 0.24 seconds. The oracle uses real connection paths
and thread-local destruction, scoped to its unique test home. This fixes expired
pool retention; initial executor creation can still fail under OS exhaustion.
Final integrated checks and the authenticated native candidate remain pending.

The `7cd5ea5` full repeat executed 3,841 tests: 3,830 passed, 11 failed,
37 skipped. The pool lifecycle regression and both previously failing skill
tests passed. The additional failure came from a version-cache test executing
the installed rustup. Inspection found that canonicalizing a launcher before
execution changes its argv[0] and collapses distinct multicall aliases. The cache
now retains absolute invocation paths and shares initialization per key without
holding its map lock during process execution. Cancellation before completion
permits retry; the one-second deadline remains unchanged. Owned alias and
concurrent-counter regressions failed before the fix; all 12 version-probe tests
pass afterward. Strict scoped clippy passes after moving the existing locked
which dependency into the production workspace declaration. Final workspace
validation and authenticated native capture still require the new source checkpoint.

At `7cd5ea5`, both workspace clippy profiles and the non-desktop MSRV check passed;
the latter required `RUSTFLAGS='-C strip=none'` on macOS 27 as documented in
[development](development.md). Cargo-deny passed; cargo-audit found zero known
vulnerabilities and retained the 17 previously disclosed warnings.

## Blocking work and limits

Final `8e3a781` integrated validation: fmt, actionlint, both strict workspace
clippy profiles, non-desktop MSRV, native release build (4m 46s), notices and
archive packaging passed. Nextest executed 3,843: **3,833 passed, 10 failed,
37 skipped** (159.449s); all ten failures are the retained MCP requirements.
The first attempt stopped before listing tests after disk exhaustion damaged a
generated binary. Old test-cache files and that corrupt binary were removed with
hash ledgers; the full rebuild/retry above is the valid test receipt. Doctests:
5 passed, 7 ignored. Cargo-deny passed; audit: zero known vulnerabilities and
17 warnings. Existing linker/future-incompatibility warnings are preserved.

The final six-member macOS ARM64 archive and paired receipt are under
`target/release-candidate-settled/`; local checksums are recorded in
[SHA256SUMS.local](release-evidence/2026-10-05/continuation/SHA256SUMS.local).
Actual extracted-archive E2E passed: two terminal workflows, completed journal
replay, daemon restart and clean project Git state. R32 independently validated
archive bytes, binary/notice/source bindings, authenticated registry archives,
system dylibs and minimum OS metadata. This is one local target, not the required
four-platform publication set. The final independent verdict is NO-GO.

The required `test-ignored` allowlist was repeated at documentation checkpoint
`062008c`, with unchanged Rust/manifest/lock source from `8e3a781`: ACP 2,
engine 3, MCP stdio 3, controlled daemon 1 and daemon restart 2 passed (11 total).
This is separate from the default nextest denominator. A native binary
replacement/full restore drill also passed: `3136b82` → `8e3a781` → restored old
binary, two completed terminal runs retained, a third post-snapshot run
quarantined, three database checks and 111 exact snapshot entries before reopening.
The first failed attempt exposed SQLite sidecar creation during read-only checks;
validation now uses a separate exact working copy. [Follow-up evidence](release-evidence/2026-10-05/followup/README.md)
preserves both attempts and independent review. Scope is same-schema terminal-only;
provider, external-effect and production restoration remain unverified.

Complete MCP containment/settlement and external-effect authority are required for
productive cold recovery. GroupOnly evidence cannot become complete cleanup merely
because its leader was reaped or the group disappeared. Current safe behavior is
`RecoveryRequired`/`Attention`, with refusal before cold effects. No fake closure
receipt or disabled acceptance test was introduced.

Before four-platform publication, require exact-revision reusable CI and actual
native build/archive receipts for all four targets. Local archive smoke covers only
macOS ARM64 on macOS 27, with minimum OS 15.0 metadata and system-only dylibs.
Actual execution on macOS 15 remains unverified. Real ACP subscriptions, GitHub/Linear accounts and Telegram delivery
are unverified. Windows Task Start private preparation and MCP child observation
remain unavailable/fail closed; Windows archive smoke does not establish parity.
Telegram `/run` is not wired; CLI start is documented. Loop Replan and complex
nested-loop recovery placeholders remain excluded from readiness claims. Desktop
creation/accessibility issues in role reports are future work outside these archives.
License selection passes cargo-deny. Exact local source notice mappings and
conservative native header/declaration inventories have independent review;
matching final build/binary receipts and new archive checks remain required.
This is scoped compiled-macOS notice coverage, not general SDK/source redistribution
assurance or four-platform closure. OpenSSL 3.6.4 complete Apache terms and
attribution were checked in the historical local archive. Config mode0600 was verified; inherited macOS ACL privacy and directory-entry
power-loss durability were not. Same-UID hostile namespace races are outside the
socket/capture ownership guarantees. Notifications after an uncertain merge are
best effort; durable receipt prevents replay even if delivery fails.

## Team and handoff

All **32 distinct agents** actually ran in waves. Roles R01–R32 cover the requested
scope, architecture, inventory, installation, build, dependencies, analysis,
backend, frontend, API, data, migrations, sessions, authorization, security,
privacy, UX, accessibility, compatibility, unit/integration/E2E/regression tests,
performance, reliability, concurrency, observability, CI, deployment, recovery,
documentation and independent release review. Their historical interim reports
are retained under [agents](release-evidence/2026-10-05/agents/); final evidence
supersedes earlier pending states. No synthetic claim of parallel work.

Use [release and rollback procedure](release-procedure.md) and
[draft release notes](release-notes-v0.1.md). The candidate is for review and isolated
testing; release publication requires separate authorization after blockers close.


## Managed MCP decision — 2026-10-05 follow-up

The user accepted managed opt-in execution. [ADR-0020](adr/0020-managed-mcp-recovery.md)
records the decision, independent domain/effect proof requirements and exact
transport constraints. This stage changes documentation only; Rust remains at
the previously tested revision. No backend, broker or new passing MCP receipt
is claimed. NO-GO and all original acceptance requirements remain open.

Read-only discovery found no installed managed VM backend, guest or configured
local Linux/Windows runner. Current macOS 27 ARM has virtualization support but
approximately 6.1 GiB free disk. A compatible provisioned macOS VM is necessary
for unchanged macOS transport acceptance; an actual delegated Linux host could
start a separately accepted Linux backend contract. Provisioning details have
been requested; no install, remote mutation or publication was performed.

Source-supported reviews: [security design](release-evidence/2026-10-05/managed-discovery/security-design.md),
[integration map](release-evidence/2026-10-05/managed-discovery/integration-map.md),
and [backend discovery](release-evidence/2026-10-05/managed-discovery/backend-discovery.md).


## Active scope narrowed to macOS

The user explicitly deferred other platforms on 2026-10-05. Further implementation
and verification target macOS only; no Linux/Windows backend or runner work is
currently requested. Historical multi-platform results above remain unchanged.
macOS readiness still requires the actual managed VM backend, host-effect broker
and original MCP acceptance. A fresh disk check confirmed 6.1 GiB available;
backend provisioning remains unresolved. This scope update changes documentation
only and does not claim any new executable evidence.


## PR #89 CI repair follow-up

[CI repair evidence](release-evidence/2026-10-05/pr89-ci.md) records the Rust 1.99
async-trait update, equivalent test-assertion modernization, CLI schema-publication
race fix, actual mock-agent build ordering and MCP startup-readiness fixture.
The final local strict Clippy gates pass in both feature configurations. These
source changes require a new compiled release candidate and provenance capture;
the prior archive remains historical. Original MCP acceptance and managed macOS
backend/effect-broker requirements remain open. NO-GO is unchanged.

## MCP recovery direction change — 2026-10-05

The maintainer replaced the managed VM and effect-broker direction with
[ADR-0021](adr/0021-mcp-restart-recovery.md): cold recovery stops the prior MCP
process group and restarts from the frozen manifest; interrupted calls are
reported to the agent and never replayed. Managed VM provisioning and the host
broker are no longer release prerequisites. The P0 row "Productive MCP cold
recovery" now means: the
[restart-recovery plan](plans/2026-10-05-001-feat-mcp-restart-recovery-plan.md)
is implemented and the ten original cases pass under its acceptance mapping,
with refusal oracles unchanged. This is a documentation decision only; no new
executable evidence is claimed and NO-GO is unchanged until that acceptance
passes.

## Windows CI is advisory for the macOS release — 2026-10-05

Windows Clippy and test jobs now run with `continue-on-error` and carry
"advisory" in their names. Their results stay visible, including the known
`GETFINALPATHNAMEBYHANDLE_FLAGS` compile error, but they no longer fail the
workflow. This reflects the user-directed macOS scope and grants no Windows
support claim; Windows repair is scheduled after v1
([release plan](plans/2026-10-05-002-feat-v1-release-plan.md)).

## MCP restart recovery implemented — 2026-10-05

ADR-0021 is implemented (group stop, `ExecutionWriterGroupStopped` at schema
v17, restart from the frozen manifest, interrupted-call notice). CI run
37391960503 on `3e5db7d` passed Test Suite, Clippy and MSRV on macOS and
Ubuntu, including the owned-flow MCP suite with refusal oracles unchanged
(oracle changes are recorded in the
[restart-recovery plan](plans/2026-10-05-001-feat-mcp-restart-recovery-plan.md)).
The "Productive MCP cold recovery" P0 row is satisfied by that evidence. NO-GO
stays until the remaining v1 phases and the final release gates pass.

Windows, same run: the `GETFINALPATHNAMEBYHANDLE_FLAGS` error is gone. Clippy
stopped on one Windows-only dead-code error (fixed next, with the other
Windows-only dead-code warnings). The advisory test job compiled and ran 3571
tests: 3456 passed, 115 failed, about 60 distinct tests. Most are Unix process
and file-ownership tests (cold host, owned flow, secure start preparation) and
one mock-bridge admission test compiled into many test binaries. These remain
advisory and belong to Windows support after v1.

## Current PR checks — 2026-10-07

The six reported failures belong to historical run `37349909332`. Current
[CI run 37415800196](https://github.com/vanyastaff/surge/actions/runs/37415800196)
completed on source revision `89117a75952b7cf33d0b89dab96dd36bfe7a06df`:

- macOS nextest: 3707 run, 3707 passed, 37 skipped, in 196.169 seconds.
- macOS doctests: 5 passed, 7 ignored, no failures.
- macOS strict Clippy, format, MSRV 1.96, release packaging contract,
  dependency security/licenses and engine benchmark: passed.
- Ubuntu tests and Clippy: passed. Windows Clippy: passed; Windows tests
  remain failed and advisory, outside the active macOS scope.

These CI results resolve the reported macOS failures. They do not replace
fresh native release-candidate provenance, the skipped-test release gates or
the remaining v1 product acceptance in the
[release plan](plans/2026-10-05-002-feat-v1-release-plan.md).

## Agent restrictions and Windows CI follow-up — 2026-10-07

V1 task 1.5 is in progress under the
[reviewed implementation plan](plans/2026-10-07-001-feat-agent-restrictions-plan.md).
Its pure core stage has 864 passing nextest tests, strict Clippy on Rust 1.98/1.99,
MSRV 1.96 and independent spec/API/security acceptance;
[source-bound receipts](release-evidence/2026-10-05/agent-restrictions-core/README.md)
retain the actual RED/GREEN evidence. Runtime enforcement remains open; no
ineffective restriction settings are advertised to users.

The user now explicitly requested Windows CI repair. Windows is included in
this follow-up, superseding the earlier instruction to defer that platform.
Current Windows Test Suite job `112948098227` in run `37666830209` ran 3626 tests:
3510 passed, 116 failed, 33 skipped. Windows Clippy passed. The advisory label
does not satisfy this repair request; failures must be investigated and repaired
without disabling tests or weakening safety checks. macOS release acceptance
remains required independently.

Windows CI prerequisite checkpoint: [source-bound local evidence](release-evidence/2026-10-05/windows-ci-precursor/README.md)
records 899 passing scoped macOS tests, strict Rust 1.99 Clippy and real mock
binary build. Windows checks are mandatory again; prerequisite paths and quoting
are corrected. Production process liveness stays unchanged for a retained-handle
exit-259 native RED. Latest baseline `87ad59a` ran 3635 Windows tests: 3520 passed,
115 failed, 33 skipped. Native repair and complete Windows acceptance remain open.

Windows atomic configuration publication is implemented and independently
reviewed, with strict macOS/Windows cross-target lint and MSRV checks;
[receipts](release-evidence/2026-10-05/windows-atomic-config/README.md) distinguish
compilation from native execution. Native Windows acceptance remains pending.

Native Windows follow-up source `14884ff`, run `37672477232`: 3636 tests run,
3586 passed, 50 failed, 33 skipped. Existing directory rename-fence regression
passes; retained terminated-process regression supplies native RED before the
production repair. macOS/Ubuntu tests and Clippy on all platforms pass.
[Native receipt](release-evidence/2026-10-05/windows-ci-precursor/README.md)
preserves this result; atomic configuration and revised integration fixtures
have local/cross-target evidence and await the next native run.

Native source 77fff58 subsequently ran 3,640 Windows tests: 3,589 passed,
51 failed, 33 skipped. The process exit-259, Git history and persisted-gate
regressions pass. macOS/Ubuntu suites and all platform Clippy jobs pass.
[Raw native receipt](release-evidence/2026-10-05/windows-ci-77fff58/README.md)
records atomic-save failures, now addressed by descriptor-owned cleanup and the
documented cooperative/denied-reader sharing contract.

The following reviewed commits repair bootstrap telemetry writer settlement and
the Windows-only shell import lint, and add a mandatory isolated non-admin NTFS
durability probe. Local tests, strict checks and MSRV receipts are retained; native
GREEN remains required. Full Windows private preparation and owned-process recovery
are still open under plans 003/004; the guardian ADR remains proposed.

Native source `493c292`, run `37682450589`: **3,646 Windows tests run,
3,612 passed, 34 failed, 34 skipped**. All platform Clippy and macOS/Ubuntu
suites pass. Atomic-save, shell-quoting and telemetry-settlement regressions
now pass natively. The required separate standard-user NTFS probe passes 1/1
with complete flags-0 file/directory flush and exact read-only failure evidence.
[Source-bound receipt](release-evidence/2026-10-05/windows-ci-493c292/README.md)
explains the extra standalone-probe skip and remaining 25 private-preparation
plus nine process-ownership failures. Full Windows CI acceptance remains open.

Native source `965ac42`, run `37687504548`: **3,654 Windows tests run,
3,620 passed, 34 failed, 37 skipped**. All eight new guardian identity tests
pass; process ownership is still unimplemented. The mandatory standard-user
flush probe passes 1/1. Its additional state-home batch reports 0/3: one real
unsafe-home refusal failure and two profile-lookup fixture failures before the
backend assertions. [Exact native receipt](release-evidence/2026-10-05/windows-ci-965ac42/README.md)
keeps those categories distinct. Windows Clippy found two test-only style lints,
repaired in `7a4b9f4` pending native revalidation. All other jobs pass.

Native source `23460aa`, run `37691951295`: **3,654 Windows tests run,
3,619 passed, 35 failed, 37 skipped**. All platform Clippy and macOS/Ubuntu
suites pass. The extra failure is intermittent AccessDenied in the unchanged
concurrent configuration reader test; diagnosis remains open. Standard-user flush
passes 1/1. Profile lookup now succeeds, but two new state-home fixtures stop on
the actual SYSTEM-owned profile ancestor; they remain setup failures, not backend
RED. The unsafe-home refusal assertion remains genuine RED.
[Full receipt](release-evidence/2026-10-05/windows-ci-23460aa/README.md) preserves
those results and the focused trusted-ancestor fixture correction.

Working Stage 1 candidate (not yet native accepted): retained Windows state-home
and SQLite owners, protected runtime fixtures and caller lifetime corrections are
implemented. [Local ownership receipts](release-evidence/2026-10-05/windows-stage1-local/README.md)
record five fork and four daemon behavioral RED failures followed by focused
17/17 and 24/24 GREEN runs. A separately reviewed daemon test signal repair passes
all three affected cases. Strict persistence checks pass on macOS; the actual
Windows module harness passes cross-target Clippy. Remaining caller gates, native
security cases and independent acceptance stay open. Private/preparation guards
remain closed; process guardian implementation remains outstanding.

Native source `0350d4a`, run `37695764909`: **3,654 Windows tests run,
3,619 passed, 35 failed, 37 skipped**. All other jobs pass. Configuration reader
diagnostics did not reproduce AccessDenied; the additional failure instead reports
an MCP helper missing its endpoint-revocation settlement deadline. Both remain
under investigation. All three standalone ownership cases now reach real backend
assertions after successful fixture setup: unprotected creation, missing manager
fence and accepted unsafe home. The standard-user flush probe remains 1/1 PASS.
[Native baseline receipt](release-evidence/2026-10-05/windows-ci-0350d4a/README.md)
separates these genuine RED results from prior fixture setup failures.

Native Stage 1 candidate `29228ea`, run `37698935672`: **3,685 Windows tests
run, 2,922 passed, 763 failed, 46 skipped**. macOS and Ubuntu tests and Clippy
passed. Many Windows failures now report an untrusted object owner; this is a
regression and its exact refused object remains unresolved. The dedicated
standard-user flush probe passed, but protected new-home creation failed with
the same refusal and prevented the remaining ten native probes from running.
Windows Clippy also found one test-only `manual_assert` lint. Commit `8dac04c`
fixes that lint and adds independent SDK owner/SDDL diagnostics for the complete
native route; it does not relax the security policy or claim behavioral repair.
[Candidate failure receipt](release-evidence/2026-10-05/windows-ci-29228ea/README.md)
retains both raw Windows logs. Windows acceptance and release remain open.

Diagnostic source `8dac04c`, run `37712492476`: all jobs pass except Windows
Test Suite, which repeats **3,685 run, 2,922 passed, 763 failed, 46 skipped**.
Independent SDK diagnostics identify the first rejected object as `C:\`, owned
by the exact Windows TrustedInstaller SID. Its effective outsider child-creation
grant also requires a separate ancestor allowance. No protected home was created.
The [native route receipt](release-evidence/2026-10-05/windows-ci-8dac04c/README.md)
establishes this probe's immediate cause, not that every failure has that cause.
The reviewed repair is limited to ancestor owner recognition and child-creation
allowance; mutation-ACE trust, protected-object and SQLite rules stay unchanged.
Native GREEN and the remaining Stage 1 security fixtures are still required.

Guardian G2a pure receipt types and canonical hashing are complete in `30e7a79`.
[Local receipts and independent verdicts](release-evidence/2026-10-05/windows-guardian-g2a/README.md)
retain a genuine strict-serde RED, 15/15 focused tests (including 14 vectors in
one matrix), 887/887 full core tests, strict Clippy and Rust 1.96 checks. This
does not implement G2b transcript validation or native guardian/Job ownership.
