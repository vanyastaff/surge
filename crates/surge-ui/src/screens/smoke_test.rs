//! Render-level smoke tests for every screen, with empty and populated state.
//!
//! These construct GPUI test windows and exercise real layout/paint and input.
//! Imports stay explicit: a glob also imports GPUI's `test` attribute macro,
//! shadowing Rust's built-in `#[test]` in this module.
//! Welcome uses the ambient recent-project list; tests do not mutate process
//! environment variables while native dependencies can concurrently read them.

use std::path::PathBuf;

// Targeted imports, not `use gpui_kit::*;` — see the module doc above for why
// the glob form is not safe to use in this file.
use gpui_kit::{AppContext as _, Context, Render, TestAppContext, Window};

use crate::app_state::{AppState, TaskEntry, UiRun, WorktreeEntry};

use super::agent_hub::AgentHubScreen;
use super::agent_terminal::AgentTerminalScreen;
use super::agents::AgentsScreen;
use super::backlog::BacklogScreen;
use super::fleet::FleetScreen;
use super::flow::FlowScreen;
use super::inbox::InboxScreen;
use super::memory::MemoryScreen;
use super::roadmap::RoadmapScreen;
use super::runs::RunsScreen;
use super::settings::SettingsScreen;
use super::spec_explorer::SpecExplorerScreen;
use super::spec_wizard::SpecWizardScreen;
use super::welcome::WelcomeScreen;
use super::worktrees::WorktreesScreen;

/// Mirrors `main.rs`'s startup sequence. `gpui_kit::component::init` registers
/// the globals `Button`/`Input`/`Select`/... read; a screen using one of
/// those widgets would panic on a bare `TestAppContext` that skipped this.
fn init_components(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init();
        crate::theme::sync_component_theme(cx);
    });
}

/// The first-run state: no project loaded, every collection empty.
fn empty_app_state() -> AppState {
    AppState::new()
}

/// One representative entry in every collection the screens branch on.
fn populated_app_state() -> AppState {
    let mut state = AppState::new();
    state.project_path = Some(PathBuf::from("/tmp/surge-ui-smoke/project"));
    state.project_name = "smoke-project".to_string();
    state.current_branch = "feat/smoke".to_string();

    state.tasks.push(TaskEntry {
        id: surge_core::TaskId::new(),
        _spec_id: surge_core::SpecId::new(),
        title: "Wire the render smoke harness".to_string(),
        description: "Construct and render every surge-ui screen under test.".to_string(),
        state: surge_core::TaskState::Executing {
            completed: 1,
            total: 3,
        },
        agent: Some("claude-acp".to_string()),
        complexity: "Standard".to_string(),
        _created_at: "2026-09-01T00:00:00Z".to_string(),
        updated_at: "2026-09-08T00:00:00Z".to_string(),
    });

    state.specs.push(surge_core::Spec::new(
        "Smoke-test coverage",
        "Give surge-ui a render-level smoke test.",
        surge_core::Complexity::Standard,
    ));

    state.worktrees.push(WorktreeEntry {
        spec_id: "spec-smoke".to_string(),
        branch: "feat/smoke".to_string(),
        path: PathBuf::from("/tmp/surge-ui-smoke/project/.worktrees/spec-smoke"),
        exists: true,
    });

    state.runs.push(UiRun {
        run_id: surge_core::RunId::new(),
        status: surge_orchestrator::engine::handle::RunStatus::Active,
        started_at: chrono::Utc::now(),
        last_event_seq: Some(7),
        ended_at: None,
    });

    state
}

/// Opens a real test window around `build`, which forces the synchronous
/// construction + layout + paint pass described above, then runs the
/// executor to parked so an effect scheduled during construction (e.g.
/// `RoadmapScreen::reload`'s `cx.observe`) also gets a chance to run — and
/// panic here — instead of only at runtime.
fn render_screen<V: 'static + Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
) {
    let (_view, window_cx) = cx.add_window_view(build);
    window_cx.run_until_parked();
}

// ── `(state: Entity<AppState>, cx)` screens: empty + populated ─────────

#[test]
fn agent_hub_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| AgentHubScreen::new(state, cx));
}

#[test]
fn agent_hub_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| AgentHubScreen::new(state, cx));
}

#[test]
fn agent_terminal_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| AgentTerminalScreen::new(state, cx));
}

#[test]
fn agent_terminal_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| AgentTerminalScreen::new(state, cx));
}

