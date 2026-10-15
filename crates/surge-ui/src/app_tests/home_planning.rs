use super::*;
use std::sync::{Arc, Mutex};
use surge_core::bootstrap_operation::BootstrapIntent;
use surge_orchestrator::engine::{
    daemon_facade::DaemonEngineFacade,
    ipc::{DaemonRequest, DaemonResponse, ErrorCode, read_request_frame, write_frame},
};
#[derive(Default)]
struct Calls {
    bootstrap: Vec<(RunId, BootstrapIntent)>,
    work_item_creates: usize,
}
#[test]
fn home_primary_opens_planning_without_launch_then_explicit_create_sends_exact_bootstrap() {
    run_isolated_app_probe("primary", "app::tests::home_planning::planning_child_probe");
}
#[test]
fn stale_wizard_create_after_project_switch_submits_nothing() {
    run_isolated_app_probe("stale", "app::tests::home_planning::planning_child_probe");
}
#[test]
fn planning_child_probe() {
    let Ok(route) = std::env::var("SURGE_TEST_PROJECT_HOME_ENTRY") else {
        return;
    };
    if !matches!(route.as_str(), "primary" | "stale") {
        return;
    }
    let root = PathBuf::from(std::env::var_os("SURGE_HOME").unwrap());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let (facade, server) = runtime.block_on(connect_probe(&root, calls.clone()));
    let _entered = runtime.enter();
    let mut cx = TestAppContext::single();
    cx.update(gpui_kit::init);
    let project = root.join("project-a");
    std::fs::create_dir(&project).unwrap();
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(project.clone());
        state.config = Some(surge_core::SurgeConfig::default());
        state
    });
    let app = cx.new(|cx| {
        let mut app = SurgeApp::new_shell(state.clone(), cx);
        app.mode = super::super::AppMode::Project {
            _path: project.clone(),
            _name: "fixture".into(),
        };
        app.navigate(Screen::Home, cx);
        app
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(app.clone(), window, cx));
    window.update(|_, cx| {
        state.update(cx, |state, _| {
            state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade)
        })
    });
    if route == "primary" {
        check_primary(window, &app, &calls, &project);
    } else {
        check_stale(window, &app, &state, &calls, &root);
    }
    runtime.block_on(async {
        server.abort();
        let _ = server.await;
    });
}
async fn connect_probe(
    root: &std::path::Path,
    calls: Arc<Mutex<Calls>>,
) -> (Arc<DaemonEngineFacade>, tokio::task::JoinHandle<()>) {
    let socket = root.join("planning.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(read);
        while let Some(request) = read_request_frame(&mut reader).await.unwrap() {
            let request_id = request.request_id();
            match request {
                DaemonRequest::StartBootstrap {
                    operation_id,
                    intent,
                    ..
                } => calls
                    .lock()
                    .unwrap()
                    .bootstrap
                    .push((operation_id, *intent)),
                DaemonRequest::WorkItem { command, .. }
                    if matches!(
                        *command,
                        surge_core::work_item::WorkItemCommand::Create { .. }
                    ) =>
                {
                    calls.lock().unwrap().work_item_creates += 1
                },
                other => panic!("unexpected planning request {other:?}"),
            }
            write_frame(
                &mut write,
                &DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::BadRequest,
                    message: "Probe recorded the exact request; no execution launched".into(),
                },
            )
            .await
            .unwrap();
        }
    });
    let facade = DaemonEngineFacade::connect(socket).await.unwrap();
    (Arc::new(facade), server)
}
fn check_primary(
    window: &mut gpui_kit::VisualTestContext,
    app: &Entity<SurgeApp>,
    calls: &Arc<Mutex<Calls>>,
    project: &std::path::Path,
) {
    use gpui_kit::test::TestWindowExt;
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click("home-add-idea", cx);
    });
    window.run_until_parked();
    window.update(|_, cx| {
        assert_eq!(
            app.read(cx).active_screen,
            Screen::SpecWizard,
            "primary Home action must open planning, not require a workflow TOML"
        )
    });
    assert!(
        calls.lock().unwrap().bootstrap.is_empty(),
        "opening planning cannot launch a run"
    );
    let submitted = Rc::new(RefCell::new(Vec::new()));
    let copy = submitted.clone();
    window.update(|_, cx| {
        let wizard = app.read(cx).spec_wizard.as_ref().unwrap().clone();
        cx.subscribe(&wizard, move |_, event: &SpecWizardEvent, _| {
            if let SpecWizardEvent::Create { run_id, .. } = event {
                copy.borrow_mut().push(*run_id);
            }
        })
        .detach();
    });
    let prompt = window.debug_bounds("planning-prompt").unwrap();
    window.simulate_click(prompt.center(), Modifiers::default());
    window.simulate_input(PROMPT);
    let submit = window.debug_bounds("planning-submit").unwrap();
    window.simulate_click(submit.center(), Modifiers::default());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while calls.lock().unwrap().bootstrap.is_empty() && std::time::Instant::now() < deadline {
        window.run_until_parked();
        std::thread::yield_now();
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls.bootstrap.len(), 1);
    assert_eq!(calls.work_item_creates, 0);
    assert_eq!(calls.bootstrap[0].0, submitted.borrow()[0]);
    assert_eq!(calls.bootstrap[0].1.project_path(), project);
    assert_eq!(calls.bootstrap[0].1.prompt(), PROMPT);
}
fn check_stale(
    window: &mut gpui_kit::VisualTestContext,
    app: &Entity<SurgeApp>,
    state: &Entity<AppState>,
    calls: &Arc<Mutex<Calls>>,
    root: &std::path::Path,
) {
    let operation = RunId::new();
    let project_b = root.join("project-b");
    std::fs::create_dir(&project_b).unwrap();
    let before = window.update(|_, cx| {
        let old = cx.new(|cx| SpecWizardScreen::new(root.join("project-a"), cx));
        let newer = cx.new(|cx| SpecWizardScreen::new(project_b.clone(), cx));
        state.update(cx, |state, _| state.project_path = Some(project_b));
        app.update(cx, |app, cx| {
            app.spec_wizard = Some(newer);
            let before = app.bootstrap_index.len();
            app.handle_wizard_event(
                old,
                &SpecWizardEvent::Create {
                    run_id: operation,
                    description: PROMPT.into(),
                },
                cx,
            );
            before
        })
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        window.run_until_parked();
        std::thread::yield_now();
    }
    let calls = calls.lock().unwrap();
    assert!(
        calls.bootstrap.is_empty(),
        "stale wizard must submit to neither project"
    );
    assert_eq!(calls.work_item_creates, 0);
    window.update(|_, cx| {
        assert_eq!(app.read(cx).bootstrap_index.len(), before);
        assert!(!app.read(cx).stream_subscribed.contains(&operation));
    });
}
