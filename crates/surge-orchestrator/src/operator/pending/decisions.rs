//! The one fold of a run's events into the operator decisions waiting on it
//! (#102). The CLI, the MCP server, the desktop app and the Telegram cockpit
//! read this set and only render it.
//!
//! [`PendingDecisions`] is fed run events in sequence order and reports, per
//! event, which decisions opened and retired. It does no I/O, takes no clock
//! (deadlines come from event timestamps and the run's frozen graph) and is
//! synchronous, so an adapter feeds it when events arrive, never while
//! drawing.
//!
//! # Sequence discipline
//! Events at or below the last folded sequence are ignored, so replaying a
//! prefix twice or overlapping hydration with live events is a no-op. A
//! forward gap (or a log the run-state fold refuses) makes the fold
//! [`FoldStatus::Incomplete`]: it stops folding and its set is not to be
//! trusted until it is rebuilt from a complete prefix.
//!
//! # What opens a decision
//! - `HumanInputRequested`, classified by [`DecisionKind`]: a bootstrap
//!   approval (`x-surge-bootstrap-stage` schema marker, or a bootstrap-mode
//!   gate node), a skill approval (`x-surge-skill-bindings` marker), a gate
//!   (namespaced gate request id), a tool-call input (any other call id), or
//!   a legacy unbound request (no call id).
//! - `SandboxElevationRequested`, `RoadmapPatchApprovalRequested`,
//!   `EscalationRequested`.
//!
//! `ApprovalRequested` / `ApprovalDecided` and `BootstrapApprovalRequested` /
//! `BootstrapApprovalDecided` are bookkeeping companions of a human-input
//! request and never open a second decision.
//!
//! # What retires one
//! - A human-input request: `HumanInputResolved` (answered) or
//!   `HumanInputTimedOut` (timed out) with the same call id; when both sides
//!   lack a call id, by node; a call id on one side only never matches.
//! - An elevation: `SandboxElevationDecided` / `SandboxElevationTimedOut` for
//!   the same node.
//! - A roadmap patch approval: `RoadmapPatchApprovalDecided` for the same
//!   patch id.
//! - An escalation has no answering event; it retires when the log shows its
//!   cause was dealt with: a capacity blind-park limit on the next
//!   `StageCompleted`; MCP restarts exhausted on the next `RunContinued`; a
//!   loop-guard trip or an unavailable selected MCP catalog on a
//!   `StageCompleted` of the node it is attributed to (the latest
//!   `StageEntered` before it). Any other cause retires only on a final
//!   outcome. Steering never retires an escalation.
//! - `RunCompleted`, `RunFailed` and `RunAborted` retire everything (final
//!   outcome). Suspension and parking retire nothing.
//!
//! # Actionability
//! A human-input decision is actionable only while it is the run's current
//! pending request, the cursor is on its node, the run is not parked, and it
//! carries a call id (a namespaced gate request id or a scoped tool call id).
//! A legacy unbound request is listed but never actionable. An elevation or
//! roadmap patch approval is actionable while open; an escalation never is
//! (there is nothing to answer).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use surge_core::RunId;
use surge_core::graph::Graph;
use surge_core::human_gate_config::HumanGateMode;
use surge_core::keys::NodeKey;
use surge_core::node::NodeConfig;
use surge_core::roadmap_patch::RoadmapPatchTarget;
use surge_core::run_event::{BootstrapStage, EscalationCause, EventPayload, RunEvent};
use surge_core::run_state::RunState;
use surge_core::skill::SkillRef;

use super::GateOption;

