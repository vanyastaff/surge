//! Property test: deterministic fold.
//!
//! For any well-formed event sequence:
//! 1. `fold(events)` is idempotent — running twice yields the same `RunState`.
//! 2. Incremental `apply()` step-by-step equals one-shot `fold()` byte-for-byte
//!    at every prefix `N ∈ [0, |events|]`.
//!
//! Strategies generate small valid graphs (1–5 inner nodes) and the linear
//! event sequence a deterministic engine would produce. Timestamps come from
//! a seeded counter — no wall-clock reads, no random IDs introduced inside
//! the strategy or fold.

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use surge_core::approvals::ApprovalPolicy;
use surge_core::content_hash::ContentHash;
use surge_core::edge::EdgeKind;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::{EventPayload, RunConfig, RunEvent};
use surge_core::run_state::{RunState, apply, fold};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};

const FIXED_RUN_TIMESTAMP_BASE: i64 = 1_736_000_000;

fn arb_node_key() -> impl Strategy<Value = NodeKey> {
    "[a-z][a-z0-9_]{2,10}".prop_map(|s| NodeKey::try_from(s.as_str()).unwrap())
}

fn arb_outcome_key() -> impl Strategy<Value = OutcomeKey> {
    "[a-z][a-z0-9_]{2,8}".prop_map(|s| OutcomeKey::try_from(s.as_str()).unwrap())
}

/// Build a linear graph of N notify-style placeholder nodes plus a terminal.
fn arb_linear_graph() -> impl Strategy<Value = (Graph, Vec<NodeKey>)> {
    (1usize..=5)
        .prop_flat_map(|n_inner| {
            (
                Just(n_inner),
                prop::collection::vec(arb_node_key(), n_inner + 1),
                arb_outcome_key(),
            )
        })
        .prop_filter(
            "unique node keys including terminal",
            |(_, keys, _)| {
                let set: std::collections::HashSet<_> = keys.iter().collect();
                set.len() == keys.len()
            },
        )
        .prop_map(|(_, keys, outcome)| build_linear_graph(keys, outcome))
}

fn build_linear_graph(keys: Vec<NodeKey>, outcome: OutcomeKey) -> (Graph, Vec<NodeKey>) {
    let mut nodes = BTreeMap::new();
    let term_key = keys.last().expect("at least one key").clone();
    let mut inner_keys = Vec::new();

    for (i, k) in keys.iter().enumerate() {
        let is_terminal = i == keys.len() - 1;
        let config = if is_terminal {
            NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            })
        } else {
            NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            })
        };
        let outcomes = if is_terminal {
            vec![]
        } else {
            vec![OutcomeDecl {
                id: outcome.clone(),
                description: String::new(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
            }]
        };
        if !is_terminal {
            inner_keys.push(k.clone());
        }
        nodes.insert(
            k.clone(),
            Node {
                id: k.clone(),
                position: Position::default(),
                declared_outcomes: outcomes,
                config,
            },
        );
    }

    let _ = term_key; // currently unused but kept for clarity
    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "proptest-linear".into(),
            description: None,
            template_origin: None,
            created_at: Utc.timestamp_opt(FIXED_RUN_TIMESTAMP_BASE, 0).unwrap(),
            author: None,
        },
        start: keys[0].clone(),
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };
    (graph, inner_keys)
}

fn ts(seq: u64) -> DateTime<Utc> {
    Utc.timestamp_opt(FIXED_RUN_TIMESTAMP_BASE + seq as i64, 0)
        .unwrap()
}

fn build_canonical_events(graph: Graph, inner: Vec<NodeKey>, run_id: RunId) -> Vec<RunEvent> {
    let mut events = Vec::new();
    let mut seq = 0u64;
    let mut push = |seq: &mut u64, payload: EventPayload| {
        *seq += 1;
        events.push(RunEvent {
            run_id,
            seq: *seq,
            timestamp: ts(*seq),
            payload,
        });
    };

    push(
        &mut seq,
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/proptest"),
            initial_prompt: "deterministic-fold".into(),
            config: RunConfig {
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        },
    );

    let graph_hash = ContentHash::compute(b"proptest-linear-canonical");
    push(
        &mut seq,
        EventPayload::PipelineMaterialized {
            graph: Box::new(graph),
            graph_hash,
        },
    );

    let outcome = OutcomeKey::try_from("done").unwrap();
    for node_key in inner {
        push(
            &mut seq,
            EventPayload::StageEntered {
                node: node_key.clone(),
                attempt: 1,
            },
        );
        push(
            &mut seq,
            EventPayload::OutcomeReported {
                node: node_key,
                outcome: outcome.clone(),
                summary: String::new(),
            },
        );
    }

    events
}

fn one_shot_fold(events: &[RunEvent]) -> RunState {
    fold(events).expect("canonical event sequence must fold cleanly")
}

fn incremental_fold(events: &[RunEvent]) -> RunState {
    let mut state = RunState::NotStarted;
    for ev in events {
        state = apply(state, ev).expect("apply must succeed for canonical sequence");
    }
    state
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn fold_is_idempotent_and_matches_incremental(spec in arb_linear_graph()) {
        let (graph, inner) = spec;
        let run_id = RunId::new();
        let events = build_canonical_events(graph, inner, run_id);

        // Idempotent: same input → same output across two calls.
        let first = one_shot_fold(&events);
        let second = one_shot_fold(&events);
        prop_assert_eq!(&first, &second);

        // Incremental matches one-shot at every prefix.
        for n in 0..=events.len() {
            let one_shot = one_shot_fold(&events[..n]);
            let incremental = incremental_fold(&events[..n]);
            prop_assert_eq!(
                &one_shot, &incremental,
                "incremental != one-shot at prefix {}",
                n
            );
        }
    }
}
