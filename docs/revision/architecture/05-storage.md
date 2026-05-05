# Architecture 05 · Storage

## Overview

Storage layer responsibilities:
- Persist event logs (per-run SQLite databases)
- Maintain registry database (cross-run queries, profiles, templates)
- Manage artifacts on filesystem (under `~/.vibe/runs/<run_id>/artifacts/`)
- Manage git worktrees (one per run)
- Support concurrent reads (UI tailing live runs) with single writer (daemon)

This document specifies the storage crate's API, persistence patterns, and worktree management. Full schema is in architecture/02-data-model.md.

## Crate API

```rust
pub struct Storage {
    registry_pool: SqlitePool,           // ~/.vibe/db/vibe.sqlite
    runs_dir: PathBuf,                   // ~/.vibe/runs/
    profiles_dir: PathBuf,               // ~/.vibe/profiles/
    templates_dir: PathBuf,              // ~/.vibe/templates/
}

impl Storage {
    pub async fn open(home: &Path) -> Result<Self>;
    
    // Run lifecycle
    pub async fn create_run(&self, run_id: RunId, project_path: &Path, pipeline_template: Option<&str>) -> Result<RunHandle>;
    pub async fn open_run(&self, run_id: &RunId) -> Result<RunHandle>;
    pub async fn list_runs(&self, filter: RunFilter) -> Result<Vec<RunSummary>>;
    pub async fn delete_run(&self, run_id: &RunId) -> Result<()>;
    
    // Profile and template registry
    pub async fn install_profile(&self, source: ProfileSource) -> Result<ProfileRef>;
    pub async fn list_profiles(&self) -> Result<Vec<ProfileRef>>;
    pub async fn load_profile(&self, profile_ref: &ProfileRef) -> Result<Profile>;
    pub async fn install_template(&self, source: TemplateSource) -> Result<TemplateRef>;
    pub async fn list_templates(&self) -> Result<Vec<TemplateRef>>;
    pub async fn load_template(&self, template_ref: &TemplateRef) -> Result<Template>;
    
    // Trust state
    pub async fn is_trusted(&self, target: TrustTarget) -> Result<bool>;
    pub async fn set_trust(&self, target: TrustTarget, trusted: bool) -> Result<()>;
}

pub struct RunHandle {
    pub run_id: RunId,
    pub events_pool: SqlitePool,         // per-run DB
    pub artifacts_dir: PathBuf,
    pub worktree: Worktree,
}

impl RunHandle {
    // Event log
    pub async fn append_event(&self, payload: EventPayload) -> Result<u64>;
    pub async fn read_events(&self, range: Range<u64>) -> Result<Vec<Event>>;
    pub async fn subscribe_events(&self) -> EventStream;
    pub async fn current_seq(&self) -> Result<u64>;
    
    // Artifacts
    pub async fn store_artifact(&self, name: &str, content: &[u8]) -> Result<ArtifactRef>;
    pub async fn read_artifact(&self, artifact_id: &ArtifactId) -> Result<Vec<u8>>;
    pub async fn list_artifacts(&self) -> Result<Vec<ArtifactRef>>;
    
    // Materialized views
    pub async fn pending_approvals(&self) -> Result<Vec<PendingApproval>>;
    pub async fn cost_summary(&self) -> Result<CostSummary>;
    pub async fn stage_executions(&self) -> Result<Vec<StageExecution>>;
    pub async fn graph_state_at(&self, seq: u64) -> Result<RunState>;
}
```

## Event log details

### Append-only invariant

The `events` table is **append-only**. The schema uses `INTEGER PRIMARY KEY` (which is automatically `ROWID`-aliased and monotonically increasing) for `seq`. There is no UPDATE or DELETE issued against this table during normal operation.

Enforcement (defense in depth):
- `BEFORE UPDATE` and `BEFORE DELETE` triggers that raise an error
- Run as low-privilege SQLite operations (no app-level admin)

### Concurrent access pattern

- **Writer**: only the run's daemon process. Uses `INSERT INTO events ...`.
- **Readers**: CLI status, runtime UI, replay, telegram bot. Use `SELECT ...`.

SQLite WAL mode (`journal_mode = WAL`) is essential — readers don't block writers, writers don't block readers.

```rust
const PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",       // tradeoff: fsync less aggressive but faster
    "PRAGMA temp_store = MEMORY",
    "PRAGMA mmap_size = 30000000000",    // memory-mapped I/O for reads
    "PRAGMA cache_size = -32000",        // 32MB cache
    "PRAGMA foreign_keys = ON",
];
```

