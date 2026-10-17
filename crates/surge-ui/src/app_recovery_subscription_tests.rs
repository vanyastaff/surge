//! Actual application subscription and durable-hydration failure/retry boundary.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, TestAppContext};
use surge_core::{EventPayload, RunId, run_event::VersionedEventPayload};
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunStatus};
use surge_orchestrator::engine::ipc::{
    DaemonRequest, DaemonResponse, read_request_frame, write_frame,
};

use super::super::SurgeApp;
use crate::app_state::{AppState, UiRun};

#[test]
fn recovery_repair_reconnect_failed_hydration_keeps_fence_until_actual_retry() {
    for route in [
        "recovery-subscription-working",
        "recovery-subscription-pending",
    ] {
        super::run_isolated_app_probe(
            route,
            "app::tests::recovery_subscription_tests::recovery_repair_reconnect_child_probe",
        );
    }
}

#[test]
fn recovery_repair_reconnect_child_probe() {
    let Ok(route) = std::env::var("SURGE_TEST_PROJECT_HOME_ENTRY") else {
        return;
    };
    if !matches!(
        route.as_str(),
        "recovery-subscription-working" | "recovery-subscription-pending"
    ) {
        return;
    }
    run_reconnect_probe(route.ends_with("pending"));
}

fn startup() -> EventPayload {
    EventPayload::RunStarted {
        pipeline_template: None,
        project_path: "/reconnect-fixture".into(),
        initial_prompt: "observe history".into(),
        config: surge_core::RunConfig {
            budget: Default::default(),
            sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
            approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: Vec::new(),
            bootstrap_edit_loop_cap: None,
        },
    }
}

fn prefix(pending: bool) -> Vec<EventPayload> {
    let mut events = vec![
        startup(),
        EventPayload::PipelineMaterialized {
            graph: Box::new(
                toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap(),
            ),
            graph_hash: surge_core::ContentHash::compute(b"reconnect graph"),
        },
        EventPayload::RunRecoveryRequired {
            control_generation: 0,
            diagnostic: "retained recovery history".into(),
        },
        EventPayload::StageEntered {
            node: "impl_1".parse().unwrap(),
            attempt: 1,
        },
    ];
    if pending {
        events.push(EventPayload::HumanInputRequested {
            node: "impl_1".parse().unwrap(),
            session: None,
            call_id: Some("original-question".into()),
            prompt: "Retain this decision".into(),
            schema: None,
        });
    }
    events
}

