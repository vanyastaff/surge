//! Locks the invariant that hook-related events are deterministic
//! pass-throughs: `HookExecuted` and `OutcomeRejectedByHook` may interleave
//! with stage events without mutating `RunMemory`.
//!
//! `RunMemory.outcomes` only grows on `OutcomeReported`; rejected outcomes
//! never reach the fold because the orchestrator drops them before persisting
//! `OutcomeReported` (see Task 1.3 wiring in agent.rs).
//!
//! Run: `cargo test -p surge-core --test fold_hook_events`
//! Accept new snapshots: `cargo insta accept`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use surge_core::approvals::ApprovalPolicy;
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::hooks::HookFailureMode;
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, RunConfig, RunEvent};
use surge_core::run_state::{RunMemory, RunState, fold};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};

fn fixed_ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 6, 0, 0, 0).single().unwrap()
}

fn ev(run_id: RunId, seq: u64, payload: EventPayload) -> RunEvent {
    RunEvent {
        run_id,
        seq,
        timestamp: fixed_ts(),
        payload,
    }
}

fn minimal_graph() -> Graph {
    let agent = NodeKey::try_from("impl_1").unwrap();
    let term = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        term.clone(),
        Node {
            id: term.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    nodes.insert(
        agent.clone(),
        Node {
            id: agent.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            // We don't drive an actual agent stage here — only fold synthetic
            // events. A `Terminal` shell-config keeps the graph valid as data.
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "fold-hook-events".into(),
            description: None,
            template_origin: None,
            created_at: fixed_ts(),
            author: None,
        },
        start: agent,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[test]
fn hook_events_are_pass_through_in_fold() {
    let run_id = RunId::new();
    let agent_node = NodeKey::try_from("impl_1").unwrap();
    let outcome_done = OutcomeKey::from_str("done").unwrap();
    let outcome_pass = OutcomeKey::from_str("pass").unwrap();
    let graph = minimal_graph();

    // 12 events: RunStarted → PipelineMaterialized → StageEntered →
    // HookExecuted (pre-tool) → OutcomeRejectedByHook → HookExecuted
    // (on_outcome) → StageEntered (retry) → HookExecuted (post-tool) →
    // OutcomeReported → HookExecuted (on_outcome accepted) → StageEntered
    // (next) → OutcomeReported.
    let events = vec![
        ev(
            run_id,
            1,
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: PathBuf::from("/tmp/run"),
                initial_prompt: "fold hooks".into(),
                config: RunConfig {
                    sandbox_default: SandboxMode::WorkspaceWrite,
                    approval_default: ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                },
            },
        ),
        ev(
            run_id,
            2,
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph.clone()),
                graph_hash: ContentHash::compute(b"fold-hook-events"),
            },
        ),
        ev(
            run_id,
            3,
            EventPayload::StageEntered {
                node: agent_node.clone(),
                attempt: 1,
            },
        ),
        ev(
            run_id,
            4,
            EventPayload::HookExecuted {
                hook_id: "deny-write".into(),
                exit_status: 0,
                on_failure: HookFailureMode::Reject,
            },
        ),
        ev(
            run_id,
            5,
            EventPayload::OutcomeRejectedByHook {
                node: agent_node.clone(),
                outcome: outcome_pass.clone(),
                hook_id: "verify-tests".into(),
            },
        ),
        ev(
            run_id,
            6,
            EventPayload::HookExecuted {
                hook_id: "verify-tests".into(),
                exit_status: 1,
                on_failure: HookFailureMode::Reject,
            },
        ),
        ev(
            run_id,
            7,
            EventPayload::StageEntered {
                node: agent_node.clone(),
                attempt: 2,
            },
        ),
        ev(
            run_id,
            8,
            EventPayload::HookExecuted {
                hook_id: "fmt-check".into(),
                exit_status: 0,
                on_failure: HookFailureMode::Warn,
            },
        ),
        ev(
            run_id,
            9,
            EventPayload::OutcomeReported {
                node: agent_node.clone(),
                outcome: outcome_done.clone(),
                summary: "implementation complete".into(),
            },
        ),
        ev(
            run_id,
            10,
            EventPayload::HookExecuted {
                hook_id: "verify-tests".into(),
                exit_status: 0,
                on_failure: HookFailureMode::Reject,
            },
        ),
        ev(
            run_id,
            11,
            EventPayload::StageEntered {
                node: agent_node.clone(),
                attempt: 3,
            },
        ),
        ev(
            run_id,
            12,
            EventPayload::OutcomeReported {
                node: agent_node.clone(),
                outcome: outcome_done.clone(),
                summary: "second pass".into(),
            },
        ),
    ];

    let final_state = fold(&events).expect("fold should succeed");
    let memory = match &final_state {
        RunState::Pipeline { memory, .. } => memory.clone(),
        other => panic!("expected Pipeline state, got {other:?}"),
    };

    // Replay-determinism guard: incremental `apply()` must equal one-shot fold.
    let mut incremental = RunState::NotStarted;
    for e in &events {
        incremental = surge_core::run_state::apply(incremental, e).expect("apply");
    }
    assert_eq!(incremental, final_state);

    // Memory invariant: only the two `OutcomeReported` calls populate
    // `RunMemory.outcomes`; the rejected `pass` and the audit `HookExecuted`
    // events leave it untouched.
    assert_only_two_outcomes_recorded(&memory, &agent_node);
    // RunMemory doesn't derive Serialize, so use a debug snapshot — sufficient
    // to lock the post-fold shape of `outcomes` and `artifacts`.
    insta::assert_debug_snapshot!("fold_hook_events_memory", memory);
}

fn assert_only_two_outcomes_recorded(memory: &RunMemory, node: &NodeKey) {
    let outcomes = memory.outcomes.get(node).expect("agent outcomes present");
    assert_eq!(
        outcomes.len(),
        2,
        "expected exactly 2 OutcomeReported entries, got {}",
        outcomes.len()
    );
    for record in outcomes {
        assert_eq!(record.outcome.as_str(), "done");
    }
}
