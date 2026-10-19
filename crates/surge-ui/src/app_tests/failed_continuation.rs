//! Native Continue response → application-owned status/polling, not a screen-only cache.
#![cfg(unix)]

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext};
use surge_core::{
    ContentHash, RunId,
    bootstrap_attempt::{BootstrapAttempt, BootstrapAttemptState as AttemptState},
    bootstrap_continuation::{BootstrapFailure, BootstrapTerminal},
    bootstrap_operation::{BootstrapOperationStatus as Status, BootstrapState},
};
use surge_orchestrator::engine::{
    daemon_facade::DaemonEngineFacade,
    handle::RunStatus,
    ipc::{DaemonRequest, DaemonResponse, ErrorCode, read_request_frame, write_frame},
};
use tokio::sync::{Semaphore, oneshot};

use super::SurgeApp;
use crate::{
    app_state::{AppState, UiRun},
    screens::runs::RunsScreen,
};

fn failed_status() -> Status {
    let implementation_run = RunId::new();
    Status {
        operation_id: RunId::new(),
        planning_run: RunId::new(),
        implementation_run,
        state: BootstrapState::Failed,
        result: Some(BootstrapTerminal::Failed {
            run_id: implementation_run,
            reason: BootstrapFailure::RunFailed,
        }),
        revision: 7,
        cancel_requested: false,
        queue_sequence: 1,
        payload_version: 1,
        continuations: Vec::new(),
    }
}

struct Peer {
    facade: Arc<DaemonEngineFacade>,
    status: Arc<Mutex<Status>>,
    continue_seen: Arc<AtomicUsize>,
    polls: Arc<AtomicUsize>,
    sent: Arc<AtomicUsize>,
    status_sent: Arc<AtomicUsize>,
    responses: Arc<Semaphore>,
    release: Option<oneshot::Sender<()>>,
    stop: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

impl Peer {
    fn new(runtime: &tokio::runtime::Runtime, home: &Path, initial: Status, held: bool) -> Self {
        let socket = home.join(format!("continue-{}.sock", RunId::new().short()));
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let status = Arc::new(Mutex::new(initial));
        let continue_seen = Arc::new(AtomicUsize::new(0));
        let polls = Arc::new(AtomicUsize::new(0));
        let sent = Arc::new(AtomicUsize::new(0));
        let status_sent = Arc::new(AtomicUsize::new(0));
        let responses = Arc::new(Semaphore::new(0));
        let (release, gate) = oneshot::channel();
        let mut gate = held.then_some(gate);
        let (stop, mut stopped) = oneshot::channel();
        let current = status.clone();
        let continued = continue_seen.clone();
        let requests = polls.clone();
        let written = sent.clone();
        let status_written = status_sent.clone();
        let permits = responses.clone();
        let server = runtime.spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = tokio::io::BufReader::new(read);
            loop {
                let request = tokio::select! {
                    _ = &mut stopped => return,
                    frame = read_request_frame(&mut read) => match frame.unwrap() {
                        Some(request) => request,
                        None => return,
                    },
                };
                let request_id = request.request_id();
                let is_status = matches!(&request, DaemonRequest::BootstrapStatus { .. });
                let response = match request {
                    DaemonRequest::ContinueBootstrapImplementation {
                        operation_id,
                        revision,
                        source_run,
                        continuation_id,
                        ..
                    } => {
                        {
                            let mut status = current.lock().unwrap();
                            assert_eq!(operation_id, status.operation_id);
                            assert_eq!(revision, status.revision);
                            assert_eq!(source_run, status.implementation_run);
                            assert!(status.continuations.is_empty());
                            status.revision += 1;
                            status.continuations.push(BootstrapAttempt {
                                continuation_id,
                                operation_id,
                                source_run,
                                run: RunId::new(),
                                boundary: 87,
                                runtime: ContentHash::compute(b"host capture"),
                                state: AttemptState::Reserved,
                            });
                        }
                        continued.fetch_add(1, Ordering::SeqCst);
                        if let Some(wait) = gate.take() {
                            tokio::select! { _ = wait => {}, _ = &mut stopped => return }
                        }
                        DaemonResponse::BootstrapOperation {
                            request_id,
                            status: Box::new(current.lock().unwrap().clone()),
                        }
                    },
                    DaemonRequest::BootstrapStatus { operation_id, .. } => {
                        assert_eq!(operation_id, current.lock().unwrap().operation_id);
                        requests.fetch_add(1, Ordering::SeqCst);
                        tokio::select! {
                            permit = permits.acquire() => permit.unwrap().forget(),
                            _ = &mut stopped => return,
                        }
                        DaemonResponse::BootstrapOperation {
                            request_id,
                            status: Box::new(current.lock().unwrap().clone()),
                        }
                    },
                    _ => DaemonResponse::Error {
                        request_id,
                        code: ErrorCode::RunNotActive,
                        message: "no active execution in controlled UI peer".into(),
                    },
                };
                write_frame(&mut write, &response).await.unwrap();
                written.fetch_add(1, Ordering::SeqCst);
                if is_status {
                    status_written.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        let facade = Arc::new(
            runtime
                .block_on(DaemonEngineFacade::connect(socket))
                .unwrap(),
        );
        Self {
            facade,
            status,
            continue_seen,
            polls,
            sent,
            status_sent,
            responses,
            release: held.then_some(release),
            stop,
            server,
        }
    }

    fn publish(&self, state: AttemptState, advance_revision: bool) {
        let mut status = self.status.lock().unwrap();
        if advance_revision {
            status.revision += 1;
        }
        status.continuations[0].state = state;
        drop(status);
        self.responses.add_permits(1);
    }

    fn close(self, runtime: &tokio::runtime::Runtime) {
        let _ = self.stop.send(());
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), self.server)
                .await
                .expect("controlled native peer stops")
                .unwrap();
        });
    }
}

