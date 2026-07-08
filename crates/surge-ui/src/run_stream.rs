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

use gpui::Hsla;
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
    pub seq: u64,
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
    HumanInput {
        call_id: Option<String>,
        prompt: String,
        schema: Option<serde_json::Value>,
    },
    /// `ApprovalRequested` at a HumanGate node.
    Gate { gate: String },
    /// `BootstrapApprovalRequested` (description / roadmap / flow).
    Bootstrap { stage: String },
    /// `SandboxElevationRequested` — capability grant.
    Elevation { capability: String },
    /// `RoadmapPatchApprovalRequested`.
    RoadmapPatch { patch_id: String },
    /// `EscalationRequested` — the engine gave up and needs a human.
    Escalation { reason: String },
}

impl DecisionKind {
    /// Short badge label for queue rows.
    pub fn badge(&self) -> &'static str {
        match self {
            Self::HumanInput { .. } => "human input",
            Self::Gate { .. } => "review gate",
            Self::Bootstrap { .. } => "plan gate",
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
            Self::Gate { .. } => 3,
            Self::Bootstrap { .. } => 3,
            Self::RoadmapPatch { .. } => 4,
        }
    }
}

/// Folded live view of one run's event stream.
#[derive(Default)]
pub struct RunStreamState {
    /// Subscription currently attached and pumping.
    pub live: bool,
    pub log: VecDeque<RunLogRow>,
    pub stages: Vec<StageRow>,
    pub pending: Vec<PendingDecision>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    /// Highest seq observed on this stream.
    pub last_seq: u64,
}

impl RunStreamState {
    pub fn apply(&mut self, event: &EngineRunEvent) {
        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        match event {
            EngineRunEvent::Persisted { seq, payload } => {
                self.last_seq = self.last_seq.max(*seq);
                self.fold_stages(payload);
                self.fold_pending(*seq, &now, payload);
                self.fold_cost(payload);
                if let Some((kind, tone, text)) = describe(payload) {
                    self.log.push_back(RunLogRow {
                        seq: *seq,
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
            EngineRunEvent::Terminal { .. } => {
                // Blocked decisions die with the run.
                self.pending.clear();
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

    fn fold_pending(&mut self, seq: u64, now: &str, payload: &EventPayload) {
        let push = |list: &mut Vec<PendingDecision>, node: String, kind: DecisionKind| {
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
                &mut self.pending,
                node.as_str().to_string(),
                DecisionKind::HumanInput {
                    call_id: call_id.clone(),
                    prompt: prompt.clone(),
                    schema: schema.clone(),
                },
            ),
            EventPayload::HumanInputResolved { call_id, node, .. }
            | EventPayload::HumanInputTimedOut { call_id, node, .. } => {
                self.pending.retain(|p| match &p.kind {
                    DecisionKind::HumanInput { call_id: c, .. } => {
                        // Match by call_id only when both sides carry one —
                        // `None == None` would otherwise remove unrelated
                        // pending inputs on other nodes. Fall back to the
                        // requesting node.
                        let resolved = match (c, call_id) {
                            (Some(a), Some(b)) => a == b,
                            _ => p.node == node.as_str(),
                        };
                        !resolved
                    },
                    _ => true,
                });
            },
            EventPayload::ApprovalRequested { gate, .. } => push(
                &mut self.pending,
                gate.as_str().to_string(),
                DecisionKind::Gate {
                    gate: gate.as_str().to_string(),
                },
            ),
            EventPayload::ApprovalDecided { gate, .. } => {
                self.pending.retain(
                    |p| !matches!(&p.kind, DecisionKind::Gate { gate: g } if g == gate.as_str()),
                );
            },
            EventPayload::BootstrapApprovalRequested { stage, .. } => push(
                &mut self.pending,
                format!("{stage:?}").to_lowercase(),
                DecisionKind::Bootstrap {
                    stage: format!("{stage:?}").to_lowercase(),
                },
            ),
            EventPayload::BootstrapApprovalDecided { stage, .. } => {
                let s = format!("{stage:?}").to_lowercase();
                self.pending.retain(
                    |p| !matches!(&p.kind, DecisionKind::Bootstrap { stage } if *stage == s),
                );
            },
            EventPayload::SandboxElevationRequested { node, capability } => push(
                &mut self.pending,
                node.as_str().to_string(),
                DecisionKind::Elevation {
                    capability: capability.clone(),
                },
            ),
            EventPayload::SandboxElevationDecided { node, .. }
            | EventPayload::SandboxElevationTimedOut { node, .. } => {
                let n = node.as_str();
                self.pending
                    .retain(|p| !(matches!(p.kind, DecisionKind::Elevation { .. }) && p.node == n));
            },
            EventPayload::RoadmapPatchApprovalRequested { patch_id, .. } => push(
                &mut self.pending,
                patch_id.to_string(),
                DecisionKind::RoadmapPatch {
                    patch_id: patch_id.to_string(),
                },
            ),
            EventPayload::RoadmapPatchApprovalDecided { patch_id, .. } => {
                let id = patch_id.to_string();
                self.pending.retain(
                    |p| !matches!(&p.kind, DecisionKind::RoadmapPatch { patch_id } if *patch_id == id),
                );
            },
            EventPayload::EscalationRequested { reason, .. } => push(
                &mut self.pending,
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

    fn persisted(seq: u64, payload: EventPayload) -> EngineRunEvent {
        EngineRunEvent::Persisted {
            seq,
            payload: Box::new(payload),
        }
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
        let mut s = RunStreamState::default();
        s.apply(&persisted(1, human_input("gate_a", Some("call-1"))));
        s.apply(&persisted(2, human_input("gate_b", Some("call-2"))));
        assert_eq!(s.pending.len(), 2);

        s.apply(&persisted(
            3,
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
        let mut s = RunStreamState::default();
        s.apply(&persisted(1, human_input("gate_a", None)));
        s.apply(&persisted(2, human_input("gate_b", None)));
        assert_eq!(s.pending.len(), 2);

        s.apply(&persisted(
            3,
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
