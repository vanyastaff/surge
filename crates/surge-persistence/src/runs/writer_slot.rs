//! In-process tracking of which RunIds currently have a live writer.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use surge_core::RunId;
use tokio::sync::Mutex;

/// Sentinel object whose Arc lifetime represents an active writer slot.
///
/// `RunWriter` holds an `Arc<WriterToken>`; the corresponding `Weak` lives in
/// `Storage::active_writers`. When the Arc drops (writer closed), the slot is
/// freed: the next `try_acquire` for the same `RunId` succeeds.
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

    /// Returns true if a live writer currently holds the slot for `run_id`.
    pub async fn is_held(&self, run_id: &RunId) -> bool {
        let g = self.inner.lock().await;
        g.get(run_id).map(|w| w.strong_count() > 0).unwrap_or(false)
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

    #[tokio::test]
    async fn is_held_reflects_state() {
        let m = ActiveWriters::default();
        let id = RunId::new();
        assert!(!m.is_held(&id).await);
        let t = m.try_acquire(id).await.unwrap();
        assert!(m.is_held(&id).await);
        drop(t);
        assert!(!m.is_held(&id).await);
    }
}