/// What a pending decision asks of the operator.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum DecisionKind {
    /// A pipeline `HumanGate`, answered with one of its outcomes.
    Gate {
        /// Namespaced gate request id the answer is delivered under.
        call_id: String,
    },
    /// A bootstrap approval (description, roadmap or flow).
    BootstrapApproval {
        /// The bootstrap stage being approved.
        stage: BootstrapStage,
        /// The request's call id, when it carries one.
        call_id: Option<String>,
    },
    /// Trust for skills an agent stage is about to bind.
    SkillApproval {
        /// The request's call id, when it carries one.
        call_id: Option<String>,
        /// The skills being bound.
        skills: Vec<SkillRef>,
    },
    /// A tool-driven `request_human_input`, answered with text or JSON.
    ToolCallInput {
        /// Scoped tool-call key the answer is delivered under.
        call_id: String,
    },
    /// A request recorded before requests carried a call id: readable, never
    /// actionable.
    LegacyUnbound,
    /// A sandbox elevation request.
    Elevation {
        /// The capability being requested.
        capability: String,
    },
    /// A roadmap patch awaiting approval through the roadmap amendment flow.
    RoadmapPatch {
        /// The patch id.
        patch_id: String,
        /// What the patch amends.
        target: RoadmapPatchTarget,
    },
    /// The engine gave up and needs the operator's attention.
    Escalation {
        /// Why it escalated.
        cause: EscalationCause,
        /// The bootstrap stage, for a bootstrap escalation.
        stage: Option<BootstrapStage>,
    },
}

impl DecisionKind {
    /// Urgency: lower is more urgent. Escalations, then elevations, then
    /// human input, then roadmap patches.
    #[must_use]
    pub fn rank(&self) -> u8 {
        match self {
            Self::Escalation { .. } => 0,
            Self::Elevation { .. } => 1,
            Self::RoadmapPatch { .. } => 3,
            _ => 2,
        }
    }

    /// Whether this is a human-input request (answered through
    /// `Engine::resolve_human_input`).
    #[must_use]
    pub fn is_human_input(&self) -> bool {
        matches!(
            self,
            Self::Gate { .. }
                | Self::BootstrapApproval { .. }
                | Self::SkillApproval { .. }
                | Self::ToolCallInput { .. }
                | Self::LegacyUnbound
        )
    }

    /// The call id an answer is delivered under, for human input.
    #[must_use]
    pub fn call_id(&self) -> Option<&str> {
        match self {
            Self::Gate { call_id } | Self::ToolCallInput { call_id } => Some(call_id),
            Self::BootstrapApproval { call_id, .. } | Self::SkillApproval { call_id, .. } => {
                call_id.as_deref()
            },
            _ => None,
        }
    }
}

/// A decision's stable identity: the run and the sequence of the event that
/// opened it (unique per decision; the kind-specific key — call id, node,
/// patch id, cause — is on [`PendingDecision::kind`] and `node`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DecisionId {
    /// The run.
    pub run: RunId,
    /// Sequence of the event that opened the decision.
    pub request_seq: u64,
}

/// One operator decision a run is (or was) waiting on.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingDecision {
    /// Stable identity.
    pub id: DecisionId,
    /// When it was requested.
    pub requested_at: DateTime<Utc>,
    /// The node it concerns: the requesting node, the elevated node, or the
    /// node an escalation is attributed to. `None` for a roadmap patch and an
    /// unattributed escalation.
    pub node: Option<NodeKey>,
    /// What it asks.
    pub kind: DecisionKind,
    /// The question or reason shown to the operator.
    pub prompt: String,
    /// The outcomes it accepts; empty when it is answered free-form or
    /// elsewhere.
    pub options: Vec<GateOption>,
    /// When it times out, when the frozen graph declares a timeout.
    pub deadline: Option<DateTime<Utc>>,
    /// Whether an answer would be accepted now (see the module docs).
    pub actionable: bool,
}

/// Why a decision is no longer pending.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retirement {
    /// A human-input request was answered.
    Answered,
    /// A human-input request or elevation timed out.
    TimedOut,
    /// An elevation or roadmap patch approval was decided.
    Decided,
    /// The log shows an escalation's cause was dealt with.
    CauseCleared,
    /// The run reached a final outcome.
    FinalOutcome,
}

/// What feeding one event changed.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionChange {
    /// A decision opened.
    Opened(Box<PendingDecision>),
    /// A decision retired.
    Retired {
        /// The retired decision.
        id: DecisionId,
        /// Why.
        reason: Retirement,
    },
}

/// Whether the folded set can be trusted.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldStatus {
    /// Every event from seq 1 to the last folded one was seen.
    Complete,
    /// The fold stopped; rebuild it from a complete prefix.
    Incomplete(Incomplete),
}

