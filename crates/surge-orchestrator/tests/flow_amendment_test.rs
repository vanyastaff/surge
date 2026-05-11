use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use surge_core::agent_config::{AgentConfig, ArtifactSource, Binding, TemplateVar};
use surge_core::content_hash::ContentHash;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone, RoadmapTask};
use surge_core::roadmap_patch::{RoadmapItemRef, RoadmapPatchApplyResult};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::validate::validate_for_m6;
use surge_orchestrator::flow_amendment::{
    FlowAmendmentError, amend_active_flow, create_follow_up_flow,
};

#[test]
fn active_terminal_only_flow_inserts_amendment_chain_without_mutating_base() {
    let base = terminal_only_graph();
    let original = base.clone();
    let patch_result = patch_result_with_milestone();

    let amended = amend_active_flow(&base, &patch_result).expect("active flow amends");

    assert_eq!(
        base, original,
        "base graph remains unchanged on clone-and-validate path"
    );
    assert_eq!(amended.graph.start, node_key("amend_001"));
    assert_eq!(amended.inserted_nodes, vec![node_key("amend_001")]);
    assert_agent_node_has_spec_binding(&amended.graph, "amend_001", "m2: Metrics");
    assert_forward_edge(&amended.graph, "amend_001", "implemented", "end");
    validate_for_m6(&amended.graph).expect("amended graph validates");
    let parsed: Graph = toml::from_str(&amended.flow_toml).expect("flow TOML parses");
    assert_eq!(parsed, amended.graph);
}

#[test]
fn active_flow_rewires_existing_success_edge_to_inserted_stage() {
    let base = existing_agent_to_success_graph();
    let patch_result = patch_result_with_task();

    let amended = amend_active_flow(&base, &patch_result).expect("active flow amends");

    let old_edge = amended
        .graph
        .edges
        .iter()
        .find(|edge| edge.id == edge_key("e_existing"))
        .expect("existing edge remains");
    assert_eq!(old_edge.to, node_key("amend_001"));
    assert_forward_edge(&amended.graph, "amend_001", "implemented", "end");
    assert_eq!(amended.graph.start, node_key("existing"));
    validate_for_m6(&amended.graph).expect("rewired graph validates");
}

#[test]
fn follow_up_flow_is_valid_and_deterministic() {
    let patch_result = patch_result_with_task();
    let created_at = Utc
        .with_ymd_and_hms(2026, 5, 11, 12, 0, 0)
        .single()
        .expect("fixed timestamp is valid");

    let flow = create_follow_up_flow(&patch_result, created_at).expect("follow-up flow builds");

    assert_eq!(flow.graph.metadata.created_at, created_at);
    assert_eq!(flow.graph.start, node_key("amend_001"));
    assert!(flow.graph.nodes.contains_key(&node_key("end")));
    assert_eq!(
        flow.graph_hash,
        ContentHash::compute(flow.flow_toml.as_bytes())
    );
    let parsed: Graph = toml::from_str(&flow.flow_toml).expect("flow TOML parses");
    validate_for_m6(&parsed).expect("follow-up graph validates");
    assert_eq!(parsed, flow.graph);
}

#[test]
fn no_amendment_items_returns_typed_error() {
    let mut patch_result = patch_result_with_milestone();
    patch_result.inserted_milestones.clear();
    patch_result.inserted_tasks.clear();
    patch_result.replaced_items.clear();

    let error = amend_active_flow(&terminal_only_graph(), &patch_result)
        .expect_err("empty amendment result is rejected");

    assert!(matches!(error, FlowAmendmentError::NoAmendmentItems));
}

fn terminal_only_graph() -> Graph {
    let end = node_key("end");
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("terminal-only", Utc::now()),
        start: end.clone(),
        nodes: [(end.clone(), success_terminal_node("end"))].into(),
        edges: Vec::new(),
        subgraphs: BTreeMap::new(),
    }
}

fn existing_agent_to_success_graph() -> Graph {
    let existing = node_key("existing");
    let end = node_key("end");
    let mut nodes = BTreeMap::new();
    nodes.insert(existing.clone(), agent_node("existing"));
    nodes.insert(end, success_terminal_node("end"));
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("existing-agent", Utc::now()),
        start: existing,
        nodes,
        edges: vec![forward_edge("e_existing", "existing", "implemented", "end")],
        subgraphs: BTreeMap::new(),
    }
}

