//! Top-level Storage facade.
//!
//! Holds the registry pool, active-writers map, config, and clock; produces
//! `RunReader`/`RunWriter` handles.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use surge_core::RunId;
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::roadmap_patches::RoadmapPatchStore;
use crate::runs::clock::{Clock, SystemClock};
use crate::runs::config::{StorageConfig, load_or_default};
use crate::runs::error::OpenError;
use crate::runs::process::ProcessProbe;
use crate::runs::writer_slot::{ActiveWriters, WriterLease};

/// Top-level storage facade. Holds registry pool, active-writers, config.
pub struct Storage {
    pub(crate) home: PathBuf,
    pub(crate) registry_pool: Pool<SqliteConnectionManager>,
    pub(crate) active_writers: Arc<ActiveWriters>,
    pub(crate) config: StorageConfig,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) process_probe: Arc<ProcessProbe>,
}

impl Storage {
    /// Open or create `~/.surge/`, apply registry migrations, load config.
    /// Uses a `SystemClock`.
    pub async fn open(home: impl AsRef<Path>) -> Result<Arc<Self>, OpenError> {
        Self::open_with(home, Arc::new(SystemClock)).await
    }

    /// Open with a caller-supplied clock (used by tests and snapshot fixtures).
    // No `.await` in this body (it's plain sync I/O). Both clippy-clean
    // alternatives change the moment of execution: `fn -> impl Future { ready(..) }`
    // runs the body eagerly at call time instead of at `.await`;
    // `fn -> impl Future { async move { .. } }` trips `manual_async_fn`.
    // Async is part of the contract here (matches sibling constructors and
    // keeps call sites lazy-until-awaited), so the lint's suggestion is
    // declined deliberately rather than worked around.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn open_with(
        home: impl AsRef<Path>,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<Self>, OpenError> {
        surge_process::owner_panic::install_owner_panic_protection();
        // Multi-thread runtime check — required by writer task design.
        match Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(RuntimeFlavor::MultiThread) => {},
            Ok(_) => return Err(OpenError::SingleThreadedRuntime),
            Err(_) => {
                return Err(OpenError::Config(
                    "Storage::open requires a tokio runtime in scope".into(),
                ));
            },
        }

        let home = home.as_ref().to_path_buf();
        std::fs::create_dir_all(home.join("db"))?;
        std::fs::create_dir_all(home.join("runs"))?;

        let config = load_or_default(&home);
        let registry_pool = crate::runs::registry::open_registry_pool(&home, clock.as_ref())?;

        Ok(Arc::new(Self {
            home,
            registry_pool,
            active_writers: Arc::new(ActiveWriters::default()),
            config,
            clock,
            process_probe: Arc::new(ProcessProbe::new()),
        }))
    }

    /// Storage home directory.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Effective storage config.
    #[must_use]
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// Registry database path.
    #[must_use]
    pub fn registry_db_path(&self) -> PathBuf {
        self.home.join("db").join("registry.sqlite")
    }

    /// Registry-level roadmap patch metadata store.
    #[must_use]
    pub fn roadmap_patch_store(&self) -> RoadmapPatchStore {
        RoadmapPatchStore::new(self.registry_pool.clone())
    }

    /// Registry-level task-ledger index store (`surge ready` / `surge ledger`).
    #[must_use]
    pub fn task_ledger_store(&self) -> crate::task_ledger::TaskLedgerStore {
        crate::task_ledger::TaskLedgerStore::new(self.registry_pool.clone())
    }

    /// Durable bootstrap operation journal; execution is owned by the daemon.
    #[must_use]
    pub fn bootstrap_operation_store(
        &self,
    ) -> super::bootstrap_operations::BootstrapOperationStore {
        super::bootstrap_operations::BootstrapOperationStore::new(self.registry_pool.clone())
    }

    /// Mirror a run's folded task-ledger into the cross-run registry index.
    ///
    /// Reads the run's per-run `task_ledger` view (maintained in the append
    /// transaction) and upserts each row into `task_ledger_index`, keyed by
    /// `(run_id, task_id)`. Idempotent — safe to call repeatedly (e.g. at run
    /// completion and again after resume). Returns the number of tasks synced.
    ///
    /// # Errors
    /// Returns [`OpenError`] when the run database cannot be opened, or a
    /// wrapped [`StorageError`] when the view read or index upsert fails.
    pub async fn sync_task_ledger_index(
        self: &Arc<Self>,
        run_id: RunId,
        project_path: &std::path::Path,
    ) -> Result<usize, OpenError> {
        let reader = self.open_run_reader(run_id.clone()).await?;
        let rows = reader
            .task_ledger()
            .await
            .map_err(|e| OpenError::Pool(e.to_string()))?;
        let store = self.task_ledger_store();
        let observed_at_ms = self.clock.now_ms();
        let project_path = project_path.to_path_buf();
        // The upserts are blocking `rusqlite` calls; run them off the async
        // executor (matches the rest of the persistence layer). Best-effort
        // completion-time mirror, so row counts are small.
        let count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
            for row in &rows {
                store
                    .upsert(&crate::task_ledger::TaskLedgerIndexUpsert {
                        run_id: run_id.clone(),
                        task_id: row.task_id.clone(),
                        project_path: project_path.clone(),
                        status: row.status,
                        verified: row.verified,
                        discovered_from: row.discovered_from.clone(),
                        last_authority_node: row.last_authority_node.clone(),
                        updated_seq: row.updated_seq.0,
                        observed_at_ms,
                        accepted_by_human: row.accepted_by_human,
                        requirement_revised: row.requirement_revised,
                    })
                    .map_err(|e| e.to_string())?;
            }
            Ok(rows.len())
        })
        .await
        .map_err(|e| OpenError::Pool(format!("task-ledger index join: {e}")))?
        .map_err(OpenError::Pool)?;
        Ok(count)
    }

    /// Acquire a registry-pool connection. Used by inbox subsystems that
    /// share the registry DB. The caller holds the connection for the
    /// duration of one logical operation; do not hold it across awaits.
    pub fn acquire_registry_conn(
        &self,
    ) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, r2d2::Error> {
        self.registry_pool.get()
    }

    pub(crate) fn run_dir(&self, run_id: &RunId) -> PathBuf {
        self.home.join("runs").join(run_id.to_string())
    }
    pub(crate) fn events_db_path(&self, run_id: &RunId) -> PathBuf {
        self.run_dir(run_id).join("events.sqlite")
    }
    pub(crate) fn lock_path(&self, run_id: &RunId) -> PathBuf {
        self.run_dir(run_id).join("events.sqlite.lock")
    }
    pub(crate) fn artifacts_dir(&self, run_id: &RunId) -> PathBuf {
        self.run_dir(run_id).join("artifacts")
    }
}

