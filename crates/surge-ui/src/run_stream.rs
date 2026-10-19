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

pub use surge_orchestrator::operator::DecisionKind;

/// A decision the run waits on, as the operator fold
/// ([`surge_orchestrator::operator::PendingDecisions`]) reports it, with
/// what the queue displays. The fold owns every matching rule; this view only
/// renders it.
#[derive(Clone)]
pub struct PendingDecision {
    /// Sequence of the event that opened the decision (its identity).
    pub seq: u64,
    /// Local request time.
    pub time: String,
    /// The node it concerns; the patch id for a roadmap patch, `engine` for
    /// an unattributed escalation.
    pub node: String,
    /// The folded decision.
    pub decision: surge_orchestrator::operator::PendingDecision,
}

impl PendingDecision {
    /// Display view of one folded decision.
    pub fn new(decision: surge_orchestrator::operator::PendingDecision) -> Self {
        let node = match (&decision.node, &decision.kind) {
            (Some(node), _) => node.to_string(),
            (None, DecisionKind::RoadmapPatch { patch_id, .. }) => patch_id.clone(),
            (None, _) => "engine".to_owned(),
        };
        Self {
            seq: decision.id.request_seq,
            time: decision
                .requested_at
                .with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string(),
            node,
            decision,
        }
    }

    /// What the decision asks.
    pub fn kind(&self) -> &DecisionKind {
        &self.decision.kind
    }

    /// The question or reason shown to the operator.
    pub fn prompt(&self) -> &str {
        &self.decision.prompt
    }

    /// The call id an answer is delivered under, for human input.
    pub fn call_id(&self) -> Option<String> {
        self.decision.kind.call_id().map(str::to_owned)
    }

    /// The outcome keys the decision accepts (empty: free-form or answered
    /// elsewhere).
    pub fn outcomes(&self) -> Vec<String> {
        self.decision
            .options
            .iter()
            .map(|option| option.outcome.clone())
            .collect()
    }

    /// Urgency rank for the Inbox (lower = more urgent).
    pub fn rank(&self) -> u8 {
        self.decision.kind.rank()
    }

    /// The bootstrap stage under review, for a bootstrap approval.
    pub fn bootstrap_stage(&self) -> Option<surge_core::run_event::BootstrapStage> {
        match &self.decision.kind {
            DecisionKind::BootstrapApproval { stage, .. } => Some(*stage),
            _ => None,
        }
    }
}

