//! Cold application observation of a real durable successor and actual subscription frames.
use super::{Harness, SeventhFixture, drive, start};
use gpui_kit::Modifiers;
use gpui_kit::test::TestWindowExt as _;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use surge_core::{
    EventPayload as E, RunId, VersionedEventPayload,
    bootstrap_attempt::{BootstrapAttempt, BootstrapAttemptState as A},
};
use surge_orchestrator::engine::{
    daemon_facade::DaemonEngineFacade,
    handle::EngineRunEvent,
    ipc::{DaemonEvent, DaemonRequest, DaemonResponse, read_request_frame, write_frame},
};

fn successor(runtime: &tokio::runtime::Runtime) -> (SeventhFixture, RunId) {
    successor_in_repository(runtime, false)
}
fn successor_in_repository(
    runtime: &tokio::runtime::Runtime,
    foreign: bool,
) -> (SeventhFixture, RunId) {
    successor_with_start(runtime, foreign, false)
}
fn successor_with_start(
    runtime: &tokio::runtime::Runtime,
    foreign: bool,
    late_start: bool,
) -> (SeventhFixture, RunId) {
    let graph: surge_core::Graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    successor_with_graph(runtime, foreign, late_start, graph, None)
}
fn successor_with_graph(
    runtime: &tokio::runtime::Runtime,
    foreign: bool,
    late_start: bool,
    graph: surge_core::Graph,
    repair: Option<surge_core::escalation::binding_repair::GeneratedBindingRepair>,
) -> (SeventhFixture, RunId) {
    let child_graph = repair
        .as_ref()
        .map_or_else(|| graph.clone(), |repair| repair.apply(&graph).unwrap());
    let mut fixture =
        SeventhFixture::with_operation_graph(runtime, true, false, true, Some(graph.clone()));
    let _source_repository = git2::Repository::init(&fixture.project).unwrap();
    let repository = if foreign {
        fixture.home.path().join("foreign-project")
    } else {
        fixture.project.clone()
    };
    let repo = git2::Repository::init(repository).unwrap();
    let tree = repo.index().unwrap().write_tree().unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
    let commit = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "base",
            &repo.find_tree(tree).unwrap(),
            &[],
        )
        .unwrap();
    let run = RunId::new();
    let path = fixture.home.path().join("worktrees").join(run.to_string());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let branch = repo
        .branch(
            &format!("successor-{run}"),
            &repo.find_commit(commit).unwrap(),
            false,
        )
        .unwrap();
    let mut options = git2::WorktreeAddOptions::new();
    options.reference(Some(branch.get()));
    repo.worktree(&run.to_string(), &path, Some(&options))
        .unwrap();
    let store = fixture.storage.bootstrap_operation_store();
    let attempt = BootstrapAttempt {
        continuation_id: RunId::new(),
        operation_id: fixture.record.status.operation_id,
        source_run: fixture.record.status.implementation_run,
        run,
        boundary: 2,
        runtime: surge_core::ContentHash::compute(b"host runtime"),
        binding_repair: repair.clone(),
        state: A::Reserved,
    };
    store
        .reserve_attempt(fixture.record.status.revision, &attempt)
        .unwrap();
    store
        .advance_attempt(attempt.continuation_id, A::Reserved, A::ForkReady)
        .unwrap();
    if repair.is_none() {
        store
            .advance_attempt(attempt.continuation_id, A::ForkReady, A::Running)
            .unwrap();
    }
    fixture.record = store.get(attempt.operation_id).unwrap().unwrap();
    runtime.block_on(async {
        let writer = fixture.storage.create_run(run, &path, None).await.unwrap();
        let mut payloads = vec![
            VersionedEventPayload::new(start(&path)),
            VersionedEventPayload::new(E::PipelineMaterialized {
                graph_hash: child_graph.identity().unwrap(),
                graph: Box::new(child_graph),
            }),
        ];
        if repair.is_some() {
            payloads.push(VersionedEventPayload::new(E::VerificationSubjectObserved {
                subject: None,
            }));
        }
        if late_start {
            payloads.swap(0, 1);
        }
        writer.append_events(payloads).await.unwrap();
        writer.close().await.unwrap();
    });
    drop(store);
    drop(branch);
    drop(repo);
    (fixture, run)
}

struct HistoryPeer {
    facade: Arc<DaemonEngineFacade>,
    seen: Arc<AtomicUsize>,
    events: tokio::sync::mpsc::UnboundedSender<EngineRunEvent>,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}
