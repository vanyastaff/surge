# Windows process liveness repair — 2026-10-07

Independent spec and unsafe/API/security review COMPLETE for implementation;
**native Windows GREEN pending**. OpenProcess success does not establish liveness.
The repair requests SYNCHRONIZE and polls the retained process object with
WaitForSingleObject(0). Signaled means dead, timeout means live. PID zero, access
failures and unexpected results stay conservatively live with tracing; explicit
invalid nonzero PID can be absent. OwnedHandle closes exactly once.

Native retained-child exit-259 RED was captured before this production change:
source `14884ff`, [job 112967493599](https://github.com/vanyastaff/surge/actions/runs/37672477232/job/112967493599).
[Raw receipt](../windows-ci-precursor/windows-14884ff.log.gz) reaches the intended
liveness assertion after child termination. No exit-code shortcut is used.

All Cargo checks used CARGO_INCREMENTAL=0.

| Command | Result | Receipt |
|---|---|---|
| `cargo test --locked -p surge-acp --lib process_tracker::tests` | macOS: 15 passed | [tests](process-mac.log.gz) |
| `cargo +1.99.0 clippy --locked -p surge-acp --all-targets --all-features -- -D warnings` | macOS: passed | [lint](mac-clippy.log.gz) |
| `cargo +1.96.0 check --locked -p surge-acp --all-targets --all-features` | macOS: passed | [MSRV](msrv-mac.log.gz) |
| Exact original module Windows cross-target Clippy 1.99 | passed; module only | [cross lint](source-cross-clippy.log.gz) |
| Exact original module Windows cross-target MSRV 1.96 | passed; module only | [cross MSRV](source-msrv-cross.log.gz) |
| Exact source rustfmt check and git diff check | passed | [format](fmt.log.gz) |

Whole ACP Windows cross-Clippy was attempted and [failed](cross-clippy.log.gz)
because ring needs MSVC headers absent on this Mac. It is not counted as a pass.
The external module probe includes the original production source via #[path],
including native tests; it does not copy/shim the implementation.
[Probe manifest](probe-Cargo.toml) and [source include](probe-lib.rs) document scope.
Executed probe commands used --manifest-path /tmp/surge-windows-a2-native-typecheck/Cargo.toml,
--all-targets --target x86_64-pc-windows-msvc, --offline and the shared workspace
CARGO_TARGET_DIR; Clippy used -D warnings, MSRV check additionally --locked.
[Manifest](manifest.json) binds source and receipts. Native CI remains decisive.
