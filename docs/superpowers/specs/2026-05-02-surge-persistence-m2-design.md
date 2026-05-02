# M2 — `surge-persistence` storage layer for vibe-flow event log

**Status:** Design
**Date:** 2026-05-02
**Predecessor:** [M1 — surge-core foundation](2026-05-02-surge-core-m1-adaptation-design.md)
**Related architecture docs:** `docs/revision/05-storage.md`, `docs/revision/02-data-model.md`, `docs/revision/03-engine.md`

---

## 1. Goal

Add per-run SQLite event log + materialized views + per-run git worktree integration to `surge-persistence`, while keeping the existing legacy persistence (aggregator/budget/pricing/memory/store) untouched. Pure-addition strategy, same as M1.

This milestone unblocks M3 (engine), which needs to:
- Append `RunEvent`s atomically into a per-run event log
- Read materialized views for branch evaluation, summaries, dashboards
- Run code in an isolated git worktree per run
- Recover from crashes via WAL + event replay

### 1.1 In scope

- Per-run SQLite event log with append-only invariant (WAL mode, single writer, multi-reader)
- Engine-side materialized views maintained in the same transaction as event append:
  `stage_executions`, `artifacts`, `pending_approvals`, `cost_summary`, `graph_snapshots`
- Registry DB (cross-run) with the `runs` table only
- Atomic content-addressed artifact storage on the filesystem
- Per-run git worktree extensions in `surge-git` (extend `GitManager`, do not split)
- Crash recovery test: kill writer mid-flight, reopen, verify integrity
- Pre-requisite addition to `surge-core`: `RunId::short()` and `RunStatus::Crashed`

### 1.2 Out of scope (deferred to later milestones)

- `profiles`, `templates`, `trusted_files`, `trusted_projects` registry tables — M3+
- `surge export <run_id>` / `surge import <tarball>` — M3+
- Real V1→V2 schema migration chain — added when the payload format actually changes
- Replay scrubber UI — M11 (but `graph_snapshots` are written now to avoid retroactive backfill)
- Engine, ACP integration, profile inheritance — entirely separate milestones
- Trait abstractions for storage mocking — defer until M5 engine has concrete test pain (see §2.4)

## 2. Strategy

### 2.1 Live inside `surge-persistence`, no new crate

Per the migration-strategy decision (no parallel "v1"/"v2" crates), all M2 work lives inside `surge-persistence` as a `runs` submodule. Legacy modules (`aggregator`, `budget`, `memory`, `models`, `pricing`, `store`) remain operational and untouched. Eventually those legacy paths may be re-implemented on top of the new event log; nothing in M2 prevents that.

### 2.2 SQLite library: rusqlite (not sqlx)

Legacy already uses `rusqlite 0.32` (bundled). Adding `sqlx` would mean two SQLite libraries inside one crate. Beyond that:

- The single-writer pattern in §6 maps naturally to one dedicated `tokio::task` with an `mpsc` command channel. SQLite physically doesn't allow concurrent writers in WAL mode anyway, so a connection pool for writes adds complexity without benefit.
- Reads happen through an `r2d2_sqlite` pool wrapped in `tokio::task::spawn_blocking`.
- `sqlx::query!` compile-time SQL checks require a live database or `sqlx-data.json` at build time, both painful in CI and cross-compilation. Tests + strong types via `ToSql`/`FromSql` cover ~90% of the same reasoning.

Stick with rusqlite. If a future milestone needs async-first storage, migration is local to `runs/`, not cross-crate.

### 2.3 Event payload encoding: JSON (M1 decision carried forward)

`VersionedEventPayload` already serializes via `serde_json::to_vec` in M1 ([surge-core/src/run_event.rs:213](../../crates/surge-core/src/run_event.rs)) — bincode 1.x doesn't support `deserialize_any` which is required by the untagged-enum patterns in `EventPayload`. The API method names `to_bincode`/`from_bincode` are kept for forward-compatibility with bincode 2.x, but the bytes today are JSON.

Tradeoff vs binary: ~30% larger storage; debuggable via `surge show event` and `sqlite3` `json_extract`. Acceptable for current event volumes (~500–2000 events per 30-min run, total run DB <5 MB).

### 2.4 No traits for mocking

`:memory:` SQLite is fast enough for unit tests (full event-log roundtrip in single-digit ms). Concrete types are simpler. If M5 engine accumulates real test pain (>100 ms of SQLite setup per unit test), introduce `EventLog`/`Registry` traits with `MockEventLog` then. Designing a trait now without a real consumer is guessing.

## 3. Filesystem layout

```
~/.surge/
├── config.toml                    # global config; [storage] and [worktrees] sections
├── db/
│   └── registry.sqlite            # registry DB (cross-run metadata)
└── runs/
    └── <run_id>/                  # full ULID display, e.g. "run-01HF2K9X..."
        ├── config.toml            # per-run config
        ├── events.sqlite          # per-run event log + materialized views
        ├── events.sqlite-wal
        ├── events.sqlite-shm
        ├── events.sqlite.lock     # fd-lock advisory file lock — held by current writer
        └── artifacts/             # content-addressed atomic writes
            └── <name>             # e.g. "spec.md", "plan.md"
```

Worktree directory is **not** under `~/.surge/runs/` — it's a sibling of the source repo: `<repo_parent>/.surge-worktrees/<short_id>/` (see §7).

`<run_id>` is the full ULID display (`"run-{ulid}"`); `<short_id>` is `RunId::short()` (10 chars without prefix). Full IDs go everywhere uniqueness matters; short IDs only in human-facing places (worktree paths, branch names, log lines).