#[test]
fn agents_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| AgentsScreen::new(state, cx));
}

#[test]
fn agents_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| AgentsScreen::new(state, cx));
}

#[test]
fn backlog_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| BacklogScreen::new(state, cx));
}

#[test]
fn backlog_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| BacklogScreen::new(state, cx));
}

#[test]
fn fleet_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| FleetScreen::new(state, cx));
}

#[test]
fn fleet_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| FleetScreen::new(state, cx));
}

#[test]
fn inbox_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| InboxScreen::new(state, cx));
}

#[test]
fn inbox_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| InboxScreen::new(state, cx));
}

#[test]
fn roadmap_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| RoadmapScreen::new(state, cx));
}

#[test]
fn roadmap_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| RoadmapScreen::new(state, cx));
}

#[test]
fn runs_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| RunsScreen::new(state, cx));
}

#[test]
fn runs_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| RunsScreen::new(state, cx));
}

#[test]
fn settings_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| SettingsScreen::new(state, cx));
}

#[test]
fn settings_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| SettingsScreen::new(state, cx));
}

#[test]
fn spec_explorer_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| SpecExplorerScreen::new(state, cx));
}

#[test]
fn spec_explorer_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| SpecExplorerScreen::new(state, cx));
}

#[test]
fn worktrees_screen_renders_empty_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| WorktreesScreen::new(state, cx));
}

#[test]
fn worktrees_screen_renders_populated_state() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| populated_app_state());
    render_screen(&mut cx, |_, cx| WorktreesScreen::new(state, cx));
}

// ── `(cx)`-only screens ──────────────────────────────────────────────────

#[test]
fn flow_screen_renders() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    render_screen(&mut cx, |_, cx| FlowScreen::new(cx));
}

#[test]
fn memory_screen_renders() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let state = cx.new(|_| empty_app_state());
    render_screen(&mut cx, |_, cx| MemoryScreen::new(state, cx));
}

#[test]
fn spec_wizard_screen_renders() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    render_screen(&mut cx, |_, cx| {
        SpecWizardScreen::new(PathBuf::from("/tmp/planning-project"), cx)
    });
}

#[test]
fn welcome_screen_renders() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    render_screen(&mut cx, |_, cx| WelcomeScreen::new(cx));
}

#[test]
fn planning_wizard_submits_the_operators_exact_multiline_prompt() {
    use super::spec_wizard::SpecWizardEvent;
    use std::cell::RefCell;
    use std::rc::Rc;
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let view = cx.new(|cx| SpecWizardScreen::new(PathBuf::from("/tmp/planning-project"), cx));
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
    let captured = Rc::new(RefCell::new(Vec::new()));
    let output = captured.clone();
    window.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &SpecWizardEvent, _| {
            if let SpecWizardEvent::Create { description, .. } = event {
                output.borrow_mut().push(description.clone());
            }
        })
        .detach()
    });
    let input = window
        .debug_bounds("planning-prompt")
        .expect("visible prompt");
    window.simulate_click(input.center(), gpui_kit::Modifiers::default());
    let prompt = "  Build a timer\nwith keyboard controls.  ";
    window.simulate_input(prompt);
    let submit = window
        .debug_bounds("planning-submit")
        .expect("submit button");
    window.simulate_click(submit.center(), gpui_kit::Modifiers::default());
    // A second click while awaiting acknowledgment must not send another run.
    window.simulate_click(submit.center(), gpui_kit::Modifiers::default());
    assert_eq!(*captured.borrow(), vec![prompt.to_string()]);
}

#[test]
fn planning_wizard_rejects_blank_input_without_submission() {
    use super::spec_wizard::SpecWizardEvent;
    use std::cell::Cell;
    use std::rc::Rc;
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    let view = cx.new(|cx| SpecWizardScreen::new(PathBuf::from("/tmp/planning-project"), cx));
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
    let count = Rc::new(Cell::new(0));
    let captured = count.clone();
    window.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &SpecWizardEvent, _| {
            if matches!(event, SpecWizardEvent::Create { .. }) {
                captured.set(captured.get() + 1);
            }
        })
        .detach()
    });
    let input = window.debug_bounds("planning-prompt").unwrap();
    window.simulate_click(input.center(), gpui_kit::Modifiers::default());
    window.simulate_input("  \n  ");
    let submit = window.debug_bounds("planning-submit").unwrap();
    window.simulate_click(submit.center(), gpui_kit::Modifiers::default());
    assert_eq!(count.get(), 0);
}