impl HistoryPeer {
    fn new(runtime: &tokio::runtime::Runtime, fixture: &SeventhFixture, run: RunId) -> Self {
        let path = fixture.home.path().join("history.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        let server = runtime.spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();let mut read = tokio::io::BufReader::new(read);
            let request = tokio::select! { _=&mut stopped=>return, frame=read_request_frame(&mut read)=>frame.unwrap().unwrap() };
            let DaemonRequest::Subscribe{request_id, run_id} = request else {panic!("actual Subscribe required")};
            assert_eq!(run_id, run);counter.fetch_add(1, Ordering::SeqCst);
            write_frame(&mut write, &DaemonResponse::SubscribeOk{request_id}).await.unwrap();
            loop {tokio::select! {
                _=&mut stopped=>return,
                event=received.recv()=> {let Some(event)=event else{return;};write_frame(&mut write, &DaemonEvent::PerRun{run_id:run,event:Box::new(event)}).await.unwrap();}
            }}
        });
        let facade = Arc::new(runtime.block_on(DaemonEngineFacade::connect(path)).unwrap());
        Self {
            facade,
            seen,
            events,
            stop,
            server,
        }
    }
    fn close(self, runtime: &tokio::runtime::Runtime) {
        let _ = self.stop.send(());
        runtime.block_on(self.server).unwrap();
        drop(self.facade);
    }
}
fn cold(fixture: &SeventhFixture) -> Harness {
    let mut h = fixture.harness();
    h.app.update(&mut h.cx, |app, cx| {
        app.update_bootstrap_view(
            fixture.record.status.operation_id,
            fixture.record.status.clone(),
            &fixture.project,
            cx,
        )
    });
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    h
}
#[test]
fn failed_continuation_history_cold_app_loads_log_and_retains_successor() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let (fixture, run) = successor(&runtime);
    let mut h = cold(&fixture);
    let loaded = h.cx.read(|cx| {
        let s = h.state.read(cx);
        s.run_prompt(&run) == Some("build timer")
            && s.runs
                .iter()
                .any(|r| r.run_id == run && r.last_event_seq == Some(2))
            && s.run_streams
                .get(&run)
                .is_some_and(|stream| stream.last_seq == 2)
    });
    h.app.update(&mut h.cx, |app, cx| {
        app.open_run_cockpit(Some(fixture.record.status.implementation_run), cx);
        let _ = app.render_screen_content(cx);
    });
    let screen = h.cx.read(|cx| h.app.read(cx).runs_screen.clone().unwrap());
    screen.update(&mut h.cx, |screen, _| {
        screen.continuation_history_home_for_test(fixture.home.path().into())
    });
    let (_, window) = h
        .cx
        .add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let button = window.debug_bounds("open-bootstrap-continuation").unwrap();
    window.simulate_click(button.center(), Modifiers::default());
    let _ = window;
    h.cx.run_until_parked();
    let selected = screen.update(&mut h.cx, |screen, cx| screen.visible_run_for_test(cx));
    h.state
        .update(&mut h.cx, |state, _| state.set_runs_from_summaries(&[]));
    h.app
        .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    let retained = h.cx.read(|cx| {
        h.state.read(cx).run_streams.contains_key(&run)
            && h.state.read(cx).runs.iter().any(|r| r.run_id == run)
    });
    drop(screen);
    drop(h);
    fixture.close();
    assert!(
        loaded,
        "cold accepted successor must actually hydrate prompt, registry watermark and Log journal"
    );
    assert_eq!(
        selected,
        Some(run),
        "actual rendered Successor selects a loaded row"
    );
    assert!(
        retained,
        "ListRuns omission must not prune accepted successor history"
    );
}

