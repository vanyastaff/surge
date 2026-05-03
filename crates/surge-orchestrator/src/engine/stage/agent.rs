//! `NodeKind::Agent` execution.
//!
//! Phase 6.1: skeleton — opens an ACP session via the BridgeFacade,
//! sends a placeholder empty message, immediately closes with a "done"
//! outcome. Phase 6.2 wires the real event loop driven by
//! `BridgeEvent::OutcomeReported`. Phase 6.3 handles tool dispatch
//! for non-injected tools. Phase 6.4 handles prompt + binding resolution.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::agent_config::AgentConfig;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_persistence::runs::run_writer::RunWriter;

use crate::engine::sandbox_factory::build_sandbox;
use crate::engine::stage::{StageError, StageResult};

/// Parameters for executing a single agent stage.
pub struct AgentStageParams<'a> {
    /// Key of the node being executed (used for tracing; wired to events in 6.2).
    pub node: &'a NodeKey,
    /// Agent node configuration from the spec graph.
    pub agent_config: &'a AgentConfig,
    /// Bridge facade for ACP session lifecycle.
    pub bridge: &'a Arc<dyn BridgeFacade>,
    /// Run writer (events emitted here in Phase 6.2+).
    pub writer: &'a RunWriter,
    /// Isolated git worktree path for this run.
    pub worktree_path: &'a Path,
}

/// Execute a single agent stage.
///
/// Phase 6.1 skeleton: opens a session, sends an empty placeholder message,
/// then immediately closes the session and returns `OutcomeKey("done")`.
/// Phase 6.2 replaces this with the real event-loop waiting for
/// `BridgeEvent::OutcomeReported`.
///
/// # Errors
/// Returns [`StageError::Bridge`] if any bridge call fails.
pub async fn execute_agent_stage(p: AgentStageParams<'_>) -> StageResult {
    // Build minimal SessionConfig. Phase 6.4 wires bindings + real prompt.
    let sandbox = build_sandbox(p.agent_config.sandbox_override.as_ref());
    let session_config = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec![] },
        working_dir: p.worktree_path.to_path_buf(),
        system_prompt: String::new(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox,
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let session_id = p
        .bridge
        .open_session(session_config)
        .await
        .map_err(|e| StageError::Bridge(format!("open_session: {e}")))?;

    // Stub: send empty message, immediately close. Phase 6.2 will replace
    // with the real prompt + event-loop wait for OutcomeReported.
    p.bridge
        .send_message(session_id, MessageContent::Text(String::new()))
        .await
        .map_err(|e| StageError::Bridge(format!("send_message: {e}")))?;

    p.bridge
        .close_session(session_id)
        .await
        .map_err(|e| StageError::Bridge(format!("close_session: {e}")))?;

    let _ = p.writer; // writer used for events in 6.2
    let _ = p.node;

    Ok(OutcomeKey::from_str("done").unwrap())
}