### 3.1 `~/.surge/config.toml` — minimal schema

```toml
[storage]
checkpoint_interval_seconds = 300          # explicit wal_checkpoint(TRUNCATE) cadence
reader_pool_size = 4                       # r2d2 pool max_size per run-DB
writer_channel_capacity = 64               # mpsc bound for WriterCommand

[worktrees]
location = "sibling"                       # "sibling" (default) | "central" | <abs path>
```

All fields have sane defaults. M2 reads only the keys it understands; future sections can be added without breaking M2.

Note: the storage `home` directory is not in the config file — it's the directory containing this `config.toml`, derived from the `Storage::open(home)` argument. Putting `home` inside `config.toml` would create a chicken-and-egg cycle (need home to find config, need config to know home). Callers that want a non-default home pass it directly to `Storage::open` (or through `SURGE_HOME` env var, resolved by the CLI before calling `Storage::open`).

## 4. Database schemas

### 4.1 Registry DB — `~/.surge/db/registry.sqlite`

```sql
CREATE TABLE runs (
    id TEXT PRIMARY KEY,                  -- full RunId display ("run-{ulid}")
    project_path TEXT NOT NULL,
    pipeline_template TEXT,
    status TEXT NOT NULL,                 -- 'bootstrapping'|'running'|'completed'|'failed'|'aborted'|'crashed'
    started_at INTEGER NOT NULL,          -- unix epoch ms
    ended_at INTEGER,
    daemon_pid INTEGER                    -- nullable; cleared on clean exit, stale on crash
);
CREATE INDEX idx_runs_status ON runs(status);
CREATE INDEX idx_runs_started ON runs(started_at DESC);

CREATE TABLE _migrations (
    id TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
);
```

`status` is a string enum mirroring `surge_core::run_state::RunStatus` (extended with `Crashed` in §8.2).

### 4.2 Per-run DB — `~/.surge/runs/<run_id>/events.sqlite`

```sql
CREATE TABLE events (
    seq INTEGER PRIMARY KEY,              -- monotonic, no gaps; SQLite ROWID alias
    timestamp INTEGER NOT NULL,           -- unix epoch ms
    kind TEXT NOT NULL,                   -- discriminant string from VersionedEventPayload
    payload BLOB NOT NULL,                -- serde_json::to_vec(VersionedEventPayload); see §2.3
    schema_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_ts   ON events(timestamp);

-- Append-only invariant: defense in depth against bugs and bad migrations
CREATE TRIGGER trg_events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(FAIL, 'events table is append-only'); END;

CREATE TRIGGER trg_events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(FAIL, 'events table is append-only'); END;

-- Materialized views — engine-side maintained in same transaction as the event INSERT.
-- Triggers are NOT used for view maintenance (limited expressiveness); they're only
-- used above for the append-only constraint.

CREATE TABLE stage_executions (
    node_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    started_seq INTEGER NOT NULL,
    ended_seq INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    outcome TEXT,
    cost_usd REAL DEFAULT 0,
    tokens_in INTEGER DEFAULT 0,
    tokens_out INTEGER DEFAULT 0,
    PRIMARY KEY(node_id, attempt)
);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,                  -- ContentHash display
    produced_by_node TEXT,
    produced_at_seq INTEGER NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,                   -- relative to <run_dir>/artifacts/
    size_bytes INTEGER NOT NULL,
    content_hash TEXT NOT NULL            -- redundant with id; indexed separately for de-dup queries
);
CREATE INDEX idx_artifacts_node ON artifacts(produced_by_node);
CREATE INDEX idx_artifacts_name ON artifacts(name);

CREATE TABLE pending_approvals (
    seq INTEGER PRIMARY KEY,              -- the ApprovalRequested event seq
    node_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    requested_at INTEGER NOT NULL,
    payload_hash TEXT NOT NULL,
    delivered INTEGER DEFAULT 0,          -- bool as int (rusqlite convention)
    message_id INTEGER                    -- Telegram message_id if delivered
);
CREATE INDEX idx_approvals_node ON pending_approvals(node_id);

CREATE TABLE cost_summary (
    metric TEXT PRIMARY KEY,              -- 'tokens_in'|'tokens_out'|'cost_usd'|'cache_hits'
    value REAL NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE graph_snapshots (
    at_seq INTEGER PRIMARY KEY,
    snapshot BLOB NOT NULL,               -- serde_json::to_vec(RunState); see §2.3
    bytes_compressed INTEGER NOT NULL     -- snapshot byte length, for monitoring
);

CREATE TABLE _migrations (
    id TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
);
```

### 4.3 Sequence allocation

Inside the writer task, each event INSERT uses SQLite's `RETURNING` clause (3.35+, supported by bundled rusqlite) to get the assigned `seq` in one round-trip:

```rust
let seq: i64 = tx.query_row(
    "INSERT INTO events (timestamp, kind, payload, schema_version)
     VALUES (?, ?, ?, ?) RETURNING seq",
    params![ts_ms, kind, payload_blob, SCHEMA_VERSION],
    |row| row.get(0),
)?;
```

For `AppendBatch` commands, each INSERT inside the same transaction returns its own `seq`. The transaction holds the write lock end-to-end, so no other writer can interleave.

## 5. Public API

### 5.1 Module surface

```rust
// crates/surge-persistence/src/runs/mod.rs

pub use error::{StorageError, OpenError, CloseError, WriterError};
pub use seq::EventSeq;
pub use registry::{RunSummary, RunFilter, RunStatus};
pub use reader::{RunReader, StageExecution, ArtifactRecord, PendingApproval, CostSummary};
pub use writer::WriterCommand;        // public for testing only; not normally constructed by callers
pub use run_writer::RunWriter;
pub use storage::Storage;
```

