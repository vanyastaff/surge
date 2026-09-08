//! Render-level smoke tests for every screen.
//!
//! `surge-ui` had zero tests that construct or render a `Screen` type —
//! `cargo test -p surge-ui` compiled the crate but never proved a screen
//! survives its own `render()`. A panic in any `Render::render` impl is a
//! live crash for a user, and before this file nothing in the gate caught
//! that.
//!
//! Each test opens a real `gpui` test window via
//! [`gpui::TestAppContext::add_window_view`], constructing the screen the
//! same way `main.rs` does (`gpui_component::init` + `crate::theme::init`
//! before anything else). Opening the window forces one full
//! layout/prepaint/paint pass synchronously — see `App::open_window`'s
//! `window.draw(cx)` call, kept unconditional specifically so a window is
//! never returned without having rendered at least once — so a
//! construction panic *or* a render panic both fail the test.
//!
//! These are plain `#[test]` functions building `TestAppContext::single()`
//! directly, not `#[gpui::test]`. Two things about this crate's build make
//! that necessary, both isolated by bisection (identical content, only the
//! named condition changed):
//!
//! 1. `#[gpui::test]` (`gpui_macros`, unlocked by the `test-support`
//!    feature) reproducibly overflows rustc's stack while expanding, at
//!    *any* `#![recursion_limit]` including 8192, only when compiled as
//!    part of `surge-ui`'s full dependency graph — an otherwise-identical
//!    `#[gpui::test]` compiles clean at the *default* limit (128) in a
//!    throwaway crate depending on nothing but `gpui`. `TestAppContext::
//!    single()` is the same public constructor `#[gpui::test]` calls
//!    internally (`gpui::TestAppContext::build`/`single` in
//!    `gpui-0.2.2/src/app/test_context.rs`), so this loses only the
//!    macro's seeded-retry sugar (unused by a smoke test), not fidelity.
//! 2. `use gpui::*;` in *this* module reproduces the same stack overflow —
//!    isolated to a two-line repro (a bare `#[test] fn` calling only
//!    `TestAppContext::single()`, no screen, no `AppState`): swapping the
//!    glob for `use gpui::TestAppContext;` alone made it compile, and
//!    swapping back reliably broke it again, in a freshly `--target-dir`'d
//!    build (rules out target-directory/sccache staleness) and across
//!    repeated runs (rules out scheduling flakiness). Every `screens/*.rs`
//!    file keeps its own `use gpui::*;` without issue, so this is specific
//!    to something about compiling that particular glob inside a new
//!    module added to this specific crate graph — left as a build-tooling
//!    finding, not chased further into `gpui_macros`/rustc internals.
//!
//! Screens driven by `AppState` are exercised against both the empty
//! first-run state (`AppState::new()` — no project, no tasks, no specs, no
//! runs, no worktrees) and a populated state, because several screens
//! branch on `state.tasks.is_empty()` / `state.specs.is_empty()` /
//! `state.worktrees.is_empty()` / `state.runs.is_empty()`, and the empty
//! branch had never executed under a test before this file.
//!
//! `WelcomeScreen` is the one screen this file cannot put in both states:
//! its recent-projects list comes from `RecentProjects::load()`, which
//! reads `$SURGE_HOME/recent.toml` from the real environment. Forcing it
//! via `std::env::set_var("SURGE_HOME", ..)` would be unsound in this
//! binary — it links vendored C (`libgit2` via `surge-orchestrator`,
//! bundled `sqlite3` via `surge-persistence`) whose `getenv` reads
//! `environ` outside std's own lock, so mutating env from a test thread
//! races with it (the same constraint `crate::project::tests` documents
//! against `RecentProjects::file_path`). `WelcomeScreen` therefore gets one
//! smoke test against whatever the ambient environment provides.

use std::path::PathBuf;

// Targeted imports, not `use gpui::*;` — see the module doc above for why
// the glob form is not safe to use in this file.
use gpui::{AppContext as _, Context, Render, TestAppContext, Window};

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

/// Mirrors `main.rs`'s startup sequence. `gpui_component::init` registers
/// the globals `Button`/`Input`/`Select`/... read; a screen using one of
/// those widgets would panic on a bare `TestAppContext` that skipped this.
fn init_components(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
    });
    crate::theme::init();
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
    render_screen(&mut cx, |_, cx| MemoryScreen::new(cx));
}

#[test]
fn spec_wizard_screen_renders() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    render_screen(&mut cx, |_, cx| SpecWizardScreen::new(cx));
}

#[test]
fn welcome_screen_renders() {
    let mut cx = TestAppContext::single();
    init_components(&mut cx);
    render_screen(&mut cx, |_, cx| WelcomeScreen::new(cx));
}
