# Final continuation evidence

Verdict: **NO-GO**. Compiled source is
`8e3a781cb26666933a314c2e0d3361810ef1a21d`, before this evidence-only commit.
The source file identity SHA-256 is
`e4e8e04e3c5eeacec4dbd78992197298727733f1802246b78a3bea0b27631f53`.
The original evidence folder remains historical; these receipts supersede its
pending notice/pool/version-cache states.

## Receipts

- [Manifest](manifest.json): actual source, candidate/proof hashes and counters.
- [Raw files](raw-files.json) and [verbatim logs](raw-logs.tar.gz): successful
  checks and failed attempts, including disk exhaustion and causal RED tests.
  The tar also contains `archive-e2e-commands.log` from the extracted candidate.
- [Local paired asset checksums](SHA256SUMS.local): one target's two assets.
- [Independent final review](reports/surge-release-r32-settled-final.md).
- [Version cache RED/GREEN](reports/r10-version-status.md) and
  [concurrency review](reports/surge-release-r26-version-cache-review.md).
- [Pool lifetime repair](reports/surge-release-r25.md) and
  [independent pool review](reports/surge-release-r26-pool-review.md).
- [Open MCP architecture work](reports/surge-release-r15-productive-plan.md).

Final checks: fmt/actionlint, both strict workspace clippy profiles, non-desktop
MSRV 1.96 with the documented macOS stripping workaround, native build,
authenticated source capture, notices collector, package validation and isolated
extracted-archive E2E passed. Python: 41 passed. Nextest: 3,843 executed,
3,833 passed, 10 failed, 37 skipped. Doctests: 5 passed, 7 ignored.
Security: deny passed; audit found zero known vulnerabilities, with 17 warnings.
No claim of all warnings or skipped requirements being resolved.

## Local artifacts and limits

The six-member archive is
`target/release-candidate-settled/surge-aarch64-apple-darwin.tar.gz`; its paired
receipt is `surge-aarch64-apple-darwin.notices.json`. Complete local producer and
collector evidence is in `target/release-proof-settled/`. These generated files
are ignored rather than committed. Manifest paths identify this workspace and
hashes permit checking copied artifacts.

The producer and independent reviewer authenticated 351 registry packages and
17,553 published files against locked archive checksums. Both binaries have
minimum OS 15.0 metadata and system-only dylibs. Smoke ran on macOS 27; execution
on macOS 15, the other native targets and real provider/account integrations
remain unverified. The four-target collector must not accept this two-asset
local set as a complete release.

Ten MCP acceptance failures remain open pending a containment/external-effect
contract decision and implementation. No test was disabled or accepted
requirement retired. No push, tag, merge, publication or production change.

Use the [release and restore-only rollback procedure](../../../release-procedure.md).
Reproducing strict source-bound packaging requires checkout of the compiled
source checkpoint; the later documentation commit has a different source ledger.
