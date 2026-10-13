//! Bootstrap handoff interface tests (#98): hand-written parent logs over a
//! temp `Storage` and `ArtifactStore`. No agents, no bridge.

use std::sync::Arc;

use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::Storage;

use super::*;

const DESCRIPTION: &str = include_str!("../../../tests/fixtures/artifacts/valid/description.md");
const ROADMAP: &str = include_str!("../../../tests/fixtures/artifacts/valid/roadmap.toml");
const INVALID_ROADMAP: &str =
    include_str!("../../../tests/fixtures/artifacts/invalid/roadmap.toml");
const FLOW: &str = include_str!("../../../tests/fixtures/golden_multi_milestone_flow.toml");

struct Fixture {
    home: crate::runtime_home_fixture::FixtureHome,
    worktree: tempfile::TempDir,
    storage: Arc<Storage>,
}

/// A hand-written parent log.
struct Parent {
    artifacts: Vec<(&'static str, String)>,
    /// `None`: no follow-up graph is materialized.
    followup: Option<Graph>,
    terminal: Option<EventPayload>,
    flow_gate: Option<serde_json::Value>,
}

impl Default for Parent {
    fn default() -> Self {
        Self {
            artifacts: vec![
                ("description", DESCRIPTION.into()),
                ("roadmap_toml", ROADMAP.into()),
                ("flow", FLOW.into()),
            ],
            followup: Some(flow_graph()),
            terminal: Some(EventPayload::RunCompleted {
                terminal_node: NodeKey::try_from("approved").unwrap(),
            }),
            flow_gate: None,
        }
    }
}

fn flow_graph() -> Graph {
    toml::from_str(FLOW).unwrap()
}

fn node(name: &str) -> NodeKey {
    NodeKey::try_from(name).unwrap()
}

impl Fixture {
    async fn new() -> Self {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        Self {
            home,
            worktree: tempfile::tempdir().unwrap(),
            storage,
        }
    }

    async fn parent(&self, spec: Parent) -> RunId {
        let run = RunId::new();
        let store = ArtifactStore::new(self.storage.home().join("runs"));
        let writer = self
            .storage
            .create_run(run, self.worktree.path(), None)
            .await
            .unwrap();
        let mut events = vec![
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: self.worktree.path().to_path_buf(),
                initial_prompt: "build it".into(),
                config: RunConfig {
                    bootstrap_edit_loop_cap: Some(3),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    budget: surge_core::budget::BudgetGuard::default(),
                },
            },
            EventPayload::PipelineMaterialized {
                graph: Box::new(
                    surge_core::BundledFlows::by_name_latest("bootstrap")
                        .unwrap()
                        .graph,
                ),
                graph_hash: ContentHash::compute(b"bootstrap"),
            },
        ];
        for (name, content) in &spec.artifacts {
            let stored = store.put(run, name, content.as_bytes()).await.unwrap();
            events.push(EventPayload::ArtifactProduced {
                node: node("planner"),
                artifact: stored.hash,
                path: format!("{name}.out").into(),
                name: (*name).into(),
                source_path: None,
            });
        }
        if let Some(graph) = spec.followup {
            events.push(EventPayload::PipelineMaterialized {
                graph_hash: graph.identity().unwrap(),
                graph: Box::new(graph),
            });
        }
        if let Some(response) = spec.flow_gate {
            events.push(EventPayload::HumanInputResolved {
                node: node(FLOW_GATE_NODE),
                call_id: None,
                response,
            });
        }
        events.extend(spec.terminal);
        writer
            .append_events(events.into_iter().map(VersionedEventPayload::new).collect())
            .await
            .unwrap();
        writer.close().await.unwrap();
        run
    }

    async fn last_seq(&self, run: RunId) -> u64 {
        let reader = self.storage.open_run_reader(run).await.unwrap();
        reader.current_seq().await.unwrap().as_u64()
    }

    async fn prepare(&self, run: RunId) -> Result<BootstrapHandoff, HandoffRejection> {
        prepare(&self.storage, run, &HandoffContext::default()).await
    }

