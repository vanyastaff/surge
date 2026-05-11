//! `NodeKind::Agent` execution.
//!
//! Phase 6.2: event loop — opens an ACP session, sends a placeholder empty
//! message, drives `BridgeEvent` until `OutcomeReported` is received (success)
//! or `SessionEnded` fires first (failure).
//! Phase 6.3: tool dispatch for non-injected tools + token usage persistence.
//! Phase 6.4: binding resolution + template substitution for the agent prompt.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::event::ToolResultPayload as AcpResultPayload;
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::agent_config::AgentConfig;
use surge_core::content_hash::ContentHash;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::OutcomeDecl;
use surge_core::run_event::{EventPayload, SessionDisposition, VersionedEventPayload};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::run_writer::RunWriter;

use surge_core::hooks::{Hook, HookTrigger};

use crate::engine::hooks::{HookContext, HookExecutor, HookOutcome, record_hook_executed};
use crate::engine::sandbox_factory::build_sandbox;
use crate::engine::stage::bindings::resolve_bindings;
use crate::engine::stage::{StageError, StageResult};
use crate::engine::tools::{
    ToolCall, ToolDispatchContext, ToolResultPayload as EngineResultPayload,
};
use crate::prompt::PromptRenderer;

/// Parameters for executing a single agent stage.
pub struct AgentStageParams<'a> {
    /// Key of the node being executed (used for tracing; wired to events in 6.2).
    pub node: &'a NodeKey,
    /// Agent node configuration from the spec graph.
    pub agent_config: &'a AgentConfig,
    /// Declared outcomes from the node — used to populate `SessionConfig::declared_outcomes`.
    /// Must be non-empty; `SessionConfig::validate()` enforces this at session-open time.
    pub declared_outcomes: &'a [OutcomeDecl],
    /// Bridge facade for ACP session lifecycle.
    pub bridge: &'a Arc<dyn BridgeFacade>,
    /// Run writer (events emitted here in Phase 6.2+).
    pub writer: &'a RunWriter,
    /// Content-addressed artifact store for canonical run artifacts.
    pub artifact_store: &'a ArtifactStore,
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
    pub tool_resolutions: &'a std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>,
        >,
    >,
    /// Timeout for `request_human_input` calls. Sourced from `EngineRunConfig`.
    pub human_input_timeout: std::time::Duration,
    /// Optional MCP registry. When `Some`, the stage wraps `tool_dispatcher`
    /// with `RoutingToolDispatcher` to expose MCP tools to the agent.
    pub mcp_registry: Option<std::sync::Arc<surge_mcp::McpRegistry>>,
    /// Run-level server list. Each entry maps a server name to its timeout
    /// and allowed-tools filter for this session's `RoutingToolDispatcher`.
    pub mcp_servers: Vec<surge_core::mcp_config::McpServerRef>,
    /// Optional profile registry. When `Some`, the stage resolves
    /// `agent_config.profile` through it to derive `AgentKind` from the
    /// merged profile's `runtime.agent_id`. When `None`, the legacy M5
    /// mock-only fast path remains active.
    pub profile_registry: Option<std::sync::Arc<crate::profile_loader::ProfileRegistry>>,
    /// Lifecycle-hook executor. The default `HookExecutor::new()` runs hooks
    /// via the OS shell; tests substitute via `HookExecutor::with_spawner`.
    pub hook_executor: &'a HookExecutor,
}

