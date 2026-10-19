//! Child-only, authenticated repair of generated splitter provider bindings.
use super::*;
use crate::ContentHash;
use crate::keys::SubgraphKey;
use serde::{Deserialize, Serialize};

/// Immutable, versioned derivation from an original materialized graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRepair")]
pub struct GeneratedBindingRepair {
    version: u32,
    original: ContentHash,
    repaired: ContentHash,
    nodes: Vec<RepairedNode>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairedNode {
    scope: Option<SubgraphKey>,
    node: NodeKey,
    retry_target: NodeKey,
    previous_runtime: Option<BTreeMap<String, String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRepair {
    version: u32,
    original: ContentHash,
    repaired: ContentHash,
    nodes: Vec<RepairedNode>,
}
impl TryFrom<RawRepair> for GeneratedBindingRepair {
    type Error = BindingRepairError;
    fn try_from(raw: RawRepair) -> Result<Self, Self::Error> {
        let paths: BTreeSet<_> = raw
            .nodes
            .iter()
            .map(|node| (&node.scope, &node.node))
            .collect();
        let invalid_runtime = raw.nodes.iter().any(|node| {
            node.previous_runtime.as_ref().is_some_and(|table| {
                table.iter().any(|(key, value)| {
                    !["agent_id", "model"].contains(&key.as_str())
                        || value.trim().is_empty()
                        || value.len() > 512
                        || value.chars().any(char::is_control)
                })
            })
        });
        if invalid_runtime
            || raw.version != 1
            || raw.nodes.is_empty()
            || raw.nodes.len() > 1024
            || paths.len() != raw.nodes.len()
            || raw.original == raw.repaired
        {
            return Err(BindingRepairError);
        }
        Ok(Self {
            version: raw.version,
            original: raw.original,
            repaired: raw.repaired,
            nodes: raw.nodes,
        })
    }
}
/// A graph does not have the exact generated escalation contract.
#[derive(Debug, thiserror::Error)]
#[error("generated binding repair refused")]
pub struct BindingRepairError;
impl GeneratedBindingRepair {
    /// Derive a missing-only repair after authenticating each generated ladder.
    /// # Errors
    /// Refuses conflicting overrides or changed generated roles/topology.
    pub fn prepare(graph: &Graph) -> Result<Option<Self>, BindingRepairError> {
        let entries = graph
            .nodes
            .iter()
            .chain(
                graph
                    .subgraphs
                    .values()
                    .flat_map(|scope| scope.nodes.iter()),
            )
            .collect::<Vec<_>>();
        let unique = entries.iter().map(|(key, _)| *key).collect::<BTreeSet<_>>();
        if unique.len() != entries.len() || entries.iter().any(|(key, node)| **key != node.id) {
            return Err(BindingRepairError);
        }
        let mut repaired = graph.clone();
        let mut nodes = Vec::new();
        repair_scope(
            None,
            &mut repaired.nodes,
            &repaired.edges,
            graph,
            &mut nodes,
        )?;
        for (scope, body) in &mut repaired.subgraphs {
            repair_scope(
                Some(scope.clone()),
                &mut body.nodes,
                &body.edges,
                graph,
                &mut nodes,
            )?;
        }
        if nodes.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            version: 1,
            original: graph.identity().map_err(|_| BindingRepairError)?,
            repaired: repaired.identity().map_err(|_| BindingRepairError)?,
            nodes,
        }))
    }
    /// Original materialization identity, before any child-only repair.
    pub fn original(&self) -> ContentHash {
        self.original
    }
    /// Exact repaired materialization identity, persisted before fork effects.
    pub fn repaired(&self) -> ContentHash {
        self.repaired
    }
    /// Recompute the complete derivation, never trust stored hashes alone.
    /// # Errors
    /// Any mismatch in source identity, paths, bindings or generated topology.
    pub fn apply(&self, graph: &Graph) -> Result<Graph, BindingRepairError> {
        if graph.identity().map_err(|_| BindingRepairError)? != self.original
            || Self::prepare(graph)?.as_ref() != Some(self)
        {
            return Err(BindingRepairError);
        }
        let mut result = graph.clone();
        for entry in &self.nodes {
            let retry = scope_nodes(&result, entry.scope.as_ref())?
                .get(&entry.retry_target)
                .and_then(|node| {
                    if let NodeConfig::Agent(agent) = &node.config {
                        Some(agent.clone())
                    } else {
                        None
                    }
                })
                .ok_or(BindingRepairError)?;
            let splitter = scope_nodes_mut(&mut result, entry.scope.as_ref())?
                .get_mut(&entry.node)
                .and_then(|node| {
                    if let NodeConfig::Agent(agent) = &mut node.config {
                        Some(agent)
                    } else {
                        None
                    }
                })
                .ok_or(BindingRepairError)?;
            inherit_explicit_bindings(splitter, &retry);
        }
        if result.identity().map_err(|_| BindingRepairError)? != self.repaired {
            return Err(BindingRepairError);
        }
        Ok(result)
    }
    /// Independently reverse and rederive a registered child's materialization.
    /// # Errors
    /// Arbitrary graph drift, changed provenance or altered original runtime tables.
    pub fn verify_repaired(&self, graph: &Graph) -> Result<(), BindingRepairError> {
        if graph.identity().map_err(|_| BindingRepairError)? != self.repaired {
            return Err(BindingRepairError);
        }
        let mut original = graph.clone();
        for entry in &self.nodes {
            let Some(Node {
                config: NodeConfig::Agent(agent),
                ..
            }) = scope_nodes_mut(&mut original, entry.scope.as_ref())?.get_mut(&entry.node)
            else {
                return Err(BindingRepairError);
            };
            match &entry.previous_runtime {
                Some(value) => {
                    agent.custom_fields.insert(
                        "runtime".into(),
                        toml::Value::Table(
                            value
                                .iter()
                                .map(|(key, value)| {
                                    (key.clone(), toml::Value::String(value.clone()))
                                })
                                .collect(),
                        ),
                    );
                },
                None => {
                    agent.custom_fields.remove("runtime");
                },
            }
        }
        if self.apply(&original)? != *graph {
            return Err(BindingRepairError);
        }
        Ok(())
    }
}
fn scope_nodes<'a>(
    graph: &'a Graph,
    scope: Option<&SubgraphKey>,
) -> Result<&'a BTreeMap<NodeKey, Node>, BindingRepairError> {
    match scope {
        None => Ok(&graph.nodes),
        Some(key) => graph
            .subgraphs
            .get(key)
            .map(|body| &body.nodes)
            .ok_or(BindingRepairError),
    }
}
fn scope_nodes_mut<'a>(
    graph: &'a mut Graph,
    scope: Option<&SubgraphKey>,
) -> Result<&'a mut BTreeMap<NodeKey, Node>, BindingRepairError> {
    match scope {
        None => Ok(&mut graph.nodes),
        Some(key) => graph
            .subgraphs
            .get_mut(key)
            .map(|body| &mut body.nodes)
            .ok_or(BindingRepairError),
    }
}
fn repair_scope(
    scope: Option<SubgraphKey>,
    nodes: &mut BTreeMap<NodeKey, Node>,
    edges: &[Edge],
    graph: &Graph,
    changes: &mut Vec<RepairedNode>,
) -> Result<(), BindingRepairError> {
    let candidates: Vec<_> = nodes
        .values()
        .filter(|node| is_split_planner(node))
        .map(|node| node.id.clone())
        .collect();
    for id in candidates {
        let target = authenticate_ladder(&id, nodes, edges, graph)?;
        let Some(Node {
            config: NodeConfig::Agent(retry),
            ..
        }) = nodes.get(&target)
        else {
            return Err(BindingRepairError);
        };
        let retry = retry.clone();
        let Some(Node {
            config: NodeConfig::Agent(agent),
            ..
        }) = nodes.get_mut(&id)
        else {
            return Err(BindingRepairError);
        };
        let previous_runtime = agent
            .custom_fields
            .get("runtime")
            .map(|value| {
                value
                    .as_table()
                    .ok_or(BindingRepairError)?
                    .iter()
                    .map(|(name, value)| {
                        Ok((
                            name.clone(),
                            value.as_str().ok_or(BindingRepairError)?.to_owned(),
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>, BindingRepairError>>()
            })
            .transpose()?;
        if let Some(table) = &previous_runtime {
            for (name, value) in table {
                let expected = match name.as_str() {
                    "agent_id" => retry.runtime_override(),
                    "model" => retry.model_override(),
                    _ => return Err(BindingRepairError),
                };
                if Some(value.as_str()) != expected {
                    return Err(BindingRepairError);
                }
            }
        }
        let original = agent.clone();
        inherit_explicit_bindings(agent, &retry);
        if *agent != original {
            changes.push(RepairedNode {
                scope: scope.clone(),
                node: id,
                retry_target: target,
                previous_runtime,
            });
        }
    }
    Ok(())
}
/// Reconstruct the host-generated addition from its capped retry source, and compare every
/// node/edge, including profile, sandbox, declarations and unrelated custom fields.
fn unique(matches: Vec<&Edge>) -> Result<&Edge, BindingRepairError> {
    let [edge] = matches.as_slice() else {
        return Err(BindingRepairError);
    };
    Ok(*edge)
}
fn authenticate_ladder(
    id: &NodeKey,
    nodes: &BTreeMap<NodeKey, Node>,
    edges: &[Edge],
    graph: &Graph,
) -> Result<NodeKey, BindingRepairError> {
    let incoming = unique(edges.iter().filter(|edge| &edge.to == id).collect())?;
    let source = &incoming.from.node;
    if incoming.from.outcome.as_str() != ESCALATION_EXHAUSTED_OUTCOME {
        return Err(BindingRepairError);
    }
    let alternate = unique(
        edges
            .iter()
            .filter(|edge| &edge.from.node == source && is_alternate_attempt(edge))
            .collect(),
    )?;
    let target = alternate.to.clone();
    let gate = &unique(
        edges
            .iter()
            .filter(|edge| {
                &edge.from.node == id && edge.from.outcome.as_str() == CANNOT_SPLIT_OUTCOME
            })
            .collect(),
    )?
    .to;
    let replaced = &unique(
        edges
            .iter()
            .filter(|edge| &edge.from.node == id && edge.from.outcome.as_str() == SPLIT_OUTCOME)
            .collect(),
    )?
    .to;
    let stopped = &unique(
        edges
            .iter()
            .filter(|edge| &edge.from.node == gate && edge.from.outcome.as_str() == STOP_OUTCOME)
            .collect(),
    )?
    .to;
    let generated = [id, gate, replaced, stopped];
    let generated_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            generated.contains(&&edge.from.node)
                || (&edge.from.node == source
                    && ESCALATION_RUNGS.contains(&edge.from.outcome.as_str()))
        })
        .collect();
    let loop_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            &edge.from.node == source
                && edge.policy.max_traversals.is_some()
                && !is_alternate_attempt(edge)
                && edge.policy.on_max_exceeded == ExceededAction::Escalate
                && !ESCALATION_RUNGS.contains(&edge.from.outcome.as_str())
        })
        .cloned()
        .collect();
    if loop_edges.is_empty() || loop_edges.iter().any(|edge| edge.to != target) {
        return Err(BindingRepairError);
    }
    let expected = expected_addition(
        source,
        &target,
        &loop_edges,
        &generated,
        &generated_edges,
        graph,
        success_target(nodes, edges, source),
    )?;
    if expected.edges.len() != generated_edges.len()
        || expected
            .edges
            .iter()
            .any(|edge| !generated_edges.contains(&edge))
    {
        return Err(BindingRepairError);
    }
    for expected in expected.nodes {
        let actual = nodes.get(&expected.id).ok_or(BindingRepairError)?;
        let mut actual = actual.clone();
        if &expected.id == id
            && let NodeConfig::Agent(agent) = &mut actual.config
        {
            agent.custom_fields.remove("runtime");
        }
        if actual != expected {
            return Err(BindingRepairError);
        }
    }
    Ok(target)
}

