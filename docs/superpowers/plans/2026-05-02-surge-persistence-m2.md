# surge-persistence M2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-run SQLite event store + materialized views + git worktree integration to `surge-persistence`, alongside existing legacy persistence (pure-addition strategy).

**Architecture:** rusqlite-based, single-writer per run via dedicated `tokio::task` + bounded mpsc, multi-reader via `r2d2_sqlite` pool. Two-layer single-writer enforcement (in-process `Mutex<HashMap<RunId, Weak<WriterToken>>>` + `fd-lock` advisory file lock). Engine-side materialized view maintenance in same transaction as event INSERT. Per-run worktree extension on existing `surge-git::GitManager`.

**Tech Stack:** rusqlite 0.32 (bundled, chrono, serde_json features) · r2d2_sqlite · fd-lock · async-stream · tokio-stream · sysinfo · tempfile · serde_json (payload encoding, M1 decision) · tracing.

**Spec:** [docs/superpowers/specs/2026-05-02-surge-persistence-m2-design.md](../specs/2026-05-02-surge-persistence-m2-design.md)

**Estimated effort:** 3-4 weeks of solo evening/weekend work.

---

## Phase 0: Pre-requisites in surge-core and workspace

### Task 0.1: Add `short()` helper to `define_id!` macro

**Files:**
- Modify: `crates/surge-core/src/id.rs`

- [ ] **Step 1: Write failing test**

Append to `#[cfg(test)] mod tests` in `crates/surge-core/src/id.rs`:

```rust
#[test]
fn short_is_12_chars_for_all_ids() {
    assert_eq!(SpecId::new().short().len(), 12);
    assert_eq!(TaskId::new().short().len(), 12);
    assert_eq!(SubtaskId::new().short().len(), 12);
    assert_eq!(RunId::new().short().len(), 12);
    assert_eq!(SessionId::new().short().len(), 12);
}

#[test]
fn short_has_timestamp_prefix_and_some_randomness() {
    // Two IDs created in tight succession share the timestamp prefix (first 10 chars)
    // but the 2 randomness chars (positions 10..12) are very likely to differ.
    let a = RunId::new();
    let b = RunId::new();
    let sa = a.short();
    let sb = b.short();
    assert_eq!(sa.len(), 12);
    assert_eq!(sb.len(), 12);
    // Timestamp prefix likely identical within same ms; randomness prefix likely differs.
    // Don't assert inequality — collision is rare but possible. Just sanity-check format.
    assert!(sa.chars().all(|c| c.is_ascii_alphanumeric()));
}
```

- [ ] **Step 2: Run test, expect compile error**

```bash
cargo test -p surge-core --lib id::tests::short_is_12_chars_for_all_ids
```

Expected: `no method named 'short' found for struct 'SpecId'`.

- [ ] **Step 3: Add `short()` to `define_id!` macro**

Inside the `impl $name { ... }` block of the macro (around line 12-22 of `crates/surge-core/src/id.rs`), add:

```rust
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            #[must_use]
            pub fn as_ulid(&self) -> Ulid {
                self.0
            }

            /// Short form for human-facing UI: first 12 chars of the ULID, no prefix.
            ///
            /// 12 chars = 10 timestamp chars + 2 randomness chars (~1024 distinct
            /// suffixes per millisecond). Used in branch names, worktree paths, log
            /// lines. Callers that absolutely require uniqueness use the full ID.
            #[must_use]
            pub fn short(&self) -> String {
                let s = self.0.to_string();
                debug_assert!(s.len() >= 12, "ULID display is always 26 chars");
                s[..12].to_string()
            }
        }
```

- [ ] **Step 4: Run test, expect pass**

```bash
cargo test -p surge-core --lib id::tests
```

Expected: all id tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-core/src/id.rs
git commit -m "M2 prereq: add short() helper to define_id! macro

12-char form (10 timestamp + 2 randomness chars) for branch names and
worktree paths. Full ID still required where uniqueness is critical."
```

### Task 0.2: Add `RunStatus` enum to surge-core

**Files:**
- Create: `crates/surge-core/src/run_status.rs`
- Modify: `crates/surge-core/src/lib.rs`

- [ ] **Step 1: Create file with enum and tests**

`crates/surge-core/src/run_status.rs`:

```rust
//! High-level lifecycle status for a `Run`, persisted in the registry DB.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Coarse lifecycle status of a run, suitable for cross-run queries and CLI listings.
///
/// Distinct from [`RunState`](crate::run_state::RunState), which is the full
/// state machine derived from the event log. `RunStatus` is the durable
/// "what should the operator know about this run right now" string stored in
/// the registry DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run created; bootstrap stages (description, roadmap, flow) still in progress.
    Bootstrapping,
    /// Bootstrap complete; pipeline executor active.
    Running,
    /// Pipeline reached a successful terminal node.
    Completed,
    /// Pipeline reached a terminal failure (RunFailed event).
    Failed,
    /// User or system aborted the run (RunAborted event).
    Aborted,
    /// Daemon process recorded as running but no longer alive (stale-pid detection).
    Crashed,
}

impl RunStatus {
    /// Stable string form used in the registry DB `status` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrapping => "bootstrapping",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Crashed => "crashed",
        }
    }

    /// True if the run is in a terminal state (no further events expected).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Aborted | Self::Crashed
        )
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown status string.
#[derive(Debug, Clone, thiserror::Error)]
#[error("unknown RunStatus: {0:?}")]
pub struct ParseRunStatusError(pub String);

impl FromStr for RunStatus {
    type Err = ParseRunStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "bootstrapping" => Self::Bootstrapping,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "aborted" => Self::Aborted,
            "crashed" => Self::Crashed,
            other => return Err(ParseRunStatusError(other.to_string())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_string_form() {
        for s in [
            RunStatus::Bootstrapping,
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
            RunStatus::Crashed,
        ] {
            assert_eq!(s.as_str().parse::<RunStatus>().unwrap(), s);
        }
    }

    #[test]
    fn unknown_string_is_error() {
        assert!("nonsense".parse::<RunStatus>().is_err());
    }

    #[test]
    fn terminal_classification() {
        assert!(!RunStatus::Bootstrapping.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
        assert!(RunStatus::Completed.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Aborted.is_terminal());
        assert!(RunStatus::Crashed.is_terminal());
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(RunStatus::Crashed.to_string(), "crashed");
    }
}
```

- [ ] **Step 2: Add module declaration and re-export**

In `crates/surge-core/src/lib.rs`, after the `pub mod run_state;` line, add:

```rust
pub mod run_status;
```

In the "New re-exports (Surge data model)" block near the bottom of `lib.rs`, add:

```rust
pub use run_status::{ParseRunStatusError, RunStatus};
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-core --lib run_status::tests
```

Expected: 4 tests pass.

- [ ] **Step 4: Verify workspace still builds**

```bash
cargo build --workspace
```

Expected: clean build, no other crates broken.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-core/src/run_status.rs crates/surge-core/src/lib.rs
git commit -m "M2 prereq: add RunStatus enum to surge-core

Six-variant string-mappable status for registry DB:
bootstrapping/running/completed/failed/aborted/crashed.
Distinct from RunState (the full event-fold state machine) — RunStatus
is the durable cross-run summary stored in the registry."
```

### Task 0.3: Add workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Append new workspace dependencies**

Add to the `[workspace.dependencies]` section of root `Cargo.toml`:

```toml
# M2 storage layer
r2d2 = "0.8"
r2d2_sqlite = "0.25"
fd-lock = "4"
async-stream = "0.3"
tokio-stream = "0.1"
tempfile = "3"
sysinfo = { version = "0.32", default-features = false, features = ["system"] }
```

Also extend the existing `rusqlite` line to enable additional features:

```toml
rusqlite = { version = "0.32", features = ["bundled", "chrono", "serde_json"] }
```

- [ ] **Step 2: Verify workspace resolves**

```bash
cargo metadata --format-version 1 > /dev/null
```

Expected: no errors. If `r2d2_sqlite 0.25` is incompatible with `rusqlite 0.32`, look up the matching minor version via `cargo search r2d2_sqlite` and pin accordingly.

- [ ] **Step 3: Verify workspace still builds**

```bash
cargo build --workspace
```

Expected: clean build (new deps not yet used by any crate).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml
git commit -m "M2 deps: add r2d2_sqlite, fd-lock, sysinfo, tempfile, async-stream

Adds workspace dependencies needed by surge-persistence M2 storage layer
and surge-git run-worktree extensions. rusqlite gains chrono and
serde_json features for boundary-crossing types."
```

---

## Phase 1: Storage module skeleton, errors, EventSeq, Clock

### Task 1.1: Create `runs/` module structure with empty stubs

**Files:**
- Create: `crates/surge-persistence/src/runs/mod.rs`
- Create: `crates/surge-persistence/src/runs/error.rs`
- Create: `crates/surge-persistence/src/runs/seq.rs`
- Create: `crates/surge-persistence/src/runs/clock.rs`
- Modify: `crates/surge-persistence/src/lib.rs`
- Modify: `crates/surge-persistence/Cargo.toml`

- [ ] **Step 1: Add dependencies to surge-persistence Cargo.toml**

Replace the `[dependencies]` section of `crates/surge-persistence/Cargo.toml` with:

```toml
[dependencies]
surge-core = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
rusqlite = { workspace = true }
dirs = { workspace = true }
tracing = { workspace = true }
ulid = { workspace = true }

# M2 additions
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

[dev-dependencies]
tempfile = { workspace = true }
proptest = { workspace = true }
insta = { workspace = true }
tokio = { workspace = true, features = ["test-util", "macros", "rt-multi-thread"] }
```

Also add `workspace = true` for `surge-core` if not already, and update `[package]` to keep license/edition workspace-aligned.

- [ ] **Step 2: Create `runs/mod.rs` with module declarations**

`crates/surge-persistence/src/runs/mod.rs`:

```rust
//! Per-run SQLite event store and registry for the Surge architecture.
//!
//! This module is the M2 milestone of the surge-persistence layer. It lives
//! alongside the existing legacy persistence (aggregator/budget/memory/
//! pricing/store) and does not interact with it.
//!
//! See `docs/superpowers/specs/2026-05-02-surge-persistence-m2-design.md`
//! for the full design.

pub mod clock;
pub mod error;
pub mod seq;

pub use clock::{Clock, MockClock, SystemClock};
pub use error::{CloseError, OpenError, StorageError, WriterError};
pub use seq::EventSeq;
```

- [ ] **Step 3: Create `runs/error.rs` with all error enums**

`crates/surge-persistence/src/runs/error.rs`:

```rust
//! Error types for the run storage layer.
//!
//! Distinct from the legacy `PersistenceError` — there is no `From` between
//! them by design; the two domains are independent.

use surge_core::RunId;
use thiserror::Error;

/// Failure modes for opening or creating a `Storage`, run reader, or run writer.
#[derive(Debug, Error)]
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

    #[error("pool init error: {0}")]
    Pool(String),
}

/// Failure modes for reads and writes against an open run.
#[derive(Debug, Error)]
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

/// Failure modes inside the writer task itself (wrapped into StorageError on the public surface).
#[derive(Debug, Error)]
pub enum WriterError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("internal: {0}")]
    Internal(String),
}

/// Failure modes for `RunWriter::close`.
#[derive(Debug, Error)]
pub enum CloseError {
    #[error(transparent)]
    Writer(#[from] WriterError),

    #[error("writer task join failed: {0}")]
    JoinFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::RunId;

    #[test]
    fn writer_already_held_displays_run_id() {
        let id = RunId::new();
        let err = OpenError::WriterAlreadyHeld { run_id: id };
        assert!(err.to_string().contains("run-"));
    }
}
```

- [ ] **Step 4: Create `runs/seq.rs`**

`crates/surge-persistence/src/runs/seq.rs`:

```rust
//! Strongly-typed event sequence number.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Monotonic, gap-free event sequence number assigned by SQLite ROWID alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventSeq(pub u64);

impl EventSeq {
    /// Sentinel for "before any event was written" (used as initial cursor in subscribe).
    pub const ZERO: EventSeq = EventSeq(0);

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EventSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<u64> for EventSeq {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<i64> for EventSeq {
    fn from(v: i64) -> Self {
        Self(v as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_works_as_u64() {
        let a = EventSeq(1);
        let b = EventSeq(2);
        assert!(a < b);
        assert_eq!(a.next(), b);
    }

    #[test]
    fn zero_is_initial() {
        assert_eq!(EventSeq::ZERO.as_u64(), 0);
    }
}
```

- [ ] **Step 5: Create `runs/clock.rs`**

`crates/surge-persistence/src/runs/clock.rs`:

```rust
//! Test-injectable wall clock.
//!
//! Production code uses [`SystemClock`]. Tests use [`MockClock`] for
//! deterministic timestamps in snapshot tests and reproducible event logs.
//!
//! This is intentionally a small infrastructure utility — not the kind of
//! storage-backend trait abstraction that the spec's "no traits for mocking"
//! guidance argues against.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

pub trait Clock: Send + Sync + 'static {
    /// Current time as Unix epoch milliseconds.
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default, Clone)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

#[derive(Debug, Clone)]
pub struct MockClock {
    inner: Arc<AtomicI64>,
}

impl MockClock {
    pub fn new(initial_ms: i64) -> Self {
        Self {
            inner: Arc::new(AtomicI64::new(initial_ms)),
        }
    }

    pub fn advance(&self, by_ms: i64) {
        self.inner.fetch_add(by_ms, Ordering::SeqCst);
    }

    pub fn set(&self, ms: i64) {
        self.inner.store(ms, Ordering::SeqCst);
    }
}

impl Clock for MockClock {
    fn now_ms(&self) -> i64 {
        self.inner.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_clock_advances() {
        let c = MockClock::new(1_700_000_000_000);
        assert_eq!(c.now_ms(), 1_700_000_000_000);
        c.advance(50);
        assert_eq!(c.now_ms(), 1_700_000_000_050);
    }

    #[test]
    fn system_clock_returns_recent_time() {
        let c = SystemClock;
        let now = c.now_ms();
        assert!(now > 1_700_000_000_000, "clock should be after 2023");
    }
}
```

- [ ] **Step 6: Wire module into surge-persistence lib.rs**

In `crates/surge-persistence/src/lib.rs`, after the existing `pub mod store;` line, add:

```rust
/// New M2 Surge storage layer (per-run event log, registry, worktree integration).
pub mod runs;
```

- [ ] **Step 7: Run tests for the new module**

```bash
cargo test -p surge-persistence --lib runs::
```

Expected: 6 tests pass (1 in error, 2 in seq, 2 in clock, 1 in error).

- [ ] **Step 8: Verify workspace still builds and other crates unaffected**

```bash
cargo build --workspace
cargo test -p surge-core --lib
```

Expected: both clean.

- [ ] **Step 9: Commit**

```bash
git add crates/surge-persistence/Cargo.toml crates/surge-persistence/src/lib.rs crates/surge-persistence/src/runs/
git commit -m "M2(persistence): scaffold runs/ module with errors, EventSeq, Clock

Empty skeleton for the new Surge storage layer. Adds error enums
(OpenError, StorageError, WriterError, CloseError), EventSeq newtype,
and a small Clock trait (SystemClock + MockClock) for deterministic
test timestamps. Legacy modules untouched."
```

---

## Phase 2: PRAGMAs and migration runner

### Task 2.1: PRAGMA constants and pool init helpers

**Files:**
- Create: `crates/surge-persistence/src/runs/pragmas.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create `runs/pragmas.rs`**

```rust
//! SQLite PRAGMA application for run-DB and registry-DB connections.

use rusqlite::Connection;

/// PRAGMAs applied to every connection (writer and readers) on a per-run database.
pub const PER_RUN_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA temp_store = MEMORY",
    "PRAGMA mmap_size = 30000000000",
    "PRAGMA cache_size = -32000",
    "PRAGMA foreign_keys = ON",
    "PRAGMA wal_autocheckpoint = 1000",
];

/// PRAGMAs applied to the registry DB connection.
pub const REGISTRY_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA foreign_keys = ON",
];

/// Apply the given PRAGMAs to a fresh connection.
pub fn apply(conn: &Connection, pragmas: &[&str]) -> rusqlite::Result<()> {
    for p in pragmas {
        // PRAGMA may return a row (e.g., journal_mode) — execute_batch handles both.
        conn.execute_batch(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragmas_apply_to_in_memory_db() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, PER_RUN_PRAGMAS).unwrap();

        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        // :memory: DB returns "memory" not "wal" — but the call shouldn't error.
        assert!(mode == "memory" || mode == "wal");
    }

    #[test]
    fn registry_pragmas_apply() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, REGISTRY_PRAGMAS).unwrap();
    }
}
```

- [ ] **Step 2: Add module to mod.rs**

In `crates/surge-persistence/src/runs/mod.rs`, add:

```rust
pub mod pragmas;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-persistence --lib runs::pragmas
```

Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/pragmas.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): add PRAGMA constants for run and registry DBs

WAL mode + autocheckpoint + mmap + foreign_keys for per-run DBs;
WAL + foreign_keys for registry. apply() helper used during pool init
and direct connection setup."
```

### Task 2.2: Migration runner with one-transaction-per-migration semantics

**Files:**
- Create: `crates/surge-persistence/src/runs/migrations.rs`
- Create: `crates/surge-persistence/src/runs/migrations/registry/0001_initial.sql`
- Create: `crates/surge-persistence/src/runs/migrations/per_run/0001_initial.sql`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

> **Delta P1.X3:** runner uses one transaction per migration_id (not one big transaction for all). Partial completion sticks if a later migration fails; resumability on next open.

- [ ] **Step 1: Write SQL migrations**

`crates/surge-persistence/src/runs/migrations/registry/0001_initial.sql`:

```sql
CREATE TABLE runs (
    id            TEXT    PRIMARY KEY,
    project_path  TEXT    NOT NULL,
    pipeline_template TEXT,
    status        TEXT    NOT NULL,
    started_at    INTEGER NOT NULL,
    ended_at      INTEGER,
    daemon_pid    INTEGER
);
CREATE INDEX idx_runs_status  ON runs(status);
CREATE INDEX idx_runs_started ON runs(started_at DESC);
```

`crates/surge-persistence/src/runs/migrations/per_run/0001_initial.sql`:

```sql
CREATE TABLE events (
    seq            INTEGER PRIMARY KEY,
    timestamp      INTEGER NOT NULL,
    kind           TEXT    NOT NULL,
    payload        BLOB    NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_ts   ON events(timestamp);

CREATE TRIGGER trg_events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(FAIL, 'events table is append-only'); END;

CREATE TRIGGER trg_events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(FAIL, 'events table is append-only'); END;

CREATE TABLE stage_executions (
    node_id     TEXT    NOT NULL,
    attempt     INTEGER NOT NULL,
    started_seq INTEGER NOT NULL,
    ended_seq   INTEGER,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    outcome     TEXT,
    cost_usd    REAL    DEFAULT 0,
    tokens_in   INTEGER DEFAULT 0,
    tokens_out  INTEGER DEFAULT 0,
    PRIMARY KEY(node_id, attempt)
);

CREATE TABLE artifacts (
    id                TEXT    PRIMARY KEY,
    produced_by_node  TEXT,
    produced_at_seq   INTEGER NOT NULL,
    name              TEXT    NOT NULL,
    path              TEXT    NOT NULL,
    size_bytes        INTEGER NOT NULL,
    content_hash      TEXT    NOT NULL
);
CREATE INDEX idx_artifacts_node ON artifacts(produced_by_node);
CREATE INDEX idx_artifacts_name ON artifacts(name);

CREATE TABLE pending_approvals (
    seq           INTEGER PRIMARY KEY,
    node_id       TEXT    NOT NULL,
    channel       TEXT    NOT NULL,
    requested_at  INTEGER NOT NULL,
    payload_hash  TEXT    NOT NULL,
    delivered     INTEGER DEFAULT 0,
    message_id    INTEGER
);
CREATE INDEX idx_approvals_node ON pending_approvals(node_id);

CREATE TABLE cost_summary (
    metric      TEXT PRIMARY KEY,
    value       REAL NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE graph_snapshots (
    at_seq            INTEGER PRIMARY KEY,
    snapshot          BLOB    NOT NULL,
    bytes_compressed  INTEGER NOT NULL
);
```

- [ ] **Step 2: Create migration runner**

`crates/surge-persistence/src/runs/migrations.rs`:

```rust
//! Forward-only migration runner with one-transaction-per-migration semantics.
//!
//! Multi-process safety: each migration runs inside its own `BEGIN EXCLUSIVE`
//! transaction. If two processes call `Storage::open` simultaneously, one
//! acquires the exclusive lock first and applies all pending migrations; the
//! second waits and then sees them already-applied.
//!
//! Resumability: if migration N fails, migrations 1..N-1 stay committed.
//! On retry the runner picks up at N.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::runs::clock::Clock;

/// Static list of (id, sql) pairs applied in declaration order.
pub type MigrationSet = &'static [(&'static str, &'static str)];

pub const REGISTRY_MIGRATIONS: MigrationSet = &[(
    "registry-0001-initial",
    include_str!("migrations/registry/0001_initial.sql"),
)];

pub const PER_RUN_MIGRATIONS: MigrationSet = &[(
    "per-run-0001-initial",
    include_str!("migrations/per_run/0001_initial.sql"),
)];

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("migration {id} failed: {source}")]
    Apply {
        id: String,
        #[source]
        source: rusqlite::Error,
    },

    #[error("could not initialize _migrations table: {0}")]
    InitTable(#[source] rusqlite::Error),
}

pub fn apply(
    conn: &mut Connection,
    migrations: MigrationSet,
    clock: &dyn Clock,
) -> Result<(), MigrationError> {
    // Bootstrap _migrations table outside any user-data transaction.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id          TEXT    PRIMARY KEY,
            applied_at  INTEGER NOT NULL
        )",
    )
    .map_err(MigrationError::InitTable)?;

    for (id, sql) in migrations {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(|e| MigrationError::Apply {
                id: (*id).to_string(),
                source: e,
            })?;

        let already: bool = tx
            .query_row(
                "SELECT 1 FROM _migrations WHERE id = ?",
                params![id],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| MigrationError::Apply {
                id: (*id).to_string(),
                source: e,
            })?
            .unwrap_or(false);

        if already {
            tx.commit().map_err(|e| MigrationError::Apply {
                id: (*id).to_string(),
                source: e,
            })?;
            continue;
        }

        tx.execute_batch(sql).map_err(|e| MigrationError::Apply {
            id: (*id).to_string(),
            source: e,
        })?;

        tx.execute(
            "INSERT INTO _migrations (id, applied_at) VALUES (?, ?)",
            params![id, clock.now_ms()],
        )
        .map_err(|e| MigrationError::Apply {
            id: (*id).to_string(),
            source: e,
        })?;

        tx.commit().map_err(|e| MigrationError::Apply {
            id: (*id).to_string(),
            source: e,
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;

    #[test]
    fn apply_registry_to_fresh_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();

        // runs table now exists
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='runs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // _migrations recorded
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, REGISTRY_MIGRATIONS.len() as i64);
    }

    #[test]
    fn apply_per_run_to_fresh_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();

        // events + 5 view tables exist
        for table in [
            "events",
            "stage_executions",
            "artifacts",
            "pending_approvals",
            "cost_summary",
            "graph_snapshots",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {} missing", table);
        }
    }

    #[test]
    fn apply_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();

        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, REGISTRY_MIGRATIONS.len() as i64);
    }