/// Merge profile-level hooks with node-level hooks for one effective agent run.
///
/// Profile hooks run first. A node hook with the same `id` replaces the
/// profile hook in-place, giving per-node config the final say without losing
/// deterministic ordering.
#[must_use]
pub(crate) fn effective_agent_hooks(
    agent_config: &AgentConfig,
    resolved_profile: Option<&surge_core::profile::registry::ResolvedProfile>,
) -> Vec<Hook> {
    let mut hooks = resolved_profile
        .map(|profile| profile.profile.hooks.entries.clone())
        .unwrap_or_default();

    for node_hook in &agent_config.hooks {
        if let Some(existing) = hooks.iter_mut().find(|hook| hook.id == node_hook.id) {
            *existing = node_hook.clone();
        } else {
            hooks.push(node_hook.clone());
        }
    }

    hooks
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
/// # Artifact event emission
/// `BridgeEvent::OutcomeReported` carries `artifacts_produced: Vec<String>`.
/// For each declared path the engine resolves it against the run's worktree,
/// computes a `ContentHash` of the file contents, and appends one
/// `EventPayload::ArtifactProduced` event **before** the `OutcomeReported`
/// event so the standard fold rule populates `RunMemory.artifacts` /
/// `RunMemory.artifacts_by_node` deterministically. A missing or unreadable
/// path is logged at WARN and skipped — it does not fail the stage.
///
/// Phase 6.4 wires prompt/bindings.
///
/// # Errors
/// Returns [`StageError::Bridge`] if any bridge call fails.
/// Returns [`StageError::AgentCrashed`] if the session ends without reporting
/// an outcome.
/// Returns [`StageError::Storage`] if event persistence fails.
#[allow(clippy::too_many_lines)]
pub async fn execute_agent_stage(p: AgentStageParams<'_>) -> StageResult {
    // Phase 6.4: resolve bindings and prompt BEFORE building SessionConfig so
    // we can wire them into the config (not just the first message).
    let resolved_bindings =
        resolve_bindings(&p.agent_config.bindings, p.run_memory, p.worktree_path)
            .await
            .map_err(|e| StageError::Internal(format!("binding resolution: {e}")))?;

    // Resolve the profile once (when a registry is wired) so we can use
    // its `runtime.agent_id` to derive `AgentKind` AND fall back to its
    // `prompt.system` when the agent_config does not supply an override.
    // Without a registry, both paths use their legacy fallbacks.
    let profile_str = p.agent_config.profile.as_ref();
    let resolved_profile = if let Some(reg) = p.profile_registry.as_deref() {
        let key_ref = surge_core::profile::keyref::parse_key_ref(profile_str).map_err(|e| {
            StageError::Internal(format!("invalid profile reference {profile_str:?}: {e}"))
        })?;
        Some(
            reg.resolve(&key_ref)
                .map_err(|e| StageError::Internal(format!("profile resolve failed: {e}")))?,
        )
    } else {
        None
    };
    let effective_hooks = effective_agent_hooks(p.agent_config, resolved_profile.as_ref());

    // Prompt selection: explicit prompt_overrides.system wins; then
    // prompt_overrides.append_system; then the resolved profile's
    // prompt.system; then empty string (legacy fallback).
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
        .or_else(|| {
            resolved_profile
                .as_ref()
                .map(|r| r.profile.prompt.system.as_str())
        })
        .unwrap_or("");
    // Lenient at runtime: missing bindings render as empty strings rather
    // than failing the stage. Strict-mode validation runs at
    // `ProfileRegistry::load` so bundled / disk profiles are caught at
    // startup; runtime forgiveness keeps the engine from blowing up over
    // optional-binding edge cases the profile schema authorizes.
    let renderer = PromptRenderer::lenient();
    let prompt_text = renderer
        .render(prompt_template, &resolved_bindings)
        .map_err(|e| StageError::Internal(format!("prompt render: {e}")))?;

    // Derive AgentKind. With a resolved profile in hand, take the agent_id
    // from its runtime block; otherwise fall through to the legacy mock
    // fast path so callers without a registry keep working.
    let agent_kind = match resolved_profile.as_ref() {
        Some(rp) => derive_agent_kind_from_id(profile_str, rp.profile.runtime.agent_id.as_str())?,
        None => derive_agent_kind(profile_str, None)?,
    };

    // Derive declared outcomes from the node's OutcomeDecl list.
    // Fall back to ["done"] when the node has no declared_outcomes so the
    // session is always valid (SessionConfig::validate requires at least one).
    let declared_outcomes: Vec<OutcomeKey> = if p.declared_outcomes.is_empty() {
        vec![OutcomeKey::try_from("done").expect("'done' is a valid OutcomeKey")]
    } else {
        p.declared_outcomes.iter().map(|d| d.id.clone()).collect()
    };

    // Derive allows_escalation from approvals_override.
    let allows_escalation = p
        .agent_config
        .approvals_override
        .as_ref()
        .is_some_and(|a| a.elevation && !a.elevation_channels.is_empty());

    // Cap bindings at 8 entries × 64 bytes (SessionConfig::validate limit).
    // Use the resolved binding values for correlation in BridgeEvent::SessionEstablished.
    let session_bindings: BTreeMap<String, String> = resolved_bindings
        .iter()
        .take(8)
        .filter(|(k, v)| k.0.len() <= 64 && v.len() <= 64)
        .map(|(k, v)| (k.0.clone(), v.clone()))
        .collect();

    // Build the effective sandbox config (needed both for session creation and
    // for the MCP sandbox heuristic below).
    let sandbox_cfg: surge_core::sandbox::SandboxConfig =
        p.agent_config.sandbox_override.clone().unwrap_or_default();

    // Build the session-scoped tool dispatcher. When an MCP registry is
    // configured, wrap the engine dispatcher with RoutingToolDispatcher so the
    // agent sees both engine built-ins and the per-stage MCP allowlist.
    let session_dispatcher: Arc<dyn crate::engine::tools::ToolDispatcher> = if let Some(ref reg) =
        p.mcp_registry
    {
        // Per-stage MCP server allowlist from ToolOverride::mcp_add.
        let allowed_servers: std::collections::HashSet<&str> = p
            .agent_config
            .tool_overrides
            .as_ref()
            .map(|o| o.mcp_add.iter().map(String::as_str).collect())
            .unwrap_or_default();

        // Short-circuit: stage doesn't expose any MCP servers, so skip
        // the potentially expensive list_all_tools call entirely.
        if allowed_servers.is_empty() {
            p.tool_dispatcher.clone()
        } else {
            let all_mcp_tools = match reg.list_all_tools().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        err = %e,
                        "MCP list_all_tools failed; proceeding with engine tools only"
                    );
                    Vec::new()
                },
            };

            // Build per-server `allowed_tools` lookup from the run-level
            // registry. `None` means "expose all tools the server reports".
            let allowed_tools_per_server: std::collections::HashMap<&str, Option<&[String]>> = p
                .mcp_servers
                .iter()
                .map(|s| (s.name.as_str(), s.allowed_tools.as_deref()))
                .collect();

            let filtered: Vec<surge_mcp::McpToolEntry> = all_mcp_tools
                .into_iter()
                .filter(|t| {
                    if !allowed_servers.contains(t.server.as_str()) {
                        return false;
                    }
                    if !sandbox_allows_mcp_tool(&sandbox_cfg, &t.server, &t.tool) {
                        return false;
                    }
                    // Per-server allowed_tools whitelist: outer Some = entry
                    // exists in the HashMap; inner Some = the field is set.
                    // If allowed_tools is None, no filtering is applied.
                    if let Some(Some(whitelist)) = allowed_tools_per_server.get(t.server.as_str()) {
                        if !whitelist.iter().any(|w| w == &t.tool) {
                            return false;
                        }
                    }
                    true
                })
                .collect();

            // Per-server timeout map from the run-level McpServerRef list.
            let timeouts: std::collections::HashMap<String, std::time::Duration> = p
                .mcp_servers
                .iter()
                .map(|s| (s.name.clone(), s.call_timeout))
                .collect();

            Arc::new(crate::engine::tools::RoutingToolDispatcher::new(
                p.tool_dispatcher.clone(),
                reg.clone(),
                &filtered,
                &timeouts,
            )) as Arc<dyn crate::engine::tools::ToolDispatcher>
        }
    } else {
        p.tool_dispatcher.clone()
    };

    // Assemble the ACP tool list from the session dispatcher's declared catalog.
    // Use ToolCategory::Builtin for all caller-supplied tools (both engine
    // built-ins and MCP tools). Injected engine tools (report_stage_outcome,
    // request_human_input) are added separately by the bridge.
    let session_tools: Vec<surge_acp::bridge::tools::ToolDef> = session_dispatcher
        .declared_tools()
        .into_iter()
        .map(|t| surge_acp::bridge::tools::ToolDef {
            name: t.name,
            description: t.description.unwrap_or_default(),
            category: surge_acp::bridge::tools::ToolCategory::Builtin,
            input_schema: t.input_schema,
        })
        .collect();

    // Build SessionConfig from derived values.
    let sandbox = build_sandbox(Some(&sandbox_cfg));
    let session_config = SessionConfig {
        agent_kind,
        working_dir: p.worktree_path.to_path_buf(),
        system_prompt: prompt_text.clone(),
        declared_outcomes,
        allows_escalation,
        tools: session_tools,
        sandbox,
        permission_policy: PermissionPolicy::default(),
        bindings: session_bindings,
    };

    // Subscribe to events BEFORE opening the session, so we don't miss the
    // SessionEstablished event (or earlier ToolCall events).
    let mut events = p.bridge.subscribe();

    // Track outcomes rejected by `on_outcome` hooks within THIS session so we
    // can surface `StageFailed` once the agent burns its retry budget.
    let max_outcome_rejections: u32 = p.agent_config.limits.max_retries;
    let mut outcome_rejection_attempts: u32 = 0;

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
                return Err(StageError::Bridge(
                    "event stream closed unexpectedly".into(),
                ));
            },
        };

        // Filter events for this session only.
        if event_session_id(&event) != Some(session_id) {
            continue;
        }

        match event {
            BridgeEvent::OutcomeReported {
                outcome,
                summary,
                artifacts_produced,
                ..
            } => {
                // on_outcome hook chain runs BEFORE OutcomeReported is persisted.
                // A rejecting hook lets the agent attempt a different outcome
                // until `limits.max_retries` is exhausted.
                let hook_ctx = HookContext::for_node(p.node)
                    .with_worktree_path(p.worktree_path)
                    .with_session(session_id)
                    .with_outcome(&outcome);
                let outcome_chain = p
                    .hook_executor
                    .run_hooks(&effective_hooks, HookTrigger::OnOutcome, &hook_ctx)
                    .await;
                for record in outcome_chain.executed() {
                    record_hook_executed(p.writer, record).await;
                }

                if let HookOutcome::Reject {
                    reason, hook_id, ..
                } = &outcome_chain
                {
                    p.writer
                        .append_event(VersionedEventPayload::new(
                            EventPayload::OutcomeRejectedByHook {
                                node: p.node.clone(),
                                outcome: outcome.clone(),
                                hook_id: hook_id.clone(),
                            },
                        ))
                        .await
                        .map_err(|e| StageError::Storage(e.to_string()))?;

                    outcome_rejection_attempts += 1;
                    tracing::info!(
                        target: "engine::stage::agent",
                        node = %p.node,
                        outcome = %outcome,
                        hook_id = %hook_id,
                        attempt = outcome_rejection_attempts,
                        max = max_outcome_rejections,
                        reason = %reason,
                        "on_outcome hook rejected outcome; awaiting agent retry"
                    );

                    if outcome_rejection_attempts > max_outcome_rejections {
                        let exhausted_reason = format!(
                            "on_outcome rejection budget exhausted (last reject from '{hook_id}')"
                        );
                        p.writer
                            .append_event(VersionedEventPayload::new(EventPayload::StageFailed {
                                node: p.node.clone(),
                                reason: exhausted_reason.clone(),
                                retry_available: false,
                            }))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        // Close the session before bailing — the agent isn't
                        // going to recover at this point.
                        let _ = p.bridge.close_session(session_id).await;
                        return Err(StageError::AgentCrashed(exhausted_reason));
                    }
                    continue;
                }

                // Task 30: emit one ArtifactProduced event per declared
                // path BEFORE the OutcomeReported event so the standard
                // fold rule populates RunMemory.artifacts deterministically.
                // A missing or unreadable path is logged and skipped — it
                // does not fail the stage.
                if !artifacts_produced.is_empty() {
                    let canonical_worktree = tokio::fs::canonicalize(p.worktree_path)
                        .await
                        .map_err(|e| StageError::Storage(e.to_string()))?;
                    let mut emitted_names = BTreeSet::new();
                    for declared_path in &artifacts_produced {
                        let Some(relative_path) = safe_declared_artifact_path(declared_path) else {
                            tracing::warn!(
                                target: "engine::stage::agent",
                                node = %p.node,
                                path = %declared_path,
                                "artifact path escapes worktree — skipping"
                            );
                            continue;
                        };
                        let absolute = p.worktree_path.join(&relative_path);
                        let canonical_path = match tokio::fs::canonicalize(&absolute).await {
                            Ok(path) => path,
                            Err(e) => {
                                tracing::warn!(
                                    target: "engine::stage::agent",
                                    node = %p.node,
                                    path = %declared_path,
                                    err = %e,
                                    "artifact path missing — skipping"
                                );
                                continue;
                            },
                        };
                        if !canonical_path.starts_with(&canonical_worktree) {
                            tracing::warn!(
                                target: "engine::stage::agent",
                                node = %p.node,
                                path = %declared_path,
                                canonical_path = %canonical_path.display(),
                                "artifact path escapes worktree — skipping"
                            );
                            continue;
                        }
                        let bytes = match tokio::fs::read(&canonical_path).await {
                            Ok(b) => b,
                            Err(e) => {
                                tracing::warn!(
                                    target: "engine::stage::agent",
                                    node = %p.node,
                                    path = %declared_path,
                                    err = %e,
                                    "artifact path missing — skipping"
                                );
                                continue;
                            },
                        };
                        let name = relative_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .map_or_else(|| declared_path.clone(), str::to_owned);
                        if !emitted_names.insert(name.clone()) {
                            return Err(StageError::Internal(format!(
                                "duplicate artifact logical name '{name}' from declared path '{declared_path}'"
                            )));
                        }
                        let artifact_ref = p
                            .artifact_store
                            .put(p.run_id, &name, &bytes)
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        tracing::info!(
                            target: "engine::stage::agent",
                            node = %p.node,
                            name = %name,
                            hash = %artifact_ref.hash,
                            store_path = %artifact_ref.path.display(),
                            "artifact_produced"
                        );
                        p.writer
                            .append_event(VersionedEventPayload::new(
                                EventPayload::ArtifactProduced {
                                    node: p.node.clone(),
                                    artifact: artifact_ref.hash,
                                    path: artifact_ref.path,
                                    name,
                                },
                            ))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                    }
                }

                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                        node: p.node.clone(),
                        outcome: outcome.clone(),
                        summary,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                break outcome;
            },
            BridgeEvent::SessionEnded { reason, .. } => {
                let disposition = match &reason {
                    surge_acp::bridge::event::SessionEndReason::Normal => {
                        SessionDisposition::Normal
                    },
                    surge_acp::bridge::event::SessionEndReason::AgentCrashed { .. } => {
                        SessionDisposition::AgentCrashed
                    },
                    surge_acp::bridge::event::SessionEndReason::Timeout { .. } => {
                        SessionDisposition::Timeout
                    },
                    surge_acp::bridge::event::SessionEndReason::ForcedClose => {
                        SessionDisposition::ForcedClose
                    },
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
            },
            BridgeEvent::ToolCall {
                call_id,
                tool,
                args_redacted_json,
                meta,
                ..
            } if !meta.injected => {
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

                // pre_tool_use hooks gate the dispatch. A Reject short-circuits
                // the call: we send a synthetic tool-error reply and continue
                // the agent loop without invoking the dispatcher.
                let hook_ctx = HookContext::for_node(p.node)
                    .with_worktree_path(p.worktree_path)
                    .with_session(session_id)
                    .with_tool(tool.as_str(), Some(args_redacted_json.as_str()));
                let pre_outcome = p
                    .hook_executor
                    .run_hooks(&effective_hooks, HookTrigger::PreToolUse, &hook_ctx)
                    .await;
                for record in pre_outcome.executed() {
                    record_hook_executed(p.writer, record).await;
                }
                if let HookOutcome::Reject {
                    reason, hook_id, ..
                } = &pre_outcome
                {
                    tracing::warn!(
                        target: "engine::stage::agent",
                        node = %p.node,
                        tool = %tool,
                        hook_id = %hook_id,
                        reason = %reason,
                        "pre_tool_use hook rejected; sending tool-error reply"
                    );
                    p.bridge
                        .reply_to_tool(
                            session_id,
                            call_id,
                            AcpResultPayload::Error {
                                message: format!(
                                    "pre_tool_use hook '{hook_id}' rejected call: {reason}"
                                ),
                            },
                        )
                        .await
                        .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                    continue;
                }

                let engine_result = session_dispatcher.dispatch(&ctx, &call).await;

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
                    },
                    EngineResultPayload::Error { message }
                    | EngineResultPayload::Unsupported { message } => {
                        ContentHash::compute(message.as_bytes())
                    },
                    EngineResultPayload::Cancelled => ContentHash::compute(b"cancelled"),
                };
                p.writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::ToolResultReceived {
                            session: session_id,
                            success,
                            result: result_hash,
                        },
                    ))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                // Convert engine payload → ACP payload and reply.
                let acp_result = match engine_result {
                    EngineResultPayload::Ok { content } => AcpResultPayload::Ok {
                        result_json: content.to_string(),
                    },
                    EngineResultPayload::Error { message } => AcpResultPayload::Error { message },
                    EngineResultPayload::Unsupported { message: _ } => {
                        AcpResultPayload::Unsupported
                    },
                    EngineResultPayload::Cancelled => AcpResultPayload::Error {
                        message: "cancelled".into(),
                    },
                };
                p.bridge
                    .reply_to_tool(session_id, call_id, acp_result)
                    .await
                    .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;

                // post_tool_use cannot un-run the call. Record execution and
                // log Reject as a warning; the agent has already received the
                // result above.
                let post_outcome = p
                    .hook_executor
                    .run_hooks(&effective_hooks, HookTrigger::PostToolUse, &hook_ctx)
                    .await;
                for record in post_outcome.executed() {
                    record_hook_executed(p.writer, record).await;
                }
                if let HookOutcome::Reject {
                    reason, hook_id, ..
                } = &post_outcome
                {
                    tracing::warn!(
                        target: "engine::stage::agent",
                        node = %p.node,
                        tool = %tool,
                        hook_id = %hook_id,
                        reason = %reason,
                        "post_tool_use hook rejected (cannot un-run; logged for audit)"
                    );
                }
            },
            BridgeEvent::TokenUsage {
                prompt_tokens,
                output_tokens,
                cache_hits,
                model,
                ..
            } => {
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
            },
            BridgeEvent::HumanInputRequested {
                call_id,
                question,
                context,
                ..
            } => {
                let prompt = match &context {
                    Some(ctx) => format!("{question}\n\n{ctx}"),
                    None => question.clone(),
                };

                p.writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::HumanInputRequested {
                            node: p.node.clone(),
                            session: Some(session_id),
                            call_id: Some(call_id.clone()),
                            prompt,
                            schema: None,
                        },
                    ))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let (tx, rx) = tokio::sync::oneshot::channel();
                p.tool_resolutions.lock().await.insert(call_id.clone(), tx);

                let resolved = tokio::select! {
                    response = rx => match response {
                        Ok(v) => Some(v),
                        Err(_) => None, // sender dropped (run aborted)
                    },
                    () = tokio::time::sleep(p.human_input_timeout) => None,
                };

                p.tool_resolutions.lock().await.remove(&call_id);

                if let Some(response) = resolved {
                    p.writer
                        .append_event(VersionedEventPayload::new(
                            EventPayload::HumanInputResolved {
                                node: p.node.clone(),
                                call_id: Some(call_id.clone()),
                                response: response.clone(),
                            },
                        ))
                        .await
                        .map_err(|e| StageError::Storage(e.to_string()))?;
                    p.bridge
                        .reply_to_tool(
                            session_id,
                            call_id,
                            AcpResultPayload::Ok {
                                result_json: response.to_string(),
                            },
                        )
                        .await
                        .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                } else {
                    p.writer
                        .append_event(VersionedEventPayload::new(
                            EventPayload::HumanInputTimedOut {
                                node: p.node.clone(),
                                call_id: Some(call_id.clone()),
                                elapsed_seconds: u32::try_from(p.human_input_timeout.as_secs())
                                    .unwrap_or(u32::MAX),
                            },
                        ))
                        .await
                        .map_err(|e| StageError::Storage(e.to_string()))?;
                    p.bridge
                        .reply_to_tool(
                            session_id,
                            call_id,
                            AcpResultPayload::Error {
                                message: "human input timed out".into(),
                            },
                        )
                        .await
                        .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                    // M5 fail-fast: timeout halts the stage.
                    return Err(StageError::HumanGateRejected);
                }
            },
            _ => {},
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

