# Required integration checks and native restore drill

Verdict remains **NO-GO**. Ten MCP acceptance failures and required other native
platform evidence remain open. Artifact identity stays `8e3a781`.

## Required local integration allowlist

The justfile's `test-ignored` commands ran individually after fresh builds.
All **11 tests passed**: ACP classification/reconnect 2, engine pipeline/concurrent
runs/crash resume 3, MCP stdio 3, controlled daemon 1 and daemon restart 2.
The controlled watchdog/child duplicate summaries count as one selected scenario.
No real provider, subscription, account or downloaded third-party MCP was used.
Tests ran at documentation checkpoint `062008c`; the Rust/manifest/lock diff from
`8e3a781` was empty. These separate tests do not rewrite the default nextest
denominator (3,833 passed, 10 failed, 37 skipped).

## Actual native rollback

The pinned `3136b82` archive created two completed terminal runs. The complete
stopped runtime, project and binaries were snapshotted. The pinned `8e3a781`
archive replayed both and created a third run. After stopping, upgraded state
was retained in quarantine and the snapshot restored to the original paths.

Before reopening, all **111 entries** matched bytes, modes and symlink targets.
Three SQLite databases passed integrity/foreign-key checks. Old binary identity,
original Git HEAD and clean project state were verified. The old daemon replayed
both original runs; the newer run was absent from restored history and retained
in quarantine. All **29 commands** returned zero; owned PID/socket settlement
passed. Live runtime files change after use; exact equality applies before reopen.

The first attempt failed because SQLite `mode=ro` created WAL/SHM sidecars in the
writable snapshot directory. The corrected drill makes an exact disposable
validation copy for SQLite checks, preserving the retained snapshot and exact
restore assertions. The [procedure](../../../release-procedure.md) now says so.
Both attempts are preserved.

This checks same-schema terminal-only restoration, not schema downgrade,
provider/MCP recovery, external-effect rollback or production restoration.
No user data was touched; temporary snapshot/quarantine state is retained.

## Evidence

- [Independent review](independent-review.md), [manifest](manifest.json),
  [raw hashes](raw-files.json) and [verbatim logs](raw-logs.tar.gz).
- [Successful receipt](native-rollback-result.json),
  [snapshot inventory](snapshot-inventory.json) and [drill source](native-rollback.py).
- [First failed source](native-rollback-first.py) and
  [failed receipt](native-rollback-first-result.json).

The scripts are evidence helpers pinned to this workspace's local archives and
owned temporary fixtures, not production recovery commands. Raw logs include
build/test outputs and both native command transcripts.
