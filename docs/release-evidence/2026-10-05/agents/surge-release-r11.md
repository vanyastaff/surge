# R11 Data integrity

Scope: surge-persistence task transactions, memory schema/search, data isolation. SQLite probes executed against SQL extracted directly from schema.rs; no Cargo/build checks yet (coordinator serializes builds).

## P2 corrected: external-content FTS index corruption

memory/schema.rs eight UPDATE/DELETE triggers used ordinary UPDATE/DELETE against external-content FTS5 tables. AFTER UPDATE reads new source values when removing old postings; AFTER DELETE source row is already absent. Probe: oldword→newword yielded MATCH oldword=[(1,'newword')] and MATCH newword=[(1,'newword')]; rank=1 integrity-check returned checksum mismatch. After deleting row, querying FTS title returned missing-row/malformed database.

Implemented exact old-column FTS5 special delete commands for all four categories, reinsert new columns after UPDATE; schema v3 transactional migration replaces eight triggers and rebuilds four indexes to repair existing corrupt postings without changing source rows/claims. v1→v2 remains unchanged, existing v1 test fixture now creates complete v1 schema including FTS.

Severity caveat: no legacy update/delete method exists in current MemoryStore production API; demonstrated through legal database SQL and schema contract, not a reproduced current user-facing update flow.

Independent plan reviewer R12: ACCEPTABLE, before edits. Python test `/tmp/surge-r11-fts-regression.py` red before implementation: AssertionError discoveries old term remains. Same test green after implementation: 4/4 discoveries/patterns/gotchas/file_contexts update and delete MATCH semantics plus rank=1 integrity checks.

Rust regression tests added in memory/store.rs `fts_regression_tests`: all four categories fresh update/delete; persisted v2 stale/orphan postings upgrade and double reopen; repair failure rolls back prior trigger and version. Rust execution UNVERIFIED pending build slot. rustfmt passed on assigned files.

## P2 confirmed: isolated home ignored by usage/artifact defaults

persistence/store.rs:168 and artifacts.rs:81 derive dirs::home_dir instead of canonical surge_core::home::surge_home_dir. CLI analytics.rs:173,526,715 and insights.rs:112 use Store::default_path, so SURGE_HOME-isolated commands inspect/create real ~/.surge/usage.db. Minimal fix same resolver already used by MemoryStore::default_path. No edits (outside sole assigned write zone). Runtime probe not run to avoid touching user data.

## Non-blocking observations

Fresh MemoryStore initialize_schema executes schema and version insert without transaction; concurrent first opens can both observe absent version then one fail duplicate version. Deferred to coordinator/migration expert; not claimed as reproduced runtime failure.

Task mutation/settlement use immediate transactions and generation fences; usage accumulation uses cursor CAS, work-item active-attempt uniqueness has SQLite partial index. No additional concrete task-data blocker identified in scoped read.

Status: NEEDS WORK pending Rust tests/clippy and independent post-implementation review; SQL fix verified 4/4 tables. No commits by agent.

Coordinator owns final Cargo tests/review/commit. R27 canonical-home patch independently reviewed COMPLETE for correctness/scope (runtime checks pending).