/// Why a fold stopped.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incomplete {
    /// An event skipped sequences.
    Gap {
        /// The sequence the fold expected next.
        expected: u64,
        /// The sequence it was fed.
        found: u64,
    },
    /// The run-state fold refused the log.
    Unfoldable(surge_core::run_state::FoldError),
}

/// What [`PendingDecisions::feed`] did with an event.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Fed {
    /// The event was folded; these decisions changed.
    Applied(Vec<DecisionChange>),
    /// The event was at or below the last folded sequence: ignored.
    Duplicate,
    /// The fold is incomplete; the event was not folded.
    Incomplete,
}

/// An escalation's retirement signal, read from existing events.
#[derive(Debug, Clone)]
enum EscalationClears {
    /// The next `StageCompleted` anywhere.
    NextStageCompleted,
    /// The next `RunContinued`.
    NextRunContinued,
    /// A `StageCompleted` on this node.
    StageCompletedOn(NodeKey),
    /// Only a final outcome.
    FinalOutcomeOnly,
}

#[derive(Debug, Clone)]
struct Open {
    decision: PendingDecision,
    clears: EscalationClears,
}

/// The fold of one run's events into its pending operator decisions.
#[derive(Debug, Clone)]
pub struct PendingDecisions {
    run: RunId,
    last_seq: u64,
    status: FoldStatus,
    state: RunState,
    /// The latest `StageEntered` node, for escalation attribution.
    entered: Option<NodeKey>,
    open: Vec<Open>,
    retired: Vec<(PendingDecision, Retirement)>,
}

impl PendingDecisions {
    /// An empty fold for `run`, expecting seq 1 next.
    #[must_use]
    pub fn new(run: RunId) -> Self {
        Self {
            run,
            last_seq: 0,
            status: FoldStatus::Complete,
            state: RunState::NotStarted,
            entered: None,
            open: Vec::new(),
            retired: Vec::new(),
        }
    }

    /// Fold a whole run's events.
    #[must_use]
    pub fn from_events(run: RunId, events: &[RunEvent]) -> Self {
        let mut fold = Self::new(run);
        for event in events {
            let _ = fold.feed(event);
        }
        fold
    }

    /// The run this fold belongs to.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }

    /// The last folded sequence.
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Whether the set can be trusted.
    #[must_use]
    pub fn status(&self) -> &FoldStatus {
        &self.status
    }

    /// The run state folded alongside, for adapters that already need it.
    #[must_use]
    pub fn run_state(&self) -> &RunState {
        &self.state
    }

    /// Open decisions in request order; empty when the fold is incomplete.
    pub fn open(&self) -> impl Iterator<Item = &PendingDecision> {
        let complete = self.status == FoldStatus::Complete;
        self.open
            .iter()
            .filter(move |_| complete)
            .map(|open| &open.decision)
    }

    /// Retired decisions in retirement order, with why. Unlike [`Self::open`],
    /// this stays readable after the fold becomes incomplete: a retirement it
    /// saw before stopping did happen.
    pub fn retired(&self) -> impl Iterator<Item = (&PendingDecision, Retirement)> {
        self.retired
            .iter()
            .map(|(decision, reason)| (decision, *reason))
    }

    /// The open decision opened at `request_seq`, if any.
    #[must_use]
    pub fn open_at(&self, request_seq: u64) -> Option<&PendingDecision> {
        self.open()
            .find(|decision| decision.id.request_seq == request_seq)
    }

    /// How the decision opened at `request_seq` retired, if it did.
    #[must_use]
    pub fn retirement_of(&self, request_seq: u64) -> Option<Retirement> {
        self.retired
            .iter()
            .find(|(decision, _)| decision.id.request_seq == request_seq)
            .map(|(_, reason)| *reason)
    }

    /// The run's current pending human-input request (the one the run-state
    /// fold carries), whether or not it is actionable.
    #[must_use]
    pub fn current_request(&self) -> Option<&PendingDecision> {
        let RunState::Pipeline {
            pending_human_input: Some(pending),
            ..
        } = &self.state
        else {
            return None;
        };
        self.open_at(pending.requested_seq)
    }

