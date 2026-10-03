//! End-to-end stdio integration: spawn the mock server fixture
//! (built via `cargo build --example mock_mcp_server --features
//! mock-server`), connect via [`McpServerConnection`], list tools,
//! call `echo`, observe response.
//!
//! Marked `#[ignore]` because the tests require the example binary
//! to be pre-built. Run with `cargo test -p surge-mcp -- --ignored`
//! after building the example.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_mcp::McpServerConnection;

fn mock_server_path() -> PathBuf {
    // Built by `cargo build --example mock_mcp_server --features mock-server`.
    // CARGO_TARGET_DIR is set by cargo when available; otherwise fall back to
    // `<workspace_root>/target`. Integration tests run with cwd set to the
    // crate manifest directory, so walk up two levels from CARGO_MANIFEST_DIR
    // (crate → workspace) to locate the target directory.
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest_dir = PathBuf::from(std::env!("CARGO_MANIFEST_DIR"));
            manifest_dir
                .parent() // crates/
                .and_then(|p| p.parent()) // workspace root
                .map(|p| p.join("target"))
                .unwrap_or_else(|| PathBuf::from("target"))
        });
    target
        .join("debug")
        .join("examples")
        .join(if cfg!(windows) {
            "mock_mcp_server.exe"
        } else {
            "mock_mcp_server"
        })
}

fn server_ref(restart: bool) -> McpServerRef {
    McpServerRef::new(
        "mock".into(),
        McpTransportConfig::stdio(mock_server_path(), vec![], HashMap::new()),
        None,
        Duration::from_secs(5),
        restart,
    )
}

#[tokio::test]
#[ignore = "requires `cargo build --example mock_mcp_server --features mock-server` first"]
async fn list_tools_includes_echo_and_crash_now() {
    let c = McpServerConnection::new(server_ref(true), None);
    let tools = c.list_tools().await.expect("list_tools");
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert!(
        names.contains(&"echo".to_string()),
        "expected 'echo' in tool list, got: {names:?}"
    );
    assert!(
        names.contains(&"crash_now".to_string()),
        "expected 'crash_now' in tool list, got: {names:?}"
    );
}

#[tokio::test]
#[ignore = "requires mock_mcp_server example built"]
async fn call_echo_round_trips() {
    let c = McpServerConnection::new(server_ref(true), None);
    let result = c
        .call_tool("echo", serde_json::json!({"text": "hello"}))
        .await
        .expect("call_tool");
    assert!(!result.is_error.unwrap_or(false));
    // Verify the echoed text is in the result content.
    let content_text: String = result
        .content
        .into_iter()
        .filter_map(|c| match c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text),
            _ => None,
        })
        .collect();
    assert!(
        content_text.contains("hello"),
        "echo response did not contain 'hello': {content_text:?}"
    );
}

#[tokio::test]
#[ignore = "requires mock_mcp_server example built"]
async fn outstanding_live_service_is_reported_as_unconfirmed_cleanup() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("echo-entered");
    let release = home.path().join("echo-release");
    let config = McpServerRef::new(
        "mock".into(),
        McpTransportConfig::stdio(
            mock_server_path(),
            vec![],
            HashMap::from([
                (
                    "SURGE_MCP_TEST_ECHO_MARKER".into(),
                    marker.display().to_string(),
                ),
                (
                    "SURGE_MCP_TEST_ECHO_RELEASE".into(),
                    release.display().to_string(),
                ),
            ]),
        ),
        None,
        Duration::from_secs(5),
        false,
    );
    let connection = std::sync::Arc::new(McpServerConnection::new(config, None));
    let caller = connection.clone();
    let in_flight = tokio::spawn(async move {
        caller
            .call_tool("echo", serde_json::json!({"text":"still-owned"}))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let error = connection.shutdown().await.unwrap_err();
    assert!(matches!(
        error,
        surge_mcp::cleanup::CleanupError::OutstandingHandle { .. }
    ));
    std::fs::write(&release, b"release").unwrap();
    // The old service really remains usable; a disconnected registry label
    // therefore cannot be promoted to a writer-disappearance proof.
    let result = in_flight.await.unwrap().unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_ownership_intent_prevents_actual_mcp_child_spawn() {
    use surge_mcp::writer_observer::{HostWriterObserver, WriterObservationError};
    struct Reject;
    #[async_trait::async_trait]
    impl HostWriterObserver for Reject {
        async fn before_child(
            &self,
            _: &str,
        ) -> Result<surge_core::id::ExecutionWriterId, WriterObservationError> {
            Err(WriterObservationError(
                "journal deliberately rejected ownership".into(),
            ))
        }
        async fn child_started(
            &self,
            _: surge_core::id::ExecutionWriterId,
            _: Option<u32>,
        ) -> Result<(), WriterObservationError> {
            panic!("rejected launch must not reach process observation")
        }
    }
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("must-not-spawn");
    let config = McpServerRef::new(
        "rejected".into(),
        McpTransportConfig::stdio(
            PathBuf::from("sh"),
            vec![
                "-c".into(),
                format!("printf effect > '{}'", marker.display()),
            ],
            HashMap::new(),
        ),
        None,
        Duration::from_secs(2),
        false,
    );
    let connection = McpServerConnection::new_owned(
        config,
        Some(home.path().into()),
        std::sync::Arc::new(Reject),
    );
    let result = connection.list_tools().await;
    assert!(
        matches!(result, Err(surge_mcp::McpError::StartFailed { reason, .. })
        if reason.contains("journal deliberately rejected"))
    );
    assert!(!marker.exists());
}
