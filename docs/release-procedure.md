# Release and rollback procedure

The final release remains NO-GO until the remaining
[macOS v1 acceptance and release gates](plans/2026-10-05-002-feat-v1-release-plan.md)
pass. MCP restart recovery now follows the implemented ADR-0021 contract;
managed VM/effect-broker proof is no longer a release prerequisite. A fresh
candidate needs matching source, native build and notice provenance receipts.
See [current readiness](release-readiness-2026-10-05.md) for revision-bound evidence.

The existing archive packaging delivers the CLI and sibling daemon; the desktop
shell is not included in those archives. The macOS v1 plan also requires bundled
desktop acceptance before its final GO. Publication requires separate authorization; preparing local
artifacts or running the branch workflow does not authorize a tag or release.

## Prepare the exact revision

1. Record the commit, workspace version and checks actually run. Keep accepted
   unfinished requirements open and distinguish skipped external tests from passes.
2. Run locked build, format, strict lint, nextest, doctests, deterministic ignored
   integration and dependency checks described in [Development](development.md).
   Packaging scripts require Python 3.11+. Run
   `python3.12 -m unittest discover -s scripts -p 'test_release*.py'`.
3. Build both executables and test the extracted archive with isolated `SURGE_HOME`
   and a temporary project: both `--version`, initialization, project description,
   terminal-only flow and daemon start/run/stop. Use a clean committed temporary
   Git source, ignore local config/runtime state, and commit generated context
   before Flow admission. `daemon stop` acknowledges the request: wait for owned
   shutdown before starting again. Keep the binaries together.
