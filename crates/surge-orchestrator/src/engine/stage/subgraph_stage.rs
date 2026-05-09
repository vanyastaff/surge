//! `NodeKind::Subgraph` stage execution — frame push at entry, output
//! projection at exit. Single-threaded per spec §6.5-6.6.

use crate::engine::frames::{Frame, ResolvedSubgraphInput, SubgraphFrame};
use crate::engine::stage::StageError;
use surge_core::graph::Graph;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_core::subgraph_config::SubgraphConfig;
use surge_persistence::runs::run_writer::RunWriter;

/// Parameters for `execute_subgraph_entry`.
pub struct SubgraphStageParams<'a> {
    /// `NodeKey` of the outer Subgraph node.
    pub node: &'a NodeKey,
    /// Subgraph configuration from the node.
    pub subgraph_config: &'a SubgraphConfig,
    /// Frozen pipeline graph (used for inner subgraph lookup).
    pub graph: &'a Graph,
    /// Run memory (used for input binding resolution).
    pub run_memory: &'a RunMemory,
    /// Run writer for persisting events.
    pub writer: &'a RunWriter,
    /// Mutable frame stack — `SubgraphFrame` pushed.
    pub frames: &'a mut Vec<Frame>,
    /// Outer-graph node to advance to when the subgraph exits.
    pub return_to: NodeKey,
}

/// Outcome of `execute_subgraph_entry`. The cursor must advance to `inner_start`.
pub struct SubgraphEntryEffect {
    /// Inner subgraph's `start` `NodeKey` — caller sets cursor to this.
    pub inner_start: NodeKey,
}

