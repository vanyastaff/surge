# Native failure diagnostic checkpoint

The [parent receipt](../README.md) records the actual failures on 23460aa.

Profile replan repair 1 changes only the fixture's profile-ancestor owner expectation: exact actual user, LocalSystem or Builtin Administrators, matching the reviewed production ancestor policy. The newly created fixture remains current-user-only. All three backend assertion bodies are unchanged. Independent security review accepted the refined plan and checked source SHA `f6198bbe2ba36a21680993f81f54dd6bad17ae314da199e7f09655de33af8cb9`. Positive backend RED still needs native execution.

The concurrent configuration test now separates File::open from Read::read_to_string and records phase, raw OS error and progress counters. Every error remains terminal and the exact known-complete-content/TOML assertions remain intact. There is no production change, retry or claim that the intermittent read defect is fixed. Independent review accepted diagnostic source SHA `cf8adb0f69077047a1b653787d4a2647428285a8c3d0dbfe139b95b07c84eddf`.

Checks passed: direct rustfmt/diff checks; actual surge-core Windows-target all-target/all-feature strict Clippy (14.16 seconds); exact profile fixture Windows SDK/type compilation through the previously documented non-executable Storage signature harness, strict Clippy (0.41 seconds). The fixture harness is not production integration or native behavior evidence. Both commands used CARGO_INCREMENTAL=0. Logs and source hashes are retained; actual native execution remains open.
