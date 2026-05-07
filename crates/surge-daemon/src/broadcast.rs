//! `BroadcastRegistry` — multi-subscriber event fan-out used by the
//! daemon's IPC server. The daemon spawns one forward task per
//! active run; that task sends events into the per-run channel here,
//! and N CLI clients subscribed via `Subscribe` IPC each get their
//! own broadcast `Receiver`.

use std::collections::HashMap;
use surge_core::id::RunId;
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

const DEFAULT_PER_RUN_CAPACITY: usize = 256;
const DEFAULT_GLOBAL_CAPACITY: usize = 64;

/// Multi-subscriber event registry. One channel per active run plus a
/// global channel for daemon-level events.
pub struct BroadcastRegistry {
    per_run: RwLock<HashMap<RunId, broadcast::Sender<EngineRunEvent>>>,
    global: broadcast::Sender<GlobalDaemonEvent>,
    /// Pending receivers for queued-then-admitted runs. When
    /// [`Self::subscribe_eventual`] is called for a run that hasn't
    /// been [`Self::register`]ed yet, the caller's oneshot is parked
    /// here. [`Self::register`] drains all matching entries and sends
    /// each waiter a fresh receiver derived from the just-created
    /// per-run sender — atomically, before the publisher is exposed
    /// to any forwarder task. This guards against the race where a
    /// `spawn_forward_task` push beats the subscriber's call to
    /// `broadcast::Sender::subscribe`.
    waiters: Mutex<HashMap<RunId, Vec<oneshot::Sender<broadcast::Receiver<EngineRunEvent>>>>>,
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
            waiters: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new run. Returns the sender so the daemon's forward
    /// task can publish events into it. Subsequent `subscribe` calls
    /// for the same `run_id` produce fresh receivers off this sender.
    /// Any pending [`Self::subscribe_eventual`] waiters are
    /// immediately woken with a fresh receiver attached to the
    /// just-created sender, before this method returns.
    pub async fn register(&self, run_id: RunId) -> broadcast::Sender<EngineRunEvent> {
        let (tx, _) = broadcast::channel(DEFAULT_PER_RUN_CAPACITY);
        // Insert the sender first so any concurrent
        // `subscribe_eventual` caller racing this method either: (a)
        // sees the sender on its fast-path subscribe and skips waiter
        // registration entirely, or (b) registers a waiter that we
        // then drain below. Either way, no event is lost as long as
        // the daemon's forward task is spawned strictly after this
        // method returns (per `drain_one_pass` ordering in
        // `server.rs`).
        {
            let mut map = self.per_run.write().await;
            map.insert(run_id, tx.clone());
        }
        let mut waiters = self.waiters.lock().await;
        if let Some(pending) = waiters.remove(&run_id) {
            for waiter in pending {
                // Each waiter gets its own receiver. If the waiter
                // was dropped meanwhile (oneshot rx side closed),
                // `send` returns Err — that's fine, just skip.
                let _ = waiter.send(tx.subscribe());
            }
        }
        tx
    }

    /// Subscribe to a run's broadcast. Returns `None` if the run is
    /// not registered (probably already terminated).
    pub async fn subscribe(&self, run_id: RunId) -> Option<broadcast::Receiver<EngineRunEvent>> {
        let map = self.per_run.read().await;
        map.get(&run_id).map(broadcast::Sender::subscribe)
    }

    /// Like [`Self::subscribe`] but returns a oneshot receiver that
    /// resolves to a per-run [`broadcast::Receiver`] **once the run
    /// is registered**. If the run is already registered, the
    /// returned oneshot is pre-fulfilled (`recv().await` returns
    /// immediately). If the run is not yet registered, the oneshot
    /// resolves when [`Self::register`] is called for the same
    /// `run_id` — and the receiver is attached to the sender BEFORE
    /// `register` returns to its caller (the daemon's drain task),
    /// closing the race against `spawn_forward_task` events being
    /// pushed before the subscriber attaches.
    ///
    /// If the run is later cancelled while still queued (no
    /// `register` ever happens), the oneshot eventually resolves
    /// with `Err(RecvError)` when the registry is dropped — callers
    /// must handle that.
    pub async fn subscribe_eventual(
        &self,
        run_id: RunId,
    ) -> oneshot::Receiver<broadcast::Receiver<EngineRunEvent>> {
        let (tx, rx) = oneshot::channel();
        // Take the waiters lock first. This serializes with
        // `register` (which also locks waiters after writing per_run),
        // so the "did register beat us?" check below is reliable: if
        // `register` ran while we were waiting for this lock, its
        // sender is already in `per_run` and we just take the fast
        // path; otherwise we park the waiter and `register` will
        // drain it on its next call.
        let mut waiters = self.waiters.lock().await;
        if let Some(sender) = self.per_run.read().await.get(&run_id) {
            let _ = tx.send(sender.subscribe());
        } else {
            waiters.entry(run_id).or_default().push(tx);
        }
        rx
    }

    /// Drop the per-run channel. Subscribers receive `Closed` from
    /// future `recv` calls. Any unfulfilled
    /// [`Self::subscribe_eventual`] waiters for this run are also
    /// dropped (their oneshot rx side will return
    /// `RecvError::Closed`); callers should treat that as "the run
    /// was cancelled before admission landed".
    pub async fn deregister(&self, run_id: RunId) {
        let mut map = self.per_run.write().await;
        map.remove(&run_id);
        drop(map);
        // Also clear any waiters: a deregister without a prior
        // register means the run never made it past admission (e.g.
        // StopRun on a queued run). Dropping the senders signals the
        // pending forwarders to exit.
        let mut waiters = self.waiters.lock().await;
        waiters.remove(&run_id);
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
        let _ = tx.send(EngineRunEvent::Terminal {
            outcome: RunOutcome::Completed {
                terminal: NodeKey::try_from("end").unwrap(),
            },
        });
        assert!(matches!(
            a.recv().await.unwrap(),
            EngineRunEvent::Terminal { .. }
        ));
        assert!(matches!(
            b.recv().await.unwrap(),
            EngineRunEvent::Terminal { .. }
        ));
    }
}