struct SubscriptionPeer {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    subscribed: Arc<AtomicUsize>,
}
impl SubscriptionPeer {
    async fn start(path: &std::path::Path, run: RunId) -> Self {
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        let (shutdown, mut stopped) = tokio::sync::oneshot::channel();
        let subscribed = Arc::new(AtomicUsize::new(0));
        let count = subscribed.clone();
        let task = tokio::spawn(async move {
            let (stream, _) = tokio::select! {
                result = listener.accept() => result.unwrap(),
                _ = &mut stopped => return,
            };
            let (read, mut write) = stream.into_split();
            let mut read = tokio::io::BufReader::new(read);
            loop {
                let request = tokio::select! {
                    result = read_request_frame(&mut read) => result.unwrap(),
                    _ = &mut stopped => return,
                };
                let Some(DaemonRequest::Subscribe { request_id, run_id }) = request else {
                    return;
                };
                assert_eq!(run_id, run, "only the exact run is subscribed");
                write_frame(&mut write, &DaemonResponse::SubscribeOk { request_id })
                    .await
                    .unwrap();
                count.fetch_add(1, Ordering::SeqCst);
            }
        });
        Self {
            shutdown: Some(shutdown),
            task: Some(task),
            subscribed,
        }
    }
    fn close(&mut self, host: &tokio::runtime::Runtime) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            host.block_on(async {
                tokio::time::timeout(Duration::from_secs(3), task)
                    .await
                    .unwrap()
                    .unwrap();
            });
        }
    }
}
impl Drop for SubscriptionPeer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn drive_until(
    host: &tokio::runtime::Runtime,
    cx: &mut TestAppContext,
    condition: impl Fn(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(cx) {
        assert!(
            Instant::now() < deadline,
            "actual application subscription/retry did not settle"
        );
        cx.run_until_parked();
        host.block_on(tokio::time::sleep(Duration::from_millis(5)));
    }
}

fn run_reconnect_probe(pending: bool) {
    let home = std::path::PathBuf::from(std::env::var_os("SURGE_HOME").unwrap());
    let home = home.canonicalize().unwrap();
    let runs = home.join("runs");
    std::fs::write(&runs, b"not a directory").unwrap();
    let host = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _entered = host.enter();
    let run = RunId::new();
    let socket = home.join("reconnect.sock");
    let mut peer = host.block_on(SubscriptionPeer::start(&socket, run));
    let facade = Arc::new(host.block_on(DaemonEngineFacade::connect(socket)).unwrap());
    let events = prefix(pending);
    let mut stream = crate::run_stream::RunStreamState::default();
    stream.begin_display_history(run, None);
    for (index, payload) in events.iter().enumerate() {
        stream.apply_recorded(
            &EngineRunEvent::Persisted {
                seq: index as u64 + 1,
                payload: Box::new(payload.clone()),
            },
            0,
        );
    }
    stream.finish_display_history(events.len() as u64);
    stream.live = true;
    assert!(stream.observation_available_for(run));
    if !pending {
        assert_eq!(stream.display().label(), "Running");
    }
    stream.apply(&EngineRunEvent::StreamError {
        message: "prior connection ended".into(),
    });
    let mut cx = TestAppContext::single();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init();
        crate::theme::sync_component_theme(cx);
    });
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some("/reconnect-fixture".into());
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone());
        state.runs.push(UiRun {
            run_id: run,
            status: RunStatus::Active,
            started_at: chrono::Utc::now(),
            last_event_seq: Some(events.len() as u64),
            ended_at: None,
        });
        state.run_streams.insert(run, stream);
        state
    });
    let app = cx.new(|cx| SurgeApp::new_shell(state.clone(), cx));
    app.update(&mut cx, |app, cx| app.sync_run_subscriptions(cx));
    drive_until(&host, &mut cx, |cx| {
        peer.subscribed.load(Ordering::SeqCst) == 1
            && cx.read(|cx| state.read(cx).run_streams[&run].live)
    });
    // The actual application has accepted the subscription and awaited the failed read.
    let blocked = cx.read(|cx| {
        let stream = &state.read(cx).run_streams[&run];
        assert_eq!(
            stream.recovery().latest().unwrap().diagnostic,
            "retained recovery history"
        );
        assert_eq!(stream.pending.len(), usize::from(pending));
        if pending {
            assert_eq!(
                stream.pending[0].call_id().as_deref(),
                Some("original-question")
            );
        }
        stream.display().label() == "Run state is unconfirmed"
            && stream.observation_error.is_some()
            && !stream.observation_available_for(run)
    });
    // Make the same retained run readable. Only the real in-loop retry may clear the fence.
    std::fs::remove_file(&runs).unwrap();
    let storage = host
        .block_on(surge_persistence::runs::Storage::open_with(
            &home,
            Arc::new(surge_persistence::runs::MockClock::new(0)),
        ))
        .unwrap();
    let mut restored_events = events.clone();
    restored_events.push(EventPayload::StageEntered {
        node: "impl_1".parse().unwrap(),
        attempt: 2,
    });
    host.block_on(async {
        let writer = storage
            .create_run(run, std::path::Path::new("/reconnect-fixture"), None)
            .await
            .unwrap();
        writer
            .append_events(
                restored_events
                    .iter()
                    .cloned()
                    .map(VersionedEventPayload::new)
                    .collect(),
            )
            .await
            .unwrap();
        writer.close().await.unwrap();
    });
    cx.background_executor.advance_clock(Duration::from_secs(1));
    drive_until(&host, &mut cx, |cx| {
        cx.read(|cx| {
            let stream = &state.read(cx).run_streams[&run];
            stream.last_seq == restored_events.len() as u64 && stream.observation_available_for(run)
        })
    });
    cx.read(|cx| {
        let stream = &state.read(cx).run_streams[&run];
        assert!(stream.observation_error.is_none());
        assert_eq!(
            stream.recovery().latest().unwrap().diagnostic,
            "retained recovery history"
        );
        assert_eq!(stream.pending.len(), usize::from(pending));
        assert_eq!(
            stream.display().label(),
            if pending {
                "Waiting for human input"
            } else {
                "Running"
            }
        );
        assert!(!stream.observation_available_for(RunId::new()));
    });
    peer.close(&host);
    drive_until(&host, &mut cx, |cx| {
        cx.read(|cx| !state.read(cx).run_streams[&run].live)
    });
    drop(app);
    drop(state);
    drop(cx);
    drop(storage);
    drop(facade);
    assert!(
        blocked,
        "successful subscription plus failed durable rehydration promoted stale history to current work"
    );
}
