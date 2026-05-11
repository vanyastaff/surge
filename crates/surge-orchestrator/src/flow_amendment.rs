//! Flow graph amendment helpers for approved roadmap patches.
//!
//! The helper keeps graph mutation conservative: clone the existing graph,
//! insert a linear chain of Agent nodes for the appended/reworked roadmap
//! items, validate the clone, and only then return it to the caller.

use std::collections::BTreeSet;

use crate::engine::validate::validate_for_m6;
use chrono::{DateTime, Utc};
use surge_core::agent_config::{AgentConfig, ArtifactSource, Binding, TemplateVar};
use surge_core::content_hash::ContentHash;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::keys::{EdgeKey, KeyParseError, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::roadmap_patch::{RoadmapItemRef, RoadmapPatchApplyResult};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};

const IMPLEMENTER_PROFILE: &str = "implementer@1.0";
const IMPLEMENTED_OUTCOME: &str = "implemented";
const END_NODE: &str = "end";
const SPEC_TEMPLATE_VAR: &str = "spec";

/// Result of generating an amended flow graph.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowAmendmentResult {
    /// Validated amended graph.
    pub graph: Graph,
    /// TOML representation of `graph`.
    pub flow_toml: String,
    /// Content hash of `flow_toml`.
    pub graph_hash: ContentHash,
    /// Nodes inserted for amendment work, in execution order.
    pub inserted_nodes: Vec<NodeKey>,
}