#[test]
fn failed_continuation_history_native_subscription_advances_actual_log_and_overview() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let (fixture, run) = successor(&runtime);
    let mut h = cold(&fixture);
    let peer = HistoryPeer::new(&runtime, &fixture, run);
    h.state.update(&mut h.cx, |state, _| {
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone());
    });
    h.app.update(&mut h.cx, |app, cx| {
        app.open_run_cockpit(Some(run), cx);
        let _ = app.render_screen_content(cx);
        app.sync_run_subscriptions(cx);
    });
    let screen = h.cx.read(|cx| h.app.read(cx).runs_screen.clone().unwrap());
    screen.update(&mut h.cx, |screen, _| {
        screen.continuation_history_home_for_test(fixture.home.path().into())
    });
    let (_, window) = h
        .cx
        .add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let mut rendered = window.clone();
    let _ = window;
    let attached = wait(&mut h.cx, |_| peer.seen.load(Ordering::SeqCst) == 1);
    let payload = E::StageEntered {
        node: "impl_1".parse().unwrap(),
        attempt: 1,
    };
    runtime.block_on(async {
        let writer = fixture.storage.open_run_writer(run).await.unwrap();
        writer
            .append_events(vec![VersionedEventPayload::new(payload.clone())])
            .await
            .unwrap();
        writer.close().await.unwrap();
    });
    peer.events
        .send(EngineRunEvent::Persisted {
            seq: 3,
            payload: Box::new(payload),
        })
        .unwrap();
    let advanced = wait(&mut h.cx, |cx| {
        cx.read(|cx| {
            let state = h.state.read(cx);
            state
                .run_streams
                .get(&run)
                .is_some_and(|stream| stream.last_seq == 3)
                && state
                    .runs
                    .iter()
                    .any(|row| row.run_id == run && row.last_event_seq == Some(2))
        })
    });
    let overview = wait(&mut h.cx, |cx| {
        cx.read(|cx| screen.read(cx).continuation_overview_through_for_test(cx)) == Some(3)
    });
    let phase = h.cx.read(|cx| {
        let state = h.state.read(cx);
        (
            state.run_streams.get(&run).map(|stream| stream.last_seq),
            state
                .runs
                .iter()
                .find(|row| row.run_id == run)
                .and_then(|row| row.last_event_seq),
        )
    });
    let (rendered_log, rendered_overview) = rendered_progress(&mut rendered);
    let unchanged = h.cx.read(|cx| {
        h.state.read(cx).bootstrap_operations[&fixture.record.status.operation_id].result
            == fixture.record.status.result
    });
    drop(rendered);
    drop(screen);
    drop(h);
    peer.close(&runtime);
    fixture.close();
    assert!(
        attached,
        "cold successor installs an actual native Subscribe request"
    );
    assert!(
        advanced,
        "actual committed successor event advances Log; durable read watermark stays2: stream/row={phase:?}, attached={attached}, overview={overview}"
    );
    assert!(
        overview,
        "actual rendered Overview reloads the successor journal through seq3"
    );
    assert!(
        rendered_log && rendered_overview,
        "actual rendered Log and Overview expose the new committed stage (log={rendered_log}, overview={rendered_overview})"
    );
    assert!(unchanged, "original Failed history is immutable");
}
fn rendered_progress(rendered: &mut gpui_kit::VisualTestContext) -> (bool, bool) {
    let rendered_log = rendered.update(|window, cx| {
        window.click("run-tab-Log", cx);
        window.render_frame(cx);
        window
            .try_find(("run-event", 0_usize))
            .and_then(|row| row.label().map(str::to_owned))
            .is_some_and(|label| label.contains("STAGE") && label.contains("impl_1"))
    });
    let rendered_overview = rendered.update(|window, cx| {
        window.click("run-tab-Overview", cx);
        window.render_frame(cx);
        window
            .try_find("mission-current-activity")
            .and_then(|item| item.label().map(str::to_owned))
            .is_some_and(|label| label.contains("Working on") && label.contains("Build"))
    });
    (rendered_log, rendered_overview)
}
fn wait(
    cx: &mut gpui_kit::TestAppContext,
    ready: impl Fn(&gpui_kit::TestAppContext) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if ready(cx) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::yield_now();
    }
}

#[test]
fn failed_continuation_history_foreign_repository_and_conflicting_association_are_refused() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let (fixture, run) = successor_in_repository(&runtime, true);
    let mut h = cold(&fixture);
    let refused = h.cx.read(|cx| {
        let state = h.state.read(cx);
        state.run_prompt(&run).is_none()
            && state
                .operation_inspection_reason(fixture.record.status.operation_id)
                .is_some()
    });
    let scope = crate::project_lifecycle::HistoryScope {
        generation: 1,
        root: fixture.project.clone(),
        repository: None,
        home: fixture.home.path().into(),
    };
    let loaded = runtime
        .block_on(crate::project_lifecycle::load_run(
            scope,
            run,
            crate::project_lifecycle::RunOwnership::Bootstrap {
                operation: fixture.record.status.operation_id,
                planning: false,
            },
            Arc::default(),
            None,
            Arc::default(),
        ))
        .unwrap();
    let unknown = matches!(
        loaded.ownership,
        crate::project_lifecycle::RunOwnership::Unknown
    );
    let mut conflicting = fixture.record.status.clone();
    conflicting.revision += 1;
    conflicting
        .continuations
        .push(conflicting.continuations[0].clone());
    h.app.update(&mut h.cx, |app, cx| {
        app.update_bootstrap_view(
            conflicting.operation_id,
            conflicting.clone(),
            &fixture.project,
            cx,
        )
    });
    let rejected = h.cx.read(|cx| {
        h.app.read(cx).bootstrap_operations[&conflicting.operation_id].1 == fixture.record.status
    });
    drop(h);
    fixture.close();
    assert!(
        refused && unknown,
        "same accepted graph and namespace cannot authorize a foreign physical repository"
    );
    assert!(
        rejected,
        "duplicate accepted associations are rejected rather than silently deduplicated"
    );
}