use surge_core::RunStatus;

use crate::runs::file_lock::FileLock;
use crate::runs::pragmas::{PER_RUN_PRAGMAS, apply as apply_pragmas};
use crate::runs::reader::RunReader;
use crate::runs::registry::{self, RunFilter, RunSummary};
use crate::runs::registry_exec;
use crate::runs::run_writer::RunWriter;
use crate::runs::writer::{WriterConfig, spawn_writer};

/// Lightweight active-run row for Triage Author's `active_runs` input.
///
/// Returned by [`Storage::snapshot_active_runs`]. Carries only fields
/// useful for dedup hints — full run metadata stays inside `RunSummary`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveRunRow {
    /// The run ID.
    pub run_id: String,
    /// The task ID, populated by Layer 2 engine integration (None for Layer 1).
    pub task_id: Option<String>,
    /// Current run status in the registry's stable form
    /// ([`RunStatus::as_str`]: `"running"` or `"bootstrapping"`).
    pub status: String,
    /// Unix epoch milliseconds of run creation.
    pub started_at_ms: i64,
}

impl Storage {
    /// Create a new run: insert into registry, init per-run DB with migrations,
    /// open the worktree-anchored writer.
    pub async fn create_run(
        self: &Arc<Self>,
        run_id: RunId,
        project_path: impl AsRef<Path>,
        pipeline_template: Option<String>,
    ) -> Result<RunWriter, OpenError> {
        let project_path = project_path.as_ref().to_path_buf();

        // Per-run dirs + migrations FIRST (no registry commit until ready).
        let run_dir = self.run_dir(&run_id);
        let artifacts_dir = self.artifacts_dir(&run_id);
        let events_path = self.events_db_path(&run_id);
        let clock = self.clock.clone();
        tokio::task::spawn_blocking(move || -> Result<(), OpenError> {
            std::fs::create_dir_all(&run_dir)?;
            std::fs::create_dir_all(&artifacts_dir)?;
            let mut conn = rusqlite::Connection::open(&events_path)?;
            apply_pragmas(&conn, PER_RUN_PRAGMAS)?;
            crate::runs::migrations::apply(
                &mut conn,
                crate::runs::migrations::PER_RUN_MIGRATIONS,
                clock.as_ref(),
            )
            .map_err(|e| OpenError::MigrationFailed(e.to_string()))
        })
        .await
        .map_err(|e| OpenError::Pool(format!("run database preparation task failed: {e}")))??;

        // Now commit to registry — past this point we have a usable per-run DB.
        let summary = RunSummary {
            id: run_id.clone(),
            project_path,
            pipeline_template,
            status: RunStatus::Bootstrapping,
            started_at_ms: self.clock.now_ms(),
            ended_at_ms: None,
            daemon_pid: Some(std::process::id() as i32),
            wake_at_ms: None,
        };
        registry_exec::write(&self.registry_pool, move |tx| {
            registry::insert_run_connection(tx, &summary)
        })
        .await
        .map_err(|e| OpenError::MigrationFailed(format!("registry insert failed: {e}")))?;

        // Open writer. If this fails, the per-run DB exists but no writer is held —
        // caller can retry open_run_writer or delete_run to clean up.
        self.open_run_writer(run_id).await
    }

