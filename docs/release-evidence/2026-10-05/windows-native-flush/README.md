# Windows native durability design gate

This implements the actual Windows `flush_complete` production primitive and a required CI probe under a dedicated standard account on local fixed NTFS. It grants no private preparation or authenticated-input capability; existing unsupported guards remain closed. The probe records effective/default owner, queried directory owner and ACL metadata, elevation, filesystem and requested rights before attempting flags-zero file and directory flush. Its readonly negative control requires STATUS_ACCESS_DENIED.

Independent implementation/spec and security/unsafe reviews are COMPLETE after repair 1. Final checks, all with CARGO_INCREMENTAL=0:

- Actual production module via #[path], real windows 0.58/thiserror/tempfile dependencies: `cargo clippy --offline --manifest-path /tmp/surge-native-flush-actual-module/Cargo.toml --target x86_64-pc-windows-msvc --all-targets -- -D warnings` — PASS.
- The same actual module on MSRV 1.96: command is retained verbatim in actual-module-windows-msrv.log.gz — PASS.
- `cargo clippy --locked -p surge-persistence --lib --tests -- -D warnings` on macOS — PASS.
- Persistence formatting — PASS, exact command in persistence-fmt.log.gz.

Full persistence Windows cross-validation stopped before this module because the local machine lacks MSVC stdlib.h. The module-only checks do not represent whole-crate or native success. PowerShell is unavailable locally. Native behavior and the real dedicated-user orchestration require the next Windows CI result.

The mandatory probe runs after the existing full nextest suite even when that suite fails. It copies only the actual persistence test executable to an isolated directory, verifies the exact standard-user SID inside the child, bounds execution and retained-process teardown, emits isolated diagnostics, and removes its ephemeral account/profile/directory after settlement. Credentials are not printed. Native launch or identity failure fails the gate; no existing test is disabled.

Local ENOSPC occurred during formatting before final checks. Only derived incremental cache was removed while Cargo was idle; successful final receipts bind the resulting source hashes. Source and receipt SHA-256 values are preserved in manifest.json; gzip files contain the original command output.
