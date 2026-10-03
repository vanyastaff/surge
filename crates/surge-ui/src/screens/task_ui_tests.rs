//! UI retries traverse the real daemon; the proxy only loses an acknowledgement.

use super::{AppState, FleetScreen};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use std::sync::Arc;
use surge_core::work_item::{WorkItemCommand, WorkItemRequirements, WorkItemResult};
use surge_orchestrator::engine::EngineFacade;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::ipc::{
    DaemonRequest, DaemonResponse, read_request_frame, read_response_frame, write_frame,
};

fn question_history() -> (
    surge_core::RunId,
    Vec<surge_core::run_event::EventPayload>,
    String,
) {
    use surge_core::run_event::EventPayload;
    let request = surge_core::id::GateRequestId::new().to_string();
    let node = surge_core::NodeKey::try_from("gate").unwrap();
    (
        surge_core::RunId::new(),
        vec![
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: "/question-fixture".into(),
                initial_prompt: "Answer original question".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
            EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
            EventPayload::HumanInputRequested {
                node,
                session: None,
                call_id: Some(request.clone()),
                prompt: "Original decision".into(),
                schema: Some(serde_json::json!({"properties":{"outcome":{"enum":["done"]}}})),
            },
        ],
        request,
    )
}

fn replay_question(
    stream: &mut crate::run_stream::RunStreamState,
    run: surge_core::RunId,
    events: &[surge_core::run_event::EventPayload],
) {
    stream.begin_display_history(run, None);
    for (index, payload) in events.iter().enumerate() {
        stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: u64::try_from(index).unwrap() + 1,
                payload: Box::new(payload.clone()),
            },
            0,
        );
    }
    stream.finish_display_history(u64::try_from(events.len()).unwrap());
}

#[test]
fn parked_owner_close_retains_decision_but_final_owner_close_retires_it() {
    use surge_orchestrator::engine::handle::{EngineRunEvent, RunOutcome};
    let (run, events, request) = question_history();
    let mut stream = crate::run_stream::RunStreamState::default();
    replay_question(&mut stream, run, &events);
    assert_eq!(stream.pending.len(), 1);
    stream.apply(&EngineRunEvent::Terminal {
        outcome: RunOutcome::Parked {
            wake_at: chrono::Utc::now(),
        },
    });
    assert_eq!(stream.pending.len(), 1);
    let crate::run_stream::DecisionKind::HumanInput { call_id, .. } = &stream.pending[0].kind
    else {
        panic!("human input");
    };
    assert_eq!(call_id.as_ref(), Some(&request));
    stream.apply(&EngineRunEvent::Terminal {
        outcome: RunOutcome::Completed {
            terminal: surge_core::NodeKey::try_from("done").unwrap(),
        },
    });
    assert!(stream.pending.is_empty());
}

#[test]
fn question_rehydration_preserves_identity_and_refuses_lower_or_gapped_prefix() {
    use surge_core::run_event::EventPayload;
    let (run, events, request) = question_history();
    let mut stream = crate::run_stream::RunStreamState::default();
    replay_question(&mut stream, run, &events);
    assert_eq!(stream.pending.len(), 1);
    stream.pending.clear();
    replay_question(&mut stream, run, &events);
    assert_eq!(stream.pending.len(), 1);
    assert_eq!(stream.pending[0].seq, 3);
    let crate::run_stream::DecisionKind::HumanInput { call_id, .. } = &stream.pending[0].kind
    else {
        panic!("human input");
    };
    assert_eq!(call_id.as_ref(), Some(&request));
    replay_question(&mut stream, run, &events);
    assert_eq!(
        stream.pending.len(),
        1,
        "duplicate hydration is one original decision"
    );
    stream.apply_recorded(
        &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
            seq: 4,
            payload: Box::new(EventPayload::HumanInputResolved {
                node: surge_core::NodeKey::try_from("gate").unwrap(),
                call_id: Some(request),
                response: serde_json::json!({"outcome":"done"}),
            }),
        },
        0,
    );
    assert!(stream.pending.is_empty());
    replay_question(&mut stream, run, &events);
    assert!(
        stream.pending.is_empty(),
        "lower prefix cannot revive a known resolved decision"
    );
    assert_eq!(
        stream.display(),
        surge_core::run_display::RunDisplayState::Unknown
    );
    assert!(!stream.session_history_confirmed());
    let mut gapped = crate::run_stream::RunStreamState::default();
    replay_question(&mut gapped, run, &events);
    gapped.apply_recorded(
        &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
            seq: 5,
            payload: Box::new(events[2].clone()),
        },
        0,
    );
    assert!(
        gapped.pending.is_empty(),
        "a gap cannot expose an actionable question"
    );
}

fn init_project(path: &std::path::Path) {
    let repo = git2::Repository::init(path).unwrap();
    std::fs::write(path.join("README.md"), "Retained task test\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("UI test", "ui-test@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .unwrap();
}

async fn start_daemon(
    home: &std::path::Path,
    project: &std::path::Path,
) -> (
    Arc<surge_persistence::runs::Storage>,
    std::path::PathBuf,
    tokio::task::JoinHandle<Result<(), surge_daemon::DaemonError>>,
) {
    use surge_orchestrator::engine::{Engine, EngineConfig};
    let storage = surge_persistence::runs::Storage::open(home).await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(
            surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                project.into(),
            ),
        ),
        EngineConfig::default(),
    ));
    let socket = home.join("ui-real.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        Arc::new(surge_orchestrator::engine::facade::LocalEngineFacade::new(
            engine.clone(),
        )),
        surge_daemon::tracked_run::TrackingContext::new(engine, storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        Default::default(),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !socket.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    (storage, socket, server)
}