struct Harness {
    app: Entity<SurgeApp>,
    state: Entity<AppState>,
    cx: TestAppContext,
}

impl Harness {
    fn new(home: &Path, project: &Path, peer: &Peer, initial: &Status, seed: bool) -> Self {
        let mut cx = TestAppContext::single();
        cx.background_executor.allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init();
            crate::theme::sync_component_theme(cx);
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.runtime_home = Some(home.into());
            state.project_path = Some(project.into());
            state.daemon_state =
                crate::daemon_link::ConnectionState::Connected(peer.facade.clone());
            state.runs = [
                (initial.planning_run, RunStatus::Completed),
                (initial.implementation_run, RunStatus::Failed),
            ]
            .into_iter()
            .map(|(run_id, status)| UiRun {
                run_id,
                status,
                started_at: chrono::Utc::now(),
                ended_at: Some(chrono::Utc::now()),
                last_event_seq: None,
            })
            .collect();
            state
        });
        let app = cx.new(|cx| {
            SurgeApp::new_shell_at(
                state.clone(),
                Some(home.join("desktop/continuation-index.json")),
                cx,
            )
        });
        if seed {
            app.update(&mut cx, |app, cx| {
                app.update_bootstrap_view(initial.operation_id, initial.clone(), project, cx)
            });
        }
        Self { app, state, cx }
    }

    fn results(&mut self, source: RunId) -> Entity<RunsScreen> {
        self.app.update(&mut self.cx, |app, cx| {
            app.open_run_cockpit(Some(source), cx);
            let _ = app.render_screen_content(cx);
            app.runs_screen.clone().unwrap()
        })
    }

    fn owned(&self, operation: RunId) -> Option<Status> {
        self.cx.read(|cx| {
            self.app
                .read(cx)
                .bootstrap_operations
                .get(&operation)
                .map(|(_, status)| status.clone())
        })
    }

    fn next_poll(&mut self, peer: &Peer, count: usize) -> bool {
        self.cx.run_until_parked();
        self.cx
            .background_executor
            .advance_clock(Duration::from_secs(3));
        drive(&mut self.cx, |_| peer.polls.load(Ordering::SeqCst) >= count)
    }
}

fn drive(cx: &mut TestAppContext, ready: impl Fn(&TestAppContext) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if ready(cx) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::yield_now();
    }
}

