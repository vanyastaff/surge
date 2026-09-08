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
use crate::runs::writer_slot::ActiveWriters;

/// Top-level storage facade. Holds registry pool, active-writers, config.
pub struct Storage {
    pub(crate) home: PathBuf,
    pub(crate) registry_pool: Pool<SqliteConnectionManager>,
    pub(crate) active_writers: ActiveWriters,
    pub(crate) config: StorageConfig,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) process_probe: ProcessProbe,
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
            active_writers: ActiveWriters::default(),
            config,
            clock,
            process_probe: ProcessProbe::new(),
        }))
    }

    /// Storage home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Effective storage config.
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// Registry database path.
    pub fn registry_db_path(&self) -> PathBuf {
        self.home.join("db").join("registry.sqlite")
    }

    /// Registry-level roadmap patch metadata store.
    pub fn roadmap_patch_store(&self) -> RoadmapPatchStore {
        RoadmapPatchStore::new(self.registry_pool.clone())
    }

    /// Registry-level task-ledger index store (`surge ready` / `surge ledger`).
    pub fn task_ledger_store(&self) -> crate::task_ledger::TaskLedgerStore {
        crate::task_ledger::TaskLedgerStore::new(self.registry_pool.clone())
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
    /// Current run status as a string (e.g., "Running", "Bootstrapping").
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
        registry::insert_run(&self.registry_pool, &summary)
            .map_err(|e| OpenError::MigrationFailed(format!("registry insert failed: {e}")))?;

        // Open writer. If this fails, the per-run DB exists but no writer is held —
        // caller can retry open_run_writer or delete_run to clean up.
        self.open_run_writer(run_id).await
    }

    /// Open a read-only handle to an existing run.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn open_run_reader(self: &Arc<Self>, run_id: RunId) -> Result<RunReader, OpenError> {
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

    /// Open the exclusive writer for an existing run.
    /// Fails with `OpenError::WriterAlreadyHeld` if another writer holds the slot.
    pub async fn open_run_writer(self: &Arc<Self>, run_id: RunId) -> Result<RunWriter, OpenError> {
        let token = self
            .active_writers
            .try_acquire(run_id.clone())
            .await
            .ok_or_else(|| OpenError::WriterAlreadyHeld {
                run_id: run_id.clone(),
            })?;

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
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn list_runs(
        &self,
        filter: RunFilter,
    ) -> Result<Vec<RunSummary>, crate::runs::error::StorageError> {
        let mut runs = registry::list_runs(&self.registry_pool, &filter)?;
        for r in &mut runs {
            if matches!(r.status, RunStatus::Running | RunStatus::Bootstrapping)
                && let Some(pid) = r.daemon_pid
                && !self.process_probe.is_alive(pid)
            {
                r.status = RunStatus::Crashed;
                r.ended_at_ms = Some(self.clock.now_ms());
                let _ = registry::update_status(
                    &self.registry_pool,
                    &r.id,
                    RunStatus::Crashed,
                    r.ended_at_ms,
                );
            }
        }
        Ok(runs)
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
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn snapshot_active_runs(
        &self,
        limit: usize,
    ) -> Result<Vec<ActiveRunRow>, crate::runs::error::StorageError> {
        let conn = self
            .registry_pool
            .get()
            .map_err(|e| crate::runs::error::StorageError::Pool(e.to_string()))?;
        // SQLite errors propagate via the `From<rusqlite::Error>` impl on
        // `StorageError`, surfacing as `StorageError::Sqlite`. Pool-acquire
        // failures above keep the `StorageError::Pool` mapping.
        let mut stmt = conn.prepare(
            "SELECT id, status, started_at FROM runs
             WHERE status IN ('Running', 'Bootstrapping')
             ORDER BY started_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(ActiveRunRow {
                run_id: row.get::<_, String>(0)?,
                task_id: None,
                status: row.get::<_, String>(1)?,
                started_at_ms: row.get::<_, i64>(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Get a single run summary, with stale-pid detection.
    ///
    /// Same allowlisted stale-pid probe as [`Self::list_runs`] — `Parked`
    /// is never rewritten here either; see that method's doc.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn get_run(
        &self,
        run_id: &RunId,
    ) -> Result<Option<RunSummary>, crate::runs::error::StorageError> {
        let Some(mut summary) = registry::get_run(&self.registry_pool, run_id)? else {
            return Ok(None);
        };
        if matches!(
            summary.status,
            RunStatus::Running | RunStatus::Bootstrapping
        ) && let Some(pid) = summary.daemon_pid
            && !self.process_probe.is_alive(pid)
        {
            summary.status = RunStatus::Crashed;
            summary.ended_at_ms = Some(self.clock.now_ms());
            let _ = registry::update_status(
                &self.registry_pool,
                &summary.id,
                RunStatus::Crashed,
                summary.ended_at_ms,
            );
        }
        Ok(Some(summary))
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
        if self.active_writers.is_held(run_id).await {
            return Err(crate::runs::error::StorageError::WriterStillActive {
                run_id: run_id.clone(),
            });
        }
        registry::delete_run(&self.registry_pool, run_id)?;
        let dir = self.run_dir(run_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Set the registry status (and optional `ended_at` ms) for a run.
    ///
    /// Used by crash recovery to mark un-resumable runs `Failed` or to
    /// reconcile a run whose event log reached a terminal state that the
    /// registry never recorded. Thin wrapper over
    /// [`registry::update_status`].
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn set_run_status(
        &self,
        run_id: &RunId,
        status: RunStatus,
        ended_at_ms: Option<i64>,
    ) -> Result<(), crate::runs::error::StorageError> {
        registry::update_status(&self.registry_pool, run_id, status, ended_at_ms)
    }

    /// Park a run: transition it to [`RunStatus::Parked`] and record when
    /// it is expected to resume on its own (Task 12, R37/R37.1). Distinct
    /// from [`Self::set_run_status`] the same way [`registry::set_run_parked`]
    /// is distinct from [`registry::update_status`] — parking carries its
    /// own payload (`wake_at`), not a bare status transition. Thin wrapper;
    /// the engine's run task (Task 12 M3) is this method's first caller.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn set_run_parked(
        &self,
        run_id: &RunId,
        wake_at_ms: i64,
    ) -> Result<(), crate::runs::error::StorageError> {
        registry::set_run_parked(&self.registry_pool, run_id, wake_at_ms)
    }

    /// Record (or replace) the durable rate-limit capacity observation for
    /// one canonical agent-runtime id (Task 12, R34-R38.1). Thin wrapper
    /// over [`crate::runs::capacity::observe`] — see that function's own
    /// doc for the normalization contract the caller must already have
    /// satisfied (this method does not normalize). Added in M3: M2 shipped
    /// the free function but no `Storage`-level door onto it, deliberately
    /// — M3's engine port (`surge_orchestrator::engine::capacity::
    /// CapacityLedger`) is this method's first caller.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn observe_capacity(
        &self,
        window: &surge_core::capacity::CapacityWindow,
    ) -> Result<(), crate::runs::error::StorageError> {
        crate::runs::capacity::observe(&self.registry_pool, window)
    }

    /// Point-read the durable capacity status for one canonical
    /// agent-runtime id. Never writes. Thin wrapper over
    /// [`crate::runs::capacity::status`] — see [`Self::observe_capacity`]'s
    /// doc for why this door did not exist before M3.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn capacity_status(
        &self,
        runtime: &str,
    ) -> Result<surge_core::capacity::CapacityStatus, crate::runs::error::StorageError> {
        crate::runs::capacity::status(&self.registry_pool, runtime)
    }

    /// Clear any durable exhaustion record for one canonical agent-runtime
    /// id (Task 12 M3 review, BLOCKING #1). Thin wrapper over
    /// [`crate::runs::capacity::clear`] — see that function's own doc for
    /// why a `runtime_capacity` row must not persist forever.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn clear_capacity(
        &self,
        runtime: &str,
    ) -> Result<(), crate::runs::error::StorageError> {
        crate::runs::capacity::clear(&self.registry_pool, runtime)
    }

    /// Resume a parked run: transition its status away from
    /// [`RunStatus::Parked`] and clear `wake_at`, atomically (Task 12 M3
    /// review, BLOCKING #3). Thin wrapper over
    /// [`registry::clear_parked`] — see that function's doc for why the
    /// two writes must never observably happen apart.
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn clear_parked(
        &self,
        run_id: &RunId,
        resumed_status: RunStatus,
    ) -> Result<(), crate::runs::error::StorageError> {
        registry::clear_parked(&self.registry_pool, run_id, resumed_status)
    }

    /// Parked runs whose `wake_at` has passed as of `now_ms` (Task 12 M3
    /// review, BLOCKING #2). Thin wrapper over [`registry::due_parked`] —
    /// see that function's own doc for the per-status eligibility
    /// rationale. `surge-daemon::recovery`'s crash-recovery scan is this
    /// method's first caller (a full periodic wake scheduler is Task 12
    /// M4).
    // No `.await` in this body — see `open_with` for why it stays `async fn`.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "async is load-bearing laziness here, not trait-impl ceremony: `ready()`/`async move` alternatives both run the body at a different time than `.await`"
    )]
    pub async fn due_parked(
        &self,
        now_ms: i64,
    ) -> Result<Vec<RunSummary>, crate::runs::error::StorageError> {
        registry::due_parked(&self.registry_pool, now_ms)
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

    #[tokio::test(flavor = "multi_thread")]
    async fn snapshot_returns_active_runs_only() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        // Insert one Running, one Bootstrapping, one Completed.
        let pool = storage.registry_pool.clone();
        let conn = pool.get().unwrap();
        for (id, status) in [
            ("01HXX0000000000000000RUN1", "Running"),
            ("01HXX0000000000000000BTS1", "Bootstrapping"),
            ("01HXX0000000000000000DONE", "Completed"),
        ] {
            conn.execute(
                "INSERT INTO runs (id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid)
                 VALUES (?1, ?2, NULL, ?3, ?4, NULL, NULL)",
                rusqlite::params![
                    id,
                    "/tmp/proj",
                    status,
                    1_700_000_000_000_i64,
                ],
            ).unwrap();
        }
        drop(conn);

        let snap = storage.snapshot_active_runs(32).await.unwrap();
        assert_eq!(snap.len(), 2, "only Running + Bootstrapping should appear");
        assert!(
            snap.iter()
                .all(|r| matches!(r.status.as_str(), "Running" | "Bootstrapping"))
        );
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