### 5.2 `Storage`

```rust
pub struct Storage { /* registry pool, active_writers map, config */ }

impl Storage {
    /// Open or create `~/.surge/`, apply registry migrations, load config.
    /// Returns `Arc<Self>` because RunReader/RunWriter hold strong references.
    pub async fn open(home: impl AsRef<Path>) -> Result<Arc<Self>, OpenError>;

    /// Create a new run: insert into registry, init per-run DB with migrations,
    /// open the worktree, return an exclusive writer.
    pub async fn create_run(
        self: &Arc<Self>,
        run_id: RunId,
        project_path: impl AsRef<Path>,
        config: RunConfig,
    ) -> Result<RunWriter, OpenError>;

    /// Open a read-only handle to an existing run.
    pub async fn open_run_reader(self: &Arc<Self>, run_id: RunId) -> Result<RunReader, OpenError>;

    /// Open the exclusive writer for an existing run.
    /// Fails with `OpenError::WriterAlreadyHeld` if another writer (this process or another)
    /// currently holds the slot. See §6.3 for enforcement.
    pub async fn open_run_writer(self: &Arc<Self>, run_id: RunId) -> Result<RunWriter, OpenError>;

    // Registry queries
    pub async fn list_runs(&self, filter: RunFilter) -> Result<Vec<RunSummary>, StorageError>;
    pub async fn get_run(&self, run_id: &RunId) -> Result<Option<RunSummary>, StorageError>;

    /// Delete a run (registry row + per-run dir + worktree).
    /// Fails if a writer is currently held for this run. Caller must ensure the run is terminated.
    pub async fn delete_run(&self, run_id: &RunId) -> Result<(), StorageError>;
}
```

`Storage::list_runs` and `get_run` perform stale-pid detection (§8.2) — runs with `status='running'` whose `daemon_pid` no longer points to a live process are returned with `status=Crashed` and the registry row is updated in-place.

### 5.3 `RunReader`

```rust
pub struct RunReader {
    storage: Arc<Storage>,
    run_id: RunId,
    pool: r2d2::Pool<SqliteConnectionManager>,
    artifacts_dir: PathBuf,
    worktree_path: PathBuf,
}

impl RunReader {
    pub fn run_id(&self) -> &RunId;
    pub fn worktree_path(&self) -> &Path;

    // Event log
    pub async fn current_seq(&self) -> Result<EventSeq, StorageError>;
    pub async fn read_events(&self, range: Range<EventSeq>) -> Result<Vec<RunEvent>, StorageError>;
    pub async fn read_event(&self, seq: EventSeq) -> Result<Option<RunEvent>, StorageError>;

    /// Polling-based subscription. Every 100 ms, queries events with seq > last_seq and yields them.
    /// Per-tick batch is capped at SUBSCRIBE_BATCH_MAX (256) to bound memory if consumer lags.
    /// Cancel-safe: dropping the stream releases all resources.
    pub fn subscribe_events(&self) -> impl Stream<Item = Result<RunEvent, StorageError>> + Send + 'static;

    // Materialized view queries
    pub async fn stage_executions(&self) -> Result<Vec<StageExecution>, StorageError>;
    pub async fn artifacts(&self) -> Result<Vec<ArtifactRecord>, StorageError>;
    pub async fn pending_approvals(&self) -> Result<Vec<PendingApproval>, StorageError>;
    pub async fn cost_summary(&self) -> Result<CostSummary, StorageError>;

    // Snapshots (for replay scrubber, M11)
    pub async fn latest_snapshot_at_or_before(&self, seq: EventSeq)
        -> Result<Option<(EventSeq, RunState)>, StorageError>;
    pub async fn list_snapshots(&self) -> Result<Vec<EventSeq>, StorageError>;

    // Artifact FS read
    pub async fn read_artifact(&self, content_hash: &ContentHash) -> Result<Vec<u8>, StorageError>;
}
```

### 5.4 `RunWriter`

```rust
pub struct RunWriter {
    _token: Arc<WriterToken>,             // in-process slot (drop frees slot)
    _file_lock: fd_lock::RwLockWriteGuard<'static, File>,  // cross-process slot
    writer_tx: mpsc::Sender<WriterCommand>,
    writer_join: tokio::task::JoinHandle<Result<(), WriterError>>,
    reader: RunReader,                    // delegate read methods through macro
}

impl RunWriter {
    pub async fn append_event(&self, payload: VersionedEventPayload) -> Result<EventSeq, StorageError>;
    pub async fn append_events(&self, payloads: Vec<VersionedEventPayload>)
        -> Result<Vec<EventSeq>, StorageError>;

    /// Wait for all previously-issued commands (append/store_artifact/write_snapshot) to
    /// be processed by the writer task and committed. Does NOT trigger WAL checkpoint
    /// or fsync — those are managed by the writer task via periodic `wal_checkpoint(TRUNCATE)`.
    pub async fn flush(&self) -> Result<(), StorageError>;

    pub async fn store_artifact(&self, name: &str, content: &[u8])
        -> Result<ArtifactRecord, StorageError>;

    pub async fn write_graph_snapshot(&self, at_seq: EventSeq, state: &RunState)
        -> Result<(), StorageError>;

    /// Truncate all materialized view tables and rebuild from events.
    /// Runs inside a single transaction; readers see pre-rebuild state until commit
    /// (WAL gives them a snapshot view), so there is no transient empty-view window
    /// from a reader perspective.
    pub async fn rebuild_views(&self) -> Result<(), StorageError>;

    /// Send Shutdown to the writer task. The task processes any pending mpsc commands,
    /// commits them, then exits. Joins the task. After `close().await`, this writer is
    /// consumed; the cross-process file lock and in-process token are released.
    /// Future calls to writer methods after close() are not possible (consumed).
    /// Prefer over Drop for clean shutdown — Drop is fire-and-forget fallback.
    pub async fn close(self) -> Result<(), CloseError>;
}

// Read methods delegated via `delegate_to_reader!` macro (~10 lines macro + 10 lines invocation):
// run_id, worktree_path, current_seq, read_events, read_event, subscribe_events,
// stage_executions, artifacts, pending_approvals, cost_summary,
// latest_snapshot_at_or_before, list_snapshots, read_artifact
```

