//! Interface tests for the pending-decisions fold (#102): plain run events in,
//! open set / retired set / per-event changes out.

use std::collections::BTreeMap;
use std::path::PathBuf;

use surge_core::approvals::{ApprovalChannel, ApprovalDuration, ApprovalPolicy};
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
use surge_core::human_gate_config::{
    ApprovalOption, HumanGateConfig, OptionStyle, SummaryTemplate, TimeoutAction,
};
use surge_core::keys::OutcomeKey;
use surge_core::node::{Node, Position};
use surge_core::run_event::RunConfig;
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};

use super::*;

const GATE: &str = "review";
const LOOP_GATE: &str = "loop_review";
const BOOT_GATE: &str = "approve_roadmap";
const AGENT: &str = "implement";

fn key(name: &str) -> NodeKey {
    NodeKey::try_from(name).unwrap()
}

fn gate(options: &[&str], timeout_seconds: Option<u32>, mode: HumanGateMode) -> Node {
    Node {
        id: key(GATE),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::HumanGate(HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Desktop {
                duration: ApprovalDuration::Persistent,
            }],
            timeout_seconds,
            on_timeout: TimeoutAction::default(),
            summary: SummaryTemplate {
                title: "Approve?".into(),
                body: "body".into(),
                show_artifacts: vec![],
            },
            options: options
                .iter()
                .map(|outcome| ApprovalOption {
                    outcome: OutcomeKey::try_from(*outcome).unwrap(),
                    label: outcome.to_uppercase(),
                    style: OptionStyle::Normal,
                })
                .collect(),
            allow_freetext: false,
            mode,
        }),
    }
}