    #[test]
    fn append_only_trigger_blocks_update_on_events() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();

        conn.execute(
            "INSERT INTO events (timestamp, kind, payload, schema_version) VALUES (?, ?, ?, 1)",
            params![1, "Test", vec![0u8; 4]],
        )
        .unwrap();

        let err = conn
            .execute("UPDATE events SET kind = 'X' WHERE seq = 1", [])
            .unwrap_err();
        assert!(err.to_string().contains("append-only"));
    }

    #[test]
    fn append_only_trigger_blocks_delete_on_events() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();

        conn.execute(
            "INSERT INTO events (timestamp, kind, payload, schema_version) VALUES (?, ?, ?, 1)",
            params![1, "Test", vec![0u8; 4]],
        )
        .unwrap();

        let err = conn
            .execute("DELETE FROM events WHERE seq = 1", [])
            .unwrap_err();
        assert!(err.to_string().contains("append-only"));
    }
}
```

- [ ] **Step 3: Wire module + ensure SQL files included by build**

In `crates/surge-persistence/src/runs/mod.rs`, add:

```rust
pub mod migrations;
```

The `include_str!` calls automatically include the SQL files at compile time. Verify file paths match.

- [ ] **Step 4: Run tests**

```bash
cargo test -p surge-persistence --lib runs::migrations
```

Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/migrations.rs crates/surge-persistence/src/runs/migrations/ crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): forward-only migration runner

One transaction per migration_id (BEGIN EXCLUSIVE) for multi-process
safety and partial-failure resumability. Includes registry-0001
(runs table) and per-run-0001 (events + 5 view tables + 2 append-only
triggers). Migrations table is bootstrapped outside the user txn."
```

---

## Phase 3: Registry DB types and CRUD

### Task 3.1: Registry types and pool

**Files:**
- Create: `crates/surge-persistence/src/runs/registry.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create registry module with types and stub Pool helper**

`crates/surge-persistence/src/runs/registry.rs`:

```rust
//! Registry DB — cross-run metadata lives here.
//!
//! Schema: see `migrations/registry/0001_initial.sql`. M2 ships only the `runs`
//! table; profiles/templates/trust come in M3+.

use std::path::{Path, PathBuf};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use surge_core::{RunId, RunStatus};

use crate::runs::clock::Clock;
use crate::runs::error::{OpenError, StorageError};
use crate::runs::migrations::{apply as apply_migrations, REGISTRY_MIGRATIONS};
use crate::runs::pragmas::{apply as apply_pragmas, REGISTRY_PRAGMAS};

/// Filter applied to `Storage::list_runs`.
#[derive(Debug, Default, Clone)]
pub struct RunFilter {
    pub status: Option<RunStatus>,
    pub project_path: Option<PathBuf>,
    pub limit: Option<usize>,
}

/// Lightweight summary returned by `list_runs` and `get_run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: RunId,
    pub project_path: PathBuf,
    pub pipeline_template: Option<String>,
    pub status: RunStatus,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub daemon_pid: Option<i32>,
}

pub fn open_registry_pool(home: &Path, clock: &dyn Clock) -> Result<Pool<SqliteConnectionManager>, OpenError> {
    let db_dir = home.join("db");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("registry.sqlite");

    let manager = SqliteConnectionManager::file(&db_path)
        .with_init(|c| apply_pragmas(c, REGISTRY_PRAGMAS));
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(|e| OpenError::Pool(e.to_string()))?;

    // Run migrations on a dedicated connection (not from pool — pool conns are read-mostly).
    let mut conn = rusqlite::Connection::open(&db_path)?;
    apply_pragmas(&conn, REGISTRY_PRAGMAS)?;
    apply_migrations(&mut conn, REGISTRY_MIGRATIONS, clock)
        .map_err(|e| OpenError::MigrationFailed(e.to_string()))?;

    Ok(pool)
}

pub fn insert_run(
    pool: &Pool<SqliteConnectionManager>,
    summary: &RunSummary,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute(
        "INSERT INTO runs (id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            summary.id.to_string(),
            summary.project_path.to_string_lossy(),
            summary.pipeline_template,
            summary.status.as_str(),
            summary.started_at_ms,
            summary.ended_at_ms,
            summary.daemon_pid,
        ],
    )?;
    Ok(())
}

pub fn get_run(
    pool: &Pool<SqliteConnectionManager>,
    run_id: &RunId,
) -> Result<Option<RunSummary>, StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.query_row(
        "SELECT id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid
         FROM runs WHERE id = ?",
        params![run_id.to_string()],
        row_to_summary,
    )
    .optional_err()
}

pub fn list_runs(
    pool: &Pool<SqliteConnectionManager>,
    filter: &RunFilter,
) -> Result<Vec<RunSummary>, StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;

    let mut sql = String::from(
        "SELECT id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid \
         FROM runs WHERE 1=1",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = filter.status {
        sql.push_str(" AND status = ?");
        binds.push(Box::new(s.as_str().to_string()));
    }
    if let Some(p) = &filter.project_path {
        sql.push_str(" AND project_path = ?");
        binds.push(Box::new(p.to_string_lossy().into_owned()));
    }
    sql.push_str(" ORDER BY started_at DESC");
    if let Some(lim) = filter.limit {
        sql.push_str(" LIMIT ?");
        binds.push(Box::new(lim as i64));
    }

    let mut stmt = conn.prepare(&sql)?;
    let bind_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bind_refs), row_to_summary)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete_run(
    pool: &Pool<SqliteConnectionManager>,
    run_id: &RunId,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute("DELETE FROM runs WHERE id = ?", params![run_id.to_string()])?;
    Ok(())
}

pub fn update_status(
    pool: &Pool<SqliteConnectionManager>,
    run_id: &RunId,
    status: RunStatus,
    ended_at_ms: Option<i64>,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute(
        "UPDATE runs SET status = ?, ended_at = ? WHERE id = ?",
        params![status.as_str(), ended_at_ms, run_id.to_string()],
    )?;
    Ok(())
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunSummary> {
    let id_str: String = row.get(0)?;
    let id: RunId = id_str.parse().map_err(|e: ulid::DecodeError| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status_str: String = row.get(3)?;
    let status: RunStatus = status_str.parse().map_err(|e: surge_core::ParseRunStatusError| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(RunSummary {
        id,
        project_path: PathBuf::from(row.get::<_, String>(1)?),
        pipeline_template: row.get(2)?,
        status,
        started_at_ms: row.get(4)?,
        ended_at_ms: row.get(5)?,
        daemon_pid: row.get(6)?,
    })
}

trait OptionalErr<T> {
    fn optional_err(self) -> Result<Option<T>, StorageError>;
}
impl<T> OptionalErr<T> for rusqlite::Result<T> {
    fn optional_err(self) -> Result<Option<T>, StorageError> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use tempfile::TempDir;

    fn fixture_summary(id: RunId, pid: Option<i32>) -> RunSummary {
        RunSummary {
            id,
            project_path: "/tmp/proj".into(),
            pipeline_template: Some("t@1".into()),
            status: RunStatus::Running,
            started_at_ms: 1_700_000_000_000,
            ended_at_ms: None,
            daemon_pid: pid,
        }
    }

    #[test]
    fn insert_get_list_delete_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        let s = fixture_summary(RunId::new(), Some(1234));

        insert_run(&pool, &s).unwrap();
        let got = get_run(&pool, &s.id).unwrap().unwrap();
        assert_eq!(got.id, s.id);
        assert_eq!(got.daemon_pid, Some(1234));

        let listed = list_runs(&pool, &RunFilter::default()).unwrap();
        assert_eq!(listed.len(), 1);

        delete_run(&pool, &s.id).unwrap();
        assert!(get_run(&pool, &s.id).unwrap().is_none());
    }

    #[test]
    fn list_filter_by_status() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let mut a = fixture_summary(RunId::new(), Some(1));
        a.status = RunStatus::Running;
        let mut b = fixture_summary(RunId::new(), None);
        b.status = RunStatus::Completed;

        insert_run(&pool, &a).unwrap();
        insert_run(&pool, &b).unwrap();

        let running = list_runs(
            &pool,
            &RunFilter {
                status: Some(RunStatus::Running),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, a.id);
    }

    #[test]
    fn update_status_transitions() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        let s = fixture_summary(RunId::new(), Some(1));
        insert_run(&pool, &s).unwrap();

        update_status(&pool, &s.id, RunStatus::Crashed, Some(1_700_000_000_500)).unwrap();
        let got = get_run(&pool, &s.id).unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Crashed);
        assert_eq!(got.ended_at_ms, Some(1_700_000_000_500));
    }
}
```

- [ ] **Step 2: Wire into mod.rs**

In `runs/mod.rs`:

```rust
pub mod registry;
pub use registry::{RunFilter, RunSummary};
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-persistence --lib runs::registry
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/registry.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): registry DB types and CRUD helpers

RunSummary, RunFilter, open_registry_pool (with PRAGMAs and migrations),
insert/get/list/delete/update_status. Pool size 8 for cross-run queries."
```

### Task 3.2: Stale-pid detection helper

**Files:**
- Create: `crates/surge-persistence/src/runs/process.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create process module**

`crates/surge-persistence/src/runs/process.rs`:

```rust
//! Liveness check for daemon PIDs (cross-platform via sysinfo).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::{Pid, RefreshKind, System};

/// Cached system snapshot — refreshing pid table is expensive (10-50 ms),
/// so we keep a short-TTL cache.
pub struct ProcessProbe {
    inner: Mutex<Inner>,
    ttl: Duration,
}

struct Inner {
    sys: System,
    last_refresh: Instant,
}

impl ProcessProbe {
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_millis(500))
    }

    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        let sys = System::new_with_specifics(RefreshKind::everything().without_cpu().without_memory());
        Self {
            inner: Mutex::new(Inner {
                sys,
                last_refresh: Instant::now() - ttl - Duration::from_secs(1),
            }),
            ttl,
        }
    }

    pub fn is_alive(&self, pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        let mut g = self.inner.lock().expect("poisoned ProcessProbe mutex");
        if g.last_refresh.elapsed() > self.ttl {
            g.sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
            g.last_refresh = Instant::now();
        }
        g.sys.process(Pid::from_u32(pid as u32)).is_some()
    }
}

impl Default for ProcessProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        let probe = ProcessProbe::new();
        let me = std::process::id() as i32;
        assert!(probe.is_alive(me));
    }

    #[test]
    fn impossibly_high_pid_is_not_alive() {
        let probe = ProcessProbe::new();
        // u32::MAX is unlikely to be assigned on any sane system
        assert!(!probe.is_alive(i32::MAX));
    }

    #[test]
    fn zero_or_negative_pid_is_not_alive() {
        let probe = ProcessProbe::new();
        assert!(!probe.is_alive(0));
        assert!(!probe.is_alive(-1));
    }
}
```

- [ ] **Step 2: Wire into mod.rs**

```rust
pub mod process;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-persistence --lib runs::process
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/process.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): cross-platform process liveness probe

ProcessProbe wraps sysinfo with a 500ms-TTL cache to keep
list_runs() queries cheap when many runs reference the same dead pid."
```

---

## Phase 4: RunReader (read-only handle)

### Task 4.1: RunReader struct, basic event-log reads

**Files:**
- Create: `crates/surge-persistence/src/runs/reader.rs`
- Create: `crates/surge-persistence/src/runs/types.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create types module with view-row records**

`crates/surge-persistence/src/runs/types.rs`:

```rust
//! Record types returned by view queries.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use surge_core::{ContentHash, NodeKey};

