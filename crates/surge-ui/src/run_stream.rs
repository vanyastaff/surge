//! Per-run live event streaming — the UI-side fold.
//!
//! The daemon already exposes `DaemonEngineFacade::subscribe_to_run`
//! (per-run broadcast of every durable [`EventPayload`]); until now the
//! UI only consumed the *global* lifecycle stream. This module folds
//! the per-run stream into a small view-state the screens can render:
//! an event log, the live stage pipeline, token/cost counters, and the
//! set of **pending operator decisions** (human inputs, gate approvals,
//! elevation requests, escalations) that powers the Inbox.
//!
//! Semantics note: per-run broadcast does NOT replay history — events
//! emitted before the subscription are not seen (daemon facade docs).
//! Screens must treat this state as "since the cockpit connected", and
//! they say so in the UI.

use std::collections::VecDeque;

use gpui_kit::Hsla;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::handle::EngineRunEvent;

use crate::theme;

/// Cap on retained log rows per run (oldest dropped first).
pub const MAX_LOG_ROWS: usize = 240;

/// Visual tone of a log row / event kind.
#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Info,
    Ok,
    Warn,
    Err,
    Accent,
}

impl Tone {
    pub fn color(self) -> Hsla {
        match self {
            Self::Info => theme::text_muted(),
            Self::Ok => theme::success(),
            Self::Warn => theme::warning(),
            Self::Err => theme::error(),
            Self::Accent => theme::accent(),
        }
    }
}

/// One rendered event-log row.
#[derive(Clone)]
pub struct RunLogRow {
    /// Local receive time (the wire event carries no timestamp).
    pub time: String,
    pub kind: &'static str,
    pub tone: Tone,
    pub text: String,
}

/// Live stage-pipeline entry, folded from Stage* events.
#[derive(Clone)]
pub struct StageRow {
    pub node: String,
    pub attempt: u32,
    pub phase: StagePhase,
    /// Latest outcome / reason / progress detail.
    pub detail: String,
}

/// One actual journal opening, including reconnects to the same provider session.
#[derive(Clone)]
pub struct RecordedSession {
    pub seq: u64,
    pub session: surge_core::SessionId,
    pub node: String,
    pub profile: String,
    pub runtime: Option<String>,
    pub opened: Option<surge_core::execution_recovery::OpenedSession>,
    pub handoff: Option<surge_core::id::WorkItemOperationId>,
    pub closed: Option<(u64, surge_core::run_event::SessionDisposition)>,
}

/// Lifecycle phase of one folded stage row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StagePhase {
    Running,
    Done,
    Failed,
}

/// A decision the engine is blocked on (or loudly asking about).
#[derive(Clone)]
pub struct PendingDecision {
    pub seq: u64,
    pub time: String,
    pub node: String,
    pub kind: DecisionKind,
}

#[derive(Clone)]
pub enum DecisionKind {
    /// `HumanInputRequested` — resolvable via `resolve_human_input`.
    ///
    /// This is the ONLY event a paused gate emits that the operator can
    /// answer in-band. Namespaced `GateRequestId` identifies a gate; other scoped
    /// call IDs identify tool requests. None is a legacy unbound request that
    /// cannot authorize a decision. For gate requests the engine requires `{"outcome": <key>}` where the valid
    /// keys are declared in `schema.properties.outcome.enum`. Bootstrap
    /// gates arrive through this same event (their companion
    /// `BootstrapApprovalRequested` is bookkeeping, not a second
    /// decision — the fold deliberately does not surface it).
    HumanInput {
        call_id: Option<String>,
        prompt: String,
        schema: Option<serde_json::Value>,
    },
    /// `SandboxElevationRequested` — capability grant.
    Elevation { capability: String },
    /// `RoadmapPatchApprovalRequested` — resolved via the
    /// roadmap-amendment flow (`submit_roadmap_amendment`), not
    /// `resolve_human_input`; surfaced as informational.
    RoadmapPatch { patch_id: String },
    /// `EscalationRequested` — the engine gave up and needs a human.
    Escalation { reason: String },
}

