//! `NodeKind::Agent` execution.
//!
//! Phase 6.2: event loop — opens an ACP session, sends a placeholder empty
//! message, drives `BridgeEvent` until `OutcomeReported` is received (success)
//! or `SessionEnded` fires first (failure).
//! Phase 6.3: tool dispatch for non-injected tools + token usage persistence.
//! Phase 6.4: binding resolution + template substitution for the agent prompt.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::event::ToolResultPayload as AcpResultPayload;
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::agent_config::{AgentConfig, ArtifactSource};
use surge_core::content_hash::ContentHash;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::OutcomeDecl;
use surge_core::profile::registry::ResolvedProfile;
use surge_core::roadmap::RoadmapTaskId;
use surge_core::run_event::{
    EscalationCause, EventPayload, SessionDisposition, VersionedEventPayload,
};
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
use crate::guard::LoopGuardTrip;
use crate::prompt::PromptRenderer;

/// Parameters for executing a single agent stage.
pub struct AgentStageParams<'a> {
    /// One-shot opening authority for a task-owned fallback candidate.
    pub quota_opening: Option<surge_persistence::work_items::recovery_cycles::QuotaOpenPermit>,
    /// Recovery state carried when this stage is being retried on a frozen fallback.
    pub quota_cycle: Option<crate::engine::quota_recovery::TaskQuotaCycle>,
    /// Persistent-task quota owner and the frozen policy for this agent node.
    /// Ordinary runs have no task-owned quota cycle.
    pub quota_owner: Option<crate::engine::quota_recovery::TaskQuotaOwner>,
    /// Current host-authorized continuation, distinct from the attempt binding.
    pub continuation: Option<surge_core::execution_recovery::WorkItemExecutionControl>,
    /// Active execution frames supplying the current loop items to the agent.
    pub frames: &'a [crate::engine::frames::Frame],
    /// Run cancellation, observed between durable writes and during approval waits.
    pub cancel: tokio_util::sync::CancellationToken,
    /// Key of the node being executed (used for tracing; wired to events in 6.2).
    pub node: &'a NodeKey,
    /// Current durable stage-attempt number from the run cursor.
    pub attempt: u32,
    /// Operator steer messages drained for this stage. When non-empty they are
    /// prepended to the prompt and each is recorded via a `SteerDelivered`
    /// event (Phase 2 B2). Empty on the common path.
    pub steers: Vec<crate::engine::steer::QueuedSteer>,
    /// Agent node configuration from the spec graph.
    pub agent_config: &'a AgentConfig,
    /// Skills already resolved and trust-gated for this node
    /// (`engine::stage::skill_binding::bind_skills`, called by the caller
    /// before this stage — R10: bound exactly like a context `Binding`,
    /// never re-resolved here). Each entry's `instructions` is appended to
    /// the system prompt below, once, before `SessionConfig` is built —
    /// this is the only place a bound skill's content reaches the agent.
    pub bound_skills: &'a [crate::engine::stage::skill_binding::BoundSkill],
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
    /// Repeat-tool-call / node-wall-clock guard thresholds
    /// (`.autopilot/competitive-waves/spec.md` §15), sourced from
    /// `EngineRunConfig::tool_call_loop_guard` — itself seeded from
    /// `SurgeConfig::tool_call_loop_guard` via
    /// `crate::project_context::with_project_context_seed`. Applied to
    /// every agent stage's `RoutingToolDispatcher`, not only nodes that
    /// declare an `mcp_add` override.
    pub tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig,
    /// Output-spill cap (§16), sourced the same way as
    /// `tool_call_loop_guard` above.
    pub output_spill: surge_core::spill_config::OutputSpillConfig,
    /// Optional profile registry. When `Some`, the stage resolves
    /// `agent_config.profile` through it to derive `AgentKind` from the
    /// merged profile's `runtime.agent_id`. When `None`, the legacy M5
    /// mock-only fast path remains active.
    pub profile_registry: Option<std::sync::Arc<crate::profile_loader::ProfileRegistry>>,
    /// Unified agent registry (user `[agents.*]` merged over the builtin
    /// catalog). `None` falls back to `Registry::builtin()`. This is the
    /// one place a provider is chosen: an entry's `command`/`args`/`env`/
    /// `settings_files` are the whole launch contract, so a custom provider
    /// needs no code.
    pub agent_registry: Option<std::sync::Arc<surge_acp::Registry>>,
    /// Lifecycle-hook executor. The default `HookExecutor::new()` runs hooks
    /// via the OS shell; tests substitute via `HookExecutor::with_spawner`.
    pub hook_executor: &'a HookExecutor,
    /// Engine-side tracker for in-flight ACP elevation requests. Populated
    /// when [`surge_acp::bridge::event::BridgeEvent::PermissionRequested`]
    /// arrives; drained by the decision router (Task 8) when the operator
    /// replies. Shared because multiple agent stages may run concurrently
    /// against the same bridge.
    pub pending_elevations: std::sync::Arc<crate::engine::elevation::PendingElevations>,
    /// Ledger task id for the loop iteration this stage runs inside, when the
    /// stage executes within a task loop. When `Some` and the reported outcome
    /// carries a [`LedgerEffect`](surge_core::node::LedgerEffect), the stage
    /// emits the matching task-ledger event. `None` outside a task loop.
    pub active_task_id: Option<RoadmapTaskId>,
}

/// Largest `verification-report` read back for re-entry feedback.
const FEEDBACK_REPORT_MAX_BYTES: usize = 256 * 1024;
/// Per-field cap so one long note cannot crowd out the stage prompt.
const FEEDBACK_FIELD_MAX_CHARS: usize = 2_000;
/// Checks listed in re-entry feedback; the rest are counted.
const FEEDBACK_MAX_CHECKS: usize = 20;

/// Why the previous attempt of this node was sent back, when it was re-entered
/// through a backtrack edge: the sending node's outcome and summary, and the
/// `verification-report` it sealed for that outcome, if any.
async fn previous_attempt_feedback(p: &AgentStageParams<'_>) -> Option<String> {
    let entry = p.run_memory.backtrack_feedback.get(p.node)?;
    let records = p.run_memory.outcomes.get(&entry.from);
    let mut sent = records
        .into_iter()
        .flatten()
        .filter(|record| record.seq < entry.edge_seq);
    let outcome = sent.next_back();
    let earlier = sent.next_back().map_or(0, |record| record.seq);
    let report_ref = p
        .run_memory
        .artifacts_by_node
        .get(&entry.from)
        .into_iter()
        .flatten()
        .rfind(|artifact| {
            artifact.name == "verification-report"
                && artifact.produced_at_seq > earlier
                && artifact.produced_at_seq < entry.edge_seq
        });
    let report = match report_ref {
        None => None,
        Some(artifact) => match p
            .artifact_store
            .open_bounded(p.run_id, artifact.hash, FEEDBACK_REPORT_MAX_BYTES)
            .await
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
                toml::from_str::<surge_core::roadmap::VerificationReportArtifact>(&text)
                    .map_err(|error| error.to_string())
            }) {
            Ok(report) => Some(report),
            Err(error) => {
                tracing::warn!(
                    target: "engine::feedback",
                    node = %p.node,
                    from = %entry.from,
                    %error,
                    "verification report unavailable for re-entry feedback"
                );
                None
            },
        },
    };
    let rendered = render_previous_attempt_feedback(&entry.from, outcome, report.as_ref());
    if rendered.is_some() {
        tracing::info!(
            target: "engine::feedback",
            node = %p.node,
            from = %entry.from,
            with_report = report.is_some(),
            "binding previous-attempt feedback into the re-entered stage prompt"
        );
    }
    rendered
}

/// Requirement revisions a human made (v1 task 1.2) that apply to this stage:
/// those for its task, and run-wide ones. Newest last, so the latest wins.
fn prepend_requirement_revisions(
    prompt: String,
    revisions: &[surge_core::run_state::RequirementRevision],
    task: Option<&surge_core::roadmap::RoadmapTaskId>,
) -> String {
    use std::fmt::Write as _;
    let applicable: Vec<_> = revisions
        .iter()
        .filter(|revision| revision.task.is_none() || revision.task.as_ref() == task)
        .collect();
    if applicable.is_empty() {
        return prompt;
    }
    let mut text = String::from(
        "## Requirement revised by the operator\n\
         The operator changed the requirement for this work. Where it conflicts \
         with the task or earlier criteria, the latest revision wins:\n",
    );
    for revision in applicable {
        let _ = writeln!(text, "- {}", clip(&revision.text));
    }
    text.push('\n');
    text.push_str(&prompt);
    text
}