4. Require successful native workflow evidence for all four configured archive
   targets before claiming four-platform binary readiness. Platform workflow smoke
   is not evidence of Windows workflow parity; see
   [runtime limitations](getting-started.md#windows-runtime-limitations).
5. Retain logs, archive checksums and exact revision identity. The release workflow
   calls reusable CI for its triggering revision and gates archive builds/publication
   on validation. A manual branch run can produce artifacts only after validation
   passes and never publishes.
   Finalize draft release notes only after evidence review and publication approval.

## Local macOS candidate

For an Apple Silicon candidate, use the same linkage and minimum OS policy as CI:

```sh
python3.12 scripts/release_notices.py --target aarch64-apple-darwin --output-dir target/release-proof --graph-only
OPENSSL_STATIC=1 MACOSX_DEPLOYMENT_TARGET=15.0 python3.12 scripts/release_native.py build --native-host --target aarch64-apple-darwin --metadata target/release-proof/production-metadata.json --dependency-ids target/release-proof/production-dependency-ids.json --output target/release-proof/build-messages.jsonl
python3.12 scripts/release_native.py produce --target aarch64-apple-darwin --metadata target/release-proof/production-metadata.json --dependency-ids target/release-proof/production-dependency-ids.json --build-messages target/release-proof/build-messages.jsonl --bin-dir target/release --output target/release-proof/native-provenance.json --strip-tool strip
python3.12 scripts/release_notices.py --target aarch64-apple-darwin --output-dir target/release-proof/notices --native-provenance target/release-proof/native-provenance.json
python3.12 scripts/release.py package --target aarch64-apple-darwin --bin-dir target/release --notices-dir target/release-proof/notices --output target/release-candidate
```

Use the release toolchain Rust 1.98.1, whose standard-library notices are pinned
in `scripts/notice-sources/runtime-reviewed.json`; the workspace MSRV remains 1.96.
A toolchain upgrade requires reviewing its runtime notice inventory.
Run these steps from committed source, without edits during the build. The native
producer records the Cargo stream, selected inputs, linkage and binary hashes
before and after stripping. Each macOS dependency path must begin with `/usr/lib/`
or `/System/Library/`. Confirm
minimum OS 15.0 in `otool -l` for both binaries. A source build without these
settings can link Homebrew OpenSSL and is not a self-contained archive candidate.
Use the appropriate native target on other platforms; no cross-platform claim
follows from these local commands. Run the isolated extracted-archive scenario
recorded in the [evidence index](release-evidence/2026-10-05/README.md).

The complete-notice check refuses missing, stale or unresolved evidence. To
inspect gaps, add `--inventory-only` to the notice command and use a separate
output directory; diagnostic receipts are never packageable. New archives require
a sixth member, `THIRD_PARTY_NOTICES.txt`, and a paired target provenance JSON.
Collection requires all four archive/receipt pairs and hashes all eight assets.
The historical five-member ARM64 archive remains review evidence; it does not
satisfy the new notice contract and must not be overwritten.

## Back up before upgrading

Migrations are forward-only. Event payload versions, SQLite storage migrations and
memory database versions are separate; see [Schema versioning](schema-versioning.md).
Existing legacy run storage upgrades through the writer-owned resume/open path;
reader-only access and raw inspection do not migrate old projections.

1. Pause new intake and controls. Close the desktop and stop the daemon with
   `surge daemon stop`. Confirm owned ACP agents, terminal subprocesses, MCP children,
   checkpoint writers and external writers have settled. A stopped daemon PID alone
   does not establish that all writers have stopped.
2. Record binary versions, Git revision, resolved runtime root (`SURGE_HOME`, or
   `~/.surge`), project roots, worktree and configuration paths. Retain old binaries.
3. Use permission-preserving tooling to snapshot the entire runtime and affected
   project directories, including `.git`, retained worktrees, ignored runtime files
   and private objects. Store the snapshot in an access-restricted destination.
   Do not prune, reset or delete source or worktrees.
4. Record the retained snapshot inventory and hashes. Make a separate complete
   disposable validation copy, including WAL/SHM files, and confirm its initial
   inventory matches the retained snapshot. Validate its databases read-only with
   SQLite `PRAGMA integrity_check` and `PRAGMA foreign_key_check`. A `mode=ro`
   connection can create sidecars in a writable directory; do not validate against
   the retained immutable snapshot directly. Recheck that retained snapshot's
   inventory afterward. Preserve original paths and permissions, including
   private-input restrictions.

Registry state is under `<runtime>/db/registry.sqlite`; run state is under
`<runtime>/runs/<run-id>/events.sqlite`. Copying only SQLite main files while writers
are active can omit committed WAL records. An online SQLite backup gives one
consistent database, but independent copies do not provide an application-wide
snapshot across databases, private objects and worktrees. Use a quiescent complete
snapshot for rollback.

## Restore-only rollback

1. Stop upgraded components and settle their owned children again.
2. Retain upgraded runtime and project state in a separate quarantine. Never overlay
   a backup onto newer database/WAL files.
3. Restore the complete pre-upgrade snapshot to its original paths with permissions
   preserved; restore the old binaries and configuration together.
4. Check the inventory, database integrity/foreign keys and Git/worktree identity.
   Start the old daemon without new intake, inspect recovered tasks and unresolved
   ownership, then allow authorized continuation. Do not automatically repeat
   uncertain provider RPCs or external effects.
5. Retain quarantined post-upgrade state for reconciliation. Restoring loses local
   progress after the snapshot and cannot undo external effects; reconcile those
   effects explicitly before resuming.

A preparation drill using synthetic temporary runtime databases applied the actual
registry and run SQL migrations, preserved committed WAL markers through SQLite
backup and complete quiescent restore, and checked integrity, foreign keys and
artifact bytes. This tests backup mechanics with Python SQLite; it does not prove
Rust daemon restart, provider recovery or a production restore. Test the procedure
in an isolated copy of your deployment before relying on it.

A subsequent [native rollback drill](release-evidence/2026-10-05/followup/README.md)
used the actual `3136b82` and `8e3a781` archive binaries in an isolated temporary
project. Two completed terminal runs survived binary replacement and full
snapshot restore; a third post-snapshot run remained in quarantine. Exact
111-entry byte/mode/symlink inventory, three SQLite integrity/foreign-key checks,
old binary identity, original Git HEAD and daemon restart passed. This checks
same-schema terminal-only restoration, not schema downgrade, provider recovery,
external-effect rollback or production restoration.
