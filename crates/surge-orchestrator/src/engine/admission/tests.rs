//! Run admission interface tests (#97): a temp `Storage`, an engine config
//! and an active-run map. No bridge, no dispatcher, no mock agent binary.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::{EventSeq, Storage};

use super::*;
use crate::engine::config::EngineConfig;
use crate::profile_loader::{DiskProfileSet, ProfileRegistry};

struct Fixture {
    home: crate::runtime_home_fixture::FixtureHome,
    worktree: tempfile::TempDir,
    storage: Arc<Storage>,
    config: EngineConfig,
    runs: tokio::sync::RwLock<HashMap<RunId, ActiveRun>>,
}

impl Fixture {
    async fn new() -> Self {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        Self {
            home,
            worktree: tempfile::tempdir().unwrap(),
            storage,
            config: EngineConfig::default(),
            runs: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    fn admission(&self) -> Admission<'_> {
        Admission {
            storage: &self.storage,
            config: &self.config,
            mcp_registry: None,
            active_runs: &self.runs,
        }
    }

    fn fresh(
        &self,
        run_id: RunId,
        graph: Graph,
        config: EngineRunConfig,
    ) -> AdmissionKind<'static> {
        AdmissionKind::Fresh {
            run_id,
            graph: Box::new(graph),
            worktree: self.worktree.path().to_path_buf(),
            config: Box::new(config),
        }
    }

    fn replayed(&self, run_id: RunId) -> AdmissionKind<'static> {
        AdmissionKind::Replayed {
            run_id,
            worktree: self.worktree.path().to_path_buf(),
        }
    }

    /// Start a run through fresh admission, then close its writer, leaving
    /// a durable non-terminal log behind for resume tests.
    async fn started(&self, config: EngineRunConfig) -> RunId {
        let run_id = RunId::new();
        let admitted = run(self
            .admission()
            .admit(self.fresh(run_id, graph("implementer@1.0"), config))
            .await);
        release(admitted).await;
        run_id
    }

    async fn kinds(&self, run_id: RunId) -> Vec<&'static str> {
        let reader = self.storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        reader
            .read_events(EventSeq(0)..last.next())
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.payload.payload.discriminant_str())
            .collect()
    }

    async fn append(&self, run_id: RunId, payload: EventPayload) {
        let writer = self.storage.open_run_writer(run_id).await.unwrap();
        writer
            .append_event(VersionedEventPayload::new(payload))
            .await
            .unwrap();
        writer.close().await.unwrap();
    }

    fn close(self) {
        drop(self.storage);
        self.home.close().unwrap();
    }
}

/// Close an admitted run's writer and drop everything else it holds (its
/// work estimator keeps a run reader open). An open handle keeps the run
/// database locked on Windows, so the fixture's directory could not be
/// removed.
async fn release(admitted: AdmittedRun) {
    let AdmittedRun { writer, .. } = admitted;
    writer.close().await.unwrap();
}

fn run(result: Result<Admitted, AdmissionRejection>) -> AdmittedRun {
    match result {
        Ok(Admitted::Run(admitted)) => *admitted,
        Ok(Admitted::Terminal { outcome, .. }) => {
            panic!("unexpected terminal admission: {outcome:?}")
        },
        Err(rejection) => panic!("unexpected rejection: {rejection}"),
    }
}