    /// Open a read-only handle to an existing run.
    /// This does not upgrade legacy schemas; opening its exclusive writer applies
    /// supported pending migrations before current materialized-view reads.
    pub async fn open_run_reader(self: &Arc<Self>, run_id: RunId) -> Result<RunReader, OpenError> {
        let events_path = self.events_db_path(&run_id);
        let reader_pool_size = self.config.reader_pool_size;
        // r2d2 `build` blocks until its initial connections open and apply
        // PRAGMAs, which can enter SQLite's busy handler.
        let pool = tokio::task::spawn_blocking(move || {
            if !events_path.exists() {
                return Ok(None);
            }
            let manager = SqliteConnectionManager::file(&events_path)
                .with_init(|c| apply_pragmas(c, PER_RUN_PRAGMAS));
            super::pool::sqlite_pool_builder()
                .max_size(reader_pool_size)
                .build(manager)
                .map(Some)
                .map_err(|e| OpenError::Pool(e.to_string()))
        })
        .await
        .map_err(|e| OpenError::Pool(format!("reader pool task failed: {e}")))??
        .ok_or_else(|| OpenError::RunNotFound(run_id.clone()))?;

        Ok(RunReader {
            run_id: run_id.clone(),
            pool,
            artifacts_dir: Arc::new(self.artifacts_dir(&run_id)),
            worktree_path: Arc::new(self.run_dir(&run_id).join("worktree")),
        })
    }

    /// Open the exclusive writer for an existing run.
    /// Pending per-run migrations apply while both writer ownership guards are held.
    /// Unknown migration IDs are refused before changing the database.
    /// Fails with `OpenError::WriterAlreadyHeld` if another writer holds the slot.
    pub async fn open_run_writer(self: &Arc<Self>, run_id: RunId) -> Result<RunWriter, OpenError> {
        let token = self
            .active_writers
            .try_acquire(run_id.clone())
            .await
            .map_err(|_| OpenError::WriterOwnershipPoisoned)?
            .ok_or_else(|| OpenError::WriterAlreadyHeld {
                run_id: run_id.clone(),
            })?;

        let lock_path = self.lock_path(&run_id);
        let file_lock = FileLock::try_acquire(&lock_path, run_id.clone())?;

        let events_path = self.events_db_path(&run_id);
        if !events_path.exists() {
            return Err(OpenError::RunNotFound(run_id));
        }
        self.upgrade_existing_run(&events_path)?;

        let reader = self.open_run_reader(run_id.clone()).await?;

        let cfg = WriterConfig {
            run_id: run_id.clone(),
            events_db_path: self.events_db_path(&run_id),
            artifacts_dir: self.artifacts_dir(&run_id),
            clock: self.clock.clone(),
            checkpoint_interval_secs: self.config.checkpoint_interval_seconds,
        };

        let lease = Arc::new(WriterLease {
            _token: token,
            _file_lock: file_lock,
        });
        let (writer_tx, writer_join) =
            spawn_writer(cfg, self.config.writer_channel_capacity, lease.clone());

        Ok(RunWriter {
            reader,
            writer_tx,
            writer_join: Some(writer_join),
            _lease: lease,
            closed: false,
        })
    }

    // Called only after acquiring both writer guards, before pools or the writer task.
    fn upgrade_existing_run(&self, events_path: &Path) -> Result<(), OpenError> {
        use crate::runs::migrations::{PER_RUN_MIGRATIONS, apply};

        let mut conn = rusqlite::Connection::open_with_flags(
            events_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let has_migrations: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_migrations')",
            [],
            |row| row.get(0),
        )?;
        if has_migrations {
            let mut statement = conn.prepare("SELECT id FROM _migrations ORDER BY id")?;
            for id in statement.query_map([], |row| row.get::<_, String>(0))? {
                let id = id?;
                if !PER_RUN_MIGRATIONS.iter().any(|(known, _)| *known == id) {
                    return Err(OpenError::MigrationFailed(format!(
                        "run database contains unsupported migration {id}; use a compatible Surge version"
                    )));
                }
            }
        }
        // Validate before WAL configuration: an unsupported schema is never upgraded.
        apply_pragmas(&conn, PER_RUN_PRAGMAS)?;
        apply(&mut conn, PER_RUN_MIGRATIONS, self.clock.as_ref())
            .map_err(|error| OpenError::MigrationFailed(error.to_string()))?;
        tracing::debug!(path = %events_path.display(), "run database schema validated before writer startup");
        Ok(())
    }