### 5.5 Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("writer already held for run {run_id}")]
    WriterAlreadyHeld { run_id: RunId },
    #[error("run not found: {0}")]
    RunNotFound(RunId),
    #[error("migration failed: {0}")]
    MigrationFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("single-threaded tokio runtime not supported by Storage; use multi-threaded runtime")]
    SingleThreadedRuntime,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization failed: {0}")]
    SerializationFailed(#[from] serde_json::Error),
    #[error("writer task died unexpectedly")]
    WriterTaskDied,
    #[error("pool error: {0}")]
    Pool(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CloseError {
    #[error(transparent)]
    Writer(#[from] WriterError),
    #[error("writer task join failed: {0}")]
    JoinFailed(String),
}
```

`StorageError` and the legacy `PersistenceError` deliberately do not have `From` between them — they belong to different domains and conflating them would obscure the source of failures.

## 6. Internals

### 6.1 Writer task

One dedicated `tokio::task` per `RunWriter`, NOT `spawn_blocking`. The task is sequential and doesn't share its thread with anything else; rusqlite's blocking calls block only this task's thread. This requires the multi-threaded tokio runtime — single-threaded runtime would starve the rest of the process during long writes.

`Storage::open` checks `tokio::runtime::Handle::current().runtime_flavor() == RuntimeFlavor::MultiThread` at startup and returns `OpenError::SingleThreadedRuntime` if not — fail loud, fail fast. The check is also documented in `Storage::open`'s rustdoc.

```rust
enum WriterCommand {
    AppendEvent {
        payload: VersionedEventPayload,
        reply: oneshot::Sender<Result<EventSeq, WriterError>>,
    },
    AppendBatch {
        payloads: Vec<VersionedEventPayload>,
        reply: oneshot::Sender<Result<Vec<EventSeq>, WriterError>>,
    },
    StoreArtifact {
        name: String,
        content: Vec<u8>,
        reply: oneshot::Sender<Result<ArtifactRecord, WriterError>>,
    },
    WriteSnapshot {
        at_seq: EventSeq,
        state: Box<RunState>,
        reply: oneshot::Sender<Result<(), WriterError>>,
    },
    RebuildViews {
        reply: oneshot::Sender<Result<(), WriterError>>,
    },
    Flush {
        reply: oneshot::Sender<Result<(), WriterError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}
```

`mpsc::channel(64)` provides bounded backpressure — if writer can't keep up, callers block on `send().await` instead of running away with memory. Each command opens a transaction, performs the write + view maintenance + (for artifacts) atomic FS write, commits, replies via `oneshot`. Writer task runs `tokio::time::interval(checkpoint_interval_seconds)` for periodic explicit `PRAGMA wal_checkpoint(TRUNCATE)` to keep `-wal` file bounded.

Tracing: each writer task has a `tracing::info_span!("writer_task", run_id = %run_id)` covering its entire lifetime; each command opens a child `debug_span!` with command kind and resulting `seq`.

### 6.2 Reader pool

```rust
let manager = SqliteConnectionManager::file(&events_path)
    .with_init(|conn| apply_read_pragmas(conn));
let pool = r2d2::Pool::builder()
    .max_size(config.reader_pool_size)         // default 4
    .build(manager)?;
```

Reads go through `tokio::task::spawn_blocking({ let pool = self.pool.clone(); move || pool.get()?.query_row(...) })`. Pool is small intentionally — too many reader connections increase WAL reader-tracking overhead and don't add throughput (SQLite serializes individual queries).

### 6.3 Single-writer enforcement (two layers)

**In-process** — `Storage::active_writers: tokio::sync::Mutex<HashMap<RunId, Weak<WriterToken>>>`. `open_run_writer` holds the mutex, checks `Weak::upgrade()` for an existing strong reference, creates `Arc<WriterToken>`, stores `Weak` in map, returns the Arc inside `RunWriter`. On `RunWriter` drop, the Arc drops → next `open_run_writer` succeeds.

```rust
struct WriterToken;  // empty marker

impl Storage {
    pub async fn open_run_writer(self: &Arc<Self>, run_id: RunId) -> Result<RunWriter, OpenError> {
        let mut active = self.active_writers.lock().await;
        if let Some(weak) = active.get(&run_id) {
            if weak.strong_count() > 0 {
                return Err(OpenError::WriterAlreadyHeld { run_id });
            }
        }
        let token = Arc::new(WriterToken);
        active.insert(run_id.clone(), Arc::downgrade(&token));
        drop(active);
        // ... acquire file lock, spawn writer task, build RunWriter
    }
}
```

**Cross-process** — `fd_lock::RwLock` advisory lock on `events.sqlite.lock`. RunWriter holds `RwLockWriteGuard`. On drop the guard releases; another process can then acquire.

```rust
let lock_file = std::fs::OpenOptions::new().create(true).read(true).write(true).open(&lock_path)?;
let mut lock = fd_lock::RwLock::new(lock_file);
let guard = lock.try_write().map_err(|_| OpenError::WriterAlreadyHeld { run_id })?;
// keep `lock` and `guard` inside RunWriter (lifetime via `'static` after Box+leak or owned lock)
```

SQLite-level locks (BEGIN EXCLUSIVE) are NOT used for the writer slot — they would block readers in WAL mode, defeating the multi-reader goal. Used only for migrations (§6.5).

### 6.4 Materialized view maintenance

`runs::views::maintain(tx, seq, payload)` — single `match` over `EventPayload` variants, performs SQL updates inside the same transaction as the event INSERT:

| EventPayload variant      | View update |
|---------------------------|-------------|
| `StageEntered`            | INSERT into `stage_executions` |
| `StageCompleted`          | UPDATE `stage_executions` (set ended_seq, ended_at, outcome) |
| `StageFailed`             | UPDATE `stage_executions` (set ended_seq, ended_at, outcome=NULL) |
| `ArtifactProduced`        | INSERT into `artifacts` (size, hash from FS metadata) |
| `TokensConsumed`          | UPDATE `cost_summary` (tokens_in, tokens_out, cache_hits) |
| `ApprovalRequested`       | INSERT into `pending_approvals` |
| `ApprovalDecided`         | DELETE FROM `pending_approvals` for that node |
| (other variants)          | no-op |

### 6.5 Migration runner

```rust
fn apply_migrations(conn: &mut Connection, migrations: &[(&str, &str)]) -> Result<(), MigrationError> {
    // BEGIN EXCLUSIVE — multi-process safe; only one writer can run migrations at a time
    let tx = conn.transaction_with_behavior(TransactionBehavior::Exclusive)?;

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (id TEXT PRIMARY KEY, applied_at INTEGER NOT NULL)"
    )?;

    for (id, sql) in migrations {
        let exists: bool = tx.query_row(
            "SELECT 1 FROM _migrations WHERE id = ?",
            params![id],
            |_| Ok(true),
        ).optional()?.unwrap_or(false);

        if !exists {
            tx.execute_batch(sql)?;
            tx.execute("INSERT INTO _migrations (id, applied_at) VALUES (?, ?)",
                params![id, now_ms()])?;
        }
    }

    tx.commit()?;
    Ok(())
}
```

`TransactionBehavior::Exclusive` ensures that if two processes start `Storage::open` simultaneously, they serialize cleanly; second one will see migrations already applied.

### 6.6 Subscribe stream

```rust
const SUBSCRIBE_BATCH_MAX: usize = 256;

pub fn subscribe_events(&self) -> impl Stream<Item = Result<RunEvent, StorageError>> + Send + 'static {
    let pool = self.pool.clone();
    async_stream::try_stream! {
        let mut last_seq = EventSeq::ZERO;
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let new = read_events_after(&pool, last_seq, SUBSCRIBE_BATCH_MAX).await?;
            for ev in new {
                last_seq = ev.seq;
                yield ev;
            }
        }
    }
}
```

Batch cap (256) bounds memory if writer is far ahead of consumer — slow consumer just catches up over multiple ticks instead of allocating a giant Vec. `MissedTickBehavior::Skip` prevents tick burst after consumer pause. Stream is cancel-safe (dropping aborts the polling task).

In-process notify channel is a future enhancement (M2.5+): writer task holds `Vec<broadcast::Sender<EventSeq>>`, push after commit, subscribers in the same process `select!` on broadcast + interval. M2 is pure polling for cross-process safety.

### 6.7 Atomic artifact write

```rust
async fn store_artifact_inner(&self, name: &str, content: &[u8]) -> Result<ArtifactRecord, WriterError> {
    let target = self.artifacts_dir.join(name);
    let dir = target.parent().unwrap_or(&self.artifacts_dir);
    let tmp = NamedTempFile::new_in(dir)?;
    tmp.as_file().write_all(content)?;
    tmp.as_file().sync_all()?;        // fsync before rename
    tmp.persist(&target)?;             // atomic rename
    let hash = ContentHash::from_bytes(content);
    Ok(ArtifactRecord { id: hash, name: name.into(), path: target, size_bytes: content.len() as u64, content_hash: hash })
}
```

`tempfile::NamedTempFile::persist` uses platform-native atomic rename (`renameat2` on Linux, `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows). FS write happens before the row INSERT into `artifacts` view inside the same transaction; if FS write fails the transaction rolls back, so the artifacts table never references a missing file (modulo the inverse — file exists but row rolled back, in which case file is orphaned but harmless and cleaned up by `surge gc`).

