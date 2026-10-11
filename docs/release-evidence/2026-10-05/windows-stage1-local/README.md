# Stage 1 local ownership evidence

This is macOS evidence for the working candidate, not native Windows acceptance
and not a release verdict. The associated production/fixture source is still
under review. The accepted plan snapshots preserve design and cancellation limits;
their original pre-implementation status lines describe when they were written.

- Fork: 15 tests in the RED run, 10 passed and five failed with immediate
  WriterAlreadyHeld. After awaited writer settlement, 17/17 passed. The JSON
  manifest records exact source hashes and command receipts. Synthetic error
  composition cases do not claim real operating-system close faults.
- Daemon: original RED includes three genuine ownership/completion failures and
  one overlong-socket setup failure. The separate valid-socket RED receipt proves
  the fourth behavioral failure. GREEN covers 24 tests across library, binary and
  inbox integration; 122 filtered tests were not executed in this focused run.
- The daemon test release signal was subsequently changed to per-completion
  stored oneshots after independent review found a lost-notification race. All
  three affected tests pass; the repair manifest binds their updated source.
- All 508 persistence library tests passed on macOS with Rust 1.98.1.
- Full-workspace all-target/all-feature strict Clippy passed with Rust 1.99.0,
  including UI, after correcting fixture lint findings. The full-workspace
  default-feature strict gate also passed. After final formatting, both gates
  passed again: 38.83 seconds all features and 32.94 seconds default features.
  Rust 1.99 formatting and git diff checks pass. The source hash manifest binds
  the final changed implementation/fixture inputs to baseline 0350d4a.
- macOS persistence strict Clippy passed in 13.07 seconds; cross-target strict
  Clippy for the actual Windows native/connection modules passed in 7.36 seconds.
  The latter is compiler evidence only and predates the later verbatim-alias test
  correction. Native review and behavioral acceptance remain pending. These
  receipts do not justify closing native Stage 1 or enabling private/preparation
  guards. Later source changes require affected gates to be rerun.

The next private namespace/preparation plan and the remaining native fixture
plan have independent pre-code acceptance. They are plans, not implemented or
executed requirements. Their status paragraphs retain exact reviewed body hashes.