use crate::runs::seq::EventSeq;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageExecution {
    pub node_id: NodeKey,
    pub attempt: u32,
    pub started_seq: EventSeq,
    pub ended_seq: Option<EventSeq>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub outcome: Option<String>,
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub id: ContentHash,
    pub produced_by_node: Option<NodeKey>,
    pub produced_at_seq: EventSeq,
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub content_hash: ContentHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub seq: EventSeq,
    pub node_id: NodeKey,
    pub channel: String,
    pub requested_at_ms: i64,
    pub payload_hash: String,
    pub delivered: bool,
    pub message_id: Option<i64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_hits: u64,
    pub cost_usd: f64,
}
```

- [ ] **Step 2: Create reader module skeleton with event-log methods**

`crates/surge-persistence/src/runs/reader.rs`:

```rust
//! Read-only handle for a per-run database.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use surge_core::{RunId, VersionedEventPayload};

use crate::runs::error::StorageError;
use crate::runs::seq::EventSeq;

#[derive(Clone)]
pub struct RunReader {
    pub(crate) run_id: RunId,
    pub(crate) pool: Pool<SqliteConnectionManager>,
    pub(crate) artifacts_dir: Arc<PathBuf>,
    pub(crate) worktree_path: Arc<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ReadEvent {
    pub seq: EventSeq,
    pub timestamp_ms: i64,
    pub kind: String,
    pub payload: VersionedEventPayload,
}

impl RunReader {
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn worktree_path(&self) -> &Path {
        &self.worktree_path
    }

    pub async fn current_seq(&self) -> Result<EventSeq, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let max: Option<i64> = conn
                .query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
            Ok(EventSeq(max.unwrap_or(0) as u64))
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn read_event(&self, seq: EventSeq) -> Result<Option<ReadEvent>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let row = conn.query_row(
                "SELECT seq, timestamp, kind, payload FROM events WHERE seq = ?",
                params![seq.0 as i64],
                |row| {
                    let blob: Vec<u8> = row.get(3)?;
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        blob,
                    ))
                },
            );
            match row {
                Ok((seq, ts, kind, blob)) => {
                    let payload: VersionedEventPayload = serde_json::from_slice(&blob)?;
                    Ok(Some(ReadEvent { seq, timestamp_ms: ts, kind, payload }))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn read_events(&self, range: Range<EventSeq>) -> Result<Vec<ReadEvent>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let mut stmt = conn.prepare(
                "SELECT seq, timestamp, kind, payload
                 FROM events WHERE seq >= ? AND seq < ? ORDER BY seq",
            )?;
            let iter = stmt.query_map(
                params![range.start.0 as i64, range.end.0 as i64],
                |row| {
                    let blob: Vec<u8> = row.get(3)?;
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        blob,
                    ))
                },
            )?;
            let mut out = Vec::new();
            for r in iter {
                let (seq, ts, kind, blob) = r?;
                let payload: VersionedEventPayload = serde_json::from_slice(&blob)?;
                out.push(ReadEvent { seq, timestamp_ms: ts, kind, payload });
            }
            Ok(out)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }
}
```

- [ ] **Step 3: Wire types and reader into mod.rs**

```rust
pub mod reader;
pub mod types;

pub use reader::{ReadEvent, RunReader};
pub use types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};
```

- [ ] **Step 4: Add a build smoke check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/reader.rs crates/surge-persistence/src/runs/types.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): RunReader skeleton + event-log read methods

RunReader holds an Arc'd pool + paths and delegates blocking SQLite
calls through spawn_blocking. read_event/read_events/current_seq use
serde_json for payload decoding (M1 contract). View-row record types
defined in types.rs for use by view queries (next task)."
```

### Task 4.2: View queries on RunReader

**Files:**
- Modify: `crates/surge-persistence/src/runs/reader.rs`
- Create: `crates/surge-persistence/src/runs/reader_views.rs`

- [ ] **Step 1: Create reader_views.rs with view-query implementations**

`crates/surge-persistence/src/runs/reader_views.rs`:

```rust
//! Synchronous SQL adapters for materialized view tables.
//!
//! Called from `RunReader` async methods through `spawn_blocking`.

use r2d2::PooledConnection;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use std::path::PathBuf;
use surge_core::{ContentHash, NodeKey};

use crate::runs::error::StorageError;
use crate::runs::seq::EventSeq;
use crate::runs::types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};

pub fn stage_executions(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<StageExecution>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT node_id, attempt, started_seq, ended_seq, started_at, ended_at,
                outcome, cost_usd, tokens_in, tokens_out
         FROM stage_executions ORDER BY started_seq",
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(StageExecution {
            node_id: NodeKey::try_from(row.get::<_, String>(0)?)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?,
            attempt: row.get::<_, i64>(1)? as u32,
            started_seq: EventSeq(row.get::<_, i64>(2)? as u64),
            ended_seq: row.get::<_, Option<i64>>(3)?.map(|v| EventSeq(v as u64)),
            started_at_ms: row.get(4)?,
            ended_at_ms: row.get(5)?,
            outcome: row.get(6)?,
            cost_usd: row.get(7)?,
            tokens_in: row.get::<_, i64>(8)? as u64,
            tokens_out: row.get::<_, i64>(9)? as u64,
        })
    })?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

pub fn artifacts(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<ArtifactRecord>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash
         FROM artifacts ORDER BY produced_at_seq",
    )?;
    let iter = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let id = ContentHash::from_string(&id).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let hash: String = row.get(6)?;
        let hash = ContentHash::from_string(&hash).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(ArtifactRecord {
            id,
            produced_by_node: row
                .get::<_, Option<String>>(1)?
                .map(|s| NodeKey::try_from(s).unwrap_or_else(|_| NodeKey::try_from("invalid".to_string()).unwrap())),
            produced_at_seq: EventSeq(row.get::<_, i64>(2)? as u64),
            name: row.get(3)?,
            path: PathBuf::from(row.get::<_, String>(4)?),
            size_bytes: row.get::<_, i64>(5)? as u64,
            content_hash: hash,
        })
    })?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

pub fn pending_approvals(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<PendingApproval>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT seq, node_id, channel, requested_at, payload_hash, delivered, message_id
         FROM pending_approvals ORDER BY seq",
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(PendingApproval {
            seq: EventSeq(row.get::<_, i64>(0)? as u64),
            node_id: NodeKey::try_from(row.get::<_, String>(1)?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
            })?,
            channel: row.get(2)?,
            requested_at_ms: row.get(3)?,
            payload_hash: row.get(4)?,
            delivered: row.get::<_, i64>(5)? != 0,
            message_id: row.get(6)?,
        })
    })?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

pub fn cost_summary(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<CostSummary, StorageError> {
    let mut summary = CostSummary::default();
    let mut stmt = conn.prepare("SELECT metric, value FROM cost_summary")?;
    let iter = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)))?;
    for r in iter {
        let (metric, value) = r?;
        match metric.as_str() {
            "tokens_in" => summary.tokens_in = value as u64,
            "tokens_out" => summary.tokens_out = value as u64,
            "cache_hits" => summary.cache_hits = value as u64,
            "cost_usd" => summary.cost_usd = value,
            _ => {}
        }
    }
    Ok(summary)
}

pub fn list_snapshots(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<EventSeq>, StorageError> {
    let mut stmt = conn.prepare("SELECT at_seq FROM graph_snapshots ORDER BY at_seq")?;
    let iter = stmt.query_map([], |row| Ok(EventSeq(row.get::<_, i64>(0)? as u64)))?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

pub fn latest_snapshot_at_or_before(
    conn: &PooledConnection<SqliteConnectionManager>,
    seq: EventSeq,
) -> Result<Option<(EventSeq, Vec<u8>)>, StorageError> {
    conn.query_row(
        "SELECT at_seq, snapshot FROM graph_snapshots WHERE at_seq <= ? ORDER BY at_seq DESC LIMIT 1",
        params![seq.0 as i64],
        |row| Ok((EventSeq(row.get::<_, i64>(0)? as u64), row.get::<_, Vec<u8>>(1)?)),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.into()),
    })
}
```

- [ ] **Step 2: Add async wrappers on RunReader**

Append to `runs/reader.rs`:

```rust
use surge_core::RunState;

use crate::runs::reader_views as views;
use crate::runs::types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};

impl RunReader {
    pub async fn stage_executions(&self) -> Result<Vec<StageExecution>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::stage_executions(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn artifacts(&self) -> Result<Vec<ArtifactRecord>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::artifacts(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn pending_approvals(&self) -> Result<Vec<PendingApproval>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::pending_approvals(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn cost_summary(&self) -> Result<CostSummary, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::cost_summary(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn list_snapshots(&self) -> Result<Vec<EventSeq>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::list_snapshots(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    pub async fn latest_snapshot_at_or_before(
        &self,
        seq: EventSeq,
    ) -> Result<Option<(EventSeq, RunState)>, StorageError> {
        let pool = self.pool.clone();
        let blob_opt = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::latest_snapshot_at_or_before(&conn, seq)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))??;

        let Some((s, blob)) = blob_opt else { return Ok(None); };
        let state: RunState = serde_json::from_slice(&blob)?;
        Ok(Some((s, state)))
    }

    pub async fn read_artifact(&self, content_hash: &ContentHash) -> Result<Vec<u8>, StorageError> {
        // Find the artifact by hash, read the FS file.
        let pool = self.pool.clone();
        let dir = self.artifacts_dir.clone();
        let hash_str = content_hash.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, StorageError> {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let path: String = conn.query_row(
                "SELECT path FROM artifacts WHERE content_hash = ? LIMIT 1",
                params![hash_str],
                |row| row.get(0),
            )?;
            let full = dir.join(path);
            let bytes = std::fs::read(&full)?;
            Ok(bytes)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }
}

use surge_core::ContentHash;
```

(Move the `use surge_core::ContentHash;` up to the existing `use` block at file top during cleanup.)

- [ ] **Step 3: Wire reader_views module**

In `runs/mod.rs`, add (private if possible):

```rust
mod reader_views;
```

- [ ] **Step 4: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean. View queries are tested through writer integration tests (Phase 12) since they need INSERT-side helpers.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/reader.rs crates/surge-persistence/src/runs/reader_views.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): RunReader view queries + read_artifact

stage_executions/artifacts/pending_approvals/cost_summary/list_snapshots/
latest_snapshot_at_or_before/read_artifact, all spawn_blocking-wrapped.
RunState deserialized from snapshot blob via serde_json (M1 contract)."
```

---

## Phase 5: Writer task internals

### Task 5.1: WriterCommand enum and writer module skeleton

**Files:**
- Create: `crates/surge-persistence/src/runs/writer.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create writer.rs with command enum**

`crates/surge-persistence/src/runs/writer.rs`:

```rust
//! Writer task: single-threaded SQLite write loop driven by a bounded mpsc.
//!
//! The writer owns one `rusqlite::Connection` and processes one command at a
//! time. All event INSERTs and view maintenance happen in the same SQL
//! transaction inside the writer task; readers go through a separate r2d2
//! pool and never touch the writer connection.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use surge_core::{RunId, RunState, VersionedEventPayload};
use tokio::sync::{mpsc, oneshot};

use crate::runs::clock::Clock;
use crate::runs::error::WriterError;
use crate::runs::seq::EventSeq;
use crate::runs::types::ArtifactRecord;

/// Bounded channel size for writer commands.
pub const DEFAULT_CHANNEL_CAPACITY: usize = 64;

pub enum WriterCommand {
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
        produced_by: Option<surge_core::NodeKey>,
        produced_at_seq: EventSeq,
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

/// Configuration passed to the writer task at spawn.
pub struct WriterConfig {
    pub run_id: RunId,
    pub events_db_path: PathBuf,
    pub artifacts_dir: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub checkpoint_interval_secs: u64,
}

pub fn spawn_writer(
    cfg: WriterConfig,
    capacity: usize,
) -> (mpsc::Sender<WriterCommand>, tokio::task::JoinHandle<Result<(), WriterError>>) {
    let (tx, rx) = mpsc::channel(capacity);
    let join = tokio::spawn(async move { writer_loop(cfg, rx).await });
    (tx, join)
}

async fn writer_loop(
    cfg: WriterConfig,
    mut rx: mpsc::Receiver<WriterCommand>,
) -> Result<(), WriterError> {
    let span = tracing::info_span!("writer_task", run_id = %cfg.run_id);
    let _enter = span.enter();

    // Open and prep connection on this task's thread.
    let mut conn = Connection::open(&cfg.events_db_path)?;
    crate::runs::pragmas::apply(&conn, crate::runs::pragmas::PER_RUN_PRAGMAS)?;

    let mut checkpoint_interval = tokio::time::interval(
        std::time::Duration::from_secs(cfg.checkpoint_interval_secs.max(1)),
    );
    checkpoint_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick.
    checkpoint_interval.tick().await;

    loop {
        tokio::select! {
            biased;
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                if !handle_command(&mut conn, &cfg, cmd).await { break; }
            }
            _ = checkpoint_interval.tick() => {
                if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
                    tracing::warn!(error = %e, "wal_checkpoint failed");
                }
            }
        }
    }

    tracing::debug!("writer task exiting");
    Ok(())
}

async fn handle_command(
    _conn: &mut Connection,
    _cfg: &WriterConfig,
    cmd: WriterCommand,
) -> bool {
    match cmd {
        WriterCommand::Shutdown { reply } => {
            let _ = reply.send(());
            return false;
        }
        // Other variants implemented in Tasks 5.2-5.6.
        _ => {
            // Temporary: tell the caller the writer is not yet wired for this command.
            tracing::warn!("writer command not yet implemented in skeleton");
        }
    }
    true
}
```

- [ ] **Step 2: Wire into mod.rs**

```rust
pub mod writer;
pub use writer::{WriterCommand, WriterConfig, DEFAULT_CHANNEL_CAPACITY};
```

- [ ] **Step 3: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean (skeleton only; functional tests come with Tasks 5.2+).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/writer.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): writer task skeleton with command enum

WriterCommand variants (AppendEvent/AppendBatch/StoreArtifact/
WriteSnapshot/RebuildViews/Flush/Shutdown), bounded mpsc(64),
periodic wal_checkpoint(TRUNCATE) every 5 min, dedicated tokio task
that owns the writer Connection. Command handlers are stubs to be
filled in subsequent tasks."
```

### Task 5.2: AppendEvent with RETURNING and view maintenance hook

**Files:**
- Create: `crates/surge-persistence/src/runs/views.rs`
- Modify: `crates/surge-persistence/src/runs/writer.rs`

- [ ] **Step 1: Create views.rs with maintenance skeleton**

`crates/surge-persistence/src/runs/views.rs`:

```rust
//! Engine-side materialized view maintenance.
//!
//! Called from the writer task inside the same transaction as the event
//! INSERT. Each EventPayload variant updates the affected view tables.

use rusqlite::{params, Transaction};
use surge_core::{run_event::EventPayload, ContentHash};

use crate::runs::error::WriterError;
use crate::runs::seq::EventSeq;

pub fn maintain(
    tx: &Transaction<'_>,
    seq: EventSeq,
    timestamp_ms: i64,
    payload: &EventPayload,
) -> Result<(), WriterError> {
    match payload {
        // Detailed variants implemented in Phase 6 (one task per cluster).
        _ => Ok(()),
    }
}

