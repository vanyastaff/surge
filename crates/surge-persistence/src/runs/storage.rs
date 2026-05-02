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
