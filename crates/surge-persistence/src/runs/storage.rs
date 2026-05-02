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
    pub async fn open_with(
        home: impl AsRef<Path>,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<Self>, OpenError> {
        // Multi-thread runtime check — required by writer task design.
        match Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(RuntimeFlavor::MultiThread) => {}
            Ok(_) => return Err(OpenError::SingleThreadedRuntime),
            Err(_) => {
                return Err(OpenError::Config(
                    "Storage::open requires a tokio runtime in scope".into(),
                ));
            }
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
        };
        registry::insert_run(&self.registry_pool, &summary)
            .map_err(|e| OpenError::MigrationFailed(format!("registry insert failed: {e}")))?;

        // Open writer. If this fails, the per-run DB exists but no writer is held —
        // caller can retry open_run_writer or delete_run to clean up.
        self.open_run_writer(run_id).await
    }

    /// Open a read-only handle to an existing run.
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

    /// Open the exclusive writer for an existing run.
    /// Fails with `OpenError::WriterAlreadyHeld` if another writer holds the slot.
    pub async fn open_run_writer(
        self: &Arc<Self>,
        run_id: RunId,
    ) -> Result<RunWriter, OpenError> {
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
    pub async fn list_runs(
        &self,
        filter: RunFilter,
    ) -> Result<Vec<RunSummary>, crate::runs::error::StorageError> {
        let mut runs = registry::list_runs(&self.registry_pool, &filter)?;
        for r in &mut runs {
            if matches!(r.status, RunStatus::Running | RunStatus::Bootstrapping) {
                if let Some(pid) = r.daemon_pid {
                    if !self.process_probe.is_alive(pid) {
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
            }
        }
        Ok(runs)
    }

    /// Get a single run summary, with stale-pid detection.
    pub async fn get_run(
        &self,
        run_id: &RunId,
    ) -> Result<Option<RunSummary>, crate::runs::error::StorageError> {
        let Some(mut summary) = registry::get_run(&self.registry_pool, run_id)? else {
            return Ok(None);
        };
        if matches!(summary.status, RunStatus::Running | RunStatus::Bootstrapping) {
            if let Some(pid) = summary.daemon_pid {
                if !self.process_probe.is_alive(pid) {
                    summary.status = RunStatus::Crashed;
                    summary.ended_at_ms = Some(self.clock.now_ms());
                    let _ = registry::update_status(
                        &self.registry_pool,
                        &summary.id,
                        RunStatus::Crashed,
                        summary.ended_at_ms,
                    );
                }
            }
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
}
