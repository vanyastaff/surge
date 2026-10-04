//! Exercise the application submission boundary with controlled daemon responses.
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext};
use surge_core::RunId;

use super::{DispatchOrigin, SurgeApp};
use crate::app_state::AppState;
use crate::router::Screen;
use crate::screens::spec_wizard::{SpecWizardEvent, SpecWizardScreen};

const PROMPT: &str = "  Build a timer\nwith keyboard controls.  ";
const PROJECT: &str = "/tmp/surge-ui-planning-fixture";

#[test]
fn project_switcher_opens_native_picker_and_preserves_project_on_cancel() {
    let (mut cx, app, _, _) = fixture();
    app.update(&mut cx, |app, cx| {
        app.install_top_bar("fixture", std::path::Path::new(PROJECT), cx)
    });
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

#[test]
fn sidebar_new_task_opens_durable_form_from_another_screen() {
    let (mut cx, app, _, _) = fixture();
    app.update(&mut cx, |app, cx| app.navigate(Screen::Settings, cx));
    let sidebar = cx.update(|cx| app.read(cx).sidebar.clone());
    sidebar.update(&mut cx, |_, cx| cx.emit(crate::sidebar::CreateTask));
    let fleet = cx.update(|cx| {
        let app = app.read(cx);
        assert_eq!(app.active_screen, Screen::Fleet);
        app.fleet.clone().unwrap()
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(fleet.clone(), window, cx));
    assert!(window.debug_bounds("task-create-form").is_some());
}

#[test]
fn sidebar_all_tasks_returns_from_creation_without_replacing_fleet() {
    let (mut cx, app, _, _) = fixture();
    app.update(&mut cx, |app, cx| app.open_new_task(cx));
    let (sidebar, fleet) = cx.update(|cx| {
        let app = app.read(cx);
        (app.sidebar.clone(), app.fleet.clone().unwrap())
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(fleet.clone(), window, cx));
    assert!(window.debug_bounds("task-create-form").is_some());
    window.update(|_, cx| {
        sidebar.update(cx, |_, cx| cx.emit(crate::sidebar::ShowTasks));
    });
    assert!(window.debug_bounds("task-create-form").is_none());
    window.update(|_, cx| assert_eq!(app.read(cx).fleet.as_ref(), Some(&fleet)));
    window.update(|_, cx| app.update(cx, |app, cx| app.open_new_task(cx)));
    assert!(window.debug_bounds("task-create-form").is_some());
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
        app.update(cx, |app, cx| {
            app.navigate(Screen::SpecWizard, cx);
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
        app.dispatch_bootstrap(
            PROMPT.into(),
            id,
            DispatchOrigin::Wizard(wizard.clone()),
            cx,
        );
    });
    cx.read(|cx| {
        assert_eq!(wizard.read(cx).prompt(cx), PROMPT);
        assert!(wizard.read(cx).error().unwrap().contains("Daemon offline"));
        assert_eq!(app.read(cx).active_screen, Screen::SpecWizard);
        assert!(app.read(cx).pending_run_selection.is_none());
    });
}

#[test]
fn planning_rejection_preserves_the_draft() {
    let (mut cx, app, wizard, id) = fixture();
    app.update(&mut cx, |app, cx| {
        app.dispatch_failed(
            id,
            &DispatchOrigin::Wizard(wizard.clone()),
            "queue full".into(),
            cx,
        );
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
        app.dispatch_bootstrap(
            PROMPT.into(),
            id,
            DispatchOrigin::Wizard(wizard.clone()),
            cx,
        );
    });
    cx.read(|cx| {
        assert_eq!(wizard.read(cx).prompt(cx), PROMPT);
        assert!(wizard.read(cx).error().unwrap().contains("USD cap"));
        assert_eq!(app.read(cx).bootstrap_index.len(), before);
    });
}

#[test]
fn planning_acceptance_selects_the_acknowledged_run() {
    let (mut cx, app, wizard, id) = fixture();
    let planning_id = RunId::new();
    app.update(&mut cx, |app, cx| {
        app.dispatch_accepted(
            id,
            planning_id,
            std::path::Path::new(PROJECT),
            &DispatchOrigin::Wizard(wizard.clone()),
            cx,
        );
    });
    cx.read(|cx| {
        assert_eq!(app.read(cx).active_screen, Screen::Runs);
        assert_eq!(app.read(cx).pending_run_selection, Some(planning_id));
        assert!(app.read(cx).spec_wizard.is_none());
    });
}

#[test]
fn planning_late_response_cannot_navigate_or_clear_another_draft() {
    for (switch_project, accepted) in [(false, true), (true, true), (false, false), (true, false)] {
        let (mut cx, app, old, id) = fixture();
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
            if accepted {
                app.dispatch_accepted(
                    id,
                    RunId::new(),
                    std::path::Path::new(PROJECT),
                    &DispatchOrigin::Wizard(old.clone()),
                    cx,
                );
            } else {
                app.dispatch_failed(
                    id,
                    &DispatchOrigin::Wizard(old.clone()),
                    "late rejection".into(),
                    cx,
                );
            }
        });
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
    let planning_id = RunId::new();
    app.update(&mut cx, |app, cx| {
        app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::Cancel, cx);
        app.dispatch_accepted(
            id,
            planning_id,
            std::path::Path::new(PROJECT),
            &DispatchOrigin::Wizard(wizard.clone()),
            cx,
        );
        assert_eq!(app.active_screen, app.wizard_return_screen);
        app.navigate(Screen::SpecWizard, cx);
        app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::OpenRun(id), cx);
        assert_eq!(app.active_screen, Screen::Runs);
        assert_eq!(app.pending_run_selection, Some(planning_id));
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
            app.dispatch_accepted(
                id,
                RunId::new(),
                std::path::Path::new(PROJECT),
                &DispatchOrigin::Wizard(wizard.clone()),
                cx,
            );
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

#[test]
fn fleet_start_offline_preserves_exact_input_at_application_boundary() {
    let (mut cx, app, _, _) = fixture();
    app.update(&mut cx, |app, cx| {
        app.state.update(cx, |state, _| {
            state.config.as_mut().unwrap().analytics.budget_usd = None
        });
        app.active_screen = Screen::Fleet;
        let _ = app.render_screen_content(cx);
    });
    let fleet = cx.update(|cx| app.read(cx).fleet.clone().unwrap());
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(fleet.clone(), window, cx));
    let prompt = window.debug_bounds("fleet-prompt-region").unwrap();
    window.simulate_click(prompt.center(), Modifiers::default());
    window.simulate_input("  Exact request, including spaces.  ");
    let submit = window.debug_bounds("fleet-start").unwrap();
    window.simulate_click(submit.center(), Modifiers::default());
    window.update(|_, cx| {
        assert_eq!(
            fleet.read(cx).draft(cx),
            "  Exact request, including spaces.  "
        );
        assert!(fleet.read(cx).error().unwrap().contains("Daemon offline"));
        assert_eq!(app.read(cx).active_screen, Screen::Fleet);
    });
}

#[test]
fn invalid_project_configuration_refuses_submission_inline() {
    let (mut cx, app, wizard, id) = fixture();
    app.update(&mut cx, |app, cx| {
        app.state.update(cx, |state, _| {
            state.project_load_error =
                Some("Invalid surge.toml: repair the analytics section.".into());
        });
        app.dispatch_bootstrap(
            PROMPT.into(),
            id,
            DispatchOrigin::Wizard(wizard.clone()),
            cx,
        );
    });
    cx.read(|cx| {
        assert_eq!(wizard.read(cx).prompt(cx), PROMPT);
        assert!(
            wizard
                .read(cx)
                .error()
                .unwrap()
                .contains("Invalid surge.toml")
        );
        assert!(!wizard.read(cx).is_submitting(id));
    });
}

fn fleet_submission_fixture() -> (
    TestAppContext,
    Entity<SurgeApp>,
    Entity<crate::screens::fleet::FleetScreen>,
    RunId,
) {
    let mut cx = TestAppContext::single();
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(PathBuf::from(PROJECT));
        state
    });
    let app = cx.new(|cx| SurgeApp::new_shell(state.clone(), cx));
    let fleet = cx.new(|cx| crate::screens::fleet::FleetScreen::new(state, cx));
    let submitted = Rc::new(RefCell::new(None));
    let captured = submitted.clone();
    {
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(fleet.clone(), window, cx)
        });
        window.update(|_, cx| {
            cx.subscribe(
                &fleet,
                move |_, event: &crate::screens::fleet::FleetAction, _| {
                    if let crate::screens::fleet::FleetAction::Dispatch(id, prompt) = event {
                        assert_eq!(prompt, "  Exact pending draft.  ");
                        captured.replace(Some(*id));
                    }
                },
            )
            .detach();
            app.update(cx, |app, _| {
                app.active_screen = Screen::Fleet;
                app.fleet = Some(fleet.clone());
            });
        });
        let prompt = window.debug_bounds("fleet-prompt-region").unwrap();
        window.simulate_click(prompt.center(), Modifiers::default());
        window.simulate_input("  Exact pending draft.  ");
        let submit = window.debug_bounds("fleet-start").unwrap();
        window.simulate_click(submit.center(), Modifiers::default());
    }
    let id = submitted
        .borrow()
        .expect("the Fleet button emitted a submission");
    (cx, app, fleet, id)
}

