//! Fork-from-here: spawn a new run that inherits a parent run's event history
//! up to a chosen `seq`, then diverges forward from that point.
//!
//! Invariant: the child log is a faithful copy of the parent's first `at_seq`
//! event payloads, so `fold(child_events) == fold(parent_events[1..=at_seq])`.
//! The parent records a `ForkCreated { new_run, fork_at_seq }` event so the
//! lineage survives in the (append-only) event log.

use std::sync::Arc;

use surge_core::id::RunId;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

use crate::engine::error::EngineError;

/// Pre-fork edits applied to the child's inherited history before it resumes.
///
/// Empty today; prompt/profile overrides land in a later M1 increment.
#[derive(Debug, Clone, Default)]
pub struct ForkEdits {}

/// Request to fork `parent` at `at_seq` into a fresh run `new_run`.
#[derive(Debug, Clone)]
pub struct ForkRequest {
    /// The run whose history is inherited.
    pub parent: RunId,
    /// The fresh run id that receives the copied prefix.
    pub new_run: RunId,
    /// Inclusive sequence to fork at: events `1..=at_seq` are copied.
    pub at_seq: u64,
    /// Optional pre-fork edits.
    pub edits: ForkEdits,
}

impl ForkRequest {
    /// Build a request with no pre-fork edits.
    #[must_use]
    pub fn new(parent: RunId, new_run: RunId, at_seq: u64) -> Self {
        Self {
            parent,
            new_run,
            at_seq,
            edits: ForkEdits::default(),
        }
    }
}

/// Outcome of a successful fork.
#[derive(Debug, Clone)]
pub struct ForkOutcome {
    /// The new run id.
    pub new_run: RunId,
    /// Number of event payloads copied from the parent (equals `at_seq`).
    pub copied_events: u64,
}

