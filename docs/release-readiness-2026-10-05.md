# Release preparation — 2026-10-05

**NO-GO.** Productive MCP cold recovery lacks complete descendant/external-effect
cleanup evidence. Exact-revision four-platform CI remains unverified. No release,
tag, push, merge or production change has been authorized or performed.

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
| SQLite maintenance settlement | P1 | R25/R26/R32 | Actual DB owner lifetime | Dropped reader workers exit promptly; live pool policies and owners preserved | RED 0/390 exits → GREEN 390/390 (524 connections, 0.24s); final workspace repeat pending |
| Productive MCP cold recovery | P0 | R15/R32 | Complete containment/effect proof | Original accepted productive and downstream recovery tests pass | OPEN; NO-GO; ten acceptance failures retained |
| Release workflow gates | P0 | R28/R19 | Same revision CI | Strict features/security/tests; native archive linkage | actionlint PASS; remote native execution unverified |
| macOS archive OpenSSL | P1 | R19 | Native release build | Static OpenSSL; no non-system dylib path; target15 | ARM64 build/linkage/archive E2E PASS; other runners unverified |
| Distribution notices | P1 | R06/R19/R32 | Linked dependency inventory | Complete required attribution/NOTICE bundle, not just license selection | Local exact source mappings accepted; final source/binary capture pending; other native platforms unverified |
| Notice/provenance tooling | P1 | R05/R06/R19/R20/R28 | Exact graph, runtime terms and source inputs | Six-member archives, paired receipts, corruption refusal and immutable source capture | Python 41/41 PASS including authenticated registry source binding (75.714s); final native capture and workspace rerun pending |
| Release/rollback docs | P0 | R30/R31 | Schema and packaging | Full quiescent snapshot; restore-only rollback | Complete; synthetic mechanics drill PASS |
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