    /// Ids of runs whose id ends with `suffix`, newest first, at most `limit`.
    ///
    /// Matched in the database against every run (not a scanned window). An
    /// empty `suffix` matches nothing.
    ///
    /// # Errors
    /// Returns [`crate::runs::error::StorageError`] if the registry cannot be read.
    pub async fn find_run_ids_by_suffix(
        &self,
        suffix: &str,
        limit: usize,
    ) -> Result<Vec<RunId>, crate::runs::error::StorageError> {
        let suffix = suffix.to_owned();
        registry_exec::read(&self.registry_pool, move |conn| {
            registry::find_ids_by_suffix_connection(conn, &suffix, limit)
        })
        .await
    }

    /// List runs matching the filter, with stale-pid detection.
    ///
    /// **`Parked` is deliberately never rewritten here** (Task 12 M2): the
    /// stale-pid probe below is an allowlist (`Running | Bootstrapping`),
    /// not a `!= Parked` blocklist, so a future status added to that
    /// `matches!` is the only way to sweep `Parked` in — it does not
    /// happen by construction. This matters because a parked run has *no*
    /// owning daemon process by design (see
    /// `surge_core::run_event::EventPayload::RunParked`'s doc: parking
    /// happens precisely so the run stops needing one until `wake_at`
    /// passes), so its `daemon_pid` is routinely stale. Rewriting `Parked`
    /// to `Crashed` on a stale pid would misreport a run that is waiting on
    /// purpose as one that failed, and — because [`RunStatus::Crashed`] is
    /// excluded from `registry::due_parked`'s scan by design (that query's
    /// own doc) — it would make the parked run's own wake-up scan stop
    /// finding it, so it would never resume on its own again. Pinned by
    /// `parked_run_with_dead_pid_stays_parked` below; see that test's doc
    /// for why this is a "does not happen by construction" guarantee, not
    /// a red→green fix.
    pub async fn list_runs(
        &self,
        filter: RunFilter,
    ) -> Result<Vec<RunSummary>, crate::runs::error::StorageError> {
        let probe = self.process_probe.clone();
        let clock = self.clock.clone();
        registry_exec::read(&self.registry_pool, move |conn| {
            let mut runs = registry::list_runs_connection(conn, &filter)?;
            for run in &mut runs {
                sweep_stale_pid(conn, &probe, clock.as_ref(), run);
            }
            Ok(runs)
        })
        .await
    }

    /// Snapshot of currently active runs (status Running or Bootstrapping).
    ///
    /// Bounded by `limit` rows. Used by Triage Author to reason about
    /// dedup against in-flight work.
    ///
    /// Layer 1 leaves `task_id` as `None` for all rows because the
    /// `ticket_index` join would require resolving cross-table foreign
    /// keys not yet materialised in this code path. Layer 2's engine
    /// integration will populate it.
    pub async fn snapshot_active_runs(
        &self,
        limit: usize,
    ) -> Result<Vec<ActiveRunRow>, crate::runs::error::StorageError> {
        registry_exec::read(&self.registry_pool, move |conn| {
            snapshot_active_runs_connection(conn, limit)
        })
        .await
    }

    /// Get a single run summary, with stale-pid detection.
    ///
    /// Same allowlisted stale-pid probe as [`Self::list_runs`] — `Parked`
    /// is never rewritten here either; see that method's doc.
    pub async fn get_run(
        &self,
        run_id: &RunId,
    ) -> Result<Option<RunSummary>, crate::runs::error::StorageError> {
        let run_id = run_id.clone();
        let probe = self.process_probe.clone();
        let clock = self.clock.clone();
        registry_exec::read(&self.registry_pool, move |conn| {
            let Some(mut summary) = registry::get_run_connection(conn, &run_id)? else {
                return Ok(None);
            };
            sweep_stale_pid(conn, &probe, clock.as_ref(), &mut summary);
            Ok(Some(summary))
        })
        .await
    }