### Live event subscription

Runtime UI needs to observe new events as they're written. Mechanisms:

**Option A: SQLite update_hook** — register a Rust callback for INSERT events. Limitation: only main connection sees it.

**Option B: Polling** — UI polls `SELECT MAX(seq)` every 100ms. Cheap but introduces latency.

**Option C: File watcher on `-wal` file** — Linux/macOS notify on file change. More complex but real-time.

**Option D: Notify channel via shared memory or unix socket** — daemon notifies subscribers explicitly.

For v1: **Option B (polling at 100ms)** is the simplest and sufficient. UI doesn't need <50ms latency — humans can't perceive it. Future optimization to D if needed.

### Payload serialization

Event payloads are stored as binary BLOB using `bincode`. Reasons:
- Fast (5-10x faster than JSON)
- Compact (smaller storage)
- Type-safe via Rust types

For human inspection (debugging, `vibe show event`), payloads are deserialized and pretty-printed as JSON.

```rust
pub fn payload_to_blob(payload: &EventPayload) -> Result<Vec<u8>> {
    bincode::serialize(payload).map_err(Into::into)
}

pub fn payload_from_blob(blob: &[u8]) -> Result<EventPayload> {
    bincode::deserialize(blob).map_err(Into::into)
}
```

### Schema versioning

Each event row carries `schema_version`. When code is updated and the event payload struct changes, old events must be deserialized using the old schema:

```rust
pub fn deserialize_event(row: &EventRow) -> Result<Event> {
    match row.schema_version {
        1 => bincode::deserialize::<EventPayloadV1>(&row.payload)?.upgrade_to_v2().upgrade_to_current(),
        2 => bincode::deserialize::<EventPayloadV2>(&row.payload)?.upgrade_to_current(),
        CURRENT_VERSION => bincode::deserialize::<EventPayload>(&row.payload)?,
        v => return Err(StorageError::UnsupportedEventSchemaVersion(v)),
    }
}
```

Every breaking change to `EventPayload` requires:
1. Bumping `CURRENT_VERSION`
2. Adding a `EventPayloadVN` historical type
3. Implementing `upgrade_to_*` chain
4. Migration test fixture proving old events still load

## Materialized views

### Maintenance strategy

Two approaches considered:

**Approach A: SQLite triggers** — DDL maintains views via SQL triggers. Pros: works without engine running. Cons: limited expressiveness, hard to debug.

**Approach B: Engine-side maintenance** — engine writes events AND updates views in same transaction. Pros: full Rust logic available. Cons: views become inconsistent if engine has bug.

For v1: **Approach B**. Engine has the type info (knows `EventPayload` shape) needed to compute views correctly. Triggers used only for simple aggregates (cost totals).

