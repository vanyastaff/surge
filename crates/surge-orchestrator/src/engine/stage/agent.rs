//! `NodeKind::Agent` execution.
//!
//! Phase 6.2: event loop — opens an ACP session, sends a placeholder empty
//! message, drives `BridgeEvent` until `OutcomeReported` is received (success)
//! or `SessionEnded` fires first (failure).
//! Phase 6.3: tool dispatch for non-injected tools + token usage persistence.
//! Phase 6.4: binding resolution + template substitution for the agent prompt.

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
    /// Dispatcher for non-injected ACP tool calls (wired in Phase 6.3).
    pub tool_dispatcher: &'a Arc<dyn crate::engine::tools::ToolDispatcher>,
    /// Accumulated run memory (artifacts, outcomes, costs) passed to tool dispatch context.
    pub run_memory: &'a surge_core::run_state::RunMemory,
    /// Identifier of the current run, forwarded to tool dispatch context.
    pub run_id: surge_core::id::RunId,
    /// Map of `call_id → oneshot::Sender<serde_json::Value>` for routing
    /// `Engine::resolve_human_input` replies to the waiting agent stage.
    pub tool_resolutions: &'a std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    /// Timeout for `request_human_input` calls. Sourced from `EngineRunConfig`.
    pub human_input_timeout: std::time::Duration,
}