    /// Delete a run (registry row + per-run dir).
    ///
    /// Refuses if a writer is currently held for this run.
    ///
    /// **Caller precondition**: ensure no `RunReader` is open for this run.
    /// On Windows, open SQLite reader handles will block `remove_dir_all`,
    /// leaving the registry row deleted but the per-run dir partially cleaned.
    /// (M2 has no in-process reader registry analogous to ActiveWriters; M3+
    /// may add one.)
    pub async fn delete_run(
        self: &Arc<Self>,
        run_id: &RunId,
    ) -> Result<(), crate::runs::error::StorageError> {
        if self
            .active_writers
            .is_held(run_id)
            .await
            .map_err(|_| crate::runs::error::StorageError::WriterOwnershipPoisoned)?
        {
            return Err(crate::runs::error::StorageError::WriterStillActive {
                run_id: run_id.clone(),
            });
        }
        let row = run_id.clone();
        registry_exec::write(&self.registry_pool, move |tx| {
            registry::delete_run_connection(tx, &row)
        })
        .await?;
        // Idempotent cleanup of an already-unregistered run; finishing after
        // an abandoned caller is harmless.
        let dir = self.run_dir(run_id);
        tokio::task::spawn_blocking(move || {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| {
            crate::runs::error::StorageError::Pool(format!(
                "run directory removal task failed: {e}"
            ))
        })?
    }

    /// Set the registry status (and optional `ended_at` ms) for a run.
    ///
    /// Used by crash recovery to mark un-resumable runs `Failed` or to
    /// reconcile a run whose event log reached a terminal state that the
    /// registry never recorded. Thin wrapper over
    /// [`registry::update_status`].
    pub async fn set_run_status(
        &self,
        run_id: &RunId,
        status: RunStatus,
        ended_at_ms: Option<i64>,
    ) -> Result<(), crate::runs::error::StorageError> {
        let run_id = run_id.clone();
        registry_exec::write(&self.registry_pool, move |tx| {
            registry::update_status_connection(tx, &run_id, status, ended_at_ms)
        })
        .await
    }

    /// Park a run: transition it to [`RunStatus::Parked`] and record when
    /// it is expected to resume on its own (Task 12, R37/R37.1). Distinct
    /// from [`Self::set_run_status`] the same way [`registry::set_run_parked`]
    /// is distinct from [`registry::update_status`] — parking carries its
    /// own payload (`wake_at`), not a bare status transition. Thin wrapper;
    /// the engine's run task (Task 12 M3) is this method's first caller.
    pub async fn set_run_parked(
        &self,
        run_id: &RunId,
        wake_at_ms: i64,
    ) -> Result<(), crate::runs::error::StorageError> {
        let run_id = run_id.clone();
        registry_exec::write(&self.registry_pool, move |tx| {
            registry::set_run_parked_connection(tx, &run_id, wake_at_ms)
        })
        .await
    }

    /// Record (or replace) the durable rate-limit capacity observation for
    /// one canonical agent-runtime id (Task 12, R34-R38.1). Thin wrapper
    /// over [`crate::runs::capacity::observe`] — see that function's own
    /// doc for the normalization contract the caller must already have
    /// satisfied (this method does not normalize). Added in M3: M2 shipped
    /// the free function but no `Storage`-level door onto it, deliberately
    /// — M3's engine port (`surge_orchestrator::engine::capacity::
    /// CapacityLedger`) is this method's first caller.
    pub async fn observe_capacity(
        &self,
        window: &surge_core::capacity::CapacityWindow,
    ) -> Result<(), crate::runs::error::StorageError> {
        let window = window.clone();
        registry_exec::write(&self.registry_pool, move |tx| {
            crate::runs::capacity::observe_connection(tx, &window)
        })
        .await
    }

    /// Point-read the durable capacity status for one canonical
    /// agent-runtime id. Never writes. Thin wrapper over
    /// [`crate::runs::capacity::status`] — see [`Self::observe_capacity`]'s
    /// doc for why this door did not exist before M3.
    pub async fn capacity_status(
        &self,
        runtime: &str,
    ) -> Result<surge_core::capacity::CapacityStatus, crate::runs::error::StorageError> {
        let runtime = runtime.to_owned();
        registry_exec::read(&self.registry_pool, move |conn| {
            crate::runs::capacity::status_connection(conn, &runtime)
        })
        .await
    }

    /// Clear any durable exhaustion record for one canonical agent-runtime
    /// id (Task 12 M3 review, BLOCKING #1). Thin wrapper over
    /// [`crate::runs::capacity::clear`] — see that function's own doc for
    /// why a `runtime_capacity` row must not persist forever.
    ///
    /// Absent rows are detected with a WAL read, which never waits on the
    /// registry write lock, so the common no-exhaustion case cannot stall a
    /// stage behind registry contention.
    pub async fn clear_capacity(
        &self,
        runtime: &str,
    ) -> Result<(), crate::runs::error::StorageError> {
        let probe = runtime.to_owned();
        let present = registry_exec::read(&self.registry_pool, move |conn| {
            crate::runs::capacity::exists_connection(conn, &probe)
        })
        .await?;
        if !present {
            return Ok(());
        }
        let runtime = runtime.to_owned();
        registry_exec::write(&self.registry_pool, move |tx| {
            crate::runs::capacity::clear_connection(tx, &runtime)
        })
        .await
    }

    /// Resume a parked run: transition its status away from
    /// [`RunStatus::Parked`] and clear `wake_at`, atomically (Task 12 M3
    /// review, BLOCKING #3). Thin wrapper over
    /// [`registry::clear_parked`] — see that function's doc for why the
    /// two writes must never observably happen apart.
    pub async fn clear_parked(
        &self,
        run_id: &RunId,
        resumed_status: RunStatus,
    ) -> Result<(), crate::runs::error::StorageError> {
        let run_id = run_id.clone();
        registry_exec::write(&self.registry_pool, move |tx| {
            registry::clear_parked_connection(tx, &run_id, resumed_status)
        })
        .await
    }

    /// Parked runs whose `wake_at` has passed as of `now_ms` (Task 12 M3
    /// review, BLOCKING #2). Thin wrapper over [`registry::due_parked`] —
    /// see that function's own doc for the per-status eligibility
    /// rationale. `surge-daemon::recovery`'s crash-recovery scan is this
    /// method's first caller (a full periodic wake scheduler is Task 12
    /// M4).
    pub async fn due_parked(
        &self,
        now_ms: i64,
    ) -> Result<Vec<RunSummary>, crate::runs::error::StorageError> {
        registry_exec::read(&self.registry_pool, move |conn| {
            registry::due_parked_connection(conn, now_ms)
        })
        .await
    }
}

// SQLite errors propagate via the `From<rusqlite::Error>` impl on
// `StorageError`, surfacing as `StorageError::Sqlite`.
fn snapshot_active_runs_connection(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<ActiveRunRow>, crate::runs::error::StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, status, started_at FROM runs
         WHERE status IN (?1, ?2)
         ORDER BY started_at DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            RunStatus::Running.as_str(),
            RunStatus::Bootstrapping.as_str(),
            limit as i64,
        ],
        |row| {
            let status: String = row.get(1)?;
            // Round-trip through the typed status so the row carries the
            // registry's canonical form and corrupt values surface as errors.
            let status = status.parse::<RunStatus>().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(ActiveRunRow {
                run_id: row.get::<_, String>(0)?,
                task_id: None,
                status: status.as_str().to_owned(),
                started_at_ms: row.get::<_, i64>(2)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Stale-pid detection shared by `list_runs` and `get_run`: an active run
/// whose daemon is gone is reported, and best-effort recorded, as `Crashed`.
/// `Parked` is never swept; see [`Storage::list_runs`].
fn sweep_stale_pid(
    conn: &rusqlite::Connection,
    probe: &ProcessProbe,
    clock: &dyn Clock,
    run: &mut RunSummary,
) {
    if matches!(run.status, RunStatus::Running | RunStatus::Bootstrapping)
        && let Some(pid) = run.daemon_pid
        && !probe.is_alive(pid)
    {
        let ended_at_ms = clock.now_ms();
        run.status = RunStatus::Crashed;
        run.ended_at_ms = Some(ended_at_ms);
        if let Err(error) =
            registry::mark_crashed_if_stale_connection(conn, &run.id, pid, ended_at_ms)
        {
            tracing::debug!(run_id = %run.id, %error, "stale-pid crash rewrite failed");
        }
    }
}

#[cfg(test)]
mod set_run_status_tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test(flavor = "multi_thread")]
    async fn set_run_status_updates_registry() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let run_id = RunId::new();
        let _writer = storage
            .create_run(run_id.clone(), "/proj", None)
            .await
            .unwrap();

        // Freshly created run is Bootstrapping.
        let before = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(before.status, RunStatus::Bootstrapping);

        storage
            .set_run_status(&run_id, RunStatus::Failed, Some(1_700_000_000_500))
            .await
            .unwrap();

        let after = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(after.status, RunStatus::Failed);
        assert_eq!(after.ended_at_ms, Some(1_700_000_000_500));
    }
}

#[cfg(test)]
mod capacity_door_tests {
    use super::*;
    use surge_core::capacity::{CapacityStatus, CapacityWindow};
    use tempfile::tempdir;

