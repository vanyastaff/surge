# R12 — schema migrations and rollback review

Scope: read-only repository inspection; no Rust builds, no user databases opened. SQL probes used Python sqlite3 in-memory databases.

## Evidence and findings

1. **P1 upgrade blocker — existing run databases never migrate.** `runs/storage.rs:232` is the sole production caller applying PER_RUN_MIGRATIONS and belongs to create_run. `open_run_reader` (:265) only opens a pool with pragmas. `open_run_writer` (:290) invokes that reader and spawn_writer; `writer_loop` (`runs/writer.rs:154`) only opens/pragmas. Current `reader_views.rs:26` selects known_cost_usd/cost_unknown and `views.rs:160,453` writes those fields/session_id. A run created by schema 0001–0006 therefore fails after upgrade. In-memory prior-schema probe produced `no such column: known_cost_usd`. Minimal fix: apply pending per-run migrations in a deliberate existing-run open upgrade path before constructing pools/spawning writer; preserve raw inspection's explicit no-migration contract. Add a regression seeded with genuine historical migrations and rows, then reopen via public Storage and read/write current projections.

2. **P2 migration regression fixture has drifted.** `work_item_upgrade_tests` at `runs/migrations.rs:466` uses REGISTRY_MIGRATIONS[..len()-1], now 29 migrations, despite claiming to check adding persistent tasks (0022). It tests 0030 instead. Minimal fix: find explicit 0022 id and slice preceding it, as verification_upgrade_tests already does for 0021.

3. **Rollback is restore-only and undocumented.** Runner is explicitly forward-only, commits each step independently; no down migrations or automatic backups. Registry `_migrations` skips known ids but does not reject unknown future ids, unlike memory/store version checks. Older executables opening new schema cannot be claimed safe. Release procedure must stop all writers, capture the entire resolved SURGE_HOME plus project-local runtime/config and associated worktrees, retain prior binary, then restore that complete snapshot on rollback. With WAL, copying only *.sqlite while writers run is unsafe; stop producers or use a consistent SQLite backup for each DB with coordinated filesystem snapshot. Backup validation and restore drill needed from recovery role.

4. **Memory upgrades have better atomicity tests.** `memory/store.rs:169` migrates v1→v2 within one transaction, preserves all legacy tables and backfills unverified claims. Tests around :2057–2294 cover all four legacy tables, id reuse, rollback on partial failure and cross-table ULID collisions. They exist but were not executed in this review. Fresh initialization DDL/version writes are not a single transaction; resumability relies on CREATE IF NOT EXISTS.

## Actually performed verification

- Read migration runner, storage opening paths, writer startup, reader/projection SQL, memory migration and tests, usage schema initialization, pragma settings.
- `rg` confirmed sole production per-run migration caller at create_run.
- Python sqlite3 applied all 30 registry and 8 per-run SQL files transactionally on separate fresh in-memory databases with foreign_keys=ON. Both PRAGMA integrity_check returned ok; foreign_key_check returned no violations.
- Genuine first six per-run SQL files applied in memory; current-reader column probe failed with no such column: known_cost_usd.

SQL checks prove DDL executes under host Python SQLite, not bundled rusqlite or public Rust API correctness. Rust migration/integration tests remain unverified pending coordinator gate.

Changes: report only. No repository files edited.

## Assigned implementation follow-up

Coordinator and R30 independently accepted writer-owned migration plan. Tests added first to `runs/storage.rs::existing_run_upgrade_tests`: genuine on-disk historical first-six schema, retained event/stage values and repeated reopen; unknown migration refusal before pragmas and ownership retry; missing run cannot create events DB. Production implementation prepared outside repository pending scheduled Cargo RED (shared build coordinator R05).

Independent post-edit review of R11 memory FTS fix: ACCEPTABLE, no blocking findings. All eight trigger payloads match external-content schemas, repair/rebuild/version share transaction, rollback test verifies earlier trigger replacement is rolled back. Bundled Rust tests remain pending.

## Implementation and actual RED

Changed `runs/storage.rs`: open_run_writer retains in-process token and OS FileLock, checks existing events file, then a dedicated READ_WRITE/no-CREATE connection rejects every unknown migration id before pragmas, applies pending supported migrations, and drops before reader pool/writer startup. Logging records validated schema boundary. Reader-only and raw inspection paths remain non-migrating.

Changed `runs/migrations.rs`: persistent-task upgrade test now uses explicit 0022 boundary.

`cargo test -p surge-persistence --lib existing_run_upgrade_tests -- --nocapture` executed by shared scheduler R26: **RED 1 passed / 2 failed**, exit 101. Actual missing known_cost_usd error and missing future-schema refusal reproduced; log `/tmp/surge-release-r12-red.log`. Production changes applied only after this RED evidence. GREEN pending scheduler.

## Actual GREEN (scheduler R26)

- `cargo test -p surge-persistence --lib existing_run_upgrade_tests -- --nocapture`: **3 passed, 0 failed**, 492 filtered; `/tmp/surge-release-r12-green.log`.
- `cargo test -p surge-persistence --lib work_item_upgrade_tests -- --nocapture`: **1 passed, 0 failed**, 494 filtered; `/tmp/surge-release-r12-registry-green.log`.
- Strengthened legacy upgrade regression also appends SessionOpened through actual writer and checks session_id projection after migration, on both reopens.
- `git diff --check -- crates/surge-persistence/src/runs/storage.rs crates/surge-persistence/src/runs/migrations.rs`: exit 0.

No commits made per coordinator instruction. Remaining limitations: reader-only old-schema access does not trigger upgrades (documented accessor rustdoc); raw inspection intentionally remains non-migrating; schema downgrade still restore-only; whole-workspace gate pending coordinator. Unknown migration guard is scoped to existing per-run writer opens, not global registry migration policy.
