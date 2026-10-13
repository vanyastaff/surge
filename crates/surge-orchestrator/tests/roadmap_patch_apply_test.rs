//! Interface tests for "apply an approved roadmap patch" (#103): a patch, a
//! roadmap and a target in; the outcome or refusal, the owning run's events,
//! the roadmap file and the patch index out.

#[path = "fixtures/runtime_home.rs"]
mod runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use surge_core::approvals::{ApprovalChannel, ApprovalChannelKind, ApprovalDuration};
use surge_core::keys::NodeKey;
use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone, RoadmapStatus, RoadmapTask};
use surge_core::roadmap_patch::{
    InsertionPoint, OperatorConflictChoice, RoadmapItemRef, RoadmapPatch, RoadmapPatchApplyResult,
    RoadmapPatchConflict, RoadmapPatchConflictCode, RoadmapPatchId, RoadmapPatchItem,
    RoadmapPatchOperation, RoadmapPatchStatus, RoadmapPatchTarget,
};
use surge_core::run_event::EventPayload;
use surge_core::{ContentHash, Graph, RunId};
use surge_orchestrator::engine::config::EngineRunConfig;
use surge_orchestrator::engine::error::EngineError;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{RunHandle, RunOutcome, RunSummary};
use surge_orchestrator::roadmap_amendment::{
    ActiveRunAmendmentOutcome, NotAppliedReason, PatchApplyError, PatchApplyOutcome,
    PatchApplyPorts, PatchApplyRefusal, PatchApplyTarget, PatchApproval, RoadmapPatchApprovalLoop,
    apply_approved_patch, assess_patch,
};
use surge_orchestrator::roadmap_target::RoadmapAmendmentPoint;
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{RunWriter, Storage};

/// Records follow-up starts and active-run submissions; can refuse starts.
#[derive(Default)]
struct FakeEngine {
    started: Mutex<Vec<(RunId, Vec<String>)>>,
    submitted: Mutex<Vec<(RunId, RoadmapPatchId)>>,
    refuse_start: bool,
}

#[async_trait::async_trait]
impl EngineFacade for FakeEngine {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: Graph,
        _worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        if self.refuse_start {
            return Err(EngineError::Internal("daemon unavailable".into()));
        }
        self.started.lock().unwrap().push((
            run_id,
            run_config
                .seed_artifacts
                .iter()
                .map(|seed| seed.content.clone())
                .collect(),
        ));
        let (_events_tx, events) = tokio::sync::broadcast::channel(8);
        let completion = tokio::spawn(async {
            RunOutcome::Completed {
                terminal: NodeKey::try_from("success").unwrap(),
            }
        });
        Ok(RunHandle {
            run_id,
            events,
            completion,
        })
    }

    async fn resume_run(&self, run_id: RunId, _: PathBuf) -> Result<RunHandle, EngineError> {
        Err(EngineError::RunNotFound(run_id))
    }

    async fn stop_run(&self, run_id: RunId, _: String) -> Result<(), EngineError> {
        Err(EngineError::RunNotFound(run_id))
    }

    async fn submit_roadmap_amendment(
        &self,
        run_id: RunId,
        patch_id: RoadmapPatchId,
        _target: RoadmapPatchTarget,
        _patch_result: RoadmapPatchApplyResult,
    ) -> Result<ActiveRunAmendmentOutcome, EngineError> {
        self.submitted
            .lock()
            .unwrap()
            .push((run_id, patch_id.clone()));
        Ok(ActiveRunAmendmentOutcome {
            run_id,
            patch_id,
            previous_graph_hash: ContentHash::compute(b"before"),
            graph_hash: ContentHash::compute(b"after"),
            inserted_nodes: Vec::new(),
            roadmap_artifact: ContentHash::compute(b"roadmap"),
            flow_artifact: ContentHash::compute(b"flow"),
        })
    }

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        _: Option<String>,
        _: serde_json::Value,
    ) -> Result<(), EngineError> {
        Err(EngineError::RunNotFound(run_id))
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(Vec::new())
    }
}

struct Fixture {
    home: FixtureHome,
    storage: Arc<Storage>,
    owner: RunId,
    writer: RunWriter,
    artifacts: ArtifactStore,
    engine: Arc<FakeEngine>,
    roadmap_path: PathBuf,
}