#[cfg(test)]
impl PendingDecision {
    /// A human-input request classified exactly as the operator fold does.
    pub(crate) fn requested(
        seq: u64,
        node: &str,
        call_id: Option<String>,
        prompt: &str,
        schema: Option<serde_json::Value>,
    ) -> Self {
        let payload = EventPayload::HumanInputRequested {
            node: surge_core::keys::NodeKey::try_from(node).unwrap(),
            session: None,
            call_id,
            prompt: prompt.into(),
            schema,
        };
        let opening = surge_orchestrator::operator::classify_opening(&payload, None).unwrap();
        Self::new(surge_orchestrator::operator::PendingDecision {
            id: surge_orchestrator::operator::DecisionId {
                run: surge_core::RunId::new(),
                request_seq: seq,
            },
            requested_at: chrono::Utc::now(),
            node: opening.node,
            kind: opening.kind,
            prompt: opening.prompt,
            options: opening.options,
            deadline: None,
            actionable: true,
        })
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
    recovery_observer: surge_core::recovery_observation::RecoveryObserver,
    display_seq: u64,
    prefix_evidence: VecDeque<(u64, surge_core::ContentHash)>,
    recorded_prefix_evidence: VecDeque<(u64, surge_core::ContentHash)>,
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
    /// Last observation failure, independent of recorded outcome history.
    pub observation_error: Option<String>,
    observation_revision: u64,
    observation_error_from_read: bool,
    pub log: VecDeque<RunLogRow>,
    pub stages: Vec<StageRow>,
    /// Open decisions, published from `decisions` only while the stream's
    /// history is trusted.
    pub pending: Vec<PendingDecision>,
    /// The operator fold of this run's decisions, fed as events arrive.
    decisions: Option<surge_orchestrator::operator::PendingDecisions>,
    pending_replay: bool,
    pending_watermark: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub cost_unknown: bool,
    pub usage_unknown: bool,
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
    pub(crate) fn registry_hint(
        run: surge_core::RunId,
        status: surge_core::RunStatus,
        path: std::path::PathBuf,
    ) -> Self {
        Self {
            display_run_id: Some(run),
            registry_status: Some(status),
            git_common_dir: crate::project::git_common_dir(&path),
            run_path: Some(path),
            ..Self::default()
        }
    }

    /// Start replaying a trusted full history for this actual run identity.
    pub fn begin_display_history(
        &mut self,
        run_id: surge_core::RunId,
        status: Option<surge_core::RunStatus>,
    ) {
        self.display_run_id = Some(run_id);
        self.display_state = Some(surge_core::RunState::NotStarted);
        self.recovery_observer = Default::default();
        self.display_seq = 0;
        self.prefix_evidence.clear();
        self.recorded_prefix_evidence.clear();
        self.sessions.clear();
        self.session_history_valid = true;
        self.trusted_work_item = None;
        self.ownership_startup_open = true;
        self.ownership_prompt = None;
        self.ownership_binding_seen = false;
        self.ownership_invalid = false;
        self.registry_status = status.or(self.registry_status);
        // Rebuild decisions from the full trusted journal below. Do not
        // expose them until the caller confirms this replay is complete.
        self.pending.clear();
        self.decisions = Some(surge_orchestrator::operator::PendingDecisions::new(run_id));
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
            && self.decisions.as_ref().is_some_and(|decisions| {
                decisions.last_seq() == through_seq
                    && *decisions.status() == surge_orchestrator::operator::FoldStatus::Complete
            })
        {
            self.publish_decisions();
            self.pending_watermark = through_seq;
        } else {
            self.invalidate_display();
        }
        self.pending_replay = false;
    }

    pub(crate) fn replay_candidate(
        run: surge_core::RunId,
        events: &[surge_persistence::runs::ReadEvent],
    ) -> Self {
        let mut candidate = Self::default();
        candidate.begin_display_history(run, None);
        for event in events {
            candidate.apply_recorded(
                &EngineRunEvent::Persisted {
                    seq: event.seq.as_u64(),
                    payload: Box::new(event.payload.payload.clone()),
                },
                event.timestamp_ms,
            );
        }
        candidate.finish_display_history(events.last().map_or(0, |event| event.seq.as_u64()));
        candidate
    }

    /// Reject a stale prefix before mutating any published fold or decisions.
    pub(crate) fn publish_candidate(&mut self, mut candidate: Self) -> Result<(), &'static str> {
        if candidate.display_seq < self.display_seq.max(self.pending_watermark) {
            return Err("older prefix");
        }
        if let Some((seq, digest)) = self.prefix_evidence.back()
            && candidate
                .prefix_evidence
                .iter()
                .find(|(candidate_seq, _)| candidate_seq == seq)
                .map(|(_, digest)| digest)
                != Some(digest)
        {
            return Err("conflicting retained prefix");
        }
        // The wire feed has no durable timestamp. Separately retain the exact
        // timestamped prefix already authenticated by disk, alongside live payloads.
        if let Some((seq, digest)) = self.recorded_prefix_evidence.back()
            && candidate
                .recorded_prefix_evidence
                .iter()
                .find(|(candidate_seq, _)| candidate_seq == seq)
                .map(|(_, digest)| digest)
                != Some(digest)
        {
            return Err("conflicting recorded prefix");
        }
        if !candidate.session_history_confirmed() || !candidate.recovery().is_complete() {
            return Err("unconfirmed history");
        }
        candidate.registry_status = self.registry_status;
        candidate.live = self.live;
        candidate.observation_error = self.observation_error.clone();
        candidate.observation_revision = self.observation_revision;
        candidate.observation_error_from_read = self.observation_error_from_read;
        candidate.pending_watermark = candidate.pending_watermark.max(self.pending_watermark);
        *self = candidate;
        Ok(())
    }

