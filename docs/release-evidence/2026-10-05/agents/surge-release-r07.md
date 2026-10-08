# Role 7 — static analysis expert

Verdict: NEEDS WORK / type and lint execution pending coordinated build slot.

## Evidence
- Read root Cargo.toml, clippy.toml, rustfmt.toml, justfile, ci.yml, security.yml, release.yml and development lint instructions.
- `cargo fmt --all --check`: exit 0, no output; PASS on the inspected tree (2026-10-05).
- No cargo compilation launched: coordinator reserved baseline release build first.
- Exact current static gate: `cargo clippy -p surge-core --all-targets --all-features -- -D warnings`; `cargo clippy -p surge-acp --all-targets -- -D warnings`; `cargo clippy --workspace --all-targets --all-features`.

## Findings / proposed fixes
1. Workspace lint warnings are non-blocking in both CI and justfile. Strictness covers 2 of 13 crates only. Proposed: after a confirmed clean baseline, enforce `-D warnings` on workspace pass in CI and local recipe. Do not suppress warnings or assert clean without running it. Keep default-feature clippy evidence too because all-features can mask cfg-specific warnings.
2. Tagged release workflow checks metadata/build/archive smoke but does not depend on final-source fmt, clippy, unit/integration tests or cargo audit. CI only targets branches main/develop and PRs; a new tag can publish without checks for that revision. CI owner should require release validation of the tag before publish.
3. Critical mocked engine integration test step uses `continue-on-error: true`. Existing explanation includes real external-runtime tests, so split deterministic mock coverage from genuinely optional external-agent checks before making required subset blocking.
4. `clippy.toml` thresholds/test exceptions configure lints; they do not enable restriction lints. No workspace.lints or crate unwrap_used/expect_used/print_stdout/print_stderr enforcement found at entry points/manifests. Thus AGENTS prohibitions are review rules rather than mandatory automation. Scoped follow-up advisable; do not enable broad pedantic/restriction groups as release refactor.
5. Large pre-existing crate-wide clippy allowances remain, including too_many_lines, casts and unused_async. A zero-warning invocation will not prove their covered behaviors checked. Removing them wholesale is out of scope; report honestly.

## Changes / residual risk
- No repository edits (read-only assignment).
- Report artifact only.
- Clippy/type checks remain unverified until coordinator releases build slot and final changes settle; formatting should be rerun at integration.