impl Fixture {
    async fn new(roadmap: &RoadmapArtifact) -> Self {
        Self::with_engine(roadmap, FakeEngine::default()).await
    }

    async fn with_engine(roadmap: &RoadmapArtifact, engine: FakeEngine) -> Self {
        let home = FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let owner = RunId::new();
        let writer = storage.create_run(owner, home.path(), None).await.unwrap();
        let roadmap_path = home.path().join("roadmap.toml");
        std::fs::write(&roadmap_path, toml::to_string(roadmap).unwrap()).unwrap();
        Self {
            artifacts: ArtifactStore::new(home.path().join("runs")),
            home,
            storage,
            owner,
            writer,
            engine: Arc::new(engine),
            roadmap_path,
        }
    }

    fn target(&self, point: RoadmapAmendmentPoint) -> PatchApplyTarget {
        PatchApplyTarget {
            target: project_target(),
            amendment_point: point,
            roadmap_path: self.roadmap_path.clone(),
            roadmap_text: std::fs::read_to_string(&self.roadmap_path).unwrap(),
            active_run: None,
            project_path: self.home.path().to_path_buf(),
            worktree: self.home.path().to_path_buf(),
            project_context: None,
        }
    }

    async fn apply(
        &self,
        patch: &RoadmapPatch,
        choice: Option<OperatorConflictChoice>,
        target: &PatchApplyTarget,
    ) -> Result<PatchApplyOutcome, PatchApplyError> {
        let engine: Arc<dyn EngineFacade> = self.engine.clone();
        apply_approved_patch(
            PatchApplyPorts {
                artifact_store: &self.artifacts,
                storage: &self.storage,
                writer: &self.writer,
                owner_run: self.owner,
                engine: &engine,
            },
            patch,
            PatchApproval {
                conflict_choice: choice,
                comment: None,
                channel: ApprovalChannel::Desktop {
                    duration: ApprovalDuration::Persistent,
                },
                channel_used: ApprovalChannelKind::Desktop,
            },
            target,
        )
        .await
    }

    /// The patch lifecycle events on the owning run's log, in order.
    async fn lifecycle(&self) -> Vec<String> {
        self.writer.flush().await.unwrap();
        let reader = self.storage.open_run_reader(self.owner).await.unwrap();
        let events = reader.read_run_events().await.unwrap();
        drop(reader);
        events
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::RoadmapPatchApprovalRequested { .. } => Some("requested".into()),
                EventPayload::RoadmapPatchApprovalDecided {
                    decision,
                    conflict_choice,
                    ..
                } => Some(format!("decided {decision:?} {conflict_choice:?}")),
                EventPayload::RoadmapPatchApplied { .. } => Some("applied".into()),
                EventPayload::RoadmapUpdated { .. } => Some("roadmap_updated".into()),
                _ => None,
            })
            .collect()
    }

    fn status(&self, patch: &RoadmapPatch) -> Option<RoadmapPatchStatus> {
        self.storage
            .roadmap_patch_store()
            .get(&patch.id)
            .unwrap()
            .map(|record| record.status)
    }

    fn roadmap(&self) -> RoadmapArtifact {
        toml::from_str(&std::fs::read_to_string(&self.roadmap_path).unwrap()).unwrap()
    }

    async fn close(self) {
        self.writer.close().await.unwrap();
        drop(self.storage);
        self.home.close().unwrap();
    }
}

fn project_target() -> RoadmapPatchTarget {
    RoadmapPatchTarget::ProjectRoadmap {
        roadmap_path: "roadmap.toml".into(),
    }
}

fn milestone(id: &str, status: RoadmapStatus) -> RoadmapMilestone {
    let mut milestone = RoadmapMilestone::new(id, format!("Milestone {id}"));
    milestone.status = status;
    let mut task = RoadmapTask::new(format!("{id}-task"), "Existing task");
    task.status = status;
    milestone.tasks.push(task);
    milestone
}

fn roadmap(milestones: &[(&str, RoadmapStatus)]) -> RoadmapArtifact {
    RoadmapArtifact::new(
        milestones
            .iter()
            .map(|(id, status)| milestone(id, *status))
            .collect(),
    )
}