## 7. Worktree management — extensions to `surge-git::GitManager`

### 7.1 New methods on existing `GitManager`

Existing legacy methods (`create_worktree(spec_id, ...)`, `commit/diff/discard/merge` keyed on `spec_id: &str`) are left unchanged. New methods accept `&RunId`:

```rust
impl GitManager {
    pub fn create_run_worktree(
        &self,
        run_id: &RunId,
        base_branch: Option<&str>,
        location: WorktreeLocation,
    ) -> Result<RunWorktreeInfo, GitError>;

    pub fn run_worktree_path(&self, run_id: &RunId, location: WorktreeLocation) -> PathBuf;

    /// Prune the worktree from git's bookkeeping, remove the directory on disk,
    /// and delete the `surge/run-{short}` branch. Symmetric with legacy `discard(spec_id)`.
    pub fn discard_run_worktree(&self, run_id: &RunId) -> Result<(), GitError>;

    pub fn commit_run_worktree(&self, run_id: &RunId, message: &str) -> Result<git2::Oid, GitError>;

    pub fn merge_run_worktree(
        &self,
        run_id: &RunId,
        target_branch: Option<&str>,
        checkout: bool,
    ) -> Result<git2::Oid, GitError>;

    /// Detect worktrees whose .git/worktrees/<name>/gitdir points to non-existent paths.
    /// Caused by user moving the repo, deleting worktree dirs manually, etc.
    pub fn find_orphaned_worktrees(&self) -> Result<Vec<OrphanedWorktree>, GitError>;

    /// `git worktree prune` for orphaned. Returns count removed.
    /// Surfaced via CLI as `surge doctor --fix` and `surge gc`.
    pub fn prune_orphaned_worktrees(&self) -> Result<u32, GitError>;
}

pub enum WorktreeLocation {
    Sibling,                              // <repo_parent>/.surge-worktrees/<short_id>/  — default
    Central,                              // ~/.surge/runs/<run_id>/worktree/
    Custom(PathBuf),                      // explicit absolute path
}

pub struct RunWorktreeInfo {
    pub run_id: RunId,
    pub path: PathBuf,
    pub branch: String,                   // "surge/run-{short_id}"
    pub exists_on_disk: bool,
}

pub struct OrphanedWorktree {
    pub name: String,
    pub recorded_path: PathBuf,
}
```

