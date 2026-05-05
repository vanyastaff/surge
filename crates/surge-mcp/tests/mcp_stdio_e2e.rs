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
    let c = McpServerConnection::new(server_ref(true));
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
    let c = McpServerConnection::new(server_ref(true));
    let result = c
        .call_tool("echo", serde_json::json!({"text": "hello"}))
        .await
        .expect("call_tool");
    assert!(!result.is_error.unwrap_or(false));
}