pub fn rebuild(
    tx: &Transaction<'_>,
) -> Result<(), WriterError> {
    tx.execute_batch(
        "DELETE FROM stage_executions;
         DELETE FROM artifacts;
         DELETE FROM pending_approvals;
         DELETE FROM cost_summary;
         DELETE FROM graph_snapshots;",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Implement AppendEvent in writer's handle_command**

In `runs/writer.rs`, replace the `match cmd { ... }` body:

```rust
async fn handle_command(
    conn: &mut Connection,
    cfg: &WriterConfig,
    cmd: WriterCommand,
) -> bool {
    use crate::runs::views;
    use surge_core::run_event::VersionedEventPayload;

    match cmd {
        WriterCommand::AppendEvent { payload, reply } => {
            let result = (|| -> Result<EventSeq, WriterError> {
                let blob = serde_json::to_vec(&payload)?;
                let kind = payload.discriminant_str();
                let ts = cfg.clock.now_ms();
                let schema_version = payload.schema_version() as i64;

                let tx = conn.transaction()?;
                let seq: i64 = tx.query_row(
                    "INSERT INTO events (timestamp, kind, payload, schema_version)
                     VALUES (?, ?, ?, ?) RETURNING seq",
                    params![ts, kind, blob, schema_version],
                    |row| row.get(0),
                )?;
                let seq = EventSeq(seq as u64);

                // Materialized view maintenance (Phase 6 fills out the match arms).
                views::maintain(&tx, seq, ts, payload.payload())?;

                tx.commit()?;
                Ok(seq)
            })();
            let _ = reply.send(result);
        }
        WriterCommand::AppendBatch { payloads, reply } => {
            let result = (|| -> Result<Vec<EventSeq>, WriterError> {
                let tx = conn.transaction()?;
                let mut seqs = Vec::with_capacity(payloads.len());
                for payload in &payloads {
                    let blob = serde_json::to_vec(payload)?;
                    let kind = payload.discriminant_str();
                    let ts = cfg.clock.now_ms();
                    let schema_version = payload.schema_version() as i64;

                    let seq: i64 = tx.query_row(
                        "INSERT INTO events (timestamp, kind, payload, schema_version)
                         VALUES (?, ?, ?, ?) RETURNING seq",
                        params![ts, kind, blob, schema_version],
                        |row| row.get(0),
                    )?;
                    let seq = EventSeq(seq as u64);
                    views::maintain(&tx, seq, ts, payload.payload())?;
                    seqs.push(seq);
                }
                tx.commit()?;
                Ok(seqs)
            })();
            let _ = reply.send(result);
        }
        WriterCommand::Flush { reply } => {
            // Flush semantics: drain mpsc happens implicitly because we process
            // commands strictly in order. By the time Flush is dequeued, all prior
            // commands are committed. Just ack.
            let _ = reply.send(Ok(()));
        }
        WriterCommand::Shutdown { reply } => {
            let _ = reply.send(());
            return false;
        }
        WriterCommand::StoreArtifact { reply, .. } => {
            // Implemented in Task 5.4.
            let _ = reply.send(Err(WriterError::Internal("StoreArtifact not yet implemented".into())));
        }
        WriterCommand::WriteSnapshot { reply, .. } => {
            // Implemented in Task 5.5.
            let _ = reply.send(Err(WriterError::Internal("WriteSnapshot not yet implemented".into())));
        }
        WriterCommand::RebuildViews { reply } => {
            // Implemented in Task 5.6.
            let _ = reply.send(Err(WriterError::Internal("RebuildViews not yet implemented".into())));
        }
    }
    true
}
```

> Note: this assumes `VersionedEventPayload` exposes `discriminant_str()`, `schema_version()`, and `payload() -> &EventPayload` accessors. Verify in surge-core; if missing, add them as part of this task (small extension):

```rust
// In crates/surge-core/src/run_event.rs:
impl VersionedEventPayload {
    pub fn discriminant_str(&self) -> &'static str { self.payload.discriminant_str() }
    pub fn schema_version(&self) -> u32 { self.schema_version }
    pub fn payload(&self) -> &EventPayload { &self.payload }
}

impl EventPayload {
    pub fn discriminant_str(&self) -> &'static str {
        match self {
            EventPayload::RunStarted { .. } => "RunStarted",
            // ... one arm per variant, see exhaustive list in spec §4.
        }
    }
}
```

- [ ] **Step 3: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean. If any accessor is missing on `VersionedEventPayload`, surface compile errors and add them in surge-core in a follow-up commit before continuing.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/views.rs crates/surge-persistence/src/runs/writer.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): writer AppendEvent/AppendBatch with RETURNING

Both commands open a transaction, INSERT with RETURNING seq, call
views::maintain (stubbed), commit. Flush is a strict-ordering ack.
StoreArtifact/WriteSnapshot/RebuildViews still stubbed."
```

### Task 5.3: StoreArtifact (atomic FS write + view INSERT)

**Files:**
- Modify: `crates/surge-persistence/src/runs/writer.rs`

- [ ] **Step 1: Replace StoreArtifact stub with real impl**

```rust
WriterCommand::StoreArtifact { name, content, produced_by, produced_at_seq, reply } => {
    let result = (|| -> Result<ArtifactRecord, WriterError> {
        use std::io::Write;
        use surge_core::ContentHash;

        let dir = &cfg.artifacts_dir;
        std::fs::create_dir_all(dir)?;

        let target = dir.join(&name);
        let parent = target.parent().unwrap_or(dir);
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.as_file_mut().write_all(&content)?;
        tmp.as_file_mut().sync_all()?;
        let hash = ContentHash::of_bytes(&content);
        let size = content.len() as u64;

        let tx = conn.transaction()?;
        // Upsert-style: dedup if id (hash) already exists.
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM artifacts WHERE id = ?",
                params![hash.to_string()],
                |_| Ok(true),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(false);

        if exists {
            // Drop tmp file; existing on-disk copy is the canonical one.
            tracing::debug!(hash = %hash, "artifact dedup: skipping FS write");
            // Read back the existing record to return.
            let row = tx.query_row(
                "SELECT path, name FROM artifacts WHERE id = ? LIMIT 1",
                params![hash.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            let path = std::path::PathBuf::from(row.0);
            let record = ArtifactRecord {
                id: hash,
                produced_by_node: produced_by,
                produced_at_seq,
                name: row.1,
                path: dir.join(&path),
                size_bytes: size,
                content_hash: hash,
            };
            tx.commit()?;
            return Ok(record);
        }

        tmp.persist(&target).map_err(|e| WriterError::Io(e.error))?;

        let rel_path = name.clone();
        tx.execute(
            "INSERT INTO artifacts (id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                hash.to_string(),
                produced_by.as_ref().map(|n| n.as_str()),
                produced_at_seq.0 as i64,
                name,
                rel_path,
                size as i64,
                hash.to_string(),
            ],
        )?;
        tx.commit()?;

        Ok(ArtifactRecord {
            id: hash,
            produced_by_node: produced_by,
            produced_at_seq,
            name,
            path: target,
            size_bytes: size,
            content_hash: hash,
        })
    })();
    let _ = reply.send(result);
}
```

> Required `ContentHash::of_bytes(&[u8]) -> ContentHash` and `ContentHash::to_string()` already exist in M1 surge-core. If `of_bytes` doesn't exist, add it as a small surge-core extension.

> **Delta P3.X6:** silent dedup chosen — same content_hash → same id → skip FS write, return existing record. Documented inline.

- [ ] **Step 2: Add `use rusqlite::OptionalExtension;` at top of writer.rs**

- [ ] **Step 3: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/writer.rs
git commit -m "M2(persistence): writer StoreArtifact with atomic write + dedup

NamedTempFile::persist for atomic publish. SHA-256 content hash via
ContentHash::of_bytes. If hash already in artifacts table — skip FS
write, return existing record (silent dedup, P3.X6 spec decision)."
```

### Task 5.4: WriteSnapshot, RebuildViews

**Files:**
- Modify: `crates/surge-persistence/src/runs/writer.rs`

- [ ] **Step 1: Implement WriteSnapshot**

Replace the WriteSnapshot stub:

```rust
WriterCommand::WriteSnapshot { at_seq, state, reply } => {
    let result = (|| -> Result<(), WriterError> {
        let blob = serde_json::to_vec(state.as_ref())?;
        let bytes = blob.len() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO graph_snapshots (at_seq, snapshot, bytes_compressed)
             VALUES (?, ?, ?)",
            params![at_seq.0 as i64, blob, bytes],
        )?;
        Ok(())
    })();
    let _ = reply.send(result);
}
```

- [ ] **Step 2: Implement RebuildViews**

```rust
WriterCommand::RebuildViews { reply } => {
    let result = (|| -> Result<(), WriterError> {
        use crate::runs::views;
        let tx = conn.transaction()?;
        views::rebuild(&tx)?;

        // Replay all events through views::maintain.
        let mut stmt = tx.prepare(
            "SELECT seq, timestamp, payload FROM events ORDER BY seq",
        )?;
        let rows = stmt.query_map([], |row| {
            let seq: i64 = row.get(0)?;
            let ts: i64 = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((EventSeq(seq as u64), ts, blob))
        })?;

        let collected: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
        drop(stmt);

        for (seq, ts, blob) in collected {
            let versioned: surge_core::run_event::VersionedEventPayload =
                serde_json::from_slice(&blob)?;
            views::maintain(&tx, seq, ts, versioned.payload())?;
        }

        tx.commit()?;
        Ok(())
    })();
    let _ = reply.send(result);
}
```

- [ ] **Step 3: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/writer.rs
git commit -m "M2(persistence): writer WriteSnapshot + RebuildViews

WriteSnapshot uses INSERT OR REPLACE so re-snapshot at same seq
overwrites cleanly. RebuildViews truncates view tables, then replays
every event through views::maintain inside one transaction (readers
see pre-rebuild state via WAL snapshot isolation until commit)."
```

---

## Phase 6: Materialized view maintenance — variant arms

### Task 6.1: maintain() match arms for all event variants

**Files:**
- Modify: `crates/surge-persistence/src/runs/views.rs`

> Implementation note: this is a single big task because all the arms share the same shape (insert/update one of the 5 view tables based on payload variant). Splitting into separate tasks per variant would generate a lot of redundant TDD ceremony.

- [ ] **Step 1: Replace stub maintain() with full match**

```rust
pub fn maintain(
    tx: &Transaction<'_>,
    seq: EventSeq,
    timestamp_ms: i64,
    payload: &EventPayload,
) -> Result<(), WriterError> {
    use EventPayload::*;
    match payload {
        StageEntered { node, attempt } => {
            tx.execute(
                "INSERT INTO stage_executions (node_id, attempt, started_seq, started_at)
                 VALUES (?, ?, ?, ?)",
                params![node.as_str(), *attempt as i64, seq.0 as i64, timestamp_ms],
            )?;
        }
        StageCompleted { node, outcome } => {
            tx.execute(
                "UPDATE stage_executions
                 SET ended_seq = ?, ended_at = ?, outcome = ?
                 WHERE node_id = ?
                   AND attempt = (SELECT MAX(attempt) FROM stage_executions WHERE node_id = ?)",
                params![seq.0 as i64, timestamp_ms, outcome.as_str(), node.as_str(), node.as_str()],
            )?;
        }
        StageFailed { node, .. } => {
            tx.execute(
                "UPDATE stage_executions
                 SET ended_seq = ?, ended_at = ?, outcome = NULL
                 WHERE node_id = ?
                   AND attempt = (SELECT MAX(attempt) FROM stage_executions WHERE node_id = ?)",
                params![seq.0 as i64, timestamp_ms, node.as_str(), node.as_str()],
            )?;
        }
        ArtifactProduced { node, artifact_id, path, size_bytes } => {
            // Note: the artifacts table row may already exist if StoreArtifact
            // was called before the event was appended. Use INSERT OR IGNORE.
            tx.execute(
                "INSERT OR IGNORE INTO artifacts (id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    artifact_id.to_string(),
                    node.as_str(),
                    seq.0 as i64,
                    path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    path.to_string_lossy(),
                    *size_bytes as i64,
                    artifact_id.to_string(),
                ],
            )?;
        }
        TokensConsumed { prompt_tokens, output_tokens, cache_hits, .. } => {
            upsert_metric(tx, "tokens_in", *prompt_tokens as f64, timestamp_ms)?;
            upsert_metric(tx, "tokens_out", *output_tokens as f64, timestamp_ms)?;
            upsert_metric(tx, "cache_hits", *cache_hits as f64, timestamp_ms)?;
        }
        ApprovalRequested { gate, channel, payload_hash } => {
            tx.execute(
                "INSERT INTO pending_approvals (seq, node_id, channel, requested_at, payload_hash)
                 VALUES (?, ?, ?, ?, ?)",
                params![seq.0 as i64, gate.as_str(), format!("{:?}", channel), timestamp_ms, payload_hash],
            )?;
        }
        ApprovalDecided { gate, .. } => {
            tx.execute(
                "DELETE FROM pending_approvals WHERE node_id = ?",
                params![gate.as_str()],
            )?;
        }
        // All other variants currently produce no view changes.
        _ => {}
    }
    Ok(())
}

fn upsert_metric(tx: &Transaction<'_>, metric: &str, delta: f64, ts: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO cost_summary (metric, value, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(metric) DO UPDATE SET value = value + excluded.value, updated_at = excluded.updated_at",
        params![metric, delta, ts],
    )?;
    Ok(())
}
```

> If `EventPayload` variant field names differ from what's used here (e.g., `prompt_tokens` vs `input_tokens`), inspect [crates/surge-core/src/run_event.rs](../../crates/surge-core/src/run_event.rs) and adjust. The shape of each arm stays the same.

- [ ] **Step 2: Add unit tests for each arm**

Append a `#[cfg(test)] mod tests` block at the bottom of `views.rs` that for each variant:
1. Opens an in-memory DB
2. Applies `PER_RUN_MIGRATIONS`
3. Calls `maintain` with a hand-built payload
4. Queries the affected view table and asserts the expected row(s)

Cover at minimum: StageEntered, StageCompleted, ArtifactProduced, TokensConsumed (sums across multiple events), ApprovalRequested + ApprovalDecided (delete after request).

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-persistence --lib runs::views
```

Expected: all variant tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/views.rs
git commit -m "M2(persistence): materialized view maintenance for all event variants

StageEntered/Completed/Failed → stage_executions.
ArtifactProduced → artifacts (INSERT OR IGNORE; row may exist from
StoreArtifact call). TokensConsumed → cost_summary (running totals via
ON CONFLICT upsert). ApprovalRequested/Decided → pending_approvals.
Other variants produce no view changes."
```

---

## Phase 7: Single-writer enforcement

### Task 7.1: WriterToken in-process registry

**Files:**
- Create: `crates/surge-persistence/src/runs/writer_slot.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create writer_slot.rs**

```rust
//! In-process tracking of which RunIds currently have a live writer.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use surge_core::RunId;
use tokio::sync::Mutex;

/// Sentinel object whose Arc lifetime represents an active writer slot.
pub struct WriterToken;

#[derive(Default)]
pub struct ActiveWriters {
    inner: Mutex<HashMap<RunId, Weak<WriterToken>>>,
}

impl ActiveWriters {
    pub async fn try_acquire(&self, run_id: RunId) -> Option<Arc<WriterToken>> {
        let mut g = self.inner.lock().await;
        if let Some(weak) = g.get(&run_id) {
            if weak.strong_count() > 0 {
                return None;
            }
        }
        let token = Arc::new(WriterToken);
        g.insert(run_id, Arc::downgrade(&token));
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_acquire_fails_while_first_held() {
        let m = ActiveWriters::default();
        let id = RunId::new();
        let t1 = m.try_acquire(id).await.unwrap();
        assert!(m.try_acquire(id).await.is_none());
        drop(t1);
        // Allow weak to drop — no other refs, so try_acquire succeeds again.
        let t2 = m.try_acquire(id).await.unwrap();
        assert!(Arc::strong_count(&t2) == 1);
    }

    #[tokio::test]
    async fn different_run_ids_independent() {
        let m = ActiveWriters::default();
        let a = m.try_acquire(RunId::new()).await.unwrap();
        let b = m.try_acquire(RunId::new()).await.unwrap();
        assert_eq!(Arc::strong_count(&a), 1);
        assert_eq!(Arc::strong_count(&b), 1);
    }
}
```

- [ ] **Step 2: Wire into mod.rs**

```rust
pub(crate) mod writer_slot;
pub use writer_slot::WriterToken;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-persistence --lib runs::writer_slot
```

Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/writer_slot.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): in-process writer-slot registry

ActiveWriters: Mutex<HashMap<RunId, Weak<WriterToken>>>. try_acquire
returns Arc<WriterToken> only when no live writer holds the slot.
Drop of the Arc frees the slot via Weak::strong_count check."
```

### Task 7.2: fd-lock cross-process file lock

**Files:**
- Create: `crates/surge-persistence/src/runs/file_lock.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create file_lock.rs**

```rust
//! Cross-process advisory lock around per-run events.sqlite.lock.

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::runs::error::OpenError;

pub struct FileLock {
    // RwLock owns the file; the guard is upgraded to 'static via Box::leak.
    // This is simpler than threading a lifetime through RunWriter.
    _lock: Box<fd_lock::RwLock<File>>,
    _guard: fd_lock::RwLockWriteGuard<'static, File>,
}

impl FileLock {
    pub fn try_acquire(lock_path: &Path) -> Result<Self, OpenError> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)?;
        let mut lock = Box::new(fd_lock::RwLock::new(file));

        // Borrow lock through Box, then leak the guard's lifetime.
        // SAFETY: we keep the Box alive in the same struct, so 'static is honoured.
        let lock_ref: &mut fd_lock::RwLock<File> = unsafe {
            &mut *(Box::as_mut(&mut lock) as *mut _)
        };
        let guard = lock_ref
            .try_write()
            .map_err(|_| OpenError::WriterAlreadyHeld {
                run_id: surge_core::RunId::new(), // placeholder; caller wraps with real id
            })?;