### 7.2 Branch naming

`surge/run-{run_id.short()}`, e.g. `surge/run-01HF2K9X3M`. Distinguishable from legacy `surge/{spec_slug}` (slug-like names like `surge/auth-feature`) by the `run-` prefix and the alphanumeric format.

Fallback: if `short()` length is changed in the future, branch parser should accept any alphanumeric suffix after `run-`.

### 7.3 Default location

`Sibling`: `<repo_parent>/.surge-worktrees/<short_id>/`. Rationale:
- Doesn't pollute main repo's `git status` (not inside the repo)
- IDE workspace search (VS Code, IntelliJ "Find in Files") doesn't see worktree files — matters for projects with multiple concurrent runs
- Same volume as repo (git worktree requires same filesystem)

### 7.4 Cross-volume safety

If user moves the repo across filesystems, all existing worktrees break (git stores absolute `.git/worktrees/<name>/gitdir` paths). Mitigation: `find_orphaned_worktrees()` + `prune_orphaned_worktrees()` exposed via `surge doctor --fix` and `surge gc`. Not auto-run — destructive.

### 7.5 Internal refactor for code reuse

The existing `worktree.rs` private helpers (`open_repo`, `signature`) are extracted into a private `internal::*` submodule of `surge-git` so both legacy spec methods and new run methods can share them without duplication. Public API is unaffected — internal restructure only.

## 8. Pre-requisite changes (surge-core)

These are small additions to `surge-core` that M2 needs but couldn't fit into M1. Change `surge-core` is part of M2's scope.

### 8.1 `RunId::short()` (and all sibling IDs)

In `crates/surge-core/src/id.rs`, the `define_id!` macro is extended:

```rust
macro_rules! define_id {
    ($name:ident, $prefix:expr) => {
        // ... existing items ...

        impl $name {
            /// Short form for human-facing UI: first 12 chars of the ULID, no prefix.
            /// Used in branch names, worktree paths, log lines.
            ///
            /// 12 chars = 10 timestamp chars + 2 randomness chars = 10 bits of randomness
            /// (~1024 distinct suffixes per millisecond). Collisions on `short()` are
            /// possible only when two IDs are generated in the same ms AND happen to draw
            /// the same 2-char randomness prefix — practically negligible for run IDs but
            /// still possible. Callers that absolutely require uniqueness use the full ID.
            #[must_use]
            pub fn short(&self) -> String {
                let s = self.0.to_string();
                debug_assert!(s.len() >= 12);
                s[..12].to_string()
            }
        }
    };
}
```

**Why 12 chars, not 10:** ULID Crockford base32 layout is 10 chars timestamp + 16 chars randomness. Taking only 10 chars yields *zero* randomness; two IDs created in the same millisecond collide on `short()`. 12 chars adds 2 randomness chars (10 bits, ~1024 variants), making intra-ms collision negligible while keeping the form typeable (`01HF2K9X3M5N` instead of `01HF2K9X3M`).

Test:
- `RunId::new().short().len() == 12` for all sibling ID types (SpecId, TaskId, SubtaskId, RunId, SessionId)
- Stress test: generate 1000 IDs in a tight loop, assert `short()` collisions <1% (probabilistic but tightly bounded by ms granularity)

### 8.2 `RunStatus::Crashed` variant + stale-pid detection

In `crates/surge-core/src/run_state.rs` (or wherever `RunStatus` lives — confirm in implementation), add:

```rust
pub enum RunStatus {
    Bootstrapping,
    Running,
    Completed,
    Failed,                               // intentional via RunFailed event
    Aborted,                              // intentional via RunAborted event
    Crashed,                              // NEW: stale daemon pid detected
}
```

`Storage::list_runs` and `get_run` perform stale-pid detection: for any run with `status='running'` and `daemon_pid IS NOT NULL`, check `process_exists(pid)` (via `sysinfo` crate or platform-specific `kill(pid, 0)` on Unix / `OpenProcess` on Windows). If the process is gone, return the run as `Crashed` and `UPDATE runs SET status='crashed', ended_at=now WHERE id=?` to make the change persistent.

Use `sysinfo = "0.32"` (already common in Rust toolchain) for cross-platform process existence check. Add to workspace deps.