#[test]
fn fleet_acknowledgment_after_leaving_does_not_navigate() {
    let (mut cx, app, fleet, operation_id) = fleet_submission_fixture();
    let planning_id = RunId::new();
    app.update(&mut cx, |app, cx| {
        app.navigate(Screen::Settings, cx);
        app.dispatch_accepted(
            operation_id,
            planning_id,
            std::path::Path::new(PROJECT),
            &DispatchOrigin::Fleet(fleet.clone()),
            cx,
        );
        assert_eq!(app.active_screen, Screen::Settings);
        assert!(app.pending_run_selection.is_none());
        assert!(app.state.read(cx).run_streams[&planning_id].live);
    });
}

#[test]
fn fleet_old_project_acknowledgment_preserves_new_project_input() {
    let (mut cx, app, old_fleet, operation_id) = fleet_submission_fixture();
    let state = cx.update(|cx| app.read(cx).state.clone());
    let newer = cx.new(|cx| crate::screens::fleet::FleetScreen::new(state, cx));
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(newer.clone(), window, cx));
    window.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.state.update(cx, |state, _| {
                state.project_path = Some(PathBuf::from("/tmp/new-project"))
            });
            app.fleet = Some(newer.clone());
        })
    });
    let prompt = window.debug_bounds("fleet-prompt-region").unwrap();
    window.simulate_click(prompt.center(), Modifiers::default());
    window.simulate_input("  New project's draft.  ");
    window.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.dispatch_accepted(
                operation_id,
                RunId::new(),
                std::path::Path::new(PROJECT),
                &DispatchOrigin::Fleet(old_fleet.clone()),
                cx,
            );
            assert_eq!(app.active_screen, Screen::Fleet);
            assert!(app.pending_run_selection.is_none());
            assert_eq!(newer.read(cx).draft(cx), "  New project's draft.  ");
        })
    });
}

