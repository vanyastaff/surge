# Authentication-required diagnostic

Base: `d34d2429`. Scope: internal `surge-acp` classification only.

Fast path, lean review. Pre-code maintainer verdict: ACCEPTABLE. The existing
`pool::is_auth_failure` owns the shared auth text classification. Reuse it and the
existing `SendMessageError::AgentAuthenticationFailed`; no API, dependency,
runtime, credential, provider RPC, or logging changes are needed. The error path
keeps its existing lowercase allocation and gains one exact phrase check.

Acceptance: `Authentication required`, case variants, and ACP-style wrapped
messages become the existing actionable auth error, preserving raw details.
Authentication retains precedence over rate-limit text. Existing generic/rate
classification stays covered by the crate tests.

## Evidence

Durable local logs: `.rust-studio/evidence/auth-required-diagnostic/` (ignored).
Cargo uses `target/auth-required-diagnostic` because the pre-existing
`target/debug` symlink points to an absent directory. That initial infrastructure
failure is recorded as `red.log` and is not behavioral RED.

Build environment: `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_TEST_STRIP=symbols`,
`CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=target/auth-required-diagnostic`.

- RED: `cargo test --locked -p surge-acp --lib authentication_required` failed
  both new tests against unchanged production code (`red-classifier.log`).
  Bridge returned `Bridge(CommandSendFailed("Authentication required"))`.
  Compile finished in 1m 07s; tests finished in 0.00s. Test-first source hashes
  are in `red-source.sha256`.
- GREEN: same command passed 2 tests, 289 filtered out
  (`green-classifier.log`); compile 7.51s, tests 0.00s.
- `cargo test --locked -p surge-acp --all-targets`: passed
  387 tests, 4 ignored, 2 filtered out across test binaries
  (`tests-default.log`); includes 291 unit tests.
- `cargo test --locked -p surge-acp --all-targets --all-features`: passed
  388 tests, 4 ignored, 2 filtered out across test binaries
  (`tests-all-features.log`); `test-helpers` additionally enables the
  `bridge_worker_panic` integration target. Includes 291 unit tests.
- `cargo fmt --all --check`: passed (`fmt.log`).
- `cargo clippy --locked -p surge-acp --all-targets -- -D warnings`: passed
  (`clippy-default.log`), 31.54s.
- `cargo clippy --locked -p surge-acp --all-targets --all-features -- -D warnings`:
  passed (`clippy-all-features.log`), 4.89s.
- `git diff --check`: passed (`diff-check.log`).
- Final Rust source hashes: `final-source.sha256`.
- Independent quality/security review: COMPLETE, no blocking or advisory findings.
  Reviewer checked shared helper callers, auth precedence, details/guidance,
  upstream redaction, final source hashes and authentic RED-to-GREEN evidence.
  Phase 0 applies; formal 5a is skipped by fast-path rules and observable
  acceptance was checked in independent 5b. No tests were rerun by the reviewer.

The shared helper also changes pool authentication validation and configured
immediate-auth-failure handling for this phrase, using their existing branches.
This fixes diagnostic classification; it does not prove the historical login
cause, repair credentials, or establish full application acceptance. No commit,
push, or merge is performed by this builder. Repair count: 0/3.
Scoped diagnostic implementation COMPLETE after independent review. The full
project-lifecycle goal remains active; new native acceptance uses a distinct
durable fixture and is still unverified.
