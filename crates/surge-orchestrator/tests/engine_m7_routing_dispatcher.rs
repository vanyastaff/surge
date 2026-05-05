//! Validates that [`RoutingToolDispatcher`]'s `declared_tools` merges
//! engine + MCP catalogs correctly (engine wins on collisions) and
//! that `dispatch` routes by [`ToolOrigin`].

use std::collections::HashMap;
use std::sync::Arc;
use surge_core::id::{RunId, SessionId};
use surge_core::run_state::RunMemory;
use surge_mcp::{McpRegistry, McpToolEntry};
use surge_orchestrator::engine::tools::{
    DeclaredTool, RoutingToolDispatcher, ToolCall, ToolDispatchContext, ToolDispatcher,
    ToolResultPayload,
};

struct EngineStub;

#[async_trait::async_trait]
impl ToolDispatcher for EngineStub {
    async fn dispatch(&self, _ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        ToolResultPayload::Ok {
            content: serde_json::json!({"engine_handled": call.tool}),
        }
    }
    fn declared_tools(&self) -> Vec<DeclaredTool> {
        vec![DeclaredTool::new(
            "shell_exec".into(),
            Some("engine version".into()),
            serde_json::json!({}),
        )]
    }
}

#[tokio::test]
async fn merged_catalog_engine_wins() {
    let registry = Arc::new(McpRegistry::from_config(&[]));
    let mcp_tools = vec![
        McpToolEntry::new(
            "mock".into(),
            "shell_exec".into(),
            Some("MCP override (should be ignored)".into()),
            serde_json::json!({}),
        ),
        McpToolEntry::new(
            "mock".into(),
            "browser_navigate".into(),
            None,
            serde_json::json!({}),
        ),
    ];
    let r = RoutingToolDispatcher::new(Arc::new(EngineStub), registry, &mcp_tools, &HashMap::new());
    let declared = r.declared_tools();
    let names: Vec<&str> = declared.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"shell_exec"));
    assert!(names.contains(&"browser_navigate"));
    // Engine version wins
    let shell_exec = declared.iter().find(|t| t.name == "shell_exec").unwrap();
    assert_eq!(shell_exec.description.as_deref(), Some("engine version"));
}

#[tokio::test]
async fn engine_route_is_taken_when_collision() {
    let registry = Arc::new(McpRegistry::from_config(&[]));
    let mcp_tools = vec![McpToolEntry::new(
        "mock".into(),
        "shell_exec".into(),
        None,
        serde_json::json!({}),
    )];
    let r = RoutingToolDispatcher::new(Arc::new(EngineStub), registry, &mcp_tools, &HashMap::new());
    let ctx = ToolDispatchContext {
        run_id: RunId::new(),
        session_id: SessionId::new(),
        worktree_root: std::path::Path::new("/tmp"),
        run_memory: &RunMemory::default(),
    };
    let call = ToolCall {
        call_id: "c1".into(),
        tool: "shell_exec".into(),
        arguments: serde_json::json!({}),
    };
    match r.dispatch(&ctx, &call).await {
        ToolResultPayload::Ok { content } => {
            assert_eq!(content["engine_handled"], "shell_exec");
        },
        other => panic!("expected engine route, got {other:?}"),
    }
}
