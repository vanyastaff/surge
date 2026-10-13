//! Execution-state interface tests (#100): recorded events and checkpoints in
//! a temporary per-run store, asked for the state at a position.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use surge_core::approvals::ApprovalPolicy;
use surge_core::budget::BudgetGuard;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, SubgraphKey};
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_core::node::{Node, NodeConfig, Position as NodePosition};
use surge_core::run_event::{RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::Storage;
use surge_persistence::runs::run_writer::RunWriter;

use super::*;
use crate::engine::frames::{LoopFrame, SubgraphFrame};
use crate::engine::snapshot::SerializableFrame;

fn graph_with_start(start: &str) -> Graph {
    let key = NodeKey::try_from(start).unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        key.clone(),
        Node {
            id: key.clone(),
            position: NodePosition::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("execution-state", chrono::Utc::now()),
        start: key,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

fn run_config(servers: Vec<McpServerRef>) -> RunConfig {
    RunConfig {
        bootstrap_edit_loop_cap: None,
        budget: BudgetGuard::default(),
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: servers,
    }
}

fn started(graph: &Graph, config: RunConfig) -> Vec<VersionedEventPayload> {
    vec![
        VersionedEventPayload::new(EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/worktree"),
            initial_prompt: String::new(),
            config,
        }),
        materialized(graph),
    ]
}

fn materialized(graph: &Graph) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::PipelineMaterialized {
        graph: Box::new(graph.clone()),
        graph_hash: graph.identity().unwrap(),
    })
}

fn revision(graph: &Graph) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::GraphRevisionAccepted {
        patch_id: RoadmapPatchId::new("rpatch-state").unwrap(),
        target: RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        },
        previous_graph_hash: ContentHash::compute(b"base"),
        graph: Box::new(graph.clone()),
        graph_hash: graph.identity().unwrap(),
        active_pickup: ActivePickupPolicy::Allowed,
    })
}

fn note(text: &str) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: NodeKey::try_from("end").unwrap(),
        outcome: "done".parse().unwrap(),
        summary: text.into(),
    })
}

struct Fixture {
    home: crate::runtime_home_fixture::FixtureHome,
    storage: Arc<Storage>,
    run: RunId,
    writer: RunWriter,
}

impl Fixture {
    async fn new() -> Self {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        Self::with_storage(home, storage).await
    }

    /// A store whose events are stamped by a fixed clock, so two stores
    /// holding the same payloads hold the same log.
    async fn with_fixed_clock() -> Self {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let clock: Arc<dyn surge_persistence::runs::Clock> =
            Arc::new(surge_persistence::runs::MockClock::new(1_760_000_000_000));
        let storage = Storage::open_with(home.path(), clock).await.unwrap();
        Self::with_storage(home, storage).await
    }

    async fn with_storage(
        home: crate::runtime_home_fixture::FixtureHome,
        storage: Arc<Storage>,
    ) -> Self {
        let run = RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        Self {
            home,
            storage,
            run,
            writer,
        }
    }

    async fn append(&self, events: Vec<VersionedEventPayload>) -> Vec<u64> {
        self.writer
            .append_events(events)
            .await
            .unwrap()
            .into_iter()
            .map(EventSeq::as_u64)
            .collect()
    }

    async fn checkpoint(&self, snapshot: &EngineSnapshot) {
        self.writer
            .write_graph_snapshot(
                EventSeq(snapshot.at_seq),
                serde_json::to_vec(snapshot).unwrap(),
            )
            .await
            .unwrap();
    }

    async fn load(&self, position: Position) -> Result<ExecutionState, ExecutionStateError> {
        let reader = self.storage.open_run_reader(self.run).await.unwrap();
        load(&reader, position).await
    }

    async fn close(self) {
        self.writer.close().await.unwrap();
        drop(self.storage);
        self.home.close().unwrap();
    }
}

