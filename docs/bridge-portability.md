# ACP bridge portability evidence

Issue [#7](https://github.com/vanyastaff/surge/issues/7) requires native execution on
Linux, macOS and Windows at one exact source revision. Cross-compilation and older
CI runs do not establish this evidence. Each platform is pending until its manifest
records `complete` for the selected revision, with the mandatory tests passing.

## Reproduce

Start from a clean checkout of the selected commit. Install the repository Rust
stable toolchain, Python 3.9+ and `cargo-nextest`. Run on the native host:

```sh
python3 -m unittest discover -s scripts -p test_bridge_portability.py -v
python3 scripts/bridge_portability.py --output /tmp/surge-bridge-evidence
```

On Windows use `python` and a fresh output path such as
`C:\Temp\surge-bridge-evidence`. The output directory must not already exist;
keep it outside the working tree or inside an ignored directory. The runner builds
the locked ACP crate and `mock_acp_agent`, lists the full ACP all-features inventory,
and runs that inventory through the `bridge-portability` nextest profile. All three
commands explicitly select the host triple reported by `rustc -Vv` and one target
directory, overriding Cargo configuration that might otherwise cross-compile. The
native mock's absolute path is injected into nextest commands and recorded in the
receipt. `CARGO_BUILD_TARGET` must be unset.

`manifest.json` records the exact Git SHA, before/after identity, native OS and
architecture, Rust/Cargo/nextest versions, CI run URL when present, every command,
exit code, elapsed time and relative stdout/stderr log filenames. The original
inventory log and JUnit XML are retained with SHA-256 artifact digests. Counts come
from actual test cases, with ignored inventory listed separately; no historical
fixed test count is used. An absent required test, ignored required test, failing
command, timeout, missing or malformed evidence, dirty checkout or changed HEAD
makes the receipt incomplete and returns a nonzero exit code. Command timeouts
bound runner waits; they do not constitute evidence of process-tree cleanup. The
lifecycle tests provide their own child-settlement observations. JUnit retains
captured successful and failing test output.

The existing CI test matrix runs this command on all three native platforms before
the broad workspace suite. Its always-uploaded artifacts are named
`bridge-portability-<runner OS>-<Git SHA>`. A failing runner remains a failing CI
step. `workflow_dispatch` can rerun the existing workflow against a selected branch;
compare every artifact's `revision` field, rather than combining results from
separate revisions.

## Required coverage

| Requirement | Test |
|---|---|
| Crash observed within the documented bound | `crash_after_n_tool_calls_surfaces_within_2s` |
| Close bounded against a stuck agent | `close_against_stuck_mock_times_out` |
| Five sessions operate independently | `five_concurrent_sessions_complete_independently` |
| Shutdown ends every opened session | `shutdown_emits_session_ended_for_each_open_session` |
| Cancelling an opening settles worker and child | `cancelled_open_settles_worker_and_child` |
| Dropping during opening requests bounded cleanup and child exit is observed | `bare_drop_during_opening_is_bounded_and_reaps` |
| Native explicit/fallback executable discovery in isolated processes | `mock_executable_discovery_uses_native_paths` |

`Drop` requests asynchronous cleanup. Its return does not prove synchronous child
reaping. The bare-Drop test bounds the return of the asynchronous cleanup request
and separately observes eventual child disappearance; it does not confirm worker
join. Explicit shutdown awaits worker completion. Process observation and
elapsed-time assertions run natively.
Discovery scenarios mutate only their isolated child's environment, preserving the
parent test environment and accommodating the native executable suffix.

## Timing contracts

The crash test measures from prompt dispatch to the observed prompt error and
session termination, with a two-second bound. It does not timestamp the mock's
internal crash instruction. The stuck-close test exercises the fixed five-second
prompt grace period, then requires close to return before its configured
15-second shutdown limit; a 30-second outer timeout bounds the entire scenario.
The ordinary shutdown scenario uses a separate 60-second outer safety timeout.
JUnit test duration includes setup and cleanup, so it is not substituted for any
of these trigger-to-observation assertions.

## Current workspace verification

Tested source revision: `ec0341a787a8f57a730184a9bcb4a98e135f23d6`.
[CI run 38230456119](https://github.com/vanyastaff/surge/actions/runs/38230456119)
completed successfully with **all 11 jobs green**. Native results below all belong
to this same SHA; the subsequent evidence-only commit preserves that identity.

| Native platform | Workspace passed | Workspace skipped | Bridge passed | Bridge inventory ignored |
|---|---:|---:|---:|---:|
| Linux x86_64 | 4169 | 36 | 383 | 4 |
| macOS arm64 | 3941 | 37 | 383 | 4 |
| Windows x86_64 | 3940 | 55 | 388 | 5 |

There were zero failures. All seven mandatory bridge scenarios passed on every
host. The [retained evidence](bridge-evidence/ec0341a787a8f57a730184a9bcb4a98e135f23d6/README.md) includes native
manifests, JUnit, inventories, all job logs, final workflow status and file hashes.
All 63 bridge artifact digests were independently checked after copying.

Windows additionally passed **48/48** native probes under an ordinary user on
NTFS: 33 persistence, eight process, one MCP and six orchestrator. The
[standard-user manifest](bridge-evidence/ec0341a787a8f57a730184a9bcb4a98e135f23d6/windows-standard-user/manifest.json)
and separate logs retain actual named test results, SID, elevation-zero and NTFS
receipts. Recorded executable hashes agree across results; binary bytes are not
retained for independent rehashing.

All configured doctest steps and strict default/all-features workspace Clippy
passed. The existing CI includes UI tests/doctests on Linux; macOS and Windows
test/doctest commands exclude surge-ui. Clippy includes all crates on all three
platforms. Ignored integration smoke commands execute on Linux only. Formatting,
MSRV 1.96, packaging, stage-transition benchmark and dependency checks also passed.

The Windows implementation now covers private Start preparation, native process
containment and recovery, retained filesystem/SQLite ownership and normal sidefile
retirement. Recovery fixtures wait for actual ownership, gate registration and
completion boundaries. The locked mio dependency is 1.2.1, which fixes the upstream
Windows named-pipe completion ownership race; daemon readiness requires an actual
Ping response. Earlier failures and cancellations remain in the
[intermediate ledger](bridge-evidence/intermediate/README.md).

## Original bridge evidence ledger

Initial source revision: `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4`.
The macOS receipt was produced locally on Darwin 27.0.0 arm64 with Rust/Cargo
1.98.1 and cargo-nextest 0.9.145, starting at 2026-10-12 01:25:56 UTC. It is local
native evidence, not a CI run (`job_url` is null). HEAD was identical and the working
tree clean before and after execution. The documentation commit follows the tested
source revision and does not change the identity of that evidence.

The inventory contains **387 tests: 383 passed and 4 ignored**. JUnit contains
383 executed test cases, with zero failures and zero skips; all seven required
coverage tests passed. The four ignored inventory rows, including the isolated
subprocess helper, are listed in the manifest and do not count as executed cases.
CI run [38192988110](https://github.com/vanyastaff/surge/actions/runs/38192988110)
produced complete native Linux, macOS and Windows receipts for this exact SHA.
Linux and macOS each have a 387-test inventory and 383 passed cases; Windows has
386 inventory tests and 382 passed cases. The inventories differ by seven Unix-only
tests and six Windows-only tests; the required bridge scenarios exist on every host.
Each platform has four ignored inventory
rows, zero failures, zero JUnit skips and all seven required tests passing. These
receipts retain Rust/Cargo 1.99.0 and nextest 0.9.148; Linux and Windows are x86_64,
macOS is arm64. All 21 artifact digests per receipt were verified after copying.
A complete bridge receipt describes the bridge step only; the broader workflow
status is independent.

The exact runner invocation was:

```sh
CARGO_INCREMENTAL=0 python3 scripts/bridge_portability.py --output /tmp/surge-bridge-portability-7f93714-macos
```

The [manifest](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/macos/manifest.json)
contains the exact build/list/run commands, native target, mock-path environment
overrides and elapsed times. Byte-preserved [inventory](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/macos/inventory.stdout.log),
[JUnit](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/macos/junit.xml)
and [nextest log](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/macos/tests.stderr.log)
are retained alongside every command's separate stdout/stderr logs. All 21 artifact
SHA-256 values were verified against the manifest after copying.

| Native platform | Source SHA | Evidence | Status |
|---|---|---|---|
| macOS | `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4` | [Local native receipt](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/macos/manifest.json) | complete: 383 passed, 4 inventory ignored |
| macOS CI | `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4` | [Native receipt](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/macos-ci/manifest.json), [CI job](https://github.com/vanyastaff/surge/actions/runs/38192988110/job/114629544365) | bridge complete: 383 passed, 4 inventory ignored |
| Linux | `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4` | [Native receipt](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/linux/manifest.json), [CI job](https://github.com/vanyastaff/surge/actions/runs/38192988110/job/114629544362) | bridge complete: 383 passed, 4 inventory ignored |
| Windows | `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4` | [Native receipt](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/windows/manifest.json), [CI job](https://github.com/vanyastaff/surge/actions/runs/38192988110/job/114629544341) | bridge complete: 382 passed, 4 inventory ignored |

## Broader CI outcome for the initial revision

Run [38192988110](https://github.com/vanyastaff/surge/actions/runs/38192988110)
completed with overall `failure`. The retained [workflow/job status JSON](bridge-evidence/7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/workflow-status.json)
records a successful native bridge evidence step on all three operating systems;
subsequent workspace suites failed. These results establish the #7 bridge coverage
at the selected SHA, not successful verification of the whole workspace.

| Broader workspace suite | Passed | Failed | Skipped |
|---|---:|---:|---:|
| Linux | 4084 | 7 | 36 |
| macOS | 3858 | 5 | 37 |
| Windows | 3733 | 67 | 53 |

Investigation found recovery/continuation and gate-identity fixture failures on
Unix. Windows also exposed unsupported private Start preparation in quota fixtures,
artifact setup before run creation, temporary runtime-home/handle settlement,
duplicate exclusive reservation and unresolved MCP readiness failures. These are
broader suite failures at the initial revision. The current workspace verification
above records the subsequent repairs and successful three-platform evidence at
one later source revision.