fn expected_addition(
    source: &NodeKey,
    target: &NodeKey,
    loop_edges: &[Edge],
    generated: &[&NodeKey],
    generated_edges: &[&Edge],
    graph: &Graph,
    accept_to: Option<NodeKey>,
) -> Result<Addition, BindingRepairError> {
    let mut names = Names {
        nodes: graph
            .nodes
            .keys()
            .chain(graph.subgraphs.values().flat_map(|body| body.nodes.keys()))
            .filter(|key| !generated.contains(key))
            .cloned()
            .collect(),
        edges: graph
            .edges
            .iter()
            .chain(graph.subgraphs.values().flat_map(|body| &body.edges))
            .filter(|edge| !generated_edges.contains(edge))
            .map(|edge| edge.id.clone())
            .collect(),
    };
    gate_for(
        source,
        target,
        loop_edges,
        Ladder {
            alternate: true,
            split: true,
            accept_to,
        },
        &mut names,
    )
    .ok_or(BindingRepairError)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_generated_splitters_require_an_exact_child_repair() {
        let mut graph = crate::bundled_flows::BundledFlows::by_name_latest("multi-milestone")
            .unwrap()
            .graph;
        for scope in graph.subgraphs.values_mut() {
            for node in scope.nodes.values_mut() {
                if let crate::node::NodeConfig::Agent(agent) = &mut node.config {
                    *agent = agent.with_runtime_override("codex-acp", Some("gpt-6.1-sol"));
                }
            }
        }
        let mut retained = super::super::with_default_escalation_gates(&graph);
        for scope in retained.subgraphs.values_mut() {
            for node in scope.nodes.values_mut() {
                if super::super::is_split_planner(node)
                    && let crate::node::NodeConfig::Agent(agent) = &mut node.config
                {
                    agent.custom_fields.remove("runtime");
                }
            }
        }
        assert!(
            GeneratedBindingRepair::prepare(&retained)
                .unwrap()
                .is_some()
        );
    }
}