```rust
impl RunHandle {
    pub async fn append_event(&self, payload: EventPayload) -> Result<u64> {
        let mut tx = self.events_pool.begin().await?;
        
        // 1. Insert event
        let seq = self.next_seq(&mut tx).await?;
        let blob = payload_to_blob(&payload)?;
        sqlx::query("INSERT INTO events (seq, timestamp, kind, payload, schema_version) VALUES (?, ?, ?, ?, ?)")
            .bind(seq)
            .bind(now_ms())
            .bind(payload.discriminant_str())
            .bind(blob)
            .bind(CURRENT_SCHEMA_VERSION)
            .execute(&mut *tx)
            .await?;
        
        // 2. Update relevant materialized views
        self.maintain_views(&mut tx, seq, &payload).await?;
        
        tx.commit().await?;
        Ok(seq)
    }
    
    async fn maintain_views(&self, tx: &mut Transaction<'_, Sqlite>, seq: u64, payload: &EventPayload) -> Result<()> {
        match payload {
            EventPayload::StageEntered { node, attempt } => {
                sqlx::query("INSERT INTO stage_executions (node_id, attempt, started_seq, started_at) VALUES (?, ?, ?, ?)")
                    .bind(node.as_str()).bind(attempt).bind(seq).bind(now_ms())
                    .execute(&mut **tx).await?;
            }
            EventPayload::StageCompleted { node, outcome } => {
                sqlx::query("UPDATE stage_executions SET ended_seq=?, ended_at=?, outcome=? WHERE node_id=? AND attempt=(SELECT MAX(attempt) FROM stage_executions WHERE node_id=?)")
                    .bind(seq).bind(now_ms()).bind(outcome.as_str())
                    .bind(node.as_str()).bind(node.as_str())
                    .execute(&mut **tx).await?;
            }
            EventPayload::ArtifactProduced { node, artifact_id, path } => {
                let metadata = std::fs::metadata(path)?;
                sqlx::query("INSERT INTO artifacts (id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash) VALUES (?, ?, ?, ?, ?, ?, ?)")
                    .bind(artifact_id.as_str()).bind(node.as_str()).bind(seq)
                    .bind(path.file_name().and_then(|n| n.to_str()))
                    .bind(path.to_str())
                    .bind(metadata.len() as i64)
                    .bind(self.compute_hash(path).await?)
                    .execute(&mut **tx).await?;
            }
            EventPayload::TokensConsumed { prompt_tokens, output_tokens, .. } => {
                sqlx::query("UPDATE cost_summary SET value = value + ? WHERE metric = 'tokens_in'")
                    .bind(*prompt_tokens as i64).execute(&mut **tx).await?;
                sqlx::query("UPDATE cost_summary SET value = value + ? WHERE metric = 'tokens_out'")
                    .bind(*output_tokens as i64).execute(&mut **tx).await?;
            }
            EventPayload::ApprovalRequested { gate, channel, payload_hash } => {
                sqlx::query("INSERT INTO pending_approvals (seq, node_id, channel, requested_at, payload_hash) VALUES (?, ?, ?, ?, ?)")
                    .bind(seq).bind(gate.as_str()).bind(format!("{:?}", channel))
                    .bind(now_ms()).bind(payload_hash)
                    .execute(&mut **tx).await?;
            }
            EventPayload::ApprovalDecided { gate, .. } => {
                sqlx::query("DELETE FROM pending_approvals WHERE node_id = ?")
                    .bind(gate.as_str()).execute(&mut **tx).await?;
            }
            // ... other event types
            _ => {}
        }
        Ok(())
    }
}
```

### Rebuild from events

If a materialized view is corrupted (rare, but possible after bug in maintenance code), it can be rebuilt:

```rust
pub async fn rebuild_views(&self) -> Result<()> {
    let mut tx = self.events_pool.begin().await?;
    
    // Truncate views
    sqlx::query("DELETE FROM stage_executions").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM artifacts").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM pending_approvals").execute(&mut *tx).await?;
    sqlx::query("UPDATE cost_summary SET value = 0").execute(&mut *tx).await?;
    
    // Replay all events
    let events: Vec<EventRow> = sqlx::query_as("SELECT * FROM events ORDER BY seq")
        .fetch_all(&mut *tx).await?;
    
    for row in events {
        let event = deserialize_event(&row)?;
        self.maintain_views(&mut tx, event.seq, &event.payload).await?;
    }
    
    tx.commit().await?;
    Ok(())
}
```

This is exposed via `vibe doctor --rebuild-views <run_id>`.

## Artifact storage

Artifacts (description.md, spec.md, source files modified, etc.) are stored on filesystem, not in the database.

### Layout

```
~/.vibe/runs/<run_id>/artifacts/
├── 01-bootstrap/
│   ├── description.md
│   ├── roadmap.md
│   └── flow.toml
├── 02-stages/
│   ├── spec_1/
│   │   └── spec.md
│   ├── plan_1/
│   │   ├── plan.md
│   │   └── adr-001.md
│   └── ...
└── final/
    └── pr-description.md
```

### Content addressing

Each artifact has an `ArtifactId` based on SHA-256 of its content. This enables:
- Deduplication if same content appears multiple times
- Integrity verification (corruption detection)
- Reference from event log without embedding content

```rust
pub fn compute_artifact_id(content: &[u8]) -> ArtifactId {
    let hash = sha256(content);
    ArtifactId::from(format!("sha256:{}", hex(hash)))
}
```

### Storage operation

```rust
pub async fn store_artifact(&self, name: &str, content: &[u8]) -> Result<ArtifactRef> {
    let id = compute_artifact_id(content);
    let target_path = self.artifacts_dir.join(name);
    
    // Atomic write: write to temp, rename
    let tmp_path = target_path.with_extension("tmp");
    fs::write(&tmp_path, content).await?;
    fs::rename(&tmp_path, &target_path).await?;
    
    Ok(ArtifactRef {
        id,
        path: target_path,
        name: name.to_string(),
    })
}
```

### Worktree integration

Source files modified by agents go into the run's worktree, not into `artifacts/`. The `artifacts/` directory is for engine-produced metadata (specs, plans, descriptions) that isn't part of the codebase.

## Worktree management