#[derive(Clone, Copy, Debug)]
enum IdentityFault {
    MissingRegistry,
    WrongRegistryPath,
    LateStartup,
}
fn identity_fault(
    runtime: &tokio::runtime::Runtime,
    fault: IdentityFault,
) -> (SeventhFixture, RunId) {
    let (fixture, run) =
        successor_with_start(runtime, false, matches!(fault, IdentityFault::LateStartup));
    if !matches!(fault, IdentityFault::LateStartup) {
        let pool = surge_persistence::runs::registry::open_registry_pool(
            fixture.home.path(),
            &surge_persistence::runs::MockClock::new(1),
        )
        .unwrap();
        {
            let connection = pool.get().unwrap();
            let changed = match fault {
                IdentityFault::MissingRegistry => connection
                    .execute("DELETE FROM runs WHERE id=?1", [run.to_string()])
                    .unwrap(),
                IdentityFault::WrongRegistryPath => connection
                    .execute(
                        "UPDATE runs SET project_path=?1 WHERE id=?2",
                        [
                            fixture.project.to_string_lossy().into_owned(),
                            run.to_string(),
                        ],
                    )
                    .unwrap(),
                IdentityFault::LateStartup => unreachable!(),
            };
            assert_eq!(changed, 1);
        }
        drop(pool);
    }
    (fixture, run)
}
fn public_identity_refusals(
    runtime: &tokio::runtime::Runtime,
    fixture: &SeventhFixture,
    run: RunId,
) -> (bool, bool, bool) {
    let mut h = cold(fixture);
    let app = h.cx.read(|cx| {
        let state = h.state.read(cx);
        state.run_prompt(&run).is_none()
            && state
                .operation_inspection_reason(fixture.record.status.operation_id)
                .is_some()
    });
    let scope = crate::project_lifecycle::HistoryScope {
        generation: 1,
        root: fixture.project.clone(),
        repository: None,
        home: fixture.home.path().into(),
    };
    let home = runtime
        .block_on(crate::project_lifecycle::load_run(
            scope,
            run,
            crate::project_lifecycle::RunOwnership::Bootstrap {
                operation: fixture.record.status.operation_id,
                planning: false,
            },
            Arc::default(),
            None,
            Arc::default(),
        ))
        .is_ok_and(|observed| {
            matches!(
                observed.ownership,
                crate::project_lifecycle::RunOwnership::Unknown
            )
        });
    // A real daemon's coarse RunAccepted projection can outlive a damaged registry row.
    // It supplies a selectable row, never confirmed journal/operation evidence.
    h.state.update(&mut h.cx, |state, _| {
        state.apply_global_event(
            &surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted { run_id: run },
        )
    });
    h.app.update(&mut h.cx, |app, cx| {
        app.open_run_cockpit(Some(run), cx);
        let _ = app.render_screen_content(cx);
    });
    let screen = h.cx.read(|cx| h.app.read(cx).runs_screen.clone().unwrap());
    screen.update(&mut h.cx, |screen, _| {
        screen.continuation_history_home_for_test(fixture.home.path().into())
    });
    let (_, window) = h
        .cx
        .add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let _ = window;
    let completed = wait(&mut h.cx, |cx| {
        cx.read(|cx| {
            screen
                .read(cx)
                .continuation_history_error_for_test(cx)
                .is_some()
                || screen
                    .read(cx)
                    .continuation_overview_through_for_test(cx)
                    .is_some()
        })
    });
    let results = completed
        && h.cx.read(|cx| {
            screen
                .read(cx)
                .continuation_history_error_for_test(cx)
                .is_some()
        });
    let old_failed = h.cx.read(|cx| {
        h.state.read(cx).bootstrap_operations[&fixture.record.status.operation_id].result
            == fixture.record.status.result
    });
    drop(screen);
    drop(h);
    assert!(
        old_failed,
        "read refusals cannot rewrite original Failed history"
    );
    (app, home, results)
}
#[test]
fn failed_continuation_identity_missing_registry_public_readers_refuse() {
    assert_identity_fault(IdentityFault::MissingRegistry);
}
#[test]
fn failed_continuation_identity_wrong_registry_path_public_readers_refuse() {
    assert_identity_fault(IdentityFault::WrongRegistryPath);
}
#[test]
fn failed_continuation_identity_late_startup_public_readers_refuse() {
    assert_identity_fault(IdentityFault::LateStartup);
}
fn assert_identity_fault(fault: IdentityFault) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let (fixture, run) = identity_fault(&runtime, fault);
    let observed = public_identity_refusals(&runtime, &fixture, run);
    fixture.close();
    assert_eq!(
        observed,
        (true, true, true),
        "actual App/Home/Results refusal for {fault:?}"
    );
}

