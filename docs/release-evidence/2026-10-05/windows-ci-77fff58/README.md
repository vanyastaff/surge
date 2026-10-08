# Native CI receipt: 77fff58

[CI run](https://github.com/vanyastaff/surge/actions/runs/37676179568) completed on 2026-10-07. macOS and Ubuntu test suites, all three Clippy jobs, format, MSRV, dependency audit, packaging and benchmark checks passed. Windows nextest ran 3,640 tests: 3,589 passed, 51 failed, 33 skipped. This is not Windows release acceptance.

The retained terminated-child handle regression now passes natively. Git historical-checkpoint inspection and both persisted gate stream tests pass. Atomic config publication still fails with native sharing violation, including the retained-reader and concurrent-writer oracles. Failed publication leaves a temporary file in the old implementation; c93c744 addresses handle drop ordering, pending native confirmation. Neither failure is hidden by weakening assertions.

The next revision includes real named-pipe readiness and shell quoting fixes from 9a2a7b4; this receipt predates those changes. Private preparation and process ownership remain explicit Windows implementation gaps. Raw job bytes are preserved in windows-job.log.gz; manifest.json records the original SHA-256 and denominator.