async fn lost_reply_proxy(
    proxy: &std::path::Path,
    daemon: std::path::PathBuf,
    observed: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    accepted_reply: bool,
) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::UnixListener::bind(proxy).unwrap();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let daemon = daemon.clone();
            let observed = observed.clone();
            let dropped = dropped.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut reader = tokio::io::BufReader::new(read);
                while let Some(request) = read_request_frame(&mut reader).await.unwrap() {
                    let is_discussion = matches!(&request, DaemonRequest::WorkItem { command, .. } if matches!(command.as_ref(), WorkItemCommand::Discuss { .. }));
                    if let DaemonRequest::WorkItem { command, .. } = &request
                        && is_discussion
                    {
                        observed
                            .lock()
                            .unwrap()
                            .push(serde_json::to_value(command).unwrap());
                    }
                    let mut upstream = tokio::net::UnixStream::connect(&daemon).await.unwrap();
                    write_frame(&mut upstream, &request).await.unwrap();
                    let response = read_response_frame(&mut tokio::io::BufReader::new(upstream))
                        .await
                        .unwrap()
                        .unwrap();
                    if is_discussion && !dropped.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        assert!(
                            if accepted_reply {
                                matches!(response, DaemonResponse::WorkItemOk { .. })
                            } else {
                                matches!(response, DaemonResponse::Error { code: surge_orchestrator::engine::ipc::ErrorCode::WorkItemConflict, .. })
                            },
                            "lose only the expected actual daemon reply"
                        );
                        // Lose an actual daemon response after its authoritative classification.
                        break;
                    }
                    if write_frame(&mut write, &response).await.is_err() {
                        break;
                    }
                }
            });
        }
    })
}

fn drive_until(
    runtime: &tokio::runtime::Runtime,
    cx: &mut TestAppContext,
    condition: impl Fn(&mut TestAppContext) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !condition(cx) {
        assert!(
            std::time::Instant::now() < deadline,
            "UI operation did not settle"
        );
        cx.run_until_parked();
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(5)));
    }
}

fn discussion(
    storage: &surge_persistence::runs::Storage,
    item: surge_core::id::WorkItemId,
) -> surge_core::work_item::WorkItemPage<surge_core::work_item::WorkItemDiscussion> {
    let WorkItemResult::Discussion(page) = storage
        .work_items()
        .query(&WorkItemCommand::Discussion {
            item,
            after: None,
            limit: 100,
        })
        .unwrap()
    else {
        panic!("expected discussion");
    };
    page
}

#[gpui_kit::test]
fn lost_ack_ui_retry_preserves_command_and_one_real_discussion(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    init_project(project.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let host_runtime = tokio::runtime::Runtime::new().unwrap();
    let (storage, socket, server) =
        host_runtime.block_on(start_daemon(home.path(), project.path()));
    let _entered = runtime.enter();
    let direct = runtime
        .block_on(DaemonEngineFacade::connect(socket.clone()))
        .unwrap();
    let WorkItemResult::Detail(detail) = runtime
        .block_on(
            direct.work_item(WorkItemCommand::Create {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                project: project.path().into(),
                title: "Retry task".into(),
                requirements: WorkItemRequirements::new(
                    "Preserve requirements".into(),
                    vec!["One discussion".into()],
                )
                .unwrap(),
            }),
        )
        .unwrap()
    else {
        panic!("expected created task");
    };
    let item = detail.item.id;
    let proxy_path = home.path().join("ui-proxy.sock");
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let proxy = host_runtime.block_on(lost_reply_proxy(
        &proxy_path,
        socket,
        observed.clone(),
        true,
    ));
    let facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(proxy_path.clone()))
            .unwrap(),
    );
    let identity = Arc::as_ptr(&facade) as usize;
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(project.path().into());
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade);
        let scope = state.tasks.begin(detail.item.workspace.repository.clone());
        state.tasks.apply_page(
            &scope,
            surge_core::work_item::WorkItemPage {
                entries: vec![detail.item.clone()],
                next_cursor: None,
            },
            false,
        );
        state.tasks.details.insert(item, *detail);
        state
    });
    let view = cx.new(|cx| FleetScreen::new(state.clone(), cx));
    let command = WorkItemCommand::Discuss {
        operation_id: surge_core::id::WorkItemOperationId::new(),
        item,
        expected_version: 1,
        body: "  Exact discussion body.  ".into(),
        proposal: None,
    };
    let expected = serde_json::to_value(&command).unwrap();
    let original = command.clone();
    view.update(cx, |view, cx| {
        view.selected_item = Some(item);
        view.task_refresh_identity = Some((identity, Some(project.path().into())));
        view.submit_task(command, cx);
    });
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    assert!(cx.read(|cx| view.read(cx).task_submissions.contains_key(&item)));
    assert!(
        storage.work_items().replay(&original).unwrap().is_some(),
        "accepted operation must already have its durable receipt"
    );
    assert_eq!(discussion(&storage, item).entries.len(), 1);
    let facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(proxy_path))
            .unwrap(),
    );
    state.update(cx, |state, cx| {
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade);
        cx.notify();
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.selected_item = Some(item);
        view.retry_task(cx);
    });
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    assert!(cx.read(|cx| !view.read(cx).task_submissions.contains_key(&item)));
    let commands = observed.lock().unwrap();
    assert_eq!(commands.as_slice(), &[expected.clone(), expected]);
    let messages = discussion(&storage, item);
    assert_eq!(messages.entries.len(), 1);
    assert_eq!(messages.entries[0].body, "  Exact discussion body.  ");
    proxy.abort();
    server.abort();
}

