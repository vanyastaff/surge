# Native Windows baseline at 0350d4a

Source: `0350d4a0f97433aed8b3322dec91a1b2cbe772b1`.
[CI run](https://github.com/vanyastaff/surge/actions/runs/37695764909),
[Windows test job](https://github.com/vanyastaff/surge/actions/runs/37695764909/job/113046988522).

The full suite ran **3,654 tests: 3,619 passed, 35 failed, 37 skipped**,
304.470 seconds. The original 34 failures remain: 25 private/preparation and nine
process-ownership cases. The additional failure is
`duplicate_active_rpc_id_closes_without_ambiguous_second_response`: the helper
failed to settle after endpoint revocation within its existing deadline. Cause
remains under investigation; no timeout increase or retry is a fix.

The concurrent configuration reader test passed in this run. That does not repair
or explain its AccessDenied failure at 23460aa. Phase diagnostics remain enabled.

The standard-user NTFS flush probe passed **1/1**. The separate ownership batch
failed **0/3**, now all at real backend assertions after successful fixture setup:

- Actual new home has an inherited, unprotected DACL instead of protected user-only
  security. The logged control value is 4, expected 4100.
- The derived manager permits database rename after disk connections retire.
- An unsafe inherited outsider grant is accepted instead of refusing before SQL.

Both positive fixtures recorded a trusted SYSTEM-owned profile ancestor and a
current-user-owned outer directory. These are genuine backend RED results, unlike
the earlier profile-lookup/ancestor setup failures. The working Stage 1 backend
addresses these assertions but is not included in this source revision.

All three platform Clippy jobs, macOS/Ubuntu tests, formatting, MSRV, dependency
policy, packaging contract and benchmark gates passed. Windows CI and release
acceptance remain open. Compressed logs preserve the exact source-bound evidence.
