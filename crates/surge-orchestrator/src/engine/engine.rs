//! `Engine` — the public API. Methods are stubbed in this task and
//! implemented incrementally in Phase 5 (lifecycle), Phase 9 (resolve),
//! Phase 11 (stop).

use crate::engine::config::{EngineConfig, EngineRunConfig};
use crate::engine::error::EngineError;
use crate::engine::handle::RunHandle;
use crate::engine::tools::ToolDispatcher;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;

pub struct Engine {
    bridge: Arc<dyn BridgeFacade>,
    storage: Arc<surge_persistence::runs::Storage>,
    tool_dispatcher: Arc<dyn ToolDispatcher>,
    config: Arc<EngineConfig>,
}

impl Engine {
    pub fn new(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        config: EngineConfig,
    ) -> Self {
        Self {
            bridge,
            storage,
            tool_dispatcher,
            config: Arc::new(config),
        }
    }

    /// Start a new run. Phase 5 implements the body.
    pub async fn start_run(
        &self,
        _run_id: RunId,
        _graph: Graph,
        _worktree_path: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        Err(EngineError::Internal(
            "Engine::start_run not yet implemented (Phase 5)".into(),
        ))
    }

    /// Resume an existing run. Phase 10 implements the body.
    pub async fn resume_run(
        &self,
        _run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        Err(EngineError::Internal(
            "Engine::resume_run not yet implemented (Phase 10)".into(),
        ))
    }

    /// Provide answer to a paused run waiting on human input. Phase 9 impl.
    pub async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Err(EngineError::Internal(
            "Engine::resolve_human_input not yet implemented (Phase 9)".into(),
        ))
    }

    /// Cancel an in-flight run. Phase 11 implements the body.
    pub async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Err(EngineError::Internal(
            "Engine::stop_run not yet implemented (Phase 11)".into(),
        ))
    }
}

// Suppress unused-field warnings on stubbed methods until Phase 5+ lands.
#[allow(dead_code)]
fn _engine_unused_field_check(e: &Engine) {
    let _ = &e.bridge;
    let _ = &e.storage;
    let _ = &e.tool_dispatcher;
    let _ = &e.config;
}
