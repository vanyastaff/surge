//! Integration test: agent calls request_human_input; nobody resolves;
//! timeout fires; stage fails. Acceptance #10.
//!
//! KNOWN LIMITATION: same as engine_human_input_resolved.rs — requires
//! M3 worker reply routing fix + engine to thread agent args. M5.1 work.
//!
//! Specifically:
//! - `execute_agent_stage` hardcodes `AgentKind::Mock { args: vec![] }`,
//!   so there's no mechanism to pass `--scenario human_input` today.
//! - The worker's `BridgeCommand::ReplyToTool` handler is a stub.
//! - The human-input timeout is configured per `NodeLimits`; threading that
//!   into the mock scenario also requires M5.1 engine work.
//!
//! # Current test body
//!
//! Uses `MockBridge` (not `AcpBridge`) for the smoke assertion, to avoid the
//! `AcpBridge::Drop::join()` hang that occurs when the OS thread is dropped
//! synchronously from inside the tokio runtime. The real end-to-end body
//! is deferred to M5.1.

mod fixtures;

use std::path::PathBuf;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig};
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
async fn request_human_input_timeout_halts_run() {
    assert!(
        mock_agent_path().exists(),
        "mock_acp_agent binary missing at {} — run `cargo build -p surge-acp` first",
        mock_agent_path().display()
    );

    // Placeholder smoke: verify the engine constructs cleanly via MockBridge.
    // Real test body lands when M5.1 ships:
    //
    //   1. Configure NodeLimits with a very short human_input_timeout (e.g. 1s).
    //   2. Start a single-agent run using `--scenario human_input` (AcpBridge).
    //   3. Do NOT call engine.resolve_human_input — let it time out.
    //   4. Await completion; assert RunOutcome::Aborted or stage failure.
    //
    // This verifies that an unresolved human-input gate causes the run to
    // halt rather than block forever. MockBridge is used here to avoid the
    // AcpBridge Drop-join hang until the M5.1 real body lands.

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge: Arc<dyn BridgeFacade> = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    // Just exercise construction; real test body deferred to M5.1.
    let _engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());
    // Construction succeeded — the let binding above is the assertion.
}
