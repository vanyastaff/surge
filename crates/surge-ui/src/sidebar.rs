use std::collections::{HashMap, HashSet};

use gpui_kit::component::Icon;
use gpui_kit::component::StyledExt;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::app_state::AppState;
use crate::daemon_link::ConnectionState;
use crate::router::Screen;
use crate::theme;
use crate::ui;

/// Action dispatched when a nav item is clicked.
#[derive(Clone, PartialEq)]
pub struct NavigateTo(pub Screen);

impl EventEmitter<NavigateTo> for AppSidebar {}

/// Open the task creation form in the current project.
#[derive(Clone, PartialEq)]
pub struct CreateTask;

impl EventEmitter<CreateTask> for AppSidebar {}

/// A real task selected from the retained project history.
#[derive(Clone, Copy, PartialEq)]
pub enum OpenRecentTask {
    Saved(surge_core::id::WorkItemId),
    Run(surge_core::RunId),
}

impl EventEmitter<OpenRecentTask> for AppSidebar {}

/// Return to the complete project task list.
#[derive(Clone, PartialEq)]
pub struct ShowTasks;

impl EventEmitter<ShowTasks> for AppSidebar {}

fn recent_tasks(state: &AppState) -> Vec<(OpenRecentTask, String)> {
    let records: Vec<_> = state
        .tasks
        .records
        .iter()
        .filter(|record| {
            state
                .tasks
                .scope
                .as_ref()
                .is_some_and(|scope| record.workspace.repository == scope.repository)
        })
        .collect();
    let mut task_runs: HashSet<_> = records
        .iter()
        .filter_map(|record| record.active_run)
        .collect();
    for record in &records {
        if let Some(history) = state.tasks.histories.get(&record.id) {
            task_runs.extend(history.attempts.entries.iter().map(|attempt| attempt.run));
        }
    }
    task_runs.extend(state.run_streams.iter().filter_map(|(run, stream)| {
        let item = stream.trusted_work_item()?;
        records
            .iter()
            .any(|record| {
                record.id == item
                    && state.run_in_project(run)
                    && stream
                        .git_common_dir
                        .as_ref()
                        .is_none_or(|repository| repository == &record.workspace.repository)
            })
            .then_some(*run)
    }));
    records
        .into_iter()
        .map(|record| (OpenRecentTask::Saved(record.id), record.title.clone()))
        .chain(
            state
                .project_runs()
                .into_iter()
                .filter(|run| !task_runs.contains(&run.run_id))
                .map(|run| {
                    (
                        OpenRecentTask::Run(run.run_id),
                        state
                            .run_prompt(&run.run_id)
                            .map_or_else(|| format!("Run {}", run.run_id), str::to_owned),
                    )
                }),
        )
        .collect()
}

/// Action to toggle sidebar collapsed state.
#[derive(Clone, PartialEq)]
pub struct ToggleSidebar;

impl EventEmitter<ToggleSidebar> for AppSidebar {}

/// Operator clicked the offline daemon footer — start the daemon.
#[derive(Clone, PartialEq)]
pub struct StartDaemon;

impl EventEmitter<StartDaemon> for AppSidebar {}

/// Operator navigation with everyday destinations, expandable project
/// tools, and live engine status.
pub struct AppSidebar {
    active: Screen,
    collapsed: bool,
    customize_expanded: bool,
    customize_focus: FocusHandle,
    create_task_focus: FocusHandle,
    recent_focus: [FocusHandle; 6],
    all_tasks_focus: FocusHandle,
    navigation_focus: HashMap<Screen, FocusHandle>,
    /// Read-only handle to app state for the live daemon footer. The
    /// rail observes it so the footer re-renders when the daemon link
    /// flips or the run list changes — no faked "LIVE".
    state: Entity<AppState>,
}