#[test]
fn failed_continue_actual_app_owns_progress_until_terminal_and_recovers_cold_index() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let initial = failed_status();
    let operation = initial.operation_id;
    let peer = Peer::new(&runtime, root.path(), initial.clone(), false);
    let mut h = Harness::new(root.path(), &project, &peer, &initial, true);
    let screen = h.results(initial.implementation_run);
    let (_, window) = h
        .cx
        .add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let button = window
        .debug_bounds("continue-bootstrap")
        .expect("actual Results action");
    window.simulate_click(button.center(), Modifiers::default());
    let _ = window;
    let state = h.state.clone();
    let response_received = drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            state
                .read(cx)
                .bootstrap_operations
                .get(&operation)
                .is_some_and(|status| !status.continuations.is_empty())
        })
    });
    let reserved_owned = h.owned(operation).is_some_and(|status| {
        status.revision > initial.revision
            && status
                .continuations
                .first()
                .is_some_and(|attempt| attempt.state == AttemptState::Reserved)
    });
    let mut running_owned = false;
    let mut equal_revision_refused = false;
    let mut terminal_owned = false;
    let mut cold_owned = false;
    if response_received
        && reserved_owned
        && drive(&mut h.cx, |_| peer.polls.load(Ordering::SeqCst) >= 1)
    {
        peer.publish(AttemptState::Running, true);
        let app = h.app.clone();
        running_owned = drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                app.read(cx).bootstrap_operations[&operation]
                    .1
                    .continuations[0]
                    .state
                    == AttemptState::Running
            })
        });
        if running_owned && h.next_poll(&peer, 2) {
            peer.publish(AttemptState::NeedsAttention, false);
            let app = h.app.clone();
            let delivered = drive(&mut h.cx, |cx| {
                cx.read(|cx| {
                    let app = app.read(cx);
                    app.operation_polls
                        .get(&operation)
                        .is_some_and(|(_, current)| {
                            app.operation_poll_settlements.get(&operation).is_some_and(
                                |(owner, completed)| owner == current && *completed >= 2,
                            )
                        })
                })
            });
            h.cx.run_until_parked();
            equal_revision_refused = delivered
                && h.owned(operation).unwrap().continuations[0].state == AttemptState::Running;
            if h.next_poll(&peer, 3) {
                peer.publish(AttemptState::Completed, true);
                let app = h.app.clone();
                terminal_owned = drive(&mut h.cx, |cx| {
                    cx.read(|cx| {
                        let app = app.read(cx);
                        app.bootstrap_operations[&operation].1.continuations[0].state
                            == AttemptState::Completed
                            && !app.operation_polls.contains_key(&operation)
                    })
                });
            }
        }
    }
    let final_status = h.owned(operation);
    drop(screen);
    drop(state);
    drop(h);
    if terminal_owned {
        let mut cold = Harness::new(root.path(), &project, &peer, &initial, false);
        cold.app.update(&mut cold.cx, |app, cx| {
            app.recover_bootstrap_operations(peer.facade.clone(), cx);
        });
        peer.responses.add_permits(1);
        let app = cold.app.clone();
        cold_owned = drive(&mut cold.cx, |cx| {
            cx.read(|cx| {
                app.read(cx)
                    .bootstrap_operations
                    .get(&operation)
                    .is_some_and(|(_, status)| Some(status) == final_status.as_ref())
            })
        });
        drop(app);
        drop(cold);
    }
    let calls = peer.continue_seen.load(Ordering::SeqCst);
    let phases = format!(
        "polls={} status_sent={} reserved={reserved_owned} running={running_owned} equal_refused={equal_revision_refused} terminal={terminal_owned} final={:?}",
        peer.polls.load(Ordering::SeqCst),
        peer.status_sent.load(Ordering::SeqCst),
        final_status
            .as_ref()
            .and_then(|status| status.continuations.first())
            .map(|attempt| attempt.state)
    );
    peer.close(&runtime);
    assert!(response_received, "native Continue response reached screen");
    assert!(
        reserved_owned,
        "Continue must reach App owner, not only AppState"
    );
    assert!(
        running_owned,
        "original Failed cannot stop successor polling"
    );
    assert!(
        equal_revision_refused,
        "equal-revision changed status must remain rejected"
    );
    assert!(
        terminal_owned,
        "successor terminal status must settle owner polling: {phases}"
    );
    assert!(
        cold_owned,
        "fresh App must recover same successor from saved index/native status"
    );
    assert_eq!(calls, 1);
    let final_status = final_status.unwrap();
    assert_eq!(final_status.state, BootstrapState::Failed);
    assert_eq!(final_status.result, initial.result);
    assert_eq!(final_status.implementation_run, initial.implementation_run);
}

#[test]
fn failed_continue_late_native_reply_cannot_cross_project_scope() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let initial = failed_status();
    let operation = initial.operation_id;
    let mut peer = Peer::new(&runtime, root.path(), initial.clone(), true);
    let mut h = Harness::new(root.path(), &project, &peer, &initial, true);
    let screen = h.results(initial.implementation_run);
    let (_, window) = h
        .cx
        .add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let button = window.debug_bounds("continue-bootstrap").unwrap();
    window.simulate_click(button.center(), Modifiers::default());
    let _ = window;
    let requested = drive(&mut h.cx, |_| {
        peer.continue_seen.load(Ordering::SeqCst) == 1
    });
    h.state.update(&mut h.cx, |state, _| {
        state.project_path = Some(root.path().join("other-project"));
        state.observation_scope();
    });
    peer.release.take().unwrap().send(()).unwrap();
    let sent = drive(&mut h.cx, |cx| {
        peer.sent.load(Ordering::SeqCst) >= 1
            && cx.read(|cx| !screen.read(cx).continuation_pending_for_test())
    });
    let app_unchanged = h.owned(operation).as_ref() == Some(&initial);
    let state_unchanged =
        h.cx.read(|cx| h.state.read(cx).bootstrap_operations.get(&operation) == Some(&initial));
    let no_poll = peer.polls.load(Ordering::SeqCst) == 0;
    drop(screen);
    drop(h);
    peer.close(&runtime);
    assert!(
        requested && sent,
        "actual gated native response was exercised"
    );
    assert!(
        app_unchanged && state_unchanged,
        "stale Continue cannot publish into new project"
    );
    assert!(
        no_poll,
        "stale Continue cannot install current-project polling"
    );
}