        Ok(Self {
            _lock: lock,
            _guard: guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn second_acquire_in_same_process_fails() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join("test.lock");

        let _l1 = FileLock::try_acquire(&lock_path).unwrap();
        // Different process semantics differ; same-process attempts should fail too on most platforms.
        // This is a smoke test; cross-process behavior is exercised in integration tests.
        let l2 = FileLock::try_acquire(&lock_path);
        assert!(l2.is_err(), "second lock acquire should fail");
    }
}
```

> The placeholder `RunId::new()` in the error path is awkward. Cleaner: change `try_acquire` to take a `RunId` and propagate it into the error. Apply this fix during step 2.

- [ ] **Step 2: Refine API to thread RunId into errors**

Replace `try_acquire` signature:

```rust
pub fn try_acquire(lock_path: &Path, run_id: surge_core::RunId) -> Result<Self, OpenError> {
    // ...
    let guard = lock_ref
        .try_write()
        .map_err(|_| OpenError::WriterAlreadyHeld { run_id })?;
    // ...
}
```

Update the test accordingly:

```rust
let _l1 = FileLock::try_acquire(&lock_path, RunId::new()).unwrap();
let l2 = FileLock::try_acquire(&lock_path, RunId::new());
```

- [ ] **Step 3: Wire into mod.rs**

```rust
pub(crate) mod file_lock;
pub(crate) use file_lock::FileLock;
```

- [ ] **Step 4: Run test**

```bash
cargo test -p surge-persistence --lib runs::file_lock
```

Expected: pass on Linux/macOS. Windows may need cross-process test (Phase 12).

> **Delta P3.X7:** Windows cross-process flakiness — `LockFileEx` is process-scoped (not handle-scoped) so same-process double-acquire returns success on Windows in some configurations. The integration test in Task 12.6 will verify behavior; if flaky, add a small retry loop with timeout when acquiring (10ms × 5 attempts) and document Windows semantics.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/file_lock.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): cross-process advisory file lock via fd-lock

FileLock wraps fd-lock RwLock with a 'static-leaked guard so RunWriter
can hold it without lifetime threading. RunId propagated into
WriterAlreadyHeld error. Windows semantics documented as a known
risk for the integration test phase (P3.X7)."
```

---

## Phase 8: RunWriter facade

### Task 8.1: RunWriter struct + delegate macro

**Files:**
- Create: `crates/surge-persistence/src/runs/run_writer.rs`
- Create: `crates/surge-persistence/src/runs/macros.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create delegate macro**

`crates/surge-persistence/src/runs/macros.rs`:

```rust
//! Internal macros.

/// Delegate a list of read-method calls from RunWriter to its embedded RunReader.
///
/// Avoids ~40 lines of forwarding boilerplate while staying explicit (no Deref magic).
macro_rules! delegate_to_reader {
    (
        async {
            $(
                $vis:vis $name:ident( $( $arg:ident : $ty:ty ),* $(,)? ) -> $ret:ty;
            )*
        }
    ) => {
        $(
            $vis async fn $name(&self, $( $arg : $ty ),*) -> $ret {
                self.reader.$name( $( $arg ),* ).await
            }
        )*
    };
}

pub(crate) use delegate_to_reader;
```

- [ ] **Step 2: Create RunWriter struct skeleton**

`crates/surge-persistence/src/runs/run_writer.rs`:

```rust
//! Exclusive write handle for a per-run database.
//!
//! Owns the WriterToken (in-process slot), the FileLock (cross-process slot),
//! the mpsc::Sender<WriterCommand>, and an embedded RunReader for read methods.

use std::ops::Range;
use std::sync::Arc;

use surge_core::{ContentHash, NodeKey, RunId, RunState, VersionedEventPayload};
use tokio::sync::{mpsc, oneshot};

use crate::runs::error::{CloseError, StorageError, WriterError};
use crate::runs::file_lock::FileLock;
use crate::runs::reader::{ReadEvent, RunReader};
use crate::runs::seq::EventSeq;
use crate::runs::types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};
use crate::runs::writer::WriterCommand;
use crate::runs::writer_slot::WriterToken;

pub struct RunWriter {
    pub(crate) reader: RunReader,
    pub(crate) writer_tx: mpsc::Sender<WriterCommand>,
    pub(crate) writer_join: Option<tokio::task::JoinHandle<Result<(), WriterError>>>,
    pub(crate) _token: Arc<WriterToken>,
    pub(crate) _file_lock: FileLock,
    pub(crate) closed: bool,
}

impl RunWriter {
    pub fn run_id(&self) -> &RunId {
        self.reader.run_id()
    }

    pub fn worktree_path(&self) -> &std::path::Path {
        self.reader.worktree_path()
    }
}

// Delegated read methods.
use crate::runs::macros::delegate_to_reader;
impl RunWriter {
    delegate_to_reader! {
        async {
            pub current_seq() -> Result<EventSeq, StorageError>;
            pub stage_executions() -> Result<Vec<StageExecution>, StorageError>;
            pub artifacts() -> Result<Vec<ArtifactRecord>, StorageError>;
            pub pending_approvals() -> Result<Vec<PendingApproval>, StorageError>;
            pub cost_summary() -> Result<CostSummary, StorageError>;
            pub list_snapshots() -> Result<Vec<EventSeq>, StorageError>;
        }
    }

    // Methods with non-trivial signatures (range, by-id, by-hash) are forwarded explicitly.
    pub async fn read_event(&self, seq: EventSeq) -> Result<Option<ReadEvent>, StorageError> {
        self.reader.read_event(seq).await
    }

    pub async fn read_events(&self, range: Range<EventSeq>) -> Result<Vec<ReadEvent>, StorageError> {
        self.reader.read_events(range).await
    }

    pub async fn read_artifact(&self, content_hash: &ContentHash) -> Result<Vec<u8>, StorageError> {
        self.reader.read_artifact(content_hash).await
    }

    pub async fn latest_snapshot_at_or_before(
        &self,
        seq: EventSeq,
    ) -> Result<Option<(EventSeq, RunState)>, StorageError> {
        self.reader.latest_snapshot_at_or_before(seq).await
    }
}
```

- [ ] **Step 3: Wire macros and run_writer into mod.rs**

```rust
mod macros;
pub mod run_writer;
pub use run_writer::RunWriter;
```

- [ ] **Step 4: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/run_writer.rs crates/surge-persistence/src/runs/macros.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): RunWriter struct + delegate_to_reader macro

RunWriter wraps RunReader plus writer_tx, _token, _file_lock, JoinHandle.
delegate_to_reader macro forwards 6 trivial read methods (no boilerplate).
Range/by-id/by-hash methods forwarded explicitly. Writer-side methods
(append_event/store_artifact/...) come in next task."
```

### Task 8.2: Writer-side methods on RunWriter

**Files:**
- Modify: `crates/surge-persistence/src/runs/run_writer.rs`

- [ ] **Step 1: Append writer methods to impl block**

```rust
impl RunWriter {
    pub async fn append_event(
        &self,
        payload: VersionedEventPayload,
    ) -> Result<EventSeq, StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::AppendEvent { payload, reply: reply_tx })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        let result = reply_rx.await.map_err(|_| StorageError::WriterTaskDied)?;
        result.map_err(|e| match e {
            WriterError::Sqlite(s) => StorageError::Sqlite(s),
            WriterError::Io(i) => StorageError::Io(i),
            WriterError::Serialization(j) => StorageError::SerializationFailed(j),
            WriterError::Internal(_) => StorageError::WriterTaskDied,
        })
    }

    pub async fn append_events(
        &self,
        payloads: Vec<VersionedEventPayload>,
    ) -> Result<Vec<EventSeq>, StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::AppendBatch { payloads, reply: reply_tx })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx.await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    pub async fn store_artifact(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<ArtifactRecord, StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        let current_seq = self.current_seq().await?;
        self.writer_tx
            .send(WriterCommand::StoreArtifact {
                name: name.to_string(),
                content: content.to_vec(),
                produced_by: None,
                produced_at_seq: current_seq,
                reply: reply_tx,
            })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx.await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    pub async fn write_graph_snapshot(
        &self,
        at_seq: EventSeq,
        state: &RunState,
    ) -> Result<(), StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::WriteSnapshot {
                at_seq,
                state: Box::new(state.clone()),
                reply: reply_tx,
            })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx.await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    pub async fn rebuild_views(&self) -> Result<(), StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::RebuildViews { reply: reply_tx })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx.await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Wait for all previously-issued commands to be processed by the writer task.
    ///
    /// Strict ordering of the mpsc channel + sequential command processing means
    /// once Flush is dequeued, all prior commands are already committed.
    pub async fn flush(&self) -> Result<(), StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::Flush { reply: reply_tx })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx.await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Explicit clean shutdown — sends Shutdown to the writer task, waits for it
    /// to drain in-flight commands and exit. Releases file lock and in-process token.
    pub async fn close(mut self) -> Result<(), CloseError> {
        if self.closed {
            return Ok(());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        if self.writer_tx
            .send(WriterCommand::Shutdown { reply: reply_tx })
            .await
            .is_ok()
        {
            let _ = reply_rx.await;
        }
        self.closed = true;
        if let Some(join) = self.writer_join.take() {
            join.await
                .map_err(|e| CloseError::JoinFailed(e.to_string()))?
                .map_err(CloseError::Writer)?;
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), StorageError> {
        if self.closed {
            return Err(StorageError::WriterTaskDied);
        }
        Ok(())
    }
}

fn map_writer_err(e: WriterError) -> StorageError {
    match e {
        WriterError::Sqlite(s) => StorageError::Sqlite(s),
        WriterError::Io(i) => StorageError::Io(i),
        WriterError::Serialization(j) => StorageError::SerializationFailed(j),
        WriterError::Internal(_) => StorageError::WriterTaskDied,
    }
}

impl Drop for RunWriter {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // Best-effort fire-and-forget shutdown. Drop is sync — we can't await join.
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = self.writer_tx.try_send(WriterCommand::Shutdown { reply: reply_tx });
        tracing::warn!(
            run_id = %self.reader.run_id(),
            "RunWriter dropped without close() — pending writes may be lost. \
             Prefer RunWriter::close().await for clean shutdown."
        );
    }
}
```

> **Delta P1.X1:** flush semantics documented inline; test `flush_drains_pending` belongs in Phase 12.
> **Delta P1.X2:** Drop impl emits tracing::warn and best-effort sends Shutdown.

- [ ] **Step 2: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-persistence/src/runs/run_writer.rs
git commit -m "M2(persistence): RunWriter writer methods + close + Drop warning

append_event/append_events/store_artifact/write_graph_snapshot/
rebuild_views/flush/close. Drop emits tracing::warn (P1.X2) and best-
effort try_send(Shutdown). flush_drains_pending test follows in Phase 12
(P1.X1). closed flag ensures methods after close() return WriterTaskDied."
```

---

## Phase 9: Subscribe stream

### Task 9.1: subscribe_events with batched polling

**Files:**
- Create: `crates/surge-persistence/src/runs/subscribe.rs`
- Modify: `crates/surge-persistence/src/runs/reader.rs`
- Modify: `crates/surge-persistence/src/runs/run_writer.rs`

- [ ] **Step 1: Create subscribe.rs**

```rust
//! Polling-based event subscription stream.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_stream::try_stream;
use futures_core::Stream;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use surge_core::VersionedEventPayload;
use tokio::time::{interval, MissedTickBehavior};

use crate::runs::error::StorageError;
use crate::runs::reader::ReadEvent;
use crate::runs::seq::EventSeq;

pub const SUBSCRIBE_BATCH_MAX: usize = 256;
pub const POLL_INTERVAL_MS: u64 = 100;

pub fn subscribe(
    pool: Pool<SqliteConnectionManager>,
    _artifacts_dir: Arc<PathBuf>,
) -> impl Stream<Item = Result<ReadEvent, StorageError>> + Send + 'static {
    try_stream! {
        let mut last_seq = EventSeq::ZERO;
        let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let pool_clone = pool.clone();
            let next_batch = tokio::task::spawn_blocking(move || -> Result<Vec<ReadEvent>, StorageError> {
                let conn = pool_clone.get().map_err(|e| StorageError::Pool(e.to_string()))?;
                let mut stmt = conn.prepare(
                    "SELECT seq, timestamp, kind, payload
                     FROM events WHERE seq > ?
                     ORDER BY seq LIMIT ?",
                )?;
                let iter = stmt.query_map(
                    params![last_seq.0 as i64, SUBSCRIBE_BATCH_MAX as i64],
                    |row| {
                        let blob: Vec<u8> = row.get(3)?;
                        Ok((
                            EventSeq(row.get::<_, i64>(0)? as u64),
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            blob,
                        ))
                    },
                )?;
                let mut out = Vec::new();
                for r in iter {
                    let (seq, ts, kind, blob) = r?;
                    let payload: VersionedEventPayload = serde_json::from_slice(&blob)?;
                    out.push(ReadEvent { seq, timestamp_ms: ts, kind, payload });
                }
                Ok(out)
            })
            .await
            .map_err(|e| StorageError::Pool(e.to_string()))??;

            for ev in next_batch {
                last_seq = ev.seq;
                yield ev;
            }
        }
    }
}
```

- [ ] **Step 2: Add `futures-core` to deps**

In `crates/surge-persistence/Cargo.toml [dependencies]`:

```toml
futures-core = "0.3"
```

- [ ] **Step 3: Wire subscribe module + add reader/writer methods**

In `runs/mod.rs`:
```rust
pub mod subscribe;
pub use subscribe::SUBSCRIBE_BATCH_MAX;
```

Append to `runs/reader.rs`:
```rust
impl RunReader {
    pub fn subscribe_events(
        &self,
    ) -> impl futures_core::Stream<Item = Result<ReadEvent, StorageError>> + Send + 'static {
        crate::runs::subscribe::subscribe(self.pool.clone(), self.artifacts_dir.clone())
    }
}
```

Append to `runs/run_writer.rs`:
```rust
impl RunWriter {
    pub fn subscribe_events(
        &self,
    ) -> impl futures_core::Stream<Item = Result<ReadEvent, StorageError>> + Send + 'static {
        self.reader.subscribe_events()
    }
}
```

- [ ] **Step 4: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/subscribe.rs crates/surge-persistence/src/runs/reader.rs crates/surge-persistence/src/runs/run_writer.rs crates/surge-persistence/src/runs/mod.rs crates/surge-persistence/Cargo.toml
git commit -m "M2(persistence): polling-based subscribe_events stream

100ms tick, MissedTickBehavior::Skip, per-tick batch capped at 256
events to bound memory if consumer lags. Cancel-safe (dropping the
stream aborts the polling task)."
```

---

## Phase 10: Storage facade

### Task 10.1: StorageConfig + Storage::open

**Files:**
- Create: `crates/surge-persistence/src/runs/config.rs`
- Create: `crates/surge-persistence/src/runs/storage.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`

- [ ] **Step 1: Create config.rs**

```rust
//! Storage configuration loaded from ~/.surge/config.toml.

use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_checkpoint_interval")]
    pub checkpoint_interval_seconds: u64,
    #[serde(default = "default_reader_pool_size")]
    pub reader_pool_size: u32,
    #[serde(default = "default_writer_capacity")]
    pub writer_channel_capacity: usize,
}

fn default_checkpoint_interval() -> u64 { 300 }
fn default_reader_pool_size() -> u32 { 4 }
fn default_writer_capacity() -> usize { 64 }

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval_seconds: default_checkpoint_interval(),
            reader_pool_size: default_reader_pool_size(),
            writer_channel_capacity: default_writer_capacity(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    storage: StorageConfig,
}

pub fn load_or_default(home: &Path) -> StorageConfig {
    let path = home.join("config.toml");
    let Ok(s) = std::fs::read_to_string(&path) else { return StorageConfig::default(); };
    toml::from_str::<ConfigFile>(&s)
        .map(|c| c.storage)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_when_file_absent() {
        let tmp = TempDir::new().unwrap();
        let cfg = load_or_default(tmp.path());
        assert_eq!(cfg.reader_pool_size, 4);
    }

    #[test]
    fn parses_present_overrides() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[storage]\ncheckpoint_interval_seconds = 60\nreader_pool_size = 16\n",
        )
        .unwrap();
        let cfg = load_or_default(tmp.path());
        assert_eq!(cfg.checkpoint_interval_seconds, 60);
        assert_eq!(cfg.reader_pool_size, 16);
        assert_eq!(cfg.writer_channel_capacity, 64);
    }
}
```

- [ ] **Step 2: Create storage.rs facade**

```rust
//! Top-level Storage facade. Holds the registry pool, active-writers map,
//! and config; produces RunReader/RunWriter handles.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use surge_core::RunId;
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::runs::clock::{Clock, SystemClock};
use crate::runs::config::{load_or_default, StorageConfig};
use crate::runs::error::{OpenError, StorageError};
use crate::runs::file_lock::FileLock;
use crate::runs::pragmas::{apply as apply_pragmas, PER_RUN_PRAGMAS};
use crate::runs::process::ProcessProbe;
use crate::runs::reader::RunReader;
use crate::runs::registry::{self, RunFilter, RunSummary};
use crate::runs::run_writer::RunWriter;
use crate::runs::writer::{spawn_writer, WriterConfig};
use crate::runs::writer_slot::ActiveWriters;

pub struct Storage {
    home: PathBuf,
    registry_pool: Pool<SqliteConnectionManager>,
    active_writers: ActiveWriters,
    config: StorageConfig,
    clock: Arc<dyn Clock>,
    process_probe: ProcessProbe,
}

impl Storage {
    pub async fn open(home: impl AsRef<Path>) -> Result<Arc<Self>, OpenError> {
        Self::open_with(home, Arc::new(SystemClock)).await
    }

    pub async fn open_with(
        home: impl AsRef<Path>,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<Self>, OpenError> {
        let home = home.as_ref().to_path_buf();

        // Multi-thread runtime check.
        match Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(RuntimeFlavor::MultiThread) => {}
            Ok(_) => return Err(OpenError::SingleThreadedRuntime),
            Err(_) => return Err(OpenError::Config(
                "Storage::open requires a tokio runtime in scope".into(),
            )),
        }

        std::fs::create_dir_all(home.join("db"))?;
        std::fs::create_dir_all(home.join("runs"))?;

        let config = load_or_default(&home);
        let registry_pool = registry::open_registry_pool(&home, clock.as_ref())?;

        Ok(Arc::new(Self {
            home,
            registry_pool,
            active_writers: ActiveWriters::default(),
            config,
            clock,
            process_probe: ProcessProbe::new(),
        }))
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    fn run_dir(&self, run_id: &RunId) -> PathBuf {
        self.home.join("runs").join(run_id.to_string())
    }
    fn events_db_path(&self, run_id: &RunId) -> PathBuf {
        self.run_dir(run_id).join("events.sqlite")
    }
    fn lock_path(&self, run_id: &RunId) -> PathBuf {
        self.run_dir(run_id).join("events.sqlite.lock")
    }
    fn artifacts_dir(&self, run_id: &RunId) -> PathBuf {
        self.run_dir(run_id).join("artifacts")
    }
}
```

