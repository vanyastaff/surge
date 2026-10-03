//! Bounded journal reconstruction of the executor's routing scope and counters.
use super::StorageError;
use std::collections::HashMap;
use surge_core::edge::EdgeKind;
use surge_core::keys::{EdgeKey, SubgraphKey};
use surge_core::{EventPayload, Graph, NodeConfig, NodeKey, OutcomeKey};

#[derive(Clone)]
enum Frame {
    Loop {
        node: NodeKey,
        body: SubgraphKey,
        counts: HashMap<EdgeKey, u32>,
    },
    Subgraph {
        node: NodeKey,
        body: SubgraphKey,
    },
}

#[derive(Clone, Default)]
pub(super) struct RouteEvidence {
    frames: Vec<Frame>,
    root_counts: HashMap<EdgeKey, u32>,
    pending: Option<(NodeKey, NodeKey, EdgeKey, EdgeKind)>,
    active_node: Option<NodeKey>,
}

impl RouteEvidence {
    pub(super) fn observe(
        &mut self,
        graph: &Graph,
        event: &EventPayload,
    ) -> Result<(), StorageError> {
        match event {
            EventPayload::StageEntered { node, .. } => self.active_node = Some(node.clone()),
            EventPayload::LoopIterationStarted { loop_id, .. } => {
                self.enter_loop(graph, loop_id)?;
            },
            EventPayload::LoopCompleted {
                loop_id,
                completed_iterations,
                final_outcome,
            } => {
                if matches!(self.frames.last(),Some(Frame::Loop {node,..}) if node==loop_id) {
                    self.frames.pop();
                } else {
                    // Empty iterable completion never creates a runtime frame.
                    let accepted = self.scope_node(graph, loop_id)?;
                    if *completed_iterations != 0
                        || final_outcome.as_str() != "loop_empty"
                        || self.active_node.as_ref() != Some(loop_id)
                        || !matches!(&accepted.config, NodeConfig::Loop(_))
                    {
                        return Err(invalid("loop completion has no matching routing scope"));
                    }
                }
            },
            EventPayload::SubgraphEntered { outer, inner } => {
                let node = self.scope_node(graph, outer)?;
                if self.active_node.as_ref() != Some(outer)
                    || !matches!(&node.config,NodeConfig::Subgraph(config) if config.inner==*inner)
                    || !graph.subgraphs.contains_key(inner)
                {
                    return Err(invalid("subgraph scope contradicts accepted graph"));
                }
                self.frames.push(Frame::Subgraph {
                    node: outer.clone(),
                    body: inner.clone(),
                });
            },
            EventPayload::SubgraphExited { outer, inner, .. } => match self.frames.pop() {
                Some(Frame::Subgraph { node, body }) if node == *outer && body == *inner => {},
                _ => return Err(invalid("subgraph completion has no matching routing scope")),
            },
            EventPayload::EdgeTraversed {
                from,
                to,
                edge: edge_id,
                kind,
            } => {
                if self.pending.is_some() {
                    return Err(invalid("routing edge has no adjacent stage completion"));
                }
                self.pending = Some((from.clone(), to.clone(), edge_id.clone(), *kind));
            },
            EventPayload::StageCompleted { node, outcome, .. } => {
                if let Some((from, to, edge, kind)) = self.pending.take() {
                    if from != *node {
                        return Err(invalid("routing edge contradicts its stage completion"));
                    }
                    self.validate_route(graph, node, outcome, &to, &edge, kind)?;
                }
            },
            _ => {},
        }
        Ok(())
    }

    fn enter_loop(&mut self, graph: &Graph, node: &NodeKey) -> Result<(), StorageError> {
        if matches!(self.frames.last(),Some(Frame::Loop {node:current,..}) if current==node) {
            // Iteration/retry keeps the original captured body and traversal counters.
            return Ok(());
        }
        let accepted = self.scope_node(graph, node)?;
        let NodeConfig::Loop(config) = &accepted.config else {
            return Err(invalid("loop scope contradicts accepted node kind"));
        };
        if self.active_node.as_ref() != Some(node) {
            return Err(invalid(
                "loop scope contradicts its active stage occurrence",
            ));
        }
        if !graph.subgraphs.contains_key(&config.body) {
            return Err(invalid("loop scope has no accepted body"));
        }
        self.frames.push(Frame::Loop {
            node: node.clone(),
            body: config.body.clone(),
            counts: HashMap::new(),
        });
        Ok(())
    }

    fn scope_node<'a>(
        &self,
        graph: &'a Graph,
        node: &NodeKey,
    ) -> Result<&'a surge_core::Node, StorageError> {
        let body = self.frames.last().map(|frame| match frame {
            Frame::Loop { body, .. } | Frame::Subgraph { body, .. } => body,
        });
        let nodes = if let Some(body) = body {
            &graph
                .subgraphs
                .get(body)
                .ok_or_else(|| invalid("captured routing body is missing"))?
                .nodes
        } else {
            &graph.nodes
        };
        nodes
            .get(node)
            .ok_or_else(|| invalid("routing scope has no accepted node in its active body"))
    }

    fn validate_route(
        &mut self,
        graph: &Graph,
        node: &NodeKey,
        outcome: &OutcomeKey,
        to: &NodeKey,
        edge: &EdgeKey,
        kind: EdgeKind,
    ) -> Result<(), StorageError> {
        let body = self.frames.last().map(|frame| match frame {
            Frame::Loop { body, .. } | Frame::Subgraph { body, .. } => body,
        });
        let edges = if let Some(body) = body {
            &graph
                .subgraphs
                .get(body)
                .ok_or_else(|| invalid("captured routing body is missing"))?
                .edges
        } else {
            &graph.edges
        };
        // Match the executor exactly: only the top frame being Loop owns counts.
        // A top Subgraph uses root counts even if a Loop exists below it.
        let counts = match self.frames.last_mut() {
            Some(Frame::Loop { counts, .. }) => counts,
            _ => &mut self.root_counts,
        };
        let mut prepared = counts.clone();
        let expected =
            surge_core::route_selection::resolve_stage_route(edges, node, outcome, &mut prepared)
                .map_err(|error| {
                invalid(&format!(
                    "declared routing could not be established: {error}"
                ))
            })?;
        if expected.target != *to || expected.edge_id != *edge || expected.kind != kind {
            return Err(invalid(
                "routing edge contradicts accepted graph and traversal policy",
            ));
        }
        *counts = prepared;
        Ok(())
    }
}

fn invalid(message: &str) -> StorageError {
    StorageError::MigrationFailed(message.into())
}
