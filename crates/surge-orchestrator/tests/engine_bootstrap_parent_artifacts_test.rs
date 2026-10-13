//! Follow-up runs inherit their bootstrap parent's approved artifacts through
//! the one bootstrap handoff (#98): each exactly once, at its canonical path,
//! and only for the graph the handoff validated.

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::approvals::ApprovalPolicy;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::bootstrap::handoff::HandoffRejection;
use surge_orchestrator::engine::config::RunSeedArtifact;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError, EngineRunConfig, RunOutcome};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, Storage};

const DESCRIPTION: &str = fixtures::bootstrap::VALID_DESCRIPTION;
const ROADMAP: &str = fixtures::bootstrap::VALID_ROADMAP;

fn terminal_graph() -> Graph {
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
        metadata: GraphMetadata::new("bootstrap-parent-artifacts", chrono::Utc::now()),
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

struct Fixture {
    home: FixtureHome,
    parent_worktree: tempfile::TempDir,
    child_worktree: tempfile::TempDir,
    storage: Arc<Storage>,
    engine: Engine,
}

impl Fixture {
    async fn new() -> Self {
        let home = FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let child_worktree = tempfile::tempdir().unwrap();
        let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(
            child_worktree.path().to_path_buf(),
        )) as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());
        Self {
            home,
            parent_worktree: tempfile::tempdir().unwrap(),
            child_worktree,
            storage,
            engine,
        }
    }

    /// A completed bootstrap run whose generated plan is `follow_up`, with
    /// the roadmap produced under the planner's duplicate-stem names.
    async fn completed_parent(&self, follow_up: &Graph) -> RunId {
        let parent = RunId::new();
        let store = ArtifactStore::new(self.storage.home().join("runs"));
        let writer = self
            .storage
            .create_run(parent, self.parent_worktree.path(), None)
            .await
            .unwrap();
        let mut events = vec![
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: self.parent_worktree.path().to_path_buf(),
                initial_prompt: "bootstrap this".into(),
                config: RunConfig {
                    bootstrap_edit_loop_cap: Some(3),
                    budget: Default::default(),
                    sandbox_default: SandboxMode::WorkspaceWrite,
                    approval_default: ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: vec![],
                },
            },
            EventPayload::PipelineMaterialized {
                graph: Box::new(
                    surge_core::BundledFlows::by_name_latest("bootstrap")
                        .unwrap()
                        .graph,
                ),
                graph_hash: surge_core::ContentHash::compute(b"bootstrap"),
            },
        ];
        let flow = toml::to_string(follow_up).unwrap();
        for (name, content) in [
            ("description", DESCRIPTION),
            ("roadmap_toml", ROADMAP),
            ("roadmap_md", "# Roadmap\n"),
            ("flow", flow.as_str()),
        ] {
            let stored = store.put(parent, name, content.as_bytes()).await.unwrap();
            events.push(EventPayload::ArtifactProduced {
                node: NodeKey::try_from("planner").unwrap(),
                artifact: stored.hash,
                path: stored.path,
                name: name.into(),
                source_path: None,
            });
        }
        events.push(EventPayload::PipelineMaterialized {
            graph: Box::new(follow_up.clone()),
            graph_hash: follow_up.identity().unwrap(),
        });
        events.push(EventPayload::RunCompleted {
            terminal_node: NodeKey::try_from("approved").unwrap(),
        });
        writer
            .append_events(events.into_iter().map(VersionedEventPayload::new).collect())
            .await
            .unwrap();
        writer.close().await.unwrap();
        parent
    }

    async fn start_child(
        &self,
        graph: Graph,
        config: EngineRunConfig,
    ) -> Result<(RunId, RunOutcome), EngineError> {
        let child = RunId::new();
        let handle = self
            .engine
            .start_run(
                child,
                graph,
                self.child_worktree.path().to_path_buf(),
                config,
            )
            .await?;
        Ok((child, handle.await_completion().await?))
    }

    fn close(self) {
        drop(self.engine);
        drop(self.storage);
        self.home.close().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_up_inherits_each_approved_artifact_exactly_once() {
    let fx = Fixture::new().await;
    let follow_up = terminal_graph();
    let parent = fx.completed_parent(&follow_up).await;

    let (child, outcome) = fx
        .start_child(
            follow_up,
            EngineRunConfig {
                bootstrap_parent: Some(parent),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );

    let reader = fx.storage.open_run_reader(child).await.unwrap();
    let events = reader.read_events(EventSeq(1)..EventSeq(64)).await.unwrap();
    let inherited: Vec<(String, std::path::PathBuf)> = events
        .iter()
        .filter_map(|event| match &event.payload.payload {
            EventPayload::ArtifactProduced {
                node, path, name, ..
            } if node.as_ref() == "bootstrap_parent" => Some((name.clone(), path.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        inherited,
        [
            ("description".into(), "description.md".into()),
            ("roadmap".into(), "roadmap.toml".into()),
            ("flow".into(), "flow.toml".into()),
        ],
        "each inherited artifact exactly once, at its canonical path"
    );
    let worktree = fx.child_worktree.path();
    assert_eq!(
        std::fs::read_to_string(worktree.join("roadmap.toml")).unwrap(),
        ROADMAP,
        "the machine-readable roadmap, not the markdown view"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("description.md")).unwrap(),
        DESCRIPTION
    );
    drop(reader);
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_up_for_another_graph_or_with_duplicate_seeds_is_refused() {
    let fx = Fixture::new().await;
    let follow_up = terminal_graph();
    let parent = fx.completed_parent(&follow_up).await;

    let mut other = follow_up.clone();
    other.metadata.name.push_str("-not-the-approved-plan");
    let refused = fx
        .start_child(
            other,
            EngineRunConfig {
                bootstrap_parent: Some(parent),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            EngineError::BootstrapHandoff(HandoffRejection::IdentityMismatch { .. })
        ),
        "{refused}"
    );

    let duplicate = fx
        .start_child(
            follow_up,
            EngineRunConfig {
                bootstrap_parent: Some(parent),
                seed_artifacts: vec![
                    RunSeedArtifact::new("roadmap", "roadmap.toml", ROADMAP, "bootstrap_parent")
                        .unwrap(),
                ],
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(duplicate, EngineError::BootstrapHandoff(_)),
        "{duplicate}"
    );
    fx.close();
}