#[test]
fn new_task_cancel_returns_to_its_origin_and_late_acknowledgment_stays_there() {
    for origin in [Screen::Fleet, Screen::Backlog] {
        let (mut cx, app, wizard, operation_id) = fixture();
        app.update(&mut cx, |app, cx| {
            app.navigate(origin, cx);
            app.navigate(Screen::SpecWizard, cx);
            app.handle_wizard_event(wizard.clone(), &SpecWizardEvent::Cancel, cx);
            assert_eq!(app.active_screen, origin);
            app.dispatch_accepted(
                operation_id,
                RunId::new(),
                std::path::Path::new(PROJECT),
                &DispatchOrigin::Wizard(wizard.clone()),
                cx,
            );
            assert_eq!(app.active_screen, origin);
            assert!(app.pending_run_selection.is_none());
        });
    }
}

#[test]
fn stale_new_task_cancel_cannot_leave_a_newer_form_or_project() {
    for switch_project in [false, true] {
        let (mut cx, app, old, _) = fixture();
        let project = if switch_project {
            "/tmp/new-project"
        } else {
            PROJECT
        };
        let newer = cx.new(|cx| SpecWizardScreen::new(PathBuf::from(project), cx));
        app.update(&mut cx, |app, cx| {
            app.state.update(cx, |state, _| {
                state.project_path = Some(PathBuf::from(project))
            });
            app.spec_wizard = Some(newer.clone());
            app.handle_wizard_event(old.clone(), &SpecWizardEvent::Cancel, cx);
            assert_eq!(app.active_screen, Screen::SpecWizard);
            assert_eq!(app.spec_wizard.as_ref(), Some(&newer));
        });
    }
}