fn cursor(node: &str) -> Cursor {
    Cursor {
        node: NodeKey::try_from(node).unwrap(),
        attempt: 2,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_prefix_starts_at_the_graph_start_and_says_so() {
    let fx = Fixture::new().await;
    let graph = graph_with_start("end");
    fx.append(started(&graph, run_config(vec![]))).await;
    let state = fx.load(Position::Latest).await.unwrap();
    assert_eq!(state.cursor.node, graph.start);
    assert_eq!(state.cursor.attempt, 1);
    assert_eq!(state.cursor_source, CursorSource::GraphStart);
    assert_eq!(state.position, 2);
    assert_eq!(state.memory_applied_seq, 2);
    assert_eq!((state.frames.len(), state.root_traversal_counts.len()), (0, 0));
    assert_eq!(state.graph, graph);
    assert_eq!(state.terminal_verdict, TerminalVerdict::Open);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_restores_cursor_loop_and_subgraph_frames_and_counters() {
    let fx = Fixture::new().await;
    let graph = graph_with_start("end");
    let seqs = fx.append(started(&graph, run_config(vec![]))).await;
    let at = seqs[1];
    let loop_frame = Frame::Loop(LoopFrame {
        loop_node: NodeKey::try_from("milestones").unwrap(),
        config: surge_core::loop_config::LoopConfig {
            iterates_over: surge_core::loop_config::IterableSource::Static(vec![]),
            body: SubgraphKey::try_from("milestone_body").unwrap(),
            iteration_var_name: "milestone".into(),
            exit_condition: surge_core::loop_config::ExitCondition::AllItems,
            on_iteration_failure: surge_core::loop_config::FailurePolicy::Abort,
            parallelism: surge_core::loop_config::ParallelismMode::Sequential,
            gate_after_each: false,
        },
        items: vec![toml::Value::Integer(1), toml::Value::Integer(2)],
        current_index: 1,
        attempts_remaining: 0,
        return_to: NodeKey::try_from("end").unwrap(),
        traversal_counts: HashMap::new(),
    });
    let subgraph_frame = Frame::Subgraph(SubgraphFrame {
        outer_node: NodeKey::try_from("review").unwrap(),
        inner_subgraph: SubgraphKey::try_from("review_body").unwrap(),
        bound_inputs: vec![],
        return_to: NodeKey::try_from("end").unwrap(),
    });
    let mut snapshot = EngineSnapshot::new(&cursor("inner_step"), at, at);
    snapshot.frames = vec![
        SerializableFrame::from(loop_frame.clone()),
        SerializableFrame::from(subgraph_frame.clone()),
    ];
    snapshot.root_traversal_counts = HashMap::from([("e1".to_string(), 3)]);
    fx.checkpoint(&snapshot).await;

    let state = fx.load(Position::Latest).await.unwrap();
    assert_eq!(state.cursor, cursor("inner_step"));
    assert_eq!(state.cursor_source, CursorSource::Checkpoint { seq: at });
    // Exactly the frames the checkpoint format carries.
    let carried: Vec<Frame> = [loop_frame, subgraph_frame]
        .into_iter()
        .map(|frame| Frame::try_from(SerializableFrame::from(frame)).unwrap())
        .collect();
    assert_eq!(state.frames, carried);
    assert!(matches!(&state.frames[0], Frame::Loop(frame) if frame.current_index == 1));
    assert!(
        matches!(&state.frames[1], Frame::Subgraph(frame) if frame.return_to.as_str() == "end")
    );
    assert_eq!(
        state.root_traversal_counts,
        HashMap::from([(EdgeKey::try_from("e1").unwrap(), 3)])
    );
    fx.close().await;
}

/// A bootstrap run appends its generated follow-up flow as a second
/// `PipelineMaterialized`; the run keeps its own graph.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_run_keeps_its_own_graph_after_a_generated_flow() {
    let fx = Fixture::new().await;
    let own = surge_core::BundledFlows::by_name_latest("bootstrap")
        .unwrap()
        .graph;
    let generated = surge_core::BundledFlows::by_name_latest("linear-3")
        .unwrap()
        .graph;
    fx.append(vec![materialized(&own), materialized(&generated)])
        .await;
    let state = fx.load(Position::Latest).await.unwrap();
    assert_eq!(state.graph.metadata.name, own.metadata.name);
    assert!(state.graph.nodes.contains_key(&state.cursor.node));
    let events = {
        let reader = fx.storage.open_run_reader(fx.run).await.unwrap();
        reader.read_events(EventSeq(1)..EventSeq(3)).await.unwrap()
    };
    assert_eq!(
        graph_in_force(&events, 0).unwrap().metadata.name,
        own.metadata.name
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_applied_revision_is_in_force_and_later_ones_are_pending_in_order() {
    let fx = Fixture::new().await;
    let base = graph_with_start("end");
    let applied = graph_with_start("amend_001");
    let later = graph_with_start("amend_002");
    let last = graph_with_start("amend_003");
    let mut events = started(&base, run_config(vec![]));
    events.extend([revision(&applied), revision(&later), revision(&last)]);
    let seqs = fx.append(events).await;
    let mut snapshot = EngineSnapshot::new(&cursor("amend_001"), seqs[2], seqs[2]);
    snapshot.applied_graph_revision_seq = seqs[2];
    fx.checkpoint(&snapshot).await;

    let state = fx.load(Position::Latest).await.unwrap();
    assert_eq!(state.graph, applied);
    assert_eq!(state.applied_graph_revision_seq, seqs[2]);
    let pending: Vec<_> = state
        .pending_graph_revisions
        .iter()
        .map(|revision| (revision.seq, revision.graph.start.to_string()))
        .collect();
    assert_eq!(
        pending,
        [
            (seqs[3], "amend_002".to_string()),
            (seqs[4], "amend_003".to_string()),
        ],
        "the run task picks these up at its next safe point, in order"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_at_n_never_reads_later_events_or_checkpoints() {
    let fx = Fixture::new().await;
    let graph = graph_with_start("end");
    let mut events = started(&graph, run_config(vec![]));
    events.push(note("before"));
    let seqs = fx.append(events).await;
    let later = fx
        .append(vec![note("after"), revision(&graph_with_start("late"))])
        .await;
    fx.checkpoint(&EngineSnapshot::new(&cursor("end"), later[0], later[0]))
        .await;

    let at = fx.load(Position::AtSeq(seqs[2])).await.unwrap();
    assert_eq!(at.position, seqs[2]);
    assert_eq!(at.cursor_source, CursorSource::GraphStart);
    assert_eq!(at.pending_graph_revisions.len(), 0);
    let latest = fx.load(Position::Latest).await.unwrap();
    assert_eq!(
        latest.cursor_source,
        CursorSource::Checkpoint { seq: later[0] }
    );
    assert_ne!(at.memory, latest.memory);
    fx.close().await;
}

/// The fork invariant in general form: state at N from the full log equals
/// state from a store that only ever held `1..=N` (with the same checkpoints
/// at or before N).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_at_n_equals_state_of_the_truncated_log() {
    let full = Fixture::with_fixed_clock().await;
    let graph = graph_with_start("end");
    let mut events = started(&graph, run_config(vec![]));
    events.extend((0..6).map(|index| note(&format!("step {index}"))));
    events.push(revision(&graph_with_start("amend_001")));
    events.push(note("tail"));
    let payloads = events.clone();
    let seqs = full.append(events).await;
    let checkpoint = EngineSnapshot::new(&cursor("end"), seqs[4], seqs[4]);
    full.checkpoint(&checkpoint).await;

    // From seq 2: the first prefix with a graph in force.
    for (index, n) in seqs.iter().copied().enumerate().skip(1) {
        let truncated = Fixture::with_fixed_clock().await;
        truncated.append(payloads[..=index].to_vec()).await;
        if checkpoint.at_seq <= n {
            truncated.checkpoint(&checkpoint).await;
        }
        let from_full = full.load(Position::AtSeq(n)).await.unwrap();
        let from_truncated = truncated.load(Position::Latest).await.unwrap();
        assert_eq!(from_full, from_truncated, "seq {n}");
        truncated.close().await;
    }
    full.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn positions_outside_the_log_and_missing_graphs_are_errors() {
    let fx = Fixture::new().await;
    assert_eq!(
        fx.load(Position::Latest).await.unwrap_err(),
        ExecutionStateError::EmptyLog
    );
    fx.append(vec![note("no graph yet")]).await;
    assert_eq!(
        fx.load(Position::Latest).await.unwrap_err(),
        ExecutionStateError::NoGraph(1)
    );
    assert_eq!(
        fx.load(Position::AtSeq(0)).await.unwrap_err(),
        ExecutionStateError::PositionZero
    );
    assert_eq!(
        fx.load(Position::AtSeq(9)).await.unwrap_err(),
        ExecutionStateError::PositionBeyond {
            position: 9,
            latest: 1
        }
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_whose_envelope_contradicts_its_row_is_refused() {
    let fx = Fixture::new().await;
    let seqs = fx
        .append(started(&graph_with_start("end"), run_config(vec![])))
        .await;
    let mut snapshot = EngineSnapshot::new(&cursor("end"), seqs[1], seqs[1]);
    snapshot.at_seq = seqs[0];
    fx.writer
        .write_graph_snapshot(EventSeq(seqs[1]), serde_json::to_vec(&snapshot).unwrap())
        .await
        .unwrap();
    assert_eq!(
        fx.load(Position::Latest).await.unwrap_err(),
        ExecutionStateError::CheckpointSeqMismatch {
            seq: seqs[1],
            envelope: seqs[0]
        }
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_persisted_run_config_is_carried_and_absent_for_old_logs() {
    let fx = Fixture::new().await;
    let server = McpServerRef::new(
        "playwright".into(),
        McpTransportConfig::stdio(PathBuf::from("mcp-playwright"), vec![], HashMap::new()),
        Some(vec!["browser_navigate".into()]),
        Duration::from_secs(60),
        true,
    );
    fx.append(started(&graph_with_start("end"), run_config(vec![server])))
        .await;
    let config = fx.load(Position::Latest).await.unwrap().run_config.unwrap();
    assert_eq!(config.mcp_servers.len(), 1);
    assert_eq!(config.mcp_servers[0].name, "playwright");
    fx.close().await;

    let old = Fixture::new().await;
    old.append(vec![materialized(&graph_with_start("end"))])
        .await;
    assert!(
        old.load(Position::Latest)
            .await
            .unwrap()
            .run_config
            .is_none()
    );
    old.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_terminal_verdict_is_reported_not_raised() {
    let fx = Fixture::new().await;
    let seqs = fx
        .append(started(&graph_with_start("end"), run_config(vec![])))
        .await;
    let aborted = |reason: &str| {
        VersionedEventPayload::new(EventPayload::RunAborted {
            reason: reason.into(),
        })
    };
    let ends = fx.append(vec![aborted("first"), aborted("second")]).await;
    assert_eq!(
        fx.load(Position::AtSeq(seqs[1]))
            .await
            .unwrap()
            .terminal_verdict,
        TerminalVerdict::Open
    );
    assert!(matches!(
        fx.load(Position::AtSeq(ends[0]))
            .await
            .unwrap()
            .terminal_verdict,
        TerminalVerdict::Terminal(_)
    ));
    assert!(matches!(
        fx.load(Position::Latest).await.unwrap().terminal_verdict,
        TerminalVerdict::Conflict { .. }
    ));
    fx.close().await;
}

/// Rebuilding memory never reads the clock: an out-of-range timestamp is a
/// typed corruption error, never an epoch or wall-clock substitute.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_timestamp_is_refused_deterministically() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let clock: Arc<dyn surge_persistence::runs::Clock> =
        Arc::new(surge_persistence::runs::MockClock::new(i64::MAX));
    let storage = Storage::open_with(home.path(), clock).await.unwrap();
    let fx = Fixture::with_storage(home, storage).await;
    let seqs = fx
        .append(started(&graph_with_start("end"), run_config(vec![])))
        .await;
    for _ in 0..2 {
        assert_eq!(
            fx.load(Position::Latest).await.unwrap_err(),
            ExecutionStateError::CorruptTimestamp {
                seq: seqs[0],
                raw_ms: i64::MAX
            }
        );
    }
    fx.close().await;
}