fn terminal(name: &str) -> Node {
    Node {
        id: key(name),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

/// A gate with a timeout at the top level, a gate inside a loop body
/// subgraph, a bootstrap gate, and a plain node an agent stands in for.
fn graph() -> Graph {
    let mut loop_gate = gate(&["keep", "redo"], None, HumanGateMode::Generic);
    loop_gate.id = key(LOOP_GATE);
    let mut boot = gate(
        &[],
        None,
        HumanGateMode::Bootstrap {
            stage: BootstrapStage::Roadmap,
        },
    );
    boot.id = key(BOOT_GATE);
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("pending-decisions", Utc::now()),
        start: key(GATE),
        nodes: BTreeMap::from([
            (
                key(GATE),
                gate(&["approve", "reject"], Some(600), HumanGateMode::Generic),
            ),
            (key(BOOT_GATE), boot),
            (key(AGENT), terminal(AGENT)),
        ]),
        edges: vec![],
        subgraphs: BTreeMap::from([(
            surge_core::keys::SubgraphKey::try_from("body").unwrap(),
            Subgraph {
                start: key(LOOP_GATE),
                nodes: BTreeMap::from([(key(LOOP_GATE), loop_gate)]),
                edges: vec![],
            },
        )]),
    }
}

/// A run log under construction; every payload gets the next sequence.
struct Log {
    run: RunId,
    events: Vec<RunEvent>,
}

impl Log {
    /// `RunStarted` and `PipelineMaterialized`.
    fn started() -> Self {
        let mut log = Self {
            run: RunId::new(),
            events: Vec::new(),
        };
        log.push(EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/tmp"),
            initial_prompt: "build".into(),
            config: RunConfig {
                bootstrap_edit_loop_cap: None,
                budget: Default::default(),
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        });
        log.push(EventPayload::PipelineMaterialized {
            graph: Box::new(graph()),
            graph_hash: ContentHash::compute(b"graph"),
        });
        log
    }

    fn push(&mut self, payload: EventPayload) -> u64 {
        let seq = self.events.len() as u64 + 1;
        self.events.push(RunEvent {
            run_id: self.run,
            seq,
            timestamp: DateTime::from_timestamp(1_760_000_000 + seq as i64, 0).unwrap(),
            payload,
        });
        seq
    }

    fn enter(&mut self, node: &str) -> u64 {
        self.push(EventPayload::StageEntered {
            node: key(node),
            attempt: 1,
        })
    }

    fn request(&mut self, node: &str, call_id: Option<&str>) -> u64 {
        self.request_with(node, call_id, None)
    }

    fn request_with(
        &mut self,
        node: &str,
        call_id: Option<&str>,
        schema: Option<serde_json::Value>,
    ) -> u64 {
        self.push(EventPayload::HumanInputRequested {
            node: key(node),
            session: None,
            call_id: call_id.map(str::to_owned),
            prompt: format!("{node}?"),
            schema,
        })
    }

    fn resolve(&mut self, node: &str, call_id: Option<&str>) -> u64 {
        self.push(EventPayload::HumanInputResolved {
            node: key(node),
            call_id: call_id.map(str::to_owned),
            response: serde_json::json!({"outcome": "approve"}),
        })
    }

    fn escalate(&mut self, cause: EscalationCause) -> u64 {
        self.push(EventPayload::EscalationRequested {
            stage: None,
            reason: format!("{cause:?}"),
            cause,
        })
    }

    fn fold(&self) -> PendingDecisions {
        PendingDecisions::from_events(self.run, &self.events)
    }
}

fn gate_id() -> String {
    surge_core::id::GateRequestId::new().to_string()
}

fn open_seqs(fold: &PendingDecisions) -> Vec<u64> {
    fold.open()
        .map(|decision| decision.id.request_seq)
        .collect()
}

#[test]
fn a_gate_request_opens_one_actionable_decision_with_its_frozen_options_and_deadline() {
    let mut log = Log::started();
    log.enter(GATE);
    let call = gate_id();
    let seq = log.request(GATE, Some(&call));
    let fold = log.fold();
    let decision = fold.open_at(seq).expect("the gate request is pending");
    assert_eq!(decision.kind, DecisionKind::Gate { call_id: call });
    assert_eq!(
        decision
            .options
            .iter()
            .map(|option| option.outcome.as_str())
            .collect::<Vec<_>>(),
        ["approve", "reject"]
    );
    assert_eq!(
        decision.deadline,
        Some(decision.requested_at + chrono::Duration::seconds(600))
    );
    assert!(decision.actionable);
    assert_eq!(fold.current_request().map(|d| d.id.request_seq), Some(seq));
}

#[test]
fn a_gate_inside_a_subgraph_gets_its_options_from_the_frozen_graph_and_no_deadline() {
    let mut log = Log::started();
    log.enter(LOOP_GATE);
    let seq = log.request(LOOP_GATE, Some(&gate_id()));
    let fold = log.fold();
    let decision = fold.open_at(seq).unwrap();
    assert_eq!(
        decision
            .options
            .iter()
            .map(|option| option.outcome.as_str())
            .collect::<Vec<_>>(),
        ["keep", "redo"]
    );
    assert_eq!(decision.deadline, None);
}

#[test]
fn bootstrap_and_skill_approvals_are_classified_with_their_fixed_options() {
    let mut log = Log::started();
    log.enter(BOOT_GATE);
    let marker = log.request_with(
        AGENT,
        Some(&gate_id()),
        Some(serde_json::json!({"x-surge-bootstrap-stage": "flow"})),
    );
    log.resolve(AGENT, None);
    let mode = log.request(BOOT_GATE, Some(&gate_id()));
    let skills = log.request_with(
        AGENT,
        Some(&gate_id()),
        Some(serde_json::json!({"x-surge-skill-bindings": [SkillRef {
            name: "review".into(),
            provider: surge_core::skill::SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }]})),
    );
    let fold = log.fold();
    let outcomes = |seq| {
        fold.open_at(seq)
            .or_else(|| {
                fold.retired()
                    .find(|(d, _)| d.id.request_seq == seq)
                    .map(|(d, _)| d)
            })
            .unwrap()
            .options
            .iter()
            .map(|option| option.outcome.clone())
            .collect::<Vec<_>>()
    };
    assert!(matches!(
        fold.open_at(marker).map(|d| &d.kind),
        Some(DecisionKind::BootstrapApproval {
            stage: BootstrapStage::Flow,
            ..
        })
    ));
    assert!(matches!(
        fold.open_at(mode).map(|d| &d.kind),
        Some(DecisionKind::BootstrapApproval {
            stage: BootstrapStage::Roadmap,
            ..
        })
    ));
    assert_eq!(outcomes(mode), ["approve", "edit", "reject"]);
    let Some(DecisionKind::SkillApproval { skills: bound, .. }) =
        fold.open_at(skills).map(|d| &d.kind)
    else {
        panic!("a skill-binding request is a skill approval");
    };
    assert_eq!(bound.len(), 1);
    assert_eq!(outcomes(skills), ["approve", "reject"]);
}

#[test]
fn companion_events_never_open_a_second_decision() {
    let mut log = Log::started();
    log.enter(BOOT_GATE);
    log.push(EventPayload::BootstrapApprovalRequested {
        stage: BootstrapStage::Roadmap,
        channel: ApprovalChannel::Desktop {
            duration: ApprovalDuration::Persistent,
        },
    });
    log.push(EventPayload::ApprovalRequested {
        gate: key(BOOT_GATE),
        channel: ApprovalChannel::Desktop {
            duration: ApprovalDuration::Persistent,
        },
        payload_hash: ContentHash::compute(b"payload"),
    });
    let seq = log.request(BOOT_GATE, Some(&gate_id()));
    log.push(EventPayload::ApprovalDecided {
        gate: key(BOOT_GATE),
        decision: "approve".into(),
        channel_used: surge_core::approvals::ApprovalChannelKind::Desktop,
        comment: None,
    });
    assert_eq!(open_seqs(&log.fold()), [seq]);
}

#[test]
fn resolving_with_a_call_id_retires_only_that_request() {
    let mut log = Log::started();
    log.enter(GATE);
    let first = log.request(GATE, Some("session-a:tool-1"));
    let second = log.request(GATE, Some("session-a:tool-2"));
    let unbound = log.request(GATE, None);
    log.resolve(GATE, Some("session-a:tool-1"));
    let fold = log.fold();
    assert_eq!(open_seqs(&fold), [second, unbound]);
    assert_eq!(fold.retirement_of(first), Some(Retirement::Answered));
}

#[test]
fn resolving_without_a_call_id_matches_by_node_and_never_a_bound_sibling() {
    let mut log = Log::started();
    log.enter(GATE);
    let bound = log.request(GATE, Some("session-a:tool-1"));
    let unbound = log.request(GATE, None);
    let elsewhere = log.request(LOOP_GATE, None);
    log.resolve(GATE, None);
    let fold = log.fold();
    assert_eq!(open_seqs(&fold), [bound, elsewhere]);
    assert_eq!(fold.retirement_of(unbound), Some(Retirement::Answered));
}

#[test]
fn a_timed_out_request_retires_as_timed_out() {
    let mut log = Log::started();
    log.enter(GATE);
    let call = gate_id();
    let seq = log.request(GATE, Some(&call));
    let change = {
        let mut fold = log.fold();
        let timeout = log.push(EventPayload::HumanInputTimedOut {
            node: key(GATE),
            call_id: Some(call),
            elapsed_seconds: 600,
        });
        fold.feed(&log.events[usize::try_from(timeout - 1).unwrap()])
    };
    assert_eq!(
        change,
        Fed::Applied(vec![DecisionChange::Retired {
            id: DecisionId {
                run: log.run,
                request_seq: seq
            },
            reason: Retirement::TimedOut,
        }])
    );
    assert_eq!(open_seqs(&log.fold()), Vec::<u64>::new());
}

#[test]
fn elevations_retire_when_decided_or_timed_out_on_their_own_node() {
    let mut log = Log::started();
    let request = |log: &mut Log, node: &str| {
        log.push(EventPayload::SandboxElevationRequested {
            node: key(node),
            capability: "network".into(),
        })
    };
    let decided = request(&mut log, AGENT);
    let timed_out = request(&mut log, GATE);
    log.push(EventPayload::SandboxElevationDecided {
        node: key(LOOP_GATE),
        decision: surge_core::run_event::ElevationDecision::Allow,
        remember: false,
    });
    assert_eq!(open_seqs(&log.fold()), [decided, timed_out]);
    log.push(EventPayload::SandboxElevationDecided {
        node: key(AGENT),
        decision: surge_core::run_event::ElevationDecision::Allow,
        remember: false,
    });
    log.push(EventPayload::SandboxElevationTimedOut {
        node: key(GATE),
        capability: "network".into(),
        elapsed_seconds: 30,
    });
    let fold = log.fold();
    assert_eq!(open_seqs(&fold), Vec::<u64>::new());
    assert_eq!(fold.retirement_of(decided), Some(Retirement::Decided));
    assert_eq!(fold.retirement_of(timed_out), Some(Retirement::TimedOut));
    assert!(matches!(
        &fold.retired().next().unwrap().0.kind,
        DecisionKind::Elevation { capability } if capability == "network"
    ));
}

#[test]
fn a_roadmap_patch_decision_retires_only_its_patch() {
    use surge_core::roadmap_patch::{RoadmapPatchApprovalDecision, RoadmapPatchId};
    let target = RoadmapPatchTarget::ProjectRoadmap {
        roadmap_path: ".ai-factory/ROADMAP.md".into(),
    };
    let mut log = Log::started();
    let request = |log: &mut Log, id: &str| {
        log.push(EventPayload::RoadmapPatchApprovalRequested {
            patch_id: RoadmapPatchId::new(id).unwrap(),
            target: target.clone(),
            channel: ApprovalChannel::Desktop {
                duration: ApprovalDuration::Persistent,
            },
            summary_hash: ContentHash::compute(id.as_bytes()),
        })
    };
    let first = request(&mut log, "rpatch-a");
    let second = request(&mut log, "rpatch-b");
    log.push(EventPayload::RoadmapPatchApprovalDecided {
        patch_id: RoadmapPatchId::new("rpatch-a").unwrap(),
        decision: RoadmapPatchApprovalDecision::Approve,
        channel_used: surge_core::approvals::ApprovalChannelKind::Desktop,
        comment: None,
        conflict_choice: None,
    });
    let fold = log.fold();
    assert_eq!(open_seqs(&fold), [second]);
    assert_eq!(fold.retirement_of(first), Some(Retirement::Decided));
    assert!(matches!(
        &fold.open_at(second).unwrap().kind,
        DecisionKind::RoadmapPatch { patch_id, .. } if patch_id == "rpatch-b"
    ));
}

#[test]
fn every_final_outcome_retires_everything_and_suspension_or_parking_retire_nothing() {
    let finals = [
        EventPayload::RunCompleted {
            terminal_node: key(AGENT),
        },
        EventPayload::RunFailed {
            error: "boom".into(),
        },
        EventPayload::RunAborted {
            reason: "stop".into(),
        },
    ];
    for final_outcome in finals {
        let mut log = Log::started();
        log.enter(GATE);
        let request = log.request(GATE, Some(&gate_id()));
        let escalation = log.escalate(EscalationCause::Unspecified);
        log.push(EventPayload::RunParked {
            wake_at: Utc::now(),
            runtime: None,
            worktree: PathBuf::from("/tmp"),
            basis: surge_core::capacity::WakeBasis::ObservedReset,
            reason: "quota".into(),
        });
        let parked = log.fold();
        assert_eq!(open_seqs(&parked), [request, escalation]);
        assert!(
            !parked.open_at(request).unwrap().actionable,
            "a request in a parked run is not actionable"
        );
        log.push(final_outcome);
        let fold = log.fold();
        assert_eq!(open_seqs(&fold), Vec::<u64>::new());
        assert_eq!(fold.retirement_of(request), Some(Retirement::FinalOutcome));
        assert_eq!(
            fold.retirement_of(escalation),
            Some(Retirement::FinalOutcome)
        );
    }
}

#[test]
fn an_escalation_survives_a_suspension() {
    let mut log = Log::started();
    let escalation = log.escalate(EscalationCause::Unspecified);
    let snapshot_seq = log.events.len() as u64;
    log.push(EventPayload::RunSuspended {
        fence: surge_core::execution_recovery::SuspensionFence {
            control_generation: 1,
            snapshot_seq,
            pending_stage: surge_core::execution_recovery::PendingStagePhase::BetweenStages,
            cleanup_confirmed: true,
            reason: surge_core::execution_recovery::SuspensionReason::Manual,
        },
    });
    assert_eq!(open_seqs(&log.fold()), [escalation]);
}

#[test]
fn each_escalation_cause_retires_on_its_own_signal() {
    let completed = |log: &mut Log, node: &str| {
        log.push(EventPayload::StageCompleted {
            node: key(node),
            outcome: OutcomeKey::try_from("done").unwrap(),
        })
    };

    // Capacity blind-park limit: the next StageCompleted anywhere.
    let mut log = Log::started();
    log.enter(AGENT);
    let blind = log.escalate(EscalationCause::CapacityBlindParkLimitExceeded);
    completed(&mut log, GATE);
    assert_eq!(
        log.fold().retirement_of(blind),
        Some(Retirement::CauseCleared)
    );

    // MCP restarts exhausted: the next RunContinued, not a StageCompleted.
    let mut log = Log::started();
    log.enter(AGENT);
    let mcp = log.escalate(EscalationCause::McpRestartsExhausted);
    completed(&mut log, AGENT);
    assert_eq!(open_seqs(&log.fold()), [mcp]);
    log.push(EventPayload::RunContinued {
        control_generation: 1,
    });
    assert_eq!(
        log.fold().retirement_of(mcp),
        Some(Retirement::CauseCleared)
    );

    // Loop guards and an unavailable catalog: a StageCompleted on the
    // attributed node only.
    for cause in [
        EscalationCause::LoopGuardRepeatedToolCall,
        EscalationCause::LoopGuardNodeDeadline,
        EscalationCause::LoopGuardNoProgress,
        EscalationCause::LoopGuardToolCallCap,
        EscalationCause::McpSelectedCatalogUnavailable,
    ] {
        let mut log = Log::started();
        log.enter(AGENT);
        let escalation = log.escalate(cause);
        log.enter(GATE);
        completed(&mut log, GATE);
        let fold = log.fold();
        assert_eq!(open_seqs(&fold), [escalation], "{cause:?}");
        assert_eq!(fold.open_at(escalation).unwrap().node, Some(key(AGENT)));
        completed(&mut log, AGENT);
        assert_eq!(
            log.fold().retirement_of(escalation),
            Some(Retirement::CauseCleared),
            "{cause:?}"
        );
    }
}

#[test]
fn an_unknown_cause_survives_everything_except_a_final_outcome() {
    for cause in [
        EscalationCause::Unspecified,
        EscalationCause::BootstrapEditLoopExhausted,
        EscalationCause::RoadmapAmendmentEditLoopExhausted,
    ] {
        let mut log = Log::started();
        log.enter(AGENT);
        let escalation = log.escalate(cause);
        log.push(EventPayload::StageCompleted {
            node: key(AGENT),
            outcome: OutcomeKey::try_from("done").unwrap(),
        });
        log.push(EventPayload::RunContinued {
            control_generation: 1,
        });
        let fold = log.fold();
        assert_eq!(open_seqs(&fold), [escalation], "{cause:?}");
        assert!(!fold.open_at(escalation).unwrap().actionable);
    }
}

#[test]
fn legacy_unbound_and_cursor_left_requests_are_listed_but_not_actionable() {
    let mut log = Log::started();
    log.enter(GATE);
    let legacy = log.request(GATE, None);
    let fold = log.fold();
    let decision = fold.open_at(legacy).unwrap();
    assert_eq!(decision.kind, DecisionKind::LegacyUnbound);
    assert!(!decision.actionable);

    let mut log = Log::started();
    log.enter(GATE);
    let request = log.request(GATE, Some(&gate_id()));
    log.enter(AGENT);
    let fold = log.fold();
    assert!(!fold.open_at(request).unwrap().actionable);
}

#[test]
fn replay_and_overlap_are_no_ops_and_a_gap_is_reported() {
    let mut log = Log::started();
    log.enter(GATE);
    log.request(GATE, Some(&gate_id()));
    log.escalate(EscalationCause::Unspecified);
    let once = log.fold();

    let mut twice = log.fold();
    for event in &log.events {
        assert_eq!(twice.feed(event), Fed::Duplicate);
    }
    assert_eq!(open_seqs(&twice), open_seqs(&once));

    let mut overlapping = PendingDecisions::from_events(log.run, &log.events[..3]);
    for event in &log.events[1..] {
        let _ = overlapping.feed(event);
    }
    assert_eq!(open_seqs(&overlapping), open_seqs(&once));
    assert_eq!(overlapping.status(), &FoldStatus::Complete);

    let mut gapped = PendingDecisions::from_events(log.run, &log.events[..2]);
    assert_eq!(gapped.feed(&log.events[3]), Fed::Incomplete);
    assert_eq!(
        gapped.status(),
        &FoldStatus::Incomplete(Incomplete::Gap {
            expected: 3,
            found: 4
        })
    );
    assert_eq!(gapped.feed(&log.events[2]), Fed::Incomplete);
    assert_eq!(gapped.open().count(), 0, "an incomplete fold lists nothing");
}

#[test]
fn a_tool_call_request_is_actionable_free_form_input() {
    let mut log = Log::started();
    log.enter(AGENT);
    let seq = log.request(AGENT, Some("session-a:tool-1"));
    let fold = log.fold();
    let decision = fold.open_at(seq).unwrap();
    assert_eq!(
        decision.kind,
        DecisionKind::ToolCallInput {
            call_id: "session-a:tool-1".into()
        }
    );
    assert!(decision.options.is_empty());
    assert!(decision.actionable);
}

#[test]
fn a_single_event_classifies_without_history_as_the_fold_does() {
    let marker = classify_opening(
        &EventPayload::HumanInputRequested {
            node: key(AGENT),
            session: None,
            call_id: Some(gate_id()),
            prompt: "approve?".into(),
            schema: Some(serde_json::json!({"x-surge-bootstrap-stage": "description"})),
        },
        None,
    )
    .unwrap();
    assert!(matches!(
        marker.kind,
        DecisionKind::BootstrapApproval {
            stage: BootstrapStage::Description,
            ..
        }
    ));
    let escalation = classify_opening(
        &EventPayload::EscalationRequested {
            stage: Some(BootstrapStage::Flow),
            reason: "edit loop exhausted".into(),
            cause: EscalationCause::BootstrapEditLoopExhausted,
        },
        None,
    )
    .unwrap();
    assert_eq!(escalation.prompt, "edit loop exhausted");
    let companion = EventPayload::BootstrapApprovalRequested {
        stage: BootstrapStage::Flow,
        channel: ApprovalChannel::Desktop {
            duration: ApprovalDuration::Persistent,
        },
    };
    assert_eq!(classify_opening(&companion, None), None);
}