## 9. Workspace dependency additions

In root `Cargo.toml [workspace.dependencies]`:

```toml
r2d2 = "0.8"
r2d2_sqlite = "0.25"                     # confirm minor version compatibility with rusqlite 0.32 in plan phase
fd-lock = "4"                             # replaces unmaintained fs2 (RUSTSEC-2025-XXXX)
async-stream = "0.3"
tokio-stream = "0.1"                      # for Stream extension types
tempfile = "3"                            # promote from surge-git dev-deps
sysinfo = { version = "0.32", default-features = false, features = ["system"] }  # for pid liveness check
```

In `crates/surge-persistence/Cargo.toml [dependencies]`:

```toml
rusqlite = { workspace = true, features = ["bundled", "chrono", "serde_json"] }
r2d2 = { workspace = true }
r2d2_sqlite = { workspace = true }
fd-lock = { workspace = true }
async-stream = { workspace = true }
tokio-stream = { workspace = true }
chrono = { workspace = true }
sha2 = { workspace = true }
hex = { workspace = true }
tempfile = { workspace = true }
sysinfo = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }
ulid = { workspace = true }
```

Bincode is **not** added — payloads use `serde_json::to_vec` per §2.3.

In `crates/surge-git/Cargo.toml [dependencies]`:

```toml
surge-core = { workspace = true }
git2 = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
ulid = { workspace = true }                # for RunId display
dirs = { workspace = true }                # for ~/.surge resolution in WorktreeLocation::Central
```

In `crates/surge-core/Cargo.toml [dependencies]`:

```toml
sysinfo = { workspace = true }             # for process_exists used by RunStatus detection helper
```

## 10. Testing strategy

Tests live in `crates/surge-persistence/tests/runs/` (integration), with unit tests in each new module.

### 10.1 Unit tests

Per-module, in `#[cfg(test)] mod tests`:
- `pragmas`: WAL mode applied; checkpoint interval honored
- `migrations`: idempotent re-application; concurrent open serializes correctly
- `seq`: `EventSeq` ordering, conversion to/from i64
- `views::maintain`: each `EventPayload` variant produces expected SQL effects (using `:memory:` DB)
- `artifacts`: atomic write rolls back on simulated FS error mid-write

### 10.2 Integration tests

In `crates/surge-persistence/tests/runs/`:

| Test | Intent |
|---|---|
| `append_read_1000_events` | append 1000 mixed-variant events, read back in ranges, verify ordering, atomicity |
| `materialized_views_consistent` | append 100 events, query each view, assert state matches expected aggregates |
| `rebuild_views_idempotent` | after appending events, snapshot view tables, call `rebuild_views()`, assert identical result |
| `single_writer_in_process` | open writer → second open fails with `WriterAlreadyHeld` → drop first → second succeeds |
| `single_writer_cross_process` | child process holds lock → parent open fails → child exits → parent succeeds |
| `crash_recovery` | spawn child writer task, kill mid-write via `Child::kill()`, reopen, verify integrity (no torn writes, no duplicate seq) |
| `concurrent_5_runs_1000_events` | 5 parallel `RunWriter`s on different runs, 1000 events each, verify no deadlock, no corruption, all converge |
| `subscribe_stream_polling` | writer appends N events, reader subscribed, all received in order, latency under 200 ms |
| `subscribe_stream_drop_no_leak` | drop the stream mid-iteration; verify no leaked tasks/handles |
| `artifact_atomic_write` | simulate crash between tmp write and rename (via fault-injection); reopen produces no partial files |
| `worktree_orphan_detection` | create worktree, manually rm directory, `find_orphaned_worktrees()` finds it, `prune_orphaned_worktrees()` cleans |
| `stale_pid_detection` | insert run with status='running' and daemon_pid of dead process, `list_runs()` returns it as Crashed and updates row |

### 10.3 Property tests (`proptest`)

| Test | Property |
|---|---|
| `append_then_read_roundtrip` | for any sequence of 1–100 random `VersionedEventPayload`s, append + read returns identical sequence |
| `view_maintenance_matches_rebuild` | for any random event sequence, incremental view state == `rebuild_views()` state |

### 10.4 Snapshot tests (`insta`)

- Handcrafted event sequence (linear flow, ~30 events) → snapshot of all view tables. Use `now_ms()` injected from a `Clock` trait that returns deterministic timestamps in tests (e.g., `1700000000000` baseline + event index).

### 10.5 No criterion benchmarks in M2

Storage perf budgets are not yet binding. Add criterion only if/when M5 engine demonstrates a performance bottleneck against storage.

## 11. Acceptance criteria

The milestone is complete when **all** of the following pass:

1. `cargo build -p surge-persistence` clean on Linux, macOS, Windows
2. `cargo test -p surge-persistence` passes — all unit, integration, property, snapshot tests
3. `cargo clippy -p surge-persistence --all-targets -- -D warnings` clean (new code added to strict-clippy CI list alongside `surge-core`)
4. `cargo build --workspace` succeeds — `surge-orchestrator`, `surge-cli`, `surge-ui`, `surge-acp`, `surge-spec` compile unchanged (pure addition guarantee)
5. Append + read of 1000 events to a run is correct, ordered, atomic
6. Materialized views consistent with event log under concurrent reads (test `materialized_views_consistent` + `concurrent_5_runs_1000_events`)
7. WAL mode allows readers to query while writer commits without blocking (verified via `subscribe_stream_polling` while parallel `append_read_1000_events`)
8. Crash recovery: SIGKILL/TerminateProcess of writer mid-flight produces consistent state on reopen (test `crash_recovery`)
9. 5 concurrent runs each writing 1000 events finish without deadlock or corruption (test `concurrent_5_runs_1000_events`)
10. Single-writer enforcement: in-process and cross-process tests pass (`single_writer_in_process`, `single_writer_cross_process`)
11. Atomic artifact write: tmp+rename pattern, no partial files visible at any time (test `artifact_atomic_write`)
12. `RunId::short()` documented, tested, used by `surge-git::create_run_worktree` and worktree paths
13. `RunStatus::Crashed` variant added; stale-pid detection works (test `stale_pid_detection`)
14. `surge-git::find_orphaned_worktrees` + `prune_orphaned_worktrees` work on test repo with intentionally broken worktree (test `worktree_orphan_detection`)
15. Subscribe stream: polling produces events with median latency under 150 ms; cancellation releases all resources (`subscribe_stream_polling`, `subscribe_stream_drop_no_leak`)
16. All public API documented with `///`, `cargo doc -p surge-persistence --no-deps` produces no warnings
17. CI strict-clippy entry added for the new `runs::*` module path inside `surge-persistence` (M2 contract). Legacy modules in `surge-persistence` (`aggregator`, `budget`, `memory`, `models`, `pricing`, `store`) remain on the workspace's permissive clippy set — they are pre-existing code outside M2 scope. Achieved either via `cargo clippy -p surge-persistence --lib -- -D warnings` plus `#![allow(clippy::...)]` markers on legacy modules, or via a CI script that lints the new module path with stricter flags. Final mechanism decided in writing-plans phase.

## 12. Risks & known unknowns

### 12.1 `r2d2_sqlite` 0.25 ↔ `rusqlite` 0.32 compatibility

`r2d2_sqlite` versions track `rusqlite` versions; minor mismatches can cause type confusion. Verify on first `cargo update` in the implementation phase; if incompatible, pin the `r2d2_sqlite` version that matches our `rusqlite` minor or upgrade both together.

### 12.2 `fd-lock` semantics on Windows and choice over `fs2`

`fs2` is unmaintained (last release 2018) and has flagged advisories in recent rustsec audits — exact ID to be confirmed during implementation when running `cargo audit`. `fd-lock` is actively maintained (Yoshua Wuyts, ferrous-systems) and has a cleaner RAII API (`RwLock<File>` + `try_write`).

`fd-lock` uses `LockFileEx` on Windows (advisory, not mandatory). Behaves correctly for cooperating processes (the surge daemon family). Does not block non-cooperating tools (e.g., Windows Explorer reading the file) — that's expected and acceptable; our enforcement target is the daemon process family, not arbitrary external tools.

### 12.3 Long-running readers blocking checkpoint

If a UI subscription holds a connection for hours, WAL can grow unbounded between checkpoints. Mitigation: writer task's periodic explicit `wal_checkpoint(TRUNCATE)` (every 5 min by default) forces a checkpoint regardless of reader activity. SQLite's `PASSIVE` checkpoint can be partial if readers are active; `TRUNCATE` blocks readers briefly (~ms) but forces full reclamation.

### 12.4 Bincode 1.x vs 2.x

Open question deferred from M1 §12. M2 doesn't use bincode at all — payloads are `serde_json::to_vec`. Decision postponed to whichever milestone first needs binary serialization for a hot path.

### 12.5 `sysinfo` cold-start cost

`sysinfo::System::new_all()` enumerates all processes; can be 10–50 ms on busy systems. For `Storage::list_runs`, only call it once and reuse the snapshot. Document this in the implementation plan.

## 13. Realistic effort estimate

Per the M1 calibration (M1 ran ~50% over its 2-week budget due to discovered platform issues), and given M2 has comparable surface area (12 new modules, 5 view tables, 2-layer locking, 12 integration tests, cross-platform crash recovery, worktree extensions, pre-requisite surge-core changes), realistic estimate is **3–4 weeks of solo evening/weekend work**.

Likely surprise sinks:
- Cross-platform single-writer file-lock semantics (Windows vs Linux quirks)
- `r2d2_sqlite` connection pool init-on-each-conn pragma application correctness
- Crash recovery test reliability (timing of `Child::kill()` vs SQLite WAL fsync)
- `tempfile::persist` on Windows when target exists and is open elsewhere

Build buffer in. Do not commit to a 2-week shipping date.

## 14. Open questions for implementation phase

These are for the writing-plans phase, not blockers for design approval:

- **Exact `r2d2_sqlite` minor version** to pin against `rusqlite 0.32`
- **`sysinfo` features** — minimum set to keep dep tree small while supporting cross-platform pid checks
- **Whether `ContentHash` from M1 needs `From<&[u8]>` constructor** (currently may only have `from_bytes`); verify at impl time
- **Concrete `Clock` trait shape** for deterministic test timestamps — likely a `pub trait Clock { fn now_ms(&self) -> i64; }` with `SystemClock` (production) and `MockClock` (tests) impls. This is **not** the kind of trait abstraction §2.4 argues against — that section is about traits over `EventLog` / `Registry` storage backends. A `Clock` is a small infrastructure utility for testability, used by exactly one production impl plus mock; introducing it now is cheap and avoids `chrono::Utc::now()` calls scattered through the code that would later need refactoring for snapshot tests. Placement: `surge-persistence::runs::clock` for now (M2-local); promote to `surge-core` if a second crate needs it.
- **CI matrix** — current CI (per M1) splits `surge-core` into strict-clippy. Plan to extend strict list to `surge-persistence` and ensure macOS/Windows runners cover the new tests.