/// Errors from flow amendment generation.
#[derive(Debug, thiserror::Error)]
pub enum FlowAmendmentError {
    /// No roadmap work items were available to turn into stages.
    #[error("roadmap patch apply result contains no flow amendment items")]
    NoAmendmentItems,
    /// Active graph has no success terminal to append before.
    #[error("active flow has no success terminal append point")]
    NoSuccessAppendPoint,
    /// Generated key violated graph key rules.
    #[error("invalid generated graph key: {0}")]
    Key(#[from] KeyParseError),
    /// Generated graph exhausted the deterministic key range.
    #[error("generated graph key space exhausted for prefix {prefix}")]
    KeySpaceExhausted {
        /// Key prefix that ran out of deterministic slots.
        prefix: &'static str,
    },
    /// Generated graph failed M6 validation.
    #[error("amended flow failed validation: {0}")]
    Validation(String),
    /// TOML serialization failed.
    #[error("failed to serialize amended flow: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Insert amendment stages into an active graph before its success terminal.
///
/// # Errors
/// Returns [`FlowAmendmentError`] when no safe append point exists, generated
/// keys are invalid, validation fails, or TOML serialization fails.
pub fn amend_active_flow(
    base: &Graph,
    patch_result: &RoadmapPatchApplyResult,
) -> Result<FlowAmendmentResult, FlowAmendmentError> {
    tracing::debug!(
        target: "flow_amendment",
        base_nodes = base.nodes.len(),
        base_edges = base.edges.len(),
        inserted_milestones = patch_result.inserted_milestones.len(),
        inserted_tasks = patch_result.inserted_tasks.len(),
        replaced_items = patch_result.replaced_items.len(),
        "active_flow_amendment_start"
    );
    let old_graph_hash = graph_content_hash(base)?;
    let specs = amendment_stage_specs(patch_result)?;
    let mut graph = base.clone();
    let append = success_append_point(&graph).ok_or(FlowAmendmentError::NoSuccessAppendPoint)?;
    let chain = insert_stage_chain(
        &mut graph,
        &specs,
        append.terminal.clone(),
        &patch_result.markdown,
    )?;
    for edge_index in append.incoming_edge_indices {
        let edge = graph
            .edges
            .get_mut(edge_index)
            .ok_or(FlowAmendmentError::NoSuccessAppendPoint)?;
        tracing::debug!(
            target: "flow_amendment",
            edge_id = %edge.id,
            from_node = %edge.from.node,
            from_outcome = %edge.from.outcome,
            old_target = %append.terminal,
            new_target = %chain.first,
            "active_flow_rewire_success_edge"
        );
        edge.to = chain.first.clone();
    }
    if append.start_is_terminal {
        tracing::debug!(
            target: "flow_amendment",
            old_start = %append.terminal,
            new_start = %chain.first,
            "active_flow_rewire_start"
        );
        graph.start = chain.first.clone();
    }
    let result = finalize_graph(graph, chain.inserted_nodes);
    match &result {
        Ok(result) => tracing::info!(
            target: "flow_amendment",
            old_graph_hash = %old_graph_hash,
            new_graph_hash = %result.graph_hash,
            inserted_nodes = result.inserted_nodes.len(),
            "active_flow_amendment_succeeded"
        ),
        Err(error) => tracing::warn!(
            target: "flow_amendment",
            old_graph_hash = %old_graph_hash,
            error = %error,
            "active_flow_amendment_rolled_back"
        ),
    }
    result
}

/// Create a standalone follow-up flow from appended roadmap work.
///
/// # Errors
/// Returns [`FlowAmendmentError`] when generated keys are invalid, validation
/// fails, or TOML serialization fails.
pub fn create_follow_up_flow(
    patch_result: &RoadmapPatchApplyResult,
    created_at: DateTime<Utc>,
) -> Result<FlowAmendmentResult, FlowAmendmentError> {
    tracing::debug!(
        target: "flow_amendment",
        inserted_milestones = patch_result.inserted_milestones.len(),
        inserted_tasks = patch_result.inserted_tasks.len(),
        replaced_items = patch_result.replaced_items.len(),
        "follow_up_flow_amendment_start"
    );
    let specs = amendment_stage_specs(patch_result)?;
    let end = NodeKey::try_from(END_NODE)?;
    let mut graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("roadmap-amendment-follow-up", created_at),
        start: end.clone(),
        nodes: [(end.clone(), success_terminal(end.clone()))].into(),
        edges: Vec::new(),
        subgraphs: Default::default(),
    };
    let chain = insert_stage_chain(&mut graph, &specs, end, &patch_result.markdown)?;
    graph.start = chain.first;
    let result = finalize_graph(graph, chain.inserted_nodes)?;
    tracing::info!(
        target: "flow_amendment",
        new_graph_hash = %result.graph_hash,
        inserted_nodes = result.inserted_nodes.len(),
        "follow_up_flow_amendment_succeeded"
    );
    Ok(result)
}

fn finalize_graph(
    graph: Graph,
    inserted_nodes: Vec<NodeKey>,
) -> Result<FlowAmendmentResult, FlowAmendmentError> {
    tracing::debug!(
        target: "flow_amendment",
        node_count = graph.nodes.len(),
        edge_count = graph.edges.len(),
        "flow_amendment_validation_start"
    );
    if let Err(error) = validate_for_m6(&graph) {
        let diagnostic = error.to_string();
        tracing::warn!(
            target: "flow_amendment",
            diagnostic = %diagnostic,
            "flow_amendment_validation_failed"
        );
        return Err(FlowAmendmentError::Validation(diagnostic));
    }
    tracing::debug!(
        target: "flow_amendment",
        node_count = graph.nodes.len(),
        edge_count = graph.edges.len(),
        "flow_amendment_validation_succeeded"
    );
    let flow_toml = toml::to_string(&graph)?;
    let graph_hash = ContentHash::compute(flow_toml.as_bytes());
    Ok(FlowAmendmentResult {
        graph,
        flow_toml,
        graph_hash,
        inserted_nodes,
    })
}

struct AppendPoint {
    terminal: NodeKey,
    incoming_edge_indices: Vec<usize>,
    start_is_terminal: bool,
}

struct InsertedChain {
    first: NodeKey,
    inserted_nodes: Vec<NodeKey>,
}

#[derive(Debug, Clone)]
struct AmendmentStageSpec {
    label: String,
}

fn amendment_stage_specs(
    patch_result: &RoadmapPatchApplyResult,
) -> Result<Vec<AmendmentStageSpec>, FlowAmendmentError> {
    let mut specs = Vec::new();
    specs.extend(
        patch_result
            .inserted_milestones
            .iter()
            .map(|id| AmendmentStageSpec {
                label: milestone_label(patch_result, id),
            }),
    );
    specs.extend(
        patch_result
            .inserted_tasks
            .iter()
            .map(|reference| AmendmentStageSpec {
                label: task_label(patch_result, reference),
            }),
    );
    specs.extend(
        patch_result
            .replaced_items
            .iter()
            .map(|reference| AmendmentStageSpec {
                label: format!("Rework roadmap item {}", item_ref_label(reference)),
            }),
    );
    if specs.is_empty() {
        return Err(FlowAmendmentError::NoAmendmentItems);
    }
    Ok(specs)
}

fn success_append_point(graph: &Graph) -> Option<AppendPoint> {
    if is_success_terminal(graph, &graph.start) {
        return Some(AppendPoint {
            terminal: graph.start.clone(),
            incoming_edge_indices: Vec::new(),
            start_is_terminal: true,
        });
    }
    let mut candidates = graph
        .nodes
        .keys()
        .filter(|node_key| is_success_terminal(graph, node_key))
        .filter_map(|terminal| {
            let incoming_edge_indices = graph
                .edges
                .iter()
                .enumerate()
                .filter_map(|(index, edge)| (&edge.to == terminal).then_some(index))
                .collect::<Vec<_>>();
            (!incoming_edge_indices.is_empty()).then(|| AppendPoint {
                terminal: terminal.clone(),
                incoming_edge_indices,
                start_is_terminal: false,
            })
        })
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

fn is_success_terminal(graph: &Graph, node_key: &NodeKey) -> bool {
    matches!(
        graph.nodes.get(node_key).map(|node| &node.config),
        Some(NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            ..
        }))
    )
}