impl AppSidebar {
    pub fn new(
        active: Screen,
        collapsed: bool,
        state: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Re-render the footer whenever app state changes (daemon link
        // transitions, run list updates).
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self {
            active,
            collapsed,
            customize_expanded: Screen::customize_items().contains(&active),
            customize_focus: cx.focus_handle(),
            create_task_focus: cx.focus_handle(),
            recent_focus: std::array::from_fn(|_| cx.focus_handle()),
            all_tasks_focus: cx.focus_handle(),
            navigation_focus: Screen::sidebar_items()
                .iter()
                .chain(Screen::customize_items())
                .map(|screen| (*screen, cx.focus_handle()))
                .collect(),
            state,
        }
    }

    pub fn set_active(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active = screen;
        if Screen::customize_items().contains(&screen) {
            self.customize_expanded = true;
        }
        cx.notify();
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

    /// Count of decisions blocked on the operator — the Inbox badge.
    /// Live gates from run streams + tasks in review + failed runs.
    fn needs_you_count(&self, cx: &Context<Self>) -> usize {
        self.state.read(cx).needs_you_count()
    }

    fn render_nav_item(&self, screen: Screen, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let is_active = self.active == screen;
        let collapsed = self.collapsed;
        let label = screen.label();
        let badge = if screen == Screen::Inbox {
            let n = self.needs_you_count(cx);
            (n > 0).then_some(n)
        } else {
            None
        };

        let base = div()
            .id(SharedString::from(format!("nav-{label}")))
            .accessibility_id(SharedString::from(format!("nav-{label}")))
            .role(Role::Button)
            .aria_label(badge.map_or_else(
                || label.to_string(),
                |count| format!("{label}, {count} waiting"),
            ))
            .h_flex()
            .gap(px(9.0))
            .items_center()
            .px(px(10.0))
            .min_h(px(38.0))
            .py(px(8.0))
            .mx(px(6.0))
            .rounded(px(ui::R_CONTROL))
            .border_1()
            .border_color(transparent_black())
            .cursor_pointer()
            .when_some(self.navigation_focus.get(&screen), |row, focus| {
                row.track_focus(focus)
            })
            .focus_visible(|style| style.border_color(theme::accent()))
            .tab_index(0)
            .tab_stop(true)
            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.emit(NavigateTo(screen));
                    cx.stop_propagation();
                }
            }))
            .when(collapsed, |el| el.justify_center())
            .tooltip(move |window, cx| {
                let mut text = screen.shortcut().map_or_else(
                    || label.to_string(),
                    |shortcut| format!("{label}  {}", ui::shortcut_label(shortcut)),
                );
                if let Some(count) = badge {
                    text.push_str(&format!(" · {count} waiting"));
                }
                Tooltip::new(text).build(window, cx)
            })
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(NavigateTo(screen));
            }));

        let base = if is_active {
            base.bg(theme::surface()).text_color(theme::text_primary())
        } else {
            base.text_color(theme::text_muted())
                .hover(|s: StyleRefinement| {
                    s.bg(theme::panel_raised())
                        .text_color(theme::text_primary())
                })
        };

        let icon_color = if is_active {
            theme::text_primary()
        } else {
            theme::text_muted()
        };
        let mut row = base.child(
            Icon::new(screen.icon())
                .size(px(17.0))
                .text_color(icon_color),
        );

        if !collapsed {
            row = row.child(
                div()
                    .flex_1()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.to_string()),
            );

            if let Some(n) = badge {
                row = row.child(
                    div()
                        .px(px(6.0))
                        .rounded_full()
                        .bg(theme::tint(theme::warning()))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child(n.to_string()),
                );
            }
        }

        row.test_support()
            .debug_selector(move || format!("nav-{label}"))
    }

    fn render_create_task(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id("sidebar-create-task")
            .accessibility_id("sidebar-create-task")
            .role(Role::Button)
            .aria_label("New task")
            .h_flex()
            .items_center()
            .gap(px(9.0))
            .min_h(px(40.0))
            .px(px(10.0))
            .mx(px(6.0))
            .mb(px(12.0))
            .rounded(px(ui::R_CONTROL))
            .border_1()
            .border_color(transparent_black())
            .text_size(px(15.0))
            .text_color(theme::text_primary())
            .cursor_pointer()
            .when(self.collapsed, |el| el.justify_center())
            .hover(|style: StyleRefinement| style.bg(theme::surface()))
            .track_focus(&self.create_task_focus)
            .focus_visible(|style| style.border_color(theme::accent()))
            .tab_index(0)
            .tab_stop(true)
            .tooltip(|window, cx| Tooltip::new("New task").build(window, cx))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.emit(CreateTask);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(CreateTask)))
            .child(Icon::new(gpui_kit::component::IconName::Plus).size(px(17.0)))
            .when(!self.collapsed, |el| el.child("New task"))
            .test_support()
            .debug_selector(|| "sidebar-create-task".into())
    }

    fn render_recent_tasks(&self, cx: &mut Context<Self>) -> Div {
        let tasks = recent_tasks(self.state.read(cx));
        let more = tasks.len() > 6 || self.state.read(cx).tasks.next_cursor.is_some();
        div()
            .v_flex()
            .mt(px(18.0))
            .gap(px(2.0))
            .when(!tasks.is_empty(), |list| {
                list.child(
                    div()
                        .mx(px(12.0))
                        .pt(px(12.0))
                        .pb(px(6.0))
                        .border_t_1()
                        .border_color(theme::hairline())
                        .text_size(px(14.0))
                        .text_color(theme::text_dim())
                        .child("Project tasks"),
                )
            })
            .children(
                tasks
                    .into_iter()
                    .take(6)
                    .enumerate()
                    .map(|(index, (target, title))| {
                        let label = ui::headline(&title, 34);
                        div()
                            .id(SharedString::from(format!("recent-task-{index}")))
                            .role(Role::Button)
                            .aria_label(format!("Open task {title}"))
                            .h_flex()
                            .gap(px(9.0))
                            .items_center()
                            .min_h(px(38.0))
                            .mx(px(6.0))
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(ui::R_CONTROL))
                            .border_1()
                            .border_color(transparent_black())
                            .text_color(theme::text_muted())
                            .text_size(px(14.0))
                            .cursor_pointer()
                            .hover(|style: StyleRefinement| {
                                style.bg(theme::surface()).text_color(theme::text_primary())
                            })
                            .track_focus(&self.recent_focus[index])
                            .focus_visible(|style| style.border_color(theme::accent()))
                            .tab_index(0)
                            .tab_stop(true)
                            .tooltip(move |window, cx| {
                                Tooltip::new(title.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(target)))
                            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    cx.emit(target);
                                    cx.stop_propagation();
                                }
                            }))
                            .child(
                                Icon::new(gpui_kit::assets::IconName::MessageCircle).size(px(17.0)),
                            )
                            .child(div().min_w_0().flex_1().truncate().child(label))
                            .test_support()
                            .debug_selector(move || format!("recent-task-{index}"))
                    }),
            )
            .when(more, |list| {
                list.child(
                    div()
                        .id("recent-tasks-all")
                        .role(Role::Button)
                        .aria_label("Show all tasks")
                        .track_focus(&self.all_tasks_focus)
                        .border_1()
                        .border_color(transparent_black())
                        .focus_visible(|style| style.border_color(theme::accent()))
                        .tab_index(0)
                        .tab_stop(true)
                        .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.emit(ShowTasks);
                                cx.stop_propagation();
                            }
                        }))
                        .mx(px(6.0))
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(ui::R_CONTROL))
                        .text_size(px(14.0))
                        .text_color(theme::text_muted())
                        .cursor_pointer()
                        .hover(|style: StyleRefinement| style.bg(theme::surface()))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ShowTasks)))
                        .child("All tasks"),
                )
            })
    }

    fn render_customize_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let expanded = self.customize_expanded;
        div()
            .id("nav-advanced")
            .accessibility_id("nav-advanced")
            .role(Role::Button)
            .aria_label(if expanded {
                "Collapse Customize"
            } else {
                "Expand Customize"
            })
            .aria_expanded(expanded)
            .tooltip(|window, cx| {
                Tooltip::new("Customize workflows, agents, and project tools").build(window, cx)
            })
            .h_flex()
            .items_center()
            .gap(px(9.0))
            .min_h(px(38.0))
            .px(px(10.0))
            .mx(px(6.0))
            .mt(px(12.0))
            .rounded(px(ui::R_CONTROL))
            .border_1()
            .border_color(transparent_black())
            .text_size(px(15.0))
            .text_color(theme::text_muted())
            .cursor_pointer()
            .when(self.collapsed, |el| el.justify_center())
            .hover(|style: StyleRefinement| style.bg(theme::surface()))
            .track_focus(&self.customize_focus)
            .focus_visible(|style| style.border_color(theme::accent()).bg(theme::surface()))
            .tab_index(0)
            .tab_stop(true)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.customize_expanded = !this.customize_expanded;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(Icon::new(gpui_kit::component::IconName::Settings).size(px(16.0)))
            .when(!self.collapsed, |el| {
                el.child(div().flex_1().child("Customize"))
                    .child(if expanded { "−" } else { "+" })
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.customize_expanded = !this.customize_expanded;
                cx.notify();
            }))
            .test_support()
            .debug_selector(|| "nav-advanced".into())
    }

    /// Footer: whether the engine is reachable (click to start it when it
    /// is not) and the collapse toggle — one row, nothing else competes.
    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let state = self.state.read(cx);
        let offline = matches!(
            &state.daemon_state,
            ConnectionState::Failed(_) | ConnectionState::Disconnected
        );
        let (dot, label): (Hsla, &str) = match &state.daemon_state {
            ConnectionState::Connected(_) => (theme::success(), "Live"),
            ConnectionState::Connecting => (theme::warning(), "Connecting…"),
            ConnectionState::Failed(_) | ConnectionState::Disconnected => {
                (theme::error(), "Offline · Start")
            },
        };
        let collapsed = self.collapsed;

        let status = div()
            .id("daemon-footer")
            .role(if offline { Role::Button } else { Role::Label })
            .aria_label(format!("Engine: {label}"))
            .h_flex()
            .gap(px(8.0))
            .items_center()
            .h(px(36.0))
            .px(px(8.0))
            .rounded(px(ui::R_CONTROL))
            .when(!collapsed, |el| el.flex_1())
            .when(offline, |el| {
                el.cursor_pointer()
                    .hover(|s: StyleRefinement| s.bg(theme::surface()))
                    .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(StartDaemon)))
            })
            .child(
                if matches!(state.daemon_state, ConnectionState::Connected(_)) {
                    ui::live_dot(dot)
                } else {
                    ui::status_dot(dot)
                },
            )
            .when(!collapsed, |el| {
                el.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(if offline {
                            theme::text_primary()
                        } else {
                            theme::text_muted()
                        })
                        .child(label),
                )
            });

        div()
            .flex()
            .when(collapsed, |el| el.flex_col())
            .items_center()
            .gap(px(4.0))
            .px(px(6.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(theme::hairline())
            .child(status)
            .child(self.render_toggle_button(cx))
    }

    fn render_toggle_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        use gpui_kit::component::IconName;
        let icon_name = if self.collapsed {
            IconName::PanelLeftOpen
        } else {
            IconName::PanelLeftClose
        };
        div()
            .id("sidebar-toggle")
            .role(Role::Button)
            .aria_label(if self.collapsed {
                "Expand sidebar"
            } else {
                "Collapse sidebar"
            })
            .size(px(36.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(ui::R_CONTROL))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.bg(theme::surface()))
            .on_click(cx.listener(|_this, _event, _window, cx| {
                cx.emit(ToggleSidebar);
            }))
            .child(
                Icon::new(icon_name)
                    .size_4()
                    .text_color(theme::text_muted()),
            )
    }
}

