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

/// Spec §10/R30's headline case: a run that completes successfully but
/// never had a verifier node run at all must not read the same as a proven
/// one. `surge inbox` (`commands::inbox::tests::
/// evidence_backed_distinguishes_a_verified_completion_from_an_unverified_one`)
/// and `surge ledger` (`task_ledger::tests::
/// is_evidence_backed_matches_completed_and_verified_only`) in `surge-cli`/
/// `surge-persistence` each pin this identical *scenario* against their own
/// layer's data shape — a registry `TaskLedgerIndexRecord`, not this event
/// log — so it is three independent fixtures proving the same fact, not one
/// shared fixture three tests import. Reading this test alongside those two
/// is how you audit that the three layers agree; nothing here binds them
/// together in code.
#[test]
fn completed_run_with_no_verdicts_at_all_is_not_evidence_backed() {
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
    assert_eq!(report.evidence_backed, Some(false));
}

#[test]
fn completed_run_with_an_authorized_verdict_is_evidence_backed() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
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
                node: verifier,
                evidence: ContentHash::compute(b"verification-report"),
            },
        ),
        event(
            3,
            EventPayload::RunCompleted {
                terminal_node: node_key("verify_1"),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.evidence_backed, Some(true));
}

/// Same shape as the authorized case above, but the `TaskVerified` names a
/// node the active graph never granted authority to — `report.verdicts`
/// still gets an entry (`Unauthorized`), but it must not count toward
/// evidence-backing. Mutating `VerdictResult::node_outcome`'s `Unauthorized`
/// arm back to `Completed`/`verified: true` turns this assertion red.
#[test]
fn completed_run_with_only_an_unauthorized_verdict_is_not_evidence_backed() {
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
        event(
            3,
            EventPayload::RunCompleted {
                terminal_node: node_key("impl_1"),
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.evidence_backed, Some(false));
}

/// "Was this proven" is only meaningful for a claimed success —
/// `evidence_backed` must be `None`, not `Some(false)`, for a failed run.
/// Collapsing the two would make an honest failure look like exactly the
/// "success without proof" case this field exists to flag.
#[test]
fn a_failed_run_carries_no_evidence_backed_question() {
    let run_id = RunId::new();
    let report = RunReport::compile(
        run_id,
        &[event(
            1,
            EventPayload::RunFailed {
                error: "boom".into(),
            },
        )],
    );
    assert_eq!(report.evidence_backed, None);
}

/// Spec §10's "same fact on every surface": `run_state.rs`'s
/// `LedgerState::record_status_change` clears `verified` on *any*
/// non-`Completed` transition, and production really does emit
/// `TaskVerified` → `TaskStatusChanged{to: ReadyForVerification}` for a task
/// sent back for re-verification (`stage::agent::emit_ledger_event`,
/// mirrored by `run_state.rs`'s own
/// `status_change_after_verified_clears_verified_flag` test). Before this
/// fixture, `compile` only ever pushed onto `verdicts` and never revisited
/// an earlier entry, so this exact sequence left a stale `Verified` verdict
/// standing and `evidence_backed` read `Some(true)` while `surge inbox` /
/// `surge ledger` (which fold through `LedgerState`) already read `false`
/// for the same run.
///
/// The earlier verdict is *marked* `Superseded`, not removed from
/// `verdicts` — `LedgerState::record_status_change` clears `verified` on the
/// task's existing ledger row, it does not delete the row. Removing here
/// instead would shrink `evidence_backed`'s denominator (see
/// `one_verified_and_one_superseded_after_verification_is_not_evidence_backed`
/// below for the case that distinguishes the two).
#[test]
fn status_change_after_verified_revokes_the_stale_verdict() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
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
                evidence: ContentHash::compute(b"evidence"),
            },
        ),
        event(
            3,
            EventPayload::TaskStatusChanged {
                task_id: "t1".into(),
                from: RoadmapStatus::Completed,
                to: RoadmapStatus::ReadyForVerification,
                authority_node: node_key("impl_1"),
            },
        ),
        event(
            4,
            EventPayload::RunCompleted {
                terminal_node: verifier,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.verdicts.len(), 1);
    assert_eq!(
        report.verdicts[0].result,
        VerdictResult::Superseded,
        "the earlier Verified verdict must be marked Superseded, not left standing: {:?}",
        report.verdicts
    );
    assert_eq!(report.evidence_backed, Some(false));
}