impl DecisionKind {
    /// Short badge label for queue rows.
    pub fn badge(&self) -> &'static str {
        match self {
            Self::HumanInput {
                call_id: Some(id), ..
            } if surge_core::id::GateRequestId::from_event_call_id(id).is_some() => "review gate",
            Self::HumanInput {
                call_id: Some(_), ..
            } => "human input",
            Self::HumanInput { call_id: None, .. } => "review gate",
            Self::Elevation { .. } => "elevation",
            Self::RoadmapPatch { .. } => "roadmap patch",
            Self::Escalation { .. } => "escalation",
        }
    }

    /// Urgency rank for the Inbox (lower = more urgent).
    pub fn rank(&self) -> u8 {
        match self {
            Self::Escalation { .. } => 0,
            Self::Elevation { .. } => 1,
            Self::HumanInput { .. } => 2,
            Self::RoadmapPatch { .. } => 4,
        }
    }

    /// Valid outcome keys for a HumanGate decision, parsed from the
    /// request schema (`properties.outcome.enum`). Empty when the gate
    /// allows free-text outcomes or the schema is absent.
    pub fn gate_outcomes(&self) -> Vec<String> {
        let Self::HumanInput {
            schema: Some(schema),
            ..
        } = self
        else {
            return Vec::new();
        };
        if schema.get("x-surge-bootstrap-stage").is_some() {
            return vec!["approve".into(), "edit".into(), "reject".into()];
        }
        schema
            .pointer("/properties/outcome/enum")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Folded live view of one run's event stream.
#[derive(Default)]
pub struct RunStreamState {
    /// Full recorded metadata; independent of the bounded visible event log.
    pub sessions: Vec<RecordedSession>,
    session_history_valid: bool,
    display_run_id: Option<surge_core::RunId>,
    display_state: Option<surge_core::RunState>,
    display_seq: u64,
    trusted_work_item: Option<surge_core::id::WorkItemId>,
    ownership_startup_open: bool,
    ownership_prompt: Option<String>,
    ownership_binding_seen: bool,
    ownership_invalid: bool,
    registry_status: Option<surge_core::RunStatus>,
    /// Immutable produced artifact locations from the durable run stream.
    pub artifacts: std::collections::HashMap<String, std::path::PathBuf>,
    /// Subscription currently attached and pumping.
    pub live: bool,
    pub log: VecDeque<RunLogRow>,
    pub stages: Vec<StageRow>,
    pub pending: Vec<PendingDecision>,
    pending_history: Vec<PendingDecision>,
    pending_replay: bool,
    pending_watermark: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    /// Highest seq observed on this stream.
    pub last_seq: u64,
    /// Where the run executes, as recorded by `RunStarted` (for isolated
    /// runs this is the run's git worktree, not the source checkout).
    pub run_path: Option<std::path::PathBuf>,
    /// Git common directory of `run_path`, resolved once when `RunStarted`
    /// is folded. Linked worktrees share the source repository's common
    /// dir, so this is what ties a worktree run back to its project.
    pub git_common_dir: Option<std::path::PathBuf>,
    /// The operator's request that started the run (`RunStarted`).
    pub prompt: Option<String>,
}

impl RunStreamState {
    /// Start replaying a trusted full history for this actual run identity.
    pub fn begin_display_history(
        &mut self,
        run_id: surge_core::RunId,
        status: Option<surge_core::RunStatus>,
    ) {
        self.display_run_id = Some(run_id);
        self.display_state = Some(surge_core::RunState::NotStarted);
        self.display_seq = 0;
        self.sessions.clear();
        self.session_history_valid = true;
        self.trusted_work_item = None;
        self.ownership_startup_open = true;
        self.ownership_prompt = None;
        self.ownership_binding_seen = false;
        self.ownership_invalid = false;
        self.registry_status = status.or(self.registry_status);
        // Rebuild actionable decisions from the full trusted journal below.
        // Do not expose them until the caller confirms this replay is complete.
        self.pending.clear();
        self.pending_history.clear();
        self.pending_replay = true;
    }

    /// Publish decisions reconstructed from a complete history through `seq`.
    pub fn finish_display_history(&mut self, through_seq: u64) {
        if !self.pending_replay {
            return;
        }
        if self.session_history_confirmed()
            && self.display_seq == through_seq
            && through_seq >= self.pending_watermark
        {
            self.pending = std::mem::take(&mut self.pending_history);
            self.pending_watermark = through_seq;
        } else {
            self.invalidate_display();
        }
        self.pending_replay = false;
    }

    /// A daemon-confirmed start supersedes old registry crash evidence.
    pub fn mark_started(&mut self) {
        self.registry_status = Some(surge_core::RunStatus::Running);
    }

    /// Missing/gapped history is not evidence of either work or daemon death.
    pub fn invalidate_display(&mut self) {
        self.display_state = None;
        self.trusted_work_item = None;
        self.session_history_valid = false;
        self.pending_history.clear();
        self.pending.clear();
    }

    pub fn session_history_confirmed(&self) -> bool {
        self.session_history_valid && self.display_seq > 0 && self.display_state.is_some()
    }

    /// Task ownership from the contiguous trusted journal, retained through terminal history.
    pub fn trusted_work_item(&self) -> Option<surge_core::id::WorkItemId> {
        self.trusted_work_item
    }

    /// A complete contiguous startup prefix distinguishes a legacy run from an unresolved link.
    pub fn task_ownership_confirmed(&self) -> bool {
        self.display_seq > 0
            && self.display_state.is_some()
            && !self.ownership_invalid
            && (!self.ownership_startup_open || self.trusted_work_item.is_some())
    }

    /// Canonical display projection; existing pending decisions remain independent.
    pub fn display(&self) -> surge_core::run_display::RunDisplayState {
        let state = (self.display_seq > 0)
            .then_some(self.display_state.as_ref())
            .flatten();
        surge_core::run_display::RunDisplayState::from_state(state, self.registry_status)
    }

    fn observe_display(&mut self, event: &EngineRunEvent) {
        let EngineRunEvent::Persisted { seq, payload } = event else {
            return;
        };
        if *seq <= self.display_seq {
            return;
        }
        if self.display_seq == 0 && !matches!(payload.as_ref(), EventPayload::RunStarted { .. }) {
            self.invalidate_display();
            return;
        }
        if *seq != self.display_seq.saturating_add(1) {
            self.invalidate_display();
            return;
        }
        let (Some(run_id), Some(state)) = (self.display_run_id, self.display_state.take()) else {
            return;
        };
        self.display_state = surge_core::run_state::apply(
            state,
            &surge_core::RunEvent {
                run_id,
                seq: *seq,
                timestamp: chrono::Utc::now(),
                payload: payload.as_ref().clone(),
            },
        )
        .ok();
        if self.display_state.is_some() {
            match payload.as_ref() {
                EventPayload::SessionOpened {
                    node,
                    session,
                    agent,
                    agent_id,
                    opened,
                    handoff,
                } => {
                    self.sessions.push(RecordedSession {
                        seq: *seq,
                        session: *session,
                        node: node.to_string(),
                        profile: agent.clone(),
                        runtime: agent_id.clone(),
                        opened: opened.clone(),
                        handoff: *handoff,
                        closed: None,
                    });
                },
                EventPayload::SessionClosed {
                    session,
                    disposition,
                } => {
                    if let Some(record) = self
                        .sessions
                        .iter_mut()
                        .rev()
                        .find(|record| record.session == *session && record.closed.is_none())
                    {
                        record.closed = Some((*seq, *disposition));
                    } else {
                        self.session_history_valid = false;
                    }
                },
                _ => {},
            }
            if *seq == 1
                && let EventPayload::RunStarted { initial_prompt, .. } = payload.as_ref()
            {
                self.ownership_prompt = Some(initial_prompt.clone());
            }
            self.ownership_startup_open &= matches!(
                payload.as_ref(),
                EventPayload::RunStarted { .. }
                    | EventPayload::PipelineMaterialized { .. }
                    | EventPayload::ArtifactProduced { .. }
                    | EventPayload::WorkItemAttemptBound { .. }
            );
            if let EventPayload::WorkItemAttemptBound { context } = payload.as_ref() {
                self.ownership_invalid |= !self.ownership_startup_open
                    || self.ownership_binding_seen
                    || self.ownership_prompt.as_deref() != Some(context.prompt().as_str());
                self.ownership_binding_seen = true;
                if !self.ownership_invalid {
                    self.trusted_work_item = Some(context.binding().item);
                }
            }
        }
        if self.display_state.is_none() || self.ownership_invalid {
            self.trusted_work_item = None;
        }
        self.display_seq = *seq;
    }

    /// Fold one wire event into the view state: log row, stage
    /// pipeline, token/cost counters and the pending-decision set.
    pub fn apply(&mut self, event: &EngineRunEvent) {
        self.apply_at(event, chrono::Local::now().format("%H:%M:%S").to_string());
    }

    /// Fold durable history using its recorded time rather than replay time.
    pub fn apply_recorded(&mut self, event: &EngineRunEvent, timestamp_ms: i64) {
        let time = chrono::DateTime::from_timestamp_millis(timestamp_ms)
            .map(|time| {
                time.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| "unknown".into());
        self.apply_at(event, time);
    }

    fn apply_at(&mut self, event: &EngineRunEvent, now: String) {
        self.observe_display(event);
        if self.pending_replay
            && let EngineRunEvent::Persisted { seq, payload } = event
            && self.session_history_valid
            && self.display_seq == *seq
        {
            Self::fold_pending_into(&mut self.pending_history, *seq, &now, payload);
            if matches!(
                payload.as_ref(),
                EventPayload::RunCompleted { .. }
                    | EventPayload::RunFailed { .. }
                    | EventPayload::RunAborted { .. }
            ) {
                self.pending_history.clear();
            }
        }
        match event {
            EngineRunEvent::Persisted { seq, payload } => {
                if *seq <= self.last_seq {
                    return;
                }
                match payload.as_ref() {
                    EventPayload::ArtifactProduced { name, path, .. } => {
                        self.artifacts.insert(name.clone(), path.clone());
                    },
                    EventPayload::RunStarted {
                        project_path,
                        initial_prompt,
                        ..
                    } => {
                        self.git_common_dir = crate::project::git_common_dir(project_path);
                        self.run_path = Some(project_path.clone());
                        let prompt = initial_prompt.trim();
                        self.prompt = (!prompt.is_empty()).then(|| prompt.to_string());
                    },
                    _ => {},
                }
                self.last_seq = self.last_seq.max(*seq);
                self.fold_stages(payload);
                if !self.pending_replay {
                    let pending_is_trusted = self.display_run_id.is_none()
                        || (self.session_history_valid && self.display_seq == *seq);
                    if pending_is_trusted {
                        Self::fold_pending_into(&mut self.pending, *seq, &now, payload);
                    } else {
                        self.pending.clear();
                    }
                    self.pending_watermark = self.pending_watermark.max(*seq);
                }
                self.fold_cost(payload);
                if matches!(
                    payload.as_ref(),
                    EventPayload::RunCompleted { .. }
                        | EventPayload::RunFailed { .. }
                        | EventPayload::RunAborted { .. }
                ) {
                    self.pending.clear();
                }
                if let Some((kind, tone, text)) = describe(payload) {
                    self.log.push_back(RunLogRow {
                        time: now,
                        kind,
                        tone,
                        text,
                    });
                    while self.log.len() > MAX_LOG_ROWS {
                        self.log.pop_front();
                    }
                }
            },
            EngineRunEvent::StreamError { message } => {
                self.live = false;
                self.log.push_back(RunLogRow {
                    time: now,
                    kind: "STREAM",
                    tone: Tone::Err,
                    text: format!("Run outcome unconfirmed: {message}"),
                });
                while self.log.len() > MAX_LOG_ROWS {
                    self.log.pop_front();
                }
            },
            EngineRunEvent::Terminal { outcome } => {
                // Suspension and quota parking close an observation stream,
                // but keep the durable run and its pending operator decision.
                // Only final outcomes retire the queue.
                if matches!(
                    outcome,
                    surge_orchestrator::engine::handle::RunOutcome::Completed { .. }
                        | surge_orchestrator::engine::handle::RunOutcome::Failed { .. }
                        | surge_orchestrator::engine::handle::RunOutcome::Aborted { .. }
                ) {
                    self.pending.clear();
                }
            },
            _ => {},
        }
    }

    fn fold_stages(&mut self, payload: &EventPayload) {
        match payload {
            EventPayload::StageEntered { node, attempt } => {
                let node = node.as_str().to_string();
                if let Some(row) = self.stages.iter_mut().find(|s| s.node == node) {
                    row.attempt = *attempt;
                    row.phase = StagePhase::Running;
                    row.detail = if *attempt > 1 {
                        format!("attempt {attempt}")
                    } else {
                        "running".to_string()
                    };
                } else {
                    self.stages.push(StageRow {
                        node,
                        attempt: *attempt,
                        phase: StagePhase::Running,
                        detail: "running".to_string(),
                    });
                }
            },
            EventPayload::StageCompleted { node, outcome } => {
                self.set_stage(
                    node.as_str(),
                    StagePhase::Done,
                    outcome.as_str().to_string(),
                );
            },
            EventPayload::StageFailed { node, reason, .. } => {
                self.set_stage(node.as_str(), StagePhase::Failed, reason.clone());
            },
            EventPayload::OutcomeReported { node, outcome, .. } => {
                if let Some(row) = self
                    .stages
                    .iter_mut()
                    .find(|s| s.node == node.as_str() && s.phase == StagePhase::Running)
                {
                    row.detail = outcome.as_str().to_string();
                }
            },
            EventPayload::RunCompleted { terminal_node } => {
                self.set_stage(terminal_node.as_str(), StagePhase::Done, "completed".into());
            },
            EventPayload::RunFailed { .. } | EventPayload::RunAborted { .. } => {
                let detail = if matches!(payload, EventPayload::RunFailed { .. }) {
                    "run failed"
                } else {
                    "run aborted"
                };
                for stage in &mut self.stages {
                    if stage.phase == StagePhase::Running {
                        stage.phase = StagePhase::Failed;
                        stage.detail = detail.into();
                    }
                }
            },
            EventPayload::LoopIterationStarted { .. } => {
                // The strip shows the CURRENT iteration's pipeline. Loop
                // bodies re-enter the same node keys, so carrying the
                // previous iteration's Done rows forward would render a
                // misleading mix of old and new stage states.
                self.stages.clear();
            },
            _ => {},
        }
    }

    fn set_stage(&mut self, node: &str, phase: StagePhase, detail: String) {
        if let Some(row) = self.stages.iter_mut().find(|s| s.node == node) {
            row.phase = phase;
            row.detail = detail;
        } else {
            self.stages.push(StageRow {
                node: node.to_string(),
                attempt: 1,
                phase,
                detail,
            });
        }
    }

    fn fold_pending_into(
        list: &mut Vec<PendingDecision>,
        seq: u64,
        now: &str,
        payload: &EventPayload,
    ) {
        let push = |list: &mut Vec<PendingDecision>, node: String, kind: DecisionKind| {
            if list.iter().any(|pending| pending.seq == seq) {
                return;
            }
            list.push(PendingDecision {
                seq,
                time: now.to_string(),
                node,
                kind,
            });
        };

        match payload {
            EventPayload::HumanInputRequested {
                node,
                call_id,
                prompt,
                schema,
                ..
            } => push(
                list,
                node.as_str().to_string(),
                DecisionKind::HumanInput {
                    call_id: call_id.clone(),
                    prompt: prompt.clone(),
                    schema: schema.clone(),
                },
            ),
            EventPayload::HumanInputResolved { call_id, node, .. }
            | EventPayload::HumanInputTimedOut { call_id, node, .. } => {
                list.retain(|p| match &p.kind {
                    DecisionKind::HumanInput { call_id: c, .. } => {
                        // A resolution with a call_id retires exactly that
                        // request; one without retires the (single) gate
                        // pause on that node. Mixed Some/None never match —
                        // a specific resolution must not sweep away a
                        // different, still-pending request on the same node.
                        let resolved = match (c, call_id) {
                            (Some(a), Some(b)) => a == b,
                            (None, None) => p.node == node.as_str(),
                            _ => false,
                        };
                        !resolved
                    },
                    _ => true,
                });
            },
            // ApprovalRequested/Decided and BootstrapApprovalRequested/
            // Decided are bookkeeping companions of the same gate's
            // HumanInputRequested (human_gate.rs emits both for one
            // pause) — folding them too would double-count a single
            // decision, and the bootstrap timeout path never emits a
            // Decided to clear it. Log-only.
            EventPayload::SandboxElevationRequested { node, capability } => push(
                list,
                node.as_str().to_string(),
                DecisionKind::Elevation {
                    capability: capability.clone(),
                },
            ),
            EventPayload::SandboxElevationDecided { node, .. }
            | EventPayload::SandboxElevationTimedOut { node, .. } => {
                let n = node.as_str();
                list.retain(|p| !(matches!(p.kind, DecisionKind::Elevation { .. }) && p.node == n));
            },
            EventPayload::RoadmapPatchApprovalRequested { patch_id, .. } => push(
                list,
                patch_id.to_string(),
                DecisionKind::RoadmapPatch {
                    patch_id: patch_id.to_string(),
                },
            ),
            EventPayload::RoadmapPatchApprovalDecided { patch_id, .. } => {
                let id = patch_id.to_string();
                list.retain(
                    |p| !matches!(&p.kind, DecisionKind::RoadmapPatch { patch_id } if *patch_id == id),
                );
            },
            EventPayload::EscalationRequested { reason, .. } => push(
                list,
                "engine".to_string(),
                DecisionKind::Escalation {
                    reason: reason.clone(),
                },
            ),
            _ => {},
        }
    }

    fn fold_cost(&mut self, payload: &EventPayload) {
        if let EventPayload::TokensConsumed {
            prompt_tokens,
            output_tokens,
            cost_usd,
            ..
        } = payload
        {
            self.tokens_in += u64::from(*prompt_tokens);
            self.tokens_out += u64::from(*output_tokens);
            self.cost_usd += cost_usd.unwrap_or(0.0);
        }
    }
}

/// One-line log description of a payload. `None` = don't log (noise).
#[allow(clippy::too_many_lines)]
fn describe(payload: &EventPayload) -> Option<(&'static str, Tone, String)> {
    use EventPayload as P;
    Some(match payload {
        P::RunStarted { initial_prompt, .. } => {
            let head: String = initial_prompt.chars().take(90).collect();
            ("START", Tone::Info, head)
        },
        P::RunCompleted { terminal_node } => (
            "END",
            Tone::Ok,
            format!("run completed · {}", terminal_node.as_str()),
        ),
        P::RunFailed { error } => ("END", Tone::Err, format!("run failed · {error}")),
        P::RunAborted { reason } => ("END", Tone::Warn, format!("aborted · {reason}")),
        P::StageEntered { node, attempt } => (
            "STAGE",
            Tone::Accent,
            if *attempt > 1 {
                format!("→ {} (attempt {attempt})", node.as_str())
            } else {
                format!("→ {}", node.as_str())
            },
        ),
        P::StageCompleted { node, outcome } => (
            "STAGE",
            Tone::Ok,
            format!("{} · {}", node.as_str(), outcome.as_str()),
        ),
        P::StageFailed { node, reason, .. } => {
            ("FAIL", Tone::Err, format!("{} · {reason}", node.as_str()))
        },
        P::SessionOpened { node, agent, .. } => (
            "AGENT",
            Tone::Info,
            format!("{agent} session @ {}", node.as_str()),
        ),
        P::ToolCalled {
            tool, mcp_server, ..
        } => (
            "TOOL",
            Tone::Info,
            match mcp_server {
                Some(s) => format!("{tool} · via {s}"),
                None => tool.clone(),
            },
        ),
        P::ArtifactProduced { name, path, .. } => {
            ("ARTIFACT", Tone::Ok, format!("{name} → {}", path.display()))
        },
        P::OutcomeReported { node, outcome, .. } => (
            "OUTCOME",
            Tone::Accent,
            format!("{} · {}", node.as_str(), outcome.as_str()),
        ),
        P::EdgeTraversed { from, to, .. } => (
            "EDGE",
            Tone::Info,
            format!("{} → {}", from.as_str(), to.as_str()),
        ),
        P::TaskStatusChanged {
            task_id, from, to, ..
        } => (
            "TASK",
            Tone::Accent,
            format!("{task_id} · {from:?} → {to:?}"),
        ),
        P::TaskDiscovered { task_id, title, .. } => (
            "TASK",
            Tone::Warn,
            format!("discovered {task_id} · {title}"),
        ),
        P::TaskVerified { task_id, node, .. } => (
            "VERIFY",
            Tone::Ok,
            format!("{task_id} verified by {}", node.as_str()),
        ),
        P::SteerDelivered { message, .. } => {
            let head: String = message.chars().take(90).collect();
            ("STEER", Tone::Accent, head)
        },
        P::TokensConsumed {
            output_tokens,
            cost_usd,
            model,
            ..
        } => (
            "COST",
            Tone::Info,
            format!(
                "+{output_tokens} tok · ${:.2} · {model}",
                cost_usd.unwrap_or(0.0)
            ),
        ),
        P::BudgetWarningRaised { pct, cost_usd, .. } => (
            "BUDGET",
            Tone::Warn,
            format!("{pct:.0}% of budget · ${cost_usd:.2}"),
        ),
        P::BudgetExceeded { cost_usd, .. } => (
            "BUDGET",
            Tone::Err,
            format!("budget exceeded · ${cost_usd:.2}"),
        ),
        P::HumanInputRequested { prompt, .. } => {
            let head: String = prompt.chars().take(90).collect();
            ("GATE", Tone::Warn, head)
        },
        P::HumanInputResolved { .. } => ("GATE", Tone::Ok, "human input resolved".to_string()),
        P::HumanInputTimedOut { .. } => ("GATE", Tone::Err, "human input timed out".to_string()),
        P::ApprovalRequested { gate, .. } => (
            "GATE",
            Tone::Warn,
            format!("approval needed @ {}", gate.as_str()),
        ),
        P::ApprovalDecided { gate, decision, .. } => {
            ("GATE", Tone::Ok, format!("{} · {decision}", gate.as_str()))
        },
        P::BootstrapApprovalRequested { stage, .. } => (
            "GATE",
            Tone::Warn,
            format!("bootstrap {stage:?} awaiting approval").to_lowercase(),
        ),
        P::SandboxElevationRequested { node, capability } => (
            "ELEVATE",
            Tone::Warn,
            format!("{} requests {capability}", node.as_str()),
        ),
        P::EscalationRequested { reason, .. } => ("ESCALATE", Tone::Err, reason.clone()),
        P::PipelineMaterialized { .. } => {
            ("GRAPH", Tone::Info, "pipeline materialized".to_string())
        },
        P::LoopIterationStarted { item, index, .. } => {
            ("LOOP", Tone::Info, format!("iteration {index} · {item}"))
        },
        P::NotifyDelivered {
            channel_kind,
            success,
            ..
        } => (
            "NOTIFY",
            if *success { Tone::Ok } else { Tone::Err },
            format!("{channel_kind:?}").to_lowercase(),
        ),
        P::GateStageOutcomeCommitted { .. } | P::GateStageRouteCommitted { .. } => return None,
        // Everything else (inputs-resolved bookkeeping, hooks, forks,
        // session close, …) stays out of the visible log.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use surge_core::NodeKey;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    use super::*;

    #[test]
    fn display_replays_capacity_keeps_parked_terminal_and_invalidates_gaps() {
        use surge_core::run_display::{RunDisplayState, WaitingReason};
        let id = surge_core::RunId::new();
        let mut stream = RunStreamState::default();
        stream.begin_display_history(id, None);
        assert_eq!(stream.display(), RunDisplayState::Unknown);
        let start = EventPayload::RunStarted {
            pipeline_template: None,
            project_path: "/proj".into(),
            initial_prompt: "work".into(),
            config: surge_core::RunConfig {
                budget: Default::default(),
                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
                bootstrap_edit_loop_cap: None,
            },
        };
        stream.apply_recorded(&persisted(1, start.clone()), 0);
        let key = node("end");
        let graph = surge_core::Graph {
            schema_version: 1,
            metadata: surge_core::graph::GraphMetadata {
                name: "display".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: key.clone(),
            nodes: [(
                key.clone(),
                surge_core::Node {
                    id: key,
                    position: Default::default(),
                    declared_outcomes: Vec::new(),
                    config: surge_core::NodeConfig::Terminal(
                        surge_core::terminal_config::TerminalConfig {
                            kind: surge_core::terminal_config::TerminalKind::Success,
                            message: None,
                        },
                    ),
                },
            )]
            .into(),
            edges: Vec::new(),
            subgraphs: Default::default(),
        };
        stream.apply_recorded(
            &persisted(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: surge_core::ContentHash::compute(b"display"),
                },
            ),
            10,
        );
        stream.finish_display_history(2);
        let until = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let park = EventPayload::RunParked {
            wake_at: until,
            runtime: Some("runtime-a".into()),
            worktree: "/proj".into(),
            basis: surge_core::capacity::WakeBasis::ObservedReset,
            reason: "capacity".into(),
        };
        stream.apply(&persisted(3, park));
        assert_eq!(stream.display().label(), "Waiting for capacity");
        assert_eq!(
            stream.display().wake(),
            Some((until, surge_core::capacity::WakeBasis::ObservedReset))
        );
        stream.apply(&EngineRunEvent::Terminal {
            outcome: surge_orchestrator::engine::handle::RunOutcome::Parked { wake_at: until },
        });
        assert!(matches!(
            stream.display(),
            RunDisplayState::Waiting(WaitingReason::Capacity { .. })
        ));
        stream.apply(&EngineRunEvent::StreamError {
            message: "disconnected".into(),
        });
        assert_eq!(
            stream.display().label(),
            "Waiting for capacity",
            "disconnect is not crash evidence"
        );
        stream.apply(&persisted(5, EventPayload::RunWokeFromPark {}));
        assert_eq!(stream.display(), RunDisplayState::Unknown);
        stream.apply(&persisted(
            6,
            EventPayload::RunAborted {
                reason: "suffix cannot repair gap".into(),
            },
        ));
        assert_eq!(stream.display(), RunDisplayState::Unknown);
        stream.begin_display_history(id, Some(surge_core::RunStatus::Crashed));
        stream.invalidate_display();
        assert_eq!(stream.display().label(), "Recovery requires an operator");
        let mut untrusted = RunStreamState::default();
        untrusted.begin_display_history(id, None);
        untrusted.apply(&persisted(
            1,
            EventPayload::StageEntered {
                node: node("end"),
                attempt: 1,
            },
        ));
        untrusted.apply(&persisted(2, start));
        assert_eq!(
            untrusted.display(),
            RunDisplayState::Unknown,
            "later start cannot repair untrusted origin"
        );
        untrusted.apply(&persisted(
            3,
            EventPayload::RunAborted {
                reason: "untrusted suffix".into(),
            },
        ));
        assert_eq!(untrusted.display(), RunDisplayState::Unknown);
    }

    fn persisted(seq: u64, payload: EventPayload) -> EngineRunEvent {
        EngineRunEvent::Persisted {
            seq,
            payload: Box::new(payload),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_metadata_reads_whole_stored_history_beyond_visible_log() {
        use surge_core::execution_recovery::{
            OpenedSession, ProviderSessionDescriptor, ProviderSessionId, SessionOpenMode,
            SessionRestoreCapabilities,
        };
        use surge_core::run_event::{SessionDisposition, VersionedEventPayload};
        let home = tempfile::tempdir().unwrap();
        let storage = surge_persistence::runs::Storage::open(home.path())
            .await
            .unwrap();
        let run = surge_core::RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let mut payloads = vec![EventPayload::RunStarted {
            pipeline_template: None,
            project_path: home.path().into(),
            initial_prompt: "Inspect recorded sessions".into(),
            config: surge_core::RunConfig {
                budget: Default::default(),
                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
                bootstrap_edit_loop_cap: None,
            },
        }];
        let invocation = surge_core::id::StageInvocationId::new();
        let first_session = surge_core::SessionId::new();
        for index in 0..251 {
            let session = if index == 0 {
                first_session
            } else {
                surge_core::SessionId::new()
            };
            let descriptor = ProviderSessionDescriptor::new(
                ProviderSessionId::new(if index < 3 {
                    "retained-provider".into()
                } else {
                    format!("provider-{index}")
                })
                .unwrap(),
                invocation,
                "runtime-a".into(),
                surge_core::ContentHash::compute(b"launch"),
                home.path().into(),
                SessionRestoreCapabilities {
                    resume: true,
                    load: true,
                },
            )
            .unwrap();
            let mode = match index {
                1 => SessionOpenMode::Resume,
                2 => SessionOpenMode::Load,
                _ => SessionOpenMode::New,
            };
            payloads.push(EventPayload::SessionOpened {
                node: node("agent"),
                session,
                agent: "implementer@1.0".into(),
                agent_id: Some("runtime-a".into()),
                opened: (index != 3)
                    .then(|| OpenedSession::new(session, descriptor, mode).unwrap()),
                handoff: (index < 3).then(surge_core::id::WorkItemOperationId::new),
            });
            if index == 0 {
                payloads.push(EventPayload::SessionClosed {
                    session,
                    disposition: SessionDisposition::Normal,
                });
            }
        }
        writer
            .append_events(
                payloads
                    .into_iter()
                    .map(VersionedEventPayload::new)
                    .collect(),
            )
            .await
            .unwrap();
        let events = surge_persistence::runs::Storage::inspect_existing_run_events(
            home.path().join("runs"),
            run,
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 253);
        let mut stream = RunStreamState::default();
        for _ in 0..2 {
            stream.begin_display_history(run, None);
            for event in &events {
                stream.apply_recorded(
                    &persisted(event.seq.as_u64(), event.payload.payload.clone()),
                    event.timestamp_ms,
                );
            }
            stream.finish_display_history(events.last().map_or(0, |event| event.seq.as_u64()));
            assert!(stream.session_history_confirmed());
            assert_eq!(
                stream.sessions.len(),
                251,
                "rehydration must retain all openings without duplicate rows"
            );
            assert!(stream.log.len() <= MAX_LOG_ROWS);
            assert_eq!(stream.sessions[0].seq, 2);
            assert_eq!(
                stream.sessions[0].closed,
                Some((3, SessionDisposition::Normal))
            );
            assert_eq!(stream.sessions[0].session, first_session);
            assert_ne!(stream.sessions[0].session, stream.sessions[1].session);
            assert_eq!(
                stream.sessions[1].opened.as_ref().unwrap().mode,
                SessionOpenMode::Resume
            );
            assert_eq!(
                stream.sessions[2].opened.as_ref().unwrap().mode,
                SessionOpenMode::Load
            );
            for record in &stream.sessions[..3] {
                assert_eq!(
                    record
                        .opened
                        .as_ref()
                        .unwrap()
                        .descriptor
                        .provider_session_id()
                        .as_str(),
                    "retained-provider"
                );
            }
            assert_ne!(stream.sessions[0].handoff, stream.sessions[1].handoff);
            assert!(
                stream.sessions[3].opened.is_none(),
                "legacy unknown provider metadata is retained honestly"
            );
        }
        stream.apply(&persisted(
            255,
            EventPayload::SessionClosed {
                session: first_session,
                disposition: SessionDisposition::ForcedClose,
            },
        ));
        assert!(!stream.session_history_confirmed());
        assert_eq!(
            stream.sessions.len(),
            251,
            "a gap retains recorded history without claiming completeness"
        );
    }

    fn node(name: &str) -> NodeKey {
        NodeKey::try_from(name).unwrap()
    }

    fn human_input(node_name: &str, call_id: Option<&str>) -> EventPayload {
        EventPayload::HumanInputRequested {
            node: node(node_name),
            session: None,
            call_id: call_id.map(str::to_string),
            prompt: "answer me".into(),
            schema: None,
        }
    }

    fn stream_with_started_run() -> RunStreamState {
        let mut stream = RunStreamState::default();
        stream.begin_display_history(surge_core::RunId::new(), None);
        stream.apply(&persisted(
            1,
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: "/project".into(),
                initial_prompt: "test pending decisions".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
        ));
        stream.finish_display_history(1);
        stream
    }

    #[test]
    fn stream_failure_preserves_last_durable_decision_without_terminal_claim() {
        let mut stream = RunStreamState {
            live: true,
            ..RunStreamState::default()
        };
        stream.apply(&persisted(17, human_input("review", Some("call-1"))));
        stream.apply(&EngineRunEvent::StreamError {
            message: "catch-up storage unavailable".into(),
        });
        assert!(!stream.live);
        assert_eq!(stream.last_seq, 17);
        assert_eq!(stream.pending.len(), 1);
        assert_eq!(stream.pending[0].node, "review");
        let error = stream.log.back().unwrap();
        assert_eq!(error.kind, "STREAM");
        assert!(error.text.contains("catch-up storage unavailable"));
        assert!(error.text.contains("unconfirmed"));
    }

    #[test]
    fn recorded_terminal_uses_durable_time_and_clears_pending() {
        let mut state = RunStreamState::default();
        state.apply(&persisted(1, human_input("gate", None)));
        assert!(!state.pending.is_empty());
        state.apply_recorded(
            &persisted(
                2,
                EventPayload::RunAborted {
                    reason: "done".into(),
                },
            ),
            0,
        );
        assert!(state.pending.is_empty());
        let expected = chrono::DateTime::from_timestamp_millis(0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string();
        assert_eq!(state.log.back().unwrap().time, expected);
    }

    #[test]
    fn hydration_overlap_does_not_duplicate_or_rewind_events() {
        let mut state = RunStreamState::default();
        let entered = persisted(
            1,
            EventPayload::StageEntered {
                node: node("implement"),
                attempt: 1,
            },
        );
        state.apply(&entered);
        state.apply(&entered);
        assert_eq!(state.log.len(), 1);
        state.apply(&persisted(
            2,
            EventPayload::StageFailed {
                node: node("implement"),
                reason: "failed".into(),
                retry_available: false,
            },
        ));
        state.apply(&entered);
        assert_eq!(state.last_seq, 2);
        assert_eq!(state.log.len(), 2);
        assert_eq!(state.stages[0].phase, StagePhase::Failed);
    }

    #[test]
    fn terminal_events_settle_running_stage_chips() {
        for (payload, phase, detail) in [
            (
                EventPayload::RunCompleted {
                    terminal_node: node("end"),
                },
                StagePhase::Done,
                "completed",
            ),
            (
                EventPayload::RunFailed {
                    error: "error".into(),
                },
                StagePhase::Failed,
                "run failed",
            ),
            (
                EventPayload::RunAborted {
                    reason: "stop".into(),
                },
                StagePhase::Failed,
                "run aborted",
            ),
        ] {
            let mut state = RunStreamState::default();
            state.apply(&persisted(
                1,
                EventPayload::StageEntered {
                    node: node("end"),
                    attempt: 1,
                },
            ));
            state.apply(&persisted(2, payload));
            assert_eq!(state.stages[0].phase, phase);
            assert_eq!(state.stages[0].detail, detail);
        }
    }

    #[test]
    fn stage_fold_tracks_enter_complete_fail() {
        let mut s = RunStreamState::default();
        s.apply(&persisted(
            1,
            EventPayload::StageEntered {
                node: node("implement"),
                attempt: 1,
            },
        ));
        assert_eq!(s.stages.len(), 1);
        assert_eq!(s.stages[0].phase, StagePhase::Running);

        s.apply(&persisted(
            2,
            EventPayload::StageFailed {
                node: node("implement"),
                reason: "tests red".into(),
                retry_available: true,
            },
        ));
        assert_eq!(s.stages.len(), 1, "same node must update, not duplicate");
        assert_eq!(s.stages[0].phase, StagePhase::Failed);
        assert_eq!(s.stages[0].detail, "tests red");

        // Retry re-enters the same stage.
        s.apply(&persisted(
            3,
            EventPayload::StageEntered {
                node: node("implement"),
                attempt: 2,
            },
        ));
        assert_eq!(s.stages[0].phase, StagePhase::Running);
        assert_eq!(s.stages[0].attempt, 2);
    }

    #[test]
    fn resolved_with_call_id_removes_only_that_request() {
        let mut s = stream_with_started_run();
        s.apply(&persisted(2, human_input("gate_a", Some("call-1"))));
        s.apply(&persisted(3, human_input("gate_b", Some("call-2"))));
        assert_eq!(s.pending.len(), 2);

        s.apply(&persisted(
            4,
            EventPayload::HumanInputResolved {
                node: node("gate_a"),
                call_id: Some("call-1".into()),
                response: serde_json::Value::Null,
            },
        ));
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0].node, "gate_b");
    }

    #[test]
    fn resolved_without_call_id_matches_by_node_only() {
        // Regression: `None == None` on call_id must NOT remove pending
        // inputs that belong to a different node.
        let mut s = stream_with_started_run();
        s.apply(&persisted(2, human_input("gate_a", None)));
        s.apply(&persisted(3, human_input("gate_b", None)));
        assert_eq!(s.pending.len(), 2);

        s.apply(&persisted(
            4,
            EventPayload::HumanInputResolved {
                node: node("gate_a"),
                call_id: None,
                response: serde_json::Value::Null,
            },
        ));
        assert_eq!(s.pending.len(), 1, "only gate_a's request may be removed");
        assert_eq!(s.pending[0].node, "gate_b");
    }

    #[test]
    fn resolving_specific_call_id_keeps_none_sibling_on_same_node() {
        // Regression: a resolution carrying call_id "c1" must not sweep
        // away a different, still-pending request on the same node that
        // has no call_id.
        let mut s = stream_with_started_run();
        s.apply(&persisted(2, human_input("gate_a", Some("c1"))));
        s.apply(&persisted(3, human_input("gate_a", None)));
        assert_eq!(s.pending.len(), 2);

        s.apply(&persisted(
            4,
            EventPayload::HumanInputResolved {
                node: node("gate_a"),
                call_id: Some("c1".into()),
                response: serde_json::Value::Null,
            },
        ));
        assert_eq!(s.pending.len(), 1, "only the c1 request may be removed");
        assert!(matches!(
            &s.pending[0].kind,
            DecisionKind::HumanInput { call_id: None, .. }
        ));
    }

    #[test]
    fn loop_iteration_clears_previous_stage_rows() {
        // Loop bodies re-enter the same node keys; the strip shows the
        // CURRENT iteration only.
        let mut s = RunStreamState::default();
        s.apply(&persisted(
            1,
            EventPayload::StageEntered {
                node: node("implement"),
                attempt: 1,
            },
        ));
        s.apply(&persisted(
            2,
            EventPayload::StageCompleted {
                node: node("implement"),
                outcome: surge_core::keys::OutcomeKey::try_from("done").unwrap(),
            },
        ));
        assert_eq!(s.stages.len(), 1);

        s.apply(&persisted(
            3,
            EventPayload::LoopIterationStarted {
                loop_id: node("milestones"),
                item: "m2".into(),
                index: 1,
            },
        ));
        assert!(
            s.stages.is_empty(),
            "new iteration must not mix with the previous one's rows"
        );
    }

    #[test]
    fn gate_outcomes_parse_schema_enum() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "outcome": { "type": "string", "enum": ["approve", "edit", "reject"] },
                "comment": { "type": "string" },
            },
            "required": ["outcome"],
        });
        let kind = DecisionKind::HumanInput {
            call_id: None,
            prompt: "gate".into(),
            schema: Some(schema),
        };
        assert_eq!(kind.gate_outcomes(), vec!["approve", "edit", "reject"]);
    }

    #[test]
    fn tokens_accumulate_and_terminal_clears_pending() {
        let mut s = RunStreamState::default();
        s.apply(&persisted(
            1,
            EventPayload::TokensConsumed {
                session: surge_core::SessionId::new(),
                prompt_tokens: 1000,
                output_tokens: 200,
                cache_hits: 0,
                model: "m".into(),
                cost_usd: Some(0.25),
            },
        ));
        s.apply(&persisted(
            2,
            EventPayload::TokensConsumed {
                session: surge_core::SessionId::new(),
                prompt_tokens: 500,
                output_tokens: 100,
                cache_hits: 0,
                model: "m".into(),
                cost_usd: Some(0.05),
            },
        ));
        assert_eq!(s.tokens_in, 1500);
        assert_eq!(s.tokens_out, 300);
        assert!((s.cost_usd - 0.30).abs() < 1e-9);

        s.apply(&persisted(3, human_input("gate_a", None)));
        assert_eq!(s.pending.len(), 1);
        s.apply(&EngineRunEvent::Terminal {
            outcome: surge_orchestrator::engine::handle::RunOutcome::Aborted {
                reason: "operator".into(),
            },
        });
        assert!(s.pending.is_empty(), "terminal run has nothing to decide");
    }
}