    /// Task 12 M3: proves the `Storage`-level doors this milestone adds
    /// (`observe_capacity`/`capacity_status`/`set_run_parked`) actually
    /// delegate to the M2 registry-level functions — a real, non-test
    /// caller for each (the engine's `CapacityLedger` port) is wired
    /// separately in `surge-orchestrator`, but this proves the door itself
    /// works in isolation, at the layer that owns it.
    /// Clearing a runtime with no row must not wait on a held registry
    /// write lock: every successful stage clears, usually with nothing to do.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_capacity_without_row_does_not_wait_on_registry_lock() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let holder = rusqlite::Connection::open(storage.registry_db_path()).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            storage.clear_capacity("claude-acp"),
        )
        .await
        .expect("absent-row clear waited on the registry write lock")
        .unwrap();
        holder.execute_batch("ROLLBACK").unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn observe_capacity_then_capacity_status_round_trips_through_storage() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        assert_eq!(
            storage.capacity_status("claude-acp").await.unwrap(),
            CapacityStatus::NeverObserved
        );

        let observed_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let window = CapacityWindow::observed_429(
            "claude-acp",
            Some(std::time::Duration::from_secs(30)),
            observed_at,
        );
        storage.observe_capacity(&window).await.unwrap();

        let CapacityStatus::Known(got) = storage.capacity_status("claude-acp").await.unwrap()
        else {
            panic!("expected Known after observe_capacity");
        };
        assert_eq!(got.runtime(), "claude-acp");
        assert_eq!(
            got.resets_at(),
            Some(observed_at + chrono::Duration::seconds(30))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_run_parked_transitions_status_and_records_wake_at_ms() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let run_id = RunId::new();
        let _writer = storage
            .create_run(run_id.clone(), "/proj", None)
            .await
            .unwrap();

        storage
            .set_run_parked(&run_id, 1_700_000_100_000)
            .await
            .unwrap();

        let after = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(after.status, RunStatus::Parked);
        assert_eq!(after.wake_at_ms, Some(1_700_000_100_000));
    }
}