fn insert_stage_chain(
    graph: &mut Graph,
    specs: &[AmendmentStageSpec],
    tail: NodeKey,
    roadmap_markdown: &str,
) -> Result<InsertedChain, FlowAmendmentError> {
    let mut existing_nodes = graph.nodes.keys().cloned().collect::<BTreeSet<_>>();
    let mut existing_edges = graph
        .edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<BTreeSet<_>>();
    let implemented = OutcomeKey::try_from(IMPLEMENTED_OUTCOME)?;
    let profile = ProfileKey::try_from(IMPLEMENTER_PROFILE)?;
    let mut node_keys = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        let key = next_node_key(&mut existing_nodes)?;
        tracing::debug!(
            target: "flow_amendment",
            node_id = %key,
            stage_label = %spec.label,
            "flow_amendment_insert_node"
        );
        graph.nodes.insert(
            key.clone(),
            amendment_agent_node(
                key.clone(),
                spec,
                index as f32,
                roadmap_markdown,
                &implemented,
                &profile,
            ),
        );
        node_keys.push(key);
    }
    for (index, node) in node_keys.iter().enumerate() {
        let to = node_keys
            .get(index + 1)
            .cloned()
            .unwrap_or_else(|| tail.clone());
        let edge_id = next_edge_key(&mut existing_edges)?;
        tracing::debug!(
            target: "flow_amendment",
            edge_id = %edge_id,
            from_node = %node,
            from_outcome = %implemented,
            to_node = %to,
            "flow_amendment_insert_edge"
        );
        graph.edges.push(Edge {
            id: edge_id,
            from: PortRef {
                node: node.clone(),
                outcome: implemented.clone(),
            },
            to,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        });
    }
    let first = node_keys
        .first()
        .cloned()
        .ok_or(FlowAmendmentError::NoAmendmentItems)?;
    Ok(InsertedChain {
        first,
        inserted_nodes: node_keys,
    })
}

