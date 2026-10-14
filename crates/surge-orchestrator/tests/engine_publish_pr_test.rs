//! #127: a run with a pull-request publisher publishes at its `Success`
//! terminal, before `RunCompleted`, and records the PR in its own log.
//! Publishing never fails the run.

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::publish::{
    PublishOutcome, PublishRequest, PublishedPullRequest, PullRequestPublisher,
};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

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
        metadata: GraphMetadata {
            name: "publish-pr".into(),
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

/// Answers with a fixed result and records each request.
#[derive(Debug)]
struct FakePublisher {
    answer: Result<PublishOutcome, String>,
    requests: Mutex<Vec<PublishRequest>>,
}

impl FakePublisher {
    fn new(answer: Result<PublishOutcome, String>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            requests: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl PullRequestPublisher for FakePublisher {
    async fn publish(&self, request: &PublishRequest) -> Result<PublishOutcome, String> {
        self.requests.lock().unwrap().push(request.clone());
        self.answer.clone()
    }
}

fn published() -> PublishOutcome {
    PublishOutcome::Published(PublishedPullRequest {
        pr: serde_json::from_value(serde_json::json!({
            "provider": "github",
            "repository": "acme/app",
            "number": 57,
            "url": "https://github.com/acme/app/pull/57",
        }))
        .unwrap(),
        head_branch: "surge/run-01abc".into(),
        head_sha: "0123456789abcdef0123456789abcdef01234567".into(),
    })
}

/// Run the terminal-only graph with `publisher` and return the run id, the
/// outcome and the event kinds of its log.
async fn run_with(publisher: Option<Arc<FakePublisher>>) -> (RunId, RunOutcome, Vec<EventPayload>) {
    let dir = FixtureHome::new().unwrap();
    let result;
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());
        let run_id = RunId::new();
        let run_config = EngineRunConfig {
            pull_request_publisher: publisher.map(|p| p as Arc<dyn PullRequestPublisher>),
            ..EngineRunConfig::default()
        };
        let outcome = engine
            .start_run(
                run_id,
                terminal_graph(),
                dir.path().to_path_buf(),
                run_config,
            )
            .await
            .expect("start_run")
            .await_completion()
            .await
            .unwrap();
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_run_events()
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.payload)
            .collect();
        drop(reader);
        drop(engine);
        drop(storage);
        result = (run_id, outcome, events);
    }
    dir.close().unwrap();
    result
}

fn auto_pr(events: &[EventPayload]) -> bool {
    events
        .iter()
        .find_map(|event| match event {
            EventPayload::RunStarted { config, .. } => Some(config.auto_pr),
            _ => None,
        })
        .expect("RunStarted")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_published_pr_is_recorded_right_before_run_completed() {
    let publisher = FakePublisher::new(Ok(published()));
    let (run_id, outcome, events) = run_with(Some(Arc::clone(&publisher))).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert!(
        auto_pr(&events),
        "RunStarted records that this run publishes"
    );
    let tail: Vec<&str> = events
        .iter()
        .rev()
        .take(2)
        .map(|event| event.discriminant_str())
        .collect();
    assert_eq!(tail, ["RunCompleted", "PullRequestOpened"]);
    let EventPayload::PullRequestOpened {
        pr, head_branch, ..
    } = &events[events.len() - 2]
    else {
        unreachable!()
    };
    assert_eq!((pr.number, head_branch.as_str()), (57, "surge/run-01abc"));
    let requests = publisher.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].run_id, run_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_to_publish_and_a_failure_both_complete_without_a_record() {
    for answer in [
        Ok(PublishOutcome::NothingToPublish),
        Err("push rejected: protected branch".to_owned()),
    ] {
        let (_, outcome, events) = run_with(Some(FakePublisher::new(answer.clone()))).await;
        assert!(
            matches!(outcome, RunOutcome::Completed { .. }),
            "{answer:?}: {outcome:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, EventPayload::PullRequestOpened { .. })),
            "{answer:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_without_a_publisher_publishes_nothing_and_says_so() {
    let (_, outcome, events) = run_with(None).await;
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert!(!auto_pr(&events));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventPayload::PullRequestOpened { .. }))
    );
}
