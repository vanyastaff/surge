//! `NodeKind::Loop` stage execution — frame-push, iteration boundary,
//! exit-condition handling. Single-threaded per spec §6.3-6.4.

use crate::engine::frames::{
    Frame, LoopFrame, MAX_LOOP_ITEMS_RESOLVED, initial_attempts_remaining,
};
use crate::engine::stage::StageError;
use std::collections::HashMap;
use surge_core::graph::Graph;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::loop_config::{IterableSource, LoopConfig};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;

/// Parameters for `execute_loop_entry`.
pub struct LoopStageParams<'a> {
    /// `NodeKey` of the Loop node being entered.
    pub node: &'a NodeKey,
    /// Loop configuration from the node.
    pub loop_config: &'a LoopConfig,
    /// Frozen pipeline graph (used for body subgraph lookup).
    pub graph: &'a Graph,
    /// In-progress run memory (artifacts / outcomes used to resolve `IterableSource::Artifact`).
    pub run_memory: &'a RunMemory,
    /// Run writer for persisting events.
    pub writer: &'a RunWriter,
    /// Mutable frame stack — frame pushed on non-empty entry.
    pub frames: &'a mut Vec<Frame>,
    /// Outer-graph node to advance to when the loop completes.
    /// Caller computes via routing's `edge_target_after_outcome_or_default`.
    pub return_to: NodeKey,
}

/// Outcome of executing a Loop entry stage.
#[derive(Debug)]
pub enum LoopEntryEffect {
    /// Empty iterable — frame NOT pushed; run loop routes via this outcome.
    Skipped(OutcomeKey),
    /// Frame pushed; run loop must advance cursor to this body-subgraph start.
    Entered(NodeKey),
}

/// Resolve the iterable + push a `LoopFrame` if non-empty. See task description.
pub async fn execute_loop_entry(p: LoopStageParams<'_>) -> Result<LoopEntryEffect, StageError> {
    let body_subgraph = p
        .graph
        .subgraphs
        .get(&p.loop_config.body)
        .ok_or_else(|| StageError::LoopBodyMissing(p.loop_config.body.clone()))?;

    let items = resolve_iterable(&p.loop_config.iterates_over, p.run_memory).await?;

    if items.len() > MAX_LOOP_ITEMS_RESOLVED {
        return Err(StageError::LoopItemsTooLarge {
            count: u32::try_from(items.len()).unwrap_or(u32::MAX),
            // MAX_LOOP_ITEMS_RESOLVED = 1000 always fits in u32.
            #[allow(clippy::cast_possible_truncation)]
            max: MAX_LOOP_ITEMS_RESOLVED as u32,
        });
    }

    if items.is_empty() {
        let outcome = OutcomeKey::try_from("loop_empty")
            .map_err(|e| StageError::Internal(format!("'loop_empty' key: {e}")))?;
        p.writer
            .append_event(VersionedEventPayload::new(EventPayload::LoopCompleted {
                loop_id: p.node.clone(),
                completed_iterations: 0,
                final_outcome: outcome.clone(),
            }))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        return Ok(LoopEntryEffect::Skipped(outcome));
    }

    let body_start = body_subgraph.start.clone();

    p.frames.push(Frame::Loop(LoopFrame {
        loop_node: p.node.clone(),
        config: p.loop_config.clone(),
        items: items.clone(),
        current_index: 0,
        attempts_remaining: initial_attempts_remaining(&p.loop_config.on_iteration_failure),
        return_to: p.return_to,
        traversal_counts: HashMap::new(),
    }));

    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::LoopIterationStarted {
                loop_id: p.node.clone(),
                item: items[0].clone(),
                index: 0,
            },
        ))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(LoopEntryEffect::Entered(body_start))
}

