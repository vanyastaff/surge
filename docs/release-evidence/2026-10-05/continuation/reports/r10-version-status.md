# Version cache correction — COMPLETE scoped validation

Source: crates/surge-orchestrator/src/engine/version_probe.rs (owned by R10); manifest dependency closure added by root: workspace which6 and orchestrator runtime which.workspace=true, old dev-only declaration removed. No version update requested or performed.

Behavior: preserves absolute invocation path without canonicalizing symlink leaf; bare program names resolve through existing which6. Distinct launcher aliases keep argv[0] behavior and separate cached results. Per-key Tokio OnceCell shares completed Result among concurrent callers; short map lock ends before process await, unrelated keys remain parallel. Cancellation before completed initialization permits retry, explicitly documented. Existing timeout 1s and child-process cancellation behavior unchanged.

Maintainer pre-code ACCEPTABLE; R26 independent spec/code review ACCEPTABLE (report /tmp/surge-release-r26-version-cache-review.md).

Evidence:
- RED command cargo test -p surge-orchestrator --lib version_cache_ -- --nocapture: 1 passed, 2 failed. Real alias version mismatch1vs2 and actual executable counter2vs1. Log /tmp/r10-version-red.log.
- Final GREEN command cargo test -p surge-orchestrator --lib engine::version_probe -- --nocapture: 12 passed,0 failed,0 ignored,420 filtered. Log /tmp/r10-version-green-final.log.
- Final cargo clippy -p surge-orchestrator --all-targets --all-features -- -D warnings: exit0. Log /tmp/r10-version-clippy-final.log.
- rustfmt --edition2024 --check ownedfile exit0; git diff --check ownedfile exit0.
All Cargo commands env CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 OPENSSL_STATIC=1 MACOSX_DEPLOYMENT_TARGET=15.0.

Initial clippy found production which missing (dev-only dependency) and nested type complexity after initial12-testGREEN; resolved by root manifest closure and private VersionCell alias. Initial failure retained /tmp/r10-version-clippy.log.

Tests: replaced ambient cargo cache test with owned POSIX shell executable and on-disk real call counter. Symlink aliases return distinct versions; concurrent callers require exactly one invocation. Cross-platform typed missing-path error cache test added. Windows execution not available, not claimed. Existing three ambient cargo/git probe tests remained and passed locally; cache regression itself is independent of installed cargo.

Source frozen, Cargo slot released to root. No commits by R10; root owns dependency-closure checkpoint and full workspace validation. Pre-existing timeout child settlement contract remains out of scope; no claim that this fix settles child descendants/effects.