fn patch(id: &str, operations: Vec<RoadmapPatchOperation>) -> RoadmapPatch {
    let mut patch = RoadmapPatch::new(
        RoadmapPatchId::new(id).unwrap(),
        project_target(),
        operations,
    );
    patch.rationale = "Feature request".into();
    patch
}

fn add_task(milestone_id: &str, task_id: &str) -> RoadmapPatchOperation {
    RoadmapPatchOperation::AddTask {
        milestone_id: milestone_id.into(),
        task: RoadmapTask::new(task_id, "New task"),
        insertion: Some(InsertionPoint::AppendToMilestone {
            milestone_id: milestone_id.into(),
        }),
    }
}

fn task_ids(roadmap: &RoadmapArtifact, milestone_id: &str) -> Vec<String> {
    roadmap
        .milestones
        .iter()
        .find(|milestone| milestone.id == milestone_id)
        .map(|milestone| {
            milestone
                .tasks
                .iter()
                .map(|task| task.id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn milestone_ids(roadmap: &RoadmapArtifact) -> Vec<String> {
    roadmap
        .milestones
        .iter()
        .map(|milestone| milestone.id.to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clean_patch_rewrites_the_project_roadmap_and_records_the_full_lifecycle() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Pending)])).await;
    let patch = patch("rpatch-clean", vec![add_task("m1", "new-task")]);
    let outcome = fixture
        .apply(
            &patch,
            None,
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        PatchApplyOutcome::AppliedToProjectRoadmap { .. }
    ));
    assert_eq!(
        fixture.lifecycle().await,
        [
            "requested",
            "decided Approve None",
            "applied",
            "roadmap_updated"
        ]
    );
    assert_eq!(task_ids(&fixture.roadmap(), "m1"), ["m1-task", "new-task"]);
    assert_eq!(fixture.status(&patch), Some(RoadmapPatchStatus::Applied));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicts_without_a_choice_or_with_an_unoffered_one_are_refused_and_nothing_is_recorded() {
    let fixture = Fixture::new(&roadmap(&[
        ("m1", RoadmapStatus::Running),
        ("m0", RoadmapStatus::Completed),
    ]))
    .await;
    let target = fixture.target(RoadmapAmendmentPoint::ProjectFile);
    let running = patch("rpatch-running", vec![add_task("m1", "new-task")]);
    let Err(PatchApplyError::Refused(PatchApplyRefusal::ConflictChoiceRequired { choices })) =
        fixture.apply(&running, None, &target).await
    else {
        panic!("a conflicted patch needs a choice");
    };
    assert_eq!(choices.len(), 4, "a running milestone offers every choice");

    let completed = patch("rpatch-completed", vec![add_task("m0", "new-task")]);
    let refused = fixture
        .apply(
            &completed,
            Some(OperatorConflictChoice::DeferToNextMilestone),
            &target,
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(PatchApplyError::Refused(
                PatchApplyRefusal::ConflictChoiceUnavailable {
                    choice: OperatorConflictChoice::DeferToNextMilestone
                }
            ))
        ),
        "completed history cannot be deferred: {refused:?}"
    );
    assert_eq!(fixture.lifecycle().await, Vec::<String>::new());
    assert_eq!(fixture.status(&running), None);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_patch_is_refused_before_anything_is_recorded() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Pending)])).await;
    let malformed = patch("rpatch-empty", Vec::new());
    let refused = fixture
        .apply(
            &malformed,
            None,
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(PatchApplyError::Refused(
                PatchApplyRefusal::MalformedShape { .. }
            ))
        ),
        "{refused:?}"
    );
    assert_eq!(fixture.lifecycle().await, Vec::<String>::new());
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reject_and_abort_choices_record_the_decision_and_apply_nothing() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Running)])).await;
    let target = fixture.target(RoadmapAmendmentPoint::ProjectFile);
    let rejected = patch("rpatch-reject", vec![add_task("m1", "a")]);
    let outcome = fixture
        .apply(
            &rejected,
            Some(OperatorConflictChoice::RejectPatch),
            &target,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PatchApplyOutcome::Rejected {
            conflict_choice: OperatorConflictChoice::RejectPatch
        }
    );
    assert_eq!(
        fixture.status(&rejected),
        Some(RoadmapPatchStatus::Rejected)
    );

    let aborted = patch("rpatch-abort", vec![add_task("m1", "b")]);
    let outcome = fixture
        .apply(
            &aborted,
            Some(OperatorConflictChoice::AbortCurrentRun),
            &target,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PatchApplyOutcome::ApprovedNotApplied {
            reason: NotAppliedReason::AbortRequired { run_id: None }
        }
    );
    assert_eq!(fixture.status(&aborted), Some(RoadmapPatchStatus::Approved));
    assert_eq!(
        fixture.lifecycle().await,
        [
            "requested",
            "decided Reject Some(RejectPatch)",
            "requested",
            "decided Approve Some(AbortCurrentRun)",
        ]
    );
    assert_eq!(task_ids(&fixture.roadmap(), "m1"), ["m1-task"]);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_up_choice_starts_a_run_with_only_the_new_work() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Running)])).await;
    let patch = patch("rpatch-follow", vec![add_task("m1", "new-task")]);
    let outcome = fixture
        .apply(
            &patch,
            Some(OperatorConflictChoice::CreateFollowUpRun),
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    let PatchApplyOutcome::FollowUpStarted {
        run_id,
        from_conflict_choice: true,
    } = outcome
    else {
        panic!("expected a follow-up from the conflict choice: {outcome:?}");
    };
    let started = fixture.engine.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].0, run_id);
    assert!(started[0].1[0].contains("new-task"));
    assert_eq!(
        task_ids(&fixture.roadmap(), "m1"),
        ["m1-task"],
        "base roadmap untouched"
    );
    assert_eq!(fixture.status(&patch), Some(RoadmapPatchStatus::Applied));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_failure_leaves_the_patch_approved_not_applied() {
    let fixture = Fixture::with_engine(
        &roadmap(&[("m1", RoadmapStatus::Running)]),
        FakeEngine {
            refuse_start: true,
            ..FakeEngine::default()
        },
    )
    .await;
    let patch = patch("rpatch-engine-down", vec![add_task("m1", "new-task")]);
    let failed = fixture
        .apply(
            &patch,
            Some(OperatorConflictChoice::CreateFollowUpRun),
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await;
    assert!(
        matches!(failed, Err(PatchApplyError::Failed(_))),
        "{failed:?}"
    );
    assert_eq!(fixture.status(&patch), Some(RoadmapPatchStatus::Approved));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_target_holds_the_patch_and_a_later_apply_lands_it_once() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Pending)])).await;
    let patch = patch("rpatch-later", vec![add_task("m1", "new-task")]);
    let deferred = fixture.target(RoadmapAmendmentPoint::Deferred);
    for _ in 0..2 {
        let outcome = fixture.apply(&patch, None, &deferred).await.unwrap();
        assert_eq!(
            outcome,
            PatchApplyOutcome::ApprovedNotApplied {
                reason: NotAppliedReason::TargetNotAtPickupBoundary
            }
        );
    }
    assert_eq!(fixture.status(&patch), Some(RoadmapPatchStatus::Approved));
    assert_eq!(
        fixture.lifecycle().await,
        ["requested", "decided Approve None"],
        "re-applying reads the recorded approval"
    );

    let outcome = fixture
        .apply(
            &patch,
            None,
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        PatchApplyOutcome::AppliedToProjectRoadmap { .. }
    ));
    let again = fixture
        .apply(
            &patch,
            None,
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await;
    assert!(
        matches!(
            again,
            Err(PatchApplyError::Refused(PatchApplyRefusal::AlreadyApplied))
        ),
        "{again:?}"
    );
    assert_eq!(
        fixture.lifecycle().await,
        [
            "requested",
            "decided Approve None",
            "applied",
            "roadmap_updated"
        ]
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_active_run_target_goes_through_the_engine_queue() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Pending)])).await;
    let patch = patch("rpatch-active", vec![add_task("m1", "new-task")]);
    let mut target = fixture.target(RoadmapAmendmentPoint::ActiveRunBoundary);
    let refused = fixture.apply(&patch, None, &target).await;
    assert!(
        matches!(
            refused,
            Err(PatchApplyError::Refused(
                PatchApplyRefusal::ActiveRunWithoutRunId
            ))
        ),
        "{refused:?}"
    );
    let run = RunId::new();
    target.active_run = Some(run);
    let outcome = fixture.apply(&patch, None, &target).await.unwrap();
    assert!(
        matches!(outcome, PatchApplyOutcome::AppliedToActiveRun(ref active) if active.run_id == run)
    );
    assert_eq!(
        fixture.engine.submitted.lock().unwrap().clone(),
        [(run, patch.id.clone())]
    );
    assert_eq!(fixture.status(&patch), Some(RoadmapPatchStatus::Applied));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferral_moves_a_task_into_the_next_pending_milestone() {
    let fixture = Fixture::new(&roadmap(&[
        ("m1", RoadmapStatus::Running),
        ("m2", RoadmapStatus::Pending),
    ]))
    .await;
    let patch = patch("rpatch-defer-task", vec![add_task("m1", "new-task")]);
    fixture
        .apply(
            &patch,
            Some(OperatorConflictChoice::DeferToNextMilestone),
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    let amended = fixture.roadmap();
    assert_eq!(task_ids(&amended, "m1"), ["m1-task"]);
    assert_eq!(task_ids(&amended, "m2"), ["m2-task", "new-task"]);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferral_past_the_last_milestone_appends_a_deferred_milestone_with_a_unique_id() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Running)])).await;
    // Two tasks for the running last milestone in one patch: each gets a
    // deferred milestone, the second a unique id.
    let first = patch(
        "rpatch-defer-1",
        vec![add_task("m1", "first"), add_task("m1", "second")],
    );
    fixture
        .apply(
            &first,
            Some(OperatorConflictChoice::DeferToNextMilestone),
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    assert_eq!(
        milestone_ids(&fixture.roadmap()),
        ["m1", "m1-deferred", "m1-deferred-2"]
    );
    assert_eq!(task_ids(&fixture.roadmap(), "m1-deferred-2"), ["second"]);

    // A later deferral finds the deferred milestone pending and joins it.
    let later = patch("rpatch-defer-2", vec![add_task("m1", "third")]);
    fixture
        .apply(
            &later,
            Some(OperatorConflictChoice::DeferToNextMilestone),
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    assert_eq!(
        task_ids(&fixture.roadmap(), "m1-deferred"),
        ["first", "third"]
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferral_moves_a_milestone_anchored_on_a_running_one_before_the_next_pending() {
    for (milestones, expected) in [
        (
            vec![
                ("m1", RoadmapStatus::Running),
                ("m2", RoadmapStatus::Pending),
            ],
            vec!["m1", "new", "m2"],
        ),
        (vec![("m1", RoadmapStatus::Running)], vec!["m1", "new"]),
    ] {
        let fixture = Fixture::new(&roadmap(&milestones)).await;
        let patch = patch(
            "rpatch-defer-milestone",
            vec![RoadmapPatchOperation::AddMilestone {
                milestone: milestone("new", RoadmapStatus::Pending),
                insertion: Some(InsertionPoint::BeforeMilestone {
                    milestone_id: "m1".into(),
                }),
            }],
        );
        let outcome = fixture
            .apply(
                &patch,
                Some(OperatorConflictChoice::DeferToNextMilestone),
                &fixture.target(RoadmapAmendmentPoint::ProjectFile),
            )
            .await;
        match outcome {
            Ok(_) => assert_eq!(milestone_ids(&fixture.roadmap()), expected),
            // Inserting before a running milestone may not conflict at all.
            Err(PatchApplyError::Refused(PatchApplyRefusal::ConflictChoiceUnavailable {
                ..
            })) => {},
            Err(error) => panic!("{error:?}"),
        }
        fixture.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferral_turns_a_replacement_of_running_work_into_new_pending_work() {
    let fixture = Fixture::new(&roadmap(&[
        ("m1", RoadmapStatus::Running),
        ("m2", RoadmapStatus::Pending),
    ]))
    .await;
    let mut replacement_task = RoadmapTask::new("m1-task-v2", "Reworked task");
    replacement_task.status = RoadmapStatus::Running;
    let patch = patch(
        "rpatch-defer-replace",
        vec![
            RoadmapPatchOperation::ReplaceDraftItem {
                target: RoadmapItemRef::Task {
                    milestone_id: "m1".into(),
                    task_id: "m1-task".into(),
                },
                replacement: RoadmapPatchItem::Task {
                    task: replacement_task,
                },
                reason: "rework".into(),
            },
            RoadmapPatchOperation::ReplaceDraftItem {
                target: RoadmapItemRef::Milestone {
                    milestone_id: "m1".into(),
                },
                replacement: RoadmapPatchItem::Milestone {
                    milestone: milestone("m1-v2", RoadmapStatus::Running),
                },
                reason: "rework".into(),
            },
        ],
    );
    fixture
        .apply(
            &patch,
            Some(OperatorConflictChoice::DeferToNextMilestone),
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    let amended = fixture.roadmap();
    assert_eq!(
        task_ids(&amended, "m1"),
        ["m1-task"],
        "running work untouched"
    );
    assert_eq!(task_ids(&amended, "m2"), ["m2-task", "m1-task-v2"]);
    assert_eq!(milestone_ids(&amended), ["m1", "m1-v2", "m2"]);
    assert!(
        amended
            .milestones
            .iter()
            .filter(|milestone| milestone.id != "m1")
            .all(|milestone| milestone.status == RoadmapStatus::Pending
                && milestone
                    .tasks
                    .iter()
                    .all(|task| task.status == RoadmapStatus::Pending)),
        "deferred work is pending"
    );
    fixture.close().await;
}

#[test]
fn assessment_merges_a_declared_conflict_and_keeps_distinct_ones() {
    let roadmap = roadmap(&[("m1", RoadmapStatus::Running)]);
    let mut patch = patch("rpatch-assess", vec![add_task("m1", "new-task")]);
    // The planner declared the conflict the apply step detects (same code and
    // item) with its own wording, plus one the roadmap cannot show.
    let Err(surge_core::roadmap_patch::RoadmapPatchApplyError::Conflicts { conflicts }) =
        patch.apply_to_roadmap(&roadmap)
    else {
        panic!("a running milestone conflicts");
    };
    let detected_item = conflicts[0].to_patch_conflict().item;
    patch.conflicts = vec![
        RoadmapPatchConflict {
            code: RoadmapPatchConflictCode::RunningMilestone,
            item: detected_item,
            message: "the planner saw m1 running".into(),
            choices: vec![OperatorConflictChoice::CreateFollowUpRun],
            selected_choice: None,
        },
        RoadmapPatchConflict {
            code: RoadmapPatchConflictCode::DependencyCycle,
            item: None,
            message: "planner saw a cycle".into(),
            choices: vec![OperatorConflictChoice::RejectPatch],
            selected_choice: None,
        },
    ];
    let assessment = assess_patch(&patch, &roadmap);
    assert_eq!(assessment.malformed, None);
    assert_eq!(
        assessment.patch.conflicts.len(),
        2,
        "the detected conflict merges into the declared one"
    );
    let running = &assessment.patch.conflicts[0];
    assert_eq!(running.message, "the planner saw m1 running");
    assert_eq!(running.choices, [OperatorConflictChoice::CreateFollowUpRun]);
    assert_eq!(assessment.patch.conflicts[1].message, "planner saw a cycle");
    assert_eq!(
        assessment.choices,
        [
            OperatorConflictChoice::CreateFollowUpRun,
            OperatorConflictChoice::RejectPatch
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_records_the_decision_after_an_approval_the_loop_requested() {
    let fixture = Fixture::new(&roadmap(&[("m1", RoadmapStatus::Pending)])).await;
    let patch = patch("rpatch-loop", vec![add_task("m1", "new-task")]);
    RoadmapPatchApprovalLoop::new(0)
        .request_approval(
            &fixture.writer,
            &patch,
            ApprovalChannel::Desktop {
                duration: ApprovalDuration::Persistent,
            },
        )
        .await
        .unwrap();
    fixture
        .apply(
            &patch,
            None,
            &fixture.target(RoadmapAmendmentPoint::ProjectFile),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.lifecycle().await,
        [
            "requested",
            "decided Approve None",
            "applied",
            "roadmap_updated"
        ]
    );
    fixture.close().await;
}
