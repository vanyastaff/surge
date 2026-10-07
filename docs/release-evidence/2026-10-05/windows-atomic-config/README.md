# Windows atomic configuration implementation — 2026-10-07

Implementation scope independently accepted; **native Windows acceptance pending**.
The Windows helper publishes the retained temporary object with native POSIX
replacement, without unlink/retry/readonly override. Unix save behavior is retained.
Four Windows-only tests cover retained readers, continuous exact-byte reads with
eight writers, readonly failure preservation/cleanup, and Unicode publication.
Existing directory-failure assertions remain unchanged.

Existing native concurrent-save failure in baseline `87ad59a` is RED; new
Windows tests were not executed before implementation. Cross-target checks below
prove compilation/lint, not Windows behavior. All Cargo commands below used
`CARGO_INCREMENTAL=0`; receipts are gzip-compressed original output bytes.

| Command | Result | Receipt |
|---|---|---|
| `cargo test -p surge-core --lib` | macOS: 864 passed, 1.08s | [core](core.log.gz) |
| `cargo check --locked -p surge-core --tests --target x86_64-pc-windows-msvc` | Rust 1.98: passed, 23.23s | [cross compile](cross-check.log.gz) |
| `cargo +1.99.0 clippy --locked -p surge-core --all-targets --target x86_64-pc-windows-msvc -- -D warnings` | passed, 10.28s | [cross lint](cross-clippy-green.log.gz) |
| `cargo +1.99.0 clippy --locked -p surge-core --all-targets -- -D warnings` | macOS: passed, 9.78s | [Mac lint](mac-clippy.log.gz) |
| `cargo +1.96.0 check --locked -p surge-core --all-targets --target x86_64-pc-windows-msvc` | passed, 17.88s | [Windows MSRV compile](msrv-cross.log.gz) |
| `cargo +1.96.0 check --locked -p surge-core --all-targets` | macOS: passed, 14.05s | [Mac MSRV](msrv-mac.log.gz) |
| Exact edited-source rustfmt check | passed | [format](fmt-final.log.gz) |

[Initial cross lint](cross-clippy.log.gz) caught nested-test structure and readonly
restoration issues; both were corrected without lint suppression before passing.
Independent spec and unsafe/API/security reviews returned COMPLETE for this
implementation. [Manifest](manifest.json) binds exact source and receipts.
[Plan](../../../plans/2026-10-07-002-fix-windows-ci-plan.md) retains native acceptance
and remaining Windows CI work. This is not Windows GO or release GO.

## Cleanup repair after dependency inspection

The initial implementation/review missed tempfile 3.27 field drop order:
NamedTempFile stores path before file, so armed TempPath cleanup ran before the
READ-only-shared handle closed. This can leave a temporary file on errors.
The repaired private OwnedTemporary declares file first and path second, built
from into_parts without reopening; early errors close before pathname cleanup.
Success still immediately disarms cleanup. Sharing and native rename flags stay
unchanged. Fresh spec and unsafe/API/security review COMPLETE for the repair.

CARGO_INCREMENTAL=0 checks passed: Windows cross-target Rust 1.99 core Clippy
all-targets ([receipt](repair1-cross-clippy.log.gz)), Windows core MSRV 1.96
all-targets ([receipt](repair1-msrv-cross.log.gz)), macOS core strict Clippy
([receipt](repair1-mac-clippy.log.gz)), and exact-source format/diff check
([receipt](repair1-fmt.log.gz)). The macOS binary is unchanged by this Windows-only
repair; its prior 864-test receipt is retained without a redundant rerun.
Native old-wrapper failure receipts and repaired cleanup GREEN are pending.
Original source hashes describe the historical initial implementation; repair1
in the manifest binds current corrected helper bytes. No release GO implied.