fn patch_result_with_milestone() -> RoadmapPatchApplyResult {
    let roadmap = RoadmapArtifact::new(vec![RoadmapMilestone::new("m2", "Metrics")]);
    RoadmapPatchApplyResult {
        markdown: roadmap.to_markdown(),
        roadmap,
        inserted_milestones: vec!["m2".into()],
        inserted_tasks: Vec::new(),
        replaced_items: Vec::new(),
        dependencies_added: Vec::new(),
    }
}

fn patch_result_with_task() -> RoadmapPatchApplyResult {
    let mut milestone = RoadmapMilestone::new("m2", "Metrics");
    milestone
        .tasks
        .push(RoadmapTask::new("t1", "Add runtime counters"));
    let roadmap = RoadmapArtifact::new(vec![milestone]);
    RoadmapPatchApplyResult {
        markdown: roadmap.to_markdown(),
        roadmap,
        inserted_milestones: Vec::new(),
        inserted_tasks: vec![RoadmapItemRef::Task {
            milestone_id: "m2".into(),
            task_id: "t1".into(),
        }],
        replaced_items: Vec::new(),
        dependencies_added: Vec::new(),
    }
}

fn assert_agent_node_has_spec_binding(graph: &Graph, node: &str, expected: &str) {
    let node = graph.nodes.get(&node_key(node)).expect("node exists");
    assert_eq!(
        node.declared_outcomes
            .iter()
            .map(|outcome| outcome.id.clone())
            .collect::<Vec<_>>(),
        vec![outcome_key("implemented")]
    );
    let NodeConfig::Agent(agent) = &node.config else {
        panic!("expected agent node");
    };
    assert_eq!(agent.profile, profile_key("implementer@1.0"));
    let binding = agent.bindings.first().expect("spec binding exists");
    assert_eq!(binding.target, TemplateVar("spec".into()));
    let ArtifactSource::Static { content } = &binding.source else {
        panic!("expected static spec binding");
    };
    assert!(content.contains(expected), "spec contains label: {content}");
    assert!(content.contains("## Amended roadmap"));
}

fn assert_forward_edge(graph: &Graph, from: &str, outcome: &str, to: &str) {
    let exists = graph.edges.iter().any(|edge| {
        edge.from.node == node_key(from)
            && edge.from.outcome == outcome_key(outcome)
            && edge.to == node_key(to)
            && edge.kind == EdgeKind::Forward
    });
    assert!(
        exists,
        "expected forward edge {from}.{outcome} -> {to}, got {:?}",
        graph.edges
    );
}

fn agent_node(id: &str) -> Node {
    let key = node_key(id);
    Node {
        id: key,
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: outcome_key("implemented"),
            description: "implemented".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: profile_key("implementer@1.0"),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![Binding {
                source: ArtifactSource::Static {
                    content: "# Spec".into(),
                },
                target: TemplateVar("spec".into()),
                optional: false,
            }],
            rules_overrides: None,
            limits: Default::default(),
            hooks: Vec::new(),
            custom_fields: Default::default(),
        }),
    }
}

fn success_terminal_node(id: &str) -> Node {
    let key = node_key(id);
    Node {
        id: key,
        position: Position::default(),
        declared_outcomes: Vec::new(),
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn forward_edge(id: &str, from: &str, outcome: &str, to: &str) -> Edge {
    Edge {
        id: edge_key(id),
        from: PortRef {
            node: node_key(from),
            outcome: outcome_key(outcome),
        },
        to: node_key(to),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }
}

fn node_key(value: &str) -> NodeKey {
    NodeKey::try_from(value).expect("valid node key")
}

fn edge_key(value: &str) -> EdgeKey {
    EdgeKey::try_from(value).expect("valid edge key")
}

fn outcome_key(value: &str) -> OutcomeKey {
    OutcomeKey::try_from(value).expect("valid outcome key")
}

fn profile_key(value: &str) -> ProfileKey {
    ProfileKey::try_from(value).expect("valid profile key")
}
