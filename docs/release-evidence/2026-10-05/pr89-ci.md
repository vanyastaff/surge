# PR #89 CI follow-up

Failed run: [37349909332](https://github.com/vanyastaff/surge/actions/runs/37349909332),
head `db3f28308fbc6792bc8832da88f0c7a19d3b0c2e`.

The CI stable toolchain was Rust 1.99.0; prior local source receipts used 1.98.1.
All three Clippy jobs stopped at eight `double_must_use` errors in generated ACP
trait declarations; the first local workspace retry also exposed the same issue
in MCP. The final fix updates the locked async-trait dependency from 0.1.89 to
0.1.92, whose published source removes the generated bare attribute. The earlier
manual trait draft was discarded: production trait declarations remain unchanged.
The upstream MSRV is 1.71, below Surge's 1.96; syn 3.0.6 was already locked.
Only this package's lock entry changed. No lint suppression or toolchain downgrade
is used. [Upstream source](https://github.com/dtolnay/async-trait/blob/0.1.92/src/expand.rs).

A shared CLI startup race caused the Ubuntu lifecycle failure: accepted launch
can precede per-run schema creation. The reader now waits for the exact registry
row that create_run publishes after migrations before strict folded inspection.
A deterministic pre-migration SQLite fixture reproduced the original missing
`events` table; the same test passes with the publication fence, and still
rejects invalid published schema. Actual CLI lifecycle tests passed 6/6; scoped
CLI strict clippy passed. R26 independently reviewed both production changes.

Original full-suite outcomes: Ubuntu 3,831 passed / 3,842 run, 11 failed,
35 skipped; macOS 3,601 passed / 3,619 run, 18 failed, 37 skipped. Windows stopped
at a platform-specific flags compilation error; its repair is deferred under
the user-directed macOS scope. Seven additional macOS failures depend on the actual mock ACP executable, which
CI previously built only after the suite and only on Ubuntu. The build now occurs
before nextest on macOS and Ubuntu. The additional catalog deadline failure was
independently reproduced by delaying Python startup 250 ms: initialization timed
out before any catalog requests. The fixture now waits for an atomically published
actual-PID startup marker before the protocol timer begins; the original 120/500 ms
catalog budgets, two-page oracle, 200 ms tool response delay and exact transport
checks remain. The causal RED failed with the same missing request file; all
13 protected diagnostic tests then passed. This is fixture readiness, not managed
process coverage. All seven affected daemon cases passed on the local Mac with the actual mock
agent prepared (7 run, 7 passed, 65 outside the selected filter);
the ten previously known MCP acceptance cases remain open. No failing check is
skipped, removed or treated as passing.

New source changes make the prior release archive an historical candidate;
it is not a compiled artifact of this CI repair revision. New verification
receipts below are scoped and do not replace final release gates.

The broader Rust 1.99 gate also exposed 42 `assert_is_empty` diagnostics (29 initially, 13 after downstream checks ran) in
existing test modules after the macro errors stopped blocking downstream checks.
Those assertions now compare the same collection length against zero, preserving
evaluation and the original empty/nonempty condition without adding element
PartialEq bounds. Independent review confirmed no production or oracle weakening.
The first full retry exhausted disk during compilation; its failure is retained
as infrastructure failure, not a successful lint receipt. Only generated test
executables in Cargo's debug cache were removed to permit a clean retry; release
archives and source/evidence were preserved.

## Verified repair receipts

- Rust 1.99.0 strict workspace Clippy, all targets/all features: PASS, 20.73 s.
- Rust 1.99.0 strict workspace Clippy, all targets/default features: PASS, 39.98 s.
- Final locked-dependency protected diagnostics suite on Rust 1.98.1: 13/13 PASS.
- Final locked-dependency selected macOS daemon regression cases on Rust 1.98.1: 7/7 PASS.
- Formatting, actionlint and diff check: PASS.

These are local macOS receipts; they are not the new hosted CI result. Future
incompatibility warnings for block/proc-macro-error2 remain disclosed.

- Final locked CLI lifecycle suite: 6/6 PASS.
- Final deterministic CLI publication regression: 1/1 PASS.

[Raw receipts and SHA-256 manifest](pr89-ci/manifest.json) preserve successful
scoped checks and failed RED/lint/infrastructure attempts. Runtime regressions
used Rust 1.98.1; full lint gates used the actual CI Rust 1.99.0.

- MSRV 1.96.0 workspace/all targets excluding UI: PASS, 2m40s, using the
  documented macOS 27 `RUSTFLAGS='-C strip=none'` workaround and locked offline
  dependencies. The new async-trait version is verified against the actual MSRV.

## Final critical-suite outcome

Verified source revision: `dd6357a` (build/lint, CLI publication and fixture/CI
stages committed separately). Full owned-flow MCP and work-item-route suites
on the local Mac: **72 run, 62 passed, 10 failed, zero skipped**, 30.942 s,
exit 100. The remaining failures are exactly the original ten MCP acceptance
cases. Extra CI macOS failures are not present in this run. This is two critical
integration binaries, not a fresh full workspace test receipt.

Release remains NO-GO; the current PR cannot be described as all checks green.
Managed macOS execution/effect settlement and the original positive recovery
requirements remain unresolved. Windows-specific compilation remains deferred
under the user's macOS scope. Hosted CI for the pushed repair is a separate
verification step.