async fn resolve_iterable(
    src: &IterableSource,
    memory: &RunMemory,
) -> Result<Vec<toml::Value>, StageError> {
    match src {
        IterableSource::Static(items) => Ok(items.clone()),
        IterableSource::Artifact {
            node: _,
            name,
            jsonpath,
        } => {
            let artifact = memory.artifacts.get(name).ok_or_else(|| {
                StageError::Internal(format!("artifact '{name}' not in RunMemory"))
            })?;

            let bytes = tokio::fs::read(&artifact.path).await.map_err(|e| {
                StageError::Internal(format!("read artifact {}: {e}", artifact.path.display()))
            })?;

            // M6 supports TOML artifacts with a simple dotted path.
            // (JSON support could be added later; current Surge artifacts
            // are TOML by convention — see CLAUDE.md.)
            let content = std::str::from_utf8(&bytes).map_err(|e| {
                StageError::Internal(format!(
                    "artifact {} not utf8: {e}",
                    artifact.path.display()
                ))
            })?;
            let parsed: toml::Value = toml::from_str(content).map_err(|e| {
                StageError::Internal(format!("toml parse {}: {e}", artifact.path.display()))
            })?;

            // Walk the dotted path.
            let mut cursor = &parsed;
            for segment in jsonpath.split('.') {
                cursor = cursor.get(segment).ok_or_else(|| {
                    StageError::Internal(format!(
                        "path segment '{segment}' not found in {jsonpath}"
                    ))
                })?;
            }

            match cursor {
                toml::Value::Array(arr) => Ok(arr.clone()),
                other => Err(StageError::Internal(format!(
                    "path {jsonpath} resolved to non-array: {other:?}"
                ))),
            }
        },
    }
}

/// Called by `run_task::execute` when the cursor reaches a `Terminal`
/// node and the top frame is a `LoopFrame` (`TerminalSignal::LoopIterDone`).
/// Decides whether to advance to the next iteration, retry the same
/// iteration (per `FailurePolicy`), or pop the frame and return to the
/// outer cursor.
#[allow(clippy::too_many_lines)]
pub async fn on_loop_iteration_done(
    just_completed_outcome: &OutcomeKey,
    graph: &Graph,
    frames: &mut Vec<Frame>,
    cursor: &mut surge_core::run_state::Cursor,
    writer: &RunWriter,
) -> Result<(), StageError> {
    let Some(Frame::Loop(lf)) = frames.last_mut() else {
        return Err(StageError::Internal(
            "on_loop_iteration_done called without Loop frame on top".into(),
        ));
    };

    // Persist the per-iteration completion event.
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::LoopIterationCompleted {
                loop_id: lf.loop_node.clone(),
                index: lf.current_index,
                outcome: just_completed_outcome.clone(),
            },
        ))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    // 1. Iteration-failure handling.
    if is_failure_outcome(just_completed_outcome) {
        match lf.config.on_iteration_failure.clone() {
            surge_core::loop_config::FailurePolicy::Abort => {
                let loop_node = lf.loop_node.clone();
                let completed_iterations = lf.current_index + 1;
                let return_to = lf.return_to.clone();
                exit_loop(
                    loop_node,
                    completed_iterations,
                    return_to,
                    frames,
                    cursor,
                    "aborted",
                    writer,
                )
                .await?;
                return Ok(());
            },
            surge_core::loop_config::FailurePolicy::Skip => {
                // fall through to advance-index
            },
            surge_core::loop_config::FailurePolicy::Retry { .. } if lf.attempts_remaining > 0 => {
                lf.attempts_remaining -= 1;
                let body_start = body_subgraph_start(graph, lf)?;
                let item = lf.items[lf.current_index as usize].clone();
                let loop_id = lf.loop_node.clone();
                let index = lf.current_index;
                cursor.node = body_start;
                cursor.attempt += 1;
                writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::LoopIterationStarted {
                            loop_id,
                            item,
                            index,
                        },
                    ))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                return Ok(());
            },
            surge_core::loop_config::FailurePolicy::Retry { .. } => {
                // attempts exhausted — treat as Abort
                let loop_node = lf.loop_node.clone();
                let completed_iterations = lf.current_index + 1;
                let return_to = lf.return_to.clone();
                exit_loop(
                    loop_node,
                    completed_iterations,
                    return_to,
                    frames,
                    cursor,
                    "aborted",
                    writer,
                )
                .await?;
                return Ok(());
            },
            surge_core::loop_config::FailurePolicy::Replan => {
                tracing::warn!(
                    loop_id = %lf.loop_node,
                    "FailurePolicy::Replan not implemented in M6 — treating as Abort (Replan needs bootstrap from M8)"
                );
                let loop_node = lf.loop_node.clone();
                let completed_iterations = lf.current_index + 1;
                let return_to = lf.return_to.clone();
                exit_loop(
                    loop_node,
                    completed_iterations,
                    return_to,
                    frames,
                    cursor,
                    "aborted",
                    writer,
                )
                .await?;
                return Ok(());
            },
        }
    }

    // 2. Exit-condition check (only reached on success or Skip path).
    {
        let Some(Frame::Loop(lf)) = frames.last() else {
            unreachable!("frame was on stack at failure check")
        };
        if exit_condition_met(lf, just_completed_outcome) {
            let loop_node = lf.loop_node.clone();
            let completed_iterations = lf.current_index + 1;
            let return_to = lf.return_to.clone();
            exit_loop(
                loop_node,
                completed_iterations,
                return_to,
                frames,
                cursor,
                "completed",
                writer,
            )
            .await?;
            return Ok(());
        }
    }

    // 3. Advance to next iteration.
    let Some(Frame::Loop(lf)) = frames.last_mut() else {
        unreachable!("frame was on stack at exit-condition check")
    };
    lf.current_index += 1;

    #[allow(clippy::cast_possible_truncation)] // items bounded by MAX_LOOP_ITEMS_RESOLVED (1000)
    if lf.current_index >= lf.items.len() as u32 {
        let loop_node = lf.loop_node.clone();
        let completed_iterations = lf.current_index + 1;
        let return_to = lf.return_to.clone();
        exit_loop(
            loop_node,
            completed_iterations,
            return_to,
            frames,
            cursor,
            "completed",
            writer,
        )
        .await?;
        return Ok(());
    }

    let body_start = body_subgraph_start(graph, lf)?;
    let item = lf.items[lf.current_index as usize].clone();
    let loop_id = lf.loop_node.clone();
    let index = lf.current_index;
    cursor.node = body_start;
    cursor.attempt = 1;

    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::LoopIterationStarted {
                loop_id,
                item,
                index,
            },
        ))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(())
}