#[test]
fn binding_repair_fork_ready_cold_app_home_and_identity_guards_agree() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let mut graph = surge_core::bundled_flows::BundledFlows::by_name_latest("multi-milestone")
        .unwrap()
        .graph;
    for body in graph.subgraphs.values_mut() {
        for node in body.nodes.values_mut() {
            if let surge_core::node::NodeConfig::Agent(agent) = &mut node.config {
                *agent = agent.with_runtime_override("codex-acp", Some("gpt-6.1-sol"));
            }
        }
    }
    let mut graph = surge_core::escalation::with_default_escalation_gates(&graph);
    for body in graph.subgraphs.values_mut() {
        for node in body.nodes.values_mut() {
            if surge_core::escalation::is_split_planner(node)
                && let surge_core::node::NodeConfig::Agent(agent) = &mut node.config
            {
                agent.custom_fields.remove("runtime");
            }
        }
    }
    let repair = surge_core::escalation::binding_repair::GeneratedBindingRepair::prepare(&graph)
        .unwrap()
        .unwrap();
    let (fixture, run) = successor_with_graph(&runtime, false, false, graph, Some(repair));
    assert_eq!(fixture.record.status.continuations[0].state, A::ForkReady);
    let rows = runtime
        .block_on(
            surge_persistence::runs::Storage::inspect_existing_run_events(
                fixture.home.path().join("runs"),
                run,
            ),
        )
        .unwrap();
    crate::app_state::validate_bootstrap_history(
        &fixture.record,
        run,
        &rows,
        Some(fixture.home.path()),
    )
    .unwrap();
    let events = rows
        .iter()
        .map(|row| row.to_run_event(run).unwrap())
        .collect::<Vec<_>>();
    runtime
        .block_on(crate::app_state::validate_bootstrap_successor(
            &fixture.record,
            fixture.home.path(),
            run,
            &events,
        ))
        .unwrap();
    let h = cold(&fixture);
    assert_eq!(
        h.cx.read(|cx| h.state.read(cx).run_prompt(&run).map(str::to_owned)),
        Some("build timer".into())
    );
    drop(h);
    let scope = crate::project_lifecycle::HistoryScope {
        generation: 1,
        root: fixture.project.clone(),
        repository: None,
        home: fixture.home.path().into(),
    };
    let observed = runtime
        .block_on(crate::project_lifecycle::load_run(
            scope,
            run,
            crate::project_lifecycle::RunOwnership::Bootstrap {
                operation: fixture.record.status.operation_id,
                planning: false,
            },
            Arc::default(),
            None,
            Arc::default(),
        ))
        .unwrap();
    assert!(matches!(
        observed.ownership,
        crate::project_lifecycle::RunOwnership::Bootstrap { .. }
    ));
    let mut changed = rows.clone();
    let mut tampered_record = fixture.record.clone();
    for row in &mut changed {
        if let E::PipelineMaterialized { graph, graph_hash } = &mut row.payload.payload {
            graph.metadata.name.push_str(" arbitrary drift");
            *graph_hash = graph.identity().unwrap();
            let mut encoded = serde_json::to_value(
                tampered_record.status.continuations[0]
                    .binding_repair
                    .as_ref()
                    .unwrap(),
            )
            .unwrap();
            encoded["repaired"] = serde_json::to_value(*graph_hash).unwrap();
            tampered_record.status.continuations[0].binding_repair =
                Some(serde_json::from_value(encoded).unwrap());
        }
    }
    assert!(
        crate::app_state::validate_bootstrap_history(
            &tampered_record,
            run,
            &changed,
            Some(fixture.home.path())
        )
        .is_err()
    );
    let changed_events = changed
        .iter()
        .map(|row| row.to_run_event(run).unwrap())
        .collect::<Vec<_>>();
    assert!(
        runtime
            .block_on(crate::app_state::validate_bootstrap_successor(
                &tampered_record,
                fixture.home.path(),
                run,
                &changed_events
            ))
            .is_err()
    );
    fixture.close();
}
