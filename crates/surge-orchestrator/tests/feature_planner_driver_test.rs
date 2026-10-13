//! Feature Planner through the standalone agent invocation (#99): scripted
//! `MockBridge` sessions in, a typed patch result (and the planner run's log
//! evidence) out.

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, RunId, SessionId};
use surge_orchestrator::agent_invocation::InvocationFailure;
use surge_orchestrator::feature_driver::{
    FeaturePlannerError, FeaturePlannerParams, FeaturePlannerResult, run_feature_planner,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::{EventSeq, Storage};
use tokio_util::sync::CancellationToken;

const VALID_PATCH: &str = r#"schema_version = 1
id = "rpatch-driver"
rationale = "The requested follow-up belongs on the current roadmap."
status = "drafted"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "add_milestone"

[operations.milestone]
id = "m2"
title = "Driver feature"

[operations.insertion]
kind = "append_to_roadmap"
"#;

/// The bundled profile's shape, launched on the in-process mock agent.
const TEST_FEATURE_PLANNER_PROFILE: &str = r#"schema_version = 1

[role]
id = "feature-planner"
version = "1.0.0"
display_name = "Feature Planner Test"
category = "agents"
description = "Feature planner on the mock agent."
when_to_use = "Feature planner driver tests."

[runtime]
recommended_model = "test-model"
agent_id = "mock"

[sandbox]
mode = "workspace-write"

[[outcomes]]
id = "patched"
description = "Roadmap patch drafted."
edge_kind_hint = "forward"
required_artifacts = ["roadmap-patch.toml"]

[[outcomes.produced_artifacts]]
path = "roadmap-patch.toml"
contract = { kind = "roadmap-patch", schema_version = 1 }

[[outcomes]]
id = "out_of_scope"
description = "Out of scope."
edge_kind_hint = "escalate"

[prompt]
system = "Request: {{request}}\nRoadmap: {{roadmap}}"
"#;

struct Fixture {
    home: FixtureHome,
    storage: Arc<Storage>,
    run_id: RunId,
    mock: Arc<fixtures::mock_bridge::MockBridge>,
    bridge: Arc<dyn BridgeFacade>,
    session: SessionId,
    registry: Arc<ProfileRegistry>,
}

impl Fixture {
    async fn new() -> Self {
        let home = FixtureHome::new().unwrap();
        let profiles_dir = home.path().join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(
            profiles_dir.join("feature-planner-1.0.toml"),
            TEST_FEATURE_PLANNER_PROFILE,
        )
        .unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let session = SessionId::new();
        mock.pin_next_session_id(session).await;
        Self {
            registry: Arc::new(ProfileRegistry::new(
                DiskProfileSet::scan(&profiles_dir).unwrap(),
            )),
            bridge: mock.clone(),
            home,
            storage,
            run_id: RunId::new(),
            mock,
            session,
        }
    }

    /// Run the planner while the agent reports `outcome` once the prompt
    /// was sent (or, with `cancel`, cancel instead).
    async fn run(
        &self,
        outcome: Option<&'static str>,
        cancel: &CancellationToken,
    ) -> Result<FeaturePlannerResult, FeaturePlannerError> {
        let writer = self
            .storage
            .create_run(self.run_id, self.home.path(), None)
            .await
            .unwrap();
        let agent = async {
            for _ in 0..500 {
                let sent = self.mock.recorded_calls.lock().await.iter().any(|call| {
                    matches!(
                        call,
                        fixtures::mock_bridge::RecordedCall::SendMessage { .. }
                    )
                });
                if sent {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            match outcome {
                Some(outcome) => {
                    self.mock
                        .enqueue_event(BridgeEvent::OutcomeReported {
                            session: self.session,
                            outcome: OutcomeKey::from_str(outcome).unwrap(),
                            summary: "planned".into(),
                            artifacts_produced: if outcome == "patched" {
                                vec!["roadmap-patch.toml".into()]
                            } else {
                                vec![]
                            },
                            verification_report: None,
                        })
                        .await;
                    self.mock.pump_scripted_events().await;
                },
                None => cancel.cancel(),
            }
        };
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(self.home.path().join("runs"));
        let planner = run_feature_planner(FeaturePlannerParams {
            request: "Add driver test".into(),
            roadmap: "## Milestones\n- m1".into(),
            bridge: &self.bridge,
            writer: &writer,
            artifact_store: &artifact_store,
            worktree_path: self.home.path(),
            run_id: self.run_id,
            profile_registry: self.registry.clone(),
            agent_registry: None,
            cancel,
            deadline: Duration::from_secs(2),
        });
        let (result, ()) = tokio::join!(planner, agent);
        writer.close().await.unwrap();
        result
    }

    async fn closes(&self) -> usize {
        self.mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, fixtures::mock_bridge::RecordedCall::CloseSession(_)))
            .count()
    }

    async fn logged_kinds(&self) -> Vec<&'static str> {
        let reader = self.storage.open_run_reader(self.run_id).await.unwrap();
        reader
            .read_events(EventSeq(0)..EventSeq(64))
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.payload.payload.discriminant_str())
            .collect()
    }

    fn close(self) {
        drop(self.storage);
        self.home.close().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_valid_patch_is_returned_and_recorded_in_the_planner_log() {
    let fx = Fixture::new().await;
    std::fs::write(fx.home.path().join("roadmap-patch.toml"), VALID_PATCH).unwrap();
    let result = fx
        .run(Some("patched"), &CancellationToken::new())
        .await
        .unwrap();
    match result {
        FeaturePlannerResult::Patched { patch, patch_path } => {
            assert_eq!(patch.id.as_str(), "rpatch-driver");
            assert!(patch_path.ends_with("roadmap-patch.toml"));
        },
        other => panic!("expected Patched, got {other:?}"),
    }
    assert_eq!(
        fx.logged_kinds().await,
        ["ArtifactProduced", "OutcomeReported"]
    );
    assert_eq!(fx.closes().await, 1);
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_scope_is_a_typed_result() {
    let fx = Fixture::new().await;
    let result = fx
        .run(Some("out_of_scope"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result, FeaturePlannerResult::OutOfScope);
    assert_eq!(fx.closes().await, 1);
    fx.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_that_stays_invalid_is_refused() {
    let fx = Fixture::new().await;
    std::fs::write(fx.home.path().join("roadmap-patch.toml"), "not = [a patch").unwrap();
    // One report only: the in-session corrections never arrive, so the
    // invocation fails at its deadline rather than accepting the patch.
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        fx.run(Some("patched"), &CancellationToken::new()),
    )
    .await
    .expect("planner finishes")
    .unwrap_err();
    assert!(
        matches!(
            error,
            FeaturePlannerError::Invocation(InvocationFailure::DeadlineElapsed(_))
        ),
        "{error}"
    );
    assert_eq!(fx.closes().await, 1);
    fx.close();
}

/// Regression (#99): Ctrl-C during `surge feature describe` did not cancel
/// the agent. The planner now stops and closes its session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_stops_the_planner_and_closes_its_session() {
    let fx = Fixture::new().await;
    let cancel = CancellationToken::new();
    let error = fx.run(None, &cancel).await.unwrap_err();
    assert!(
        matches!(
            error,
            FeaturePlannerError::Invocation(InvocationFailure::Cancelled)
        ),
        "{error}"
    );
    assert_eq!(fx.closes().await, 1);
    fx.close();
}