#[gpui_kit::test]
fn definitive_version_conflict_keeps_task_draft_until_explicit_dismissal(cx: &mut TestAppContext) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    init_project(project.path());
    let host = tokio::runtime::Runtime::new().unwrap();
    let (storage, socket, server) = host.block_on(start_daemon(home.path(), project.path()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(socket.clone()))
            .unwrap(),
    );
    let WorkItemResult::Detail(detail) = runtime
        .block_on(
            facade.work_item(WorkItemCommand::Create {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                project: project.path().into(),
                title: "Conflicting draft".into(),
                requirements: WorkItemRequirements::new(
                    "Accepted original".into(),
                    vec!["Original criterion".into()],
                )
                .unwrap(),
            }),
        )
        .unwrap()
    else {
        panic!("detail");
    };
    let item = detail.item.id;
    cx.update(gpui_kit::init);
    let identity = Arc::as_ptr(&facade) as usize;
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(project.path().into());
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone());
        state.tasks.begin(detail.item.workspace.repository.clone());
        state.tasks.details.insert(item, (*detail).clone());
        state.tasks.records.push(detail.item.clone());
        state.tasks.fresh = true;
        state
    });
    let view = cx.new(|cx| FleetScreen::new(state, cx));
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| gpui_kit::component::Root::new(view.clone(), window, cx))
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.selected_item = Some(item);
            view.task_refresh_identity = Some((identity, Some(project.path().into())));
            view.ensure_task_draft(&detail, window, cx);
            view.task_drafts[&item].comment.update(cx, |input, cx| {
                input.set_value("Unsaved discussion draft", window, cx)
            });
            view.task_drafts[&item]
                .requirements
                .update(cx, |input, cx| {
                    input.set_value("Unsaved requirements", window, cx)
                });
        });
    })
    .unwrap();
    view.update(cx, |view, cx| {
        view.state.update(cx, |state, _| {
            state.tasks.details.remove(&item);
        });
        view.reload_item(item, cx);
    });
    drive_until(&runtime, cx, |cx| {
        cx.read(|cx| {
            let state = view.read(cx).state.read(cx);
            state.tasks.fresh
                && state
                    .tasks
                    .details
                    .get(&item)
                    .is_some_and(|detail| detail.item.version == 1)
        })
    });
    runtime
        .block_on(
            facade.work_item(WorkItemCommand::Edit {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item,
                expected_version: detail.item.version,
                expected_revision: detail.revision.revision,
                requirements: WorkItemRequirements::new(
                    "New accepted content".into(),
                    vec!["New criterion".into()],
                )
                .unwrap(),
            }),
        )
        .unwrap();
    view.update(cx, |view, cx| {
        assert!(view.task_detail_confirmed(item, cx), "original real hydrated view remains locally current before the unseen remote edit is observed");
        assert_eq!(view.state.read(cx).tasks.records[0].version, 1);
        view.submit_task_draft(item, false, false, cx);
    });
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    assert!(cx.read(|cx| view.read(cx).task_rejections.contains(&item)));
    assert!(cx.read(|cx| view.read(cx).task_submissions.contains_key(&item)));
    assert!(discussion(&storage, item).entries.is_empty());
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let proxy_path = home.path().join("rejected-reply.sock");
    let proxy = host.block_on(lost_reply_proxy(
        &proxy_path,
        socket.clone(),
        observed.clone(),
        false,
    ));
    let retry_facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(proxy_path))
            .unwrap(),
    );
    let pending = cx.read(|cx| view.read(cx).task_submissions[&item].retry());
    view.update(cx, |view, cx| {
        view.state.update(cx, |state, _| {
            state.daemon_state = crate::daemon_link::ConnectionState::Connected(retry_facade)
        });
        view.retry_task(cx);
    });
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    view.update(cx, |view, cx| {
        assert!(!view.task_rejections.contains(&item));
        assert_eq!(
            serde_json::to_value(view.task_submissions[&item].retry()).unwrap(),
            serde_json::to_value(&pending).unwrap()
        );
        view.dismiss_rejected_task_operation(cx);
        assert!(
            view.task_submissions.contains_key(&item),
            "uncertainty cannot authorize dismissal"
        );
        view.state.update(cx, |state, _| {
            state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade)
        });
        view.retry_task(cx);
    });
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    assert!(cx.read(|cx| view.read(cx).task_rejections.contains(&item)));
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        &[serde_json::to_value(&pending).unwrap()]
    );
    assert!(discussion(&storage, item).entries.is_empty());
    cx.update_window(handle, |_, _, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.task_drafts[&item].comment.read(cx).value().as_ref(),
                "Unsaved discussion draft"
            );
            assert_eq!(
                view.task_drafts[&item]
                    .requirements
                    .read(cx)
                    .value()
                    .as_ref(),
                "Unsaved requirements"
            );
            assert_eq!(view.task_drafts[&item].version, 1);
            view.state.update(cx, |state, _| {
                state.tasks.details.remove(&item);
            });
            view.dismiss_rejected_task_operation(cx);
            assert!(!view.task_submissions.contains_key(&item));
            assert_eq!(
                view.task_drafts[&item]
                    .requirements
                    .read(cx)
                    .value()
                    .as_ref(),
                "Unsaved requirements"
            );
            assert_eq!(view.task_drafts[&item].version, 1);
        })
    })
    .unwrap();
    drive_until(&runtime, cx, |cx| {
        cx.read(|cx| {
            view.read(cx)
                .state
                .read(cx)
                .tasks
                .details
                .get(&item)
                .is_some_and(|detail| detail.item.version == 2)
        })
    });
    println!(
        "semantic assertions completed: typed refusal, uncertain retry, immutable command, retained drafts"
    );
    let direct = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(socket))
            .unwrap(),
    );
    cx.update_window(handle, |_, _, cx| {
        view.update(cx, |view, cx| {
            view.state.update(cx, |state, _| {
                state.daemon_state = crate::daemon_link::ConnectionState::Connected(direct);
                state.tasks.fresh = true;
            });
            view.rebase_task_draft(item, cx);
            view.submit_task_draft(item, true, true, cx);
        });
    })
    .unwrap();
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.task_drafts[&item].revision, 3,
                "own accepted edit rebases unchanged draft"
            );
            assert_eq!(
                view.task_drafts[&item]
                    .requirements
                    .read(cx)
                    .value()
                    .as_ref(),
                "Unsaved requirements"
            );
            assert_eq!(
                view.task_drafts[&item].comment.read(cx).value().as_ref(),
                "Unsaved discussion draft"
            );
            assert!(view.task_feedback[&item].contains("revision 3"));
        });
        window.render_frame(cx);
    })
    .unwrap();
    drive_until(&runtime, cx, |cx| {
        cx.read(|cx| {
            let state = view.read(cx).state.read(cx);
            state.tasks.fresh
                && state
                    .tasks
                    .records
                    .iter()
                    .any(|record| record.id == item && record.version == 3)
                && state
                    .tasks
                    .details
                    .get(&item)
                    .is_some_and(|detail| detail.item.version == 3)
        })
    });
    view.update(cx, |view, cx| {
        view.submit_task_draft(item, false, false, cx)
    });
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    drive_until(&runtime, cx, |cx| {
        cx.read(|cx| {
            let state = view.read(cx).state.read(cx);
            state.tasks.fresh
                && state
                    .tasks
                    .records
                    .iter()
                    .any(|record| record.id == item && record.version == 4)
                && state
                    .tasks
                    .details
                    .get(&item)
                    .is_some_and(|detail| detail.item.version == 4)
        })
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.task_drafts[&item].comment.read(cx).value().as_ref(),
                "",
                "confirmed unchanged discussion clears in its owning window"
            );
            assert_eq!(
                view.task_drafts[&item]
                    .requirements
                    .read(cx)
                    .value()
                    .as_ref(),
                "Unsaved requirements"
            );
            assert_eq!(view.task_feedback[&item], "Discussion accepted");
            assert_eq!(
                view.task_drafts[&item].version, 4,
                "own discussion ACK advances its joined revision version"
            );
            view.rebase_task_draft(item, cx);
            view.task_drafts[&item].comment.update(cx, |input, cx| {
                input.set_value("Submitted discussion", window, cx)
            });
            view.submit_task_draft(item, false, false, cx);
            view.task_drafts[&item].comment.update(cx, |input, cx| {
                input.set_value("New discussion written during dispatch", window, cx)
            });
        });
    })
    .unwrap();
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(discussion(&storage, item).entries.len(), 2);
    drive_until(&runtime, cx, |cx| {
        cx.read(|cx| {
            let state = view.read(cx).state.read(cx);
            state.tasks.fresh
                && state
                    .tasks
                    .records
                    .iter()
                    .any(|record| record.id == item && record.version == 5)
                && state
                    .tasks
                    .details
                    .get(&item)
                    .is_some_and(|detail| detail.item.version == 5)
        })
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.task_drafts[&item].comment.read(cx).value().as_ref(),
                "New discussion written during dispatch"
            );
            view.submit_task_draft(item, true, true, cx);
            view.task_drafts[&item]
                .requirements
                .update(cx, |input, cx| {
                    input.set_value("Requirements edited during dispatch", window, cx)
                });
        });
    })
    .unwrap();
    drive_until(&runtime, cx, |cx| cx.read(|cx| !view.read(cx).task_busy));
    cx.update_window(handle, |_, _, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.task_drafts[&item]
                    .requirements
                    .read(cx)
                    .value()
                    .as_ref(),
                "Requirements edited during dispatch"
            );
            assert_eq!(
                view.task_drafts[&item].revision, 3,
                "changed in-flight requirements keep their original base"
            );
            assert!(view.task_feedback[&item].contains("revision 4"));
        });
    })
    .unwrap();
    let draft_handles = cx.read(|cx| {
        let draft = &view.read(cx).task_drafts[&item];
        [
            draft.comment.downgrade(),
            draft.requirements.downgrade(),
            draft.criteria.downgrade(),
            draft.flow.downgrade(),
        ]
    });
    let owner = view.downgrade();
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.state.update(cx, |state, _| {
                state.daemon_state = crate::daemon_link::ConnectionState::Disconnected;
            })
        });
        window.remove_window();
    })
    .unwrap();
    drop(view);
    // App::update flushes the dropped-entity queue; pumping its scheduler alone does not.
    cx.update(|_| {});
    assert!(
        owner.upgrade().is_none(),
        "task view owner must be released"
    );
    assert!(
        draft_handles.iter().all(|input| input.upgrade().is_none()),
        "all original draft inputs must be released with their owner"
    );
    cx.run_until_parked();
    proxy.abort();
    server.abort();
    host.block_on(async {
        let _ = proxy.await;
        let _ = server.await;
    });
}