#[cfg(test)]
mod snapshot_active_runs_tests {
    use super::*;
    use tempfile::tempdir;

    /// Rows are written by the production path (`registry::insert_run`,
    /// which stores `RunStatus::as_str`), not hand-written status strings,
    /// so the filter is checked against the form the registry actually holds.
    #[tokio::test(flavor = "multi_thread")]
    async fn snapshot_returns_active_runs_only() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let mut expected = Vec::new();
        for (offset, status) in [
            RunStatus::Running,
            RunStatus::Bootstrapping,
            RunStatus::Completed,
            RunStatus::Parked,
            RunStatus::Crashed,
            RunStatus::Running,
        ]
        .into_iter()
        .enumerate()
        {
            let summary = RunSummary {
                id: RunId::new(),
                project_path: "/tmp/proj".into(),
                pipeline_template: None,
                status,
                started_at_ms: 1_700_000_000_000 + offset as i64,
                ended_at_ms: None,
                daemon_pid: None,
                wake_at_ms: None,
            };
            registry::insert_run(&storage.registry_pool, &summary).unwrap();
            if matches!(status, RunStatus::Running | RunStatus::Bootstrapping) {
                expected.push((summary.id.to_string(), status.as_str().to_owned()));
            }
        }
        expected.reverse();

        let snap = storage.snapshot_active_runs(32).await.unwrap();
        let got: Vec<_> = snap
            .iter()
            .map(|row| (row.run_id.clone(), row.status.clone()))
            .collect();
        assert_eq!(got, expected, "active runs only, newest first");

        let limited = storage.snapshot_active_runs(1).await.unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].run_id, expected[0].0);
    }
}

#[cfg(test)]
mod parked_survives_stale_pid_tests {
    use super::*;
    use tempfile::tempdir;

    /// Task 12 M2: pinned as intent, not a red→green fix — the stale-pid
    /// probe in `list_runs`/`get_run` is already an allowlist
    /// (`Running | Bootstrapping`), so `Parked` already falls through
    /// untouched today, the same way `RunStatus::is_terminal()` already
    /// excluded `Parked` before Task 12 M1 added a test for it. This test
    /// exists so a future edit that widens that allowlist (or flips it to a
    /// `!= Parked` blocklist, which a new status added later would slip
    /// past) fails loudly instead of silently starting to reap runs that
    /// are waiting on purpose.
    #[tokio::test(flavor = "multi_thread")]
    async fn parked_run_with_dead_pid_stays_parked() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let run_id = RunId::new();
        let writer = storage
            .create_run(run_id.clone(), "/proj", None)
            .await
            .unwrap();
        writer.close().await.unwrap();

        // A parked run has no owning daemon by design; fake one with a pid
        // that cannot possibly be alive (i32::MAX, same fixture as
        // `stale_pid.rs`'s existing Running/Bootstrapping regression test).
        registry::set_run_parked(&storage.registry_pool, &run_id, 1_700_000_100_000).unwrap();
        {
            let conn = storage.registry_pool.get().unwrap();
            conn.execute(
                "UPDATE runs SET daemon_pid = ? WHERE id = ?",
                rusqlite::params![i32::MAX, run_id.to_string()],
            )
            .unwrap();
        }

