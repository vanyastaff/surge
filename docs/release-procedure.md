# Release and rollback procedure

The release delivers the CLI and sibling daemon. The optional desktop shell is not
in the archives. Publication requires separate authorization; preparing local
artifacts or running the branch workflow does not authorize a tag or release.

## Prepare the exact revision

1. Record the commit, workspace version and checks actually run. Keep accepted
   unfinished requirements open and distinguish skipped external tests from passes.
2. Run locked build, format, strict lint, nextest, doctests, deterministic ignored
   integration and dependency checks described in [Development](development.md).
   Packaging scripts require Python 3.11+. Run
   `python3.12 -m unittest discover -s scripts -p test_release.py`.
3. Build both executables and test the extracted archive with isolated `SURGE_HOME`
   and a temporary project: both `--version`, initialization, project description,
   terminal-only flow and daemon start/run/stop. Keep the binaries together.
4. Require successful native workflow evidence for all four configured archive
   targets before claiming four-platform binary readiness. Platform workflow smoke
   is not evidence of Windows workflow parity; see
   [runtime limitations](getting-started.md#windows-runtime-limitations).
5. Retain logs, archive checksums and exact revision identity. The release workflow
   calls reusable CI for its triggering revision and gates archive builds/publication
   on validation. A manual branch run produces artifacts without publishing.
   Finalize draft release notes only after evidence review and publication approval.

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
4. Validate copied databases read-only with SQLite `PRAGMA integrity_check` and
   `PRAGMA foreign_key_check`; record an inventory and hashes. Preserve the original
   paths and permissions, including private-input restrictions.

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
