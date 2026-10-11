# Windows candidate 04c4678

Run [37714922396](https://github.com/vanyastaff/surge/actions/runs/37714922396), exact source `04c467854de7f4dc3a03de0c9acc915d70cf59f5`. All three Clippy jobs, Ubuntu tests, MSRV, format, packaging, dependency and benchmark jobs passed.

Windows: **3,703 tests run, 3,504 passed, 199 failed, 46 skipped**. This differs from the prior 3,685 denominator because G2a and ancestor-policy tests were added. The prior 763 failures are not all individually attributed to the same cause.

The exact TrustedInstaller-owned C:\ route now permits the dedicated standard-user home creation. Three independent owner/ACE role tests also pass natively. The nine first native probes passed: complete flush, protected home creation, derived-pool retention, unsafe-home refusal, control lifecycle, exclusive run reservation, SQLite owner lifetime/WAL close, failed-initializer settlement, and actual SQLITE_BUSY/fatal-Drop process settlement.

Probe 10 fails its fixture precondition: the attempted unprotected DACL remains identical to the original protected DACL. It does not reach the backend refusal assertion. Probes 11 and 12 do not execute because the launcher stops on this failure. The first-observable creation probe is not in this source revision. No complete Stage 1 acceptance is claimed.

macOS: **3,760 run, 3,759 passed, 1 failed, 37 skipped**. The quota recovery frozen-budget test reached Attention while waiting for terminal completion. Existing printed diagnostics omit attempt-level refusal reasons; investigation remains open. No timeout widening or retry-to-green is applied.

Raw job logs and selected exact results are retained here. Release remains **NO-GO**.

## Independent bounded repair verdicts

Phase 5a spec acceptance: ACCEPTABLE for the root-owner repair only. The reviewer verified frozen `security.rs` SHA256 `274e106e9df39257624bef4254354936643474e811f44d9a9747d639d1eba1dd`, the final two-rule plan and actual 8dac native refusal followed by 04c protected-home success on the same exact TrustedInstaller owner/ACL pattern.

Separate subsequent Phase 5b security review: ACCEPTABLE; no actionable defect found. Exact SDK SID equality does not admit service-name/prefix families. Ancestor owner recognition does not trust a TrustedInstaller mutation ACE. Outsider child-create allowance excludes entry replacement, delete-child, DACL/owner/write-attribute/write-EA and generic-write authority. Retained no-delete handles and strict protected-object collision checks remain intact; SQLite captured-default-owner exception is unchanged.

These verdicts close this bounded production repair, not Stage 1, the native fixture batch, Windows CI or the release.
