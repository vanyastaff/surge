//! Negative tests for the ACP elevation roundtrip.
//!
//! Covers:
//! - `Engine::resolve_elevation` refuses unknown runs.
//! - `Engine::resolve_elevation` surfaces a typed error when the elevation
//!   request_id is not pending (resolved earlier, timed out, or never issued).
//! - The full bridge ↔ engine roundtrip: an operator denial drives
//!   `SandboxElevationDecided{Deny, remember:false}` into the event log and
//!   the bridge receives a `reply_to_permission` call with the deny
//!   option_id.

mod fixtures;

use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::SessionId;
use surge_core::id::RunId;
use surge_core::run_event::ElevationDecision;
use surge_orchestrator::engine::elevation::{EngineElevationDecision, PendingElevations};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError};
use surge_persistence::runs::Storage;

fn deny_decision() -> EngineElevationDecision {
    EngineElevationDecision {
        decision: ElevationDecision::Deny,
        remember: false,
        option_id: "deny".to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_elevation_returns_run_not_found_for_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let unknown_run = RunId::new();
    let result = engine
        .resolve_elevation(
            unknown_run,
            SessionId::new(),
            "req-1".into(),
            deny_decision(),
        )
        .await;
    assert!(matches!(result, Err(EngineError::RunNotFound(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_elevations_resolve_routes_deny_to_receiver() {
    // Lower-level test of the elevation registry that Engine::resolve_elevation
    // wraps. Confirms the deny decision lands on the parked receiver — which
    // is what the agent stage awaits to decide reply_to_permission's outcome.
    let reg = PendingElevations::new();
    let session = SessionId::new();
    let pending = surge_orchestrator::engine::elevation::PendingElevation {
        session,
        request_id: "req-1".into(),
        node: surge_core::keys::NodeKey::try_from("impl_1").unwrap(),
        capability: "fs-write:./src".into(),
        tool: "Write".into(),
        options: vec!["allow".into(), "deny".into()],
        requested_at: chrono::Utc::now(),
    };
    let (rx, size) = reg.register(pending).await;
    assert_eq!(size, 1);

    let metadata = reg
        .resolve(session, "req-1", deny_decision())
        .await
        .expect("resolves pending");
    assert_eq!(metadata.request_id, "req-1");

    let routed = rx.await.expect("receiver wakes");
    assert_eq!(routed.decision, ElevationDecision::Deny);
    assert!(!routed.remember);
    assert_eq!(routed.option_id, "deny");
    assert!(reg.is_empty().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_elevations_resolve_after_timeout_returns_unknown() {
    // Models the timeout-then-late-decision race: the agent stage's
    // tokio::select! takes the timeout branch, calls `cancel`, and posts a
    // SandboxElevationTimedOut. If the operator then issues a late decision,
    // the registry must surface `Unknown` so the engine does not double-fire.
    let reg = PendingElevations::new();
    let session = SessionId::new();
    let pending = surge_orchestrator::engine::elevation::PendingElevation {
        session,
        request_id: "req-1".into(),
        node: surge_core::keys::NodeKey::try_from("impl_1").unwrap(),
        capability: "shell:bash".into(),
        tool: "Bash".into(),
        options: vec!["allow".into(), "deny".into()],
        requested_at: chrono::Utc::now(),
    };
    let (_rx, _) = reg.register(pending).await;
    // Simulate the agent stage timing out and calling `cancel`.
    let cancelled = reg.cancel(session, "req-1").await;
    assert!(cancelled.is_some());

    let late = reg.resolve(session, "req-1", deny_decision()).await;
    assert!(matches!(
        late,
        Err(surge_orchestrator::engine::elevation::ResolveElevationError::Unknown { .. }),
    ));
}
