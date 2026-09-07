//! Unit tests for `run_report::compile` — pure fold over event fixtures,
//! no DB (the acceptance criterion this ticket shipped against). Split from
//! `mod.rs` per the file-size guidance in `rules/core.md`: a `#[cfg(test)]`
//! module that dominates its file belongs in its own `#[path]`-split module.

use super::*;
use crate::approvals::{ApprovalDuration, ApprovalPolicy};
use crate::budget::BudgetGuard;
use crate::graph::{GraphMetadata, SCHEMA_VERSION};
use crate::node::{Node, NodeConfig, Position};
use crate::run_event::RunConfig;
use crate::sandbox::SandboxMode;
use crate::terminal_config::{TerminalConfig, TerminalKind};
use std::collections::BTreeMap;

fn node_key(name: &str) -> NodeKey {
    NodeKey::try_from(name).unwrap()
}

fn outcome_key(name: &str) -> OutcomeKey {
    OutcomeKey::try_from(name).unwrap()
}

fn event(seq: u64, payload: EventPayload) -> RunEvent {
    RunEvent {
        run_id: RunId::new(),
        seq,
        timestamp: chrono::Utc::now(),
        payload,
    }
}

/// A minimal one-node graph whose sole node declares `LedgerEffect::Verified`
/// — used to exercise the `TaskVerified` authority check.
fn verifier_graph(verifier_node: &str) -> Graph {
    let node = node_key(verifier_node);
    let mut nodes = BTreeMap::new();
    nodes.insert(
        node.clone(),
        Node {
            id: node.clone(),
            position: Position::default(),
            declared_outcomes: vec![crate::node::OutcomeDecl {
                id: outcome_key("verified"),
                description: String::new(),
                edge_kind_hint: crate::edge::EdgeKind::Forward,
                is_terminal: true,
                ledger_effect: crate::node::LedgerEffect::Verified,
            }],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "verifier-graph".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: node,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

/// A graph whose sole node does NOT declare verification authority.
fn non_verifier_graph(node_name: &str) -> Graph {
    let node = node_key(node_name);
    let mut nodes = BTreeMap::new();
    nodes.insert(
        node.clone(),
        Node {
            id: node.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "non-verifier-graph".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: node,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

fn run_started(seq: u64) -> RunEvent {
    event(
        seq,
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/project"),
            initial_prompt: "build it".into(),
            config: RunConfig {
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
                budget: BudgetGuard::default(),
            },
        },
    )
}

#[test]
fn empty_event_slice_compiles_to_an_incomplete_empty_report() {
    let run_id = RunId::new();
    let report = RunReport::compile(run_id, &[]);

    assert_eq!(report.run_id, run_id);
    assert_eq!(report.completion, RunCompletion::Incomplete);
    assert!(!report.completion.is_terminal());
    assert!(report.nodes.is_empty());
    assert!(report.outcomes.is_empty());
    assert!(report.verdicts.is_empty());
    assert!(report.evidence.is_empty());
    assert!(report.skills.is_empty());
    assert!(report.memory_receipts.is_empty());
    assert!(report.steers.is_empty());
    assert!(report.approvals.is_empty());
    assert_eq!(report.cost, CostTotals::default());
}

/// R27.1: a run whose log stops mid-flight (crashed, still running)
/// still compiles — no terminal lifecycle event means `Incomplete`, not
/// an error and not a guess at some other status.
#[test]
fn a_torn_run_with_no_terminal_event_compiles_as_incomplete() {
    let run_id = RunId::new();
    let events = vec![
        run_started(1),
        event(
            2,
            EventPayload::StageEntered {
                node: node_key("impl_1"),
                attempt: 1,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.completion, RunCompletion::Incomplete);
    assert_eq!(report.nodes.len(), 1);
    assert_eq!(report.nodes[0].status, NodeStatus::InProgress);
}

#[test]
fn run_completed_is_reflected_in_completion() {
    let run_id = RunId::new();
    let events = vec![
        run_started(1),
        event(
            2,
            EventPayload::RunCompleted {
                terminal_node: node_key("end"),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(
        report.completion,
        RunCompletion::Completed {
            terminal_node: node_key("end")
        }
    );
    assert!(report.completion.is_terminal());
}

#[test]
fn run_failed_and_aborted_are_distinguished() {
    let run_id = RunId::new();
    let failed = RunReport::compile(
        run_id,
        &[event(
            1,
            EventPayload::RunFailed {
                error: "boom".into(),
            },
        )],
    );
    assert_eq!(
        failed.completion,
        RunCompletion::Failed {
            error: "boom".into()
        }
    );

    let aborted = RunReport::compile(
        run_id,
        &[event(
            1,
            EventPayload::RunAborted {
                reason: "operator cancelled".into(),
            },
        )],
    );
    assert_eq!(
        aborted.completion,
        RunCompletion::Aborted {
            reason: "operator cancelled".into()
        }
    );
}

#[test]
fn node_tracks_attempts_and_last_status_across_a_retry() {
    let run_id = RunId::new();
    let node = node_key("impl_1");
    let events = vec![
        event(
            1,
            EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        ),
        event(
            2,
            EventPayload::StageFailed {
                node: node.clone(),
                reason: "compile error".into(),
                retry_available: true,
            },
        ),
        event(
            3,
            EventPayload::StageEntered {
                node: node.clone(),
                attempt: 2,
            },
        ),
        event(
            4,
            EventPayload::StageCompleted {
                node: node.clone(),
                outcome: outcome_key("done"),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.nodes.len(), 1, "one node, not one row per attempt");
    assert_eq!(report.nodes[0].attempts, 2);
    assert_eq!(
        report.nodes[0].status,
        NodeStatus::Completed {
            outcome: outcome_key("done")
        }
    );
}

#[test]
fn nodes_stay_in_first_seen_execution_order() {
    let run_id = RunId::new();
    let events = vec![
        event(
            1,
            EventPayload::StageEntered {
                node: node_key("b_node"),
                attempt: 1,
            },
        ),
        event(
            2,
            EventPayload::StageEntered {
                node: node_key("a_node"),
                attempt: 1,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    let order: Vec<&str> = report.nodes.iter().map(|n| n.node.as_str()).collect();
    assert_eq!(
        order,
        vec!["b_node", "a_node"],
        "must stay execution-ordered, not sorted alphabetically"
    );
}

#[test]
fn outcome_reported_appends_an_outcome_entry() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::OutcomeReported {
            node: node_key("impl_1"),
            outcome: outcome_key("approve"),
            summary: "looks good".into(),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].node, node_key("impl_1"));
    assert_eq!(report.outcomes[0].outcome, outcome_key("approve"));
    assert_eq!(report.outcomes[0].summary, "looks good");
}

#[test]
fn task_verified_by_an_authorized_node_is_verified() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
    let evidence_hash = ContentHash::compute(b"verification-report");
    let events = vec![
        event(
            1,
            EventPayload::PipelineMaterialized {
                graph: Box::new(verifier_graph("verify_1")),
                graph_hash: ContentHash::compute(b"graph"),
            },
        ),
        event(
            2,
            EventPayload::TaskVerified {
                task_id: "t1".into(),
                node: verifier.clone(),
                evidence: evidence_hash,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(report.verdicts[0].task_id, "t1");
    assert_eq!(
        report.verdicts[0].result,
        VerdictResult::Verified {
            evidence: evidence_hash
        }
    );
}

/// The load-bearing verdict-correctness case: a `TaskVerified` event
/// exists in the log (nothing stops an event from being written), but
/// the active graph never granted the reporting node verification
/// authority. R33 needs this surfaced, not silently accepted as a
/// legitimate `Verified` the way a naive "just read `TaskVerified`"
/// projection would.
#[test]
fn task_verified_by_an_unauthorized_node_is_flagged_not_verified() {
    let run_id = RunId::new();
    let impostor = node_key("impl_1");
    let events = vec![
        event(
            1,
            EventPayload::PipelineMaterialized {
                graph: Box::new(non_verifier_graph("impl_1")),
                graph_hash: ContentHash::compute(b"graph"),
            },
        ),
        event(
            2,
            EventPayload::TaskVerified {
                task_id: "t1".into(),
                node: impostor,
                evidence: ContentHash::compute(b"forged"),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(report.verdicts[0].result, VerdictResult::Unauthorized);
}

#[test]
fn task_verified_with_no_graph_at_all_is_unauthorized_not_verified() {
    // No `PipelineMaterialized` ever appeared before the `TaskVerified` —
    // `compile` must not default-trust an unknown graph.
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::TaskVerified {
            task_id: "t1".into(),
            node: node_key("verify_1"),
            evidence: ContentHash::compute(b"evidence"),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.verdicts[0].result, VerdictResult::Unauthorized);
}

#[test]
fn failed_verification_status_change_is_a_rejected_verdict() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::TaskStatusChanged {
            task_id: "t1".into(),
            from: RoadmapStatus::ReadyForVerification,
            to: RoadmapStatus::FailedVerification,
            authority_node: node_key("verify_1"),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(report.verdicts[0].result, VerdictResult::Rejected);
}

#[test]
fn artifact_produced_and_bootstrap_artifact_are_both_evidence() {
    let run_id = RunId::new();
    let node_hash = ContentHash::compute(b"spec");
    let bootstrap_hash = ContentHash::compute(b"description");
    let events = vec![
        event(
            1,
            EventPayload::ArtifactProduced {
                node: node_key("spec_1"),
                artifact: node_hash,
                path: PathBuf::from("artifacts/spec.md"),
                name: "spec.md".into(),
            },
        ),
        event(
            2,
            EventPayload::BootstrapArtifactProduced {
                stage: BootstrapStage::Description,
                artifact: bootstrap_hash,
                name: "description.md".into(),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.evidence.len(), 2);
    assert_eq!(
        report.evidence[0].origin,
        EvidenceOrigin::Node {
            node: node_key("spec_1")
        }
    );
    assert_eq!(
        report.evidence[0].path,
        Some(PathBuf::from("artifacts/spec.md"))
    );
    assert_eq!(
        report.evidence[1].origin,
        EvidenceOrigin::Bootstrap {
            stage: BootstrapStage::Description
        }
    );
    assert_eq!(report.evidence[1].path, None);
}

#[test]
fn cost_sums_tokens_and_dollars_across_multiple_events() {
    let run_id = RunId::new();
    let events = vec![
        event(
            1,
            EventPayload::TokensConsumed {
                session: crate::id::SessionId::new(),
                prompt_tokens: 1_000,
                output_tokens: 500,
                cache_hits: 100,
                model: "claude-opus-4-7".into(),
                cost_usd: Some(0.02),
            },
        ),
        event(
            2,
            EventPayload::TokensConsumed {
                session: crate::id::SessionId::new(),
                prompt_tokens: 200,
                output_tokens: 50,
                cache_hits: 0,
                model: "claude-opus-4-7".into(),
                cost_usd: None,
            },
        ),
        event(
            3,
            EventPayload::BudgetWarningRaised {
                dimension: crate::budget::BudgetDimension::Usd,
                pct: 80,
                cost_usd: 0.02,
                total_tokens: 1_500,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.cost.prompt_tokens, 1_200);
    assert_eq!(report.cost.output_tokens, 550);
    assert_eq!(report.cost.cache_hits, 100);
    assert!((report.cost.cost_usd - 0.02).abs() < f64::EPSILON);
    assert!(report.cost.budget_warning_raised);
    assert!(!report.cost.budget_exceeded);
}

/// R14 — the report's `skills` section must reconstruct every bound
/// skill from `SkillBound` alone, with no dependency on `surge.toml` or
/// a node's own declaration.
#[test]
fn skill_bound_events_are_listed_verbatim() {
    let run_id = RunId::new();
    let hash = ContentHash::compute(b"skill-pack-content");
    let events = vec![event(
        1,
        EventPayload::SkillBound {
            node: node_key("implement"),
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            hash,
            gate_enabled: true,
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.skills.len(), 1);
    assert_eq!(report.skills[0].name, "code-reviewer");
    assert_eq!(report.skills[0].provider, SkillProvider::ProjectDir);
    assert_eq!(report.skills[0].hash, hash);
    assert!(report.skills[0].gate_enabled);
}

#[test]
fn steer_delivered_appends_a_steer_entry() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::SteerDelivered {
            id: "steer-1".into(),
            node: node_key("impl_1"),
            message: "focus on the edge cases".into(),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.steers.len(), 1);
    assert_eq!(report.steers[0].id, "steer-1");
    assert_eq!(report.steers[0].message, "focus on the edge cases");
}

#[test]
fn human_input_request_resolve_and_timeout_all_appear_as_approvals() {
    let run_id = RunId::new();
    let events = vec![
        event(
            1,
            EventPayload::HumanInputRequested {
                node: node_key("gate_1"),
                session: None,
                call_id: None,
                prompt: "approve?".into(),
                schema: None,
            },
        ),
        event(
            2,
            EventPayload::HumanInputResolved {
                node: node_key("gate_1"),
                call_id: None,
                response: serde_json::json!({"decision": "approve"}),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.approvals.len(), 2);
    assert!(matches!(
        report.approvals[0],
        ApprovalEntry::HumanInputRequested { .. }
    ));
    assert!(matches!(
        report.approvals[1],
        ApprovalEntry::HumanInputResolved { .. }
    ));
}

#[test]
fn sandbox_elevation_lifecycle_appears_as_approvals() {
    let run_id = RunId::new();
    let events = vec![
        event(
            1,
            EventPayload::SandboxElevationRequested {
                node: node_key("impl_1"),
                capability: "network: api.example.com".into(),
            },
        ),
        event(
            2,
            EventPayload::SandboxElevationDecided {
                node: node_key("impl_1"),
                decision: ElevationDecision::AllowAndRemember,
                remember: true,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.approvals.len(), 2);
    assert!(matches!(
        report.approvals[1],
        ApprovalEntry::SandboxElevationDecided {
            decision: ElevationDecision::AllowAndRemember,
            remember: true,
            ..
        }
    ));
}

#[test]
fn bootstrap_and_roadmap_patch_approvals_appear() {
    let run_id = RunId::new();
    let patch_id = RoadmapPatchId::new("rpatch-1").unwrap();
    let events = vec![
        event(
            1,
            EventPayload::BootstrapApprovalDecided {
                stage: BootstrapStage::Roadmap,
                decision: BootstrapDecision::Approve,
                comment: None,
            },
        ),
        event(
            2,
            EventPayload::RoadmapPatchApprovalDecided {
                patch_id: patch_id.clone(),
                decision: RoadmapPatchApprovalDecision::Approve,
                channel_used: crate::approvals::ApprovalChannelKind::Desktop,
                comment: None,
                conflict_choice: None,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.approvals.len(), 2);
    assert!(matches!(
        report.approvals[0],
        ApprovalEntry::BootstrapApprovalDecided {
            decision: BootstrapDecision::Approve,
            ..
        }
    ));
    assert!(matches!(
        &report.approvals[1],
        ApprovalEntry::RoadmapPatchApprovalDecided { patch_id: p, decision: RoadmapPatchApprovalDecision::Approve }
        if *p == patch_id
    ));
}

/// `ApprovalRequested`/`ApprovalDecided` are a documented dead primitive
/// (ADR-0015) with no production emitter — `compile` must not surface
/// them even if a stray one exists in an old log.
#[test]
fn legacy_approval_requested_decided_events_are_ignored() {
    let run_id = RunId::new();
    let events = vec![
        event(
            1,
            EventPayload::ApprovalRequested {
                gate: node_key("gate_1"),
                channel: crate::approvals::ApprovalChannel::Desktop {
                    duration: ApprovalDuration::Transient,
                },
                payload_hash: ContentHash::compute(b"payload"),
            },
        ),
        event(
            2,
            EventPayload::ApprovalDecided {
                gate: node_key("gate_1"),
                decision: "approve".into(),
                channel_used: crate::approvals::ApprovalChannelKind::Desktop,
                comment: None,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert!(
        report.approvals.is_empty(),
        "the dead ApprovalRequested/Decided pair must not appear in the report"
    );
}

#[test]
fn memory_receipts_is_always_empty_today_no_event_carries_it_yet() {
    // Named limitation (see module doc): no `EventPayload` variant
    // carries a `PackReceipt` yet, so this is the correct, honest
    // output — not a bug in this compiler.
    let run_id = RunId::new();
    let report = RunReport::compile(run_id, &[run_started(1)]);
    assert!(report.memory_receipts.is_empty());
}

/// Covers eight of the nine named sections plus header/escalations —
/// `memory_receipts` is the sole section this fixture leaves empty,
/// and that is asserted explicitly (not left to a stray missing
/// assertion) because no event carries a `PackReceipt` yet (see
/// `memory_receipts_is_always_empty_today_no_event_carries_it_yet`).
#[test]
fn a_full_run_covers_eight_of_nine_sections_end_to_end() {
    let run_id = RunId::new();
    let node = node_key("implement");
    let events = vec![
        run_started(1),
        event(
            2,
            EventPayload::PipelineMaterialized {
                graph: Box::new(verifier_graph("implement")),
                graph_hash: ContentHash::compute(b"graph"),
            },
        ),
        event(
            3,
            EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        ),
        event(
            4,
            EventPayload::SkillBound {
                node: node.clone(),
                name: "archify".into(),
                provider: SkillProvider::ProjectDir,
                hash: ContentHash::compute(b"archify-pack"),
                gate_enabled: true,
            },
        ),
        event(
            5,
            EventPayload::ArtifactProduced {
                node: node.clone(),
                artifact: ContentHash::compute(b"diff"),
                path: PathBuf::from("artifacts/diff.patch"),
                name: "diff.patch".into(),
            },
        ),
        event(
            6,
            EventPayload::TokensConsumed {
                session: crate::id::SessionId::new(),
                prompt_tokens: 500,
                output_tokens: 300,
                cache_hits: 0,
                model: "claude-opus-4-7".into(),
                cost_usd: Some(0.01),
            },
        ),
        event(
            7,
            EventPayload::OutcomeReported {
                node: node.clone(),
                outcome: outcome_key("done"),
                summary: "implemented".into(),
            },
        ),
        event(
            8,
            EventPayload::TaskVerified {
                task_id: "t1".into(),
                node: node.clone(),
                evidence: ContentHash::compute(b"verification"),
            },
        ),
        event(
            9,
            EventPayload::StageCompleted {
                node: node.clone(),
                outcome: outcome_key("done"),
            },
        ),
        event(
            10,
            EventPayload::SteerDelivered {
                id: "steer-1".into(),
                node: node.clone(),
                message: "keep going".into(),
            },
        ),
        event(
            11,
            EventPayload::SandboxElevationRequested {
                node: node.clone(),
                capability: "network: api.example.com".into(),
            },
        ),
        event(
            12,
            EventPayload::RunCompleted {
                terminal_node: node.clone(),
            },
        ),
    ];

    let report = RunReport::compile(run_id, &events);
    assert!(report.completion.is_terminal());
    assert_eq!(report.header.initial_prompt.as_deref(), Some("build it"));
    assert!(report.header.first_event_at.is_some());
    assert!(report.header.last_event_at.is_some());
    assert!(report.escalations.is_empty());
    assert_eq!(report.nodes.len(), 1);
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(report.evidence.len(), 1);
    assert_eq!(report.skills.len(), 1);
    assert_eq!(report.steers.len(), 1);
    assert_eq!(
        report.approvals.len(),
        1,
        "SandboxElevationRequested must appear in approvals"
    );
    assert!(report.cost.prompt_tokens > 0);
    // The one section this fixture deliberately leaves empty — see the
    // module doc's "named limitation" and the dedicated test above.
    assert!(report.memory_receipts.is_empty());
}

// ── R33's "why did it stop" — header + escalations ──────────────────

#[test]
fn header_captures_initial_prompt_and_first_last_event_timestamps() {
    let run_id = RunId::new();
    let events = vec![
        run_started(1),
        event(
            2,
            EventPayload::StageEntered {
                node: node_key("impl_1"),
                attempt: 1,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.header.initial_prompt.as_deref(), Some("build it"));
    assert!(report.header.first_event_at.is_some());
    assert!(report.header.last_event_at.is_some());
    assert!(report.header.first_event_at <= report.header.last_event_at);
}

#[test]
fn header_initial_prompt_is_none_when_run_started_is_missing() {
    // A torn log whose read window starts after `RunStarted` — the
    // header must say so honestly rather than fabricating a prompt.
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::StageEntered {
            node: node_key("impl_1"),
            attempt: 1,
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.header.initial_prompt, None);
    assert!(report.header.first_event_at.is_some());
}

#[test]
fn escalation_requested_is_recorded_with_its_typed_cause() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::EscalationRequested {
            stage: None,
            reason: "node loop guard tripped".into(),
            cause: crate::run_event::EscalationCause::LoopGuardNodeDeadline,
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.escalations.len(), 1);
    assert_eq!(
        report.escalations[0].cause,
        crate::run_event::EscalationCause::LoopGuardNodeDeadline
    );
    assert_eq!(report.escalations[0].reason, "node loop guard tripped");
}

/// The scenario the review round named explicitly: a run stopped by a
/// guard must not report as a bare, reasonless `Incomplete` — the
/// escalation is visible in its own section even though `completion`
/// itself has no terminal event to report.
#[test]
fn a_guard_stopped_run_is_incomplete_but_names_why() {
    let run_id = RunId::new();
    let events = vec![
        run_started(1),
        event(
            2,
            EventPayload::EscalationRequested {
                stage: None,
                reason: "repeated identical tool call".into(),
                cause: crate::run_event::EscalationCause::LoopGuardRepeatedToolCall,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.completion, RunCompletion::Incomplete);
    assert_eq!(report.escalations.len(), 1);
    assert_eq!(
        report.escalations[0].cause,
        crate::run_event::EscalationCause::LoopGuardRepeatedToolCall
    );
}

// ── RunParked / RunWokeFromPark ──────────────────────────────────────

#[test]
fn run_parked_is_its_own_completion_not_bare_incomplete() {
    let run_id = RunId::new();
    let wake_at = chrono::Utc::now();
    let events = vec![event(
        1,
        EventPayload::RunParked {
            wake_at,
            runtime: Some("claude-acp".into()),
            worktree: PathBuf::from("/wt"),
            basis: crate::capacity::WakeBasis::ObservedReset,
            reason: "rate limited".into(),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert!(!report.completion.is_terminal());
    assert!(matches!(report.completion, RunCompletion::Parked { .. }));
    assert_ne!(
        report.completion,
        RunCompletion::Incomplete,
        "a parked run proves *why* it paused; collapsing it into bare \
         Incomplete throws that fact away"
    );
}

#[test]
fn run_woke_from_park_reverts_to_incomplete_pending_a_real_terminal_event() {
    let run_id = RunId::new();
    let events = vec![
        event(
            1,
            EventPayload::RunParked {
                wake_at: chrono::Utc::now(),
                runtime: Some("claude-acp".into()),
                worktree: PathBuf::from("/wt"),
                basis: crate::capacity::WakeBasis::PolicyBackoff,
                reason: "blind backoff".into(),
            },
        ),
        event(2, EventPayload::RunWokeFromPark {}),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.completion, RunCompletion::Incomplete);
}

// ── OutcomeRejectedByHook ─────────────────────────────────────────────

#[test]
fn outcome_rejected_by_hook_flips_the_matching_entry_not_a_new_one() {
    let run_id = RunId::new();
    let node = node_key("impl_1");
    let outcome = outcome_key("done");
    let events = vec![
        event(
            1,
            EventPayload::OutcomeReported {
                node: node.clone(),
                outcome: outcome.clone(),
                summary: "looks done".into(),
            },
        ),
        event(
            2,
            EventPayload::OutcomeRejectedByHook {
                node: node.clone(),
                outcome: outcome.clone(),
                hook_id: "test-runner".into(),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(
        report.outcomes.len(),
        1,
        "a rejection of an in-slice outcome must update it in place, not append a second entry"
    );
    assert_eq!(
        report.outcomes[0].status,
        OutcomeStatus::RejectedByHook {
            hook_id: "test-runner".into()
        }
    );
    assert_eq!(report.outcomes[0].summary, "looks done");
}

/// Internal-consistency check the review round named directly: the
/// verdicts section already flags a forged `TaskVerified` instead of
/// rendering it as legitimate — a hook-rejected outcome must get the
/// same treatment, not silently read as accepted.
#[test]
fn outcome_rejected_by_hook_is_distinguishable_from_accepted() {
    let run_id = RunId::new();
    let node = node_key("impl_1");
    let outcome = outcome_key("done");
    let accepted_events = vec![event(
        1,
        EventPayload::OutcomeReported {
            node: node.clone(),
            outcome: outcome.clone(),
            summary: "ok".into(),
        },
    )];
    let rejected_events = vec![
        event(
            1,
            EventPayload::OutcomeReported {
                node: node.clone(),
                outcome: outcome.clone(),
                summary: "ok".into(),
            },
        ),
        event(
            2,
            EventPayload::OutcomeRejectedByHook {
                node,
                outcome,
                hook_id: "fmt-check".into(),
            },
        ),
    ];
    let accepted = RunReport::compile(run_id, &accepted_events);
    let rejected = RunReport::compile(run_id, &rejected_events);
    assert_eq!(accepted.outcomes[0].status, OutcomeStatus::Accepted);
    assert_ne!(accepted.outcomes[0].status, rejected.outcomes[0].status);
}

#[test]
fn outcome_rejected_by_hook_with_no_matching_prior_report_is_still_recorded() {
    // A torn log whose read window starts after the original
    // `OutcomeReported` — the rejection must not be dropped.
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::OutcomeRejectedByHook {
            node: node_key("impl_1"),
            outcome: outcome_key("done"),
            hook_id: "fmt-check".into(),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(
        report.outcomes[0].status,
        OutcomeStatus::RejectedByHook {
            hook_id: "fmt-check".into()
        }
    );
}

// ── Cost: uncosted events are counted, never silently free ──────────

#[test]
fn tokens_consumed_without_a_price_increments_uncosted_count_not_cost() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::TokensConsumed {
            session: crate::id::SessionId::new(),
            prompt_tokens: 100,
            output_tokens: 50,
            cache_hits: 0,
            model: "some-model".into(),
            cost_usd: None,
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.cost.cost_usd, 0.0);
    assert_eq!(report.cost.uncosted_token_events, 1);
}

// ── A torn log's `StageCompleted`/`StageFailed` with no prior `StageEntered` ──

#[test]
fn stage_completed_with_no_prior_stage_entered_still_creates_a_node_entry() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::StageCompleted {
            node: node_key("impl_1"),
            outcome: outcome_key("done"),
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(
        report.nodes.len(),
        1,
        "a torn log missing StageEntered must not lose the node entirely"
    );
    assert_eq!(report.nodes[0].attempts, 1);
    assert_eq!(
        report.nodes[0].status,
        NodeStatus::Completed {
            outcome: outcome_key("done")
        }
    );
}

#[test]
fn stage_failed_with_no_prior_stage_entered_still_creates_a_node_entry() {
    let run_id = RunId::new();
    let events = vec![event(
        1,
        EventPayload::StageFailed {
            node: node_key("impl_1"),
            reason: "boom".into(),
            retry_available: false,
        },
    )];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.nodes.len(), 1);
    assert_eq!(report.nodes[0].attempts, 1);
    assert_eq!(
        report.nodes[0].status,
        NodeStatus::Failed {
            reason: "boom".into(),
            retry_available: false,
        }
    );
}

// ── caveats ───────────────────────────────────────────────────────────

#[test]
fn caveats_always_names_the_memory_receipts_gap_in_every_format() {
    // Checked at the `RunReport` level (not just a renderer's prose) so
    // a JSON consumer sees the same warning a Markdown/HTML reader does
    // — an empty `memory_receipts: []` alone reads as "memory was not
    // used," which is exactly the false reading this field prevents.
    let run_id = RunId::new();
    let report = RunReport::compile(run_id, &[]);
    assert!(
        report
            .caveats
            .iter()
            .any(|c| c.contains("memory_receipts is always empty")),
        "caveats: {:?}",
        report.caveats
    );
}