fn clip(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(FEEDBACK_FIELD_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn render_previous_attempt_feedback(
    from: &NodeKey,
    outcome: Option<&surge_core::run_state::OutcomeRecord>,
    report: Option<&surge_core::roadmap::VerificationReportArtifact>,
) -> Option<String> {
    use std::fmt::Write as _;
    use surge_core::roadmap::VerificationCheckResult;

    let summary = outcome
        .map(|record| clip(&record.summary))
        .unwrap_or_default();
    if summary.is_empty() && report.is_none() {
        return None;
    }
    let mut text = String::from(
        "## Feedback from the previous attempt
",
    );
    match outcome {
        Some(record) => {
            let _ = writeln!(
                text,
                "Stage `{from}` sent this work back with outcome `{}`. Address it before \
                 reporting again.",
                record.outcome
            );
        },
        None => {
            let _ = writeln!(text, "Stage `{from}` sent this work back.");
        },
    }
    if !summary.is_empty() {
        let _ = writeln!(text, "\nSummary: {summary}");
    }
    if let Some(report) = report {
        let report_summary = clip(&report.summary);
        if !report_summary.is_empty() && report_summary != summary {
            let _ = writeln!(text, "\nVerifier summary: {report_summary}");
        }
        let open: Vec<_> = report
            .checks
            .iter()
            .filter(|check| check.result != VerificationCheckResult::Passed)
            .collect();
        if !open.is_empty() {
            text.push_str("\nChecks that did not pass:\n");
            for check in open.iter().take(FEEDBACK_MAX_CHECKS) {
                let result = match check.result {
                    VerificationCheckResult::Failed => "failed",
                    VerificationCheckResult::Skipped => "skipped",
                    VerificationCheckResult::Cancelled => "cancelled",
                    VerificationCheckResult::Passed => "passed",
                };
                let _ = write!(text, "- `{}` — {result}", clip(&check.command));
                if let Some(note) = check.note.as_deref().map(clip).filter(|n| !n.is_empty()) {
                    let _ = write!(text, ": {note}");
                }
                text.push('\n');
            }
            if open.len() > FEEDBACK_MAX_CHECKS {
                let _ = writeln!(text, "- …and {} more", open.len() - FEEDBACK_MAX_CHECKS);
            }
        }
    }
    text.push('\n');
    Some(text)
}

/// Tell the agent which MCP calls a previous session left without a result
/// (ADR-0021 decision 4). Their outcome is unknown and Surge never replays them.
fn prepend_interrupted_mcp_calls(
    prompt: String,
    calls: &[surge_core::run_state::UnresolvedMcpCall],
) -> String {
    if calls.is_empty() {
        return prompt;
    }
    let mut notice = String::from(
        "## Interrupted tool calls\n\
         A previous session ended while these MCP tool calls were running. Their \
         outcome is unknown: check the current state before retrying, and do not \
         repeat an action that may already have happened.\n",
    );
    for call in calls {
        use std::fmt::Write as _;
        let _ = writeln!(
            notice,
            "- `{}` on MCP server `{}` (event {})",
            call.tool, call.server, call.seq
        );
    }
    notice.push('\n');
    notice.push_str(&prompt);
    notice
}

fn append_completion_contract(mut prompt: String, outcomes: &[OutcomeKey]) -> String {
    prompt.push_str(
        "\n\n## Stage completion protocol\n\
        Before ending your turn, call the available report_stage_outcome tool \
        (it may be exposed with an MCP server prefix). A final text message alone \
        does not complete this stage. Choose exactly one of these declared outcomes: ",
    );
    prompt.push_str(
        &outcomes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    );
    prompt.push_str(
        ".\n\
        Follow the tool's input schema: provide the outcome, a factual summary, \
        and artifacts_produced paths for files you produced. When call_id is required, \
        use a unique ID for each revised report. If validation rejects a report, \
        repair the reported problem before submitting a new candidate. A receipt \
        acknowledges a candidate; it does not certify verification or approval. \
        Never claim checks passed unless you actually ran them.\n",
    );
    prompt
}

fn append_iteration_context(
    prompt: String,
    frames: &[crate::engine::frames::Frame],
) -> Result<String, StageError> {
    let mut context = Vec::new();
    for frame in frames {
        let crate::engine::frames::Frame::Loop(frame) = frame else {
            continue;
        };
        let item = frame
            .items
            .get(frame.current_index as usize)
            .ok_or_else(|| {
                StageError::Internal(format!("loop {} has no current item", frame.loop_node))
            })?;
        context.push(serde_json::json!({
            "loop_node": frame.loop_node.as_str(),
            "variable": frame.config.iteration_var_name,
            "index": frame.current_index,
            "item": item,
        }));
    }
    if context.is_empty() {
        return Ok(prompt);
    }
    let context = serde_json::to_string_pretty(&context)
        .map_err(|e| StageError::Internal(format!("serialize loop context: {e}")))?;
    Ok(format!(
        "{prompt}\n\n# Current workflow iteration\n\
         The following data identifies the active loop items, outermost first. \
         Perform this stage for the innermost item, using its description and \
         acceptance criteria. Outer items provide context; other tasks listed there \
         are not the current task. Apply the stage's implementation or verification \
         role to this item.\n\n```json\n{context}\n```"
    ))
}

/// Pick the effective [`ApprovalConfig`] for an agent stage.
///
/// Precedence (highest first):
///   1. `agent_config.approvals_override` — per-node override declared on
///      the graph node.
///   2. The resolved profile's `approvals` — inherited from
///      `Profile.approvals` when the agent stage was wired via the profile
///      registry.
///   3. [`ApprovalConfig::default`] — the last-resort fallback (elevation
///      timeout = 24 h, empty channels, etc.).
///
/// Whole-replace semantics: a node override drops the profile-level
/// approvals entirely. Granular field-level merging is left for a future
/// refactor (mirrors how `effective_agent_hooks` works at the hook level).
#[must_use]
pub(crate) fn effective_approvals(
    agent_config: &AgentConfig,
    resolved_profile: Option<&surge_core::profile::registry::ResolvedProfile>,
) -> surge_core::approvals::ApprovalConfig {
    if let Some(override_cfg) = agent_config.approvals_override.as_ref() {
        return override_cfg.clone();
    }
    if let Some(profile) = resolved_profile {
        return profile.profile.approvals.clone();
    }
    surge_core::approvals::ApprovalConfig::default()
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

/// Resolve the effective system-prompt template for an agent stage.
///
/// The *base* is the node's `prompt_overrides.system`, else the resolved
/// profile's `prompt.system`, else empty. If `prompt_overrides.append_system`
/// is set, it is appended to that base (separated by a blank line) — so an
/// append augments the base prompt instead of replacing it. Pre-fork `--prompt`
/// edits rely on this: appending a corrective hint must not discard the node's
/// or profile's existing system prompt.
pub(crate) fn effective_system_prompt(
    agent_config: &AgentConfig,
    resolved_profile: Option<&surge_core::profile::registry::ResolvedProfile>,
) -> String {
    let base = agent_config
        .prompt_overrides
        .as_ref()
        .and_then(|po| po.system.as_deref())
        .or_else(|| resolved_profile.map(|r| r.profile.prompt.system.as_str()))
        .unwrap_or("");
    let append = agent_config
        .prompt_overrides
        .as_ref()
        .and_then(|po| po.append_system.as_deref())
        .filter(|s| !s.is_empty());
    match append {
        Some(extra) if base.is_empty() => extra.to_string(),
        Some(extra) => format!("{base}\n\n{extra}"),
        None => base.to_string(),
    }
}

/// Append each bound skill's instructions onto `prompt`, one `## Skill:
/// <name>` section per entry, in binding order.
///
/// A no-op when `bound_skills` is empty (the common path — most nodes
/// declare none), so this never adds a stray heading to a prompt that has
/// nothing to bind.
fn append_bound_skills(
    prompt: String,
    bound_skills: &[crate::engine::stage::skill_binding::BoundSkill],
) -> String {
    if bound_skills.is_empty() {
        return prompt;
    }
    let mut out = prompt;
    for skill in bound_skills {
        out.push_str("\n\n## Skill: ");
        out.push_str(&skill.name);
        out.push_str("\n\n");
        out.push_str(&skill.instructions);
    }
    out
}

/// Drain `dispatcher`'s pending loop-guard escalations and append each as an
/// `EscalationRequested` event, typed by [`EscalationCause::LoopGuardRepeatedToolCall`]
/// / [`EscalationCause::LoopGuardNodeDeadline`] per trip kind. Shared by the
/// two call sites that can observe a trip: right after a tool dispatch, and
/// the timer-driven wall-clock poll inside the stage's event loop — both need
/// the same drain-then-append behavior, and a shared helper keeps them from
/// drifting apart the way a second hand-rolled loop would risk.
///
/// Returns the drained trips (in append order) so a caller that must react
/// to *which* trip fired — the wall-clock poll ends the stage on a
/// `NodeDeadlineExceeded`, see its call site — does not have to re-derive
/// that from the just-written event.
async fn append_loop_escalations(
    writer: &RunWriter,
    dispatcher: &Arc<dyn crate::engine::tools::ToolDispatcher>,
) -> Result<Vec<LoopGuardTrip>, StageError> {
    let mut trips = Vec::new();
    for esc in dispatcher.drain_loop_escalations() {
        let cause = match &esc.trip {
            LoopGuardTrip::RepeatedToolCall { .. } => EscalationCause::LoopGuardRepeatedToolCall,
            LoopGuardTrip::NodeDeadlineExceeded { .. } => EscalationCause::LoopGuardNodeDeadline,
            LoopGuardTrip::NoProgress { .. } => EscalationCause::LoopGuardNoProgress,
            LoopGuardTrip::ToolCallCapExceeded { .. } => EscalationCause::LoopGuardToolCallCap,
        };
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::EscalationRequested {
                    stage: None,
                    reason: esc.trip.operator_message(),
                    cause,
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        trips.push(esc.trip);
    }
    Ok(trips)
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
/// provider and context usage observations without inventing token consumption.
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
///
/// # Task-owned quota
/// When the stage is task-owned and its selected candidate reports
/// exhaustion, the next frozen candidate runs as a fresh attempt — one
/// provider opening per attempt, decided between attempts by
/// [`crate::engine::quota_recovery::TaskQuotaOwner::after_exhaustion`]. When
/// no candidate remains the original rate-limit error is returned with the
/// capacity suspension already requested.
pub async fn execute_agent_stage(mut p: AgentStageParams<'_>) -> StageResult {
    loop {
        let mut exhausted = None;
        let result = execute_agent_attempt(&mut p, &mut exhausted).await;
        let (Some(quota), Some(owner)) = (exhausted, p.quota_owner.as_ref()) else {
            return result;
        };
        match owner.after_exhaustion(&quota, p.worktree_path, p.agent_registry.as_deref())? {
            crate::engine::quota_recovery::AfterExhaustion::Retry(next) => {
                p.quota_cycle = Some(next.cycle);
                p.quota_opening = Some(next.opening);
            },
            crate::engine::quota_recovery::AfterExhaustion::Suspended => return result,
        }
    }
}

/// One provider attempt of [`execute_agent_stage`]. On a rate limit with a
/// task-owned cycle, `exhausted` receives the cycle carrying the recorded
/// exhaustion so the caller can decide the next candidate.
#[allow(clippy::too_many_lines)]
async fn execute_agent_attempt(
    p: &mut AgentStageParams<'_>,
    exhausted: &mut Option<crate::engine::quota_recovery::TaskQuotaCycle>,
) -> StageResult {
    let mut targets = BTreeSet::new();
    for binding in &p.agent_config.bindings {
        if !targets.insert(&binding.target.0) {
            return Err(StageError::Internal(format!(
                "duplicate binding target: {}",
                binding.target.0
            )));
        }
    }

    // Phase 6.4: resolve bindings and prompt BEFORE building SessionConfig so
    // we can wire them into the config (not just the first message).
    let mut resolved_bindings =
        resolve_bindings(&p.agent_config.bindings, p.run_memory, p.worktree_path)
            .await
            .map_err(|e| StageError::Internal(format!("binding resolution: {e}")))?;
    let memory_receipt = apply_memory_claim_pack(p, &mut resolved_bindings).await?;

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
    if let Some(profile) = &resolved_profile {
        crate::engine::validate::validate_agent_inputs(p.agent_config, &profile.profile)
            .map_err(|e| StageError::Internal(format!("required profile input: {e}")))?;
        for input in profile
            .profile
            .bindings
            .expected
            .iter()
            .filter(|input| !input.optional)
        {
            if !resolved_bindings
                .iter()
                .any(|(target, content)| target.0 == input.name && !content.trim().is_empty())
            {
                return Err(StageError::Internal(format!(
                    "required profile input '{}' resolved to empty content",
                    input.name
                )));
            }
        }
    }
    let effective_approval_cfg = effective_approvals(p.agent_config, resolved_profile.as_ref());

    // Effective system prompt: node `system` override (or the profile's system)
    // as the base, with `append_system` appended (see `effective_system_prompt`).
    // Appending — rather than the old either/or — is what makes pre-fork
    // `--prompt` edits take effect even when the node or profile already carries
    // a base system prompt.
    let prompt_template = effective_system_prompt(p.agent_config, resolved_profile.as_ref());
    // Required profile inputs were checked above. Lenient rendering lets
    // omitted optional inputs remain empty without weakening that check.
    let renderer = PromptRenderer::lenient();
    let prompt_text = renderer
        .render(&prompt_template, &resolved_bindings)
        .map_err(|e| StageError::Internal(format!("prompt render: {e}")))?;
    let prompt_text = if crate::engine::bootstrap::is_flow_generator_profile(profile_str) {
        crate::engine::bootstrap::append_flow_serialization_reference(&prompt_text)
    } else {
        prompt_text
    };
    let prompt_text = append_iteration_context(prompt_text, p.frames)?;

    // Append every bound skill's instructions *after* template rendering,
    // not before: a pack's own Markdown body can legitimately contain
    // `{{...}}`-shaped text (code samples, its own placeholder syntax) that
    // must reach the agent verbatim, not be mistaken for one of this
    // stage's template variables. This is the one place a resolved skill's
    // content reaches the agent — bound "exactly like a context Binding"
    // means baked into the system prompt at session-open time, the same
    // rendering pass, never re-fetched mid-turn.
    let prompt_text = append_bound_skills(prompt_text, p.bound_skills);

    // Derive AgentKind + its resolved spawn environment. With a resolved
    // profile in hand, take the agent_id from its runtime block; otherwise
    // fall through to the legacy mock fast path so callers without a
    // registry keep working. The registry is the merged catalog (user
    // `[agents.*]` over builtins), so a custom provider resolves exactly
    // like a builtin one.
    let mut agent_launch = match resolved_profile.as_ref() {
        Some(rp) => derive_agent_kind_from_id(
            profile_str,
            effective_agent_id(p.agent_config, rp),
            p.agent_registry.as_deref(),
        )?,
        None => AgentLaunch {
            kind: derive_agent_kind(profile_str),
            env: BTreeMap::new(),
            settings_files: Vec::new(),
        },
    };
    if let Some(permit) = &p.quota_opening {
        let launch = permit.launch();
        if launch.mode() != surge_core::execution_recovery::SessionOpenMode::New {
            return Err(StageError::Internal(
                "quota fallback currently requires a new provider session".into(),
            ));
        }
        let runtime = launch.candidate().candidate().runtime();
        if resolved_profile.is_none() {
            return Err(StageError::Internal(
                "quota fallback requires a resolved profile".into(),
            ));
        }
        agent_launch =
            derive_agent_kind_from_id(profile_str, runtime, p.agent_registry.as_deref())?;
        let actual_hash = ContentHash::compute(format!("{:?}", agent_launch.kind).as_bytes());
        if &actual_hash != launch.candidate().launch_hash() {
            return Err(StageError::Internal(
                "quota candidate launch fingerprint differs from frozen policy".into(),
            ));
        }
    }
    if let Some(registry) = p.agent_registry.as_deref()
        && let Some(route) = registry
            .find_normalized(p.quota_opening.as_ref().map_or_else(
                || {
                    resolved_profile.as_ref().map_or("mock", |profile| {
                        effective_agent_id(p.agent_config, profile)
                    })
                },
                |permit| permit.launch().candidate().candidate().runtime(),
            ))
            .and_then(|entry| entry.capacity_route.as_ref())
    {
        crate::engine::capacity_routes::materialize_managed_env(route, &mut agent_launch.env);
    }
    let AgentLaunch {
        kind: agent_kind,
        env: agent_env,
        settings_files,
    } = agent_launch;

    // Materialise any settings files the agent's entry declares (e.g. the
    // Claude-agent entries' `.claude/settings.json` with a non-interactive
    // permission mode). Data-driven: the entry says which file and what
    // content, so a new provider needs no code here. Best-effort — a
    // failure degrades the agent to its global configuration, never fails
    // the run on its own.
    if !settings_files.is_empty() {
        let worktree = p.worktree_path.to_path_buf();
        let files = settings_files.clone();
        // Synchronous (durable) file I/O — keep it off the async launch path.
        let _ = tokio::task::spawn_blocking(move || {
            surge_acp::settings_seed::seed_settings_files(&files, &worktree);
        })
        .await;
    }

    // Pin after trusted provider setup has finished writing its settings, but
    // before the agent can inspect or mutate the checked workspace.
    let verification_input_seq = p
        .writer
        .current_seq()
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    let verification_input = super::verification::begin(p).await?;
    let mut prompt_text = prompt_text;
    if let Some(input) = &verification_input {
        let _ = write!(
            prompt_text,
            "\nVerification criteria (every ID required): {:?}. Submit verification_report inline to report_stage_outcome with task_id={:?}, outcome=passed, summary and checks (command, result, covers=[IDs]); do not write a report file or supply binding.\n",
            input.criteria.definitions,
            p.active_task_id.as_ref().map_or("", |id| id.as_str())
        );
    }

    // Derive declared outcomes from the node's OutcomeDecl list.
    // Fall back to ["done"] when the node has no declared_outcomes so the
    // session is always valid (SessionConfig::validate requires at least one).
    let declared_outcomes: Vec<OutcomeKey> = if p.declared_outcomes.is_empty() {
        vec![OutcomeKey::try_from("done").expect("'done' is a valid OutcomeKey")]
    } else {
        p.declared_outcomes.iter().map(|d| d.id.clone()).collect()
    };

    let prompt_text = if let Some(context) = &p.run_memory.work_item {
        format!("{}\n\n{}", context.prompt(), prompt_text)
    } else {
        prompt_text
    };
    let prompt_text = append_completion_contract(prompt_text, &declared_outcomes);

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
    let sandbox_cfg: surge_core::sandbox::SandboxConfig = p
        .agent_config
        .sandbox_override
        .clone()
        .or_else(|| {
            resolved_profile
                .as_ref()
                .map(|resolved| resolved.profile.sandbox.clone())
        })
        .unwrap_or_default();

    // Build the session-scoped tool dispatcher. `RoutingToolDispatcher`
    // always wraps the engine dispatcher — not only when the node declares
    // `mcp_add` — so the per-node loop guard and output-spill policy apply
    // to every agent stage: R39/R40 are engine-level policy, not something
    // that only exists on the MCP branch (first-review finding: a plain
    // node repeating `read_file` used to get `p.tool_dispatcher` bare, with
    // no guard and no spill at all).
    //
    // Per-stage MCP server allowlist from ToolOverride::mcp_add.
    let allowed_servers: std::collections::HashSet<&str> = p
        .agent_config
        .tool_overrides
        .as_ref()
        .map(|o| o.mcp_add.iter().map(String::as_str).collect())
        .unwrap_or_default();

    // Short-circuit: stage doesn't expose any MCP servers, so skip the
    // potentially expensive catalog call entirely. Also covers the
    // no-MCP-registry-configured case (`p.mcp_registry` is `None`) — there
    // is nothing to list either way.
    let (filtered_mcp_tools, mcp_timeouts): (
        Vec<surge_mcp::McpToolEntry>,
        std::collections::HashMap<String, std::time::Duration>,
    ) = if allowed_servers.is_empty() {
        (Vec::new(), std::collections::HashMap::new())
    } else if let Some(ref reg) = p.mcp_registry {
        // Build per-server `allowed_tools` lookup from the run-level
        // registry. `None` means "expose all tools the server reports".
        let allowed_tools_per_server: std::collections::HashMap<&str, Option<&[String]>> = p
            .mcp_servers
            .iter()
            .map(|s| (s.name.as_str(), s.allowed_tools.as_deref()))
            .collect();

        // Resolve the canonical MCP spawn policy once per server
        // (not per tool): a `Denied` server is hidden entirely, and
        // the unconstrained-intent operator WARN fires at most once
        // per server rather than once per tool.
        let mcp_denied_servers: std::collections::HashSet<&str> = p
            .mcp_servers
            .iter()
            .filter(|s| allowed_servers.contains(s.name.as_str()))
            .filter_map(|s| {
                let effective = s.sandbox.unwrap_or(sandbox_cfg.mode);
                warn_if_unconstrained_mcp(&s.name, effective);
                match mcp_spawn_policy(sandbox_cfg.mode, s.sandbox) {
                    McpSpawnPolicy::Allowed => None,
                    McpSpawnPolicy::Denied => Some(s.name.as_str()),
                }
            })
            .collect();

        let permitted_servers: Vec<String> = p
            .mcp_servers
            .iter()
            .filter(|server| allowed_servers.contains(server.name.as_str()))
            .filter(|server| !mcp_denied_servers.contains(server.name.as_str()))
            .map(|server| server.name.clone())
            .collect();
        // Per-server outcomes: one failing server must not hide the other
        // selected servers' tools, and its failure must reach the operator.
        let mut all_mcp_tools = Vec::new();
        let mut unavailable: Vec<(String, String)> = Vec::new();
        match reg.list_tools_per_server(&permitted_servers).await {
            Ok(catalogs) => {
                for catalog in catalogs {
                    match catalog.tools {
                        Ok(tools) => all_mcp_tools.extend(tools),
                        Err(error) => unavailable.push((catalog.server, error.to_string())),
                    }
                }
            },
            Err(error) => {
                let error = error.to_string();
                unavailable.extend(
                    permitted_servers
                        .iter()
                        .map(|server| (server.clone(), error.clone())),
                );
            },
        }
        // The stage still runs (its engine tools and any healthy servers
        // remain useful), but a server the node explicitly selected going
        // missing is an operator-visible degradation, not a log line:
        // record it as a replay-safe `EscalationRequested`. `McpError`
        // displays carry only fixed reason codes and durations, never
        // child output.
        for (server, error) in unavailable {
            tracing::warn!(
                node = %p.node,
                server = %server,
                err = %error,
                "MCP selected catalog failed; stage proceeds without this server's tools"
            );
            p.writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::EscalationRequested {
                        stage: None,
                        reason: format!(
                            "stage '{}' selected MCP server '{server}', but its tool catalog \
                             is unavailable ({error}); the stage proceeds without its tools",
                            p.node
                        ),
                        cause: EscalationCause::McpSelectedCatalogUnavailable,
                    },
                ))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
        }

        let filtered: Vec<surge_mcp::McpToolEntry> = all_mcp_tools
            .into_iter()
            .filter(|t| {
                if !allowed_servers.contains(t.server.as_str()) {
                    return false;
                }
                if mcp_denied_servers.contains(t.server.as_str()) {
                    return false;
                }
                // Per-server allowed_tools whitelist: outer Some = entry
                // exists in the HashMap; inner Some = the field is set.
                // If allowed_tools is None, no filtering is applied.
                if let Some(Some(whitelist)) = allowed_tools_per_server.get(t.server.as_str())
                    && !whitelist.iter().any(|w| w == &t.tool)
                {
                    return false;
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

        (filtered, timeouts)
    } else {
        // Node declares `mcp_add` but this run has no MCP registry
        // configured at all — nothing to route to.
        (Vec::new(), std::collections::HashMap::new())
    };

    // Fall back to an empty in-process registry when the run has none
    // configured: cheap (no servers, no spawn — a connection only spawns on
    // first use) and lets `RoutingToolDispatcher` wrap every agent stage
    // unconditionally instead of only when MCP is in play.
    let mcp_registry_for_stage: Arc<surge_mcp::McpRegistry> = p
        .mcp_registry
        .clone()
        .unwrap_or_else(|| Arc::new(surge_mcp::McpRegistry::from_config(&[], None)));

    let session_dispatcher: Arc<dyn crate::engine::tools::ToolDispatcher> = Arc::new(
        crate::engine::tools::RoutingToolDispatcher::new(
            p.tool_dispatcher.clone(),
            mcp_registry_for_stage,
            &filtered_mcp_tools,
            &mcp_timeouts,
        )
        .with_tool_call_loop_guard_config(p.tool_call_loop_guard)
        .with_output_spill_config(p.output_spill)
        // The stage already holds the run's own artifact store — spilled
        // output must land there, not in a second store built from
        // `ArtifactStore::from_default_path()` (`~/.surge/runs`), which
        // would be unreachable to whatever reads the run's artifacts back
        // (`.autopilot/competitive-waves/spec.md` §16).
        .with_artifact_store(p.artifact_store.clone()),
    )
        as Arc<dyn crate::engine::tools::ToolDispatcher>;

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
    let mut config_selections = node_config_selections(
        p.agent_config,
        resolved_profile
            .as_ref()
            .and_then(|resolved| resolved.profile.role.min_effort.as_deref()),
    );
    if let Some(model) = p
        .quota_opening
        .as_ref()
        .and_then(|permit| permit.launch().candidate().model())
    {
        let selection = surge_acp::bridge::session::ConfigSelection {
            category: surge_acp::bridge::session::ConfigCategory::Model,
            value: model.to_owned(),
            best_effort: false,
        };
        if let Some(existing) = config_selections
            .iter_mut()
            .find(|choice| choice.category == surge_acp::bridge::session::ConfigCategory::Model)
        {
            *existing = selection;
        } else {
            config_selections.push(selection);
        }
    }
    let mut session_config = SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: p
            .quota_opening
            .as_ref()
            .map_or_else(surge_core::id::StageInvocationId::new, |permit| {
                permit.launch().provider_invocation()
            }),
        runtime: p.quota_opening.as_ref().map_or_else(
            || {
                resolved_profile.as_ref().map_or_else(
                    || agent_kind.label().into(),
                    |profile| {
                        canonical_runtime_id_for(
                            p.agent_registry.as_deref(),
                            p.agent_config,
                            profile,
                        )
                        .into_string()
                    },
                )
            },
            |permit| permit.launch().candidate().candidate().runtime().to_owned(),
        ),
        opening: p.quota_opening.as_ref().map_or_else(
            surge_core::execution_recovery::SessionOpening::default,
            |_| surge_core::execution_recovery::SessionOpening::New,
        ),
        stage_mcp: None,
        agent_kind,
        working_dir: p.worktree_path.to_path_buf(),
        system_prompt: prompt_text.clone(),
        declared_outcomes,
        allows_escalation,
        tools: session_tools,
        sandbox,
        permission_policy: PermissionPolicy::default(),
        bindings: session_bindings,
        env: agent_env,
        // Per-step model / reasoning level chosen by the operator; applied
        // through the agent's standard ACP session options.
        config_selections,
    };

    if p.quota_opening.is_none()
        && let Some(fence) = &p.run_memory.suspension
        && let surge_core::execution_recovery::PendingStagePhase::Interrupted { node, invocation } =
            &fence.pending_stage
        && node == p.node
    {
        let saved = p.run_memory.provider_sessions.get(invocation)
                    .and_then(|history| history.last())
                    .ok_or_else(|| StageError::Bridge("saved invocation has no durable provider identity; explicit recovery choice required".into()))?;
        session_config.invocation = *invocation;
        session_config.opening = if p
            .continuation
            .as_ref()
            .is_some_and(|control| control.allow_new_session)
        {
            surge_core::execution_recovery::SessionOpening::New
        } else {
            surge_core::execution_recovery::SessionOpening::Continue(saved.descriptor.clone())
        };
    }

    if let Some(owner) = p.quota_owner.as_ref()
        && let Err(error) = owner.validate_sources(
            p.quota_cycle.as_ref(),
            p.quota_opening.as_ref(),
            p.worktree_path,
            p.agent_registry.as_deref(),
            &session_config,
        )
    {
        if let Some(permit) = p.quota_opening.take() {
            owner.invalidate_unexecuted(permit).map_err(|failure| {
                StageError::RecoveryRequired(format!(
                    "retain source-changed opening containment: {failure}"
                ))
            })?;
        }
        return Err(error);
    }

    // Persist the complete resolved inputs, independently of the capped ACP echo.
    // This proves resolution for this attempt, before a session can be opened.
    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::StageInputsResolved {
                node: p.node.clone(),
                attempt: p.attempt,
                bindings: resolved_bindings
                    .iter()
                    .map(|(target, content)| {
                        (target.0.clone(), ContentHash::compute(content.as_bytes()))
                    })
                    .collect(),
                memory_receipt,
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;

    let stage_context = surge_core::stage_tool::StageToolContext {
        run: p.run_id,
        node: p.node.clone(),
        session: surge_core::SessionId::new(),
        generation: surge_core::id::StageGenerationId::new(),
    };
    let accepted_outcomes = session_config.declared_outcomes.clone();
    let (stage_endpoint, mut stage_requests, mut stage_calls) =
        super::stage_tools::prepare(&mut session_config, stage_context)?;

    // Subscribe to events BEFORE opening the session, so we don't miss the
    // SessionEstablished event (or earlier ToolCall events).
    let mut events = p.bridge.subscribe();

    // Track outcomes rejected by `on_outcome` hooks within THIS session so we
    // can surface `StageFailed` once the agent burns its retry budget.
    let max_outcome_rejections: u32 = p.agent_config.limits.max_retries;
    let mut rejection_budget =
        super::outcome_acceptance::RejectionBudget::new(max_outcome_rejections);

    let execution_writer = session_config.writer_id;
    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::ExecutionWriterIntent {
                intent: surge_core::execution_recovery::process::ExecutionWriterIntent {
                    writer: execution_writer,
                    invocation: session_config.invocation,
                    kind: surge_core::execution_recovery::process::ExecutionWriterKind::Provider,
                    owner: surge_acp::process_evidence::observe(std::process::id())
                        .ok()
                        .map(|(identity, _)| identity),
                    local_effects: true,
                },
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::SessionEstablishmentRequested {
                authority: Some(stage_calls.context.clone()),
                node: p.node.clone(),
                invocation: session_config.invocation,
                restore: matches!(
                    session_config.opening,
                    surge_core::execution_recovery::SessionOpening::Continue(_)
                ),
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;

    let provider_invocation = session_config.invocation;
    let logical_invocation = p
        .quota_cycle
        .as_ref()
        .map(|quota| quota.cycle.invocation.parse())
        .transpose()
        .map_err(|_| StageError::RecoveryRequired("invalid planned logical invocation".into()))?
        .unwrap_or(session_config.invocation);
    let logical_runtime = session_config.runtime.clone();
    let quota_opening = p.quota_opening.take();
    let is_quota_fallback = quota_opening.is_some();
    let quota_handoff = quota_opening.map(|permit| permit.into_opening().0);
    let is_new_opening = matches!(
        session_config.opening,
        surge_core::execution_recovery::SessionOpening::New
    );
    let opened = match p.bridge.open_session(session_config).await {
        Ok(session) => session,
        Err(error) => {
            let cleanup = stage_endpoint.close().await;
            let diagnostic = format!("open_session: {error}; stage endpoint cleanup: {cleanup:?}");
            return Err(if p.continuation.as_ref().is_some_and(|control|control.state == surge_core::execution_recovery::ExecutionControlState::ContinueReserved) {
                StageError::RecoveryRequired(diagnostic)
            } else { StageError::Bridge(diagnostic) });
        },
    };

    if opened
        .execution_writer
        .as_ref()
        .is_some_and(|observed| observed.writer() != execution_writer)
    {
        let endpoint_cleanup = stage_endpoint.close().await;
        let cleanup = p.bridge.close_session(opened.session).await;
        return Err(StageError::Bridge(format!(
            "provider writer identity mismatched its durable intent; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
        )));
    }
    let session_id = opened.session;
    let opened = p
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::SessionOpened {
            handoff: quota_handoff,
            opened: Some(opened),
            node: p.node.clone(),
            session: session_id,
            agent: p.agent_config.profile.to_string(),
            // The actual runtime identity, not the role/profile above —
            // `None` **only** via the no-profile-registry legacy path (see
            // `resolved_profile` above, which has no runtime block to read
            // one from). When a profile *is* resolved, this is always
            // `Some`: normalized through the registry when that succeeds,
            // falling back to the **raw** `agent_id` when it does not
            // (review finding #4) — the bundled `mock` profile
            // (`bundled/profiles/mock-1.0.toml`, `agent_id = "mock"`) is a
            // real, shipped case of the latter (`normalize_agent_id("mock")`
            // is `None`: "mock" is neither a registry id nor an alias), and
            // silently dropping to `None` there would have every run on it
            // report `CapacityStatus::Unclassified` where the raw string
            // would have read as `Known` — a real identity discarded for
            // want of a registry entry, not the "nothing was ever known"
            // `None` is reserved for elsewhere in this same field.
            //
            // **Normalized here, at the point of writing this fact — not
            // left for each reader to normalize on its own (Task 12 M1,
            // resolving the gap the M0 review found).** Before this, this
            // field carried the raw, un-normalized `agent_id` unconditionally,
            // while `StageError::RateLimited.runtime` two call sites below
            // already went through `normalize_agent_id`: one fact (which
            // runtime a session belongs to) had two different values
            // depending on which event you read, and
            // `claude`/`claude-code`/`claude-acp` could fragment across
            // three keys on the one path (`surge-cli`'s inbox scan) that
            // actually keys off this field instead of the registry-id path
            // M2's persisted ledger will use. Normalizing at write, not at
            // read, because: (a) this is the *only* place this fact is ever
            // produced, while it already has at least one real reader
            // (`surge-cli`'s inbox scan) and will gain another (M2's
            // ledger, reading historical `SessionOpened` for keys already
            // collapsed rather than raw); asking every future reader to
            // remember to normalize is the "remember-to-call-me" shape this
            // crate's own standards single out as the wrong contract. (b) A
            // run's persisted event log is append-only — events written
            // **before** this change keep their raw, un-normalized
            // `agent_id` forever; this fix closes the gap for every session
            // opened from here on, not retroactively. A reader spanning
            // both eras still needs its own normalization pass over
            // historical data if it must collapse aliases there too — a
            // fact for that reader to state, not something this write-site
            // can undo.
            agent_id: if is_quota_fallback {
                Some(logical_runtime.clone())
            } else {
                resolved_profile.as_ref().map(|rp| {
                    canonical_runtime_id_for(p.agent_registry.as_deref(), p.agent_config, rp)
                        .into_string()
                })
            },
        }))
        .await;
    if let Err(error) = opened {
        let endpoint_cleanup = stage_endpoint.close().await;
        let cleanup = p.bridge.close_session(session_id).await;
        return Err(StageError::Storage(format!(
            "SessionOpened: {error}; cleanup: {cleanup:?}; stage endpoint: {endpoint_cleanup:?}"
        )));
    }

    let mut task_quota_cycle = p.quota_cycle.take();
    if let Some(operation) = quota_handoff {
        let sequence = opened.as_ref().map_err(|error| {
            StageError::Storage(format!("quota opening sequence unavailable: {error}"))
        })?;
        let Some(owner) = &p.quota_owner else {
            let endpoint_cleanup = stage_endpoint.close().await;
            let cleanup = p.bridge.close_session(session_id).await;
            return Err(StageError::RecoveryRequired(format!(
                "quota opening has no task owner; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
            )));
        };
        if let Err(error) = owner.confirm_open(operation, sequence.0) {
            let endpoint_cleanup = stage_endpoint.close().await;
            let cleanup = p.bridge.close_session(session_id).await;
            return Err(StageError::RecoveryRequired(format!(
                "quota opening confirmation failed: {error}; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
            )));
        }
        if let Some(quota) = &mut task_quota_cycle {
            quota.opening_seq = sequence.0;
        }
    }

    // Bind the actual first provider opening to the task's frozen quota policy
    // before prompting. This is evidence only: the primary ACP opening above
    // is already complete, while every later candidate still needs its own
    // persisted one-shot opening permit.
    if is_new_opening
        && !is_quota_fallback
        && task_quota_cycle.is_none()
        && let Some(owner) = &p.quota_owner
    {
        let sequence = opened.as_ref().map_err(|error| {
            StageError::Storage(format!("quota opening sequence unavailable: {error}"))
        })?;
        let control_generation = p
            .continuation
            .as_ref()
            .map_or(0, |control| control.generation);
        match owner.begin(logical_invocation, sequence.0, control_generation) {
            Ok(cycle) => task_quota_cycle = Some(cycle),
            Err(error) => {
                let endpoint_cleanup = stage_endpoint.close().await;
                let cleanup = p.bridge.close_session(session_id).await;
                return Err(StageError::RecoveryRequired(format!(
                    "task quota cycle initialization failed: {error}; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
                )));
            },
        }
    }

    let mut continued_seq = None;
    if let Some(control) = &p.continuation
        && control.state == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
    {
        let sequence = match p
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::RunContinued {
                control_generation: control.generation,
            }))
            .await
        {
            Ok(sequence) => sequence.0,
            Err(error) => {
                let endpoint_cleanup = stage_endpoint.close().await;
                let cleanup = p.bridge.close_session(session_id).await;
                return Err(StageError::RecoveryRequired(format!(
                    "RunContinued: {error}; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
                )));
            },
        };
        continued_seq = Some(sequence);
    }

    if let (Some(operation), Some(owner)) = (quota_handoff, &p.quota_owner) {
        let opened_seq = match opened.as_ref() {
            Ok(sequence) => sequence.0,
            Err(error) => {
                let endpoint_cleanup = stage_endpoint.close().await;
                let cleanup = p.bridge.close_session(session_id).await;
                return Err(StageError::Storage(format!(
                    "quota opening sequence unavailable: {error}; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
                )));
            },
        };
        if let Err(reason) = owner.authorize_prompt(operation, opened_seq, continued_seq) {
            let endpoint_cleanup = stage_endpoint.close().await;
            let cleanup = p.bridge.close_session(session_id).await;
            return Err(StageError::RecoveryRequired(format!(
                "{reason}; cleanup: {cleanup:?}; endpoint: {endpoint_cleanup:?}"
            )));
        }
    }

    let prompt_text = match previous_attempt_feedback(p).await {
        Some(feedback) => feedback + &prompt_text,
        None => prompt_text,
    };
    let prompt_text = prepend_requirement_revisions(
        prompt_text,
        &p.run_memory.requirement_revisions,
        p.active_task_id.as_ref(),
    );
    // Prepend any queued operator steer messages to this turn's prompt (B2).
    // Non-destructive: steering lands here, at the stage boundary, because ACP
    // v1 offers no mid-turn injection channel.
    let prompt_text = if p.steers.is_empty() {
        prompt_text
    } else {
        let mut steered =
            String::from("## Operator steering\nApply this guidance to the work below:\n");
        for steer in &p.steers {
            steered.push_str("- ");
            steered.push_str(steer.message.trim());
            steered.push('\n');
        }
        steered.push('\n');
        steered.push_str(&prompt_text);
        steered
    };
    let interrupted = &p.run_memory.unresolved_mcp_calls;
    if !interrupted.is_empty() {
        tracing::info!(
            target: "mcp::recovery",
            node = %p.node,
            calls = interrupted.len(),
            "notifying agent of interrupted MCP calls with unknown outcome"
        );
    }
    let prompt_text = prepend_interrupted_mcp_calls(prompt_text, interrupted);
    let prompt_msg = MessageContent::Text(prompt_text);
    let mut prompt_finished = tokio_util::sync::CancellationToken::new();
    let prompt_signal = prompt_finished.clone();
    let prompt_bridge = Arc::clone(p.bridge);
    let mut prompt_task = tokio::spawn(async move {
        let _finished = prompt_signal.drop_guard();
        prompt_bridge.send_message(session_id, prompt_msg).await
    });
    let mut prompt_joined = false;
    let mut session_disposition = None;
    let mut deferred_events = std::collections::VecDeque::new();
    let stage_result = async {
    let mut prompt_success = false;
    let mut candidates = std::collections::VecDeque::new();
    let mut retry_feedback: Option<String> = None;
    let mut missing_outcome_reminders: u32 = 0;
    // Timer-driven poll of the loop guard's wall-clock deadline
    // (`.autopilot/competitive-waves/spec.md` §15). `check_loop_guard`
    // (inside `RoutingToolDispatcher::dispatch`) only sees the deadline when
    // a tool call arrives — a node stuck in one long agent turn (streaming
    // `AgentMessage`s, no tool calls at all) would otherwise never trip its
    // budget. A 1s period bounds trip latency cheaply against a default
    // one-hour budget; `tick()` fires immediately on the first poll, so a
    // deadline that is already exceeded at session start (e.g. a `0`-second
    // configured limit) is caught right away rather than a full period late.
    let mut deadline_poll = tokio::time::interval(std::time::Duration::from_secs(1));
    deadline_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // A loop-protection trip observed while dispatching a tool call; the
    // attempt ends at the top of the next iteration, after that call's result
    // is durable and delivered (v1 task 1.3).
    let mut guard_trip: Option<LoopGuardTrip> = None;

    // Drive the event loop until OutcomeReported (success) or SessionEnded
    // (failure / abnormal termination).
    let outcome = loop {
        if let Some(trip) = guard_trip.take() {
            return Err(StageError::LoopGuardTripped(trip));
        }
        if prompt_success && candidates.is_empty()
            && !p.bridge.legacy_stage_event_adapter()
            && let Some(feedback) = retry_feedback.take()
        {
            // Validation runs only after a complete provider turn. Start a new
            // turn asynchronously so this loop can continue servicing MCP calls.
            // The rejection counter was checked before scheduling this retry.
            prompt_finished = tokio_util::sync::CancellationToken::new();
            let signal = prompt_finished.clone();
            let bridge = Arc::clone(p.bridge);
            prompt_task = tokio::spawn(async move {
                let _finished = signal.drop_guard();
                bridge.send_message(session_id, MessageContent::Text(feedback)).await
            });
            prompt_joined = false;
            prompt_success = false;
        }
        let event = if prompt_success && !candidates.is_empty() {
            candidates.pop_front().ok_or_else(|| StageError::Bridge("missing buffered outcome".into()))?
        } else if let Some(event) = deferred_events.pop_front() {
            if is_legacy_stage_control(&event) && !p.bridge.legacy_stage_event_adapter() { continue; }
            event
        } else if prompt_success && !p.bridge.legacy_stage_event_adapter() {
            match events.try_recv() {
                Ok(event) => {
                    if is_legacy_stage_control(&event) && !p.bridge.legacy_stage_event_adapter() { continue; }
                    event
                },
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => return Err(record_usage_stream_lag(p.writer, session_id, skipped).await?),
                Err(_) if missing_outcome_reminders < MAX_MISSING_OUTCOME_REMINDERS => {
                    // Agents routinely finish real work and then end the turn
                    // without calling the stage tool. Failing the run there
                    // discards completed work; ask for the outcome instead,
                    // a bounded number of times, in the same session.
                    missing_outcome_reminders += 1;
                    tracing::warn!(
                        target: "engine::stage::agent",
                        node = %p.node,
                        session = %session_id,
                        reminder = missing_outcome_reminders,
                        max = MAX_MISSING_OUTCOME_REMINDERS,
                        "turn ended without an accepted outcome; requesting it"
                    );
                    retry_feedback = Some(missing_outcome_prompt(p.declared_outcomes));
                    continue;
                },
                Err(_) => return Err(StageError::Bridge(format!(
                    "prompt completed without an accepted, valid outcome candidate \
                     (agent ended {} turns without an accepted report_stage_outcome call)",
                    missing_outcome_reminders + 1
                ))),
            }
        } else { tokio::select! {
            biased;
            () = p.cancel.cancelled() => return Err(StageError::Cancelled),
            _ = deadline_poll.tick() => {
                session_dispatcher.poll_wall_clock_deadline();
                let trips = append_loop_escalations(p.writer, &session_dispatcher).await?;
                // Every loop-protection trip ends the attempt (v1 task 1.3):
                // the engine routes it as a failed attempt into the stage's
                // retry ladder, or fails the run when the stage has none.
                if let Some(trip) = trips.into_iter().next() {
                    return Err(StageError::LoopGuardTripped(trip));
                }
                continue;
            },
            request = stage_requests.recv() => {
                let Some(request) = request else { return Err(StageError::Bridge("stage MCP endpoint ended".into())); };
                let Some(key) = stage_calls.admit(request) else { continue; };
                match stage_calls.event(p.writer, &key, &accepted_outcomes).await? {
                    Some(event) => event,
                    None => continue,
                }
            },
            joined = &mut prompt_task, if !prompt_joined => {
                prompt_joined = true;
                persist_queued_usage(&mut events, &mut deferred_events, session_id, p.writer).await?;
                let sent = joined.map_err(|error| StageError::Bridge(format!("prompt task: {error}")))?;
                if let Err(error @ surge_acp::bridge::error::SendMessageError::RateLimited { .. }) = &sent
                    && let (Some(owner), Some(quota)) = (&p.quota_owner, &mut task_quota_cycle)
                {
                    owner.record_rate_limit(
                        quota,
                        provider_invocation,
                        session_id,
                        error,
                        p.worktree_path,
                        p.agent_registry.as_deref(),
                        execution_writer,
                    )?;
                }
                sent.map_err(|e| match e {
            surge_acp::bridge::error::SendMessageError::RateLimited {
                retry_after,
                details,
            } => StageError::RateLimited {
                // The resolved profile's runtime registry id, normalized
                // through the same `surge_acp::Registry` the engine already
                // used to derive `agent_kind` above — so "claude",
                // "claude-code", and "claude-acp" (aliases for one entry,
                // see `Registry::normalize_agent_id`) collapse to the same
                // string instead of quietly fragmenting one runtime's
                // observations across three keys. `None` **only** via the
                // no-profile-registry legacy path (no runtime block to read
                // an id from). When `agent_id` does not resolve through the
                // registry at all despite `agent_kind` deriving successfully
                // above — a **real**, shipped case, not an unreachable one:
                // the bundled `mock` profile (`bundled/profiles/
                // mock-1.0.toml`, `agent_id = "mock"`) is special-cased for
                // `AgentKind` derivation (`derive_agent_kind_from_id`'s
                // `agent_id == "mock"` arm) without ever touching the
                // registry, so `normalize_agent_id` legitimately returns
                // `None` for it — this falls back to the **raw** `agent_id`
                // (mirroring `SessionOpened.agent_id`'s construction above)
                // rather than discarding the identity: a mock-profile run's
                // capacity signal must still key consistently, not vanish
                // into `None` for want of a registry entry.
                //
                // This is NOT a distinct-login identifier — `RuntimeCfg::
                // agent_id`'s own doc calls it "the agent runtime this
                // profile targets", and every profile pointed at the same
                // runtime (the common case: one CLI, one logged-in session)
                // normalizes to the same string regardless of how many
                // profiles reference it. Whether Surge can ever observe two
                // distinct logins sharing one runtime is an open question
                // for the capacity ledger's design (M2), not settled here.
                runtime: resolved_profile.as_ref().map(|_|logical_runtime.clone()),
                retry_after,
                details,
            },
            other => StageError::Bridge(format!("send_message: {other}")),
        })?;
                prompt_success = true;
    // Record each steer delivery only after the prompt was actually sent, so a
    // failed `send_message` never leaves a `SteerDelivered` claiming otherwise.
    for steer in p.steers.iter().filter(|_| rejection_budget.attempts() == 0) {
        p.writer
            .append_event(VersionedEventPayload::new(EventPayload::SteerDelivered {
                id: steer.id.clone(),
                node: p.node.clone(),
                message: steer.message.clone(),
            }))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
    }


                continue;
            },
            recv = events.recv() => match recv {
                Ok(ev) => {
                    if is_legacy_stage_control(&ev) && !p.bridge.legacy_stage_event_adapter() { continue; }
                    ev
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => return Err(record_usage_stream_lag(p.writer, session_id, skipped).await?),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err(StageError::Bridge(
                        "event stream closed unexpectedly".into(),
                    ));
                },
            },
        }};

        // Filter events for this session only.
        if event_session_id(&event) != Some(session_id) {
            continue;
        }
        // Any event from this session is progress for loop protection.
        session_dispatcher.note_activity();

        if matches!(event, BridgeEvent::OutcomeReported { .. }) && !prompt_success {
            if candidates.len() >= 64 {
                return Err(StageError::Bridge("too many outcomes before prompt completion".into()));
            }
            candidates.push_back(event);
            continue;
        }

        match event {
            BridgeEvent::OutcomeReported {
                outcome,
                summary,
                artifacts_produced,
                verification_report,
                ..
            } => {
                let acceptance = super::outcome_acceptance::accept(
                    &super::outcome_acceptance::AcceptanceContext {
                        stage: p,
                        effective_hooks: &effective_hooks,
                        sandbox_mode: sandbox_cfg.mode,
                        resolved_profile: resolved_profile.as_ref(),
                        verification_input: verification_input.as_ref(),
                        verification_input_seq,
                        stage_context: &stage_calls.context,
                        provider_invocation,
                        session_id,
                    },
                    super::outcome_acceptance::Candidate {
                        outcome,
                        summary,
                        artifacts_produced,
                        verification_report,
                    },
                    &mut rejection_budget,
                )
                .await?;
                match acceptance {
                    super::outcome_acceptance::Acceptance::Accepted(outcome) => break outcome,
                    super::outcome_acceptance::Acceptance::Rejected { feedback, remaining } => {
                        tracing::debug!(target: "engine::stage::agent", node = %p.node, remaining,
                            "awaiting a retried outcome");
                        retry_feedback = Some(feedback);
                    },
                }
            },
            BridgeEvent::PermissionRequested {
                request_id,
                tool,
                capability,
                options,
                ..
            } => {
                handle_permission_request(
                    p,
                    &effective_approval_cfg,
                    &prompt_finished,
                    session_id,
                    PermissionRequest { request_id, tool, capability, options },
                )
                .await?;
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
                session_disposition = Some(disposition);
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
                    writer: Some(p.writer),
                    invocation: Some(provider_invocation),
                    run_id: p.run_id,
                    session_id,
                    worktree_root: p.worktree_path,
                    run_memory: p.run_memory,
                };

                // pre_tool_use hooks gate the dispatch. A Reject short-circuits
                // the call: we send a synthetic tool-error reply and continue
                // the agent loop without invoking the dispatcher.
                let hook_ctx = HookContext::for_node(p.node)
                    .with_writer(p.writer, provider_invocation)
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
                    stage_calls
                        .reply(p.writer, p.bridge.as_ref(),
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

                // Resolve which MCP server (if any) serves this tool so
                // delegation is attributable per server in the replay
                // log. Engine-built-in tools resolve to `None`.
                let mcp_server = session_dispatcher.resolved_origin(&tool);

                // ToolCalled is durable before dispatch so a crash mid-call leaves
                // an unresolved call in the log; the resumed stage is told its
                // outcome is unknown (ADR-0021 decision 4).
                let args_redacted_hash = ContentHash::compute(args_redacted_json.as_bytes());
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ToolCalled {
                        session: session_id,
                        tool: tool.clone(),
                        args_redacted: args_redacted_hash,
                        mcp_server: mcp_server.clone(),
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let engine_result = session_dispatcher.dispatch(&ctx, &call).await;

                // Surface MCP restart-exhaustion as a replay-safe
                // `EscalationRequested` (fold pass-through). Emitted
                // BEFORE the fallible `ToolResultReceived` append: if
                // that storage write fails, the `?` returns and the
                // escalation would otherwise be silently dropped —
                // making permanent MCP failure invisible to the AFK
                // operator (the cockpit only renders give-up from this
                // event). Relative event-seq order vs `ToolResultReceived`
                // is immaterial (both are fold pass-throughs). Only the
                // stable give-up fact is recorded — the non-deterministic
                // attempt count is in the message, not a folded field.
                for esc in session_dispatcher.drain_mcp_escalations() {
                    p.writer
                        .append_event(VersionedEventPayload::new(
                            EventPayload::EscalationRequested {
                                stage: None,
                                reason: format!(
                                    "MCP server '{}' restart policy exhausted after {} attempts; \
                                     calls to it fail until the run is restarted",
                                    esc.server, esc.attempts
                                ),
                                cause: EscalationCause::McpRestartsExhausted,
                            },
                        ))
                        .await
                        .map_err(|e| StageError::Storage(e.to_string()))?;
                }

                // Loop-guard trips (repeated tool call or wall-clock
                // deadline) surface as `EscalationRequested` the same way —
                // mirrors the MCP block above (R39: "raising
                // EscalationRequested ... rather than burning budget").
                // Emitted before the fallible `ToolResultReceived` append
                // for the same reason: a storage failure must not silently
                // drop the escalation. The trip already refused this exact
                // dispatch (see `check_loop_guard`); the attempt ends once
                // this call's result is recorded and delivered (v1 task 1.3).
                if let Some(trip) = append_loop_escalations(p.writer, &session_dispatcher)
                    .await?
                    .into_iter()
                    .next()
                {
                    guard_trip = Some(trip);
                }

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
                            mcp_server,
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
                stage_calls
                    .reply(p.writer, p.bridge.as_ref(),session_id, call_id, acp_result)
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
            BridgeEvent::ProviderUsageObserved { usage, .. } => {
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ProviderUsageObserved {
                        session: session_id,
                        usage,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
            },
            BridgeEvent::ContextUsageObserved { usage, .. } => {
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ContextUsageObserved {
                        session: session_id,
                        usage,
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

                let (tx, rx) = tokio::sync::oneshot::channel();
                p.tool_resolutions.lock().await.insert(call_id.clone(), tx);
                let requested = p.writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::HumanInputRequested {
                            node: p.node.clone(),
                            session: Some(session_id),
                            call_id: Some(call_id.clone()),
                            prompt,
                            schema: None,
                        },
                    ))
                    .await;
                if let Err(error) = requested {
                    p.tool_resolutions.lock().await.remove(&call_id);
                    return Err(StageError::Storage(error.to_string()));
                }

                let resolved = tokio::select! {
                    biased;
                    () = p.cancel.cancelled() => Err(StageError::Cancelled),
                    () = prompt_finished.cancelled() => Err(StageError::Bridge("prompt ended while human input was pending".into())),
                    () = stage_calls.disconnected(&call_id) => Err(StageError::Bridge("MCP caller disconnected while human input was pending".into())),
                    response = rx => response.map(Some).map_err(|_| StageError::Cancelled),
                    () = tokio::time::sleep(p.human_input_timeout) => Ok(None),
                };

                p.tool_resolutions.lock().await.remove(&call_id);

                if let Some(response) = resolved? {
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
                    stage_calls
                        .reply(p.writer, p.bridge.as_ref(),
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
                    stage_calls
                        .reply(p.writer, p.bridge.as_ref(),
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

    Ok(outcome)
    }.await;
    let endpoint_closed = stage_endpoint.close().await;
    let closed = p.bridge.close_session(session_id).await;
    if !prompt_joined {
        // A failed close leaves resource ownership in the bridge. Stop only this
        // caller task so an unconfirmed cleanup cannot hang the engine driver.
        if closed.is_err() {
            prompt_task.abort();
        }
        if let Err(error) = prompt_task.await
            && (!error.is_cancelled() || closed.is_ok())
        {
            return Err(StageError::Bridge(format!(
                "prompt cleanup: {error}; close: {closed:?}; stage: {stage_result:?}"
            )));
        }
    }
    persist_queued_usage(&mut events, &mut deferred_events, session_id, p.writer).await?;
    // Outcome validation and artifact persistence already finished above.
    // Reaped forced cleanup must not discard that result; unconfirmed cleanup
    // still prevents the next stage from starting.
    let forced_cleanup = matches!(
        &closed,
        Err(surge_acp::bridge::error::CloseSessionError::GracefulTimedOut { killed: true, .. })
    );
    if forced_cleanup {
        tracing::warn!(%session_id, "stage session was forcibly closed and reaped");
    } else {
        closed.map_err(|error| match &stage_result {
            Err(stage_error) => StageError::Bridge(format!(
                "stage failed: {stage_error}; session cleanup also failed: {error}"
            )),
            Ok(outcome) => StageError::Bridge(format!(
                "session cleanup failed: {error}; stage outcome: {outcome}"
            )),
        })?;
    }
    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::SessionClosed {
            session: session_id,
            disposition: session_disposition.unwrap_or(
                if stage_result.is_ok() && !forced_cleanup {
                    SessionDisposition::Normal
                } else {
                    SessionDisposition::ForcedClose
                },
            ),
        }))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    endpoint_closed.map_err(|error| {
        StageError::Bridge(format!(
            "stage endpoint cleanup: {error}; stage: {stage_result:?}"
        ))
    })?;
    if matches!(stage_result, Err(StageError::RateLimited { .. })) {
        *exhausted = task_quota_cycle;
    }
    stage_result
}

async fn apply_memory_claim_pack(
    p: &AgentStageParams<'_>,
    resolved: &mut [(surge_core::agent_config::TemplateVar, String)],
) -> Result<Option<surge_core::context_pack::PackReceipt>, StageError> {
    let has_memory_binding = p.agent_config.bindings.iter().any(|binding| {
        matches!(
            &binding.source,
            ArtifactSource::RunArtifact { name } if name == "project_memory"
        )
    });
    if !has_memory_binding {
        return Ok(None);
    }
    let Some(snapshot_ref) = p
        .run_memory
        .artifacts
        .get(crate::engine::engine::MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME)
    else {
        return Ok(None);
    };
    let bytes = p
        .artifact_store
        .open(p.run_id, snapshot_ref.hash)
        .await
        .map_err(|error| StageError::Storage(format!("memory candidate snapshot read: {error}")))?;
    let snapshot: crate::project_context::MemoryClaimSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| {
            StageError::Internal(format!("memory candidate snapshot decode: {error}"))
        })?;
    let (body, receipt) =
        crate::project_context::render_memory_claims_pack(snapshot.claims, snapshot.budget);
    if let Some(body) = body {
        for (binding, (_, value)) in p.agent_config.bindings.iter().zip(resolved.iter_mut()) {
            if matches!(
                &binding.source,
                ArtifactSource::RunArtifact { name } if name == "project_memory"
            ) {
                if !value.is_empty() {
                    value.push('\n');
                }
                value.push_str(&body);
            }
        }
    }
    Ok(Some(receipt))
}

/// Follow-up turns granted to an agent that ends its turn without calling
/// `report_stage_outcome`, before the stage fails.
const MAX_MISSING_OUTCOME_REMINDERS: u32 = 2;

/// Follow-up turn after the agent ended its turn without reporting.
fn missing_outcome_prompt(declared: &[OutcomeDecl]) -> String {
    let outcomes = if declared.is_empty() {
        "done".to_string()
    } else {
        declared
            .iter()
            .map(|o| format!("`{}` ({})", o.id.as_str(), o.description.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Your turn ended without an accepted report_stage_outcome call, so this stage has no result yet. \
         Do not redo finished work: check the current state of the workspace, finish anything \
         still missing, then call report_stage_outcome with a new unique call_id. \
         Allowed outcomes: {outcomes}."
    )
}

/// Inline handler for `BridgeEvent::PermissionRequested` that:
///   1. appends `SandboxElevationRequested` to the event log,
///   2. registers the request in `PendingElevations`,
///   3. `select!`s between the operator's decision and `elevation_timeout`,
///   4. appends `SandboxElevationDecided` (and `SandboxElevationTimedOut` on
///      the timeout path),
///   5. calls `AcpBridge::reply_to_permission` to release the agent.
///
/// Holding the agent stage on this handler matches ACP semantics: the agent
/// itself is blocked on its `request_permission` call until surge replies,
/// so there is no concurrent agent activity to drain.
struct PermissionRequest {
    request_id: String,
    tool: String,
    capability: String,
    options: Vec<String>,
}

#[allow(clippy::too_many_lines)]
async fn handle_permission_request(
    p: &AgentStageParams<'_>,
    effective_approval_cfg: &surge_core::approvals::ApprovalConfig,
    prompt_finished: &tokio_util::sync::CancellationToken,
    session_id: surge_core::id::SessionId,
    request: PermissionRequest,
) -> Result<(), StageError> {
    use agent_client_protocol::schema::v1::{
        PermissionOptionId, RequestPermissionOutcome, RequestPermissionResponse,
        SelectedPermissionOutcome,
    };
    use surge_core::run_event::ElevationDecision;
    use tokio::time::sleep;
    let PermissionRequest {
        request_id,
        tool,
        capability,
        options,
    } = request;

    tracing::info!(
        target: "surge_orch.elevation",
        session = ?session_id,
        request_id = %request_id,
        capability = %capability,
        tool = %tool,
        "elevation requested by agent — appending SandboxElevationRequested",
    );
    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::SandboxElevationRequested {
                node: p.node.clone(),
                capability: capability.clone(),
            },
        ))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    let (rx, pending_count) = p
        .pending_elevations
        .register(crate::engine::elevation::PendingElevation {
            session: session_id,
            request_id: request_id.clone(),
            node: p.node.clone(),
            capability: capability.clone(),
            tool: tool.clone(),
            options: options.clone(),
            requested_at: chrono::Utc::now(),
        })
        .await;
    if pending_count >= crate::engine::elevation::PENDING_REGISTRY_WARN_THRESHOLD {
        tracing::warn!(
            target: "surge_orch.elevation",
            pending_count,
            "engine pending-elevation registry growing — approval channels may be slow",
        );
    }

    // Resolve the per-stage elevation timeout. The effective config (computed
    // in `execute_agent_stage` via `effective_approvals`) layers
    // `agent_config.approvals_override` over the resolved profile's
    // approvals, so profile-level `elevation_timeout` is honoured when the
    // node does not override it.
    let timeout = effective_approval_cfg.resolved_elevation_timeout();

    tracing::debug!(
        target: "surge_orch.elevation",
        session = ?session_id,
        request_id = %request_id,
        timeout_secs = timeout.as_secs(),
        "awaiting operator decision",
    );

    let outcome = tokio::select! {
        biased;
        () = p.cancel.cancelled() => {
            let _ = p.pending_elevations.cancel(session_id, &request_id).await;
            return Err(StageError::Cancelled);
        },
        () = prompt_finished.cancelled() => {
            let _ = p.pending_elevations.cancel(session_id, &request_id).await;
            return Ok(());
        },
        result = rx => result.ok(),
        () = sleep(timeout) => None,
    };

    let response = if let Some(decision) = outcome {
        tracing::info!(
            target: "surge_orch.elevation",
            session = ?session_id,
            request_id = %request_id,
            decision = ?decision.decision,
            remember = decision.remember,
            "elevation decided by operator",
        );
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::SandboxElevationDecided {
                    node: p.node.clone(),
                    decision: decision.decision,
                    remember: decision.remember,
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;

        let resolved_id = resolve_option_id(
            &decision.option_id,
            decision.decision,
            &options,
            session_id,
            &request_id,
        );
        RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(PermissionOptionId::new(resolved_id)),
        ))
    } else {
        // Timeout path. Remove the pending entry so a late
        // `Engine::resolve_elevation` cannot fire into the void.
        let _ = p.pending_elevations.cancel(session_id, &request_id).await;
        let elapsed_seconds = u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX);
        tracing::warn!(
            target: "surge_orch.elevation",
            session = ?session_id,
            request_id = %request_id,
            elapsed_seconds,
            "elevation timed out — denying by default",
        );
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::SandboxElevationTimedOut {
                    node: p.node.clone(),
                    capability: capability.clone(),
                    elapsed_seconds,
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::SandboxElevationDecided {
                    node: p.node.clone(),
                    decision: ElevationDecision::Deny,
                    remember: false,
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
    };

    if let Err(err) = p
        .bridge
        .reply_to_permission(session_id, request_id.clone(), response)
        .await
    {
        // The bridge rejection is non-fatal to the stage: the agent is
        // already gone (SessionGone) or the request_id is unknown (already
        // resolved). Log and continue — the OutcomeReported / SessionEnded
        // path will pick up the stage status.
        tracing::error!(
            target: "surge_orch.elevation",
            session = ?session_id,
            request_id = %request_id,
            error = ?err,
            "bridge.reply_to_permission failed; agent may have already disconnected",
        );
    }
    Ok(())
}

/// Pick the `option_id` to send back to the agent.
///
/// 1. If `desired` is in the agent's offered `options`, use it verbatim.
/// 2. Else, look for the literal `"allow"` / `"deny"` token in `options`
///    matching the decision kind — this catches the common case where the
///    operator surface assumes the protocol uses "allow"/"deny" but the
///    agent actually offered `"allow-once"` / `"allow-always"`.
/// 3. Else, fall back to the first offered option (or `desired` verbatim
///    if the agent supplied an empty list, which would be a protocol bug
///    on the agent's side).
///
/// Every fallback emits `tracing::warn!` so the regression is observable.
fn resolve_option_id(
    desired: &str,
    decision: surge_core::run_event::ElevationDecision,
    options: &[String],
    session_id: surge_core::id::SessionId,
    request_id: &str,
) -> String {
    use surge_core::run_event::ElevationDecision;

    if options.iter().any(|o| o == desired) {
        return desired.to_string();
    }

    let literal = match decision {
        ElevationDecision::Allow | ElevationDecision::AllowAndRemember => "allow",
        ElevationDecision::Deny => "deny",
    };
    if let Some(literal_match) = options.iter().find(|o| o.as_str() == literal) {
        tracing::warn!(
            target: "surge_orch.elevation",
            session = ?session_id,
            request_id,
            desired = %desired,
            chosen = %literal_match,
            offered = ?options,
            "operator option_id not in agent-offered options; falling back to literal allow/deny match"
        );
        return literal_match.clone();
    }

    if let Some(first) = options.first() {
        tracing::warn!(
            target: "surge_orch.elevation",
            session = ?session_id,
            request_id,
            desired = %desired,
            chosen = %first,
            offered = ?options,
            "operator option_id not in offered options and no literal allow/deny match; falling back to first option"
        );
        return first.clone();
    }

    tracing::warn!(
        target: "surge_orch.elevation",
        session = ?session_id,
        request_id,
        desired = %desired,
        "agent offered no options; replying with desired option_id verbatim — agent may reject"
    );
    desired.to_string()
}

fn usage_stream_lag(skipped: u64) -> StageError {
    StageError::Bridge(format!("usage evidence stream lagged by {skipped} events"))
}

async fn record_usage_stream_lag(
    writer: &RunWriter,
    session: surge_core::SessionId,
    skipped: u64,
) -> Result<StageError, StageError> {
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::ProviderUsageObserved {
                session,
                usage: surge_core::TokenUsageObservation::Unavailable {
                    reason: surge_core::UsageUnavailableReason::Missing,
                    reported: None,
                },
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    Ok(usage_stream_lag(skipped))
}

/// A successful prompt join is a publication barrier for final provider usage.
/// Persist those queued observations before accepting a buffered stage outcome.
/// Other events retain their relative order for the normal stage event loop.
async fn persist_queued_usage(
    events: &mut tokio::sync::broadcast::Receiver<BridgeEvent>,
    deferred: &mut std::collections::VecDeque<BridgeEvent>,
    session: surge_core::SessionId,
    writer: &RunWriter,
) -> Result<(), StageError> {
    for _ in 0..events.len() {
        let event = match events.try_recv() {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                return Err(record_usage_stream_lag(writer, session, skipped).await?);
            },
            Err(_) => break,
        };
        let payload = match event {
            BridgeEvent::ProviderUsageObserved {
                session: actual,
                usage,
            } if actual == session => EventPayload::ProviderUsageObserved { session, usage },
            BridgeEvent::ContextUsageObserved {
                session: actual,
                usage,
            } if actual == session => EventPayload::ContextUsageObserved { session, usage },
            other => {
                deferred.push_back(other);
                continue;
            },
        };
        writer
            .append_event(VersionedEventPayload::new(payload))
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
    }
    Ok(())
}

fn is_legacy_stage_control(event: &BridgeEvent) -> bool {
    matches!(
        event,
        BridgeEvent::OutcomeReported { .. }
            | BridgeEvent::HumanInputRequested { .. }
            | BridgeEvent::ToolCall { .. }
    )
}

fn event_session_id(event: &BridgeEvent) -> Option<surge_core::id::SessionId> {
    match event {
        BridgeEvent::SessionEstablished { session, .. }
        | BridgeEvent::AgentMessage { session, .. }
        | BridgeEvent::ProviderUsageObserved { session, .. }
        | BridgeEvent::ContextUsageObserved { session, .. }
        | BridgeEvent::ToolObserved { session, .. }
        | BridgeEvent::ToolCall { session, .. }
        | BridgeEvent::ToolResult { session, .. }
        | BridgeEvent::OutcomeReported { session, .. }
        | BridgeEvent::HumanInputRequested { session, .. }
        | BridgeEvent::PermissionRequested { session, .. }
        | BridgeEvent::SessionEnded { session, .. } => Some(*session),
        BridgeEvent::Error { session, .. } => *session,
    }
}

/// The canonical agent-runtime id for an already-resolved profile —
/// normalize `rp.profile.runtime.agent_id` through `surge_acp::Registry`,
/// falling back to the raw id when normalization fails despite a profile
/// resolving successfully. `normalize_agent_id` legitimately returns `None`
/// for a real, shipped case: the bundled `mock` profile
/// (`bundled/profiles/mock-1.0.toml`, `agent_id = "mock"`) is special-cased
/// for `AgentKind` derivation without ever touching the registry, so
/// falling back to the raw id here (rather than discarding the identity
/// into `None`) keeps a mock-profile run's capacity signal keying
/// consistently instead of vanishing for want of a registry entry.
///
/// The **one** place this normalize-or-raw-fallback computation happens —
/// `SessionOpened.agent_id` and `StageError::RateLimited.runtime`
/// (both below) and Task 12 M3's pre-dispatch capacity check
/// ([`resolve_profile_runtime_id`]) all call this, so the three facts can
/// never quietly diverge on what "the runtime" means for the same profile
/// (Task 12 M3, acceptance criterion A).
fn canonical_runtime_id_for(
    agent_registry: Option<&surge_acp::Registry>,
    agent_config: &AgentConfig,
    rp: &ResolvedProfile,
) -> crate::engine::capacity::CanonicalRuntimeId {
    crate::engine::capacity::CanonicalRuntimeId::resolve_for_run(
        agent_registry,
        effective_agent_id(agent_config, rp),
    )
}

/// Rank of a reasoning-effort level, lowest first. `None` for a value this
/// ranking does not know, which is then left exactly as the operator set it.
fn effort_rank(level: &str) -> Option<usize> {
    ["minimal", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .position(|known| known.eq_ignore_ascii_case(level))
}

/// The reasoning level a node runs with: its own choice, raised to the
/// profile's `min_effort` floor when it is lower (or absent). Unranked values
/// on either side are never rewritten.
fn effective_effort<'a>(node_effort: Option<&'a str>, floor: Option<&'a str>) -> Option<&'a str> {
    match (node_effort, floor) {
        (Some(node), Some(floor)) => match (effort_rank(node), effort_rank(floor)) {
            (Some(n), Some(f)) if n < f => Some(floor),
            _ => Some(node),
        },
        (None, floor) => floor,
        (node, None) => node,
    }
}

/// Session options a node asks for (model, reasoning level). `min_effort` is
/// the resolved profile's reasoning floor: a preference, so an agent that does
/// not offer reasoning levels (Claude Code today) keeps its own default
/// instead of failing the stage.
fn node_config_selections(
    agent_config: &AgentConfig,
    min_effort: Option<&str>,
) -> Vec<surge_acp::bridge::session::ConfigSelection> {
    use surge_acp::bridge::session::{ConfigCategory, ConfigSelection};
    let explicit_effort = agent_config.effort_override();
    let effort = effective_effort(explicit_effort, min_effort);
    // Only a value the operator never wrote is best-effort: the floor filling
    // a gap, or raising an explicit choice that was lower than it.
    let effort_is_floor = effort != explicit_effort;
    [
        (ConfigCategory::Model, agent_config.model_override(), false),
        (ConfigCategory::ThoughtLevel, effort, effort_is_floor),
    ]
    .into_iter()
    .filter_map(|(category, value, best_effort)| {
        value.map(|value| ConfigSelection {
            category,
            value: value.to_string(),
            best_effort,
        })
    })
    .collect()
}

/// The provider a node actually runs on: its own
/// [`AgentConfig::runtime_override`] when set, else the profile's
/// `runtime.agent_id`. Every runtime-identity fact (launch, `SessionOpened`,
/// rate-limit attribution, capacity precheck) goes through this.
fn effective_agent_id<'a>(agent_config: &'a AgentConfig, rp: &'a ResolvedProfile) -> &'a str {
    agent_config
        .runtime_override()
        .unwrap_or(rp.profile.runtime.agent_id.as_str())
}

/// [`resolve_profile_runtime_id`] for a concrete node, honouring its
/// provider override — what the capacity precheck must key on.
#[must_use]
pub(crate) fn resolve_node_runtime_id(
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
    agent_registry: Option<&surge_acp::Registry>,
    agent_config: &AgentConfig,
) -> Option<crate::engine::capacity::CanonicalRuntimeId> {
    let registry = profile_registry?;
    let key_ref = surge_core::profile::keyref::parse_key_ref(agent_config.profile.as_ref()).ok()?;
    let resolved = registry.resolve(&key_ref).ok()?;
    Some(canonical_runtime_id_for(
        agent_registry,
        agent_config,
        &resolved,
    ))
}

/// Resolve the canonical agent-runtime id a node's `agent_config.profile`
/// would use, **without** resolving the rest of the profile or opening a
/// session — Task 12 M3's pre-dispatch capacity check
/// (`engine::run_task`) needs only this, before `execute_agent_stage` does
/// its own (separate, fuller) resolve for prompt/hooks/sandbox.
///
/// `None` when there is no profile registry wired (the legacy mock-only
/// path — no runtime identity exists to check capacity for at all) *or*
/// the profile reference itself does not resolve. A genuine profile
/// resolution failure is reported exactly once, by `execute_agent_stage`'s
/// own resolve moments later — this function must not duplicate that
/// error, only silently decline to produce a capacity key when it cannot.
#[must_use]
pub(crate) fn resolve_profile_runtime_id(
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
    agent_registry: Option<&surge_acp::Registry>,
    profile_str: &str,
) -> Option<crate::engine::capacity::CanonicalRuntimeId> {
    let registry = profile_registry?;
    let key_ref = surge_core::profile::keyref::parse_key_ref(profile_str).ok()?;
    let resolved = registry.resolve(&key_ref).ok()?;
    Some(
        crate::engine::capacity::CanonicalRuntimeId::resolve_for_run(
            agent_registry,
            &resolved.profile.runtime.agent_id,
        ),
    )
}

/// Why legacy runtime-only capacity routing refuses a rotation target.
///
/// Runtime kind cannot distinguish accounts or configured authentication sources.
/// Task-owned routing instead freezes explicit registry route declarations and
/// opaque host-authenticated source snapshots in `engine::capacity_routes`.
/// Those snapshots prove configured source identity, never an observed account.
/// Ordinary unowned flow rotation requires the shared ownership coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationRefusal {
    /// The current node's own `agent_id` does not resolve to a registry
    /// entry with a known `runtime` — nothing to rotate *from*.
    CurrentRuntimeUnresolved,
    /// The candidate profile's `agent_id` does not resolve to a registry
    /// entry with a known `runtime` — nothing to rotate *to*.
    CandidateRuntimeUnresolved,
    /// Both resolved, but to different runtimes — rotation, as specified,
    /// is "the next profile of the **same** runtime" (a different account
    /// on the same agent CLI), not a switch to a different agent entirely.
    DifferentRuntimes {
        /// Runtime the current node's profile targets.
        current: surge_core::RuntimeKind,
        /// Runtime the candidate profile targets.
        candidate: surge_core::RuntimeKind,
    },
    /// Both resolved to the identical runtime — the case R41 originally
    /// asked for. Refused anyway: one runtime has exactly one launch
    /// configuration today, so this candidate is the same account, and
    /// "rotating" to it is a no-op that would silently repeat the
    /// already-exhausted dispatch.
    SameRuntimeIsSameAccount {
        /// The runtime both profiles resolve to.
        runtime: surge_core::RuntimeKind,
    },
}

/// Refuse a rotation based only on runtime kind. Task-owned configured routing
/// uses the stronger host-frozen source identity path; this legacy verifier
/// grants no account or provider-opening authority.
#[must_use]
pub fn verify_rotation_target(
    registry: &surge_acp::Registry,
    current_agent_id: &str,
    candidate_agent_id: &str,
) -> RotationRefusal {
    let runtime_of = |agent_id: &str| registry.find_normalized(agent_id).and_then(|e| e.runtime);

    let Some(current) = runtime_of(current_agent_id) else {
        return RotationRefusal::CurrentRuntimeUnresolved;
    };
    let Some(candidate) = runtime_of(candidate_agent_id) else {
        return RotationRefusal::CandidateRuntimeUnresolved;
    };
    if current != candidate {
        return RotationRefusal::DifferentRuntimes { current, candidate };
    }
    RotationRefusal::SameRuntimeIsSameAccount { runtime: current }
}

/// Resolve `profile_str` into an `AgentKind` through the legacy M5 fallback
/// path.
///
/// Only called when no profile registry is wired (the caller already
/// resolved a profile and used [`derive_agent_kind_from_id`] otherwise):
/// every profile resolves to `AgentKind::Mock` with a one-time WARN so the
/// legacy test path keeps working while production wiring is encouraged.
fn derive_agent_kind(profile_str: &str) -> AgentKind {
    if profile_str != "mock" && !profile_str.starts_with("mock@") {
        tracing::warn!(
            target: "engine::stage::agent",
            profile = %profile_str,
            "no profile_registry wired; falling back to AgentKind::Mock (legacy M5 path)"
        );
    }
    AgentKind::Mock { args: vec![] }
}

/// Map an `agent_id` string to an `AgentKind` via the agent registry,
/// together with the agent's resolved spawn environment and the settings
/// files its entry declares.
///
/// Pulled out so [`execute_agent_stage`] can call it directly when the
/// caller already resolved the profile and just needs the id translated.
///
/// `registry` is the merged catalog (user `[agents.*]` over builtins). The
/// launch contract is the entry itself — `command`, `default_args`, `env`,
/// `settings_files` — and nothing in this function branches on a vendor:
/// adding a provider is adding a registry entry.
///
/// Environment resolution happens here (not in `surge-core`, which is
/// I/O-free): the entry's `env` spec is turned into concrete `(name, value)`
/// pairs through [`surge_acp::agent_env::resolve`], reading the operator's
/// process environment at stage time. A required-but-unset source variable
/// fails the stage with a typed, actionable `StageError::Internal` — the run
/// never spawns an agent with a silently absent credential.
pub(crate) fn derive_agent_kind_from_id(
    profile_str: &str,
    agent_id: &str,
    registry: Option<&surge_acp::Registry>,
) -> Result<AgentLaunch, StageError> {
    // Debug-only test seam: force the in-process mock agent regardless of the
    // profile's runtime, so CLI/plumbing smoke tests can start runs without
    // spawning (and then having to tear down) a real ACP subprocess. Gated on
    // `debug_assertions` so release builds never honor it.
    if cfg!(debug_assertions) && std::env::var_os("SURGE_FORCE_AGENT_MOCK").is_some() {
        tracing::debug!(
            target: "engine::stage::agent",
            profile = %profile_str,
            "SURGE_FORCE_AGENT_MOCK set (debug build); using AgentKind::Mock"
        );
        return Ok(AgentLaunch::mock());
    }
    if agent_id.is_empty() {
        return Err(StageError::Internal(format!(
            "profile {profile_str:?} has empty runtime.agent_id"
        )));
    }
    if agent_id == "mock" {
        return Ok(AgentLaunch::mock());
    }
    let agent_registry = crate::engine::capacity::run_agent_registry_or_builtin(registry);
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
            "profile {profile_str:?} references agent_id {agent_id:?} not present in the agent registry. Known ids and aliases: {known}"
        ))
    })?;
    let entry = agent_registry.find(&normalized_agent_id).ok_or_else(|| {
        StageError::Internal(format!(
            "normalized agent_id {normalized_agent_id:?} missing from the agent registry"
        ))
    })?;
    let binary = std::path::PathBuf::from(&entry.command);
    let extra_args = entry.default_args.clone();
    // The registry launches agents through a wrapper (`npx`, `uvx`, a native
    // binary), and `default_args` already encodes the COMPLETE invocation —
    // including any `--acp`-style flag the wrapper needs. Those entries
    // therefore resolve to `Custom`, which spawns `command + args`
    // verbatim. The typed `ClaudeCode`/`Codex`/`GeminiCli` arms exist for
    // the DIRECT-CLI launch model (e.g. `claude --acp`, exercised by the
    // env-gated `real_acp_smoke` test), where `build_agent_command` injects
    // the runtime's subcommand. Mapping a wrapper entry onto a typed arm
    // would double/misplace that flag and break the launch — so the choice
    // is driven by the entry's shape, not by a vendor name.
    let kind = if entry.is_npx() || entry.is_uvx() {
        AgentKind::Custom {
            binary,
            args: extra_args,
        }
    } else {
        match entry.id.as_str() {
            "claude-code" => AgentKind::ClaudeCode { binary, extra_args },
            "codex" => AgentKind::Codex { binary, extra_args },
            "gemini-cli" => AgentKind::GeminiCli { binary, extra_args },
            _ => AgentKind::Custom {
                binary,
                args: extra_args,
            },
        }
    };
    // Resolve the entry's env spec against the operator's environment.
    // `entry.id` names the agent in any missing-variable error.
    let env = surge_acp::agent_env::resolve(&entry.id, &entry.env).map_err(|e| {
        tracing::warn!(
            target: "engine::stage::agent",
            profile = %profile_str,
            agent_id = %agent_id,
            error = %e,
            "agent env resolution failed"
        );
        StageError::Internal(format!(
            "profile {profile_str:?} runtime {agent_id:?} cannot resolve its environment: {e}"
        ))
    })?;
    tracing::debug!(
        target: "engine::stage::agent",
        profile = %profile_str,
        agent_id = %agent_id,
        normalized_agent_id = %entry.id,
        kind = kind.label(),
        env_count = env.len(),
        settings_file_count = entry.settings_files.len(),
        "derived AgentKind from the agent registry"
    );
    Ok(AgentLaunch {
        kind,
        env,
        settings_files: entry.settings_files.clone(),
    })
}

/// An `AgentKind` paired with the concrete spawn environment and settings
/// files resolved from its registry entry. Produced by
/// [`derive_agent_kind_from_id`] and threaded into `SessionConfig` / the
/// worktree seed.
#[derive(Debug)]
pub(crate) struct AgentLaunch {
    pub(crate) kind: AgentKind,
    pub(crate) env: BTreeMap<String, String>,
    settings_files: Vec<surge_core::config::AgentSettingsFile>,
}

impl AgentLaunch {
    fn mock() -> Self {
        Self {
            kind: AgentKind::Mock { args: vec![] },
            env: BTreeMap::new(),
            settings_files: Vec::new(),
        }
    }
}

/// Whether an MCP server may be spawned/exposed for a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpSpawnPolicy {
    /// MCP tools from this server are exposed to the agent stage.
    Allowed,
    /// MCP is denied — the server is not spawned and its tools are
    /// hidden from the catalog.
    Denied,
}

/// Canonical, single-site resolver for whether MCP is permitted under
/// a sandbox intent. Replaces the former `sandbox_allows_mcp_tool`
/// stopgap. The effective mode is the per-server override when set,
/// otherwise the run's mode (an explicit per-server setting wins).
///
/// `ReadOnly` denies MCP entirely (parity with the retired heuristic).
/// `SandboxMode` is `#[non_exhaustive]`; the catch-all **fails closed**
/// so a future stricter tier cannot silently leak MCP access. Deeper
/// OS-level enforcement of the spawned child is delegated to the agent
/// runtime per ADR-0006 (see ADR-0014) — surge configures intent and
/// applies portable hygiene (env/cwd, U1), it does not police syscalls.
pub(crate) fn mcp_spawn_policy(
    run_mode: surge_core::sandbox::SandboxMode,
    server_override: Option<surge_core::sandbox::SandboxMode>,
) -> McpSpawnPolicy {
    use surge_core::sandbox::SandboxMode;
    let effective = server_override.unwrap_or(run_mode);
    match effective {
        SandboxMode::WorkspaceWrite
        | SandboxMode::WorkspaceNetwork
        | SandboxMode::FullAccess
        | SandboxMode::Custom => McpSpawnPolicy::Allowed,
        // `ReadOnly` denies MCP; the `_` arm is the fail-closed default
        // for future stricter `#[non_exhaustive]` tiers.
        _ => McpSpawnPolicy::Denied,
    }
}

/// Emit a one-time operator-visibility WARN when an MCP server runs
/// under an unconstrained intent (`FullAccess`/`WorkspaceNetwork`):
/// surge delegates OS-level enforcement to the runtime per ADR-0006,
/// so the operator must only configure trusted binaries for these.
fn warn_if_unconstrained_mcp(server: &str, effective: surge_core::sandbox::SandboxMode) {
    use surge_core::sandbox::SandboxMode;
    if matches!(
        effective,
        SandboxMode::FullAccess | SandboxMode::WorkspaceNetwork
    ) {
        tracing::warn!(
            target: "mcp::supervisor",
            server = %server,
            mode = ?effective,
            "MCP server runs with an unconstrained sandbox intent; OS-level \
             enforcement is delegated to the runtime (ADR-0006) — configure \
             only trusted binaries for this mode"
        );
    }
}

#[cfg(test)]
mod effort_floor_tests {
    use super::effective_effort;

    #[test]
    fn floor_raises_a_lower_node_effort_and_fills_a_missing_one() {
        assert_eq!(effective_effort(Some("low"), Some("high")), Some("high"));
        assert_eq!(effective_effort(None, Some("medium")), Some("medium"));
    }

    #[test]
    fn a_floor_fill_is_best_effort_but_an_explicit_choice_is_not() {
        use super::node_config_selections;
        let mut cfg: surge_core::agent_config::AgentConfig =
            toml::from_str(r#"profile = "implementer@1.0""#).unwrap();
        let floor_only = node_config_selections(&cfg, Some("medium"));
        assert_eq!(floor_only.len(), 1);
        assert!(floor_only[0].best_effort);
        cfg.custom_fields.insert(
            "runtime".into(),
            toml::from_str::<toml::Value>("effort = \"high\"").unwrap(),
        );
        let explicit = node_config_selections(&cfg, Some("medium"));
        assert!(!explicit[0].best_effort);
    }

    #[test]
    fn floor_never_lowers_or_rewrites_unranked_values() {
        assert_eq!(effective_effort(Some("max"), Some("high")), Some("max"));
        assert_eq!(effective_effort(Some("turbo"), Some("high")), Some("turbo"));
        assert_eq!(effective_effort(Some("low"), None), Some("low"));
        assert_eq!(effective_effort(None, None), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::profile::VerificationCfg;

    #[test]
    fn interrupted_mcp_calls_are_named_before_the_prompt_and_absent_otherwise() {
        assert_eq!(
            prepend_interrupted_mcp_calls("Do it.".into(), &[]),
            "Do it."
        );
        let calls = [surge_core::run_state::UnresolvedMcpCall {
            session: surge_core::id::SessionId::new(),
            tool: "add_comment".into(),
            server: "github".into(),
            seq: 42,
        }];
        let prompt = prepend_interrupted_mcp_calls("Do it.".into(), &calls);
        assert!(prompt.starts_with("## Interrupted tool calls"));
        assert!(prompt.contains("`add_comment` on MCP server `github` (event 42)"));
        assert!(prompt.contains("outcome is unknown"));
        assert!(prompt.ends_with("Do it."));
    }

    #[test]
    fn previous_attempt_feedback_names_outcome_summary_and_open_checks() {
        use surge_core::roadmap::{
            VerificationCheck, VerificationCheckResult, VerificationReportArtifact,
            VerificationReportOutcome,
        };
        let from = NodeKey::try_from("verify_1").unwrap();
        let record = surge_core::run_state::OutcomeRecord {
            outcome: OutcomeKey::try_from("failed").unwrap(),
            summary: "Login form accepts empty passwords.".into(),
            seq: 7,
        };
        assert_eq!(render_previous_attempt_feedback(&from, None, None), None);
        let empty = surge_core::run_state::OutcomeRecord {
            summary: "  ".into(),
            ..record.clone()
        };
        assert_eq!(
            render_previous_attempt_feedback(&from, Some(&empty), None),
            None
        );

        let check = |command: &str, result, note: Option<&str>| VerificationCheck {
            command: command.into(),
            result,
            covers: Vec::new(),
            note: note.map(Into::into),
        };
        let mut checks = vec![
            check("cargo nextest run", VerificationCheckResult::Passed, None),
            check(
                "cargo nextest run login",
                VerificationCheckResult::Failed,
                Some("empty_password_is_rejected panicked"),
            ),
        ];
        checks.extend((0..FEEDBACK_MAX_CHECKS).map(|n| {
            check(
                &format!("probe {n}"),
                VerificationCheckResult::Skipped,
                None,
            )
        }));
        let report = VerificationReportArtifact {
            task_id: "T1".into(),
            outcome: VerificationReportOutcome::Failed,
            summary: "One acceptance criterion is unmet.".into(),
            checks,
            ..VerificationReportArtifact::default()
        };
        let text = render_previous_attempt_feedback(&from, Some(&record), Some(&report)).unwrap();
        assert!(text.starts_with("## Feedback from the previous attempt\n"));
        assert!(text.contains("Stage `verify_1` sent this work back with outcome `failed`"));
        assert!(text.contains("Summary: Login form accepts empty passwords."));
        assert!(text.contains("Verifier summary: One acceptance criterion is unmet."));
        assert!(
            text.contains(
                "- `cargo nextest run login` — failed: empty_password_is_rejected panicked"
            )
        );
        assert!(!text.contains("`cargo nextest run` —"));
        assert!(text.contains("- …and 1 more"));

        let long = "x".repeat(FEEDBACK_FIELD_MAX_CHARS + 50);
        let clipped = surge_core::run_state::OutcomeRecord {
            summary: long,
            ..record
        };
        let text = render_previous_attempt_feedback(&from, Some(&clipped), None).unwrap();
        assert!(text.contains(&format!("{}…", "x".repeat(FEEDBACK_FIELD_MAX_CHARS))));
        assert!(!text.contains(&"x".repeat(FEEDBACK_FIELD_MAX_CHARS + 1)));
    }

    #[test]
    fn requirement_revisions_apply_to_their_task_and_run_wide() {
        use surge_core::run_state::RequirementRevision;
        let revision = |task: Option<&str>, text: &str, seq| RequirementRevision {
            node: NodeKey::try_from("verify").unwrap(),
            task: task.map(surge_core::roadmap::RoadmapTaskId::from),
            text: text.into(),
            seq,
        };
        let revisions = [
            revision(Some("login"), "Allow empty passwords in dev", 3),
            revision(Some("logout"), "Keep sessions on logout", 4),
            revision(None, "Use UTC everywhere", 5),
        ];
        let login = surge_core::roadmap::RoadmapTaskId::from("login");
        let prompt = prepend_requirement_revisions("Do it.".into(), &revisions, Some(&login));
        assert!(prompt.starts_with("## Requirement revised by the operator"));
        assert!(prompt.contains("- Allow empty passwords in dev"));
        assert!(prompt.contains("- Use UTC everywhere"));
        assert!(!prompt.contains("Keep sessions on logout"));
        assert!(prompt.ends_with("Do it."));
        assert_eq!(
            prepend_requirement_revisions("Do it.".into(), &revisions[..2], None),
            "Do it."
        );
    }

    #[test]
    fn completion_contract_requires_tool_submission_with_actual_outcomes() {
        let outcomes = vec![OutcomeKey::try_from("ready_for_verification").unwrap()];
        let prompt = append_completion_contract("Implement the app.".into(), &outcomes);
        assert!(prompt.starts_with("Implement the app."));
        assert!(prompt.contains("call the available report_stage_outcome tool"));
        assert!(prompt.contains("ready_for_verification"));
        assert!(prompt.contains("final text message alone"));
        assert!(prompt.contains("unique ID for each revised report"));
        assert!(prompt.contains("does not certify verification or approval"));
    }

    #[test]
    fn derive_agent_kind_carries_entry_env_and_settings_files() {
        // A custom provider in the merged registry is a first-class runtime:
        // its env spec and settings files flow into the launch with no
        // vendor-specific code anywhere.
        use std::collections::{BTreeMap, HashMap};
        use surge_core::config::{AgentEnvValue, AgentSettingsFile};

        let mut env = BTreeMap::new();
        env.insert(
            "PROVIDER_BASE_URL".to_string(),
            AgentEnvValue::Literal("https://example.invalid".to_string()),
        );
        let mut agents = HashMap::new();
        agents.insert(
            "my-provider".to_string(),
            surge_core::config::AgentConfig {
                command: "my-agent".to_string(),
                args: vec!["--acp".to_string()],
                transport: surge_core::config::Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
                env,
                capacity_route: None,
                settings_files: vec![AgentSettingsFile::new(
                    ".my-agent/settings.json",
                    "{\"mode\":\"headless\"}\n",
                )],
            },
        );
        let registry = surge_acp::Registry::from_config(agents);

        let launch = derive_agent_kind_from_id("implementer@1.0", "my-provider", Some(&registry))
            .expect("custom provider must resolve");
        assert_eq!(launch.kind.label(), "custom");
        assert_eq!(
            launch.env.get("PROVIDER_BASE_URL").map(String::as_str),
            Some("https://example.invalid"),
        );
        assert_eq!(launch.settings_files.len(), 1);
        assert_eq!(launch.settings_files[0].path, ".my-agent/settings.json");
    }

    #[test]
    fn derive_agent_kind_refuses_a_required_env_var_that_is_unset() {
        use std::collections::{BTreeMap, HashMap};
        use surge_core::config::AgentEnvValue;

        let mut env = BTreeMap::new();
        env.insert(
            "SURGE_TEST_DEFINITELY_UNSET".to_string(),
            AgentEnvValue::Inject {
                from: "SURGE_TEST_DEFINITELY_UNSET_SOURCE".to_string(),
                default: None,
                required: true,
            },
        );
        let mut agents = HashMap::new();
        agents.insert(
            "strict-provider".to_string(),
            surge_core::config::AgentConfig {
                command: "my-agent".to_string(),
                args: vec![],
                transport: surge_core::config::Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
                env,
                settings_files: vec![],
                capacity_route: None,
            },
        );
        let registry = surge_acp::Registry::from_config(agents);

        let err = derive_agent_kind_from_id("implementer@1.0", "strict-provider", Some(&registry))
            .expect_err("a required-but-unset source must refuse the launch");
        let rendered = err.to_string();
        assert!(
            rendered.contains("SURGE_TEST_DEFINITELY_UNSET_SOURCE"),
            "the refusal must name the missing variable: {rendered}",
        );
    }

    #[test]
    fn builtin_registry_entries_carry_their_settings_files() {
        // The builtin catalog declares the Claude-agent settings seed as
        // data. If that declaration is dropped, the adapter fails at the
        // handshake on any machine whose global mode it rejects — this pins
        // the data, not a code path.
        let registry = surge_acp::Registry::builtin();
        for id in ["claude-acp", "dsh-acp"] {
            let entry = registry.find(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(
                !entry.settings_files.is_empty(),
                "{id} must declare its settings seed",
            );
        }
    }

    #[test]
    fn mcp_spawn_policy_read_only_denies_writable_allows() {
        use surge_core::sandbox::SandboxMode;
        assert_eq!(
            mcp_spawn_policy(SandboxMode::ReadOnly, None),
            McpSpawnPolicy::Denied
        );
        assert_eq!(
            mcp_spawn_policy(SandboxMode::WorkspaceWrite, None),
            McpSpawnPolicy::Allowed
        );
        assert_eq!(
            mcp_spawn_policy(SandboxMode::FullAccess, None),
            McpSpawnPolicy::Allowed
        );
        assert_eq!(
            mcp_spawn_policy(SandboxMode::Custom, None),
            McpSpawnPolicy::Allowed
        );
    }

    #[test]
    fn mcp_spawn_policy_server_override_wins() {
        use surge_core::sandbox::SandboxMode;
        // Restrictive per-server override beats a permissive run mode.
        assert_eq!(
            mcp_spawn_policy(SandboxMode::FullAccess, Some(SandboxMode::ReadOnly)),
            McpSpawnPolicy::Denied
        );
        // And a permissive override is honored over a stricter run.
        assert_eq!(
            mcp_spawn_policy(SandboxMode::ReadOnly, Some(SandboxMode::WorkspaceWrite)),
            McpSpawnPolicy::Allowed
        );
    }

    use surge_core::approvals::ApprovalConfig;
    use surge_core::edge::EdgeKind;
    use surge_core::hooks::{HookFailureMode, HookInheritance, MatcherSpec};
    use surge_core::profile::registry::{Provenance, ResolvedProfile};
    use surge_core::profile::{
        InspectorUi, Profile, ProfileBindings, ProfileHooks, ProfileOutcome, PromptTemplate, Role,
        RoleCategory, RuntimeCfg, ToolsCfg,
    };
    use surge_core::sandbox::SandboxConfig;

    #[tokio::test]
    async fn flow_generator_defers_content_validation_but_other_profiles_do_not() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("flow.toml"), "schema_version = 1\n").unwrap();
        let mut profile = resolved_profile(Vec::new());
        profile.profile = toml::from_str(include_str!(
            "../../../../surge-core/bundled/profiles/flow-generator-1.0.toml"
        ))
        .unwrap();
        let outcome = OutcomeKey::try_from("drafted").unwrap();
        let artifacts = vec!["flow.toml".to_owned()];
        let deferred = super::super::outcome_acceptance::validate_profile_artifact_contracts(
            Some(&profile),
            "flow-generator@1.0",
            &outcome,
            &artifacts,
            directory.path(),
            false,
        )
        .await
        .unwrap();
        assert!(deferred.is_none());
        let rejected = super::super::outcome_acceptance::validate_profile_artifact_contracts(
            Some(&profile),
            "implementer@1.0",
            &outcome,
            &artifacts,
            directory.path(),
            false,
        )
        .await
        .unwrap();
        assert!(rejected.is_some());
        let missing = super::super::outcome_acceptance::validate_profile_artifact_contracts(
            Some(&profile),
            "flow-generator@1.0",
            &outcome,
            &[],
            directory.path(),
            false,
        )
        .await
        .unwrap();
        assert!(missing.is_some());
    }

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
                    color: None,
                    min_effort: None,
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
                verification: VerificationCfg::default(),
            },
            provenance: Provenance::Bundled,
            chain: vec![profile_key],
        }
    }

    #[test]
    fn effective_system_prompt_appends_not_replaces() {
        use surge_core::agent_config::PromptOverride;

        // The minimal-agent case Copilot flagged: a node with an existing
        // `system` override must still receive the fork append, not drop it.
        let mut cfg = agent_config(vec![]);
        cfg.prompt_overrides = Some(PromptOverride {
            system: Some("BASE".into()),
            append_system: Some("retry: prefer X".into()),
        });
        assert_eq!(
            effective_system_prompt(&cfg, None),
            "BASE\n\nretry: prefer X"
        );

        // Append onto the resolved profile's system prompt when the node has no
        // `system` of its own (profile.prompt.system == "Implement").
        let mut cfg = agent_config(vec![]);
        cfg.prompt_overrides = Some(PromptOverride {
            system: None,
            append_system: Some("retry: prefer X".into()),
        });
        let rp = resolved_profile(vec![]);
        assert_eq!(
            effective_system_prompt(&cfg, Some(&rp)),
            "Implement\n\nretry: prefer X"
        );

        // No append → base is returned unchanged.
        let mut cfg = agent_config(vec![]);
        cfg.prompt_overrides = Some(PromptOverride {
            system: Some("BASE".into()),
            append_system: None,
        });
        assert_eq!(effective_system_prompt(&cfg, None), "BASE");

        // Append with an empty base → just the append.
        let mut cfg = agent_config(vec![]);
        cfg.prompt_overrides = Some(PromptOverride {
            system: None,
            append_system: Some("only".into()),
        });
        assert_eq!(effective_system_prompt(&cfg, None), "only");
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

    /// A registry whose one entry ("no-runtime") resolves but carries
    /// `runtime: None` — `Registry::builtin()`'s five entries all populate
    /// `runtime` (verified against `builtin_registry.json`), so this is
    /// the only way to exercise `RotationRefusal::CandidateRuntimeUnresolved`
    /// / `CurrentRuntimeUnresolved` against a *resolving* id rather than an
    /// unknown one. `Registry::from_config` always sets `runtime: None` on
    /// every entry it builds (the custom-agent path has no `RuntimeKind` to
    /// supply), which is exactly the shape case (b) needs.
    fn registry_with_one_runtimeless_entry() -> surge_acp::Registry {
        surge_acp::Registry::from_config(std::collections::HashMap::from([(
            "no-runtime".to_string(),
            surge_core::config::AgentConfig {
                command: "true".to_string(),
                args: vec![],
                transport: surge_core::config::Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
                env: std::collections::BTreeMap::new(),
                settings_files: vec![],
                capacity_route: None,
            },
        )]))
    }

    #[test]
    fn rotation_refused_when_current_agent_id_does_not_resolve() {
        // Case (a): `agent_id` not present in the registry at all.
        let registry = surge_acp::Registry::builtin();
        let refusal = verify_rotation_target(&registry, "totally-unknown-agent", "claude-acp");
        assert_eq!(refusal, RotationRefusal::CurrentRuntimeUnresolved);
    }

    #[test]
    fn rotation_refused_when_current_entry_has_no_runtime() {
        // Case (b), the current side: the entry exists (resolves) but
        // carries `runtime: None`. Candidate is a real, runtime-populated
        // id (`"claude-acp"`) to show the current-side check fires first,
        // regardless of whether the candidate would otherwise resolve.
        let registry = surge_acp::Registry::merged(
            surge_acp::Registry::builtin(),
            registry_with_one_runtimeless_entry(),
        );
        let refusal = verify_rotation_target(&registry, "no-runtime", "claude-acp");
        assert_eq!(refusal, RotationRefusal::CurrentRuntimeUnresolved);
    }

    #[test]
    fn rotation_refused_when_candidate_entry_has_no_runtime() {
        // Case (b), the candidate side: current resolves fine, candidate
        // resolves but has no known runtime.
        let mut registry = surge_acp::Registry::builtin();
        registry = surge_acp::Registry::merged(registry, registry_with_one_runtimeless_entry());
        let refusal = verify_rotation_target(&registry, "claude-acp", "no-runtime");
        assert_eq!(refusal, RotationRefusal::CandidateRuntimeUnresolved);
    }

    #[test]
    fn rotation_refused_when_runtimes_differ() {
        // Case (c): both resolve, to different runtimes.
        let registry = surge_acp::Registry::builtin();
        let refusal = verify_rotation_target(&registry, "claude-acp", "codex-acp");
        assert_eq!(
            refusal,
            RotationRefusal::DifferentRuntimes {
                current: surge_core::RuntimeKind::ClaudeCode,
                candidate: surge_core::RuntimeKind::Codex,
            }
        );
    }

    #[test]
    fn rotation_refused_when_runtimes_are_the_same_account() {
        // Case (d) — the one the original R41 design called "allowed".
        // `"claude"` and `"claude-code"` are both registry aliases that
        // normalize to the identical `"claude-acp"` entry
        // (`REGISTRY_ID_ALIASES`), so this also proves the refusal fires
        // even when the two agent_id spellings differ but the underlying
        // account does not.
        let registry = surge_acp::Registry::builtin();
        let refusal = verify_rotation_target(&registry, "claude", "claude-code");
        assert_eq!(
            refusal,
            RotationRefusal::SameRuntimeIsSameAccount {
                runtime: surge_core::RuntimeKind::ClaudeCode,
            }
        );
    }

    #[test]
    fn resolve_profile_runtime_id_is_none_without_a_profile_registry() {
        // The legacy no-registry path: no runtime identity exists to check
        // capacity for, so the pre-dispatch capacity check must skip
        // entirely rather than fabricate a key.
        assert!(resolve_profile_runtime_id(None, None, "mock").is_none());
    }
}