/// Resolve subgraph input bindings, push a `SubgraphFrame`, emit
/// `SubgraphEntered`, and return the inner subgraph's start node.
pub async fn execute_subgraph_entry(
    p: SubgraphStageParams<'_>,
) -> Result<SubgraphEntryEffect, StageError> {
    let inner = p
        .graph
        .subgraphs
        .get(&p.subgraph_config.inner)
        .ok_or_else(|| StageError::SubgraphMissing(p.subgraph_config.inner.clone()))?;

    let bound_inputs = resolve_subgraph_inputs(&p.subgraph_config.inputs, p.run_memory)?;

    p.frames.push(Frame::Subgraph(SubgraphFrame {
        outer_node: p.node.clone(),
        inner_subgraph: p.subgraph_config.inner.clone(),
        bound_inputs,
        return_to: p.return_to,
    }));

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::SubgraphEntered {
            outer: p.node.clone(),
            inner: p.subgraph_config.inner.clone(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(SubgraphEntryEffect {
        inner_start: inner.start.clone(),
    })
}

fn resolve_subgraph_inputs(
    inputs: &[surge_core::subgraph_config::SubgraphInput],
    memory: &RunMemory,
) -> Result<Vec<ResolvedSubgraphInput>, StageError> {
    inputs
        .iter()
        .map(|i| {
            let value = resolve_artifact_source(&i.outer_binding.source, memory)?;
            Ok(ResolvedSubgraphInput {
                inner_var: i.inner_var.clone(),
                value,
            })
        })
        .collect()
}

fn resolve_artifact_source(
    src: &surge_core::agent_config::ArtifactSource,
    memory: &RunMemory,
) -> Result<serde_json::Value, StageError> {
    use surge_core::agent_config::ArtifactSource;
    match src {
        ArtifactSource::NodeOutput { node, artifact } => {
            let aref = memory
                .artifacts_by_node
                .get(node)
                .and_then(|list| list.iter().find(|a| a.name == *artifact))
                .ok_or_else(|| {
                    StageError::Internal(format!(
                        "artifact '{artifact}' not produced by node '{node}'"
                    ))
                })?;
            Ok(serde_json::json!({
                "path": aref.path.to_string_lossy(),
                "hash": aref.hash.to_string(),
            }))
        },
        ArtifactSource::RunArtifact { name } => {
            let aref = memory.artifacts.get(name).ok_or_else(|| {
                StageError::Internal(format!("run artifact '{name}' not in RunMemory"))
            })?;
            Ok(serde_json::json!({
                "path": aref.path.to_string_lossy(),
                "hash": aref.hash.to_string(),
            }))
        },
        ArtifactSource::GlobPattern {
            node: _,
            pattern: _,
        } => Err(StageError::Internal(
            "ArtifactSource::GlobPattern not yet implemented in M6 (M7+)".into(),
        )),
        ArtifactSource::Static { content } => Ok(serde_json::Value::String(content.clone())),
        ArtifactSource::InitialPrompt => {
            let aref = memory.artifacts.get("user_prompt").ok_or_else(|| {
                StageError::Internal(
                    "ArtifactSource::InitialPrompt requested but no \"user_prompt\" artifact \
                     present in RunMemory (engine seed not yet wired)"
                        .into(),
                )
            })?;
            Ok(serde_json::json!({
                "path": aref.path.to_string_lossy(),
                "hash": aref.hash.to_string(),
            }))
        },
        ArtifactSource::EditFeedback { from_node: _ } => Err(StageError::Internal(
            "ArtifactSource::EditFeedback resolution not yet wired (bootstrap milestone)".into(),
        )),
    }
}

/// Called by `run_task::execute` when the cursor reaches a `Terminal`
/// node and the top frame is a `SubgraphFrame` (`TerminalSignal::SubgraphDone`).
/// Projects the inner outcome to an outer outcome via
/// `SubgraphConfig::outputs` (first match wins), pops the frame, and
/// resumes the outer cursor at the frame's `return_to`.
pub async fn on_subgraph_done(
    outputs: &[surge_core::subgraph_config::SubgraphOutput],
    memory: &RunMemory,
    frames: &mut Vec<Frame>,
    cursor: &mut surge_core::run_state::Cursor,
    writer: &RunWriter,
) -> Result<(), StageError> {
    // Snapshot frame fields BEFORE the mutable pop (avoid double-borrow).
    let (outer_node, inner_subgraph, return_to) = match frames.last() {
        Some(Frame::Subgraph(sf)) => (
            sf.outer_node.clone(),
            sf.inner_subgraph.clone(),
            sf.return_to.clone(),
        ),
        _ => {
            return Err(StageError::Internal(
                "on_subgraph_done called without Subgraph frame on top".into(),
            ));
        },
    };

    // First-match output projection.
    let outcome = outputs
        .iter()
        .find_map(|o| project_output(o, memory).ok())
        .ok_or_else(|| {
            StageError::Internal(format!(
                "no SubgraphConfig::outputs entry resolved successfully for subgraph \
                 {inner_subgraph}"
            ))
        })?;

    writer
        .append_event(VersionedEventPayload::new(EventPayload::SubgraphExited {
            outer: outer_node.clone(),
            inner: inner_subgraph.clone(),
            outcome: outcome.clone(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: outer_node,
            outcome,
            summary: format!("subgraph {inner_subgraph} completed"),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    frames.pop();
    cursor.node = return_to;
    cursor.attempt = 1;

    Ok(())
}

fn project_output(
    out: &surge_core::subgraph_config::SubgraphOutput,
    memory: &RunMemory,
) -> Result<surge_core::keys::OutcomeKey, StageError> {
    // Resolve the inner_artifact to verify it exists. If yes, the
    // configured outer_outcome is the projection.
    let _ = resolve_artifact_source(&out.inner_artifact, memory)?;
    Ok(out.outer_outcome.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::agent_config::{ArtifactSource, TemplateVar};
    use surge_core::graph::{GraphMetadata, SCHEMA_VERSION, Subgraph};
    use surge_core::keys::{NodeKey, OutcomeKey, SubgraphKey};
    use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
    use surge_core::subgraph_config::{SubgraphInput, SubgraphOutput};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_persistence::runs::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn entry_pushes_subgraph_frame_and_advances_to_inner_start() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let outer_key = NodeKey::try_from("sg_1").unwrap();
        let inner_key = SubgraphKey::try_from("review_block").unwrap();
        let inner_start = NodeKey::try_from("inner_start").unwrap();

        let inner_node = Node {
            id: inner_start.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        };
        let mut inner_nodes = std::collections::BTreeMap::new();
        inner_nodes.insert(inner_start.clone(), inner_node);

        let mut subgraphs = std::collections::BTreeMap::new();
        subgraphs.insert(
            inner_key.clone(),
            Subgraph {
                start: inner_start.clone(),
                nodes: inner_nodes,
                edges: vec![],
            },
        );

        let outer_node = Node {
            id: outer_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "ok".into(),
                edge_kind_hint: surge_core::edge::EdgeKind::Forward,
                is_terminal: false,
            }],
            config: NodeConfig::Subgraph(SubgraphConfig {
                inner: inner_key.clone(),
                inputs: vec![],
                outputs: vec![SubgraphOutput {
                    inner_artifact: ArtifactSource::Static {
                        content: "ok".into(),
                    },
                    outer_outcome: OutcomeKey::try_from("done").unwrap(),
                }],
            }),
        };
        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(outer_key.clone(), outer_node);

        let graph = surge_core::graph::Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: outer_key.clone(),
            nodes,
            edges: vec![],
            subgraphs,
        };

        let cfg = match &graph.nodes[&outer_key].config {
            NodeConfig::Subgraph(c) => c.clone(),
            _ => unreachable!(),
        };
        let memory = RunMemory::default();
        let mut frames: Vec<Frame> = vec![];

        let effect = execute_subgraph_entry(SubgraphStageParams {
            node: &outer_key,
            subgraph_config: &cfg,
            graph: &graph,
            run_memory: &memory,
            writer: &writer,
            frames: &mut frames,
            return_to: NodeKey::try_from("after").unwrap(),
        })
        .await
        .unwrap();

        assert_eq!(effect.inner_start, inner_start);
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            Frame::Subgraph(sf) => {
                assert_eq!(sf.outer_node, outer_key);
                assert_eq!(sf.inner_subgraph, inner_key);
            },
            Frame::Loop(_) => panic!("expected Subgraph frame"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_subgraph_reference_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let outer_key = NodeKey::try_from("sg_1").unwrap();
        let missing_inner = SubgraphKey::try_from("does_not_exist").unwrap();

        let cfg = SubgraphConfig {
            inner: missing_inner.clone(),
            inputs: vec![],
            outputs: vec![],
        };

        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(
            outer_key.clone(),
            Node {
                id: outer_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Subgraph(cfg.clone()),
            },
        );

        let graph = surge_core::graph::Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: outer_key.clone(),
            nodes,
            edges: vec![],
            subgraphs: std::collections::BTreeMap::new(),
        };

        let memory = RunMemory::default();
        let mut frames: Vec<Frame> = vec![];

        let result = execute_subgraph_entry(SubgraphStageParams {
            node: &outer_key,
            subgraph_config: &cfg,
            graph: &graph,
            run_memory: &memory,
            writer: &writer,
            frames: &mut frames,
            return_to: NodeKey::try_from("after").unwrap(),
        })
        .await;

        assert!(matches!(result, Err(StageError::SubgraphMissing(k)) if k == missing_inner));
    }

    #[test]
    fn resolve_artifact_source_static_returns_string_value() {
        let memory = RunMemory::default();
        let src = surge_core::agent_config::ArtifactSource::Static {
            content: "hello".into(),
        };
        let result = resolve_artifact_source(&src, &memory).unwrap();
        assert_eq!(result, serde_json::Value::String("hello".into()));
    }

    #[test]
    fn resolve_artifact_source_glob_pattern_returns_error() {
        let memory = RunMemory::default();
        let src = surge_core::agent_config::ArtifactSource::GlobPattern {
            node: NodeKey::try_from("n1").unwrap(),
            pattern: "*.md".into(),
        };
        let result = resolve_artifact_source(&src, &memory);
        assert!(matches!(result, Err(StageError::Internal(_))));
    }

    #[test]
    fn resolve_subgraph_inputs_empty_slice_returns_empty_vec() {
        let memory = RunMemory::default();
        let result = resolve_subgraph_inputs(&[], &memory).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_subgraph_inputs_static_binding_succeeds() {
        let memory = RunMemory::default();
        let inputs = vec![SubgraphInput {
            outer_binding: surge_core::agent_config::Binding {
                source: ArtifactSource::Static {
                    content: "val".into(),
                },
                target: TemplateVar("unused".into()),
            },
            inner_var: TemplateVar("x".into()),
        }];
        let result = resolve_subgraph_inputs(&inputs, &memory).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].inner_var, TemplateVar("x".into()));
        assert_eq!(result[0].value, serde_json::Value::String("val".into()));
    }

    use surge_core::run_state::Cursor;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_pops_frame_and_projects_first_matching_output() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let outer_key = NodeKey::try_from("sg_1").unwrap();
        let inner_key = SubgraphKey::try_from("review_block").unwrap();
        let return_to = NodeKey::try_from("after").unwrap();

        // RunMemory has the inner artifact registered.
        let inner_artifact_path = dir.path().join("review.md");
        std::fs::write(&inner_artifact_path, "approved").unwrap();
        let mut memory = RunMemory::default();
        let aref = surge_core::run_state::ArtifactRef {
            hash: surge_core::content_hash::ContentHash::compute(b"approved"),
            path: inner_artifact_path,
            name: "review.md".into(),
            produced_by: NodeKey::try_from("review_inner").unwrap(),
            produced_at_seq: 1,
        };
        memory.artifacts.insert("review.md".into(), aref.clone());
        memory
            .artifacts_by_node
            .entry(NodeKey::try_from("review_inner").unwrap())
            .or_default()
            .push(aref);

        // Frame stack with one Subgraph frame.
        let mut frames: Vec<Frame> = vec![Frame::Subgraph(SubgraphFrame {
            outer_node: outer_key.clone(),
            inner_subgraph: inner_key.clone(),
            bound_inputs: vec![],
            return_to: return_to.clone(),
        })];

        // Outputs: first match wins. Configure a single matching output.
        let outputs = vec![SubgraphOutput {
            inner_artifact: ArtifactSource::NodeOutput {
                node: NodeKey::try_from("review_inner").unwrap(),
                artifact: "review.md".into(),
            },
            outer_outcome: OutcomeKey::try_from("approved").unwrap(),
        }];

        let mut cursor = Cursor {
            node: NodeKey::try_from("inner_terminal").unwrap(),
            attempt: 1,
        };

        on_subgraph_done(&outputs, &memory, &mut frames, &mut cursor, &writer)
            .await
            .unwrap();

        assert!(frames.is_empty(), "frame popped");
        assert_eq!(cursor.node, return_to, "cursor restored to return_to");
    }
}
