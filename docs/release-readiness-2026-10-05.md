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
`8c4b45a` CLI outcomes and `3136b82` owned fixture closure. Native candidate source
is `3136b82`; the historical candidate precedes the new notice/provenance packaging
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
| Productive MCP cold recovery | P0 | R15/R32 | Complete containment/effect proof | Original accepted productive and downstream recovery tests pass | OPEN; NO-GO; ten acceptance failures retained |
| Release workflow gates | P0 | R28/R19 | Same revision CI | Strict features/security/tests; native archive linkage | actionlint PASS; remote native execution unverified |
| macOS archive OpenSSL | P1 | R19 | Native release build | Static OpenSSL; no non-system dylib path; target15 | ARM64 build/linkage/archive E2E PASS; other runners unverified |
| Distribution notices | P1 | R06/R19/R32 | Linked dependency inventory | Complete required attribution/NOTICE bundle, not just license selection | OPEN; checksum-bound inventory and native capture implemented; upstream correspondence and native mappings unresolved |
| Notice/provenance tooling | P1 | R05/R06/R19/R20/R28 | Exact graph, runtime terms and source inputs | Six-member archives, paired receipts, corruption refusal and immutable source capture | Python 34/34 PASS; real native capture pending |
| Release/rollback docs | P0 | R30/R31 | Schema and packaging | Full quiescent snapshot; restore-only rollback | Complete; synthetic mechanics drill PASS |
| Final integrated validation | P0 | Coordinator/R20–24 | Frozen source | Honest build/lint/test/smoke denominators | Frozen clippy/fmt/MSRV/build/smoke PASS; nextest NO-GO |
| Independent verdict | P0 | R32 | Final evidence | Reviewable GO/NO-GO | Final independent NO-GO; safety/local ARM64 candidate ACCEPTABLE |

## Verification

Final frozen full workspace nextest (four threads): **3,840 executed: 3,830 passed,
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
License selection passes cargo-deny, but full linked dependency attribution/NOTICE
inventory and archive notice bundle remain unverified; OpenSSL 3.6.4 complete Apache
terms and attribution were checked in the local archive. Config mode0600 was verified; inherited macOS ACL privacy and directory-entry
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