    /// Called only after a replacement publisher's SubscribeOk, before its first read.
    pub(crate) fn attach_live_owner(&mut self) {
        self.live = true;
        if self.observation_error.is_some() {
            self.history_unconfirmed("New live connection attached; restoring run history");
        }
    }

    pub(crate) fn current_registry_hint(&self) -> Option<surge_core::RunStatus> {
        self.registry_status
    }

    pub(crate) fn observation_revision(&self) -> u64 {
        self.observation_revision
    }
    pub(crate) fn clear_observation_error_if_unchanged(&mut self, revision: u64) {
        if revision != u64::MAX
            && self.observation_revision == revision
            && self.observation_error.is_some()
            && self.observation_error_from_read
        {
            self.observation_error = None;
            self.observation_error_from_read = false;
            self.observation_revision = self.observation_revision.saturating_add(1);
        }
    }

    /// A daemon-confirmed start supersedes old registry crash evidence.
    pub fn mark_started(&mut self) {
        self.registry_status = Some(surge_core::RunStatus::Running);
    }

    /// Missing/gapped history is not evidence of either work or daemon death.
    pub fn invalidate_display(&mut self) {
        self.recovery_observer.invalidate();
        self.display_state = None;
        self.trusted_work_item = None;
        self.session_history_valid = false;
        self.decisions = None;
        self.pending.clear();
    }

    /// Publish the fold's open set, or nothing when the fold is untrusted.
    fn publish_decisions(&mut self) {
        self.pending = self
            .decisions
            .as_ref()
            .filter(|decisions| {
                *decisions.status() == surge_orchestrator::operator::FoldStatus::Complete
            })
            .map(|decisions| {
                decisions
                    .open()
                    .cloned()
                    .map(PendingDecision::new)
                    .collect()
            })
            .unwrap_or_default();
    }

    /// Feed one durable event to the operator fold.
    fn feed_decisions(
        &mut self,
        seq: u64,
        at: chrono::DateTime<chrono::Utc>,
        payload: &EventPayload,
    ) {
        let Some(decisions) = self.decisions.as_mut() else {
            return;
        };
        let _ = decisions.feed(&surge_core::RunEvent {
            run_id: decisions.run(),
            seq,
            timestamp: at,
            payload: payload.clone(),
        });
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

    /// Shared recorded recovery history, retained when current observation is unavailable.
    pub fn recovery(&self) -> &surge_core::recovery_observation::RecoveryObservation {
        self.recovery_observer.observation()
    }

    /// Whether the exact run has an attached, trusted current observation.
    pub fn observation_available_for(&self, run: surge_core::RunId) -> bool {
        self.display_run_id == Some(run)
            && self.live
            && self.observation_error.is_none()
            && self.session_history_confirmed()
            && self.recovery().is_complete()
    }

    /// Independent recorded host loss retained while current observation is unavailable.
    pub(crate) fn recorded_host_loss_detail(&self) -> Option<&'static str> {
        (self.registry_status == Some(surge_core::RunStatus::Crashed)
            && self.observation_error.is_some()).then_some(
                "Recorded loss of daemon ownership; current observation is unconfirmed. Run outcome and cleanup remain unconfirmed. Inspect recorded history before recovery."
            )
    }