/// Fork a run: copy parent events `1..=at_seq` into `new_run`, then record a
/// `ForkCreated { new_run, fork_at_seq }` event on the parent for lineage.
///
/// The child's log is a byte-for-byte copy of the parent prefix, so folding it
/// reproduces the parent's state at `at_seq` exactly. The child does not start
/// executing here; the caller resumes it (see `Engine::resume_run`).
///
/// # Errors
///
/// Returns [`EngineError`] if the parent has no readable log, `at_seq` is out
/// of bounds, the prefix lacks a `RunStarted` event, or persistence fails.
pub async fn fork(storage: &Arc<Storage>, req: ForkRequest) -> Result<ForkOutcome, EngineError> {
    let reader = storage
        .open_run_reader(req.parent)
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    let max = reader
        .current_seq()
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?
        .as_u64();
    if req.at_seq == 0 || req.at_seq > max {
        return Err(EngineError::ForkInvalid(format!(
            "seq {} is out of bounds (parent run has {max} events)",
            req.at_seq
        )));
    }

    // Inherited prefix: parent events 1..=at_seq.
    let prefix = reader
        .read_events(EventSeq(1)..EventSeq(req.at_seq + 1))
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    // The prefix must begin with `RunStarted` so the child has a project path
    // and config to fold; reuse the parent's so the fork stays in the same
    // project (a fresh worktree is provisioned by the caller).
    let (project_path, pipeline_template) = prefix
        .iter()
        .find_map(|e| match &e.payload.payload {
            EventPayload::RunStarted {
                project_path,
                pipeline_template,
                ..
            } => Some((project_path.clone(), pipeline_template.clone())),
            _ => None,
        })
        .ok_or_else(|| {
            EngineError::ForkInvalid(
                "inherited prefix has no RunStarted event to seed the fork".into(),
            )
        })?;

    // Create the child run and copy the prefix payloads verbatim. Folding the
    // child therefore reproduces the parent's state at `at_seq` exactly.
    let child_writer = storage
        .create_run(
            req.new_run,
            &project_path,
            pipeline_template.map(|t| t.to_string()),
        )
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    let copied: Vec<VersionedEventPayload> = prefix.iter().map(|e| e.payload.clone()).collect();
    child_writer
        .append_events(copied)
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    // Inherit the parent's latest snapshot at-or-before the fork point so the
    // child resumes at the fork position rather than `graph.start`. Without
    // this, `replay` would fall back to `graph.start` for a snapshot-less log.
    if let Some((snap_seq, blob)) = reader
        .latest_snapshot_at_or_before(EventSeq(req.at_seq))
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?
    {
        child_writer
            .write_graph_snapshot(snap_seq, blob)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
    }

    // Record lineage on the parent. Appends are allowed even after a terminal
    // event (the append-only triggers block UPDATE/DELETE, not INSERT).
    let parent_writer = storage
        .open_run_writer(req.parent)
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    parent_writer
        .append_events(vec![VersionedEventPayload::new(
            EventPayload::ForkCreated {
                new_run: req.new_run,
                fork_at_seq: req.at_seq,
            },
        )])
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    Ok(ForkOutcome {
        new_run: req.new_run,
        // The bounds check guarantees the prefix holds exactly `at_seq`
        // contiguous events (the log has no gaps from seq 1).
        copied_events: req.at_seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::content_hash::ContentHash;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::{NodeKey, OutcomeKey};
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::run_event::RunConfig;
    use surge_core::run_state::RunMemory;
    use surge_core::sandbox::SandboxMode;
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_persistence::runs::reader::ReadEvent;

    fn minimal_graph() -> Graph {
        let end = NodeKey::try_from("end").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            end.clone(),
            Node {
                id: end.clone(),
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
                name: "fork-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: end,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    /// Fold the given events (whose `seq <= upto`) into a fresh `RunMemory`,
    /// mirroring what `engine::replay::replay` does internally.
    fn fold_to(events: &[ReadEvent], run_id: RunId, upto: u64) -> RunMemory {
        use chrono::TimeZone;
        let mut mem = RunMemory::default();
        for ev in events.iter().filter(|e| e.seq.as_u64() <= upto) {
            let timestamp = chrono::Utc
                .timestamp_millis_opt(ev.timestamp_ms)
                .single()
                .unwrap();
            let core = surge_core::run_event::RunEvent {
                run_id,
                seq: ev.seq.as_u64(),
                timestamp,
                payload: ev.payload.payload.clone(),
            };
            mem.apply_event(&core);
        }
        mem
    }

    async fn read_all(storage: &Arc<Storage>, run_id: RunId) -> Vec<ReadEvent> {
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let max = reader.current_seq().await.unwrap();
        reader
            .read_events(EventSeq(0)..EventSeq(max.as_u64() + 1))
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_copies_prefix_and_records_lineage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let parent = RunId::new();
        let worktree = dir.path().to_path_buf();
        let writer = storage.create_run(parent, &worktree, None).await.unwrap();

        let graph = minimal_graph();
        let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
        let node = NodeKey::try_from("end").unwrap();
        let outcome = OutcomeKey::try_from("done").unwrap();
        let config = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
        };

        // 5 events; fork at seq 3 so events 4 and 5 must be excluded.
        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree.clone(),
                    initial_prompt: "orig".into(),
                    config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash,
                }),
                VersionedEventPayload::new(EventPayload::StageEntered {
                    node: node.clone(),
                    attempt: 1,
                }),
                VersionedEventPayload::new(EventPayload::OutcomeReported {
                    node: node.clone(),
                    outcome: outcome.clone(),
                    summary: "post-fork".into(),
                }),
                VersionedEventPayload::new(EventPayload::StageCompleted { node, outcome }),
            ])
            .await
            .unwrap();
        // Release the writer slot so `fork` can append `ForkCreated`.
        drop(writer);

        let child = RunId::new();
        let out = fork(&storage, ForkRequest::new(parent, child, 3))
            .await
            .expect("fork should succeed");
        assert_eq!(out.copied_events, 3);
        assert_eq!(out.new_run, child);

        // Child must hold exactly the 3-event prefix.
        let cevents = read_all(&storage, child).await;
        assert_eq!(cevents.len(), 3, "child must have exactly 3 copied events");

        // Child fold == parent fold at seq 3 (the fork invariant).
        let pevents = read_all(&storage, parent).await;
        let parent_at_3 = fold_to(&pevents, parent, 3);
        let child_fold = fold_to(&cevents, child, u64::MAX);
        assert_eq!(
            child_fold, parent_at_3,
            "child fold must equal parent fold at seq 3"
        );

        // Parent recorded ForkCreated lineage.
        let lineage = pevents.iter().find_map(|e| match &e.payload.payload {
            EventPayload::ForkCreated {
                new_run,
                fork_at_seq,
            } => Some((*new_run, *fork_at_seq)),
            _ => None,
        });
        assert_eq!(
            lineage,
            Some((child, 3)),
            "parent must record ForkCreated for the child at seq 3"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_inherits_snapshot_so_child_resumes_at_fork_position() {
        use crate::engine::replay::replay;
        use crate::engine::snapshot::EngineSnapshot;
        use surge_core::run_state::Cursor;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let parent = RunId::new();
        let worktree = dir.path().to_path_buf();
        let writer = storage.create_run(parent, &worktree, None).await.unwrap();

        let graph = minimal_graph(); // start == "end"
        let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
        let config = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
        };
        let seqs = writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree.clone(),
                    initial_prompt: "orig".into(),
                    config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash,
                }),
                VersionedEventPayload::new(EventPayload::StageEntered {
                    node: NodeKey::try_from("mid_node").unwrap(),
                    attempt: 1,
                }),
            ])
            .await
            .unwrap();

        // Snapshot whose cursor points at a node that is NOT graph.start, so a
        // child that ignored it would (wrongly) resume at "end".
        let snap_seq = seqs[2]; // after StageEntered mid_node
        let cursor = Cursor {
            node: NodeKey::try_from("mid_node").unwrap(),
            attempt: 1,
        };
        let snapshot = EngineSnapshot::new(&cursor, snap_seq.as_u64(), 0);
        writer
            .write_graph_snapshot(snap_seq, serde_json::to_vec(&snapshot).unwrap())
            .await
            .unwrap();
        drop(writer);

        let child = RunId::new();
        fork(&storage, ForkRequest::new(parent, child, 3))
            .await
            .expect("fork should succeed");

        // Replaying the child must resume at the fork position (mid_node),
        // inherited from the parent snapshot — not at graph.start ("end").
        let creader = storage.open_run_reader(child).await.unwrap();
        let replayed = replay(&creader).await.expect("replay child");
        assert_eq!(
            replayed.cursor.node,
            NodeKey::try_from("mid_node").unwrap(),
            "forked child must resume at the snapshot cursor, not graph.start"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_rejects_out_of_bounds_seq() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let parent = RunId::new();
        let worktree = dir.path().to_path_buf();
        let writer = storage.create_run(parent, &worktree, None).await.unwrap();

        let graph = minimal_graph();
        let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
        let config = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
        };
        // 2 events total.
        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree.clone(),
                    initial_prompt: String::new(),
                    config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash,
                }),
            ])
            .await
            .unwrap();
        drop(writer);

        let zero = fork(&storage, ForkRequest::new(parent, RunId::new(), 0)).await;
        assert!(
            matches!(zero, Err(EngineError::ForkInvalid(_))),
            "seq 0 must be rejected, got {zero:?}"
        );
        let past = fork(&storage, ForkRequest::new(parent, RunId::new(), 99)).await;
        assert!(
            matches!(past, Err(EngineError::ForkInvalid(_))),
            "seq past the last event must be rejected, got {past:?}"
        );
    }
}