fn safe_declared_artifact_path(declared_path: &str) -> Option<PathBuf> {
    if declared_path.trim().is_empty() {
        return None;
    }

    let path = Path::new(declared_path);
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {},
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    has_normal_component.then(|| path.to_path_buf())
}

fn event_session_id(event: &BridgeEvent) -> Option<surge_core::id::SessionId> {
    match event {
        BridgeEvent::SessionEstablished { session, .. }
        | BridgeEvent::AgentMessage { session, .. }
        | BridgeEvent::TokenUsage { session, .. }
        | BridgeEvent::ToolCall { session, .. }
        | BridgeEvent::ToolResult { session, .. }
        | BridgeEvent::OutcomeReported { session, .. }
        | BridgeEvent::HumanInputRequested { session, .. }
        | BridgeEvent::SessionEnded { session, .. } => Some(*session),
        BridgeEvent::Error { session, .. } => *session,
    }
}

/// Resolve `profile_str` (the value of `AgentConfig::profile`) into an
/// `AgentKind` using the profile registry, with the M5 mock fast path as
/// the documented fallback when no registry is wired.
///
/// Order:
/// 1. If `profile_str` is `"mock"` or `"mock@..."` and the registry is
///    `None`, short-circuit to `AgentKind::Mock`. Preserves legacy tests
///    that build the engine without a registry.
/// 2. If a registry is supplied, parse the reference, resolve through the
///    full disk + bundled chain, take `merged.runtime.agent_id`, and map
///    that id to a concrete `AgentKind` via `surge_acp::Registry::builtin`.
///    Unknown agent ids surface a `StageError::Internal` rather than a
///    silent mock.
/// 3. If no registry is supplied AND the profile is non-mock, also fall
///    back to `AgentKind::Mock` with a one-time WARN log so the test path
///    keeps working but production wiring is still encouraged.
fn derive_agent_kind(
    profile_str: &str,
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
) -> Result<AgentKind, StageError> {
    // Step 1 / step 3 fallback path: no registry wired.
    if profile_registry.is_none() {
        if profile_str != "mock" && !profile_str.starts_with("mock@") {
            tracing::warn!(
                target: "engine::stage::agent",
                profile = %profile_str,
                "no profile_registry wired; falling back to AgentKind::Mock (legacy M5 path)"
            );
        }
        return Ok(AgentKind::Mock { args: vec![] });
    }
    let registry = profile_registry.expect("checked Some above");

    // Step 2: registry-driven resolution.
    let key_ref = surge_core::profile::keyref::parse_key_ref(profile_str).map_err(|e| {
        StageError::Internal(format!("invalid profile reference {profile_str:?}: {e}"))
    })?;
    let resolved = registry
        .resolve(&key_ref)
        .map_err(|e| StageError::Internal(format!("profile resolve failed: {e}")))?;

    derive_agent_kind_from_id(profile_str, resolved.profile.runtime.agent_id.as_str())
}

