use super::*;
use crate::project_lifecycle::{
    Availability, Coverage, Freshness, ProjectRun, RunOwnership, SourceObservation,
};
use crate::screens::{project_home::HomeAction, runs::RecordedTarget};
use std::sync::Arc;
fn recorded_events(run: RunId) -> (Vec<surge_core::RunEvent>, surge_core::SessionId) {
    let node = surge_core::NodeKey::try_from("implement").unwrap();
    let first = surge_core::SessionId::new();
    let mut events = vec![
        surge_core::RunEvent {
            run_id: run,
            seq: 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload: surge_core::EventPayload::RunStarted {
                pipeline_template: None,
                project_path: PROJECT.into(),
                initial_prompt: "Exact recorded navigation".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
        },
        surge_core::RunEvent {
            run_id: run,
            seq: 2,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload: surge_core::EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        },
    ];
    for index in 0..45 {
        events.push(surge_core::RunEvent {
            run_id: run,
            seq: index + 3,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload: surge_core::EventPayload::SessionOpened {
                node: node.clone(),
                session: if index == 0 {
                    first
                } else {
                    surge_core::SessionId::new()
                },
                agent: "implementer".into(),
                agent_id: Some("Sol".into()),
                opened: None,
                handoff: None,
            },
        });
    }
    (events, first)
}
fn recorded_stream(
    run: RunId,
    events: &[surge_core::RunEvent],
) -> crate::run_stream::RunStreamState {
    let mut stream = crate::run_stream::RunStreamState::default();
    stream.begin_display_history(run, None);
    for event in events {
        stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: event.seq,
                payload: Box::new(event.payload.clone()),
            },
            0,
        );
    }
    stream.finish_display_history(events.last().unwrap().seq);
    stream
}
fn state_with_history(run: RunId, events: &[surge_core::RunEvent]) -> AppState {
    let mut state = AppState::new();
    state.project_path = Some(PathBuf::from(PROJECT));
    state.runs.push(crate::app_state::UiRun {
        run_id: run,
        status: surge_orchestrator::engine::RunStatus::Completed,
        started_at: chrono::DateTime::UNIX_EPOCH,
        last_event_seq: Some(47),
        ended_at: None,
    });
    state.run_streams.insert(run, recorded_stream(run, events));
    let execution = surge_core::task_execution::TaskExecutionView::compile(run, events).unwrap();
    state.project_history.runs.insert(
        run,
        Arc::new(ProjectRun {
            run,
            ownership: RunOwnership::RecordedProject,
            requested: RunOwnership::RecordedProject,
            source: SourceObservation {
                availability: Availability::Available,
                freshness: Freshness::Observed,
                coverage: Coverage::Complete,
                through: Some(47),
            },
            events: Arc::new(events.to_vec()),
            execution: Some(Arc::new(execution)),
            mission: None,
            decisions: Arc::default(),
        }),
    );
    state
}
fn open_target(
    window: &mut gpui_kit::VisualTestContext,
    app: &Entity<SurgeApp>,
    run: RunId,
    target: RecordedTarget,
) {
    window.update(|_, cx| app.update(cx, |app, cx| app.navigate(Screen::Home, cx)));
    window.run_until_parked();
    window.update(|_, cx| {
        let home = app.read(cx).home.as_ref().unwrap().clone();
        home.update(cx, |_, cx| {
            cx.emit(HomeAction::OpenRecorded { run, target })
        });
    });
    window.run_until_parked();
}
#[gpui_kit::test]
fn home_typed_destinations_reach_exact_old_session_page_and_recorded_stage(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt;
    cx.update(gpui_kit::init);
    let run = RunId::new();
    let (events, first) = recorded_events(run);
    let state = cx.new(|_| state_with_history(run, &events));
    let app = cx.new(|cx| {
        let mut app = SurgeApp::new_shell(state, cx);
        app.mode = super::super::AppMode::Project {
            _path: PathBuf::from(PROJECT),
            _name: "fixture".into(),
        };
        app.navigate(Screen::Home, cx);
        app
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(app.clone(), window, cx));
    open_target(
        window,
        &app,
        run,
        RecordedTarget::Session {
            entered: 2,
            opened: 3,
            connection: first,
        },
    );
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(app.read(cx).active_screen, Screen::Runs);
        assert_eq!(
            window.find("focused-recorded-session").label(),
            Some(format!("Requested session opening #3: {first}").as_str())
        );
    });
    assert!(
        window.debug_bounds("session-opening-3").is_some(),
        "oldest opening must be on the selected page"
    );
    assert!(
        window.debug_bounds("session-opening-47").is_none(),
        "newest page cannot stand in for the exact requested opening"
    );
    open_target(window, &app, run, RecordedTarget::Stage { entered: 2 });
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("focused-recorded-stage").label(),
            Some("Requested stage entry #2")
        );
    });
}
#[gpui_kit::test]
fn exact_destinations_never_fall_back_when_missing_partial_or_foreign(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    cx.update(gpui_kit::init);
    let run = RunId::new();
    let (events, first) = recorded_events(run);
    let state = cx.new(|_| state_with_history(run, &events));
    let app = cx.new(|cx| {
        let mut app = SurgeApp::new_shell(state.clone(), cx);
        app.mode = super::super::AppMode::Project {
            _path: PathBuf::from(PROJECT),
            _name: "fixture".into(),
        };
        app.navigate(Screen::Home, cx);
        app
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(app.clone(), window, cx));
    for scenario in ["missing", "cold", "partial", "foreign"] {
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                *state = state_with_history(run, &events);
                match scenario {
                    "cold" => {
                        state.run_streams.remove(&run);
                    },
                    "partial" => {
                        Arc::make_mut(state.project_history.runs.get_mut(&run).unwrap())
                            .source
                            .coverage = Coverage::Partial;
                    },
                    "foreign" => {
                        let other = RunId::new();
                        let (foreign, _) = recorded_events(other);
                        Arc::make_mut(state.project_history.runs.get_mut(&run).unwrap())
                            .execution = Some(Arc::new(
                            surge_core::task_execution::TaskExecutionView::compile(other, &foreign)
                                .unwrap(),
                        ));
                    },
                    _ => {},
                }
            })
        });
        let opened = if scenario == "missing" { 999 } else { 3 };
        open_target(
            window,
            &app,
            run,
            RecordedTarget::Session {
                entered: 2,
                opened,
                connection: first,
            },
        );
        window.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("requested-run-evidence-unavailable").label(),
                Some("Requested evidence unavailable")
            );
        });
        assert!(
            window.debug_bounds("session-opening-47").is_none(),
            "{scenario}: unavailable exact target cannot show an unrelated newer session"
        );
    }
    open_target(window, &app, run, RecordedTarget::Stage { entered: 999 });
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("requested-run-evidence-unavailable").label(),
            Some("Requested evidence unavailable")
        );
    });
}
