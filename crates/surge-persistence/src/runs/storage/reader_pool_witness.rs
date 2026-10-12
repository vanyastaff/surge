//! Test-support observations of actual reader pool manager destruction.
//!
//! The only strong marker is captured by the actual manager's initializer.
//! A pool clone (including one inside an unpolled SQL stream) keeps it alive.
//! This observes manager ownership, not the later physical SQLite close order.

use std::sync::{Arc, Weak};

use surge_core::RunId;
use tokio::sync::oneshot;

use super::{OpenError, Storage};

pub(super) struct ManagerLifetime(Option<oneshot::Sender<()>>);

impl Drop for ManagerLifetime {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

/// Non-owning observation of a real reader pool manager.
pub struct ReaderPoolWitness {
    owner: Weak<ManagerLifetime>,
    released: oneshot::Receiver<()>,
}

impl ReaderPoolWitness {
    /// Whether the actual manager's initializer still retains its marker.
    #[must_use]
    pub fn is_retained(&self) -> bool {
        self.owner.upgrade().is_some()
    }

    /// Wait for actual manager destruction; the caller supplies its deadline.
    pub async fn released(&mut self) -> Result<(), oneshot::error::RecvError> {
        (&mut self.released).await
    }
}

pub(super) fn new() -> (Arc<ManagerLifetime>, ReaderPoolWitness) {
    let (sender, released) = oneshot::channel();
    let owner = Arc::new(ManagerLifetime(Some(sender)));
    let witness = ReaderPoolWitness {
        owner: Arc::downgrade(&owner),
        released,
    };
    (owner, witness)
}

impl Storage {
    /// Drain test-only observations without retaining any pool or connection.
    ///
    /// # Errors
    /// Returns an error if the observation mutex was poisoned.
    pub fn take_reader_pool_witnesses(
        &self,
        run_id: RunId,
    ) -> Result<Vec<ReaderPoolWitness>, OpenError> {
        let mut entries = self
            .reader_pool_witnesses
            .lock()
            .map_err(|error| OpenError::Pool(format!("reader witness lock: {error}")))?;
        let (selected, remaining): (Vec<_>, Vec<_>) =
            entries.drain(..).partition(|(id, _)| *id == run_id);
        *entries = remaining;
        Ok(selected.into_iter().map(|(_, witness)| witness).collect())
    }
}

/// Non-owning observation of a captured registry namespace's final teardown.
/// This does not observe unrelated readers, writers or other Storage owners.
#[cfg(windows)]
pub struct RegistryPoolWitness {
    owner: Weak<()>,
}

#[cfg(windows)]
impl RegistryPoolWitness {
    /// True until actual SQLite close and all captured native handles settle.
    #[must_use]
    pub fn is_retained(&self) -> bool {
        self.owner.strong_count() != 0
    }
}

#[cfg(windows)]
impl Storage {
    /// Capture a test-only observation without retaining the registry pool.
    /// Drop all Engine/Storage and other owners before waiting for release.
    ///
    /// # Errors
    /// Returns an error if a registry connection cannot be acquired or has no
    /// native namespace owner.
    pub fn registry_pool_witness(&self) -> Result<RegistryPoolWitness, OpenError> {
        let connection = self
            .acquire_registry_conn()
            .map_err(|error| OpenError::Pool(error.to_string()))?;
        let owner = connection.settlement_marker().ok_or_else(|| {
            OpenError::Config("registry connection has no native namespace owner".into())
        })?;
        Ok(RegistryPoolWitness { owner })
    }
}
