//! Engine-side tracking and decision routing for in-flight ACP elevation
//! requests.
//!
//! Wire shape:
//! 1. Bridge ([`surge_acp::bridge::client::BridgeClient`]) parks a
//!    `oneshot::Sender<RequestPermissionResponse>` and broadcasts
//!    [`surge_acp::bridge::event::BridgeEvent::PermissionRequested`].
//! 2. Engine observes the event in the agent stage's bridge-event loop,
//!    appends [`surge_core::EventPayload::SandboxElevationRequested`], and
//!    [`PendingElevations::register`]s a per-request `oneshot::Sender` so
//!    the engine itself can be told the operator's decision.
//! 3. Operator-facing surfaces (the resolved-via-event-log notification
//!    subsystem, tests, integration callers) reach the engine through
//!    [`crate::engine::engine::Engine::resolve_elevation`], which fires the
//!    [`PendingElevations`] sender.
//! 4. The agent stage `tokio::select!`s between the receiver and a timeout;
//!    on either path it appends the appropriate decision/timeout event and
//!    calls `AcpBridge::reply_to_permission` to release the agent.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use surge_core::SessionId;
use surge_core::keys::NodeKey;
use surge_core::run_event::ElevationDecision;
use tokio::sync::{Mutex, oneshot};

/// Metadata for a single in-flight elevation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingElevation {
    /// Session that originated the request.
    pub session: SessionId,
    /// Bridge-generated correlator (`request_id`) — pass back to
    /// `AcpBridge::reply_to_permission` to resolve the request.
    pub request_id: String,
    /// Agent node that owns this stage.
    pub node: NodeKey,
    /// Capability label derived by the bridge from the tool call.
    pub capability: String,
    /// Tool name supplied by the agent (may be empty).
    pub tool: String,
    /// Option IDs the agent offered. The engine selects one of these when
    /// fulfilling the request.
    pub options: Vec<String>,
    /// UTC timestamp at observation.
    pub requested_at: DateTime<Utc>,
}

/// Threshold beyond which the registry emits a `warn` log. Hit usually
/// indicates the engine isn't draining decisions fast enough or that the
/// approval channels are mis-configured.
pub const PENDING_REGISTRY_WARN_THRESHOLD: usize = 32;

/// Operator's decision on an elevation request, as routed by the engine.
///
/// Distinct from [`ElevationDecision`] because callers also supply the
/// ACP `option_id` they want surge to send back to the agent — the bridge
/// uses it directly in `RequestPermissionResponse::Selected(option_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineElevationDecision {
    /// Surge-side decision recorded in the event log.
    pub decision: ElevationDecision,
    /// Whether the operator chose the "remember" variant (e.g. allow-always).
    pub remember: bool,
    /// ACP-side `option_id` surge will send back to the agent. Should be
    /// one of the option IDs the agent offered (an unknown id flows through
    /// to the agent and the agent decides).
    pub option_id: String,
}

/// One row in the pending-elevations registry.
struct PendingState {
    metadata: PendingElevation,
    decision_tx: oneshot::Sender<EngineElevationDecision>,
}

/// Error returned by [`PendingElevations::resolve`] when the engine cannot
/// route an operator decision.
#[derive(Debug, thiserror::Error)]
pub enum ResolveElevationError {
    /// No pending elevation for `(session, request_id)` — already resolved,
    /// timed out, or never registered.
    #[error("no pending elevation for session={session} request_id={request_id}")]
    Unknown {
        /// Session the resolution attempt was bound to.
        session: SessionId,
        /// Request identifier from the matching `PermissionRequested` event.
        request_id: String,
    },
    /// Receiver side dropped before the decision was delivered. The agent
    /// stage already gave up on this elevation (typically because the
    /// session ended).
    #[error("receiver dropped for session={session} request_id={request_id}")]
    ReceiverDropped {
        /// Session the resolution attempt was bound to.
        session: SessionId,
        /// Request identifier from the matching `PermissionRequested` event.
        request_id: String,
    },
}

/// In-memory map of pending elevations keyed by `(SessionId, request_id)`.
///
/// Behind a `tokio::sync::Mutex` because the engine's bridge-event observer,
/// the decision router, and operator-facing entry points all touch it
/// concurrently.
#[derive(Default)]
pub struct PendingElevations {
    inner: Mutex<HashMap<(SessionId, String), PendingState>>,
}

impl std::fmt::Debug for PendingElevations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingElevations").finish()
    }
}