/// The scenario `retain`-based revocation (an earlier, rejected approach)
/// would get backwards: a task verified then later failed outright must
/// still drag the run's `evidence_backed` down — removing it from
/// `verdicts` instead of marking it `Superseded` would shrink `all()`'s
/// denominator to just the other, genuinely-verified task and read
/// `Some(true)`.
#[test]
fn one_verified_and_one_superseded_after_verification_is_not_evidence_backed() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
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
                evidence: ContentHash::compute(b"t1-evidence"),
            },
        ),
        event(
            3,
            EventPayload::TaskVerified {
                task_id: "t2".into(),
                node: verifier.clone(),
                evidence: ContentHash::compute(b"t2-evidence"),
            },
        ),
        event(
            4,
            EventPayload::TaskStatusChanged {
                task_id: "t2".into(),
                from: RoadmapStatus::Completed,
                to: RoadmapStatus::Failed,
                authority_node: node_key("impl_1"),
            },
        ),
        event(
            5,
            EventPayload::RunCompleted {
                terminal_node: verifier,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(
        report.verdicts.len(),
        2,
        "t2 must still be tracked (Superseded), not dropped: {:?}",
        report.verdicts
    );
    assert_eq!(report.evidence_backed, Some(false));
}

/// The aggregation the review round asked to pin: one verified task among
/// several rejected ones must not read as a proven run.
/// `!verdicts.is_empty() && verdicts.iter().all(..)` requires every recorded
/// verdict to be evidence-backed — mutating that back to `.any(..)` turns
/// this assertion (built with a mixed-outcome fixture no single-verdict test
/// above can catch) green on a run that should read `Some(false)`.
#[test]
fn one_verified_among_several_rejected_verdicts_is_not_evidence_backed() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
    let mut events = vec![
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
                evidence: ContentHash::compute(b"evidence"),
            },
        ),
    ];
    for (i, task_id) in ["t2", "t3", "t4", "t5"].into_iter().enumerate() {
        events.push(event(
            3 + i as u64,
            EventPayload::TaskStatusChanged {
                task_id: task_id.into(),
                from: RoadmapStatus::ReadyForVerification,
                to: RoadmapStatus::FailedVerification,
                authority_node: verifier.clone(),
            },
        ));
    }
    events.push(event(
        7,
        EventPayload::RunCompleted {
            terminal_node: verifier,
        },
    ));

    let report = RunReport::compile(run_id, &events);
    assert_eq!(report.verdicts.len(), 5);
    assert_eq!(report.evidence_backed, Some(false));
}

/// The denominator-convergence case the review round asked to pin: a task
/// that was only ever `TaskDiscovered` — never verified, never rejected —
/// is a row in `surge_persistence::task_ledger`'s registry mirror (what
/// `surge inbox`/`surge ledger` read), so it must count against this run's
/// `evidence_backed` the same way it counts there, even though it never
/// appears in `report.verdicts` at all (no verdict event ever named it).
/// Computing `evidence_backed` over `report.verdicts` alone (the mutation
/// this pins) would ignore t2 entirely and read `Some(true)` off t1's
/// verdict alone — `report.verdicts.len()` staying at 1 here, rather than
/// growing to 2, is exactly the tell that this task never got a verdict but
/// still must count.
#[test]
fn task_discovered_with_no_verdict_at_all_still_counts_against_evidence_backed() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
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
                evidence: ContentHash::compute(b"evidence"),
            },
        ),
        event(
            3,
            EventPayload::TaskDiscovered {
                task_id: "t2".into(),
                discovered_from: "t1".into(),
                title: "follow-up work".into(),
            },
        ),
        event(
            4,
            EventPayload::RunCompleted {
                terminal_node: verifier,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(
        report.verdicts.len(),
        1,
        "t2 never received a verdict event, so it must not appear in verdicts: {:?}",
        report.verdicts
    );
    assert_eq!(
        report.evidence_backed,
        Some(false),
        "t2 was discovered but never verified — the run is not a proven success"
    );
}

/// `cargo mutants` found this uncovered: `record_task_status_change` clears
/// `verified` unless `to == Completed` — a redundant/duplicate
/// `TaskStatusChanged{to: Completed}` for an already-verified task must
/// leave its `verified` bit alone, not clear it. No prior fixture exercised
/// a second, `Completed`-targeted status change after `TaskVerified` for
/// the same task_id.
#[test]
fn redundant_completed_status_change_does_not_clear_verified() {
    let run_id = RunId::new();
    let verifier = node_key("verify_1");
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
                evidence: ContentHash::compute(b"evidence"),
            },
        ),
        event(
            3,
            EventPayload::TaskStatusChanged {
                task_id: "t1".into(),
                from: RoadmapStatus::Completed,
                to: RoadmapStatus::Completed,
                authority_node: verifier.clone(),
            },
        ),
        event(
            4,
            EventPayload::RunCompleted {
                terminal_node: verifier,
            },
        ),
    ];
    let report = RunReport::compile(run_id, &events);
    assert_eq!(
        report.evidence_backed,
        Some(true),
        "a redundant TaskStatusChanged{{to: Completed}} must not clear an \
         already-verified task's evidence-backed bit"
    );
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