/// Returns `true` for outcomes that count as iteration failures for
/// `FailurePolicy` purposes. Conservative literal match — authors who
/// want richer failure semantics declare an explicit `Branch` node
/// before the loop.
fn is_failure_outcome(outcome: &OutcomeKey) -> bool {
    matches!(outcome.as_ref(), "failed" | "fail" | "error")
}

fn exit_condition_met(lf: &LoopFrame, just_completed: &OutcomeKey) -> bool {
    use surge_core::loop_config::ExitCondition;
    match &lf.config.exit_condition {
        #[allow(clippy::cast_possible_truncation)]
        // items bounded by MAX_LOOP_ITEMS_RESOLVED (1000)
        ExitCondition::AllItems => lf.current_index + 1 >= lf.items.len() as u32,
        ExitCondition::UntilOutcome {
            from_node: _,
            outcome,
        } => just_completed == outcome,
        ExitCondition::MaxIterations { n } => lf.current_index + 1 >= *n,
    }
}

async fn exit_loop(
    loop_node: NodeKey,
    completed_iterations: u32,
    return_to: NodeKey,
    frames: &mut Vec<Frame>,
    cursor: &mut surge_core::run_state::Cursor,
    final_outcome_str: &str,
    writer: &RunWriter,
) -> Result<(), StageError> {
    let final_outcome = OutcomeKey::try_from(final_outcome_str)
        .map_err(|e| StageError::Internal(format!("'{final_outcome_str}' outcome key: {e}")))?;
    writer
        .append_event(VersionedEventPayload::new(EventPayload::LoopCompleted {
            loop_id: loop_node,
            completed_iterations,
            final_outcome,
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;
    frames.pop();
    cursor.node = return_to;
    cursor.attempt = 1;
    Ok(())
}

fn body_subgraph_start(graph: &Graph, lf: &LoopFrame) -> Result<NodeKey, StageError> {
    Ok(graph
        .subgraphs
        .get(&lf.config.body)
        .ok_or_else(|| StageError::LoopBodyMissing(lf.config.body.clone()))?
        .start
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::graph::{GraphMetadata, SCHEMA_VERSION, Subgraph};
    use surge_core::keys::{NodeKey, SubgraphKey};
    use surge_core::loop_config::{ExitCondition, FailurePolicy, ParallelismMode};
    use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_persistence::runs::Storage;

    fn graph_with_loop_body(items: Vec<toml::Value>) -> (Graph, LoopConfig, NodeKey) {
        let loop_key = NodeKey::try_from("loop_1").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("body_start").unwrap();

        let body_node = Node {
            id: body_start.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        };

        let cfg = LoopConfig {
            iterates_over: IterableSource::Static(items),
            body: body_key.clone(),
            iteration_var_name: "item".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Abort,
            parallelism: ParallelismMode::Sequential,
            gate_after_each: false,
        };

        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(
            loop_key.clone(),
            Node {
                id: loop_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![OutcomeDecl {
                    id: OutcomeKey::try_from("completed").unwrap(),
                    description: "ok".into(),
                    edge_kind_hint: surge_core::edge::EdgeKind::Forward,
                    is_terminal: false,
                }],
                config: NodeConfig::Loop(cfg.clone()),
            },
        );

        let mut body_nodes = std::collections::BTreeMap::new();
        body_nodes.insert(body_start.clone(), body_node);

        let mut subgraphs = std::collections::BTreeMap::new();
        subgraphs.insert(
            body_key,
            Subgraph {
                start: body_start,
                nodes: body_nodes,
                edges: vec![],
            },
        );

        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: loop_key.clone(),
            nodes,
            edges: vec![],
            subgraphs,
        };

        (graph, cfg, loop_key)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_iterable_skips_frame_push() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let (graph, cfg, loop_key) = graph_with_loop_body(vec![]);
        let memory = RunMemory::default();
        let mut frames: Vec<Frame> = vec![];

        let effect = execute_loop_entry(LoopStageParams {
            node: &loop_key,
            loop_config: &cfg,
            graph: &graph,
            run_memory: &memory,
            writer: &writer,
            frames: &mut frames,
            return_to: NodeKey::try_from("after").unwrap(),
        })
        .await
        .unwrap();

        match effect {
            LoopEntryEffect::Skipped(o) => assert_eq!(o.as_ref(), "loop_empty"),
            LoopEntryEffect::Entered(_) => panic!("expected Skipped for empty iterable"),
        }
        assert!(frames.is_empty(), "frame stack should remain empty");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn three_items_pushes_frame_and_advances_to_body_start() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let items = vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
            toml::Value::Integer(3),
        ];
        let (graph, cfg, loop_key) = graph_with_loop_body(items);
        let memory = RunMemory::default();
        let mut frames: Vec<Frame> = vec![];

        let effect = execute_loop_entry(LoopStageParams {
            node: &loop_key,
            loop_config: &cfg,
            graph: &graph,
            run_memory: &memory,
            writer: &writer,
            frames: &mut frames,
            return_to: NodeKey::try_from("after").unwrap(),
        })
        .await
        .unwrap();

        match effect {
            LoopEntryEffect::Entered(node) => {
                assert_eq!(node, NodeKey::try_from("body_start").unwrap());
            },
            LoopEntryEffect::Skipped(_) => panic!("expected Entered for non-empty iterable"),
        }
        assert_eq!(frames.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn items_above_resolved_cap_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        // Note: surge-core validation rejects Static lists > MAX_LOOP_ITEMS_STATIC
        // at TOML load — so we bypass core validation by constructing the
        // graph in-memory and calling execute_loop_entry directly. This
        // exercises the engine-side defensive cap (MAX_LOOP_ITEMS_RESOLVED).
        #[allow(clippy::cast_possible_wrap)]
        let items: Vec<toml::Value> = (0..=MAX_LOOP_ITEMS_RESOLVED)
            .map(|i| toml::Value::Integer(i as i64))
            .collect();
        let (graph, cfg, loop_key) = graph_with_loop_body(items);
        let memory = RunMemory::default();
        let mut frames: Vec<Frame> = vec![];

        let result = execute_loop_entry(LoopStageParams {
            node: &loop_key,
            loop_config: &cfg,
            graph: &graph,
            run_memory: &memory,
            writer: &writer,
            frames: &mut frames,
            return_to: NodeKey::try_from("after").unwrap(),
        })
        .await;

        match result {
            Err(StageError::LoopItemsTooLarge { count, max }) => {
                // MAX_LOOP_ITEMS_RESOLVED = 1000 always fits in u32.
                #[allow(clippy::cast_possible_truncation)]
                let expected_count = MAX_LOOP_ITEMS_RESOLVED as u32 + 1;
                #[allow(clippy::cast_possible_truncation)]
                let expected_max = MAX_LOOP_ITEMS_RESOLVED as u32;
                assert_eq!(count, expected_count);
                assert_eq!(max, expected_max);
            },
            other => panic!("expected LoopItemsTooLarge, got {other:?}"),
        }
    }

    use surge_core::run_state::Cursor;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn iteration_advance_increments_index() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let items = vec![toml::Value::Integer(1), toml::Value::Integer(2)];
        let (graph, cfg, loop_key) = graph_with_loop_body(items.clone());
        let return_to = NodeKey::try_from("after").unwrap();
        let mut frames: Vec<Frame> = vec![Frame::Loop(LoopFrame {
            loop_node: loop_key.clone(),
            config: cfg,
            items,
            current_index: 0,
            attempts_remaining: 0,
            return_to: return_to.clone(),
            traversal_counts: HashMap::new(),
        })];
        let mut cursor = Cursor {
            node: NodeKey::try_from("body_start").unwrap(),
            attempt: 1,
        };

        let just_completed = OutcomeKey::try_from("done").unwrap();
        on_loop_iteration_done(&just_completed, &graph, &mut frames, &mut cursor, &writer)
            .await
            .unwrap();

        let Frame::Loop(lf) = &frames[0] else {
            panic!("expected Loop frame")
        };
        assert_eq!(lf.current_index, 1, "advanced to next iteration");
        assert_eq!(
            cursor.node,
            NodeKey::try_from("body_start").unwrap(),
            "cursor reset to body start"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn iteration_done_at_last_index_pops_frame() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let items = vec![toml::Value::Integer(1)];
        let (graph, cfg, loop_key) = graph_with_loop_body(items.clone());
        let return_to = NodeKey::try_from("after").unwrap();
        let mut frames: Vec<Frame> = vec![Frame::Loop(LoopFrame {
            loop_node: loop_key,
            config: cfg,
            items,
            current_index: 0,
            attempts_remaining: 0,
            return_to: return_to.clone(),
            traversal_counts: HashMap::new(),
        })];
        let mut cursor = Cursor {
            node: NodeKey::try_from("body_start").unwrap(),
            attempt: 1,
        };

        let just_completed = OutcomeKey::try_from("done").unwrap();
        on_loop_iteration_done(&just_completed, &graph, &mut frames, &mut cursor, &writer)
            .await
            .unwrap();

        assert!(frames.is_empty(), "frame popped after last iteration");
        assert_eq!(cursor.node, return_to, "cursor restored to return_to");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn artifact_iterable_resolves_dotted_path() {
        let dir = tempfile::tempdir().unwrap();

        // Write an artifact with a TOML array under "tasks".
        let artifact_content = r#"
tasks = ["task1", "task2", "task3"]
"#;
        let artifact_path = dir.path().join("plan.toml");
        std::fs::write(&artifact_path, artifact_content).unwrap();

        // Build RunMemory with the artifact registered.
        let mut memory = RunMemory::default();
        memory.artifacts.insert(
            "plan.toml".into(),
            surge_core::run_state::ArtifactRef {
                hash: surge_core::content_hash::ContentHash::compute(artifact_content.as_bytes()),
                path: artifact_path,
                name: "plan.toml".into(),
                produced_by: NodeKey::try_from("planner").unwrap(),
                produced_at_seq: 1,
            },
        );

        let src = IterableSource::Artifact {
            node: NodeKey::try_from("planner").unwrap(),
            name: "plan.toml".into(),
            jsonpath: "tasks".into(),
        };

        let items = resolve_iterable(&src, &memory).await.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], toml::Value::String("task1".into()));
        assert_eq!(items[2], toml::Value::String("task3".into()));
    }
}
