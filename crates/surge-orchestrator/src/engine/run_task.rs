//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::tools::ToolDispatcher;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_persistence::runs::RunWriter;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunTaskParams {
    pub writer: RunWriter,
    pub bridge: Arc<dyn BridgeFacade>,
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    pub graph: Graph,
    pub worktree_path: PathBuf,
    pub run_config: EngineRunConfig,
    pub event_tx: broadcast::Sender<EngineRunEvent>,
    pub cancel: CancellationToken,
    /// True when resuming from a snapshot — skips RunStarted/PipelineMaterialized emission.
    pub resume_mode: bool,
}

pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    // Phase 5 stub: emit "not implemented" then exit so the start_run path
    // is exercisable end-to-end. Phase 6+ wires in real stage execution.
    let _ = params.writer; // silence unused warnings
    let _ = params.bridge;
    let _ = params.tool_dispatcher;
    let _ = params.graph;
    let _ = params.worktree_path;
    let _ = params.run_config;
    let _ = params.cancel;
    let _ = params.resume_mode;
    let _ = params.event_tx.send(EngineRunEvent::Terminal(RunOutcome::Failed {
        error: "run_task::execute is a Phase 5 stub; real lifecycle lands in Phase 7+".into(),
    }));
    RunOutcome::Failed {
        error: "run_task::execute Phase 5 stub".into(),
    }
}