#[test]
fn older_inbox_plan_opens_its_exact_run_in_workflows() {
    let mut cx = TestAppContext::single();
    cx.update(gpui_kit::init);
    let root = tempfile::tempdir().unwrap();
    let older = RunId::new();
    let newer = RunId::new();
    let old_path = root.path().join("older-flow.toml");
    let new_path = root.path().join("newer-flow.toml");
    let source = include_str!("../testdata/four_level_flow.toml");
    std::fs::write(&old_path, source).unwrap();
    std::fs::write(&new_path, source.replace("final_spec", "newer_spec")).unwrap();
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.project_path = Some(root.path().into());
        for (run, path, seq, age) in [(older, old_path, 1, 1), (newer, new_path, 2, 0)] {
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now() - chrono::Duration::seconds(age),
                last_event_seq: None,
                ended_at: None,
            });
            let mut stream = crate::run_stream::RunStreamState::default();
            stream.artifacts.insert("flow".into(), path);
            stream.pending.push(crate::run_stream::PendingDecision {
                seq,
                time: String::new(),
                node: "flow_gate".into(),
                kind: crate::run_stream::DecisionKind::HumanInput {
                    call_id: Some(format!("gate-{seq}")),
                    prompt: format!("Review plan {seq}"),
                    schema: Some(serde_json::json!({"x-surge-bootstrap-stage":"flow"})),
                },
            });
            state.run_streams.insert(run, stream);
        }
        state
    });
    let app = cx.new(|cx| {
        let mut app = SurgeApp::new_shell(state.clone(), cx);
        app.mode = super::AppMode::Project {
            _path: root.path().into(),
            _name: "fixture".into(),
        };
        app.navigate(Screen::Inbox, cx);
        app
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(app.clone(), window, cx));
    window.run_until_parked();
    let open = window.debug_bounds("decision-open-plan").unwrap();
    window.simulate_click(open.center(), Modifiers::default());
    window.run_until_parked();
    window.update(|_, cx| {
        assert_eq!(app.read(cx).active_screen, Screen::Flow);
        let flow = app.read(cx).flow.as_ref().unwrap();
        assert_eq!(flow.read(cx).displayed_plan_run(), Some(older));
    });
    use gpui_kit::test::TestWindowExt as _;
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from("plan-node-final_spec"), cx);
    });
    window.run_until_parked();
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from("effort-final_spec-3"), cx);
    });
    window.update(|_, cx| {
        assert_eq!(
            state.read(cx).plan_edits[&older].0["final_spec"]
                .effort
                .as_deref(),
            Some("high")
        );
        assert!(!state.read(cx).plan_edits.contains_key(&newer));
    });
}