/// Map an `agent_id` string to an `AgentKind` via `surge_acp::Registry`.
///
/// Pulled out so [`execute_agent_stage`] can call it directly when the
/// caller already resolved the profile and just needs the id translated.
fn derive_agent_kind_from_id(profile_str: &str, agent_id: &str) -> Result<AgentKind, StageError> {
    if agent_id.is_empty() {
        return Err(StageError::Internal(format!(
            "profile {profile_str:?} has empty runtime.agent_id"
        )));
    }
    if agent_id == "mock" {
        return Ok(AgentKind::Mock { args: vec![] });
    }
    let agent_registry = surge_acp::Registry::builtin();
    let normalized_agent_id = agent_registry.normalize_agent_id(agent_id).ok_or_else(|| {
        let known = agent_registry.known_ids_and_aliases().join(", ");
        tracing::warn!(
            target: "engine::stage::agent",
            profile = %profile_str,
            agent_id = %agent_id,
            known = %known,
            "unknown profile runtime agent id"
        );
        StageError::Internal(format!(
            "profile {profile_str:?} references agent_id {agent_id:?} not present in surge_acp::Registry. Known ids and aliases: {known}"
        ))
    })?;
    let entry = agent_registry.find(&normalized_agent_id).ok_or_else(|| {
        StageError::Internal(format!(
            "normalized agent_id {normalized_agent_id:?} missing from surge_acp::Registry"
        ))
    })?;
    let binary = std::path::PathBuf::from(&entry.command);
    let extra_args = entry.default_args.clone();
    let kind = match entry.id.as_str() {
        "claude-code" => AgentKind::ClaudeCode { binary, extra_args },
        "codex" => AgentKind::Codex { binary, extra_args },
        "gemini-cli" => AgentKind::GeminiCli { binary, extra_args },
        _ => AgentKind::Custom {
            binary,
            args: extra_args,
        },
    };
    tracing::debug!(
        target: "engine::stage::agent",
        profile = %profile_str,
        agent_id = %agent_id,
        normalized_agent_id = %entry.id,
        kind = kind.label(),
        "derived AgentKind from profile registry"
    );
    Ok(kind)
}

