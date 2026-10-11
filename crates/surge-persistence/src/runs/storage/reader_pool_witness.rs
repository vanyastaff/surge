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