fn amendment_agent_node(
    key: NodeKey,
    spec: &AmendmentStageSpec,
    index: f32,
    roadmap_markdown: &str,
    implemented: &OutcomeKey,
    profile: &ProfileKey,
) -> Node {
    Node {
        id: key,
        position: Position {
            x: 240.0,
            y: 160.0 + index * 120.0,
        },
        declared_outcomes: vec![OutcomeDecl {
            id: implemented.clone(),
            description: "Amendment item completed".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: profile.clone(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![Binding {
                source: ArtifactSource::Static {
                    content: amendment_spec_text(&spec.label, roadmap_markdown),
                },
                target: TemplateVar(SPEC_TEMPLATE_VAR.into()),
                optional: false,
            }],
            rules_overrides: None,
            limits: Default::default(),
            hooks: Vec::new(),
            custom_fields: Default::default(),
        }),
    }
}

fn success_terminal(key: NodeKey) -> Node {
    Node {
        id: key,
        position: Position { x: 480.0, y: 160.0 },
        declared_outcomes: Vec::new(),
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: Some("follow-up roadmap amendment complete".into()),
        }),
    }
}

fn next_node_key(existing: &mut BTreeSet<NodeKey>) -> Result<NodeKey, FlowAmendmentError> {
    for index in 1..=999 {
        let key = NodeKey::try_from(format!("amend_{index:03}"))?;
        if existing.insert(key.clone()) {
            return Ok(key);
        }
    }
    Err(FlowAmendmentError::KeySpaceExhausted { prefix: "amend" })
}

fn next_edge_key(existing: &mut BTreeSet<EdgeKey>) -> Result<EdgeKey, FlowAmendmentError> {
    for index in 1..=999 {
        let key = EdgeKey::try_from(format!("e_amend_{index:03}"))?;
        if existing.insert(key.clone()) {
            return Ok(key);
        }
    }
    Err(FlowAmendmentError::KeySpaceExhausted { prefix: "e_amend" })
}

fn item_ref_label(reference: &RoadmapItemRef) -> String {
    match reference {
        RoadmapItemRef::Milestone { milestone_id } => milestone_id.clone(),
        RoadmapItemRef::Task {
            milestone_id,
            task_id,
        } => {
            format!("{milestone_id}/{task_id}")
        },
    }
}

fn milestone_label(patch_result: &RoadmapPatchApplyResult, milestone_id: &str) -> String {
    let title = patch_result
        .roadmap
        .milestones
        .iter()
        .find(|milestone| milestone.id == milestone_id)
        .map(|milestone| milestone.title.as_str());
    match title {
        Some(title) => format!("Implement roadmap milestone {milestone_id}: {title}"),
        None => format!("Implement roadmap milestone {milestone_id}"),
    }
}

fn task_label(patch_result: &RoadmapPatchApplyResult, reference: &RoadmapItemRef) -> String {
    let RoadmapItemRef::Task {
        milestone_id,
        task_id,
    } = reference
    else {
        return format!("Implement roadmap item {}", item_ref_label(reference));
    };
    let title = patch_result
        .roadmap
        .milestones
        .iter()
        .find(|milestone| milestone.id == milestone_id.as_str())
        .and_then(|milestone| {
            milestone
                .tasks
                .iter()
                .find(|task| task.id == task_id.as_str())
                .map(|task| task.title.as_str())
        });
    match title {
        Some(title) => format!("Implement roadmap task {milestone_id}/{task_id}: {title}"),
        None => format!("Implement roadmap task {milestone_id}/{task_id}"),
    }
}

fn amendment_spec_text(label: &str, roadmap_markdown: &str) -> String {
    format!(
        "# Roadmap Amendment Item\n\n\
         ## Context\n\
         This stage was generated from an approved roadmap amendment.\n\n\
         ## What needs to be done\n\
         - {label}\n\n\
         ## Acceptance criteria\n\
         - Implement the requested roadmap amendment item using existing project patterns.\n\
         - Preserve completed roadmap history and avoid unrelated refactors.\n\
         - Run the focused verification that matches the touched code.\n\n\
         ## Amended roadmap\n\n\
         {roadmap_markdown}"
    )
}

pub(crate) fn graph_content_hash(graph: &Graph) -> Result<ContentHash, toml::ser::Error> {
    let toml = toml::to_string(graph)?;
    Ok(ContentHash::compute(toml.as_bytes()))
}
