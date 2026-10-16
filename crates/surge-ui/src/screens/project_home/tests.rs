use super::{HomeAction, ProjectHomeScreen, lifecycle};
use crate::app_state::AppState;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, WindowOptions};
#[cfg(unix)]
use lifecycle::HomeProjection;
use lifecycle::RunOwnership;
use std::{cell::RefCell, rc::Rc};
use surge_core::RunId;
#[gpui_kit::test]
fn recorded_current_work_and_unknown_history_open_their_exact_run_owner(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let run = RunId::new();
    let unknown = RunId::new();
    let selected = Rc::new(RefCell::new(Vec::new()));
    let state = cx.update(|cx| cx.new(|_| AppState::new()));
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| ProjectHomeScreen::new(state, cx))
        })
        .unwrap()
    });
    let copy = selected.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, action: &HomeAction, _| {
            copy.borrow_mut().push(action.clone())
        })
        .detach();
        view.update(cx, |view, _| {
            view.model.current.push(lifecycle::CurrentWork {
                run,
                entered: 17,
                title: "Implement accepted scope".into(),
                profile: Some("implementer".into()),
                runtime: Some("Sol".into()),
                session: Some(surge_core::SessionId::new()),
            });
            view.model
                .unknown
                .push((unknown, "Journal unavailable; ownership unconfirmed".into()));
        });
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("home-add-idea").label(),
            Some("Plan application")
        );
        assert_eq!(
            window.find(format!("home-current-{run}")).label(),
            Some("Inspect this run")
        );
        window.click(format!("home-current-{run}"), cx);
        assert_eq!(
            window.find(format!("home-unknown-{unknown}")).label(),
            Some(format!("Inspect unassigned run {unknown}").as_str())
        );
        window.click(format!("home-unknown-{unknown}"), cx);
    })
    .unwrap();
    assert_eq!(
        *selected.borrow(),
        vec![HomeAction::OpenRun(run), HomeAction::OpenRun(unknown)]
    );
}
#[cfg(unix)]
#[gpui_kit::test]
fn expanding_ninth_task_loads_its_exact_accepted_scope_and_all_initial_history(
    cx: &mut TestAppContext,
) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use surge_core::work_item::*;
    use surge_orchestrator::engine::ipc::{
        DaemonRequest, DaemonResponse, read_request_frame, write_frame,
    };
    let home = tempfile::tempdir().unwrap();
    git2::Repository::init(home.path()).unwrap();
    let project = crate::project::ProjectScope::resolve(home.path());
    let repository = project.repository().unwrap().to_path_buf();
    let root = project.root().to_path_buf();
    let requirements = WorkItemRequirements::new(
        "Ninth exact accepted request".into(),
        vec!["Keep both attempts".into()],
    )
    .unwrap();
    let origin = AcceptedWorkItemOrigin::Requirements(requirements);
    let revision = WorkItemRevision {
        revision: 7,
        hash: origin.hash().unwrap(),
        origin,
        actor: "human".into(),
        accepted_proposal: None,
        accepted_at_ms: 1,
    };
    let mut items = Vec::new();
    for index in 0..9 {
        items.push(WorkItemRecord {
            id: surge_core::id::WorkItemId::new(),
            project: surge_core::id::WorkItemProjectId::new(),
            title: format!("Task {}", index + 1),
            accepted_revision: 7,
            version: 9,
            archived_at_ms: None,
            active_run: Some(RunId::new()),
            generation: 2,
            workspace: WorkItemWorkspace {
                repository: repository.clone(),
                checkout: home.path().into(),
                path: home.path().join(format!("task-{index}")),
                ownership: "fixture".into(),
                branch: format!("task/{index}"),
                base_commit: "a".repeat(40),
            },
        });
    }
    let ninth = items[8].clone();
    let detail = WorkItemDetail {
        item: ninth.clone(),
        revision: revision.clone(),
        pr: None,
        usage: WorkItemUsage::default(),
        control: None,
    };
    let older = RunId::new();
    let graph: surge_core::graph::Graph =
        toml::from_str(include_str!("../../../testdata/nested_loops_flow.toml")).unwrap();
    let attempts = [older, ninth.active_run.unwrap()]
        .into_iter()
        .enumerate()
        .map(|(index, run)| WorkItemAttempt {
            item: ninth.id,
            run,
            ordinal: index as u64 + 1,
            binding: WorkItemBinding {
                item: ninth.id,
                revision: 7,
                requirements_hash: revision.hash,
                generation: index as u64 + 1,
            },
            accepted_revision_relation: AcceptedRevisionRelation::MatchesCurrent,
            graph: Box::new(graph.clone()),
            config: "{}".into(),
            state: if index == 0 {
                WorkItemAttemptState::Completed
            } else {
                WorkItemAttemptState::Launched
            },
            diagnostic: None,
            usage_seq: 0,
            input_tokens: 0,
            output_tokens: 0,
            known_cost_usd: 0.0,
            usage_unknown: true,
        })
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let socket = home.path().join("ninth.sock");
    let (facade, server) = runtime.block_on(async {
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let count = requests.clone();
        let served = detail.clone();
        let attempts = attempts.clone();
        let revision = revision.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(read);
            while let Some(DaemonRequest::WorkItem {
                request_id,
                command,
            }) = read_request_frame(&mut reader).await.unwrap()
            {
                let result = match *command {
                    WorkItemCommand::Show { item } => {
                        assert_eq!(item, served.item.id);
                        WorkItemResult::Detail(Box::new(served.clone()))
                    },
                    WorkItemCommand::Attempts {
                        item, after: None, ..
                    } => {
                        assert_eq!(item, served.item.id);
                        WorkItemResult::Attempts(WorkItemPage {
                            entries: attempts.clone(),
                            next_cursor: None,
                        })
                    },
                    WorkItemCommand::Discussion {
                        item, after: None, ..
                    } => {
                        assert_eq!(item, served.item.id);
                        WorkItemResult::Discussion(WorkItemPage {
                            entries: vec![WorkItemDiscussion {
                                sequence: 1,
                                actor: "human".into(),
                                body: "Ninth discussion".into(),
                                proposal: None,
                                created_at_ms: 1,
                            }],
                            next_cursor: None,
                        })
                    },
                    WorkItemCommand::Revisions {
                        item, after: None, ..
                    } => {
                        assert_eq!(item, served.item.id);
                        WorkItemResult::Revisions(WorkItemPage {
                            entries: vec![revision.clone()],
                            next_cursor: None,
                        })
                    },
                    other => panic!("unexpected request {other:?}"),
                };
                count.fetch_add(1, Ordering::SeqCst);
                write_frame(
                    &mut write,
                    &DaemonResponse::WorkItemOk {
                        request_id,
                        result: Box::new(result),
                    },
                )
                .await
                .unwrap();
            }
        });
        let facade = surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket)
            .await
            .unwrap();
        (Arc::new(facade), server)
    });
    let _entered = runtime.enter();
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(home.path().into());
        state.project_scope = Some(project);
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone());
        let scope = state.tasks.begin(repository.clone());
        state.tasks.apply_page(
            &scope,
            WorkItemPage {
                entries: items.clone(),
                next_cursor: None,
            },
            false,
        );
        state.tasks.refresh_identity =
            Some((Arc::as_ptr(&facade) as usize, Some(home.path().into())));
        for item in items.iter().take(8) {
            state.tasks.details.insert(
                item.id,
                Arc::new(WorkItemDetail {
                    item: item.clone(),
                    ..detail.clone()
                }),
            );
        }
        state
            .project_history
            .begin(root, Some(repository), home.path().into(), false);
        state.project_history.loading = false;
        state
    });
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| ProjectHomeScreen::new(state.clone(), cx))
        })
        .unwrap()
    });
    view.update(cx, |view, cx| {
        view.model = HomeProjection::compile(&state.read(cx).tasks, &state.read(cx).project_history)
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "home-add-idea",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.0),
                gpui_kit::px(-10000.0),
            )),
            cx,
        );
        window.render_frame(cx);
        window.click(
            format!(
                "home-operation-{:?}",
                lifecycle::OperationKey::Task {
                    project: ninth.project,
                    item: ninth.id
                }
            ),
            cx,
        );
    })
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline
        && !cx.update(|cx| state.read(cx).tasks.details.contains_key(&ninth.id))
    {
        cx.run_until_parked();
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(5)));
    }
    let cached = cx.update(|cx| state.read(cx).tasks.clone());
    runtime.block_on(async {
        server.abort();
        let _ = server.await;
    });
    assert_eq!(
        requests.load(Ordering::SeqCst),
        4,
        "expansion must fetch the ninth item through the common owner"
    );
    assert_eq!(cached.details[&ninth.id].revision.hash, revision.hash);
    assert_eq!(
        cached.histories[&ninth.id]
            .attempts
            .entries
            .iter()
            .map(|attempt| attempt.run)
            .collect::<Vec<_>>(),
        vec![older, ninth.active_run.unwrap()]
    );
    assert_eq!(
        cached.histories[&ninth.id].discussion.entries[0].body,
        "Ninth discussion"
    );
    let model = cx.update(|cx| HomeProjection::compile(&cached, &state.read(cx).project_history));
    assert!(
        matches!(model.owners[&ninth.active_run.unwrap()],RunOwnership::Modern{ref binding,..} if binding.item==ninth.id && binding.revision==7 && binding.generation==2)
    );
}
#[gpui_kit::test]
fn recorded_stage_and_session_have_exact_navigation_controls(cx: &mut TestAppContext) {
    use std::sync::Arc;
    cx.update(gpui_kit::init);
    let run = RunId::new();
    let session = surge_core::SessionId::new();
    let events = vec![
        surge_core::RunEvent {
            run_id: run,
            seq: 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload: surge_core::EventPayload::StageEntered {
                node: "implement".parse().unwrap(),
                attempt: 1,
            },
        },
        surge_core::RunEvent {
            run_id: run,
            seq: 2,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload: surge_core::EventPayload::SessionOpened {
                node: "implement".parse().unwrap(),
                session,
                agent: "implementer".into(),
                agent_id: Some("Sol".into()),
                opened: None,
                handoff: None,
            },
        },
    ];
    let execution = surge_core::task_execution::TaskExecutionView::compile(run, &events).unwrap();
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_history.runs.insert(
            run,
            Arc::new(lifecycle::ProjectRun {
                run,
                ownership: RunOwnership::RecordedProject,
                requested: RunOwnership::RecordedProject,
                source: lifecycle::SourceObservation {
                    availability: lifecycle::Availability::Available,
                    freshness: lifecycle::Freshness::Observed,
                    coverage: lifecycle::Coverage::Complete,
                    through: Some(2),
                },
                events: Arc::new(events),
                execution: Some(Arc::new(execution)),
                mission: None,
                decisions: Arc::default(),
            }),
        );
        state
    });
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| ProjectHomeScreen::new(state, cx))
        })
        .unwrap()
    });
    view.update(cx, |screen, _| {
        screen.model.operations.push(lifecycle::HomeOperation {
            key: lifecycle::OperationKey::Legacy(run),
            title: "Recorded owner".into(),
            asked: None,
            accepted: None,
            revisions: Vec::new(),
            runs: vec![run],
            status: "Recorded".into(),
            history_more: false,
        });
        screen.expanded.insert(lifecycle::OperationKey::Legacy(run));
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(format!("home-stage-{run}-1")).label(),
            Some("Inspect recorded stage")
        );
        assert_eq!(
            window.find(format!("home-session-{run}-2")).label(),
            Some("Inspect recorded session")
        );
    })
    .unwrap();
}
#[gpui_kit::test]
fn absent_project_files_stay_out_of_primary_home_but_unavailable_input_remains_visible(
    cx: &mut TestAppContext,
) {
    use std::sync::Arc;
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_history.planning_inputs = Arc::new(vec![
            lifecycle::PlanningInput {
                path: "/fixture/absent/roadmap.toml".into(),
                source: lifecycle::SourceObservation {
                    availability: lifecycle::Availability::Absent,
                    freshness: lifecycle::Freshness::Observed,
                    coverage: lifecycle::Coverage::Complete,
                    through: None,
                },
                roadmap: None,
            },
            lifecycle::PlanningInput {
                path: "/fixture/corrupt/roadmap.toml".into(),
                source: lifecycle::SourceObservation {
                    availability: lifecycle::Availability::Unavailable(
                        "Invalid authored plan".into(),
                    ),
                    freshness: lifecycle::Freshness::Observed,
                    coverage: lifecycle::Coverage::Unknown,
                    through: None,
                },
                roadmap: None,
            },
        ]);
        state
    });
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| ProjectHomeScreen::new(state, cx))
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("home-planning-input-0").is_none());
        assert!(window.try_find("home-planning-input-1").is_some());
        assert!(window.try_find("home-history-provenance").is_none());
        window.click("home-history-details", cx);
        window.render_frame(cx);
        assert!(window.try_find("home-history-provenance").is_some());
        assert_eq!(
            window.find("home-add-idea").label(),
            Some("Plan application")
        );
        assert_eq!(window.find("home-create-task").label(), Some("Create task"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn confirmed_decision_opens_exact_request_and_uncertainty_never_says_none_pending(
    cx: &mut TestAppContext,
) {
    use surge_orchestrator::operator::{DecisionId, DecisionKind, PendingDecision};
    cx.update(gpui_kit::init);
    let run = RunId::new();
    let selected = Rc::new(RefCell::new(Vec::new()));
    let state = cx.new(|_| AppState::new());
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|cx| ProjectHomeScreen::new(state, cx))
        })
        .unwrap()
    });
    let copy = selected.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, action: &HomeAction, _| {
            copy.borrow_mut().push(action.clone())
        })
        .detach();
        view.update(cx, |view, _| {
            view.model.operations.push(lifecycle::HomeOperation {
                key: lifecycle::OperationKey::Legacy(run),
                title: "Recorded request".into(),
                asked: None,
                accepted: None,
                revisions: Vec::new(),
                runs: vec![run],
                status: "Recorded workflow".into(),
                history_more: false,
            });
            view.model.decisions.push(PendingDecision {
                id: DecisionId {
                    run,
                    request_seq: 47,
                },
                requested_at: chrono::DateTime::UNIX_EPOCH,
                node: Some("review".try_into().unwrap()),
                kind: DecisionKind::Gate {
                    call_id: "exact-request".into(),
                },
                prompt: "Approve accepted scope?".into(),
                options: Vec::new(),
                deadline: None,
                actionable: true,
            });
        });
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(format!("home-decision-{run}-47"), cx);
        view.update(cx, |view, _| { view.model.decisions.clear(); view.model.decisions_uncertain=true; });
        window.render_frame(cx);
        assert_eq!(window.find("home-decision-status").label(), Some("Pending decisions are unconfirmed. Some history is incomplete or unavailable; inspect a recorded run for earlier requests."));
        assert!(window.try_find(format!("home-decision-{run}-47")).is_none());
    }).unwrap();
    assert_eq!(
        *selected.borrow(),
        vec![HomeAction::OpenDecision {
            run,
            request_seq: 47
        }]
    );
}