impl PendingElevations {
    /// Construct an empty tracker.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record an observed elevation request and return:
    /// - the `oneshot::Receiver` the agent stage awaits for the decision;
    /// - the new size of the registry (compare against
    ///   [`PENDING_REGISTRY_WARN_THRESHOLD`] to decide whether to log a warn).
    pub async fn register(
        &self,
        metadata: PendingElevation,
    ) -> (oneshot::Receiver<EngineElevationDecision>, usize) {
        let (tx, rx) = oneshot::channel();
        let key = (metadata.session, metadata.request_id.clone());
        let mut map = self.inner.lock().await;
        map.insert(
            key,
            PendingState {
                metadata,
                decision_tx: tx,
            },
        );
        let size = map.len();
        (rx, size)
    }

    /// Fire the decision on the pending registration. Returns the metadata
    /// of the removed entry on success, or [`ResolveElevationError`] when no
    /// matching entry exists or the receiver was already dropped.
    pub async fn resolve(
        &self,
        session: SessionId,
        request_id: &str,
        decision: EngineElevationDecision,
    ) -> Result<PendingElevation, ResolveElevationError> {
        let mut map = self.inner.lock().await;
        let state = map
            .remove(&(session, request_id.to_string()))
            .ok_or_else(|| ResolveElevationError::Unknown {
                session,
                request_id: request_id.to_string(),
            })?;
        let metadata = state.metadata;
        state
            .decision_tx
            .send(decision)
            .map_err(|_| ResolveElevationError::ReceiverDropped {
                session,
                request_id: request_id.to_string(),
            })?;
        Ok(metadata)
    }

    /// Cancel and remove a pending entry without firing the sender. Used by
    /// the agent stage on timeout or session-end to release the registry
    /// slot before posting the resulting event.
    pub async fn cancel(
        &self,
        session: SessionId,
        request_id: &str,
    ) -> Option<PendingElevation> {
        let mut map = self.inner.lock().await;
        map.remove(&(session, request_id.to_string())).map(|s| s.metadata)
    }

    /// Current registry size (for tests and observability snapshots).
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// `true` when no elevations are in flight.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }

    /// Snapshot of pending metadata (clones; intended for read-only views
    /// such as `surge doctor` and tests).
    pub async fn snapshot(&self) -> Vec<PendingElevation> {
        self.inner
            .lock()
            .await
            .values()
            .map(|s| s.metadata.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(session: SessionId, request_id: &str) -> PendingElevation {
        PendingElevation {
            session,
            request_id: request_id.to_string(),
            node: NodeKey::try_from("test_node").expect("valid node key"),
            capability: "fs-write:./test".into(),
            tool: "Write".into(),
            options: vec!["allow".into(), "deny".into()],
            requested_at: Utc::now(),
        }
    }

    fn allow_decision() -> EngineElevationDecision {
        EngineElevationDecision {
            decision: ElevationDecision::Allow,
            remember: false,
            option_id: "allow".into(),
        }
    }

    #[tokio::test]
    async fn register_then_resolve_routes_decision() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let (rx, size) = reg.register(pending(s, "r1")).await;
        assert_eq!(size, 1);
        let metadata = reg.resolve(s, "r1", allow_decision()).await.expect("resolves");
        assert_eq!(metadata.request_id, "r1");
        let routed = rx.await.expect("receiver wakes");
        assert_eq!(routed.decision, ElevationDecision::Allow);
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn resolve_returns_unknown_for_missing_entry() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let err = reg.resolve(s, "missing", allow_decision()).await.unwrap_err();
        assert!(matches!(err, ResolveElevationError::Unknown { .. }));
    }

    #[tokio::test]
    async fn resolve_after_receiver_drop_reports_receiver_dropped() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let (rx, _) = reg.register(pending(s, "r1")).await;
        drop(rx);
        let err = reg.resolve(s, "r1", allow_decision()).await.unwrap_err();
        assert!(matches!(err, ResolveElevationError::ReceiverDropped { .. }));
        // Entry removed regardless of receiver state.
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn cancel_removes_entry_without_firing_sender() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let (mut rx, _) = reg.register(pending(s, "r1")).await;
        let metadata = reg.cancel(s, "r1").await.expect("cancelled");
        assert_eq!(metadata.request_id, "r1");
        // The sender was dropped → receiver should yield Err on poll.
        assert!(rx.try_recv().is_err());
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn register_overwrites_duplicate_key() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let (rx_first, _) = reg.register(pending(s, "r1")).await;
        let mut second = pending(s, "r1");
        second.capability = "shell:bash".into();
        let (_rx_second, _) = reg.register(second).await;
        // The first receiver should observe a dropped sender.
        assert!(rx_first.await.is_err());
        let metadata = reg.snapshot().await.into_iter().next().unwrap();
        assert_eq!(metadata.capability, "shell:bash");
    }

    #[tokio::test]
    async fn snapshot_clones_pending_metadata() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let (_rx, _) = reg.register(pending(s, "r1")).await;
        let snap = reg.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].request_id, "r1");
        // Registry is still populated after snapshot.
        assert_eq!(reg.len().await, 1);
    }
}
