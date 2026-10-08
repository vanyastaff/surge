# R30 — recovery and rollback

## Evidence
Performed a synthetic temporary-runtime drill with Python sqlite3 against the repository's actual SQL migration files. No user/production runtime was opened or copied. Evidence: `/var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-r30-restore-13d8rpon/evidence.json`.

- Registry: all 30 SQL migrations executed; active WAL 1,174,232 bytes.
- Per-run: all 8 SQL migrations executed; active WAL 329,632 bytes.
- sqlite3 online backup on both connections retained an explicitly committed marker still in the live WAL.
- After closing all synthetic writers, copied the complete runtime into a snapshot, restored into a distinct directory, reopened both databases: integrity_check `ok`, foreign_key_check no violations, committed marker retained.
- Synthetic artifact bytes identical after restore.

This verifies SQLite backup mechanics and actual SQL compatibility with Python's SQLite; it does not claim Rust migration-runner execution or daemon/provider restart correctness. Cargo was intentionally not invoked while R22 owns execution coordination.

## Recovery boundary
Runtime database paths are `HOME/db/registry.sqlite` and `HOME/runs/<run-id>/events.sqlite`; per-run artifacts and private object references also matter. DB-only backups do not capture task worktree modifications, source Git ownership records, or private objects. Capture runtime, affected project source/Git metadata, task worktrees, and original old binaries/config together at a quiescent point. Preserve permissions and relative/absolute paths: restoring at new paths requires separate validation, not blind launch.

Never copy only SQLite main files while writers are active; committed records can be in WAL. Online backup gives a consistent individual DB, but independent database copies do not produce an application-wide cross-DB atomic snapshot. For release rollback use a quiescent full snapshot, not staggered online copies.

## Exact restore-only rollback procedure
1. Before upgrade: pause new intake and controls; stop the desktop and daemon cleanly using the documented CLI. Confirm all owned ACP agents, terminal subprocesses, MCP children, checkpoint writers, and external writers have settled. A daemon PID stopping alone is insufficient.
2. Record current binary versions, Git revision, runtime root, project roots, worktree paths, and configuration paths. Retain old binaries. Snapshot the complete runtime and affected project directories (including `.git`, retained worktrees, ignored runtime/private files) into an access-restricted backup destination with permission-preserving tooling. Do not prune or reset source/worktrees.
3. Validate copied registry/per-run databases read-only with integrity_check and foreign_key_check. Store snapshot inventory/hashes and location securely; it can contain credentials and prompts.
4. For rollback: stop upgraded components and settle their owned children again. Move the upgraded runtime/project state into a separate retained quarantine; never overlay the backup onto a newer database/WAL pair. Restore the entire pre-upgrade snapshot to its original paths with permissions preserved; use old binaries/config.
5. Check restored snapshot inventory, database integrity/foreign keys and project Git/worktree identity. Start old daemon with intake disabled, inspect recovered tasks and unresolved writer ownership, then permit user-authorized continuation. Do not rerun uncertain provider RPC/external effects automatically.
6. Retain quarantined post-upgrade state for reconciliation. Restoring pre-upgrade state loses subsequent local progress and cannot undo external effects; if effects occurred after snapshot, reconcile explicitly before restart.

There are forward-only schema migrations and no down migration; opening upgraded data with old binaries is not a rollback plan. No production backup or rollback was executed.

## R12 independent review
At first inspection R12 diff added storage.rs tests for six-migration legacy run upgrade/history retention and unknown-future migration refusal/released writer ownership. The current implementation had not yet been added: open_run_writer acquired an in-process token and OS file lock, then opened reader immediately. Final implementation must apply migrations while BOTH ownership guards are held, reject future migration IDs before modifying schema/PRAGMAs, and release guards on every failure. Test execution remains under the shared cargo owner.
