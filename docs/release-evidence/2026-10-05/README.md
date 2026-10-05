# Release evidence — 2026-10-05

**NO-GO.** Preparation evidence does not authorize publication. Baseline
`d102cf6` was clean; branch is `codex/release-readiness-20261005`.
All 32 requested agents actually ran in waves. Role reports are historical
snapshots; final receipts supersede intermediate pending states.

## Revision and native artifact

Frozen Rust source and packaged README/licenses: `3136b8268cb33009305acf114ff6cc075bbc3b8d`.
The frozen workspace checks ran before stage commits on this identical source
content; only Git version metadata changed when commits were recorded. Optimized
binaries were built after the source commits. Later changes are documentation and
receipts; no Rust/manifest/lock/packaging input change follows this checkpoint.
CLI reports `surge 0.1.0 (3136b82, 2026-10-05)`; daemon reports `surge-daemon 0.1.0`.

Native ARM64 build and smoke ran on **macOS 27.0**, Rust 1.98.1, local OpenSSL 3.6.4.
The extracted binaries target minOS 15.0 and have only system dylibs. No runtime
execution on macOS15, macOS Intel, Linux or Windows is implied. Exact-revision
remote CI and the configured four-target native release set remain unverified.

Local candidate: [surge-aarch64-apple-darwin.tar.gz](../../../target/release-readiness/surge-aarch64-apple-darwin.tar.gz).
[SHA256SUMS](SHA256SUMS) covers this one native candidate, not the four-platform set.
[Artifact manifest](surge-release-frozen-artifact.json) records inputs and limits.

```text
88569c9a783f26296cb31cd16c5b47f6db53c8d35a74201e2f32d9e659573e6b  surge-aarch64-apple-darwin.tar.gz
```

## Final checks

| Check | Actual result | Receipt |
|---|---|---|
| Strict Clippy default/all-features | PASS, exits 0 | [log](logs/surge-release-frozen-clippy-all.log) |
| Format | PASS, exit 0 | [log](logs/surge-release-frozen-fmt.log) |
| MSRV 1.96 CLI/daemon workspace (UI excluded) | PASS, exit 0; incremental-off retry | [log](logs/surge-release-frozen-msrv-retry.log) |
| Full nextest, default workspace including UI | FAIL, exit 100:3,840 run / 3,830 passed / 10 failed / 37 skipped | [log](logs/surge-release-frozen-nextest.log) |
| Doctests | PASS,5 passed / 7 ignored | [log](logs/surge-release-integrated-doc.log) |
| Ignored mock ACP | PASS,2 tests | [log](logs/surge-release-integrated-ignored-acp.log) |
| Ignored mock engine | PASS,3 tests | [log](logs/surge-release-integrated-ignored-engine.log) |
| Ignored MCP stdio | PASS,3 tests | [log](logs/surge-release-integrated-ignored-mcp.log) |
| Controlled daemon MCP/approval smoke | PASS,1 selected parent/child scenario | [log](logs/surge-release-controlled-green.log) |
| Real daemon restart | PASS,2 tests; idle and SIGSTOP-delayed | [log](logs/surge-release-frozen-restart.log) |
| Dependency deny policy | PASS, exit 0 | [log](logs/surge-release-integrated-deny.log) |
| Dependency audit | PASS exit 0 with 17 allowed warnings | [log](logs/surge-release-integrated-audit.log) |
| Workflow actionlint | PASS, exit 0 | [log](logs/surge-release-frozen-actionlint.log) |
| Packaging contract | PASS,5 tests | [log](logs/surge-release-frozen-packaging.log) |
| Final optimized CLI+daemon build | PASS,9m 01s; static OpenSSL / target 15.0 | [log](logs/surge-release-frozen-native-build.log) |
| Extracted stripped binaries linkage/minOS | PASS, system-only dylibs/minOS 15.0 | [log](logs/surge-release-frozen-extracted-linkage.log) |
| Archive validation and actual license bytes | PASS, five members / complete gzip / Apache terms | [log](logs/surge-release-frozen-archive-check.log) |
| Extracted archive durable E2E | PASS, init / describe / two flows / replay / restart / clean HEAD | [log](logs/surge-release-frozen-archive-smoke-green2.log) |

Final-source benchmark exits 0 with the 64-sample p95 ≤ 5 ms assertion enabled.
Criterion reports 224.19–233.83 µs; this interval is not the p95 value. The
comparison is not statistically significant (p = 0.07); no improvement claim.
[Benchmark receipt](logs/surge-release-frozen-bench.log) and [raw estimates](criterion/estimates.json). The [machine-readable command list](final-results.json) and
[sequential integration runner](surge-release-final-checks.py) retain exact commands.
The [archive E2E script](surge-release-r22-smoke.py) and
[complete command outputs](logs/surge-release-frozen-archive-commands.log) show
actual isolated operations, not a synthetic success oracle. Mock/controlled
receipts do not establish authenticated provider, tracker or Telegram delivery.