#[gpui_kit::test]
fn disconnected_task_with_cached_working_stream_is_unconfirmed(cx: &mut TestAppContext) {
    use surge_core::id::WorkItemProjectId;
    use surge_core::work_item::{WorkItemRecord, WorkItemWorkspace};
    use surge_core::{ContentHash, RunId, run_event::EventPayload};
    use surge_orchestrator::engine::handle::EngineRunEvent;
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    init_project(project.path());
    let host = tokio::runtime::Runtime::new().unwrap();
    let (_, socket, server) = host.block_on(start_daemon(home.path(), project.path()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(socket))
            .unwrap(),
    );
    let run = RunId::new();
    let mut stream = crate::run_stream::RunStreamState::default();
    stream.begin_display_history(run, None);
    for (seq, payload) in [
        (
            1,
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: "/repo".into(),
                initial_prompt: "work".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: vec![],
                    bootstrap_edit_loop_cap: None,
                },
            },
        ),
        (
            2,
            EventPayload::PipelineMaterialized {
                graph: Box::new(
                    toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml"))
                        .unwrap(),
                ),
                graph_hash: ContentHash::compute(b"fixture"),
            },
        ),
    ] {
        stream.apply_recorded(
            &EngineRunEvent::Persisted {
                seq,
                payload: Box::new(payload),
            },
            0,
        );
    }
    assert_eq!(stream.display().label(), "Running");
    let record = WorkItemRecord {
        id: surge_core::id::WorkItemId::new(),
        project: WorkItemProjectId::new(),
        title: "Cached".into(),
        accepted_revision: 1,
        version: 1,
        archived_at_ms: None,
        active_run: Some(run),
        generation: 1,
        workspace: WorkItemWorkspace {
            repository: "/repo/.git".into(),
            checkout: "/repo".into(),
            path: "/repo/task".into(),
            ownership: "fixture".into(),
            branch: "task/fixture".into(),
            base_commit: "a".repeat(40),
        },
    };
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade);
        state.tasks.records.push(record.clone());
        state.tasks.fresh = true;
        state.run_streams.insert(run, stream);
        state.tasks.disconnected();
        state
    });
    let view = cx.new(|cx| FleetScreen::new(state.clone(), cx));
    view.update(cx, |view, cx| {
        let state = view.state.read(cx);
        assert!(!state.tasks.fresh);
        assert_eq!(state.tasks.records[0].id, record.id);
        assert_eq!(state.run_streams[&run].display().label(), "Running");
        assert_eq!(view.durable_status(&record, cx), "Run state is unconfirmed");
    });
    state.update(cx, |state, _| {
        state.tasks.fresh = true;
        state.run_streams.get_mut(&run).unwrap().live = false;
    });
    view.update(cx, |view, cx| {
        assert!(view.state.read(cx).tasks.fresh);
        assert!(view.state.read(cx).daemon_state.facade().is_some());
        assert_eq!(view.durable_status(&record, cx), "Run state is unconfirmed");
    });
    server.abort();
}