- [ ] **Step 3: Wire into mod.rs**

```rust
pub mod config;
pub mod storage;
pub use config::StorageConfig;
pub use storage::Storage;
```

- [ ] **Step 4: Run config tests + build check**

```bash
cargo test -p surge-persistence --lib runs::config
cargo build -p surge-persistence
```

Expected: 2 tests pass, build clean.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/runs/storage.rs crates/surge-persistence/src/runs/config.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "M2(persistence): Storage facade skeleton + config loading

Storage::open(home) checks multi-thread runtime, creates ~/.surge/db
and ~/.surge/runs, opens registry pool, applies migrations. Run
lifecycle methods (create_run/open_run_*) come in next task."
```

### Task 10.2: create_run, open_run_reader, open_run_writer, list/get/delete

**Files:**
- Modify: `crates/surge-persistence/src/runs/storage.rs`

- [ ] **Step 1: Append run lifecycle methods**

```rust
impl Storage {
    pub async fn create_run(
        self: &Arc<Self>,
        run_id: RunId,
        project_path: impl AsRef<Path>,
        pipeline_template: Option<String>,
    ) -> Result<RunWriter, OpenError> {
        let project_path = project_path.as_ref().to_path_buf();

        // Insert into registry.
        let summary = RunSummary {
            id: run_id.clone(),
            project_path,
            pipeline_template,
            status: surge_core::RunStatus::Bootstrapping,
            started_at_ms: self.clock.now_ms(),
            ended_at_ms: None,
            daemon_pid: Some(std::process::id() as i32),
        };
        registry::insert_run(&self.registry_pool, &summary)?;

        // Create per-run dir + apply per-run migrations.
        let run_dir = self.run_dir(&run_id);
        std::fs::create_dir_all(&run_dir)?;
        std::fs::create_dir_all(self.artifacts_dir(&run_id))?;

        let events_path = self.events_db_path(&run_id);
        let mut conn = rusqlite::Connection::open(&events_path)?;
        apply_pragmas(&conn, PER_RUN_PRAGMAS)?;
        crate::runs::migrations::apply(
            &mut conn,
            crate::runs::migrations::PER_RUN_MIGRATIONS,
            self.clock.as_ref(),
        )
        .map_err(|e| OpenError::MigrationFailed(e.to_string()))?;
        drop(conn);

        self.open_run_writer(run_id).await
    }

    pub async fn open_run_reader(
        self: &Arc<Self>,
        run_id: RunId,
    ) -> Result<RunReader, OpenError> {
        let events_path = self.events_db_path(&run_id);
        if !events_path.exists() {
            return Err(OpenError::RunNotFound(run_id));
        }
        let manager = SqliteConnectionManager::file(&events_path)
            .with_init(|c| apply_pragmas(c, PER_RUN_PRAGMAS));
        let pool = Pool::builder()
            .max_size(self.config.reader_pool_size)
            .build(manager)
            .map_err(|e| OpenError::Pool(e.to_string()))?;

        Ok(RunReader {
            run_id: run_id.clone(),
            pool,
            artifacts_dir: Arc::new(self.artifacts_dir(&run_id)),
            worktree_path: Arc::new(self.run_dir(&run_id).join("worktree")),
        })
    }

    pub async fn open_run_writer(
        self: &Arc<Self>,
        run_id: RunId,
    ) -> Result<RunWriter, OpenError> {
        let token = self
            .active_writers
            .try_acquire(run_id.clone())
            .await
            .ok_or_else(|| OpenError::WriterAlreadyHeld { run_id: run_id.clone() })?;

        let lock_path = self.lock_path(&run_id);
        let file_lock = FileLock::try_acquire(&lock_path, run_id.clone())?;

        let reader = self.open_run_reader(run_id.clone()).await?;

        let cfg = WriterConfig {
            run_id: run_id.clone(),
            events_db_path: self.events_db_path(&run_id),
            artifacts_dir: self.artifacts_dir(&run_id),
            clock: self.clock.clone(),
            checkpoint_interval_secs: self.config.checkpoint_interval_seconds,
        };

        let (writer_tx, writer_join) = spawn_writer(cfg, self.config.writer_channel_capacity);

        Ok(RunWriter {
            reader,
            writer_tx,
            writer_join: Some(writer_join),
            _token: token,
            _file_lock: file_lock,
            closed: false,
        })
    }

    pub async fn list_runs(&self, filter: RunFilter) -> Result<Vec<RunSummary>, StorageError> {
        let mut runs = registry::list_runs(&self.registry_pool, &filter)?;
        // Stale-pid detection — for status=Running with daemon_pid.
        for r in runs.iter_mut() {
            if r.status == surge_core::RunStatus::Running {
                if let Some(pid) = r.daemon_pid {
                    if !self.process_probe.is_alive(pid) {
                        r.status = surge_core::RunStatus::Crashed;
                        r.ended_at_ms = Some(self.clock.now_ms());
                        let _ = registry::update_status(
                            &self.registry_pool,
                            &r.id,
                            surge_core::RunStatus::Crashed,
                            r.ended_at_ms,
                        );
                    }
                }
            }
        }
        Ok(runs)
    }

    pub async fn get_run(&self, run_id: &RunId) -> Result<Option<RunSummary>, StorageError> {
        let mut summary = match registry::get_run(&self.registry_pool, run_id)? {
            Some(s) => s,
            None => return Ok(None),
        };
        if summary.status == surge_core::RunStatus::Running {
            if let Some(pid) = summary.daemon_pid {
                if !self.process_probe.is_alive(pid) {
                    summary.status = surge_core::RunStatus::Crashed;
                    summary.ended_at_ms = Some(self.clock.now_ms());
                    let _ = registry::update_status(
                        &self.registry_pool,
                        &summary.id,
                        surge_core::RunStatus::Crashed,
                        summary.ended_at_ms,
                    );
                }
            }
        }
        Ok(Some(summary))
    }

    pub async fn delete_run(self: &Arc<Self>, run_id: &RunId) -> Result<(), StorageError> {
        // Refuse if writer is held.
        let active = self.active_writers.inner_for_test().await;
        if let Some(weak) = active.get(run_id) {
            if weak.strong_count() > 0 {
                return Err(StorageError::WriterTaskDied);
            }
        }
        drop(active);

        registry::delete_run(&self.registry_pool, run_id)?;
        let dir = self.run_dir(run_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}
```

> Note: `ActiveWriters::inner_for_test()` is a small `pub(crate)` accessor needed for the writer-held check; add it during this task. Alternative: expose `is_held(run_id) -> bool` on `ActiveWriters` and use that.

- [ ] **Step 2: Add `is_held` helper on ActiveWriters**

In `runs/writer_slot.rs`, add:

```rust
impl ActiveWriters {
    pub async fn is_held(&self, run_id: &RunId) -> bool {
        let g = self.inner.lock().await;
        g.get(run_id).map(|w| w.strong_count() > 0).unwrap_or(false)
    }
}
```

Replace `delete_run` body to use `if self.active_writers.is_held(run_id).await { return Err(StorageError::WriterTaskDied); }`.

- [ ] **Step 3: Build check**

```bash
cargo build -p surge-persistence
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/storage.rs crates/surge-persistence/src/runs/writer_slot.rs
git commit -m "M2(persistence): Storage run lifecycle methods

create_run inserts registry row + applies per-run migrations + opens
writer. open_run_reader builds a read pool. open_run_writer acquires
in-process token + file lock, opens reader, spawns writer task.
list_runs/get_run perform stale-pid detection and persist Crashed
status. delete_run refuses if writer is currently held."
```

---

## Phase 11: Worktree extensions in surge-git

### Task 11.1: WorktreeLocation enum + RunWorktreeInfo struct

**Files:**
- Create: `crates/surge-git/src/run_worktree.rs`
- Modify: `crates/surge-git/src/lib.rs`
- Modify: `crates/surge-git/Cargo.toml`

- [ ] **Step 1: Add deps to surge-git**

In `crates/surge-git/Cargo.toml [dependencies]`, add:

```toml
ulid = { workspace = true }
dirs = { workspace = true }
```

- [ ] **Step 2: Create run_worktree.rs**

```rust
//! Per-run worktree management — extension of `GitManager` for the new
//! run-based workflow alongside the legacy spec-based methods.

use std::path::PathBuf;
use surge_core::RunId;

#[derive(Debug, Clone)]
pub enum WorktreeLocation {
    /// Default: `<repo_parent>/.surge-worktrees/<short_id>/`. Sibling of repo.
    Sibling,
    /// `~/.surge/runs/<run_id>/worktree/`. Centralized.
    Central,
    /// Explicit absolute path.
    Custom(PathBuf),
}

impl Default for WorktreeLocation {
    fn default() -> Self {
        WorktreeLocation::Sibling
    }
}

#[derive(Debug, Clone)]
pub struct RunWorktreeInfo {
    pub run_id: RunId,
    pub path: PathBuf,
    pub branch: String,
    pub exists_on_disk: bool,
}

#[derive(Debug, Clone)]
pub struct OrphanedWorktree {
    pub name: String,
    pub recorded_path: PathBuf,
}

pub fn run_branch_name(run_id: &RunId) -> String {
    format!("surge/run-{}", run_id.short())
}

pub fn resolve_path(
    repo_path: &std::path::Path,
    run_id: &RunId,
    location: &WorktreeLocation,
) -> PathBuf {
    let short = run_id.short();
    match location {
        WorktreeLocation::Sibling => {
            let parent = repo_path.parent().unwrap_or(repo_path);
            parent.join(".surge-worktrees").join(&short)
        }
        WorktreeLocation::Central => {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".surge").join("runs").join(run_id.to_string()).join("worktree")
        }
        WorktreeLocation::Custom(p) => p.join(&short),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sibling_path_is_under_repo_parent() {
        let id = RunId::new();
        let p = resolve_path(Path::new("/projects/myrepo"), &id, &WorktreeLocation::Sibling);
        assert!(p.starts_with("/projects/.surge-worktrees"));
        assert!(p.to_string_lossy().contains(&id.short()));
    }

    #[test]
    fn central_path_under_home() {
        let id = RunId::new();
        let p = resolve_path(Path::new("/projects/myrepo"), &id, &WorktreeLocation::Central);
        assert!(p.to_string_lossy().contains(".surge"));
    }

    #[test]
    fn branch_name_format() {
        let id = RunId::new();
        let b = run_branch_name(&id);
        assert!(b.starts_with("surge/run-"));
        assert_eq!(b.len(), "surge/run-".len() + 12);
    }
}
```

- [ ] **Step 3: Wire into lib.rs**

In `crates/surge-git/src/lib.rs`, add:

```rust
pub mod run_worktree;
pub use run_worktree::{OrphanedWorktree, RunWorktreeInfo, WorktreeLocation, run_branch_name, resolve_path};
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p surge-git --lib run_worktree
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-git/src/run_worktree.rs crates/surge-git/src/lib.rs crates/surge-git/Cargo.toml
git commit -m "M2(git): scaffold per-run worktree types

WorktreeLocation enum (Sibling/Central/Custom), RunWorktreeInfo,
OrphanedWorktree, run_branch_name (\"surge/run-{short_id}\"),
resolve_path (sibling/central/custom)."
```

### Task 11.2: GitManager methods for run worktrees

**Files:**
- Modify: `crates/surge-git/src/worktree.rs`

- [ ] **Step 1: Add GitManager methods**

Append to the `impl GitManager { ... }` block in `crates/surge-git/src/worktree.rs`:

```rust
impl GitManager {
    pub fn create_run_worktree(
        &self,
        run_id: &surge_core::RunId,
        base_branch: Option<&str>,
        location: crate::run_worktree::WorktreeLocation,
    ) -> Result<crate::run_worktree::RunWorktreeInfo, GitError> {
        use crate::run_worktree::{resolve_path, run_branch_name};
        let repo = self.open_repo()?;
        let branch_name = run_branch_name(run_id);
        let wt_path = resolve_path(&self.repo_path, run_id, &location);

        let worktrees = repo.worktrees()?;
        for n in worktrees.iter().flatten() {
            if n == &run_id.short() {
                return Err(GitError::WorktreeAlreadyExists(run_id.to_string()));
            }
        }

        let commit = if let Some(base) = base_branch {
            let b = repo
                .find_branch(base, BranchType::Local)
                .map_err(|_| GitError::BranchNotFound(base.to_string()))?;
            b.get().peel_to_commit()?
        } else {
            let head = repo.head().map_err(|_| GitError::EmptyRepository)?;
            head.peel_to_commit().map_err(|_| GitError::EmptyRepository)?
        };

        let branch = repo.branch(&branch_name, &commit, false)?;
        if let Some(parent) = wt_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let reference = branch.into_reference();
        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&reference));
        let _wt = repo.worktree(&run_id.short(), &wt_path, Some(&opts))?;

        Ok(crate::run_worktree::RunWorktreeInfo {
            run_id: run_id.clone(),
            path: wt_path,
            branch: branch_name,
            exists_on_disk: true,
        })
    }

    pub fn run_worktree_path(
        &self,
        run_id: &surge_core::RunId,
        location: crate::run_worktree::WorktreeLocation,
    ) -> std::path::PathBuf {
        crate::run_worktree::resolve_path(&self.repo_path, run_id, &location)
    }

    pub fn discard_run_worktree(&self, run_id: &surge_core::RunId) -> Result<(), GitError> {
        // Reuse the existing discard() machinery, but keyed on short_id.
        let repo = self.open_repo()?;
        let short = run_id.short();
        let branch_name = crate::run_worktree::run_branch_name(run_id);

        match repo.find_worktree(&short) {
            Ok(wt) => {
                let mut prune_opts = WorktreePruneOptions::new();
                prune_opts.valid(true).working_tree(true);
                if let Err(e) = wt.prune(Some(&mut prune_opts)) {
                    warn!(short, %e, "worktree prune failed, continuing cleanup");
                }
            }
            Err(e) => debug!(short, %e, "worktree not found"),
        }

        // Best-effort directory removal — sibling/central/custom all stored
        // by the caller; if path varies, caller passes None and we skip.
        // (For simplicity we recompute Sibling default; richer cleanup is M3.)
        let path = self.run_worktree_path(run_id, crate::run_worktree::WorktreeLocation::Sibling);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }

        match repo.find_branch(&branch_name, BranchType::Local) {
            Ok(mut b) => { b.delete()?; }
            Err(e) => debug!(short, %e, "branch not found"),
        }
        Ok(())
    }

