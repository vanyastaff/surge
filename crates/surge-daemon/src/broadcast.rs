//! `BroadcastRegistry` — multi-subscriber event fan-out used by the
//! daemon's IPC server. The daemon spawns one forward task per
//! active run; that task sends events into the per-run channel here,
//! and N CLI clients subscribed via `Subscribe` IPC each get their
//! own broadcast `Receiver`.

use std::collections::HashMap;
use surge_core::id::RunId;
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use tokio::sync::{RwLock, broadcast};

const DEFAULT_PER_RUN_CAPACITY: usize = 256;
const DEFAULT_GLOBAL_CAPACITY: usize = 64;

/// Multi-subscriber event registry. One channel per active run plus a
/// global channel for daemon-level events.
pub struct BroadcastRegistry {
    per_run: RwLock<HashMap<RunId, broadcast::Sender<EngineRunEvent>>>,
    global: broadcast::Sender<GlobalDaemonEvent>,
}

impl BroadcastRegistry {
    /// Construct an empty registry with default channel capacities
    /// (256 per-run, 64 global).
    #[must_use]
    pub fn new() -> Self {
        let (global_tx, _) = broadcast::channel(DEFAULT_GLOBAL_CAPACITY);
        Self {
            per_run: RwLock::new(HashMap::new()),
            global: global_tx,
        }
    }

    /// Register a new run. Returns the sender so the daemon's forward
    /// task can publish events into it. Subsequent `subscribe` calls
    /// for the same `run_id` produce fresh receivers off this sender.
    pub async fn register(&self, run_id: RunId) -> broadcast::Sender<EngineRunEvent> {
        let mut map = self.per_run.write().await;
        let (tx, _) = broadcast::channel(DEFAULT_PER_RUN_CAPACITY);
        map.insert(run_id, tx.clone());
        tx
    }

    /// Subscribe to a run's broadcast. Returns `None` if the run is
    /// not registered (probably already terminated).
    pub async fn subscribe(&self, run_id: RunId) -> Option<broadcast::Receiver<EngineRunEvent>> {
        let map = self.per_run.read().await;
        map.get(&run_id).map(broadcast::Sender::subscribe)
    }

    /// Drop the per-run channel. Subscribers receive `Closed` from
    /// future `recv` calls.
    pub async fn deregister(&self, run_id: RunId) {
        let mut map = self.per_run.write().await;
        map.remove(&run_id);
    }

    /// Subscribe to global daemon events.
    #[must_use]
    pub fn subscribe_global(&self) -> broadcast::Receiver<GlobalDaemonEvent> {
        self.global.subscribe()
    }

    /// Publish a global daemon event. Best-effort (no error if no
    /// subscribers).
    pub fn publish_global(&self, event: GlobalDaemonEvent) {
        let _ = self.global.send(event);
    }

    /// Number of currently-registered per-run channels (active runs).
    pub async fn active_count(&self) -> usize {
        self.per_run.read().await.len()
    }
}

impl Default for BroadcastRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::keys::NodeKey;
    use surge_orchestrator::engine::handle::RunOutcome;

    #[tokio::test]
    async fn register_subscribe_deregister() {
        let r = BroadcastRegistry::new();
        let id = RunId::new();
        let _tx = r.register(id).await;
        assert_eq!(r.active_count().await, 1);
        let rx = r.subscribe(id).await;
        assert!(rx.is_some());
        r.deregister(id).await;
        assert_eq!(r.active_count().await, 0);
        let rx2 = r.subscribe(id).await;
        assert!(rx2.is_none());
    }

    #[tokio::test]
    async fn global_publish_reaches_subscribers() {
        let r = BroadcastRegistry::new();
        let mut rx = r.subscribe_global();
        let id = RunId::new();
        r.publish_global(GlobalDaemonEvent::RunAccepted { run_id: id });
        let ev = rx.recv().await.unwrap();
        match ev {
            GlobalDaemonEvent::RunAccepted { run_id } => assert_eq!(run_id, id),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn per_run_event_fanout_to_two_subscribers() {
        let r = BroadcastRegistry::new();
        let id = RunId::new();
        let tx = r.register(id).await;
        let mut a = r.subscribe(id).await.unwrap();
        let mut b = r.subscribe(id).await.unwrap();
        let _ = tx.send(EngineRunEvent::Terminal(RunOutcome::Completed {
            terminal: NodeKey::try_from("end").unwrap(),
        }));
        assert!(matches!(
            a.recv().await.unwrap(),
            EngineRunEvent::Terminal(_)
        ));
        assert!(matches!(
            b.recv().await.unwrap(),
            EngineRunEvent::Terminal(_)
        ));
    }
}
