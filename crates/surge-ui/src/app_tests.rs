//! Exercise the application submission boundary with controlled daemon responses.
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext};
use surge_core::RunId;
use surge_orchestrator::engine::{EngineError, RunHandle, RunOutcome};

use super::SurgeApp;
use crate::app_state::AppState;
use crate::router::Screen;
use crate::screens::spec_wizard::{SpecWizardEvent, SpecWizardScreen};

const PROMPT: &str = "  Build a timer\nwith keyboard controls.  ";
const PROJECT: &str = "/tmp/surge-ui-planning-fixture";

#[test]
fn project_switcher_opens_native_picker_and_preserves_project_on_cancel() {
    let (mut cx, app, _, _) = fixture();
    app.update(&mut cx, |app, cx| app.install_top_bar("fixture", std::path::Path::new(PROJECT), cx));
    let bar = cx.update(|cx| app.read(cx).top_bar.clone().unwrap());
    assert!(!cx.did_prompt_for_paths());
    bar.update(&mut cx, |_, cx| {
        cx.emit(crate::top_bar::TopBarEvent::OpenOther)
    });
    cx.run_until_parked();
    assert!(
        cx.did_prompt_for_paths(),
        "top-bar events must reach the application"
    );
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories);
        assert!(!options.files);
        None
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            app.read(cx).state.read(cx).project_path.as_deref(),
            Some(std::path::Path::new(PROJECT)),
        );
    });
}

fn fixture() -> (
    TestAppContext,
    Entity<SurgeApp>,
    Entity<SpecWizardScreen>,
    RunId,
) {
    let mut cx = TestAppContext::single();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init();
        crate::theme::sync_component_theme(cx);
    });
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(PathBuf::from(PROJECT));
        let mut config = surge_core::config::SurgeConfig::default();
        config.analytics.budget_usd = Some(12.5);
        state.config = Some(config);
        state
    });
    let app = cx.new(|cx| SurgeApp::new_shell(state, cx));
    let wizard = cx.new(|cx| SpecWizardScreen::new(PathBuf::from(PROJECT), cx));
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(wizard.clone(), window, cx));
    let submitted = Rc::new(RefCell::new(None));
    let copy = submitted.clone();
    window.update(|_, cx| {
        cx.subscribe(&wizard, move |_, event: &SpecWizardEvent, _| {
            if let SpecWizardEvent::Create {
                run_id,
                description,
            } = event
            {
                assert_eq!(description, PROMPT);
                copy.replace(Some(*run_id));
            }
        })
        .detach();
        app.update(cx, |app, _| {
            app.active_screen = Screen::SpecWizard;
            app.spec_wizard = Some(wizard.clone());
            app.wizard_drafts
                .insert(PathBuf::from(PROJECT), wizard.clone());
        });
    });
    let prompt = window.debug_bounds("planning-prompt").unwrap();
    window.simulate_click(prompt.center(), Modifiers::default());
    window.simulate_input(PROMPT);
    let submit = window.debug_bounds("planning-submit").unwrap();
    window.simulate_click(submit.center(), Modifiers::default());
    let id = submitted.borrow().expect("the form submitted");
    (cx, app, wizard, id)
}

fn accepted_handle(id: RunId) -> RunHandle {
    let (_, events) = tokio::sync::broadcast::channel(1);
    RunHandle {
        run_id: id,
        events,
        completion: tokio::spawn(async {
            RunOutcome::Completed {
                terminal: "end".try_into().unwrap(),
            }
        }),
    }
}

#[test]
fn planning_offline_keeps_the_actual_input_and_inline_error() {
    let (mut cx, app, wizard, id) = fixture();
    app.update(&mut cx, |app, cx| {
        // The fixture's USD cap is refused before the daemon is consulted;
        // this test is about the offline path.
        app.state.update(cx, |state, _| {
            if let Some(config) = state.config.as_mut() {
                config.analytics.budget_usd = None;
            }
        });
        app.dispatch_bootstrap(PROMPT.into(), id, Some(wizard.clone()), cx);
    });
    cx.read(|cx| {
        assert_eq!(wizard.read(cx).prompt(cx), PROMPT);
        assert!(wizard.read(cx).error().unwrap().contains("Daemon offline"));
        assert_eq!(app.read(cx).active_screen, Screen::SpecWizard);
        assert!(app.read(cx).pending_run_selection.is_none());
        assert!(app.read(cx).state.read(cx).tasks.is_empty());
    });
}

#[test]
fn planning_rejection_preserves_draft_and_captured_project_configuration() {
    let (mut cx, app, wizard, id) = fixture();
    let request = Rc::new(RefCell::new(None));
    let capture = request.clone();
    app.update(&mut cx, |app, cx| {
        app.dispatch_with(
            PROMPT.into(),
            "bootstrap",
            id,
            Some(wizard.clone()),
            move |request| async move {
                capture.replace(Some(request));
                Err(EngineError::Internal("queue full".into()))
            },
            cx,
        );
    });
    cx.run_until_parked();
    let request = request.borrow();
    let request = request.as_ref().unwrap();
    assert_eq!(request.run_id, id);
    assert_eq!(request.project_path, PathBuf::from(PROJECT));
    assert_eq!(request.config.initial_prompt, PROMPT);
    assert_eq!(request.graph.metadata.name, "bootstrap");
    assert_eq!(request.config.budget, {
        surge_core::config::AnalyticsConfig {
            budget_usd: Some(12.5),
            ..Default::default()
        }
        .budget_guard()
    });
    cx.read(|cx| {
        assert_eq!(wizard.read(cx).prompt(cx), PROMPT);
        assert!(wizard.read(cx).error().unwrap().contains("queue full"));
        assert!(!wizard.read(cx).is_submitting(id));
        assert_eq!(app.read(cx).active_screen, Screen::SpecWizard);
        assert!(app.read(cx).pending_run_selection.is_none());
    });
}