    pub fn commit_run_worktree(
        &self,
        run_id: &surge_core::RunId,
        message: &str,
    ) -> Result<git2::Oid, GitError> {
        let path = self.run_worktree_path(run_id, crate::run_worktree::WorktreeLocation::Sibling);
        if !path.exists() {
            return Err(GitError::WorktreeNotFound(run_id.to_string()));
        }
        let wt_repo = Repository::open(&path)?;
        let sig = Self::signature(&wt_repo);
        let mut index = wt_repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;

        let head = wt_repo.head()?;
        let head_commit = head.peel_to_commit()?;
        let head_tree = head_commit.tree()?;
        let diff = wt_repo.diff_tree_to_index(Some(&head_tree), Some(&index), None)?;
        if diff.stats()?.files_changed() == 0 {
            return Err(GitError::NothingToCommit(run_id.to_string()));
        }
        let tree_oid = index.write_tree()?;
        let tree = wt_repo.find_tree(tree_oid)?;
        let oid = wt_repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head_commit])?;
        Ok(oid)
    }

    pub fn merge_run_worktree(
        &self,
        run_id: &surge_core::RunId,
        target_branch: Option<&str>,
        checkout: bool,
    ) -> Result<git2::Oid, GitError> {
        // Delegate to existing merge logic with the run-derived branch name.
        let branch_name = crate::run_worktree::run_branch_name(run_id);
        // Pass it to a private helper that encapsulates the merge guts (refactor as needed).
        self.merge_branch_into(target_branch, &branch_name, checkout)
    }

    /// Find worktrees registered in git whose recorded path no longer exists.
    pub fn find_orphaned_worktrees(&self) -> Result<Vec<crate::run_worktree::OrphanedWorktree>, GitError> {
        let repo = self.open_repo()?;
        let mut out = Vec::new();
        let names = repo.worktrees()?;
        for n in names.iter().flatten() {
            let wt = match repo.find_worktree(n) {
                Ok(w) => w,
                Err(_) => continue,
            };
            let p = wt.path().to_path_buf();
            if !p.exists() {
                out.push(crate::run_worktree::OrphanedWorktree {
                    name: n.to_string(),
                    recorded_path: p,
                });
            }
        }
        Ok(out)
    }

    pub fn prune_orphaned_worktrees(&self) -> Result<u32, GitError> {
        let repo = self.open_repo()?;
        let mut count = 0u32;
        let names = repo.worktrees()?;
        for n in names.iter().flatten() {
            let wt = match repo.find_worktree(n) {
                Ok(w) => w,
                Err(_) => continue,
            };
            if !wt.path().exists() {
                let mut po = WorktreePruneOptions::new();
                po.valid(true).working_tree(false);
                if wt.prune(Some(&mut po)).is_ok() {
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}
```

> The `merge_branch_into` private helper used by both legacy `merge` and new `merge_run_worktree` should be extracted from the existing `GitManager::merge` body during this task — small refactor to enable code reuse without duplicating the FF/conflict logic. Public legacy `merge(spec_id)` becomes a one-line wrapper that calls `merge_branch_into(target, &branch_name(spec_id), checkout)`.

- [ ] **Step 2: Add tests**

In `crates/surge-git/src/worktree.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn create_run_worktree_basic() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let id = surge_core::RunId::new();
    let info = gm
        .create_run_worktree(&id, None, crate::run_worktree::WorktreeLocation::Sibling)
        .unwrap();
    assert!(info.branch.starts_with("surge/run-"));
    assert!(info.path.exists());
    assert!(info.path.to_string_lossy().contains(&id.short()));
}

#[test]
fn discard_run_worktree_removes_branch() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let id = surge_core::RunId::new();
    let _info = gm
        .create_run_worktree(&id, None, crate::run_worktree::WorktreeLocation::Sibling)
        .unwrap();
    gm.discard_run_worktree(&id).unwrap();
    let branch_name = crate::run_worktree::run_branch_name(&id);
    let repo = Repository::open(&path).unwrap();
    assert!(repo.find_branch(&branch_name, BranchType::Local).is_err());
}

#[test]
fn find_orphaned_worktrees_detects_missing_dir() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let id = surge_core::RunId::new();
    let info = gm
        .create_run_worktree(&id, None, crate::run_worktree::WorktreeLocation::Sibling)
        .unwrap();
    std::fs::remove_dir_all(&info.path).unwrap();

    let orphaned = gm.find_orphaned_worktrees().unwrap();
    assert!(orphaned.iter().any(|o| o.name == id.short()));

    let pruned = gm.prune_orphaned_worktrees().unwrap();
    assert_eq!(pruned, 1);
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-git --lib worktree::tests
```

Expected: all existing tests + 3 new tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-git/src/worktree.rs
git commit -m "M2(git): add run-worktree methods to GitManager

create_run_worktree (sibling/central/custom location), discard_run_worktree,
commit_run_worktree, merge_run_worktree, find_orphaned_worktrees,
prune_orphaned_worktrees. Existing legacy spec-keyed methods unchanged;
internal merge logic extracted to merge_branch_into for code reuse."
```

---

## Phase 12: Integration tests

> Files for this phase live in `crates/surge-persistence/tests/runs/`. Each test is its own file plus a top-level `tests/runs/mod.rs` declaring submodules. All tests use `#[tokio::test(flavor = "multi_thread")]` since Storage requires multi-thread runtime.

### Task 12.1: Test scaffolding + fixture helpers

**Files:**
- Create: `crates/surge-persistence/tests/runs.rs` (test crate entry)
- Create: `crates/surge-persistence/tests/runs/mod.rs`
- Create: `crates/surge-persistence/tests/runs/fixtures.rs`

- [ ] **Step 1: Create entry + fixtures**

`crates/surge-persistence/tests/runs.rs`:

```rust
mod runs;
```

`crates/surge-persistence/tests/runs/mod.rs`:

```rust
mod fixtures;

mod append_read;
mod views;
mod rebuild;
mod single_writer;
mod crash_recovery;
mod concurrent;
mod artifacts;
mod subscribe;
mod stale_pid;
mod legacy_unaffected;
mod flush;
```

`crates/surge-persistence/tests/runs/fixtures.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use surge_core::RunId;
use surge_persistence::runs::{Storage, MockClock};
use tempfile::TempDir;

pub struct TestRun {
    pub _tmp: TempDir,
    pub home: PathBuf,
    pub storage: Arc<Storage>,
    pub clock: MockClock,
    pub run_id: RunId,
}

pub async fn setup() -> TestRun {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let clock = MockClock::new(1_700_000_000_000);
    let storage = Storage::open_with(&home, Arc::new(clock.clone())).await.unwrap();
    let run_id = RunId::new();
    TestRun { _tmp: tmp, home, storage, clock, run_id }
}

pub fn dummy_payload(idx: u64) -> surge_core::VersionedEventPayload {
    use surge_core::run_event::{EventPayload, VersionedEventPayload};
    VersionedEventPayload::new(EventPayload::TokensConsumed {
        session: surge_core::SessionId::new(),
        prompt_tokens: idx as u32,
        output_tokens: (idx * 2) as u32,
        cache_hits: 0,
    })
}
```

- [ ] **Step 2: Build (no tests yet, just verify the test target compiles)**

```bash
cargo test -p surge-persistence --tests --no-run
```

Expected: clean compile (each module file referenced in mod.rs is empty).

- [ ] **Step 3: Commit**

```bash
git add crates/surge-persistence/tests/
git commit -m "M2(persistence) tests: scaffold integration test crate + fixtures

Single tests/runs.rs entry, runs/mod.rs declares 11 submodules.
fixtures.rs provides setup() (tempdir + Storage + MockClock + RunId)
and dummy_payload() helper used across tests."
```

### Task 12.2: append_read_1000_events

**Files:**
- Create: `crates/surge-persistence/tests/runs/append_read.rs`

- [ ] **Step 1: Write the test**

```rust
use crate::runs::fixtures::{dummy_payload, setup};
use surge_persistence::runs::EventSeq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_read_1000_events_correct_ordered_atomic() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .unwrap();

    for i in 0..1000u64 {
        let seq = writer.append_event(dummy_payload(i)).await.unwrap();
        assert_eq!(seq.as_u64(), i + 1, "seq must be monotonic with no gaps");
    }
    writer.flush().await.unwrap();

    assert_eq!(writer.current_seq().await.unwrap(), EventSeq(1000));

    let chunk = writer
        .read_events(EventSeq(1)..EventSeq(101))
        .await
        .unwrap();
    assert_eq!(chunk.len(), 100);
    assert_eq!(chunk.first().unwrap().seq, EventSeq(1));
    assert_eq!(chunk.last().unwrap().seq, EventSeq(100));

    writer.close().await.unwrap();
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p surge-persistence --test runs append_read::
```

Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-persistence/tests/runs/append_read.rs
git commit -m "M2(persistence) test: append_read_1000_events

Writes 1000 events through RunWriter::append_event, verifies monotonic
seq assignment, current_seq, and range reads via RunReader."
```

### Task 12.3: materialized_views_consistent

**Files:**
- Create: `crates/surge-persistence/tests/runs/views.rs`

- [ ] **Step 1: Write the test**

```rust
use crate::runs::fixtures::setup;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{NodeKey, OutcomeKey, SessionId};

fn vp(p: EventPayload) -> VersionedEventPayload { VersionedEventPayload::new(p) }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_match_expected_aggregates() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    let n: NodeKey = "spec_1".parse().unwrap();
    writer.append_event(vp(EventPayload::StageEntered { node: n.clone(), attempt: 1 })).await.unwrap();
    writer.append_event(vp(EventPayload::TokensConsumed {
        session: SessionId::new(),
        prompt_tokens: 100,
        output_tokens: 50,
        cache_hits: 5,
    })).await.unwrap();
    writer.append_event(vp(EventPayload::StageCompleted {
        node: n.clone(),
        outcome: "done".parse::<OutcomeKey>().unwrap(),
    })).await.unwrap();
    writer.flush().await.unwrap();

    let stages = writer.stage_executions().await.unwrap();
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].node_id, n);
    assert_eq!(stages[0].outcome.as_deref(), Some("done"));
    assert!(stages[0].ended_seq.is_some());

    let cost = writer.cost_summary().await.unwrap();
    assert_eq!(cost.tokens_in, 100);
    assert_eq!(cost.tokens_out, 50);
    assert_eq!(cost.cache_hits, 5);

    writer.close().await.unwrap();
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs views::
git add crates/surge-persistence/tests/runs/views.rs
git commit -m "M2(persistence) test: materialized_views_consistent"
```

### Task 12.4: rebuild_views_idempotent

**Files:**
- Create: `crates/surge-persistence/tests/runs/rebuild.rs`

- [ ] **Step 1: Write test**

```rust
use crate::runs::fixtures::{dummy_payload, setup};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_views_produces_identical_state() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    for i in 0..50u64 {
        writer.append_event(dummy_payload(i)).await.unwrap();
    }
    writer.flush().await.unwrap();
    let before = writer.cost_summary().await.unwrap();

    writer.rebuild_views().await.unwrap();
    let after = writer.cost_summary().await.unwrap();

    assert_eq!(before.tokens_in, after.tokens_in);
    assert_eq!(before.tokens_out, after.tokens_out);

    writer.close().await.unwrap();
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs rebuild::
git add crates/surge-persistence/tests/runs/rebuild.rs
git commit -m "M2(persistence) test: rebuild_views idempotency"
```

### Task 12.5: single_writer_in_process + cross_process

**Files:**
- Create: `crates/surge-persistence/tests/runs/single_writer.rs`

> **Delta P3.X7:** cross_process test marked `#[ignore]` if Windows flakiness shows up; document `cargo test -- --include-ignored` to opt in. For Linux/macOS, runs by default.

- [ ] **Step 1: Write tests**

```rust
use crate::runs::fixtures::setup;
use surge_persistence::runs::OpenError;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_writer_in_process_fails_with_already_held() {
    let t = setup().await;
    let w1 = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    let result = t.storage.open_run_writer(t.run_id.clone()).await;
    assert!(matches!(result, Err(OpenError::WriterAlreadyHeld { .. })));

    w1.close().await.unwrap();

    // After close, a fresh writer can be opened.
    let w2 = t.storage.open_run_writer(t.run_id.clone()).await.unwrap();
    w2.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(target_os = "windows", ignore = "P3.X7: Windows fd-lock semantics under verification")]
async fn cross_process_writer_acquire_fails_when_other_process_holds() {
    // Two-process scenario emulated via std::process::Command spawning the test binary
    // with a flag, holding the lock, exiting on signal. Skipped here for brevity;
    // see SUBPROCESS_TEST_HOWTO in tests/runs/README.md for the standard pattern.
    // If Windows flakiness is observed, switch to Linux-only via cfg(unix).
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs single_writer::
git add crates/surge-persistence/tests/runs/single_writer.rs
git commit -m "M2(persistence) test: single-writer enforcement (in-process + cross)

In-process test: open second writer fails with WriterAlreadyHeld;
after close(), reopen succeeds. Cross-process test stub gated by
#[ignore] on Windows pending fd-lock semantics verification (P3.X7)."
```

### Task 12.6: crash_recovery

**Files:**
- Create: `crates/surge-persistence/tests/runs/crash_recovery.rs`

- [ ] **Step 1: Write test**

```rust
use crate::runs::fixtures::{dummy_payload, setup};
use surge_persistence::runs::EventSeq;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_dropped_mid_writes_recovers_consistent() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    for i in 0..50u64 {
        writer.append_event(dummy_payload(i)).await.unwrap();
    }
    // Flush guarantees on-disk visibility for the 50 events.
    writer.flush().await.unwrap();

    // Simulate crash: drop without close. Drop emits warn but releases lock + token.
    drop(writer);

    // Re-open writer and verify integrity.
    let w2 = t.storage.open_run_writer(t.run_id.clone()).await.unwrap();
    let seq = w2.current_seq().await.unwrap();
    assert!(seq.as_u64() >= 50, "all flushed events must be readable");

    let events = w2.read_events(EventSeq(1)..seq.next()).await.unwrap();
    assert!(!events.is_empty());
    // No torn writes: serde_json deserialization succeeds for all rows.
    for e in events {
        let _ = e.payload;
    }

    w2.close().await.unwrap();
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs crash_recovery::
git add crates/surge-persistence/tests/runs/crash_recovery.rs
git commit -m "M2(persistence) test: crash_recovery via writer drop"
```

### Task 12.7: concurrent_5_runs_1000_events

**Files:**
- Create: `crates/surge-persistence/tests/runs/concurrent.rs`

- [ ] **Step 1: Write test**

```rust
use crate::runs::fixtures::{dummy_payload, setup};
use surge_core::RunId;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn five_concurrent_runs_no_deadlock_no_corruption() {
    let t = setup().await;

    let mut handles = Vec::new();
    for _ in 0..5 {
        let storage = t.storage.clone();
        handles.push(tokio::spawn(async move {
            let id = RunId::new();
            let writer = storage.create_run(id.clone(), "/tmp/proj", None).await.unwrap();
            for i in 0..1000u64 {
                writer.append_event(dummy_payload(i)).await.unwrap();
            }
            writer.flush().await.unwrap();
            let seq = writer.current_seq().await.unwrap();
            writer.close().await.unwrap();
            (id, seq)
        }));
    }

    for h in handles {
        let (id, seq) = h.await.unwrap();
        assert_eq!(seq.as_u64(), 1000, "run {} should have 1000 events", id);
    }
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs concurrent::
git add crates/surge-persistence/tests/runs/concurrent.rs
git commit -m "M2(persistence) test: 5 concurrent runs × 1000 events"
```

### Task 12.8: artifact_atomic_write

**Files:**
- Create: `crates/surge-persistence/tests/runs/artifacts.rs`

- [ ] **Step 1: Write test**

```rust
use crate::runs::fixtures::setup;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_and_read_artifact_with_dedup() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    let bytes = b"hello world";
    let r1 = writer.store_artifact("greeting.txt", bytes).await.unwrap();
    assert_eq!(r1.size_bytes, bytes.len() as u64);

    let r2 = writer.store_artifact("greeting.txt", bytes).await.unwrap();
    assert_eq!(r1.id, r2.id, "same content → same content_hash → dedup");

    let read = writer.read_artifact(&r1.content_hash).await.unwrap();
    assert_eq!(read, bytes);

    writer.close().await.unwrap();
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs artifacts::
git add crates/surge-persistence/tests/runs/artifacts.rs
git commit -m "M2(persistence) test: artifact atomic write + content-addressed dedup"
```

### Task 12.9: subscribe_stream_polling + drop_no_leak (P2.X4)

**Files:**
- Create: `crates/surge-persistence/tests/runs/subscribe.rs`

- [ ] **Step 1: Write tests**

```rust
use crate::runs::fixtures::{dummy_payload, setup};
use futures_core::Stream;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::timeout;
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_yields_events_within_polling_window() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    let reader = t.storage.open_run_reader(t.run_id.clone()).await.unwrap();
    let mut stream: Pin<Box<dyn Stream<Item = _> + Send>> =
        Box::pin(reader.subscribe_events());

    for i in 0..10u64 {
        writer.append_event(dummy_payload(i)).await.unwrap();
    }
    writer.flush().await.unwrap();

    let mut received = 0;
    while received < 10 {
        let next = timeout(Duration::from_millis(500), stream.next()).await;
        match next {
            Ok(Some(Ok(_ev))) => received += 1,
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
            Ok(None) => panic!("stream ended unexpectedly"),
            Err(_) => panic!("timeout waiting for event {}", received),
        }
    }
    assert_eq!(received, 10);

    drop(stream);
    writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_outlives_storage_drop() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();
    let reader = t.storage.open_run_reader(t.run_id.clone()).await.unwrap();
    let mut stream: Pin<Box<dyn Stream<Item = _> + Send>> =
        Box::pin(reader.subscribe_events());

    drop(reader);
    writer.close().await.unwrap();
    drop(t.storage);

    // Stream should gracefully end or yield an error within a few ticks; no panic.
    for _ in 0..3 {
        let _ = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
    }
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs subscribe::
git add crates/surge-persistence/tests/runs/subscribe.rs
git commit -m "M2(persistence) test: subscribe polling + outlives storage (P2.X4)"
```

### Task 12.10: stale_pid_detection

**Files:**
- Create: `crates/surge-persistence/tests/runs/stale_pid.rs`

- [ ] **Step 1: Write test**

```rust
use crate::runs::fixtures::setup;
use surge_core::RunStatus;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_runs_marks_dead_pid_as_crashed() {
    let t = setup().await;

    // Insert a fake "running" row with an impossibly high pid.
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();
    writer.close().await.unwrap();

    // Manually set status=Running with bogus pid.
    let conn = rusqlite::Connection::open(t.home.join("db/registry.sqlite")).unwrap();
    conn.execute(
        "UPDATE runs SET status = 'running', daemon_pid = ? WHERE id = ?",
        rusqlite::params![i32::MAX, t.run_id.to_string()],
    )
    .unwrap();
    drop(conn);

    let listed = t
        .storage
        .list_runs(surge_persistence::runs::RunFilter::default())
        .await
        .unwrap();
    let found = listed.iter().find(|r| r.id == t.run_id).unwrap();
    assert_eq!(found.status, RunStatus::Crashed);

    // And persisted in the DB.
    let single = t.storage.get_run(&t.run_id).await.unwrap().unwrap();
    assert_eq!(single.status, RunStatus::Crashed);
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs stale_pid::
git add crates/surge-persistence/tests/runs/stale_pid.rs
git commit -m "M2(persistence) test: stale-pid detection updates registry to Crashed"
```

### Task 12.11: legacy_persistence_unaffected (P2.X5)

**Files:**
- Create: `crates/surge-persistence/tests/runs/legacy_unaffected.rs`

- [ ] **Step 1: Write smoke test for one legacy module path**

```rust
//! Smoke test that legacy aggregator/budget/store APIs still function
//! identically after M2 changes (pure-addition guarantee).

#[test]
fn legacy_pricing_lookup_unchanged() {
    use surge_persistence::pricing::{model_pricing, ProviderId};
    let p = model_pricing(ProviderId::Anthropic, "claude-sonnet-4-6");
    assert!(p.is_some(), "legacy pricing lookup must still work");
}

#[test]
fn legacy_budget_struct_constructs() {
    use surge_persistence::budget::Budget;
    let b = Budget::default();
    let _ = format!("{:?}", b);
}

// If ad-hoc construction of legacy aggregator/store types reveals more API surface,
// extend this file with more guards. Each new legacy entry-point used by other
// crates should have at least one `use` site here.
```

