//! U3 — deterministic per-run MCP teardown (R4) and the U11
//! cancellation seam.
//!
//! Proves the registry-teardown contract the engine's `run_task::execute`
//! wrapper invokes on every terminal path (completed / failed / aborted):
//! - `shutdown()` cancels the registry token (the seam U11's health
//!   monitors bind to) so monitors stop instead of racing teardown,
//! - every connection ends `Disconnected` (no orphaned child handle),
//! - `shutdown()` is idempotent (recovery / double-terminal safe).
//!
//! The `RestartExhausted -> EscalationRequested` path's type-safe
//! capture is unit-covered by `surge-mcp`'s `restart_decision`
//! exhaustion test plus the `RoutingToolDispatcher` straight-line typed
//! match (no string heuristic); the engine appends the event in the
//! agent stage's drain loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_mcp::{McpHealth, McpRegistry};

fn unreachable_server(name: &str) -> McpServerRef {
    McpServerRef::new(
        name.into(),
        McpTransportConfig::stdio(
            PathBuf::from("surge_nonexistent_mcp_binary_xyz"),
            vec![],
            std::collections::HashMap::new(),
        ),
        None,
        Duration::from_millis(50),
        true,
    )
}

#[tokio::test]
async fn shutdown_cancels_token_and_disconnects_all() {
    let registry = Arc::new(McpRegistry::from_config(
        &[unreachable_server("a"), unreachable_server("b")],
        None,
    ));

    let token = registry.cancel_token();
    assert!(!token.is_cancelled(), "token must start live");

    registry.shutdown().await;

    assert!(
        token.is_cancelled(),
        "shutdown must cancel the U11 health-monitor seam"
    );
    for (name, health) in registry.statuses().await {
        assert_eq!(
            health,
            McpHealth::Disconnected,
            "server {name} must be Disconnected after shutdown (no orphaned handle)"
        );
    }
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    let registry = Arc::new(McpRegistry::from_config(&[unreachable_server("a")], None));
    // A run that terminates, then a recovery pass that terminates again,
    // must not panic or hang on a second teardown.
    registry.shutdown().await;
    registry.shutdown().await;
    assert!(registry.cancel_token().is_cancelled());
    let statuses = registry.statuses().await;
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].1, McpHealth::Disconnected);
}

#[tokio::test]
async fn empty_registry_shutdown_is_a_noop() {
    let registry = Arc::new(McpRegistry::from_config(&[], None));
    // Must return promptly and cancel the token even with no servers.
    tokio::time::timeout(Duration::from_secs(2), registry.shutdown())
        .await
        .expect("empty-registry shutdown must not block");
    assert!(registry.cancel_token().is_cancelled());
    assert!(registry.statuses().await.is_empty());
}
