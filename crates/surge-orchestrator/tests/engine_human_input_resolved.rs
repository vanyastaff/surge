//! Integration test: agent calls request_human_input; engine.resolve_human_input
//! delivers the answer; agent reports done. Acceptance #9.
//!
//! KNOWN LIMITATION: M3 worker's BridgeCommand::ReplyToTool handler is
//! currently a stub (Task 2.1.5) that logs + acks but doesn't route the
//! reply through ACP. This test is expected to fail until the worker fix
//! lands. Documented for the M5.1 follow-up.
//!
//! Additionally, `execute_agent_stage` hardcodes `AgentKind::Mock { args: vec![] }`
//! — there is no current mechanism to pass `--scenario human_input` to
//! mock_acp_agent without modifying the engine. Both blockers are M5.1 work.
//!
//! # Current test body
//!
//! Uses `MockBridge` (not `AcpBridge`) for the smoke assertion, to avoid the
//! `AcpBridge::Drop::join()` hang that occurs when the OS thread is dropped
//! synchronously from inside the tokio runtime (the bridge worker's dedicated
//! OS thread blocks `tokio::runtime::Handle` from exiting cleanly).
//! The real end-to-end body is deferred to M5.1.

mod fixtures;

use std::path::PathBuf;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::id::RunId;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError};
use surge_persistence::runs::Storage;

/// Resolve the `mock_acp_agent` binary path.
///
/// `CARGO_BIN_EXE_mock_acp_agent` is set by Cargo only when the test binary
/// and the `[[bin]]` live in the same package — they don't here. Fall back to
/// an absolute path derived from `CARGO_MANIFEST_DIR` (compile-time constant),
/// which is always `<workspace_root>/crates/surge-orchestrator`.
fn mock_agent_path() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_mock_acp_agent") {
        return PathBuf::from(path);
    }
    // CARGO_MANIFEST_DIR = …/crates/surge-orchestrator; workspace root is ../../
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));
    let bin = if cfg!(windows) {
        "mock_acp_agent.exe"
    } else {
        "mock_acp_agent"
    };
    target.join("debug").join(bin)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent + worker reply routing fix (M5.1)"]
async fn request_human_input_resolved_completes_run() {
    assert!(
        mock_agent_path().exists(),
        "mock_acp_agent binary missing at {} — run `cargo build -p surge-acp` first",
        mock_agent_path().display()
    );

    // The mock_acp_agent's `human_input` scenario emits request_human_input
    // and then waits for a reply before reporting done. To use it, we'd
    // need to override the engine's hardcoded AgentKind::Mock { args: vec![] }
    // to pass `--scenario human_input`. Since that's not currently possible,
    // the test body is a placeholder.
    //
    // When the engine threads agent args through AgentConfig (M5.1+) and
    // the worker's reply routing is implemented, replace this body with:
    //
    //   1. Start a single-agent run that triggers human_input.
    //   2. Subscribe to handle.events; await HumanInputRequested.
    //   3. Extract the call_id from the event.
    //   4. engine.resolve_human_input(run_id, Some(call_id), json!({"answer":"go"})).await?;
    //   5. Await completion; assert Completed.
    //
    // For now, use MockBridge (not AcpBridge) to avoid the AcpBridge
    // Drop-join hang, and just assert the API surface exists by exercising
    // resolve_human_input on an unknown run (returns RunNotFound).

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge: Arc<dyn BridgeFacade> = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    // Smoke: resolve_human_input on unknown run returns RunNotFound (proves API exists).
    let r = engine
        .resolve_human_input(
            RunId::new(),
            Some("c1".into()),
            serde_json::json!({"answer": "go"}),
        )
        .await;
    assert!(
        matches!(r, Err(EngineError::RunNotFound(_))),
        "expected RunNotFound, got {r:?}"
    );
}