impl Render for AppSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = if self.collapsed { px(60.0) } else { px(270.0) };

        let items: Vec<_> = Screen::sidebar_items()
            .iter()
            .filter(|&&screen| screen != Screen::Settings)
            .map(|&screen| self.render_nav_item(screen, cx))
            .collect();

        div()
            .v_flex()
            .w(width)
            .h_full()
            .font_family(ui::BODY)
            .flex_shrink_0()
            .bg(theme::sidebar_bg())
            .border_r_1()
            .border_color(theme::hairline())
            // Nav items
            .child(
                div().id("sidebar-navigation").v_flex().flex_1().min_h_0().overflow_y_scroll().gap(px(4.0)).pt(px(16.0))
                    .child(self.render_create_task(cx))
                    .children(items)
                    .child(self.render_customize_toggle(cx))
                    .when(self.customize_expanded, |el| {
                        el.children(Screen::customize_items().iter().map(|&screen| self.render_nav_item(screen, cx)))
                    })
                    .when(!self.collapsed, |el| el.child(self.render_recent_tasks(cx)))
            )
            // Settings stays within reach below the scrollable navigation.
            .child(self.render_nav_item(Screen::Settings, cx))
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui_kit::{AppContext as _, Modifiers, TestAppContext};

    use super::{AppSidebar, CreateTask, NavigateTo};
    use crate::{app_state::AppState, router::Screen};

    #[test]
    fn recent_history_is_scoped_and_does_not_duplicate_durable_active_runs() {
        use std::path::PathBuf;
        use surge_core::work_item::{WorkItemRecord, WorkItemWorkspace};
        let repository = PathBuf::from("/project/a/.git");
        let mut state = AppState::new();
        state.tasks.begin(repository.clone());
        let run = surge_core::RunId::new();
        let item = surge_core::id::WorkItemId::new();
        let record = WorkItemRecord {
            id: item,
            project: surge_core::id::WorkItemProjectId::new(),
            title: "Real retained task".into(),
            accepted_revision: 1,
            version: 1,
            archived_at_ms: None,
            active_run: Some(run),
            generation: 1,
            workspace: WorkItemWorkspace {
                repository,
                checkout: PathBuf::from("/project/a"),
                path: PathBuf::from("/project/a/task"),
                ownership: "fixture".into(),
                branch: "task/fixture".into(),
                base_commit: "a".repeat(40),
            },
        };
        let mut foreign = record.clone();
        foreign.id = surge_core::id::WorkItemId::new();
        foreign.workspace.repository = PathBuf::from("/project/b/.git");
        foreign.title = "Foreign task".into();
        state.tasks.records = vec![record, foreign];
        state.runs.push(crate::app_state::UiRun {
            run_id: run,
            status: surge_orchestrator::engine::handle::RunStatus::Completed,
            started_at: chrono::Utc::now(),
            last_event_seq: None,
            ended_at: None,
        });
        let rows = super::recent_tasks(&state);
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].0, super::OpenRecentTask::Saved(id) if id == item));
        assert_eq!(rows[0].1, "Real retained task");
    }

    #[test]
    fn sidebar_main_routes_and_advanced_navigation() {
        assert_eq!(
            Screen::sidebar_items(),
            [
                Screen::Fleet,
                Screen::Roadmap,
                Screen::Inbox,
                Screen::Runs,
                Screen::Settings,
            ]
        );
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let state = cx.new(|_| AppState::new());
        let sidebar = cx.new(|cx| AppSidebar::new(Screen::Fleet, false, state, cx));
        let navigated = Rc::new(RefCell::new(None));
        let emitted = navigated.clone();
        let created = Rc::new(RefCell::new(0));
        let created_event = created.clone();
        cx.update(|cx| {
            cx.subscribe(&sidebar, move |_, _: &CreateTask, _| {
                *created_event.borrow_mut() += 1;
            })
            .detach();
            cx.subscribe(&sidebar, move |_, event: &NavigateTo, _| {
                emitted.replace(Some(event.0));
            })
            .detach();
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(sidebar.clone(), window, cx)
        });
        let new_task = window.debug_bounds("sidebar-create-task").unwrap();
        window.simulate_click(new_task.center(), Modifiers::default());
        assert_eq!(*created.borrow(), 1);
        window.update(|window, cx| {
            let focus = sidebar.read(cx).create_task_focus.clone();
            focus.focus(window, cx);
        });
        window.simulate_keystrokes("enter");
        assert_eq!(*created.borrow(), 2);
        for id in [
            "nav-Tasks",
            "nav-Plan",
            "nav-Decisions",
            "nav-Results",
            "nav-Settings",
        ] {
            assert!(window.debug_bounds(id).is_some());
        }
        window.update(|window, cx| {
            let focus = sidebar.read(cx).navigation_focus[&Screen::Fleet].clone();
            focus.focus(window, cx);
        });
        window.simulate_keystrokes("enter");
        assert_eq!(*navigated.borrow(), Some(Screen::Fleet));
        assert!(window.debug_bounds("nav-Workflows").is_none());
        let toggle = window.debug_bounds("nav-advanced").unwrap();
        window.simulate_click(toggle.center(), Modifiers::default());
        let roadmap = window.debug_bounds("nav-Workflows").unwrap();
        window.simulate_click(roadmap.center(), Modifiers::default());
        assert_eq!(*navigated.borrow(), Some(Screen::Flow));
        window.simulate_click(toggle.center(), Modifiers::default());
        assert!(window.debug_bounds("nav-Workflows").is_none());
        window.update(|window, cx| {
            let focus = sidebar.read(cx).customize_focus.clone();
            focus.focus(window, cx);
        });
        window.simulate_keystrokes("enter");
        assert!(window.debug_bounds("nav-Workflows").is_some());
        window.simulate_keystrokes("space");
        assert!(window.debug_bounds("nav-Workflows").is_none());
        window.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.set_active(Screen::ContextMemory, cx)
            })
        });
        assert!(window.debug_bounds("nav-Memory").is_some());
    }
}