Each run gets its own git worktree branch.

```rust
pub struct Worktree {
    pub root: PathBuf,           // path to worktree dir
    pub branch: String,          // branch name (e.g., "vibe/run-abc123")
    pub source_repo: PathBuf,    // original repo
}

impl Worktree {
    pub async fn create(source_repo: &Path, run_id: &RunId) -> Result<Self> {
        let short_id = run_id.short();
        let branch = format!("vibe/run-{}", short_id);
        let target = source_repo.parent().unwrap()
            .join(format!(".vibe-worktrees/{}", short_id));
        
        // git worktree add <target> -b <branch> HEAD
        Command::new("git").args(&["-C", source_repo.to_str().unwrap(),
            "worktree", "add", target.to_str().unwrap(),
            "-b", &branch, "HEAD"
        ]).status().await?.success_or(...)?;
        
        Ok(Self { root: target, branch, source_repo: source_repo.into() })
    }
    
    pub async fn cleanup(&self) -> Result<()> {
        // git worktree remove
        Command::new("git").args(&["-C", self.source_repo.to_str().unwrap(),
            "worktree", "remove", self.root.to_str().unwrap(), "--force"
        ]).status().await?;
        // branch remains; user can merge or delete it
        Ok(())
    }
    
    pub async fn merge_to(&self, target_branch: &str) -> Result<()> {
        // git merge into target
        Command::new("git").args(&["-C", self.source_repo.to_str().unwrap(),
            "checkout", target_branch
        ]).status().await?;
        Command::new("git").args(&["-C", self.source_repo.to_str().unwrap(),
            "merge", "--no-ff", &self.branch
        ]).status().await?;
        Ok(())
    }
}
```

### Worktree placement

By default: `<repo_parent>/.vibe-worktrees/<short_id>`. Reasons:
- Outside the source repo, so it's not visible to `git status` of source
- Sibling to source, so file permissions are typically the same
- Hidden directory

User can override via `~/.vibe/config.toml`:

```toml
[worktrees]
location = "auto"                    # auto | "<absolute_path>"
```

### Cleanup policy

After a run terminates:
- Worktree's filesystem is preserved by default (user may want to inspect)
- Branch is preserved
- After `prune_after_days = 30` (configurable), engine offers to clean up

`vibe gc` command runs cleanup explicitly.

## Backups and exports

### Run export

```bash
vibe export <run_id> [--output <file.tar.gz>]
```

Creates a tar.gz containing:
- Event log SQLite file
- All artifacts
- Run config TOML
- Excludes worktree (too large; user can re-clone if needed)

### Run import

```bash
vibe import <file.tar.gz>
```

Imports a previously exported run. Useful for:
- Sharing reproducible test cases
- Backing up critical runs
- Moving runs between machines

### Backup strategy

For backup, recommended approach:
- `~/.vibe/profiles/`, `~/.vibe/templates/` — user-edited content; back up
- `~/.vibe/db/vibe.sqlite` — registry; back up
- `~/.vibe/runs/` — selectively back up specific runs via `vibe export`

A `vibe backup` command (future) automates this into a single tarball.

## Performance budgets

For a typical 30-minute run:
- Events: 500-2000 (mostly tool calls, artifacts, costs)
- Event log size: 1-5 MB
- Artifacts: 100KB-10MB depending on what's produced
- Worktree: depends on project, often dominates

For 100 concurrent runs (extreme case):
- DB connections: 100 (one per daemon, one per UI client)
- SQLite handles this fine in WAL mode
- Memory: ~50MB per daemon

For long-term storage (1000 runs):
- Total: ~5GB (mostly worktrees)
- Mitigation: aggressive cleanup, archive policy

## Acceptance criteria

The storage layer is correctly implemented when:

1. Append + read of 1000 events to a run is correct, ordered, and atomic.
2. Materialized views remain consistent with event log under concurrent reads.
3. WAL mode allows the runtime UI to read events while the daemon writes events without blocking.
4. Artifact storage is atomic: a partially written artifact is never observable.
5. Worktree creation, cleanup, and merge work correctly across Linux, macOS, Windows.
6. Run export and import produce equivalent runs (event log + artifacts).
7. Schema migration: an event log from schema_version=1 successfully reads under schema_version=2 code.
8. Materialized view rebuild from events produces identical view state to incremental maintenance.
9. SQLite databases survive `kill -9` of writer with no corruption (WAL recovery on next open).
10. End-to-end: 5 concurrent runs each writing 1000 events finish without deadlock or corruption.