    /// Fold one event (see the module docs for the rules).
    pub fn feed(&mut self, event: &RunEvent) -> Fed {
        if self.status != FoldStatus::Complete {
            return Fed::Incomplete;
        }
        if event.seq <= self.last_seq {
            return Fed::Duplicate;
        }
        if event.seq != self.last_seq + 1 {
            self.status = FoldStatus::Incomplete(Incomplete::Gap {
                expected: self.last_seq + 1,
                found: event.seq,
            });
            return Fed::Incomplete;
        }
        let state = std::mem::replace(&mut self.state, RunState::NotStarted);
        match surge_core::run_state::apply(state, event) {
            Ok(state) => self.state = state,
            Err(error) => {
                self.status = FoldStatus::Incomplete(Incomplete::Unfoldable(error));
                return Fed::Incomplete;
            },
        }
        self.last_seq = event.seq;
        let mut changes = Vec::new();
        self.retire_for(event, &mut changes);
        if let Some(open) = self.open_for(event) {
            changes.push(DecisionChange::Opened(Box::new(open.decision.clone())));
            self.open.push(open);
        }
        if let EventPayload::StageEntered { node, .. } = &event.payload {
            self.entered = Some(node.clone());
        }
        self.refresh_actionable();
        Fed::Applied(changes)
    }

    fn graph(&self) -> Option<&Arc<Graph>> {
        match &self.state {
            RunState::Pipeline { graph, .. } => Some(graph),
            _ => None,
        }
    }

    fn open_for(&self, event: &RunEvent) -> Option<Open> {
        let graph = self.graph().map(Arc::as_ref);
        let Opening {
            node,
            kind,
            prompt,
            options,
        } = classify_opening(&event.payload, graph)?;
        let deadline = node
            .as_ref()
            .filter(|_| kind.is_human_input())
            .and_then(|node| self.gate_timeout(node))
            .and_then(|seconds| {
                event
                    .timestamp
                    .checked_add_signed(chrono::Duration::seconds(i64::from(seconds)))
            });
        // An escalation is attributed to the latest entered stage.
        let (node, clears) = match &kind {
            DecisionKind::Escalation { cause, .. } => {
                let attributed = self.entered.clone();
                let clears = match cause {
                    EscalationCause::CapacityBlindParkLimitExceeded => {
                        EscalationClears::NextStageCompleted
                    },
                    EscalationCause::McpRestartsExhausted => EscalationClears::NextRunContinued,
                    EscalationCause::LoopGuardRepeatedToolCall
                    | EscalationCause::LoopGuardNodeDeadline
                    | EscalationCause::LoopGuardNoProgress
                    | EscalationCause::LoopGuardToolCallCap
                    | EscalationCause::McpSelectedCatalogUnavailable => attributed
                        .clone()
                        .map_or(EscalationClears::FinalOutcomeOnly, |node| {
                            EscalationClears::StageCompletedOn(node)
                        }),
                    _ => EscalationClears::FinalOutcomeOnly,
                };
                (attributed, clears)
            },
            _ => (node, EscalationClears::FinalOutcomeOnly),
        };
        Some(Open {
            decision: PendingDecision {
                id: DecisionId {
                    run: self.run,
                    request_seq: event.seq,
                },
                requested_at: event.timestamp,
                node,
                kind,
                prompt,
                options,
                deadline,
                actionable: false,
            },
            clears,
        })
    }

    fn gate_timeout(&self, node: &NodeKey) -> Option<u32> {
        match &self.graph()?.find_node(node)?.config {
            NodeConfig::HumanGate(config) => config.timeout_seconds,
            _ => None,
        }
    }

    fn retire_for(&mut self, event: &RunEvent, changes: &mut Vec<DecisionChange>) {
        let final_outcome = matches!(
            event.payload,
            EventPayload::RunCompleted { .. }
                | EventPayload::RunFailed { .. }
                | EventPayload::RunAborted { .. }
        );
        let open = std::mem::take(&mut self.open);
        for entry in open {
            let reason = if final_outcome {
                Some(Retirement::FinalOutcome)
            } else {
                retires(&entry, &event.payload)
            };
            match reason {
                Some(reason) => {
                    changes.push(DecisionChange::Retired {
                        id: entry.decision.id.clone(),
                        reason,
                    });
                    let mut decision = entry.decision;
                    decision.actionable = false;
                    self.retired.push((decision, reason));
                },
                None => self.open.push(entry),
            }
        }
    }

