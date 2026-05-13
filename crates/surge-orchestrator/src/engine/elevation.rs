//! Engine-side tracking for in-flight ACP elevation requests.
//!
//! The bridge owns the `oneshot::Sender` that fulfils the agent's
//! `request_permission` call (see `surge_acp::bridge::client::BridgeClient`).
//! The engine observes [`surge_acp::bridge::event::BridgeEvent::PermissionRequested`]
//! via the bridge's broadcast and appends the matching
//! [`surge_core::EventPayload::SandboxElevationRequested`] so the elevation
//! shows up in the run's event log and `surge replay` view.
//!
//! Task 7 wires the observability and append path. Task 8 reuses this
//! registry to dispatch notifications and route the operator's decision
//! back via `AcpBridge::reply_to_permission`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use surge_core::SessionId;
use surge_core::keys::NodeKey;
use tokio::sync::Mutex;

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

/// In-memory map of pending elevations keyed by `(SessionId, request_id)`.
///
/// Behind a `tokio::sync::Mutex` because the engine's bridge-event observer
/// and the future Task 8 decision router both touch it concurrently.
#[derive(Debug, Default)]
pub struct PendingElevations {
    inner: Mutex<HashMap<(SessionId, String), PendingElevation>>,
}

impl PendingElevations {
    /// Construct an empty tracker.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record an observed elevation request. Returns the new size of the
    /// registry; callers may compare against
    /// [`PENDING_REGISTRY_WARN_THRESHOLD`] to decide whether to log a warning.
    pub async fn register(&self, pending: PendingElevation) -> usize {
        let mut map = self.inner.lock().await;
        map.insert((pending.session, pending.request_id.clone()), pending);
        map.len()
    }

    /// Remove and return the pending entry for the given `(session, request_id)`,
    /// if present.
    pub async fn resolve(
        &self,
        session: SessionId,
        request_id: &str,
    ) -> Option<PendingElevation> {
        let mut map = self.inner.lock().await;
        map.remove(&(session, request_id.to_string()))
    }

    /// Current registry size (for tests and observability snapshots).
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// `true` when no elevations are in flight.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
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

    #[tokio::test]
    async fn register_and_resolve() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        let size = reg.register(pending(s, "r1")).await;
        assert_eq!(size, 1);
        let resolved = reg.resolve(s, "r1").await.expect("registered");
        assert_eq!(resolved.request_id, "r1");
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn resolve_returns_none_when_absent() {
        let reg = PendingElevations::new();
        let s = SessionId::new();
        assert!(reg.resolve(s, "missing").await.is_none());
    }

    #[tokio::test]
    async fn register_overwrites_duplicate_key() {
        // Two distinct requests with the same (session, request_id) would
        // indicate a bridge bug, but we make the tracker robust: overwrite,
        // do not panic.
        let reg = PendingElevations::new();
        let s = SessionId::new();
        reg.register(pending(s, "r1")).await;
        let mut p2 = pending(s, "r1");
        p2.capability = "shell:bash".into();
        reg.register(p2.clone()).await;
        let resolved = reg.resolve(s, "r1").await.unwrap();
        assert_eq!(resolved.capability, "shell:bash");
    }
}