#[gpui_kit::test]
fn completed_attempt_superseded_by_stored_edit_needs_current_revision_execution(
    cx: &mut TestAppContext,
) {
    use surge_core::work_item::{AcceptedRevisionRelation, WorkItemAttemptState};
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    init_project(project.path());
    let host = tokio::runtime::Runtime::new().unwrap();
    let (storage, socket, server) = host.block_on(start_daemon(home.path(), project.path()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(socket))
            .unwrap(),
    );
    let WorkItemResult::Detail(created) = runtime
        .block_on(
            facade.work_item(WorkItemCommand::Create {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                project: project.path().into(),
                title: "Revision work".into(),
                requirements: WorkItemRequirements::new(
                    "Original goal".into(),
                    vec!["Original criterion".into()],
                )
                .unwrap(),
            }),
        )
        .unwrap()
    else {
        panic!("created detail");
    };
    let item = created.item.id;
    runtime
        .block_on(
            facade.work_item(
                crate::work_items::reviewed_start_command(
                    item,
                    created.item.version,
                    include_str!("../../../../examples/flow_terminal_only.toml"),
                )
                .unwrap(),
            ),
        )
        .unwrap();
    let complete = runtime
        .block_on(tokio::time::timeout(
            std::time::Duration::from_secs(5),
            async {
                loop {
                    let WorkItemResult::Detail(detail) = facade
                        .work_item(WorkItemCommand::Show { item })
                        .await
                        .unwrap()
                    else {
                        panic!("detail");
                    };
                    if detail.item.active_run.is_none() {
                        break detail;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            },
        ))
        .unwrap();
    runtime
        .block_on(
            facade.work_item(WorkItemCommand::Edit {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item,
                expected_version: complete.item.version,
                expected_revision: complete.item.accepted_revision,
                requirements: WorkItemRequirements::new(
                    "New accepted goal".into(),
                    vec!["New criterion".into()],
                )
                .unwrap(),
            }),
        )
        .unwrap();
    let (detail, history) = runtime
        .block_on(crate::work_items::load_task(&facade, item))
        .unwrap();
    assert_eq!(detail.item.accepted_revision, 2);
    assert_eq!(history.attempts.entries.len(), 1);
    assert_eq!(
        history.attempts.entries[0].state,
        WorkItemAttemptState::Completed
    );
    assert_eq!(
        history.attempts.entries[0].accepted_revision_relation,
        AcceptedRevisionRelation::Superseded
    );
    let run = history.attempts.entries[0].run;
    let inspection = host.block_on(storage.inspect_run(run)).unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("actual run journal");
    };
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade);
        state.tasks.records.push(detail.item.clone());
        state.tasks.details.insert(item, detail.clone());
        state.tasks.histories.insert(item, history);
        state.tasks.fresh = true;
        state
    });
    let view = cx.new(|cx| FleetScreen::new(state.clone(), cx));
    view.update(cx, |view, cx| {
        assert_eq!(
            view.durable_status(&detail.item, cx),
            "Needs execution for current revision"
        )
    });

    view.update(cx, |view, cx| {
        view.task_refresh_identity = Some((
            Arc::as_ptr(&view.state.read(cx).daemon_state.facade().unwrap()) as usize,
            None,
        ));
    });
    let (handle, root) = cx.update(|cx| {
        gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| gpui_kit::component::Root::new(view.clone(), window, cx))
        })
        .unwrap()
    });
    // The attempt page can be absent during reconnect. Hydrate ownership from
    // the actual completed daemon journal, not a guessed run-to-task identity.
    state.update(cx, |state, _| {
        state.tasks.histories.clear();
        state.runs.push(crate::app_state::UiRun {
            run_id: run,
            status: surge_orchestrator::engine::handle::RunStatus::Completed,
            started_at: chrono::Utc::now(),
            last_event_seq: None,
            ended_at: None,
        });
    });
    view.update(cx, |view, cx| {
        assert_eq!(
            view.tasks(cx).len(),
            1,
            "unhydrated legacy ownership remains unresolved"
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("filter-all").label(),
            Some("All  2"),
            "one durable task plus one unresolved recorded run"
        );
        assert_eq!(
            window.find(format!("ownership-unconfirmed-{run}")).label(),
            Some("Task ownership unconfirmed")
        );
    })
    .unwrap();
    state.update(cx, |state, _| {
        let scope = state.tasks.begin(detail.item.workspace.repository.clone());
        state.tasks.apply_page(
            &scope,
            surge_core::work_item::WorkItemPage {
                entries: vec![detail.item.clone()],
                next_cursor: None,
            },
            false,
        );
        let stream = state.run_streams.entry(run).or_default();
        stream.begin_display_history(run, Some(surge_core::RunStatus::Completed));
        for event in &events {
            stream.apply_recorded(
                &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                    seq: event.seq.0,
                    payload: Box::new(event.payload.payload.clone()),
                },
                event.timestamp_ms,
            );
        }
        stream.finish_display_history(events.last().unwrap().seq.0);
        assert_eq!(
            stream.trusted_work_item(),
            Some(item),
            "actual startup binding survives terminal hydration"
        );
        let recorded_prompt = stream.prompt.clone().unwrap();
        assert!(recorded_prompt.starts_with("Accepted task requirements"));
        assert_eq!(state.run_prompt(&run), Some(recorded_prompt.as_str()));
        assert_eq!(
            state.run_mission_title(&run),
            Some(detail.item.title.as_str())
        );
        state.tasks.scope.as_mut().unwrap().repository = home.path().join("other.git");
        assert_eq!(
            state.run_mission_title(&run),
            Some(recorded_prompt.as_str())
        );
        state.tasks.scope.as_mut().unwrap().repository = detail.item.workspace.repository.clone();
    });
    view.update(cx, |view, cx| {
        assert!(
            view.tasks(cx).is_empty(),
            "owned completed run must not duplicate its durable task without attempt pages"
        )
    });
    // Mutate this ACTUAL recorded prefix; a late or duplicated valid context
    // must not create ownership authority just because its bytes deserialize.
    let bound = events
        .iter()
        .find(|event| {
            matches!(
                event.payload.payload,
                surge_core::EventPayload::WorkItemAttemptBound { .. }
            )
        })
        .unwrap();
    for kind in ["gap", "duplicate", "late", "prompt mismatch"] {
        let mut payloads: Vec<_> = events
            .iter()
            .map(|event| event.payload.payload.clone())
            .collect();
        let bound_index = payloads
            .iter()
            .position(|payload| {
                matches!(
                    payload,
                    surge_core::EventPayload::WorkItemAttemptBound { .. }
                )
            })
            .unwrap();
        match kind {
            "duplicate" => {
                payloads.insert(bound_index + 1, bound.payload.payload.clone());
            },
            "late" => {
                let binding = payloads.remove(bound_index);
                payloads.push(binding);
            },
            "prompt mismatch" => {
                if let surge_core::EventPayload::RunStarted { initial_prompt, .. } =
                    &mut payloads[0]
                {
                    *initial_prompt = "Different accepted input".into();
                }
            },
            _ => {},
        }
        let mut stream = crate::run_stream::RunStreamState::default();
        stream.begin_display_history(run, Some(surge_core::RunStatus::Completed));
        for (index, payload) in payloads.into_iter().enumerate() {
            if kind == "gap" && index == bound_index {
                continue;
            }
            stream.apply(
                &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                    seq: index as u64 + 1,
                    payload: Box::new(payload),
                },
            );
        }
        assert_eq!(
            stream.trusted_work_item(),
            None,
            "{kind} invalidates ownership"
        );
        assert!(
            !stream.task_ownership_confirmed(),
            "{kind} remains explicitly unconfirmed"
        );
    }
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("filter-all").label(),
            Some("All  1"),
            "actual journal links the terminal run to one task"
        );
        window.remove_window();
    })
    .unwrap();
    drop(root);
    server.abort();
    host.block_on(async {
        let _ = server.await;
    });
}

