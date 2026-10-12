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

## Native evidence ledger

Selected source revision: `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4`.
The macOS receipt was produced locally on Darwin 27.0.0 arm64 with Rust/Cargo
1.98.1 and cargo-nextest 0.9.145, starting at 2026-10-12 01:25:56 UTC. It is local
native evidence, not a CI run (`job_url` is null). HEAD was identical and the working
tree clean before and after execution. The documentation commit follows the tested
source revision and does not change the identity of that evidence.

The inventory contains **387 tests: 383 passed and 4 ignored**. JUnit contains
383 executed test cases, with zero failures and zero skips; all seven required
coverage tests passed. The four ignored inventory rows, including the isolated
subprocess helper, are listed in the manifest and do not count as executed cases.
Linux and Windows remain pending for this same SHA; no older CI result substitutes
for either native receipt.

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
| Linux | `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4` | pending | pending |
| Windows | `7f937140f84be2df7133c55aeeb2dd01f5b1d5f4` | pending | pending |
