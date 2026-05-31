//! Fork-from-here: spawn a new run that inherits a parent run's event history
//! up to a chosen `seq`, then diverges forward from that point.
//!
//! Invariant: without [`ForkEdits`], the child log is a faithful copy of the
//! parent's first `at_seq` event payloads, so
//! `fold(child_events) == fold(parent_events[1..=at_seq])`. Pre-fork edits
//! rewrite a single node in the child's materialized graph (and its hash) so a
//! fork can retry a stage with a corrective hint or a different profile. The
//! parent records a `ForkCreated { new_run, fork_at_seq }` event so the lineage
//! survives in the (append-only) event log.

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_core::agent_config::PromptOverride;
use surge_core::content_hash::ContentHash;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, ProfileKey};
use surge_core::node::NodeConfig;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

use crate::engine::error::EngineError;

/// Pre-fork edits applied to the child's materialized graph before it resumes,
/// so a fork can retry a stage with a corrective hint or a different profile
/// without re-running the stages that already succeeded. Empty by default.
#[derive(Debug, Clone, Default)]
pub struct ForkEdits {
    /// Per Agent node: text appended to that node's system prompt
    /// (`prompt_overrides.append_system`) in the child's graph.
    pub prompt_appends: BTreeMap<NodeKey, String>,
    /// Per Agent node: replacement profile key in the child's graph.
    pub profile_overrides: BTreeMap<NodeKey, ProfileKey>,
}

impl ForkEdits {
    /// True when no edits are requested.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prompt_appends.is_empty() && self.profile_overrides.is_empty()
    }
}

/// Request to fork `parent` at `at_seq` into a fresh run `new_run`, with
/// optional pre-fork [`ForkEdits`].
#[derive(Debug, Clone)]
pub struct ForkRequest {
    /// The run whose history is inherited.
    pub parent: RunId,
    /// The fresh run id that receives the copied prefix.
    pub new_run: RunId,
    /// Inclusive sequence to fork at: events `1..=at_seq` are copied.
    pub at_seq: u64,
    /// Edits applied to the child's graph before it resumes.
    pub edits: ForkEdits,
}

impl ForkRequest {
    /// Build a fork request with no pre-fork edits.
    #[must_use]
    pub fn new(parent: RunId, new_run: RunId, at_seq: u64) -> Self {
        Self {
            parent,
            new_run,
            at_seq,
            edits: ForkEdits::default(),
        }
    }