    fn close(self) {
        drop(self.storage);
        self.home.close().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_parent_hands_off_ordered_canonical_artifacts_read_only() {
    let fx = Fixture::new().await;
    let parent = fx.parent(Parent::default()).await;
    let before = fx.last_seq(parent).await;

    let handoff = fx.prepare(parent).await.unwrap();
    assert_eq!(handoff.parent, parent);
    assert_eq!(handoff.parent_worktree, fx.worktree.path());
    // The generated graph, never the bootstrap graph itself.
    assert_eq!(handoff.graph, flow_graph());
    assert_eq!(handoff.identity, flow_graph().identity().unwrap());
    assert_eq!(handoff.edits_applied, 0);
    let names: Vec<_> = handoff
        .artifacts
        .iter()
        .map(|artifact| (artifact.name, artifact.seed_path.clone()))
        .collect();
    assert_eq!(
        names,
        [
            ("description", "description.md".into()),
            ("roadmap", "roadmap.toml".into()),
            ("flow", "flow.toml".into()),
        ]
    );
    assert_eq!(handoff.artifacts[1].content, ROADMAP);
    assert_eq!(handoff.source_refs()[1].name, "roadmap");
    let seeds = handoff.seed_artifacts().unwrap();
    assert_eq!(seeds.len(), 3);
    assert!(
        seeds
            .iter()
            .all(|seed| seed.producer.as_str() == BOOTSTRAP_PARENT_NODE)
    );
    // Read-only: nothing was appended to the parent.
    assert_eq!(fx.last_seq(parent).await, before);
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_roadmap_is_found_under_every_producer_spelling() {
    let fx = Fixture::new().await;
    for spelling in ["roadmap", "roadmap.toml", "roadmap-toml", "roadmap_toml"] {
        let parent = fx
            .parent(Parent {
                artifacts: vec![
                    ("description", DESCRIPTION.into()),
                    (spelling, ROADMAP.into()),
                    ("roadmap_md", "# Roadmap\n".into()),
                    ("flow", FLOW.into()),
                ],
                ..Parent::default()
            })
            .await;
        let handoff = fx.prepare(parent).await.unwrap();
        assert_eq!(handoff.artifacts[1].content, ROADMAP, "{spelling}");
    }
    fx.close();
}

/// Which rejection a case expects.
type Expect = fn(&HandoffRejection) -> bool;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_plans_are_refused_with_their_reason() {
    let fx = Fixture::new().await;
    let cases: Vec<(Parent, Expect)> = vec![
        (
            Parent {
                artifacts: vec![
                    ("description", DESCRIPTION.into()),
                    ("roadmap", ROADMAP.into()),
                    ("roadmap-toml", ROADMAP.into()),
                    ("flow", FLOW.into()),
                ],
                ..Parent::default()
            },
            |r| {
                matches!(
                    r,
                    HandoffRejection::ArtifactAmbiguous {
                        name: "roadmap",
                        ..
                    }
                )
            },
        ),
        (
            Parent {
                artifacts: vec![("roadmap", ROADMAP.into()), ("flow", FLOW.into())],
                ..Parent::default()
            },
            |r| matches!(r, HandoffRejection::ArtifactMissing("description")),
        ),
        (
            Parent {
                artifacts: vec![
                    ("description", DESCRIPTION.into()),
                    ("roadmap", INVALID_ROADMAP.into()),
                    ("flow", FLOW.into()),
                ],
                ..Parent::default()
            },
            |r| {
                matches!(
                    r,
                    HandoffRejection::ArtifactContract {
                        name: "roadmap",
                        ..
                    }
                )
            },
        ),
        (
            Parent {
                followup: Some({
                    let mut other = flow_graph();
                    other.metadata.name.push_str("-changed");
                    other
                }),
                ..Parent::default()
            },
            |r| matches!(r, HandoffRejection::FlowMismatch),
        ),
        (
            Parent {
                followup: None,
                ..Parent::default()
            },
            |r| matches!(r, HandoffRejection::FollowUpGraphMissing),
        ),
        (
            Parent {
                flow_gate: Some(serde_json::json!({
                    "outcome": "approve",
                    "node_overrides": { "no_such_step": { "agent_id": "claude-acp" } }
                })),
                ..Parent::default()
            },
            |r| matches!(r, HandoffRejection::EditsInvalid(_)),
        ),
    ];
    for (spec, expected) in cases {
        let parent = fx.parent(spec).await;
        let rejection = fx.prepare(parent).await.unwrap_err();
        assert!(expected(&rejection), "{rejection}");
        assert!(!rejection.is_retryable(), "{rejection}");
    }
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_artifact_bytes_are_refused() {
    let fx = Fixture::new().await;
    let parent = fx.parent(Parent::default()).await;
    // The store entry for the description disappears: the recorded hash no
    // longer resolves to verified bytes.
    let description = ContentHash::compute(DESCRIPTION.as_bytes());
    let blob = fx
        .storage
        .home()
        .join("runs")
        .join(parent.to_string())
        .join("artifacts")
        .join(description.to_hex());
    std::fs::write(&blob, b"tampered").unwrap();
    let rejection = fx.prepare(parent).await.unwrap_err();
    assert!(
        matches!(
            rejection,
            HandoffRejection::ArtifactCorrupt {
                name: "description",
                ..
            }
        ),
        "{rejection}"
    );
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_profile_is_refused_for_the_childs_registry() {
    let fx = Fixture::new().await;
    let parent = fx.parent(Parent::default()).await;
    let profiles =
        crate::profile_loader::ProfileRegistry::new(crate::profile_loader::DiskProfileSet::empty());
    let mut graph = flow_graph();
    let agent = graph
        .nodes
        .values_mut()
        .chain(
            graph
                .subgraphs
                .values_mut()
                .flat_map(|sub| sub.nodes.values_mut()),
        )
        .find_map(|node| match &mut node.config {
            surge_core::node::NodeConfig::Agent(agent) => Some(agent),
            _ => None,
        })
        .unwrap();
    agent.profile = surge_core::keys::ProfileKey::try_from("no-such-profile@1.0").unwrap();
    let text = toml::to_string(&graph).unwrap();
    let parent_unknown = fx
        .parent(Parent {
            artifacts: vec![
                ("description", DESCRIPTION.into()),
                ("roadmap", ROADMAP.into()),
                ("flow", text),
            ],
            followup: Some(graph),
            ..Parent::default()
        })
        .await;
    let context = HandoffContext {
        profiles: Some(&profiles),
        agents: None,
    };
    let rejection = prepare(&fx.storage, parent_unknown, &context)
        .await
        .unwrap_err();
    assert!(
        matches!(rejection, HandoffRejection::GraphInvalid(_)),
        "{rejection}"
    );
    // The bundled profiles resolve the unchanged plan.
    prepare(&fx.storage, parent, &context).await.unwrap();
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flow_gate_edits_reach_the_graph_and_the_flow_bytes() {
    let fx = Fixture::new().await;
    let graph = flow_graph();
    let step = graph
        .nodes
        .values()
        .chain(graph.subgraphs.values().flat_map(|sub| sub.nodes.values()))
        .find(|node| matches!(node.config, surge_core::node::NodeConfig::Agent(_)))
        .map(|node| node.id.to_string())
        .unwrap();
    let parent = fx
        .parent(Parent {
            flow_gate: Some(serde_json::json!({
                "outcome": "approve",
                "node_overrides": { step.clone(): { "agent_id": "claude-acp" } }
            })),
            ..Parent::default()
        })
        .await;
    let handoff = fx.prepare(parent).await.unwrap();
    assert_eq!(handoff.edits_applied, 1);
    assert_ne!(handoff.graph, flow_graph());
    assert_eq!(handoff.identity, handoff.graph.identity().unwrap());
    let flow = &handoff.artifacts[2];
    let seeded: Graph = toml::from_str(&flow.content).unwrap();
    assert_eq!(seeded, handoff.graph, "the flow seed is what will run");
    assert_eq!(flow.hash, ContentHash::compute(flow.content.as_bytes()));
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unconfirmed_parents_are_retryable() {
    let fx = Fixture::new().await;
    let running = fx
        .parent(Parent {
            terminal: None,
            ..Parent::default()
        })
        .await;
    let failed = fx
        .parent(Parent {
            terminal: Some(EventPayload::RunFailed {
                error: "planner died".into(),
            }),
            ..Parent::default()
        })
        .await;
    for parent in [running, failed] {
        let rejection = fx.prepare(parent).await.unwrap_err();
        assert_eq!(rejection, HandoffRejection::ParentNotCompleted);
        assert!(rejection.is_retryable());
    }
    let absent = fx.prepare(RunId::new()).await.unwrap_err();
    assert!(
        matches!(absent, HandoffRejection::ParentUnreadable(_)),
        "{absent}"
    );
    assert!(absent.is_retryable());
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_requires_the_pinned_identity() {
    let fx = Fixture::new().await;
    let parent = fx.parent(Parent::default()).await;
    let pinned = flow_graph().identity().unwrap();
    let context = HandoffContext::default();
    reload(&fx.storage, parent, &context, pinned).await.unwrap();
    let stale = ContentHash::compute(b"another plan");
    assert_eq!(
        reload(&fx.storage, parent, &context, stale)
            .await
            .unwrap_err(),
        HandoffRejection::IdentityMismatch {
            expected: stale,
            actual: pinned,
        }
    );
    fx.close();
}

#[test]
fn every_run_outcome_has_one_bootstrap_stop() {
    let wake_at = chrono::Utc::now();
    let cases = [
        (
            RunOutcome::Completed {
                terminal: node("end"),
            },
            BootstrapStop::Completed,
        ),
        (
            RunOutcome::Parked { wake_at },
            BootstrapStop::Parked { wake_at },
        ),
        (
            RunOutcome::RecoveryRequired {
                control_generation: 2,
                diagnostic: "fence lost".into(),
            },
            BootstrapStop::RecoveryRequired {
                diagnostic: "fence lost".into(),
            },
        ),
        (
            RunOutcome::Failed {
                error: "boom".into(),
            },
            BootstrapStop::Failed {
                error: "boom".into(),
            },
        ),
        (
            RunOutcome::Aborted {
                reason: "stop".into(),
            },
            BootstrapStop::Aborted {
                reason: "stop".into(),
            },
        ),
    ];
    for (outcome, stop) in cases {
        assert_eq!(classify(&outcome), stop);
    }
    assert!(matches!(
        classify(&RunOutcome::SettlementFailed {
            original: Box::new(RunOutcome::Completed {
                terminal: node("end")
            }),
            failures: vec![crate::engine::handle::RunSettlementFailure::WriterSeal],
        }),
        BootstrapStop::SettlementFailed { .. }
    ));
}