/// Execute a single agent stage.
///
/// Phase 6.2: opens a session, sends an empty placeholder message, then drives
/// the `BridgeEvent` loop until `BridgeEvent::OutcomeReported` arrives
/// (success) or `BridgeEvent::SessionEnded` fires without a prior outcome
/// (failure).
///
/// Phase 6.3: dispatches non-injected `BridgeEvent::ToolCall` events through
/// the `ToolDispatcher`, persists `ToolCalled`/`ToolResultReceived` events, and
/// replies to the agent via `bridge.reply_to_tool`. Also persists
/// `TokensConsumed` events from `BridgeEvent::TokenUsage`.
///
/// # Known M5 limitation — artifact events
/// `BridgeEvent::OutcomeReported` carries `artifacts_produced: Vec<String>`,
/// but there is no separate `BridgeEvent::ArtifactProduced` variant in M3.
/// Artifacts are therefore noted in the `OutcomeReported` event but are NOT
/// stored as individual `EventPayload::ArtifactProduced` events in this phase.
/// TODO(M5): emit `EventPayload::ArtifactProduced` for each path in
///   `artifacts_produced` when handling `BridgeEvent::OutcomeReported`.
///
/// Phase 6.4 wires prompt/bindings.
///
/// # Errors
/// Returns [`StageError::Bridge`] if any bridge call fails.
/// Returns [`StageError::AgentCrashed`] if the session ends without reporting
/// an outcome.
/// Returns [`StageError::Storage`] if event persistence fails.
pub async fn execute_agent_stage(p: AgentStageParams<'_>) -> StageResult {
    use surge_acp::bridge::event::BridgeEvent;
    use surge_core::run_event::{EventPayload, SessionDisposition, VersionedEventPayload};

    // Build SessionConfig. Bindings + prompt are resolved below (Phase 6.4).
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

    use crate::engine::stage::bindings::{resolve_bindings, substitute_template};

    let resolved_bindings = resolve_bindings(
        &p.agent_config.bindings,
        p.run_memory,
        p.worktree_path,
    )
    .await
    .map_err(|e| StageError::Internal(format!("binding resolution: {e}")))?;

    let prompt_template = p
        .agent_config
        .prompt_overrides
        .as_ref()
        .and_then(|po| po.system.as_deref())
        .or_else(|| {
            p.agent_config
                .prompt_overrides
                .as_ref()
                .and_then(|po| po.append_system.as_deref())
        })
        .unwrap_or("");
    let prompt_text = substitute_template(prompt_template, &resolved_bindings);

    let prompt_msg = MessageContent::Text(prompt_text);
    p.bridge
        .send_message(session_id, prompt_msg)
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
            BridgeEvent::ToolCall { call_id, tool, args_redacted_json, meta, .. }
                if !meta.injected =>
            {
                use crate::engine::tools::{ToolCall, ToolDispatchContext, ToolResultPayload as EngineResultPayload};
                use surge_acp::bridge::event::ToolResultPayload as AcpResultPayload;
                use surge_core::content_hash::ContentHash;

                // Parse args from JSON for the dispatcher.
                let arguments: serde_json::Value =
                    serde_json::from_str(&args_redacted_json).unwrap_or(serde_json::Value::Null);

                let call = ToolCall {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    arguments,
                };
                let ctx = ToolDispatchContext {
                    run_id: p.run_id,
                    session_id,
                    worktree_root: p.worktree_path,
                    run_memory: p.run_memory,
                };
                let engine_result = p.tool_dispatcher.dispatch(&ctx, &call).await;

                // Persist ToolCalled + ToolResultReceived.
                let args_redacted_hash = ContentHash::compute(args_redacted_json.as_bytes());
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ToolCalled {
                        session: session_id,
                        tool: tool.clone(),
                        args_redacted: args_redacted_hash,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let success = matches!(engine_result, EngineResultPayload::Ok { .. });
                let result_hash = match &engine_result {
                    EngineResultPayload::Ok { content } => {
                        ContentHash::compute(content.to_string().as_bytes())
                    }
                    EngineResultPayload::Error { message } => {
                        ContentHash::compute(message.as_bytes())
                    }
                    EngineResultPayload::Unsupported { message } => {
                        ContentHash::compute(message.as_bytes())
                    }
                    EngineResultPayload::Cancelled => ContentHash::compute(b"cancelled"),
                };
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ToolResultReceived {
                        session: session_id,
                        success,
                        result: result_hash,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                // Convert engine payload → ACP payload and reply.
                let acp_result = match engine_result {
                    EngineResultPayload::Ok { content } => {
                        AcpResultPayload::Ok { result_json: content.to_string() }
                    }
                    EngineResultPayload::Error { message } => AcpResultPayload::Error { message },
                    EngineResultPayload::Unsupported { message: _ } => AcpResultPayload::Unsupported,
                    EngineResultPayload::Cancelled => {
                        AcpResultPayload::Error { message: "cancelled".into() }
                    }
                };
                p.bridge
                    .reply_to_tool(session_id, call_id, acp_result)
                    .await
                    .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
            }
            BridgeEvent::TokenUsage { prompt_tokens, output_tokens, cache_hits, model, .. } => {
                // cost_usd is not carried by BridgeEvent::TokenUsage in M3 —
                // a future layer can compute it from token counts + model name.
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::TokensConsumed {
                        session: session_id,
                        prompt_tokens,
                        output_tokens,
                        cache_hits,
                        model,
                        cost_usd: None,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
            }
            BridgeEvent::HumanInputRequested { call_id, question, context, .. } => {
                use surge_acp::bridge::event::ToolResultPayload as AcpResultPayload;

                let prompt = match &context {
                    Some(ctx) => format!("{question}\n\n{ctx}"),
                    None => question.clone(),
                };

                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::HumanInputRequested {
                        node: p.node.clone(),
                        session: Some(session_id),
                        call_id: Some(call_id.clone()),
                        prompt,
                        schema: None,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let (tx, rx) = tokio::sync::oneshot::channel();
                p.tool_resolutions.lock().await.insert(call_id.clone(), tx);

                let resolved = tokio::select! {
                    response = rx => match response {
                        Ok(v) => Some(v),
                        Err(_) => None, // sender dropped (run aborted)
                    },
                    _ = tokio::time::sleep(p.human_input_timeout) => None,
                };

                p.tool_resolutions.lock().await.remove(&call_id);

                match resolved {
                    Some(response) => {
                        p.writer
                            .append_event(VersionedEventPayload::new(EventPayload::HumanInputResolved {
                                node: p.node.clone(),
                                call_id: Some(call_id.clone()),
                                response: response.clone(),
                            }))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        p.bridge
                            .reply_to_tool(
                                session_id,
                                call_id,
                                AcpResultPayload::Ok { result_json: response.to_string() },
                            )
                            .await
                            .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                    }
                    None => {
                        p.writer
                            .append_event(VersionedEventPayload::new(EventPayload::HumanInputTimedOut {
                                node: p.node.clone(),
                                call_id: Some(call_id.clone()),
                                elapsed_seconds: p.human_input_timeout.as_secs() as u32,
                            }))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        p.bridge
                            .reply_to_tool(
                                session_id,
                                call_id,
                                AcpResultPayload::Error { message: "human input timed out".into() },
                            )
                            .await
                            .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                        // M5 fail-fast: timeout halts the stage.
                        return Err(StageError::HumanGateRejected);
                    }
                }
            }
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