    /// Attach pre-fork edits to the request.
    #[must_use]
    pub fn with_edits(mut self, edits: ForkEdits) -> Self {
        self.edits = edits;
        self
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
/// The child's log copies the parent prefix verbatim (optionally rewriting one
/// node's prompt/profile per [`ForkEdits`]), so without edits folding it
/// reproduces the parent's state at `at_seq` exactly. The child does not start
/// executing here; the caller resumes it (see `Engine::resume_run`).
///
/// # Errors
///
/// Returns [`EngineError`] if the parent has no readable log, `at_seq` is out
/// of bounds, the inherited prefix does not begin with a `RunStarted` event, a
/// pre-fork edit targets an unknown or non-Agent node, or persistence fails.
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

    // The prefix must BEGIN with `RunStarted` (it is always seq 1 in a
    // well-formed log). Requiring the first event — rather than scanning for
    // any `RunStarted` — rejects a malformed parent log that would otherwise be
    // copied into the child with an impossible event order. The child reuses
    // the parent's project path so the fork stays in the same project (a fresh
    // worktree is provisioned by the caller).
    let (project_path, pipeline_template) = match prefix.first().map(|e| &e.payload.payload) {
        Some(EventPayload::RunStarted {
            project_path,
            pipeline_template,
            ..
        }) => (project_path.clone(), pipeline_template.clone()),
        _ => {
            return Err(EngineError::ForkInvalid(
                "inherited prefix has no RunStarted event to seed the fork".into(),
            ));
        },
    };

    // Build the child's payloads and apply any pre-fork edits up front. Edit
    // validation is pure — no run exists yet — so an unknown or non-Agent target
    // fails before any child is created (never leaving an orphan behind). With
    // edits the child intentionally diverges from a byte-for-byte copy.
    let mut copied: Vec<VersionedEventPayload> = prefix.iter().map(|e| e.payload.clone()).collect();
    if !req.edits.is_empty() {
        apply_fork_edits(&mut copied, &req.edits)?;
    }

    // The fork is not atomic across the two per-run logs (parent and child are
    // separate SQLite databases — there is no shared transaction). To keep the
    // worst-case failure benign we: (1) acquire the parent writer up front so
    // an actively-running parent fails fast, before any orphan child is
    // created; (2) write the child fully (it is self-contained and the
    // authoritative artifact); then (3) append the parent lineage last. The
    // only residual failure is a usable child without a parent back-ref — never
    // a `ForkCreated` pointing at a child that does not exist. Each fork uses a
    // fresh child id, so a retry creates a distinct run, not a duplicate.
    let parent_writer = storage
        .open_run_writer(req.parent)
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    // Create the child run and write the (possibly edited) prefix payloads.
    // Folding the child reproduces the parent's state at `at_seq` (modulo edits).
    let child_writer = storage
        .create_run(
            req.new_run,
            &project_path,
            pipeline_template.map(|t| t.to_string()),
        )
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
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

    // Record lineage on the parent last (append-only; valid even after a
    // terminal event, since the triggers block UPDATE/DELETE, not INSERT).
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

/// Apply pre-fork [`ForkEdits`] to the child's materialized graph, rewriting the
/// `PipelineMaterialized` event in `copied` (graph + recomputed hash).
///
/// Runs with mid-run graph revisions are rejected: the child would fold to the
/// revised graph, so editing the base `PipelineMaterialized` would silently have
/// no effect. Supporting that is a follow-up.
fn apply_fork_edits(
    copied: &mut [VersionedEventPayload],
    edits: &ForkEdits,
) -> Result<(), EngineError> {
    if copied
        .iter()
        .any(|ev| matches!(ev.payload, EventPayload::GraphRevisionAccepted { .. }))
    {
        return Err(EngineError::ForkInvalid(
            "pre-fork edits are not yet supported for runs with mid-run graph revisions".into(),
        ));
    }
    let idx = copied
        .iter()
        .position(|ev| matches!(ev.payload, EventPayload::PipelineMaterialized { .. }))
        .ok_or_else(|| {
            EngineError::ForkInvalid(
                "cannot apply pre-fork edits: the inherited prefix has no materialized graph"
                    .into(),
            )
        })?;

    let EventPayload::PipelineMaterialized { graph, .. } = &copied[idx].payload else {
        unreachable!("idx points at a PipelineMaterialized event")
    };
    let mut graph = (**graph).clone();
    apply_edits_to_graph(&mut graph, edits)?;

    let graph_hash = ContentHash::compute(
        &serde_json::to_vec(&graph)
            .map_err(|e| EngineError::Internal(format!("graph serialize: {e}")))?,
    );
    copied[idx].payload = EventPayload::PipelineMaterialized {
        graph: Box::new(graph),
        graph_hash,
    };
    Ok(())
}

/// Mutate `graph` in place per `edits`. Validates every targeted node up front
/// (it must exist and be an `Agent`) so the operation is all-or-nothing.
fn apply_edits_to_graph(graph: &mut Graph, edits: &ForkEdits) -> Result<(), EngineError> {
    for node_key in edits
        .prompt_appends
        .keys()
        .chain(edits.profile_overrides.keys())
    {
        match graph.nodes.get(node_key) {
            None => {
                return Err(EngineError::ForkInvalid(format!(
                    "pre-fork edit targets unknown node '{node_key}'"
                )));
            },
            Some(node) if !matches!(node.config, NodeConfig::Agent(_)) => {
                return Err(EngineError::ForkInvalid(format!(
                    "pre-fork edit targets non-Agent node '{node_key}'"
                )));
            },
            Some(_) => {},
        }
    }

    for (node_key, text) in &edits.prompt_appends {
        if let Some(node) = graph.nodes.get_mut(node_key) {
            if let NodeConfig::Agent(cfg) = &mut node.config {
                cfg.prompt_overrides
                    .get_or_insert_with(|| PromptOverride {
                        system: None,
                        append_system: None,
                    })
                    .append_system = Some(text.clone());
            }
        }
    }
    for (node_key, profile) in &edits.profile_overrides {
        if let Some(node) = graph.nodes.get_mut(node_key) {
            if let NodeConfig::Agent(cfg) = &mut node.config {
                cfg.profile = profile.clone();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_core::agent_config::{AgentConfig, NodeLimits};
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::content_hash::ContentHash;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::{NodeKey, OutcomeKey, ProfileKey};
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_rejects_prefix_not_starting_with_run_started() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let parent = RunId::new();
        let worktree = dir.path().to_path_buf();
        let writer = storage.create_run(parent, &worktree, None).await.unwrap();

        let graph = minimal_graph();
        let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
        // Malformed log: the first event is NOT RunStarted. A scan-anywhere
        // check would accept this; requiring `prefix.first()` rejects it.
        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash,
                }),
                VersionedEventPayload::new(EventPayload::StageEntered {
                    node: NodeKey::try_from("end").unwrap(),
                    attempt: 1,
                }),
            ])
            .await
            .unwrap();
        drop(writer);

        let res = fork(&storage, ForkRequest::new(parent, RunId::new(), 2)).await;
        assert!(
            matches!(res, Err(EngineError::ForkInvalid(_))),
            "fork must reject a prefix that does not begin with RunStarted, got {res:?}"
        );
    }