/// A one-agent graph whose agent references `profile`.
fn graph(profile: &str) -> Graph {
    let agent = NodeKey::try_from("implement").unwrap();
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent.clone(),
        Node {
            id: agent.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "stage completed".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: surge_core::LedgerEffect::default(),
            }],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from(profile).unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields: BTreeMap::default(),
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
        metadata: GraphMetadata::new("admission-harness", chrono::Utc::now()),
        start: agent.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("e_done").unwrap(),
            from: PortRef {
                node: agent,
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

fn registry() -> Arc<ProfileRegistry> {
    Arc::new(ProfileRegistry::new(DiskProfileSet::empty()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_profile_is_rejected_before_any_run_row_exists() {
    let mut fx = Fixture::new().await;
    fx.config.profile_registry = Some(registry());
    let run_id = RunId::new();
    let rejection = fx
        .admission()
        .admit(fx.fresh(
            run_id,
            graph("definitely-not-a-real-profile@1.0"),
            EngineRunConfig::default(),
        ))
        .await
        .err()
        .expect("an unknown profile must not be admitted");
    assert!(
        matches!(rejection, AdmissionRejection::GraphInvalid(_)),
        "{rejection}"
    );
    assert!(fx.storage.get_run(&run_id).await.unwrap().is_none());
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_skills_declaration_is_rejected_before_any_run_row_exists() {
    let fx = Fixture::new().await;
    let mut graph = graph("implementer@1.0");
    let NodeConfig::Agent(agent) = &mut graph
        .nodes
        .get_mut(&NodeKey::try_from("implement").unwrap())
        .unwrap()
        .config
    else {
        unreachable!()
    };
    let mut broken = toml::map::Map::new();
    broken.insert("provider".into(), toml::Value::String("project_dir".into()));
    agent.custom_fields.insert(
        "skills".into(),
        toml::Value::Array(vec![toml::Value::Table(broken)]),
    );
    let run_id = RunId::new();
    let rejection = fx
        .admission()
        .admit(fx.fresh(run_id, graph, EngineRunConfig::default()))
        .await
        .err()
        .expect("a broken skills declaration must not be admitted");
    assert!(
        matches!(rejection, AdmissionRejection::GraphInvalid(_)),
        "{rejection}"
    );
    assert!(fx.storage.get_run(&run_id).await.unwrap().is_none());
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_rotation_without_a_claim_is_rejected() {
    let mut fx = Fixture::new().await;
    fx.config.capacity.rotation = surge_core::capacity::RotationPolicy::Candidate {
        profile: "rotation".into(),
    };
    let run_id = RunId::new();
    let rejection = fx
        .admission()
        .admit(fx.fresh(run_id, graph("implementer@1.0"), EngineRunConfig::default()))
        .await
        .err()
        .expect("rotation needs durable task ownership");
    assert!(
        matches!(&rejection, AdmissionRejection::GraphInvalid(error) if error.to_string().contains("task ownership")),
        "{rejection}"
    );
    assert!(fx.storage.get_run(&run_id).await.unwrap().is_none());
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_worktree_and_active_runs_are_refused_before_any_run_row() {
    let fx = Fixture::new().await;
    let run_id = RunId::new();
    let missing = fx.worktree.path().join("gone");
    let rejection = fx
        .admission()
        .admit(AdmissionKind::Fresh {
            run_id,
            graph: Box::new(graph("implementer@1.0")),
            worktree: missing.clone(),
            config: Box::default(),
        })
        .await
        .err()
        .unwrap();
    assert!(matches!(&rejection, AdmissionRejection::WorktreeMissing(path) if *path == missing));

    let (amendments, _rx) = tokio::sync::mpsc::channel(1);
    fx.runs.write().await.insert(
        run_id,
        ActiveRun {
            cancel: tokio_util::sync::CancellationToken::new(),
            gate_resolutions: Arc::default(),
            tool_resolutions: Arc::default(),
            roadmap_amendments: amendments,
            pending_elevations: crate::engine::elevation::PendingElevations::new(),
            pending_steers: crate::engine::steer::new_queue(),
        },
    );
    let rejection = fx
        .admission()
        .admit(fx.fresh(run_id, graph("implementer@1.0"), EngineRunConfig::default()))
        .await
        .err()
        .unwrap();
    assert!(matches!(rejection, AdmissionRejection::AlreadyActive(id) if id == run_id));
    let rejection = fx
        .admission()
        .admit(fx.replayed(run_id))
        .await
        .err()
        .unwrap();
    assert!(matches!(rejection, AdmissionRejection::AlreadyActive(id) if id == run_id));
    assert!(fx.storage.get_run(&run_id).await.unwrap().is_none());
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resuming_a_parked_run_clears_the_row_records_the_wake_and_carries_the_evidence() {
    let fx = Fixture::new().await;
    let run_id = fx.started(EngineRunConfig::default()).await;
    fx.storage.set_run_parked(&run_id, 1).await.unwrap();

    let admitted = run(fx.admission().admit(fx.replayed(run_id)).await);
    let resumed = admitted
        .resume
        .as_ref()
        .expect("a resume carries replayed state");
    assert!(resumed.evidence.was_parked);
    assert!(resumed.evidence.bypasses_precheck());
    release(admitted).await;

    let summary = fx.storage.get_run(&run_id).await.unwrap().unwrap();
    assert_eq!(summary.status, surge_core::RunStatus::Running);
    assert_eq!(fx.kinds(run_id).await.last(), Some(&"RunWokeFromPark"));
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_re_arms_the_frozen_limits_and_resolves_the_memory_store() {
    let mut fx = Fixture::new().await;
    let budget = surge_core::budget::BudgetGuard {
        limits: surge_core::budget::BudgetLimits {
            usd: Some(2.5),
            tokens: Some(9_000),
            ..surge_core::budget::BudgetLimits::default()
        },
        ..surge_core::budget::BudgetGuard::default()
    };
    let mut config = EngineRunConfig {
        budget,
        ..EngineRunConfig::default()
    };
    config.bootstrap.edit_loop_cap = 7;
    let run_id = fx.started(config).await;
    let store = fx.worktree.path().join("memory.db");
    fx.config.memory_store_path = Some(store.clone());

    let admitted = run(fx.admission().admit(fx.replayed(run_id)).await);
    assert_eq!(admitted.run_config.budget, budget);
    assert_eq!(admitted.run_config.bootstrap.edit_loop_cap, 7);
    assert_eq!(admitted.run_config.memory_store_path, Some(store));
    let resumed = admitted.resume.as_ref().unwrap();
    assert!(!resumed.evidence.was_parked);
    assert!(!resumed.evidence.bypasses_precheck());
    release(admitted).await;
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resuming_a_terminal_log_yields_its_outcome() {
    let fx = Fixture::new().await;
    let run_id = fx.started(EngineRunConfig::default()).await;
    fx.append(
        run_id,
        EventPayload::RunAborted {
            reason: "operator".into(),
        },
    )
    .await;
    match fx.admission().admit(fx.replayed(run_id)).await {
        Ok(Admitted::Terminal { writer, outcome }) => {
            assert_eq!(
                outcome,
                RunOutcome::Aborted {
                    reason: "operator".into()
                }
            );
            writer.close().await.unwrap();
        },
        Ok(Admitted::Run(_)) => panic!("a terminal log must never execute"),
        Err(rejection) => panic!("unexpected rejection: {rejection}"),
    }
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_terminal_history_is_refused_without_side_effects() {
    let fx = Fixture::new().await;
    let run_id = fx.started(EngineRunConfig::default()).await;
    for reason in ["first", "second"] {
        fx.append(
            run_id,
            EventPayload::RunAborted {
                reason: reason.into(),
            },
        )
        .await;
    }
    fx.storage.set_run_parked(&run_id, 1).await.unwrap();
    let before = fx.kinds(run_id).await;
    let rejection = fx
        .admission()
        .admit(fx.replayed(run_id))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(rejection, AdmissionRejection::TerminalConflict { .. }),
        "{rejection}"
    );
    assert_eq!(fx.kinds(run_id).await, before);
    assert_eq!(
        fx.storage.get_run(&run_id).await.unwrap().unwrap().status,
        surge_core::RunStatus::Parked
    );
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_after_a_profile_was_removed_is_refused_before_any_mutation() {
    // Started without a registry (no profile resolution), then resumed under
    // one in which the referenced profile does not exist: the same as the
    // profile being removed while the run was parked.
    let mut fx = Fixture::new().await;
    let run_id = RunId::new();
    let admitted = run(fx
        .admission()
        .admit(fx.fresh(
            run_id,
            graph("removed-profile@1.0"),
            EngineRunConfig::default(),
        ))
        .await);
    release(admitted).await;
    fx.storage.set_run_parked(&run_id, 1).await.unwrap();
    let before = fx.kinds(run_id).await;
    fx.config.profile_registry = Some(registry());

    let rejection = fx
        .admission()
        .admit(fx.replayed(run_id))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(rejection, AdmissionRejection::ResumeGraphInvalid(_)),
        "{rejection}"
    );
    assert!(matches!(
        EngineError::from(rejection),
        EngineError::ResumeGraphInvalid(_)
    ));
    // Nothing durable changed: still parked, no wake, no failure.
    assert_eq!(fx.kinds(run_id).await, before);
    assert_eq!(
        fx.storage.get_run(&run_id).await.unwrap().unwrap().status,
        surge_core::RunStatus::Parked
    );
    // And the writer is free: another owner can open it.
    fx.storage
        .open_run_writer(run_id)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    fx.close();
}

#[test]
fn only_matching_capacity_continue_or_a_park_bypasses_the_precheck() {
    let control = |reserved, generation, capacity_reason| ResumeEvidence {
        was_parked: false,
        continue_control: Some(ContinueEvidence {
            reserved,
            generation,
            capacity_reason,
            claim_generation: 3,
        }),
    };
    assert!(control(true, 3, true).bypasses_precheck());
    assert!(
        !control(true, 2, true).bypasses_precheck(),
        "another generation's Continue"
    );
    assert!(
        !control(true, 3, false).bypasses_precheck(),
        "a non-capacity Continue"
    );
    assert!(!control(false, 3, true).bypasses_precheck(), "not reserved");
    assert!(
        ResumeEvidence {
            was_parked: true,
            continue_control: None
        }
        .bypasses_precheck()
    );
    assert!(
        !ResumeEvidence {
            was_parked: false,
            continue_control: None
        }
        .bypasses_precheck()
    );
}

impl Fixture {
    /// Reserve a durable task whose frozen config is the default and graph `graph`,
    /// exactly as the daemon's reservation step leaves it. Returns its run.
    fn reserve(&self, graph: Graph) -> RunId {
        use surge_core::work_item::{
            WorkItemCommand, WorkItemRequirements, WorkItemResult, WorkItemWorkspace,
        };
        let path = self.worktree.path().to_path_buf();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let workspace = WorkItemWorkspace {
            repository: path.join(".git").canonicalize().unwrap(),
            checkout: path.clone(),
            path: path.clone(),
            ownership: RunId::new().to_string(),
            branch: "fixture".into(),
            base_commit: "a".repeat(40),
        };
        let create = WorkItemCommand::Create {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            project: path,
            title: "Admission oracle".into(),
            requirements: WorkItemRequirements::new("Do it".into(), vec!["done".into()]).unwrap(),
        };
        let WorkItemResult::Detail(created) = self
            .storage
            .work_items()
            .mutate(&create, Some(&workspace), None, "human", 1)
            .unwrap()
        else {
            panic!("created")
        };
        let start = WorkItemCommand::Start {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: created.item.id,
            expected_version: created.item.version,
            graph: Box::new(graph),
            quota_recovery: None,
        };
        let frozen = serde_json::to_string(&EngineRunConfig::default()).unwrap();
        let WorkItemResult::Attempt(attempt) = self
            .storage
            .work_items()
            .mutate(&start, None, Some(&frozen), "host", 2)
            .unwrap()
        else {
            panic!("reserved")
        };
        attempt.run
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untracked_start_or_resume_of_a_reserved_task_requires_ownership() {
    let fx = Fixture::new().await;
    let run_id = fx.reserve(graph("implementer@1.0"));
    let rejection = fx
        .admission()
        .admit(fx.fresh(run_id, graph("implementer@1.0"), EngineRunConfig::default()))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(rejection, AdmissionRejection::OwnershipRequired(_)),
        "{rejection}"
    );
    let rejection = fx
        .admission()
        .admit(fx.replayed(run_id))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(rejection, AdmissionRejection::OwnershipRequired(_)),
        "{rejection}"
    );
    assert!(matches!(
        EngineError::from(rejection),
        EngineError::OwnershipRequired(_)
    ));
    assert!(fx.storage.get_run(&run_id).await.unwrap().is_none());
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_start_trusts_its_frozen_reservation_and_refuses_a_used_journal() {
    // The reservation's graph references a profile the registry does not
    // hold. Fresh admission would refuse it; a task start must not re-run
    // validation — freezing is the only producer of a task's config.
    let mut fx = Fixture::new().await;
    let run_id = fx.reserve(graph("not-in-the-registry@1.0"));
    fx.config.profile_registry = Some(registry());
    let claim = fx.storage.work_items().claim(run_id).unwrap();

    let admitted = run(fx.admission().admit(AdmissionKind::TaskStart(&claim)).await);
    assert_eq!(admitted.run_id, run_id);
    assert!(admitted.claim.is_some());
    assert!(admitted.resume.is_none());
    release(admitted).await;
    assert_eq!(fx.kinds(run_id).await.first(), Some(&"RunStarted"));

    let rejection = fx
        .admission()
        .admit(AdmissionKind::TaskStart(&claim))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(rejection, AdmissionRejection::NonEmptyJournal),
        "{rejection}"
    );
    fx.close();
}