    /// Canonical display projection; existing pending decisions remain independent.
    pub fn display(&self) -> surge_core::run_display::RunDisplayState {
        if self.observation_error.is_some()
            && !matches!(
                self.display_state,
                Some(surge_core::RunState::Terminal { .. })
            )
        {
            return surge_core::run_display::RunDisplayState::Unknown;
        }
        let state = (self.display_seq > 0)
            .then_some(self.display_state.as_ref())
            .flatten();
        surge_core::run_display::RunDisplayState::from_state_with_observation(
            state,
            self.registry_status,
            self.display_run_id
                .is_some_and(|run| self.observation_available_for(run)),
        )
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
        let previous = self.prefix_evidence.back().map(|(_, hash)| *hash);
        let Ok(bytes) = serde_json::to_vec(&(previous, seq, payload)) else {
            self.invalidate_display();
            return;
        };
        self.prefix_evidence
            .push_back((*seq, surge_core::ContentHash::compute(&bytes)));
        if self.prefix_evidence.len() > 4096 {
            self.prefix_evidence.pop_front();
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
                    | EventPayload::OwnedFlowInputsBound { .. }
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
        self.apply_at(event, chrono::Utc::now());
    }

    /// Fold durable history using its recorded time rather than replay time.
    pub fn apply_recorded(&mut self, event: &EngineRunEvent, timestamp_ms: i64) {
        if let EngineRunEvent::Persisted { seq, payload } = event {
            let previous = self.recorded_prefix_evidence.back().copied();
            if *seq == previous.map_or(1, |(seq, _)| seq.saturating_add(1)) {
                let Ok(bytes) = serde_json::to_vec(&(
                    previous.map(|(_, hash)| hash),
                    seq,
                    timestamp_ms,
                    payload,
                )) else {
                    self.invalidate_display();
                    return;
                };
                self.recorded_prefix_evidence
                    .push_back((*seq, surge_core::ContentHash::compute(&bytes)));
                if self.recorded_prefix_evidence.len() > 4096 {
                    self.recorded_prefix_evidence.pop_front();
                }
            }
        }
        match chrono::DateTime::from_timestamp_millis(timestamp_ms) {
            Some(at) => self.apply_at(event, at),
            None => {
                // An unreadable durable time cannot date a decision.
                self.invalidate_display();
                self.apply_at(event, chrono::Utc::now());
            },
        }
    }

    fn apply_at(&mut self, event: &EngineRunEvent, at: chrono::DateTime<chrono::Utc>) {
        let now = at
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string();
        if let (Some(run_id), EngineRunEvent::Persisted { seq, payload }) =
            (self.display_run_id, event)
            && *seq > self.display_seq
        {
            self.recovery_observer.observe(&surge_core::RunEvent {
                run_id,
                seq: *seq,
                timestamp: at,
                payload: payload.as_ref().clone(),
            });
        }
        self.observe_display(event);
        if self.pending_replay
            && let EngineRunEvent::Persisted { seq, payload } = event
            && self.session_history_valid
            && self.display_seq == *seq
        {
            self.feed_decisions(*seq, at, payload);
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
                    // Decisions are shown only from a fold over a trusted,
                    // contiguous history; anything else shows none rather
                    // than stale buttons.
                    if self.session_history_valid && self.display_seq == *seq {
                        self.feed_decisions(*seq, at, payload);
                        self.publish_decisions();
                    } else {
                        self.pending.clear();
                    }
                    self.pending_watermark = self.pending_watermark.max(*seq);
                }
                self.fold_cost(payload);
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
                self.record_observation_error(message, now);
            },
            EngineRunEvent::Terminal {
                outcome:
                    surge_orchestrator::engine::handle::RunOutcome::SettlementFailed {
                        failures, ..
                    },
            } => {
                self.apply_at(
                    &EngineRunEvent::StreamError {
                        message: format!("resource settlement failed: {failures:?}"),
                    },
                    at,
                );
            },
            EngineRunEvent::Terminal { outcome } => {
                self.live = false;
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

    /// A failed durable read does not end an independently attached subscription.
    pub(crate) fn history_unconfirmed(&mut self, message: &str) {
        self.record_observation_error(message, chrono::Local::now().format("%H:%M:%S").to_string());
        self.observation_error_from_read = true;
    }

    pub(crate) fn history_unconfirmed_if_unchanged(&mut self, message: &str, revision: u64) {
        if revision != u64::MAX
            && self.observation_revision == revision
            && (self.observation_error.is_none() || self.observation_error_from_read)
        {
            self.history_unconfirmed(message);
        }
    }

    fn record_observation_error(&mut self, message: &str, time: String) {
        let message = surge_core::recovery_observation::recovery_diagnostic(message);
        self.observation_revision = self.observation_revision.saturating_add(1);
        self.observation_error = Some(message.clone());
        self.observation_error_from_read = false;
        self.log.push_back(RunLogRow {
            time,
            kind: "STREAM",
            tone: Tone::Err,
            text: format!("Run outcome unconfirmed: {message}"),
        });
        while self.log.len() > MAX_LOG_ROWS {
            self.log.pop_front();
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

    fn fold_cost(&mut self, payload: &EventPayload) {
        if let Some((_, input, output, cost)) = payload.token_delta() {
            if let (Some(input), Some(output)) = (
                self.tokens_in.checked_add(input),
                self.tokens_out.checked_add(output),
            ) {
                self.tokens_in = input;
                self.tokens_out = output;
            } else {
                self.usage_unknown = true;
            }
            if let Some(cost) = cost {
                self.cost_usd += cost;
            } else {
                self.cost_unknown = true;
            }
        } else if matches!(payload, EventPayload::ProviderUsageObserved { .. }) {
            self.usage_unknown = true;
            self.cost_unknown = true;
        }
    }
}

/// One-line log description of a payload. `None` = don't log (noise).
#[allow(clippy::too_many_lines)]
fn describe(payload: &EventPayload) -> Option<(&'static str, Tone, String)> {
    if let EventPayload::RunRecoveryRequired { diagnostic, .. } = payload {
        return Some((
            "RECOVERY",
            Tone::Warn,
            surge_core::recovery_observation::recovery_diagnostic(diagnostic),
        ));
    }
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
                "+{output_tokens} tok · {} · {model}",
                cost_usd.map_or_else(|| "cost unavailable".into(), |cost| format!("${cost:.2}"))
            ),
        ),
        P::ProviderUsageObserved { usage, .. } => match usage {
            surge_core::usage::TokenUsageObservation::Reported { cumulative, delta } => (
                "USAGE",
                Tone::Info,
                format!(
                    "reported: {} input · {} output · +{} total tokens · cost unavailable",
                    cumulative.input_tokens, cumulative.output_tokens, delta.total_tokens
                ),
            ),
            surge_core::usage::TokenUsageObservation::Unavailable { reason, .. } => (
                "USAGE",
                Tone::Warn,
                format!("token spend unavailable: {reason:?}"),
            ),
        },
        P::ContextUsageObserved { usage, .. } => (
            "CONTEXT",
            Tone::Info,
            format!(
                "context occupancy: {} / {}{}",
                usage.used,
                usage.size,
                usage.cost.as_ref().map_or_else(String::new, |cost| format!(
                    " · session cost {} {}",
                    cost.amount, cost.currency
                ))
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
            "Run state is unconfirmed",
            "disconnect does not confirm current capacity or crash state"
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
        assert!(stream.observation_error.is_some());
        assert_eq!(
            stream.display().label(),
            "Run state is unconfirmed",
            "a registry refresh does not resolve the earlier observation error"
        );
        let mut crash_only = RunStreamState::default();
        crash_only.begin_display_history(id, Some(surge_core::RunStatus::Crashed));
        crash_only.invalidate_display();
        assert_eq!(
            crash_only.display().label(),
            "Recovery requires an operator",
            "registry crash evidence remains visible independently of an observation error"
        );
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
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
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
                None,
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
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
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

    /// A stream whose history is trusted from `RunStarted` at seq 1: the
    /// only kind that shows pending decisions.
    fn trusted_stream() -> RunStreamState {
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

    fn human_input(node_name: &str, call_id: Option<&str>) -> EventPayload {
        EventPayload::HumanInputRequested {
            node: node(node_name),
            session: None,
            call_id: call_id.map(str::to_string),
            prompt: "answer me".into(),
            schema: None,
        }
    }

    #[test]
    fn recovery_repair_buffered_history_overlap_preserves_complete_projection() {
        let run = surge_core::RunId::new();
        let events = [
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: "/project".into(),
                initial_prompt: "buffered overlap".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
            EventPayload::StageEntered {
                node: node("work"),
                attempt: 1,
            },
            EventPayload::StageEntered {
                node: node("next"),
                attempt: 1,
            },
        ];
        let mut stream = RunStreamState::default();
        stream.begin_display_history(run, None);
        for (index, payload) in events[..2].iter().enumerate() {
            stream.apply_recorded(&persisted(index as u64 + 1, payload.clone()), 0);
        }
        stream.finish_display_history(2);
        stream.live = true;
        for (index, payload) in events.iter().enumerate() {
            stream.apply(&persisted(index as u64 + 1, payload.clone()));
        }
        assert!(
            stream.recovery().is_complete(),
            "already hydrated broadcast envelopes are transport overlap, not corrupt journal ordering"
        );
        assert!(stream.observation_available_for(run));
        assert_eq!(stream.display().label(), "Running");
        assert_eq!(stream.last_seq, 3);
        stream.begin_display_history(run, None);
        for (index, payload) in events.iter().enumerate() {
            stream.apply_recorded(&persisted(index as u64 + 1, payload.clone()), 0);
        }
        stream.finish_display_history(3);
        assert!(
            stream.recovery().is_complete(),
            "full rehydration must not be deduplicated against retained transport last_seq"
        );
        assert!(stream.observation_available_for(run));
        stream.apply(&persisted(
            5,
            EventPayload::StageEntered {
                node: node("gap"),
                attempt: 1,
            },
        ));
        assert!(!stream.recovery().is_complete());
        assert!(!stream.observation_available_for(run));
        assert_eq!(stream.display().label(), "Run state is unconfirmed");
    }

    #[test]
    fn recovery_observability_availability_requires_exact_run_error_free_complete_feed() {
        let mut stream = trusted_stream();
        let run = stream.display_run_id.unwrap();
        stream.live = true;
        assert!(stream.observation_available_for(run));
        assert!(!stream.observation_available_for(surge_core::RunId::new()));
        stream.apply(&EngineRunEvent::StreamError {
            message: "failed read".into(),
        });
        assert!(!stream.observation_available_for(run));
        stream.live = true;
        assert!(
            !stream.observation_available_for(run),
            "an old error remains unconfirmed until actual reattachment"
        );
        stream.observation_error = None;
        assert!(stream.observation_available_for(run));
        stream.apply(&persisted(
            3,
            EventPayload::StageEntered {
                node: node("work"),
                attempt: 1,
            },
        ));
        assert!(!stream.observation_available_for(run));
    }

    #[test]
    fn recovery_observability_cold_progress_needs_attached_observation_for_working() {
        let mut stream = trusted_stream();
        assert_eq!(stream.display().label(), "Run state is unconfirmed");
        stream.live = true;
        assert_eq!(stream.display().label(), "Running");
        stream.apply(&EngineRunEvent::StreamError {
            message: "disconnected".into(),
        });
        assert_eq!(stream.display().label(), "Run state is unconfirmed");
        assert!(!stream.live);
    }

    #[test]
    fn recovery_observability_cold_and_gap_keep_bounded_literal_history() {
        let mut stream = trusted_stream();
        let diagnostic = format!(
            "<tag>[link](https://example.test)\u{001b}{}",
            "界".repeat(5000)
        );
        stream.apply(&persisted(
            2,
            EventPayload::RunRecoveryRequired {
                control_generation: 0,
                diagnostic: diagnostic.clone(),
            },
        ));
        stream.finish_display_history(2);
        assert!(!stream.live);
        assert_eq!(stream.display().label(), "Recovery requires an operator");
        let row = stream.log.back().unwrap();
        assert_eq!(row.kind, "RECOVERY");
        assert!(row.text.starts_with("<tag>[link](https://example.test)�"));
        assert!(row.text.ends_with("[diagnostic truncated]"));
        assert!(row.text.len() <= surge_core::recovery_observation::RECOVERY_DIAGNOSTIC_BYTES);
        assert_eq!(stream.recovery().latest().unwrap().diagnostic, diagnostic);
        stream.apply(&persisted(
            4,
            EventPayload::StageEntered {
                node: node("work"),
                attempt: 1,
            },
        ));
        assert_eq!(stream.display().label(), "Run state is unconfirmed");
        assert!(stream.recovery().active().is_none());
        assert_eq!(stream.recovery().latest().unwrap().diagnostic, diagnostic);
    }

    #[test]
    fn recovery_observability_incremental_every_prefix_matches_shared_compiler() {
        let run = surge_core::RunId::new();
        let mut stream = RunStreamState::default();
        stream.begin_display_history(run, None);
        let payloads = [
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: "/project".into(),
                initial_prompt: "parity".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
            EventPayload::RunRecoveryRequired {
                control_generation: 2,
                diagnostic: "first".into(),
            },
            EventPayload::RunContinued {
                control_generation: 3,
            },
            EventPayload::RunRecoveryRequired {
                control_generation: 0,
                diagnostic: "latest lower generation".into(),
            },
            EventPayload::StageEntered {
                node: node("work"),
                attempt: 1,
            },
            EventPayload::RunAborted {
                reason: "stopped".into(),
            },
        ];
        let mut events = Vec::new();
        for (index, payload) in payloads.into_iter().enumerate() {
            let event = surge_core::RunEvent {
                run_id: run,
                seq: index as u64 + 1,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                payload,
            };
            stream.apply(&persisted(event.seq, event.payload.clone()));
            events.push(event);
            let compiled =
                surge_core::task_execution::TaskExecutionView::compile(run, &events).unwrap();
            assert_eq!(
                stream.recovery(),
                &compiled.recovery,
                "prefix {}",
                index + 1
            );
        }
        let before = stream.recovery().latest().cloned();
        stream.apply(&persisted(
            2,
            EventPayload::RunRecoveryRequired {
                control_generation: 99,
                diagnostic: "stale duplicate".into(),
            },
        ));
        assert_eq!(stream.recovery().latest(), before.as_ref());
        assert!(
            stream.recovery().is_complete(),
            "transport overlap does not invalidate a previously accepted prefix"
        );
        assert_eq!(
            stream.recovery().latest().unwrap().diagnostic,
            "latest lower generation"
        );
        assert!(stream.recovery().active().is_none());
    }

    #[test]
    fn recovery_observability_pre_materialization_bound_owner_survives_transition() {
        use surge_core::work_item::{WorkItemBinding, WorkItemContext, WorkItemRequirements};
        let mut stream = trusted_stream();
        let requirements = WorkItemRequirements::new("build".into(), vec!["works".into()]).unwrap();
        let context = WorkItemContext::new(
            WorkItemBinding {
                item: "00000000000000000000000001".parse().unwrap(),
                revision: 1,
                requirements_hash: requirements.hash().unwrap(),
                generation: 1,
            },
            requirements,
        )
        .unwrap();
        stream.apply(&persisted(
            2,
            EventPayload::WorkItemAttemptBound { context },
        ));
        stream.apply(&persisted(
            3,
            EventPayload::RunRecoveryRequired {
                control_generation: 0,
                diagnostic: "bound owner refused".into(),
            },
        ));
        assert_eq!(stream.display().label(), "Recovery requires an operator");
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
        stream.apply(&persisted(
            4,
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash: surge_core::ContentHash::compute(b"graph"),
            },
        ));
        stream.apply(&persisted(
            5,
            EventPayload::StageEntered {
                node: node("end"),
                attempt: 1,
            },
        ));
        assert_eq!(
            stream.display().label(),
            "Recovery requires an operator",
            "stage progress cannot bypass the bound owner's Continue fence"
        );
        stream.apply(&persisted(
            6,
            EventPayload::RunContinued {
                control_generation: 1,
            },
        ));
        assert_ne!(stream.display().label(), "Recovery requires an operator");
    }

    #[test]
    fn recovery_observability_terminal_keeps_decision_and_literal_diagnostic() {
        let mut stream = trusted_stream();
        stream.live = true;
        stream.apply(&persisted(2, human_input("review", Some("call-1"))));
        stream.apply(&persisted(
            3,
            EventPayload::RunRecoveryRequired {
                control_generation: 0,
                diagnostic: "conflicting durable gate occurrences".into(),
            },
        ));
        stream.apply(&EngineRunEvent::Terminal {
            outcome: surge_orchestrator::engine::handle::RunOutcome::RecoveryRequired {
                control_generation: 0,
                diagnostic: "conflicting durable gate occurrences".into(),
            },
        });
        assert!(
            !stream.live,
            "an ended observation is not a live subscription"
        );
        assert_eq!(stream.pending.len(), 1);
        let row = stream
            .log
            .iter()
            .find(|row| row.kind == "RECOVERY")
            .unwrap();
        assert_eq!(row.text, "conflicting durable gate occurrences");
        assert_eq!(stream.display().label(), "Recovery requires an operator");
    }

    #[test]
    fn settlement_failure_preserves_pending_decision_and_reports_unconfirmed() {
        let mut stream = trusted_stream();
        stream.live = true;
        stream.apply(&persisted(2, human_input("review", Some("call-1"))));
        stream.apply(&EngineRunEvent::Terminal {
            outcome: surge_orchestrator::engine::handle::RunOutcome::SettlementFailed {
                original: Box::new(surge_orchestrator::engine::handle::RunOutcome::Completed {
                    terminal: "end".try_into().unwrap(),
                }),
                failures: vec![
                    surge_orchestrator::engine::handle::RunSettlementFailure::ForwarderRead,
                ],
            },
        });
        assert!(!stream.live);
        assert_eq!(stream.pending.len(), 1);
        assert_eq!(stream.pending[0].node, "review");
        assert!(stream.log.back().unwrap().text.contains("unconfirmed"));
    }

    #[test]
    fn stream_failure_preserves_last_durable_decision_without_terminal_claim() {
        let mut stream = trusted_stream();
        stream.live = true;
        stream.apply(&persisted(2, human_input("review", Some("call-1"))));
        stream.apply(&EngineRunEvent::StreamError {
            message: "catch-up storage unavailable".into(),
        });
        assert!(!stream.live);
        assert_eq!(stream.last_seq, 2);
        assert_eq!(stream.pending.len(), 1);
        assert_eq!(stream.pending[0].node, "review");
        let error = stream.log.back().unwrap();
        assert_eq!(error.kind, "STREAM");
        assert!(error.text.contains("catch-up storage unavailable"));
        assert!(error.text.contains("unconfirmed"));
    }

    #[test]
    fn recorded_terminal_uses_durable_time_and_clears_pending() {
        let mut state = trusted_stream();
        state.apply(&persisted(2, human_input("gate", None)));
        assert_eq!(state.pending.len(), 1);
        state.apply_recorded(
            &persisted(
                3,
                EventPayload::RunAborted {
                    reason: "done".into(),
                },
            ),
            0,
        );
        assert_eq!(state.pending.len(), 0);
        let expected = chrono::DateTime::from_timestamp_millis(0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string();
        assert_eq!(state.log.back().unwrap().time, expected);
    }

    /// #121: a stream whose first history load failed shows no decisions;
    /// a later successful load (the app retries) replays the journal, which
    /// already holds every live event received before it, so the request
    /// appears, and a duplicate live delivery is ignored.
    #[test]
    fn a_late_history_restore_reveals_decisions_without_losing_live_events() {
        let run = surge_core::RunId::new();
        let started = persisted(
            1,
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: "/project".into(),
                initial_prompt: "late restore".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
        );
        let request = persisted(2, human_input("review", Some("call-1")));

        // The first restore failed: live events arrive without trusted history.
        let mut stream = RunStreamState {
            live: true,
            ..Default::default()
        };
        stream.apply(&request);
        assert!(
            stream.pending.is_empty(),
            "no decisions without trusted history"
        );

        // The retry reads the journal (seq 1..=2) and replays it.
        stream.begin_display_history(run, None);
        for event in [&started, &request] {
            stream.apply_recorded(event, 1_760_000_000_000);
        }
        stream.finish_display_history(2);
        assert_eq!(stream.pending.len(), 1);
        assert_eq!(stream.pending[0].node, "review");

        // A live copy of an already-replayed event changes nothing.
        stream.apply(&request);
        assert_eq!(stream.pending.len(), 1);
        assert_eq!(stream.last_seq, 2);
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
    fn tokens_accumulate_and_terminal_clears_pending() {
        let mut s = trusted_stream();
        s.apply(&persisted(
            2,
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
            3,
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

        s.apply(&persisted(4, human_input("gate_a", None)));
        assert_eq!(s.pending.len(), 1);
        s.apply(&EngineRunEvent::Terminal {
            outcome: surge_orchestrator::engine::handle::RunOutcome::Aborted {
                reason: "operator".into(),
            },
        });
        assert_eq!(s.pending.len(), 0, "terminal run has nothing to decide");
    }
}