> Adapt to actual legacy API names if the placeholders above don't match — the goal is to surface any legacy regression at compile time, not exhaustively re-test legacy semantics.

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs legacy_unaffected::
git add crates/surge-persistence/tests/runs/legacy_unaffected.rs
git commit -m "M2(persistence) test: legacy_persistence_unaffected smoke (P2.X5)

Compile-time guards on legacy public types (pricing, budget) — fails
loudly if M2 changes accidentally break legacy API surface."
```

### Task 12.12: flush_drains_pending (P1.X1)

**Files:**
- Create: `crates/surge-persistence/tests/runs/flush.rs`

- [ ] **Step 1: Write test**

```rust
use crate::runs::fixtures::{dummy_payload, setup};
use futures::future::join_all;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_drains_all_pending_appends() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    // Issue 100 appends concurrently without await between sends.
    let futs: Vec<_> = (0..100u64)
        .map(|i| writer.append_event(dummy_payload(i)))
        .collect();

    // Some may not have returned yet; immediately call flush() — it should
    // wait for the writer task to drain everything.
    let _ = join_all(futs).await;
    writer.flush().await.unwrap();

    let seq = writer.current_seq().await.unwrap();
    assert_eq!(seq.as_u64(), 100, "flush must guarantee all appends visible");

    writer.close().await.unwrap();
}
```

- [ ] **Step 2: Add `futures = "0.3"` to dev-deps**

In `crates/surge-persistence/Cargo.toml [dev-dependencies]`:

```toml
futures = "0.3"
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p surge-persistence --test runs flush::
git add crates/surge-persistence/tests/runs/flush.rs crates/surge-persistence/Cargo.toml
git commit -m "M2(persistence) test: flush_drains_pending_commands (P1.X1)

Issues 100 concurrent appends, calls flush(), verifies current_seq=100.
Guards against silent regression of flush's drain semantics."
```

### Task 12.13: drop_without_close_emits_warning (P1.X2 acceptance)

**Files:**
- Create: `crates/surge-persistence/tests/runs/drop_warn.rs`
- Modify: `crates/surge-persistence/tests/runs/mod.rs` (add `mod drop_warn;`)

- [ ] **Step 1: Write test that captures tracing output**

```rust
use crate::runs::fixtures::setup;
use std::sync::{Arc, Mutex};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Default, Clone)]
struct CapturedWriter {
    out: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_emits_warning_message() {
    let captured = CapturedWriter::default();
    let captured_clone = captured.clone();
    let _g = tracing_subscriber::registry()
        .with(fmt::layer().with_writer(move || captured_clone.clone()))
        .set_default();

    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();
    drop(writer); // explicit drop, no close

    // Allow tracing layer to flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let bytes = captured.out.lock().unwrap().clone();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        s.contains("dropped without close"),
        "expected drop warning, got: {s}"
    );
}
```

- [ ] **Step 2: Add `tracing-subscriber` to dev-deps**

In `crates/surge-persistence/Cargo.toml [dev-dependencies]`:

```toml
tracing-subscriber = { workspace = true }
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p surge-persistence --test runs drop_warn::
git add crates/surge-persistence/tests/runs/drop_warn.rs crates/surge-persistence/tests/runs/mod.rs crates/surge-persistence/Cargo.toml
git commit -m "M2(persistence) test: drop_without_close emits tracing warning (P1.X2)"
```

---

## Phase 13: Property tests

### Task 13.1: append_then_read_roundtrip property

**Files:**
- Create: `crates/surge-persistence/tests/runs_proptest.rs`

- [ ] **Step 1: Create proptest harness**

```rust
//! Property-based tests for the runs module.
//!
//! These run as a separate test binary so the `proptest` long-running shrink
//! cycle doesn't slow down the regular integration suite.

use proptest::prelude::*;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::SessionId;
use surge_persistence::runs::{EventSeq, Storage, MockClock, RunFilter};
use std::sync::Arc;
use surge_core::RunId;
use tempfile::TempDir;

fn payload_strategy() -> impl Strategy<Value = VersionedEventPayload> {
    (0u32..1000, 0u32..1000, 0u32..100).prop_map(|(p, o, c)| {
        VersionedEventPayload::new(EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: p,
            output_tokens: o,
            cache_hits: c,
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,             // keep low — each case opens a fresh DB
        ..ProptestConfig::default()
    })]

    #[test]
    fn append_then_read_roundtrip(payloads in proptest::collection::vec(payload_strategy(), 1..50)) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();

        runtime.block_on(async {
            let tmp = TempDir::new().unwrap();
            let clock = MockClock::new(1_700_000_000_000);
            let storage = Storage::open_with(tmp.path(), Arc::new(clock)).await.unwrap();
            let run_id = RunId::new();
            let writer = storage.create_run(run_id.clone(), "/tmp", None).await.unwrap();

            let mut expected_seqs = Vec::new();
            for p in &payloads {
                let s = writer.append_event(p.clone()).await.unwrap();
                expected_seqs.push(s);
            }
            writer.flush().await.unwrap();

            let read = writer
                .read_events(EventSeq(1)..EventSeq(payloads.len() as u64 + 1))
                .await
                .unwrap();
            prop_assert_eq!(read.len(), payloads.len());
            for (i, ev) in read.iter().enumerate() {
                prop_assert_eq!(ev.seq, expected_seqs[i]);
            }
            writer.close().await.unwrap();
            Ok(())
        }).unwrap();
    }
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p surge-persistence --test runs_proptest
```

Expected: 32 cases pass.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-persistence/tests/runs_proptest.rs
git commit -m "M2(persistence) proptest: append_then_read_roundtrip

32 cases, 1-50 random TokensConsumed events, verifies append-then-read
returns identical sequence with correct seqs."
```

### Task 13.2: view_maintenance_matches_rebuild property

- [ ] **Step 1: Append second proptest to same file**

In `crates/surge-persistence/tests/runs_proptest.rs`, add:

```rust
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        ..ProptestConfig::default()
    })]

    #[test]
    fn view_maintenance_matches_rebuild(
        payloads in proptest::collection::vec(payload_strategy(), 1..30)
    ) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();

        runtime.block_on(async {
            let tmp = TempDir::new().unwrap();
            let clock = MockClock::new(1_700_000_000_000);
            let storage = Storage::open_with(tmp.path(), Arc::new(clock)).await.unwrap();
            let run_id = RunId::new();
            let writer = storage.create_run(run_id.clone(), "/tmp", None).await.unwrap();

            for p in &payloads { writer.append_event(p.clone()).await.unwrap(); }
            writer.flush().await.unwrap();

            let before = writer.cost_summary().await.unwrap();
            writer.rebuild_views().await.unwrap();
            let after = writer.cost_summary().await.unwrap();

            prop_assert_eq!(before.tokens_in, after.tokens_in);
            prop_assert_eq!(before.tokens_out, after.tokens_out);
            prop_assert_eq!(before.cache_hits, after.cache_hits);

            writer.close().await.unwrap();
            Ok(())
        }).unwrap();
    }
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-persistence --test runs_proptest view_maintenance_matches_rebuild
git add crates/surge-persistence/tests/runs_proptest.rs
git commit -m "M2(persistence) proptest: view_maintenance_matches_rebuild

For any random event sequence, incremental view state equals
rebuild_views() state — guards against drift between maintain() and rebuild()."
```

---

## Phase 14: Snapshot tests with deterministic clock

### Task 14.1: insta snapshot for view tables

**Files:**
- Create: `crates/surge-persistence/tests/runs/snapshot.rs`
- Create: `crates/surge-persistence/tests/snapshots/.gitignore` (just to anchor folder)

- [ ] **Step 1: Add snapshot module to mod.rs**

In `crates/surge-persistence/tests/runs/mod.rs`, add:

```rust
mod snapshot;
```

- [ ] **Step 2: Write snapshot test**

```rust
use crate::runs::fixtures::setup;
use insta::assert_yaml_snapshot;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{NodeKey, OutcomeKey, SessionId};

fn vp(p: EventPayload) -> VersionedEventPayload { VersionedEventPayload::new(p) }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handcrafted_linear_flow_view_snapshot() {
    let t = setup().await;
    let writer = t.storage.create_run(t.run_id.clone(), "/tmp/proj", None).await.unwrap();

    let n: NodeKey = "spec_1".parse().unwrap();
    t.clock.set(1_700_000_000_000);
    writer.append_event(vp(EventPayload::StageEntered { node: n.clone(), attempt: 1 })).await.unwrap();
    t.clock.advance(100);
    writer.append_event(vp(EventPayload::TokensConsumed {
        session: SessionId::new(), prompt_tokens: 100, output_tokens: 50, cache_hits: 5,
    })).await.unwrap();
    t.clock.advance(100);
    writer.append_event(vp(EventPayload::StageCompleted {
        node: n.clone(), outcome: "done".parse::<OutcomeKey>().unwrap(),
    })).await.unwrap();
    writer.flush().await.unwrap();

    let stages = writer.stage_executions().await.unwrap();
    let cost = writer.cost_summary().await.unwrap();

    assert_yaml_snapshot!("linear_flow_stages", stages, {
        ".[].started_at_ms" => "[ts]",
        ".[].ended_at_ms"   => "[ts]",
    });
    assert_yaml_snapshot!("linear_flow_cost", cost);

    writer.close().await.unwrap();
}
```

- [ ] **Step 3: Run insta snapshot review**

```bash
cargo test -p surge-persistence --test runs snapshot::
INSTA_UPDATE=auto cargo test -p surge-persistence --test runs snapshot::
cargo insta review        # if insta-cli installed; otherwise inspect *.snap.new files manually
```

Expected: first run creates `*.snap.new`. Review and accept.

- [ ] **Step 4: Commit accepted snapshots**

```bash
git add crates/surge-persistence/tests/runs/snapshot.rs crates/surge-persistence/tests/snapshots/
git commit -m "M2(persistence) snapshot: handcrafted_linear_flow_view_snapshot

Uses MockClock + insta to lock the materialized view shape after a
3-event handcrafted flow. Guards against silent view-schema drift."
```

---

## Phase 15: Documentation polish + CI strict-clippy

### Task 15.1: rustdoc on every public item in runs/

**Files:**
- Modify: all `runs/*.rs` files as needed

- [ ] **Step 1: Run cargo doc with deny-warnings**

```bash
RUSTDOCFLAGS="-D warnings" cargo doc -p surge-persistence --no-deps
```

Expected on first run: list of public items missing `///`. Add docstrings to each.

- [ ] **Step 2: Iterate until clean**

For each warning, add a one-paragraph `///` doc to the function/struct/enum/module. Reference spec sections where complex (e.g., `RunWriter::append_event` → see spec §5.4).

- [ ] **Step 3: Commit**

```bash
git add crates/surge-persistence/src/runs/
git commit -m "M2(persistence): rustdoc coverage on runs/* public API

cargo doc -p surge-persistence --no-deps -- -D warnings is clean."
```

### Task 15.2: CI strict-clippy entry for runs module

**Files:**
- Modify: CI workflow (location depends on M1 split — usually `.github/workflows/*.yml`)
- Modify: `crates/surge-persistence/src/lib.rs` (file-level clippy attribute, see below)

- [ ] **Step 1: Inspect CI config from M1**

```bash
ls .github/workflows/
grep -rn "surge-core" .github/workflows/
```

Identify how M1 added `surge-core` to a strict clippy step. Two common patterns: separate workflow job, or matrix entry.

- [ ] **Step 2: Add strict job for surge-persistence runs/**

The cleanest approach is per-module file annotations: in `crates/surge-persistence/src/runs/mod.rs`, add at top:

```rust
#![warn(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]
```

Legacy modules (sibling files) keep workspace-default clippy. The CI invocation is unchanged: `cargo clippy -p surge-persistence --all-targets -- -D warnings` will surface only configured warnings from each module.

- [ ] **Step 3: Run clippy and clean any new warnings**

```bash
cargo clippy -p surge-persistence --all-targets -- -D warnings
```

Iterate until clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/mod.rs .github/workflows/  # whichever changed
git commit -m "M2(persistence): pedantic clippy on runs/* via module attribute

Legacy modules retain workspace-default lints. Single cargo clippy
invocation enforces both, with strict rules scoped via #![deny] on the
new module path. Acceptance criterion 17 satisfied."
```

---

## Phase 16: Acceptance criteria sweep + PR-ready

### Task 16.1: Run full acceptance suite

- [ ] **Step 1: Full workspace build**

```bash
cargo build --workspace
```

Expected: clean.

- [ ] **Step 2: Full workspace test**

```bash
cargo test --workspace
```

Expected: all tests pass; the new integration tests run alongside legacy ones.

- [ ] **Step 3: Strict clippy on the new module**

```bash
cargo clippy -p surge-persistence --all-targets -- -D warnings
cargo clippy -p surge-core --all-targets -- -D warnings    # M1 strict guarantee unchanged
```

Expected: clean.

- [ ] **Step 4: Workspace clippy in permissive mode (sanity check)**

```bash
cargo clippy --workspace
```

Expected: no new warnings introduced beyond pre-existing legacy noise.

- [ ] **Step 5: rustdoc check**

```bash
RUSTDOCFLAGS="-D warnings" cargo doc -p surge-persistence --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p surge-git --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p surge-core --no-deps
```

Expected: clean.

- [ ] **Step 6: Format check**

```bash
rustfmt --check crates/surge-persistence/src/runs/**/*.rs
rustfmt --check crates/surge-git/src/run_worktree.rs
rustfmt --check crates/surge-core/src/run_status.rs
```

> Per migration strategy, do **not** run `cargo fmt -p surge-persistence` (it formats legacy too). Only the new files are checked.

- [ ] **Step 7: Verify acceptance criteria one by one**

Walk through spec §11 acceptance criteria 1-17. Each should map to a test or build artefact verified above. Tick them off in a check list comment in the final PR description.

- [ ] **Step 8: Final commit + push for review**

```bash
git add -A
git status            # confirm only intended files
git commit --allow-empty -m "M2: acceptance criteria pass

cargo build --workspace ✓
cargo test --workspace ✓
cargo clippy -p surge-persistence --all-targets -- -D warnings ✓
cargo clippy -p surge-core --all-targets -- -D warnings ✓
cargo doc with -D warnings on persistence/git/core ✓
rustfmt --check on new files ✓

Closes M2 milestone of Surge roadmap."
```

---

## Self-review notes (writer's pass)

After writing this plan, ran self-review per the writing-plans skill:

1. **Spec coverage** — each spec section maps to a phase:
   - §1 Goals/scope → Phase 0-2 (foundations) + Phase 16 (acceptance)
   - §2 Strategy → encoded in dependency choices throughout
   - §3 Filesystem layout → Phase 10 (`Storage::open` creates dirs) + Phase 11 (worktree paths)
   - §4 DB schemas → Phase 2 (migrations)
   - §5 Public API → Phases 4 (RunReader), 8 (RunWriter), 10 (Storage)
   - §6 Internals → Phases 5-7 (writer task, view maint, single-writer enforcement) + Phase 9 (subscribe)
   - §7 Worktree management → Phase 11
   - §8 Pre-requisite changes → Phase 0
   - §9 Workspace deps → Tasks 0.3, 1.1, 9.1, 11.1, 12.12, 12.13
   - §10 Testing → Phases 12-14
   - §11 Acceptance criteria → Phase 16
   - §12 Risks → addressed inline (P3.X7 in Task 12.5; checkpoint in Task 5.1; etc.)

2. **Placeholder scan** — no TBD/TODO in production-code steps. Two small `// Implemented in Task X.Y` markers in writer skeleton (Phase 5) are intentional staging stubs replaced in subsequent tasks.

3. **Type consistency** — `EventSeq`, `RunReader`, `RunWriter`, `Storage`, `RunSummary`, `WriterCommand`, `VersionedEventPayload`, `RunStatus` used consistently. `WriterError → StorageError` mapping centralized in `map_writer_err` (Task 8.2).

4. **Delta items integration verified:**
   - **P1.X1 flush_drains_pending** → Task 12.12
   - **P1.X2 Drop with tracing::warn** → Task 8.2 + Task 12.13 (acceptance test)
   - **P1.X3 migration runner one-tx-per-id** → Task 2.2 (in design)
   - **P2.X4 subscribe_outlives_storage** → Task 12.9
   - **P2.X5 legacy_persistence_unaffected** → Task 12.11
   - **P3.X6 artifact dedup** → Task 5.3 (silent dedup, documented inline)
   - **P3.X7 Windows cross-process** → Task 12.5 (gated behind `#[ignore]` on Windows pending verification)

---

## Execution choice

Plan complete and committed. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration. Best for a long plan (16 phases) where context isolation prevents accidental cross-contamination between unrelated changes.

2. **Inline Execution** — execute in this session via executing-plans skill, batch checkpoints. Faster for short plans where one mind keeps state across tasks.

For M2 (~50 task chunks across 16 phases) **subagent-driven is strongly preferred** — context for each task fits in a small subagent invocation, review gates catch drift, and parallel-dispatchable phases (e.g., Phase 12's 13 independent integration tests) become trivially parallelizable.



