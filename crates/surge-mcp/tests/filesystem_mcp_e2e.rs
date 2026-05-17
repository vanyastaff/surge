//! U9 — opt-in end-to-end test against the real
//! `@modelcontextprotocol/server-filesystem` MCP server.
//!
//! The deterministic CI path stays on the mock fixture
//! (`mcp_stdio_e2e.rs`). This test is doubly gated so CI stays green
//! and mock-only:
//!
//! - `#[ignore]` — excluded from `cargo test`; needs `--ignored`.
//! - `SURGE_MCP_REAL=1` — even with `--ignored` it prints a SKIPPED
//!   banner and returns success unless the env var is set.
//!
//! Run it locally (Windows-safe — `npx` is resolved via rmcp's
//! `which_command`, which finds `npx.cmd`):
//!
//! ```text
//! SURGE_MCP_REAL=1 cargo test -p surge-mcp --test filesystem_mcp_e2e -- --ignored --nocapture
//! ```
//!
//! Requires Node/`npx` on PATH; the server is fetched on first run via
//! `npx -y @modelcontextprotocol/server-filesystem`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_mcp::McpRegistry;

fn skip_banner(reason: &str) {
    eprintln!(
        "[filesystem_mcp_e2e] SKIPPED: {reason}\n\
         Set SURGE_MCP_REAL=1 (and have `npx` on PATH) to run the real \
         filesystem-MCP smoke test."
    );
}

#[tokio::test]
#[ignore = "requires SURGE_MCP_REAL=1 + npx; real third-party MCP server"]
async fn filesystem_mcp_lists_tools_reads_a_file_and_shuts_down_cleanly() {
    if std::env::var("SURGE_MCP_REAL").ok().as_deref() != Some("1") {
        skip_banner("SURGE_MCP_REAL not set to 1");
        return;
    }

    // Resolve `npx` to a fully-qualified path (Windows: `npx.cmd`).
    let npx = match rmcp::transport::which_command("npx") {
        Ok(cmd) => PathBuf::from(cmd.as_std().get_program()),
        Err(e) => {
            skip_banner(&format!("`npx` not found on PATH: {e}"));
            return;
        },
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let marker = "surge-mcp-e2e-marker-7f3a";
    let file_path = tmp.path().join("hello.txt");
    std::fs::write(&file_path, format!("greetings {marker}\n")).expect("write fixture file");

    let server = McpServerRef::new(
        "filesystem".into(),
        McpTransportConfig::stdio(
            npx,
            vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
                tmp.path().to_string_lossy().into_owned(),
            ],
            HashMap::new(),
        ),
        None,
        // First run downloads the package via npx — generous timeout.
        Duration::from_secs(120),
        true,
    );

    let registry = Arc::new(McpRegistry::from_config(std::slice::from_ref(&server), None));

    let tools = registry
        .list_all_tools()
        .await
        .expect("filesystem MCP must list its tools");
    assert!(
        !tools.is_empty(),
        "filesystem MCP advertised no tools: {tools:?}"
    );

    // Tool names drift across server versions; pick a read-file-ish one.
    let read_tool = tools
        .iter()
        .find(|t| {
            let n = t.tool.to_lowercase();
            n.contains("read") && (n.contains("file") || n.contains("text"))
        })
        .map(|t| t.tool.clone())
        .unwrap_or_else(|| panic!("no read-file tool in {tools:?}"));

    let result = registry
        .call_tool(
            "filesystem",
            &read_tool,
            serde_json::json!({ "path": file_path.to_string_lossy() }),
            Duration::from_secs(30),
        )
        .await
        .expect("read tool call must succeed");
    assert!(!result.is_error, "read tool reported is_error: {result:?}");

    let text: String = result
        .content
        .iter()
        .filter_map(|c| match c {
            surge_mcp::McpContent::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains(marker),
        "read content did not contain the written marker; got: {text:?}"
    );

    // Deterministic teardown: no orphaned child; idempotent.
    registry.shutdown().await;
    registry.shutdown().await;
    for (name, health) in registry.statuses().await {
        assert_eq!(
            health,
            surge_mcp::McpHealth::Disconnected,
            "server {name} not Disconnected after shutdown"
        );
    }
}