/// Conservative M7 heuristic for whether a sandbox tier permits an
/// MCP server's tool. Will be replaced by M4 (sandbox milestone)
/// proper enforcement.
#[allow(clippy::needless_pass_by_value)]
fn sandbox_allows_mcp_tool(
    sandbox: &surge_core::sandbox::SandboxConfig,
    _server: &str,
    _tool: &str,
) -> bool {
    use surge_core::sandbox::SandboxMode;
    match sandbox.mode {
        SandboxMode::ReadOnly => false,
        // All other modes allow MCP — refine in M4 when sandbox enforcement lands.
        SandboxMode::WorkspaceWrite
        | SandboxMode::WorkspaceNetwork
        | SandboxMode::FullAccess
        | SandboxMode::Custom => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::approvals::ApprovalConfig;
    use surge_core::edge::EdgeKind;
    use surge_core::hooks::{HookFailureMode, HookInheritance, MatcherSpec};
    use surge_core::profile::registry::{Provenance, ResolvedProfile};
    use surge_core::profile::{
        InspectorUi, Profile, ProfileBindings, ProfileHooks, ProfileOutcome, PromptTemplate, Role,
        RoleCategory, RuntimeCfg, ToolsCfg,
    };
    use surge_core::sandbox::SandboxConfig;

    fn hook(id: &str, command: &str) -> Hook {
        Hook {
            id: id.to_string(),
            trigger: HookTrigger::PreToolUse,
            matcher: MatcherSpec::default(),
            command: command.to_string(),
            on_failure: HookFailureMode::Warn,
            timeout_seconds: None,
            inherit: HookInheritance::default(),
        }
    }

    fn agent_config(hooks: Vec<Hook>) -> AgentConfig {
        AgentConfig {
            profile: surge_core::keys::ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: Vec::new(),
            rules_overrides: None,
            limits: surge_core::agent_config::NodeLimits::default(),
            hooks,
            custom_fields: BTreeMap::new(),
        }
    }

    fn resolved_profile(hooks: Vec<Hook>) -> ResolvedProfile {
        let profile_key = surge_core::keys::ProfileKey::try_from("implementer").unwrap();
        ResolvedProfile {
            profile: Profile {
                schema_version: 1,
                role: Role {
                    id: profile_key.clone(),
                    version: semver::Version::new(1, 0, 0),
                    display_name: "Implementer".into(),
                    icon: None,
                    category: RoleCategory::Agents,
                    description: "Implements".into(),
                    when_to_use: "Tests".into(),
                    extends: None,
                },
                runtime: RuntimeCfg {
                    recommended_model: "claude-opus-4-7".into(),
                    default_temperature: 0.2,
                    default_max_tokens: 200_000,
                    load_rules_lazily: None,
                    agent_id: "claude-code".into(),
                },
                sandbox: SandboxConfig::default(),
                tools: ToolsCfg::default(),
                approvals: ApprovalConfig::default(),
                outcomes: vec![ProfileOutcome {
                    id: OutcomeKey::try_from("done").unwrap(),
                    description: "Done".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    required_artifacts: Vec::new(),
                    produced_artifacts: Vec::new(),
                }],
                bindings: ProfileBindings::default(),
                hooks: ProfileHooks { entries: hooks },
                prompt: PromptTemplate {
                    system: "Implement".into(),
                },
                inspector_ui: InspectorUi::default(),
            },
            provenance: Provenance::Bundled,
            chain: vec![profile_key],
        }
    }

    #[test]
    fn effective_hooks_append_node_hooks_after_profile_hooks() {
        let profile = resolved_profile(vec![hook("profile", "profile-cmd")]);
        let agent = agent_config(vec![hook("node", "node-cmd")]);

        let effective = effective_agent_hooks(&agent, Some(&profile));

        assert_eq!(
            effective
                .iter()
                .map(|hook| hook.id.as_str())
                .collect::<Vec<_>>(),
            vec!["profile", "node"]
        );
    }

    #[test]
    fn node_hooks_override_profile_hooks_by_id() {
        let profile = resolved_profile(vec![hook("validate", "profile-cmd")]);
        let agent = agent_config(vec![hook("validate", "node-cmd")]);

        let effective = effective_agent_hooks(&agent, Some(&profile));

        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].command, "node-cmd");
    }
}