    fn refresh_actionable(&mut self) {
        let current = match &self.state {
            RunState::Pipeline {
                cursor,
                pending_human_input: Some(pending),
                parked: None,
                ..
            } if cursor.node == pending.node => Some(pending.requested_seq),
            _ => None,
        };
        for entry in &mut self.open {
            let decision = &mut entry.decision;
            decision.actionable = match &decision.kind {
                DecisionKind::Elevation { .. } | DecisionKind::RoadmapPatch { .. } => true,
                DecisionKind::Escalation { .. } | DecisionKind::LegacyUnbound => false,
                kind => current == Some(decision.id.request_seq) && kind.call_id().is_some(),
            };
        }
    }
}

/// A decision one event opens, classified from the event alone (and the
/// run's graph when the caller has it). [`PendingDecisions`] classifies with
/// this, so a surface that sees single events (the Telegram live dispatch)
/// classifies exactly as the fold does.
#[derive(Debug, Clone, PartialEq)]
pub struct Opening {
    /// The requesting or elevated node; `None` for a roadmap patch and for an
    /// escalation (the fold attributes escalations to the latest stage).
    pub node: Option<NodeKey>,
    /// What it asks.
    pub kind: DecisionKind,
    /// The question or reason shown to the operator.
    pub prompt: String,
    /// The outcomes it accepts.
    pub options: Vec<GateOption>,
}

/// The decision `payload` opens, if any. Without `graph`, a bootstrap-mode
/// gate is recognised only by its `x-surge-bootstrap-stage` schema marker and
/// gate options only from the request schema.
#[must_use]
pub fn classify_opening(payload: &EventPayload, graph: Option<&Graph>) -> Option<Opening> {
    match payload {
        EventPayload::HumanInputRequested {
            node,
            call_id,
            prompt,
            schema,
            ..
        } => {
            let (kind, options) =
                classify_request(graph, node, call_id.as_deref(), schema.as_ref());
            Some(Opening {
                node: Some(node.clone()),
                kind,
                prompt: prompt.clone(),
                options,
            })
        },
        EventPayload::SandboxElevationRequested { node, capability } => Some(Opening {
            node: Some(node.clone()),
            kind: DecisionKind::Elevation {
                capability: capability.clone(),
            },
            prompt: format!("{node} requests {capability}"),
            options: Vec::new(),
        }),
        EventPayload::RoadmapPatchApprovalRequested {
            patch_id, target, ..
        } => Some(Opening {
            node: None,
            kind: DecisionKind::RoadmapPatch {
                patch_id: patch_id.to_string(),
                target: target.clone(),
            },
            prompt: format!("roadmap patch {patch_id} awaits approval"),
            options: Vec::new(),
        }),
        EventPayload::EscalationRequested {
            stage,
            reason,
            cause,
        } => Some(Opening {
            node: None,
            kind: DecisionKind::Escalation {
                cause: *cause,
                stage: *stage,
            },
            prompt: reason.clone(),
            options: Vec::new(),
        }),
        _ => None,
    }
}