Doctests and unchanged mock integrations preceded the final fixture-only edits;
frozen nextest/lint/MSRV and final archive tests follow those edits. The two real
restart tests and corrected controlled MCP smoke were rerun after their fixes.

Synthetic restore mechanics applied the actual 30 registry and eight run SQL
migrations with Python SQLite, retained live-WAL markers, checked integrity/FKs
and restored artifact bytes. [Drill receipt](synthetic-restore-evidence.json) is
not evidence of production or provider recovery.

## Failures and remaining gates

All ten full-suite failures are `owned_flow_mcp_recovery`. Original productive
recovery and downstream refusal/manifest oracles stay enabled; accepted
requirements stay open. Safe `RecoveryRequired`/`Attention` is proven, productive
cold recovery is not. A leader reap or empty GroupOnly domain cannot prove escaped
descendant or external-effect cleanup.

General third-party copyright/license/NOTICE collection for all linked Rust/native
dependencies remains unverified. cargo-deny checks permitted license selection,
not bundle completeness. Local OpenSSL 3.6.4 complete Apache terms/attribution were
checked in the actual archive. See [license inventory](../../../THIRD_PARTY.md).

[Reviewable logs](logs/) retain genuine RED tests, compilation failures and repairs.
Their trailing spaces and repeated blank EOF lines are normalized for Git; no
output words or internal line ordering are changed. [Raw logs](raw-logs.tar.gz)
preserve exact original bytes; [per-file hashes](raw-log-hashes.json) authenticate
those originals.
Names containing `green` or `final` are not verdicts: read exit/test summaries.
Abort-projection-red selected zero tests; red2 is the actual failing regression.
MCP green/green2 failed; green3 is the actual safety PASS. Three later compilation
attempts exhausted disk before tests/check completion; these are environment
failures, not passes. The frozen MSRV retry disables incremental cache and passes.
The helper runner itself hit disk exhaustion while writing results, so
final-results.json records the subsequent commands separately and preserves logs.

Archive smoke first refused dirty source after init/describe; the next attempt
completed both runs but raced asynchronous daemon stop/start. Corrected script
commits context, ignores local runtime configuration, asserts clean source and
unchanged HEAD, and waits on actual isolated PID/socket disappearance before one
start. Both failed fixtures/logs remain retained. The controlled smoke initially
seeded stale runtime availability and an absent prompt binding; its corrected
fixture keeps the complete Git-state oracle. Original idle restart asserted grace
must fully elapse; the new actual idle/delayed owner tests verify both contracts.
No productive acceptance test was disabled or weakened.

## Team

| Role | Area | Historical report |
|---|---|---|
| R01 | Release scope | [report](agents/surge-release-r01.md) |
| R02 | Architecture | [report](agents/surge-release-r02.md) |
| R03 | Unfinished work | [report](agents/surge-release-r03.md) |
| R04 | Installation | [report](agents/surge-release-r04.md) |
| R05 | Build | [report](agents/surge-release-r05.md) |
| R06 | Dependencies | [report](agents/surge-release-r06.md) |
| R07 | Static analysis | [report](agents/surge-release-r07.md) |
| R08 | Backend | [report](agents/surge-release-r08.md) |
| R09 | Frontend | [report](agents/surge-release-r09.md) |
| R10 | API | [report](agents/surge-release-r10.md) |
| R11 | Data | [report](agents/surge-release-r11.md) |
| R12 | Migrations | [report](agents/surge-release-r12.md) |
| R13 | Sessions | [report](agents/surge-release-r13.md) |
| R14 | Authorization | [report](agents/surge-release-r14.md) |
| R15 | Security | [report](agents/surge-release-r15.md) |
| R16 | Secrets/privacy | [report](agents/surge-release-r16.md) |
| R17 | UX | [report](agents/surge-release-r17.md) |
| R18 | Accessibility | [report](agents/surge-release-r18.md) |
| R19 | Compatibility | [report](agents/surge-release-r19.md) |
| R20 | Unit tests | [report](agents/surge-release-r20.md) |
| R21 | Integration tests | [report](agents/surge-release-r21.md) |
| R22 | E2E | [report](agents/surge-release-r22.md) |
| R23 | Regression QA | [report](agents/surge-release-r23.md) |
| R24 | Performance | [report](agents/surge-release-r24.md) |
| R25 | Reliability | [report](agents/surge-release-r25.md) |
| R26 | Concurrency | [report](agents/surge-release-r26.md) |
| R27 | Observability | [report](agents/surge-release-r27.md) |
| R28 | CI | [report](agents/surge-release-r28.md) |
| R29 | Deployment | [report](agents/surge-release-r29.md) |
| R30 | Recovery | [report](agents/surge-release-r30.md) |
| R31 | Documentation | [report](agents/surge-release-r31.md) |
| R32 | Independent release review | [report](agents/surge-release-r32.md) |

See [task ledger](../../release-readiness-2026-10-05.md),
[release/rollback procedure](../../release-procedure.md) and
[draft notes](../../release-notes-v0.1.md). Independent final verdict is recorded
in R32; scoped source ACCEPTABLE does not mean release GO.
