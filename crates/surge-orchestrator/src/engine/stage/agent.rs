//! `NodeKind::Agent` execution.
//!
//! Phase 6.2: event loop — opens an ACP session, sends a placeholder empty
//! message, drives `BridgeEvent` until `OutcomeReported` is received (success)
//! or `SessionEnded` fires first (failure). Phase 6.3 handles tool dispatch
//! for non-injected tools. Phase 6.4 handles prompt + binding resolution.

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
/// Phase 6.2: opens a session, sends an empty placeholder message, then drives
/// the `BridgeEvent` loop until `BridgeEvent::OutcomeReported` arrives
/// (success) or `BridgeEvent::SessionEnded` fires without a prior outcome
/// (failure). Phase 6.3 wires tool dispatch; Phase 6.4 wires prompt/bindings.
///
/// # Errors
/// Returns [`StageError::Bridge`] if any bridge call fails.
/// Returns [`StageError::AgentCrashed`] if the session ends without reporting
/// an outcome.
/// Returns [`StageError::Storage`] if event persistence fails.
pub async fn execute_agent_stage(p: AgentStageParams<'_>) -> StageResult {
    use surge_acp::bridge::event::BridgeEvent;
    use surge_core::run_event::{EventPayload, SessionDisposition, VersionedEventPayload};

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
        bindings: Default::default(),
    };

    // Subscribe to events BEFORE opening the session, so we don't miss the
    // SessionEstablished event (or earlier ToolCall events).
    let mut events = p.bridge.subscribe();

    let session_id = p
        .bridge
        .open_session(session_config)
        .await
        .map_err(|e| StageError::Bridge(format!("open_session: {e}")))?;

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::SessionOpened {
            node: p.node.clone(),
            session: session_id,
            agent: p.agent_config.profile.to_string(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    // Send the prompt. Phase 6.4 will replace this empty stub with the real
    // binding-resolved prompt.
    let empty_msg = MessageContent::Text(String::new());
    p.bridge
        .send_message(session_id, empty_msg)
        .await
        .map_err(|e| StageError::Bridge(format!("send_message: {e}")))?;

    // Drive the event loop until OutcomeReported (success) or SessionEnded
    // (failure / abnormal termination).
    let outcome = loop {
        let event = match events.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(StageError::Bridge("event stream closed unexpectedly".into()));
            }
        };

        // Filter events for this session only.
        if event_session_id(&event) != Some(session_id) {
            continue;
        }

        match event {
            BridgeEvent::OutcomeReported { outcome, summary, .. } => {
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                        node: p.node.clone(),
                        outcome: outcome.clone(),
                        summary,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                break outcome;
            }
            BridgeEvent::SessionEnded { reason, .. } => {
                let disposition = match &reason {
                    surge_acp::bridge::event::SessionEndReason::Normal => {
                        SessionDisposition::Normal
                    }
                    surge_acp::bridge::event::SessionEndReason::AgentCrashed { .. } => {
                        SessionDisposition::AgentCrashed
                    }
                    surge_acp::bridge::event::SessionEndReason::Timeout { .. } => {
                        SessionDisposition::Timeout
                    }
                    surge_acp::bridge::event::SessionEndReason::ForcedClose => {
                        SessionDisposition::ForcedClose
                    }
                };
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::SessionClosed {
                        session: session_id,
                        disposition,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                return Err(StageError::AgentCrashed(format!(
                    "session ended before OutcomeReported: {reason:?}"
                )));
            }
            // Tool dispatch + token usage + artifact handling come in 6.3.
            _ => continue,
        }
    };

    p.bridge
        .close_session(session_id)
        .await
        .map_err(|e| StageError::Bridge(format!("close_session: {e}")))?;

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::SessionClosed {
            session: session_id,
            disposition: SessionDisposition::Normal,
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(outcome)
}

fn event_session_id(
    event: &surge_acp::bridge::event::BridgeEvent,
) -> Option<surge_core::id::SessionId> {
    use surge_acp::bridge::event::BridgeEvent;
    match event {
        BridgeEvent::SessionEstablished { session, .. } => Some(*session),
        BridgeEvent::AgentMessage { session, .. } => Some(*session),
        BridgeEvent::TokenUsage { session, .. } => Some(*session),
        BridgeEvent::ToolCall { session, .. } => Some(*session),
        BridgeEvent::ToolResult { session, .. } => Some(*session),
        BridgeEvent::OutcomeReported { session, .. } => Some(*session),
        BridgeEvent::HumanInputRequested { session, .. } => Some(*session),
        BridgeEvent::SessionEnded { session, .. } => Some(*session),
        BridgeEvent::Error { session, .. } => *session,
    }
}
