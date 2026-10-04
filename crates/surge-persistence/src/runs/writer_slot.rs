//! In-process tracking of which RunIds currently have a live writer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use super::file_lock::FileLock;
use surge_core::RunId;

/// Poisoned ownership bookkeeping is distinct from a legitimately busy writer.
#[derive(Debug, thiserror::Error)]
#[error("writer ownership bookkeeping is poisoned")]
pub(crate) struct WriterSlotError;

/// Shared actual actor and wrapper lifetime, including cancelled caller futures.
pub(crate) struct WriterLease {
    pub(crate) _token: Arc<WriterToken>,
    pub(crate) _file_lock: FileLock,
}

/// Sentinel object whose Arc lifetime represents an active writer slot.
///
/// The actual actor and caller share one `WriterLease`; its token is indexed
/// weakly in `Storage::active_writers`. Only their final release frees the slot.
pub struct WriterToken;

/// Process-wide registry of which RunIds currently have a live writer.
#[derive(Default)]
pub struct ActiveWriters {
    inner: Mutex<HashMap<RunId, Weak<WriterToken>>>,
}

impl ActiveWriters {
    /// Try to acquire the writer slot for the given RunId.
    ///
    /// Returns `Some(Arc<WriterToken>)` if no live writer holds the slot.
    /// Returns `None` if another live writer is currently holding the slot.
    pub fn try_acquire(
        &self,
        run_id: RunId,
    ) -> impl std::future::Future<Output = Result<Option<Arc<WriterToken>>, WriterSlotError>> {
        let inner = &self.inner;
        async move { Self::acquire_from(inner, run_id) }
    }

    /// Runtime-independent entry sharing precisely the normal writer's token.
    pub(crate) fn try_acquire_sync(
        &self,
        run_id: RunId,
    ) -> Result<Option<Arc<WriterToken>>, WriterSlotError> {
        Self::acquire_from(&self.inner, run_id)
    }

    fn acquire_from(
        inner: &Mutex<HashMap<RunId, Weak<WriterToken>>>,
        run_id: RunId,
    ) -> Result<Option<Arc<WriterToken>>, WriterSlotError> {
        let mut state = inner.lock().map_err(|_| WriterSlotError)?;
        if state
            .get(&run_id)
            .is_some_and(|weak| weak.strong_count() > 0)
        {
            return Ok(None);
        }
        let token = Arc::new(WriterToken);
        state.insert(run_id, Arc::downgrade(&token));
        Ok(Some(token))
    }

    /// Returns whether the actual actor or its caller still holds a writer lease.
    pub fn is_held(
        &self,
        run_id: &RunId,
    ) -> impl std::future::Future<Output = Result<bool, WriterSlotError>> {
        let inner = &self.inner;
        async move {
            let state = inner.lock().map_err(|_| WriterSlotError)?;
            Ok(state
                .get(run_id)
                .is_some_and(|weak| weak.strong_count() > 0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_acquire_fails_while_first_held() {
        let m = ActiveWriters::default();
        let id = RunId::new();
        let t1 = m.try_acquire(id).await.unwrap().unwrap();
        assert!(m.try_acquire(id).await.unwrap().is_none());
        drop(t1);
        let t2 = m.try_acquire(id).await.unwrap().unwrap();
        assert_eq!(Arc::strong_count(&t2), 1);
    }

    #[tokio::test]
    async fn different_run_ids_independent() {
        let m = ActiveWriters::default();
        let a = m.try_acquire(RunId::new()).await.unwrap().unwrap();
        let b = m.try_acquire(RunId::new()).await.unwrap().unwrap();
        assert_eq!(Arc::strong_count(&a), 1);
        assert_eq!(Arc::strong_count(&b), 1);
    }

    #[tokio::test]
    async fn is_held_reflects_state() {
        let m = ActiveWriters::default();
        let id = RunId::new();
        assert!(!m.is_held(&id).await.unwrap());
        let t = m.try_acquire(id).await.unwrap().unwrap();
        assert!(m.is_held(&id).await.unwrap());
        drop(t);
        assert!(!m.is_held(&id).await.unwrap());
    }
}
