# Guardian G2a: pure receipt schema and canonical hashes

Scope: immutable validated claim DTOs, strict wire decoding and canonical SHA-256.
This adds no journal consumer, guardian process, Job ownership, authentication,
admission authority or recovery behavior. ADR-0023 and native recovery remain open.

## Executed local evidence

Host: macOS Apple Silicon. Rust 1.99.0, CARGO_INCREMENTAL=0, locked dependencies.

- API-absence compile RED is retained separately; it is not a runtime bug reproduction.
- First implementation: 6 tests passed, 1 failed because the Uncertain outcome
  accepted an extra field. An explicitly empty validated payload repairs that defect.
- Focused guardian-ledger suite: 15/15 PASS. One matrix test checks all 14 frozen
  preimages and digests; this does not mean 29 test functions ran.
- Full core library with all features: 887/887 PASS, no skips.
- Core all-target/all-feature Clippy with -D warnings: PASS.
- Formatting and scoped diff checks: PASS.
- Rust 1.96.0 all-target/all-feature core check: PASS; receipt retained separately.

The independent forward byte reader checks every field, exact input exhaustion,
digests, tag coverage and original oracles: all 14 PASS. Its original absolute
input paths are retained in the script to document the actual executed command.
The frozen vector SHA is af57237c6e7d33324b2164bd77340ae9d0a11e37828d35366574fcac2a705ab5.

## Independent verdicts

Phase 5a spec compliance: ACCEPTABLE after constructor, raw wire, phase/null,
context and encoding review and inspection of actual RED/GREEN receipts.
Phase 5b API/code quality: ACCEPTABLE, no blocking finding.
Phase 5b security: ACCEPTABLE, no actionable defect. All eight source hashes were
rechecked against the preserved source-freeze manifest. The parent owning-core
check confirms no dependency, unsafe, I/O, runtime or G1 change in this closure.

Nonblocking follow-ups: explicit public reexport lists would make future API
expansion easier to review; encoding a borrowed container could avoid a temporary
binding/endpoint clone. Neither changes the current accepted schema or gates.

G2a is COMPLETE within this pure schema scope. G2b transcript validation, native
process ownership, Windows CI and the release remain unfinished.