        let listed = storage.list_runs(RunFilter::default()).await.unwrap();
        let row = listed.iter().find(|r| r.id == run_id).unwrap();
        assert_eq!(
            row.status,
            RunStatus::Parked,
            "a dead pid must not rewrite a parked run to Crashed"
        );

        let single = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(single.status, RunStatus::Parked);
    }
}

#[cfg(test)]
mod existing_run_upgrade_tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::migrations::{PER_RUN_MIGRATIONS, apply};
    use crate::runs::seq::EventSeq;
    use surge_core::{EventPayload, VersionedEventPayload};

    fn seed_legacy_run(storage: &Storage, run_id: &RunId) {
        std::fs::create_dir_all(storage.run_dir(run_id)).unwrap();
        let mut conn = rusqlite::Connection::open(storage.events_db_path(run_id)).unwrap();
        apply(&mut conn, &PER_RUN_MIGRATIONS[..6], &MockClock::new(100)).unwrap();
        let payload = VersionedEventPayload::new(EventPayload::RunAborted {
            reason: "historical reason".into(),
        });
        conn.execute(
            "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(1,42,'RunAborted',?,?)",
            rusqlite::params![serde_json::to_vec(&payload).unwrap(), payload.schema_version()],
        ).unwrap();
        conn.execute(
            "INSERT INTO stage_executions(node_id,attempt,started_seq,started_at,cost_usd,tokens_in,tokens_out) VALUES('worker',2,1,42,1.25,11,7)",
            [],
        ).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn writer_upgrades_legacy_run_before_current_reads_and_preserves_history() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = RunId::new();
        seed_legacy_run(&storage, &run_id);

        for _ in 0..2 {
            let writer = storage.open_run_writer(run_id.clone()).await.unwrap();
            let reader = writer.reader();
            let stages = reader.stage_executions().await.unwrap();
            assert_eq!(stages.len(), 1);
            let stage = &stages[0];
            assert_eq!(stage.node_id.to_string(), "worker");
            assert_eq!(stage.attempt, 2);
            assert_eq!(stage.started_seq, EventSeq(1));
            assert_eq!(stage.started_at_ms, 42);
            assert_eq!(
                (stage.cost_usd, stage.tokens_in, stage.tokens_out),
                (1.25, 11, 7)
            );
            assert_eq!(stage.known_cost_usd, None);
            assert!(!stage.cost_unknown);
            let event = reader.read_event(EventSeq(1)).await.unwrap().unwrap();
            assert_eq!(event.timestamp_ms, 42);
            assert_eq!(event.kind, "RunAborted");
            assert!(
                matches!(event.payload.payload(), EventPayload::RunAborted { reason } if reason == "historical reason")
            );
            let session = surge_core::SessionId::new();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::SessionOpened {
                    handoff: None,
                    opened: None,
                    node: "worker".parse().unwrap(),
                    session,
                    agent: "legacy-profile".into(),
                    agent_id: None,
                }))
                .await
                .unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run_id)).unwrap();
            let recorded_session: String = conn
                .query_row("SELECT session_id FROM stage_executions", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(recorded_session, session.as_ulid().to_string());
            let applied: usize = conn
                .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
                .unwrap();
            assert_eq!(applied, PER_RUN_MIGRATIONS.len());
            writer.close().await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn writer_missing_run_does_not_create_events_database() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = RunId::new();
        std::fs::create_dir_all(storage.run_dir(&run_id)).unwrap();
        assert!(matches!(
            storage.open_run_writer(run_id.clone()).await,
            Err(OpenError::RunNotFound(_))
        ));
        assert!(!storage.events_db_path(&run_id).exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn writer_refuses_unknown_schema_without_upgrade_and_releases_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = RunId::new();
        seed_legacy_run(&storage, &run_id);
        let conn = rusqlite::Connection::open(storage.events_db_path(&run_id)).unwrap();
        conn.execute(
            "INSERT INTO _migrations(id,applied_at) VALUES('per-run-9999-future',100)",
            [],
        )
        .unwrap();
        let before: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let result = storage.open_run_writer(run_id.clone()).await;
        assert!(
            matches!(result, Err(OpenError::MigrationFailed(message)) if message.contains("per-run-9999-future"))
        );
        let after: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, after);
        let applied: usize = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(applied, 7);
        conn.execute("DELETE FROM _migrations WHERE id='per-run-9999-future'", [])
            .unwrap();
        storage
            .open_run_writer(run_id)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
    }
}