    /// A graph with one Agent node `impl_1` (profile `implementer@1.0`) and a
    /// terminal `end`. Structural validity is irrelevant — fork edits operate on
    /// the node map, not the edges.
    fn agent_graph() -> Graph {
        let impl_node = NodeKey::try_from("impl_1").unwrap();
        let end = NodeKey::try_from("end").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            impl_node.clone(),
            Node {
                id: impl_node.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Agent(AgentConfig {
                    profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                    prompt_overrides: None,
                    tool_overrides: None,
                    sandbox_override: None,
                    approvals_override: None,
                    bindings: vec![],
                    rules_overrides: None,
                    limits: NodeLimits::default(),
                    hooks: vec![],
                    custom_fields: BTreeMap::new(),
                }),
            },
        );
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
                name: "fork-edit-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: impl_node,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    /// Seed a parent run `[RunStarted, PipelineMaterialized(agent_graph)]`.
    async fn seed_agent_parent(dir: &std::path::Path) -> (Arc<Storage>, RunId) {
        let storage = Storage::open(dir).await.unwrap();
        let parent = RunId::new();
        let worktree = dir.to_path_buf();
        let writer = storage.create_run(parent, &worktree, None).await.unwrap();
        let graph = agent_graph();
        let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
        let config = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
        };
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
            ])
            .await
            .unwrap();
        drop(writer);
        (storage, parent)
    }

    /// Read the child's materialized graph back from storage.
    async fn child_graph(storage: &Arc<Storage>, child: RunId) -> Graph {
        let reader = storage.open_run_reader(child).await.unwrap();
        let max = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(max.as_u64() + 1))
            .await
            .unwrap();
        events
            .iter()
            .find_map(|e| match &e.payload.payload {
                EventPayload::PipelineMaterialized { graph, .. } => Some((**graph).clone()),
                _ => None,
            })
            .expect("child has a PipelineMaterialized graph")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_prompt_append_edits_child_graph() {
        let dir = tempfile::tempdir().unwrap();
        let (storage, parent) = seed_agent_parent(dir.path()).await;
        let child = RunId::new();

        let mut edits = ForkEdits::default();
        edits.prompt_appends.insert(
            NodeKey::try_from("impl_1").unwrap(),
            "retry: prefer X".into(),
        );
        fork(
            &storage,
            ForkRequest::new(parent, child, 2).with_edits(edits),
        )
        .await
        .expect("fork with prompt edit");

        let g = child_graph(&storage, child).await;
        let node = g.nodes.get(&NodeKey::try_from("impl_1").unwrap()).unwrap();
        let NodeConfig::Agent(cfg) = &node.config else {
            panic!("impl_1 must be an Agent node");
        };
        assert_eq!(
            cfg.prompt_overrides
                .as_ref()
                .and_then(|o| o.append_system.as_deref()),
            Some("retry: prefer X"),
            "child graph must carry the appended prompt"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_profile_override_edits_child_graph() {
        let dir = tempfile::tempdir().unwrap();
        let (storage, parent) = seed_agent_parent(dir.path()).await;
        let child = RunId::new();
        let new_profile = ProfileKey::try_from("reviewer@1.0").unwrap();

        let mut edits = ForkEdits::default();
        edits
            .profile_overrides
            .insert(NodeKey::try_from("impl_1").unwrap(), new_profile.clone());
        fork(
            &storage,
            ForkRequest::new(parent, child, 2).with_edits(edits),
        )
        .await
        .expect("fork with profile edit");

        let g = child_graph(&storage, child).await;
        let node = g.nodes.get(&NodeKey::try_from("impl_1").unwrap()).unwrap();
        let NodeConfig::Agent(cfg) = &node.config else {
            panic!("impl_1 must be an Agent node");
        };
        assert_eq!(
            cfg.profile, new_profile,
            "child graph must carry the swapped profile"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_edit_unknown_node_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (storage, parent) = seed_agent_parent(dir.path()).await;

        let mut edits = ForkEdits::default();
        edits
            .prompt_appends
            .insert(NodeKey::try_from("nope").unwrap(), "x".into());
        let res = fork(
            &storage,
            ForkRequest::new(parent, RunId::new(), 2).with_edits(edits),
        )
        .await;
        assert!(
            matches!(res, Err(EngineError::ForkInvalid(_))),
            "an edit targeting an unknown node must be rejected, got {res:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_edit_non_agent_node_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (storage, parent) = seed_agent_parent(dir.path()).await;

        let mut edits = ForkEdits::default();
        edits
            .prompt_appends
            .insert(NodeKey::try_from("end").unwrap(), "x".into());
        let res = fork(
            &storage,
            ForkRequest::new(parent, RunId::new(), 2).with_edits(edits),
        )
        .await;
        assert!(
            matches!(res, Err(EngineError::ForkInvalid(_))),
            "an edit targeting a non-Agent node must be rejected, got {res:?}"
        );
    }
}