// Keep this independent of Fleet async command/reload ownership.
struct FourDraftInputs {
    inputs: Vec<gpui_kit::Entity<gpui_kit::component::input::TextareaState>>,
}

impl gpui_kit::Render for FourDraftInputs {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        gpui_kit::div().flex().flex_col().children(
            self.inputs
                .iter()
                .map(gpui_kit::component::input::Textarea::new),
        )
    }
}

#[gpui_kit::test]
fn four_textarea_inputs_release_after_window_owner_drop(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut inputs = Vec::new();
    let (handle, root) = cx.update(|cx| {
        gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
            let owner = cx.new(|cx| FourDraftInputs {
                inputs: (0..4)
                    .map(|_| {
                        cx.new(|cx| gpui_kit::component::input::TextareaState::new(window, cx))
                    })
                    .collect(),
            });
            inputs = owner
                .read(cx)
                .inputs
                .iter()
                .map(gpui_kit::Entity::downgrade)
                .collect();
            cx.new(|cx| gpui_kit::component::Root::new(owner, window, cx))
        })
        .unwrap()
    });
    // Render the same four real input elements, then close the complete window.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.remove_window();
    })
    .unwrap();
    assert_eq!(inputs.len(), 4);
    drop(root);
    cx.update(|_| {});
    cx.run_until_parked();
    cx.update(|_| {});
    assert!(
        inputs.iter().all(|input| input.upgrade().is_none()),
        "window-owned inputs must be released"
    );
}

async fn show_task(
    facade: &DaemonEngineFacade,
    item: surge_core::id::WorkItemId,
) -> surge_core::work_item::WorkItemDetail {
    let WorkItemResult::Detail(detail) = facade
        .work_item(WorkItemCommand::Show { item })
        .await
        .unwrap()
    else {
        panic!("detail");
    };
    *detail
}