fn classify_request(
    graph: Option<&Graph>,
    node: &NodeKey,
    call_id: Option<&str>,
    schema: Option<&serde_json::Value>,
) -> (DecisionKind, Vec<GateOption>) {
    let marker = |key: &str| schema.and_then(|schema| schema.get(key));
    let gate = graph.and_then(|graph| match graph.find_node(node) {
        Some(found) => match &found.config {
            NodeConfig::HumanGate(config) => Some(config.clone()),
            _ => None,
        },
        None => None,
    });
    let stage = marker("x-surge-bootstrap-stage")
        .and_then(|value| serde_json::from_value::<BootstrapStage>(value.clone()).ok())
        .or_else(|| match gate.as_ref().map(|config| &config.mode) {
            Some(HumanGateMode::Bootstrap { stage }) => Some(*stage),
            _ => None,
        });
    if let Some(stage) = stage {
        return (
            DecisionKind::BootstrapApproval {
                stage,
                call_id: call_id.map(str::to_owned),
            },
            fixed_options(&[
                ("approve", "Approve"),
                ("edit", "Edit"),
                ("reject", "Reject"),
            ]),
        );
    }
    if let Some(skills) = marker("x-surge-skill-bindings")
        .and_then(|value| serde_json::from_value::<Vec<SkillRef>>(value.clone()).ok())
    {
        return (
            DecisionKind::SkillApproval {
                call_id: call_id.map(str::to_owned),
                skills,
            },
            fixed_options(&[("approve", "Approve"), ("reject", "Reject")]),
        );
    }
    let gate_options = || match &gate {
        Some(config) if !config.options.is_empty() => config
            .options
            .iter()
            .map(|option| GateOption {
                outcome: option.outcome.to_string(),
                label: option.label.clone(),
            })
            .collect(),
        _ => schema_options(schema),
    };
    match call_id {
        None => (DecisionKind::LegacyUnbound, gate_options()),
        Some(call_id) if surge_core::id::GateRequestId::from_event_call_id(call_id).is_some() => (
            DecisionKind::Gate {
                call_id: call_id.to_owned(),
            },
            gate_options(),
        ),
        Some(call_id) => (
            DecisionKind::ToolCallInput {
                call_id: call_id.to_owned(),
            },
            Vec::new(),
        ),
    }
}

/// The retirement an event causes for one open decision, final outcomes
/// aside.
fn retires(entry: &Open, payload: &EventPayload) -> Option<Retirement> {
    let decision = &entry.decision;
    match payload {
        EventPayload::HumanInputResolved { node, call_id, .. }
        | EventPayload::HumanInputTimedOut { node, call_id, .. }
            if decision.kind.is_human_input() =>
        {
            let request = decision.kind.call_id();
            let matched = match (request, call_id.as_deref()) {
                (Some(request), Some(answer)) => request == answer,
                (None, None) => decision.node.as_ref() == Some(node),
                _ => false,
            };
            matched.then_some(
                if matches!(payload, EventPayload::HumanInputResolved { .. }) {
                    Retirement::Answered
                } else {
                    Retirement::TimedOut
                },
            )
        },
        EventPayload::SandboxElevationDecided { node, .. }
            if matches!(decision.kind, DecisionKind::Elevation { .. })
                && decision.node.as_ref() == Some(node) =>
        {
            Some(Retirement::Decided)
        },
        EventPayload::SandboxElevationTimedOut { node, .. }
            if matches!(decision.kind, DecisionKind::Elevation { .. })
                && decision.node.as_ref() == Some(node) =>
        {
            Some(Retirement::TimedOut)
        },
        EventPayload::RoadmapPatchApprovalDecided { patch_id, .. } => match &decision.kind {
            DecisionKind::RoadmapPatch { patch_id: open, .. } if *open == patch_id.to_string() => {
                Some(Retirement::Decided)
            },
            _ => None,
        },
        EventPayload::StageCompleted { node, .. } => match &entry.clears {
            EscalationClears::NextStageCompleted => Some(Retirement::CauseCleared),
            EscalationClears::StageCompletedOn(attributed) if attributed == node => {
                Some(Retirement::CauseCleared)
            },
            _ => None,
        },
        EventPayload::RunContinued { .. } => {
            matches!(entry.clears, EscalationClears::NextRunContinued)
                .then_some(Retirement::CauseCleared)
        },
        _ => None,
    }
}

fn fixed_options(options: &[(&str, &str)]) -> Vec<GateOption> {
    options
        .iter()
        .map(|(outcome, label)| GateOption {
            outcome: (*outcome).to_owned(),
            label: (*label).to_owned(),
        })
        .collect()
}

/// Outcomes a request schema enumerates under `properties.outcome.enum`.
fn schema_options(schema: Option<&serde_json::Value>) -> Vec<GateOption> {
    schema
        .and_then(|schema| schema.pointer("/properties/outcome/enum"))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|outcome| GateOption {
                    outcome: outcome.to_owned(),
                    label: outcome.to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
