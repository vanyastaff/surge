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