#[gpui_kit::test]
fn newly_admitted_suspend_cannot_use_old_executing_detail(cx: &mut TestAppContext) {
    use surge_core::execution_recovery::ExecutionControlState;
    use surge_core::run_event::EventPayload;
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    init_project(project.path());
    let host = tokio::runtime::Runtime::new().unwrap();
    let (storage, socket, server) = host.block_on(start_daemon(home.path(), project.path()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let facade = Arc::new(
        runtime
            .block_on(DaemonEngineFacade::connect(socket))
            .unwrap(),
    );
    let WorkItemResult::Detail(created) = runtime
        .block_on(
            facade.work_item(WorkItemCommand::Create {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                project: project.path().into(),
                title: "Current control".into(),
                requirements: WorkItemRequirements::new(
                    "Hold this task".into(),
                    vec!["Keep original decision".into()],
                )
                .unwrap(),
            }),
        )
        .unwrap()
    else {
        panic!("created");
    };
    let item = created.item.id;
    let mut graph: surge_core::Graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    graph.nodes.get_mut(&graph.start).unwrap().config = serde_json::from_value(serde_json::json!({
        "node_kind":"human_gate", "delivery_channels":[], "summary":{"title":"Hold", "body":"Wait for operator"},
        "options":[{"outcome":"done","label":"Done"}], "allow_freetext":true,
    })).unwrap();
    assert!(!surge_core::validation::validate(&graph).has_errors());
    let WorkItemResult::Attempt(started) = runtime
        .block_on(facade.work_item(WorkItemCommand::Start {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item,
            expected_version: created.item.version,
            graph: Box::new(graph),
            quota_recovery: None,
        }))
        .unwrap()
    else {
        panic!("attempt");
    };
    let run = started.run;
    let prefix = host
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let inspection = storage.inspect_run(run).await.unwrap();
                    if let surge_persistence::runs::inspection::RunDatabaseInspection::Present {
                        events,
                    } = inspection.database
                        && let Some(index) = events.iter().position(|event| {
                            matches!(
                                event.payload.payload,
                                EventPayload::HumanInputRequested { .. }
                            )
                        })
                    {
                        break events.into_iter().take(index).collect::<Vec<_>>();
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
        })
        .unwrap();
    let initial = runtime.block_on(show_task(&facade, item));
    let WorkItemResult::Control(paused) = runtime
        .block_on(facade.work_item(WorkItemCommand::Suspend {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item,
            expected_version: initial.item.version,
        }))
        .unwrap()
    else {
        panic!("pause");
    };
    assert_eq!(paused.state, ExecutionControlState::Suspended);
    let inspection = host.block_on(storage.inspect_run(run)).unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present {
        events: paused_events,
    } = inspection.database
    else {
        panic!("paused journal");
    };
    let requests: Vec<_> = paused_events
        .iter()
        .filter_map(|event| match &event.payload.payload {
            EventPayload::HumanInputRequested {
                call_id: Some(request),
                ..
            } => Some((event.seq.as_u64(), request.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        requests.len(),
        1,
        "real suspended run has exactly its original durable question"
    );
    let mut gate_stream = crate::run_stream::RunStreamState::default();
    gate_stream.begin_display_history(run, None);
    for event in &paused_events {
        gate_stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: event.seq.as_u64(),
                payload: Box::new(event.payload.payload.clone()),
            },
            event.timestamp_ms,
        );
    }
    gate_stream.finish_display_history(paused_events.last().unwrap().seq.as_u64());
    assert_eq!(
        gate_stream.pending.len(),
        1,
        "actual original question is present before the owner closes"
    );
    gate_stream.apply(
        &surge_orchestrator::engine::handle::EngineRunEvent::Terminal {
            outcome: surge_orchestrator::engine::handle::RunOutcome::Suspended {
                fence: Box::new(paused.fence.clone().unwrap()),
            },
        },
    );
    assert_eq!(
        gate_stream.pending.len(),
        1,
        "nonterminal suspension must retain the original decision"
    );
    gate_stream.begin_display_history(run, None);
    for event in &paused_events {
        gate_stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: event.seq.as_u64(),
                payload: Box::new(event.payload.payload.clone()),
            },
            event.timestamp_ms,
        );
    }
    gate_stream.finish_display_history(paused_events.last().unwrap().seq.as_u64());
    assert_eq!(
        gate_stream.pending.len(),
        1,
        "trusted hydration restores the question independently of old stream watermark"
    );
    assert_eq!(gate_stream.pending[0].seq, requests[0].0);
    let crate::run_stream::DecisionKind::HumanInput { call_id, .. } = &gate_stream.pending[0].kind
    else {
        panic!("original human input");
    };
    assert_eq!(call_id.as_ref(), Some(&requests[0].1));
    let paused_detail = runtime.block_on(show_task(&facade, item));
    runtime
        .block_on(facade.work_item(WorkItemCommand::Continue {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item,
            expected_version: paused_detail.item.version,
            new_session: false,
        }))
        .unwrap();
    let old =
        runtime
            .block_on(tokio::time::timeout(
                std::time::Duration::from_secs(5),
                async {
                    loop {
                        let detail = show_task(&facade, item).await;
                        if detail.control.as_ref().is_some_and(|control| {
                            control.state == ExecutionControlState::Executing
                        }) {
                            break detail;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                },
            ))
            .unwrap();
    let WorkItemResult::Control(new_control) = runtime
        .block_on(facade.work_item(WorkItemCommand::Suspend {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item,
            expected_version: old.item.version,
        }))
        .unwrap()
    else {
        panic!("admitted control");
    };
    let fresh = runtime.block_on(show_task(&facade, item));
    assert!(fresh.item.version > old.item.version);
    assert!(new_control.generation > old.control.as_ref().unwrap().generation);
    assert_ne!(new_control.state, ExecutionControlState::Executing);
    let mut stream = crate::run_stream::RunStreamState::default();
    stream.begin_display_history(run, None);
    for event in prefix {
        stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: event.seq.0,
                payload: Box::new(event.payload.payload),
            },
            event.timestamp_ms,
        );
    }
    assert_eq!(
        stream.display().label(),
        "Running",
        "actual cached journal prefix was working"
    );
    stream.live = true;
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade);
        state.tasks.begin(old.item.workspace.repository.clone());
        state.tasks.records.push(old.item.clone());
        state.tasks.details.insert(item, old.clone());
        state.tasks.fresh = true;
        state.run_streams.insert(run, stream);
        state
    });
    let view = cx.new(|cx| FleetScreen::new(state.clone(), cx));
    view.update(cx, |view, cx| {
        assert_eq!(view.durable_status(&old.item, cx), "Running")
    });
    state.update(cx, |state, _| {
        state.tasks.records[0] = fresh.item.clone();
    });
    view.update(cx, |view, cx| {
        assert_eq!(
            view.state.read(cx).tasks.details[&item]
                .control
                .as_ref()
                .unwrap()
                .state,
            ExecutionControlState::Executing
        );
        assert_eq!(
            view.durable_status(&fresh.item, cx),
            "Run state is unconfirmed",
            "new admitted control cannot be replaced by an older executing detail"
        );
        assert!(!view.task_detail_confirmed(item, cx));
        view.selected_item = Some(item);
        view.submit_task(
            WorkItemCommand::Suspend {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item,
                expected_version: old.item.version,
            },
            cx,
        );
        assert!(
            !view.task_submissions.contains_key(&item),
            "stale detail cannot create a new control operation"
        );
    });
    let facade = cx.read(|cx| state.read(cx).daemon_state.facade().unwrap());
    let (_, history) = runtime
        .block_on(crate::work_items::load_task(&facade, item))
        .unwrap();
    state.update(cx, |state, _| {
        let old_request = state.tasks.begin_detail(item);
        let new_request = state.tasks.begin_detail(item);
        state
            .tasks
            .apply_detail(new_request, fresh.clone(), history.clone());
        state
            .tasks
            .apply_detail(old_request, old.clone(), history.clone());
        let later_request = state.tasks.begin_detail(item);
        state
            .tasks
            .apply_detail(later_request, old.clone(), history);
        let scope = state.tasks.scope.clone().unwrap();
        state.tasks.apply_page(
            &scope,
            surge_core::work_item::WorkItemPage {
                entries: vec![old.item.clone()],
                next_cursor: None,
            },
            false,
        );
        assert_eq!(state.tasks.records[0].version, fresh.item.version);
        assert_eq!(
            state.tasks.details[&item]
                .control
                .as_ref()
                .unwrap()
                .generation,
            new_control.generation
        );
    });
    view.update(cx, |view, cx| {
        view.submit_task(
            WorkItemCommand::Suspend {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item: surge_core::id::WorkItemId::new(),
                expected_version: fresh.item.version,
            },
            cx,
        );
        assert!(
            !view.task_submissions.contains_key(&item),
            "old callback cannot target a different selected task"
        );
        view.submit_task(
            WorkItemCommand::Suspend {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item,
                expected_version: old.item.version,
            },
            cx,
        );
        assert!(
            !view.task_submissions.contains_key(&item),
            "old rendered version cannot dispatch after refresh"
        );
    });
    let before_continue = runtime.block_on(show_task(&facade, item));
    runtime
        .block_on(facade.work_item(WorkItemCommand::Continue {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item,
            expected_version: before_continue.item.version,
            new_session: false,
        }))
        .unwrap();
    let request_node = paused_events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::HumanInputRequested { node, .. } => Some(node.clone()),
            _ => None,
        })
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match facade
                    .resolve_gate_input(
                        run,
                        request_node.clone(),
                        requests[0].1.parse().unwrap(),
                        serde_json::json!({"outcome":"done"}),
                    )
                    .await
                {
                    Ok(()) => break,
                    Err(surge_orchestrator::engine::error::EngineError::StaleGateRequest) => {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await
                    },
                    Err(error) => panic!("original answer: {error}"),
                }
            }
        })
        .await
        .unwrap();
    });
    let completed_events = host.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let inspection = storage.inspect_run(run).await.unwrap();
                if let surge_persistence::runs::inspection::RunDatabaseInspection::Present {
                    events,
                } = inspection.database
                    && events.iter().any(|event| {
                        matches!(event.payload.payload, EventPayload::RunCompleted { .. })
                    })
                {
                    break events;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    });
    assert_eq!(
        completed_events
            .iter()
            .filter(|event| matches!(
                event.payload.payload,
                EventPayload::HumanInputRequested { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        completed_events
            .iter()
            .filter(|event| matches!(
                event.payload.payload,
                EventPayload::HumanInputResolved { .. }
            ))
            .count(),
        1
    );
    gate_stream.begin_display_history(run, None);
    for event in &completed_events {
        gate_stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: event.seq.as_u64(),
                payload: Box::new(event.payload.payload.clone()),
            },
            event.timestamp_ms,
        );
    }
    gate_stream.finish_display_history(completed_events.last().unwrap().seq.as_u64());
    assert!(
        gate_stream.pending.is_empty(),
        "actual resolved and completed question is retired"
    );
    gate_stream.begin_display_history(run, None);
    for event in &paused_events {
        gate_stream.apply_recorded(
            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                seq: event.seq.as_u64(),
                payload: Box::new(event.payload.payload.clone()),
            },
            event.timestamp_ms,
        );
    }
    gate_stream.finish_display_history(paused_events.last().unwrap().seq.as_u64());
    assert!(
        gate_stream.pending.is_empty(),
        "an older valid prefix cannot revive a finished question"
    );
    assert_eq!(
        gate_stream.display(),
        surge_core::run_display::RunDisplayState::Unknown
    );
    assert!(!gate_stream.session_history_confirmed());
    server.abort();
    host.block_on(async {
        let _ = server.await;
    });
}