#[test]
fn durable_bootstrap_refuses_unsupported_usd_cap_without_dropping_it() {
    let (mut cx, app, wizard, id) = fixture();
    // The index is loaded from SURGE_HOME, which may hold real operations;
    // the contract is that a refused submission adds nothing to it.
    let before = cx.read(|cx| app.read(cx).bootstrap_index.len());
    app.update(&mut cx, |app, cx| {
        app.dispatch_bootstrap(PROMPT.into(), id, Some(wizard.clone()), cx);
    });
    cx.read(|cx| {
        assert_eq!(wizard.read(cx).prompt(cx), PROMPT);
        assert!(wizard.read(cx).error().unwrap().contains("USD cap"));
        assert_eq!(app.read(cx).bootstrap_index.len(), before);
    });
}

#[test]
fn planning_acceptance_selects_acknowledged_run_only_after_response() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let (mut cx, app, wizard, id) = fixture();
    let (answer, reply) = tokio::sync::oneshot::channel();
    app.update(&mut cx, |app, cx| {
        app.dispatch_with(
            PROMPT.into(),
            "bootstrap",
            id,
            Some(wizard.clone()),
            |_| async move { reply.await.unwrap() },
            cx,
        );
    });
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(app.read(cx).active_screen, Screen::SpecWizard);
        assert!(app.read(cx).pending_run_selection.is_none());
    });
    assert!(answer.send(Ok(accepted_handle(id))).is_ok());
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(app.read(cx).active_screen, Screen::Runs);
        assert_eq!(app.read(cx).pending_run_selection, Some(id));
        assert!(app.read(cx).state.read(cx).tasks.is_empty());
        assert!(app.read(cx).spec_wizard.is_none());
    });
}

#[test]
fn planning_late_response_cannot_navigate_or_clear_another_draft() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    for (switch_project, accepted) in [(false, true), (true, true), (false, false), (true, false)] {
        let (mut cx, app, old, id) = fixture();
        let (answer, reply) = tokio::sync::oneshot::channel();
        app.update(&mut cx, |app, cx| {
            app.dispatch_with(
                PROMPT.into(),
                "bootstrap",
                id,
                Some(old.clone()),
                |_| async move { reply.await.unwrap() },
                cx,
            );
        });
        cx.run_until_parked();
        let project = if switch_project {
            "/tmp/another-project"
        } else {
            PROJECT
        };
        let newer = cx.new(|cx| SpecWizardScreen::new(PathBuf::from(project), cx));
        app.update(&mut cx, |app, cx| {
            app.state.update(cx, |state, _| {
                state.project_path = Some(PathBuf::from(project))
            });
            app.spec_wizard = Some(newer.clone());
        });
        let response = if accepted {
            Ok(accepted_handle(id))
        } else {
            Err(EngineError::Internal("late rejection".into()))
        };
        assert!(answer.send(response).is_ok());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(app.read(cx).active_screen, Screen::SpecWizard);
            assert_eq!(app.read(cx).spec_wizard.as_ref(), Some(&newer));
            assert!(app.read(cx).pending_run_selection.is_none());
            assert_eq!(old.read(cx).prompt(cx), PROMPT);
            assert!(!old.read(cx).is_submitting(id));
            assert!(newer.read(cx).error().is_none());
            if !accepted {
                assert!(old.read(cx).error().unwrap().contains("late rejection"));
            }
        });
    }
}

#[test]
fn planning_opening_a_late_accepted_run_allows_a_fresh_task() {
    let (mut cx, app, wizard, id) = fixture();
    app.update(&mut cx, |app, cx| {
        app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::Cancel, cx);
        app.dispatch_accepted(id, std::path::Path::new(PROJECT), Some(&wizard), cx);
        assert_eq!(app.active_screen, Screen::Backlog);
        app.navigate(Screen::SpecWizard, cx);
        app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::OpenRun(id), cx);
        assert_eq!(app.active_screen, Screen::Runs);
        assert_eq!(app.pending_run_selection, Some(id));
        app.navigate(Screen::SpecWizard, cx);
        let _ = app.render_screen_content(cx);
        let next = app.spec_wizard.as_ref().expect("new task form");
        assert_ne!(
            next, &wizard,
            "accepted request must not occupy the next task form"
        );
        assert_eq!(next.read(cx).prompt(cx), "");
    });
}

#[test]
fn planning_open_run_ignores_a_mismatched_project_form_or_run() {
    for mismatch in ["project", "form", "run"] {
        let (mut cx, app, wizard, id) = fixture();
        app.update(&mut cx, |app, cx| {
            app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::Cancel, cx);
            app.dispatch_accepted(id, std::path::Path::new(PROJECT), Some(&wizard), cx);
            app.navigate(Screen::SpecWizard, cx);
            let mut requested_id = id;
            match mismatch {
                "project" => app.state.update(cx, |state, _| {
                    state.project_path = Some(PathBuf::from("/tmp/different-project"));
                }),
                "form" => {
                    app.spec_wizard =
                        Some(cx.new(|cx| SpecWizardScreen::new(PathBuf::from(PROJECT), cx)));
                },
                _ => requested_id = RunId::new(),
            }
            let current = app.spec_wizard.clone();
            app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::OpenRun(requested_id), cx);
            assert_eq!(app.active_screen, Screen::SpecWizard);
            assert_eq!(app.spec_wizard, current);
            assert_eq!(
                app.wizard_drafts.get(&PathBuf::from(PROJECT)),
                Some(&wizard)
            );
            assert!(app.pending_run_selection.is_none());
        });
    }
}
