//! Backlog — every task of this project's missions, where it stands.
//!
//! Columns are derived from the task ledger (the index `surge ready` and
//! `surge ledger` read), so a card never disagrees with the engine:
//!
//! - **To do** — pending; ready ones first, then the ones waiting on a
//!   dependency ("blocked by …"), plus tasks found mid-run
//! - **In progress** — running or being checked
//! - **Needs you** — failed, failed its check, or paused
//! - **Done** — completed (verified or not, shown apart) or skipped
//!
//! Work a finished mission left behind can be started again as a new
//! mission from its card. An empty project shows an empty board — never
//! sample cards.

use std::collections::HashSet;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::RunId;
use surge_core::roadmap::RoadmapStatus;

use crate::app_state::AppState;
use crate::backlog_source::{self, BacklogTask};
use crate::theme::{self, Semantic};
use crate::ui;

/// What the board can ask the app shell to do.
#[derive(Clone)]
pub enum BacklogAction {
    /// Open the mission a task belongs to.
    OpenMission(RunId),
    /// Start describing a new mission.
    NewTask,
    /// Start a task again as a new mission (prompt = the task).
    Dispatch { prompt: String },
}

impl EventEmitter<BacklogAction> for BacklogScreen {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Column {
    Todo,
    InProgress,
    NeedsYou,
    Done,
}

impl Column {
    const ALL: [Column; 4] = [Self::Todo, Self::InProgress, Self::NeedsYou, Self::Done];

    fn title(self) -> &'static str {
        match self {
            Self::Todo => "To do",
            Self::InProgress => "In progress",
            Self::NeedsYou => "Needs you",
            Self::Done => "Done",
        }
    }

    fn role(self) -> Semantic {
        match self {
            Self::Todo => Semantic::External,
            Self::InProgress => Semantic::Agent,
            Self::NeedsYou => Semantic::Failure,
            Self::Done => Semantic::Verified,
        }
    }

    fn of(status: RoadmapStatus) -> Self {
        match status {
            RoadmapStatus::Pending => Self::Todo,
            RoadmapStatus::Running | RoadmapStatus::ReadyForVerification => Self::InProgress,
            RoadmapStatus::Failed | RoadmapStatus::FailedVerification | RoadmapStatus::Paused => {
                Self::NeedsYou
            },
            RoadmapStatus::Completed | RoadmapStatus::Skipped => Self::Done,
        }
    }
}

/// The prompt that starts a left-over task as its own mission.
fn restart_prompt(task: &BacklogTask) -> String {
    let mut prompt = task.title.clone();
    if let Some(desc) = &task.description {
        prompt.push_str("\n\n");
        prompt.push_str(desc);
    }
    if !task.acceptance.is_empty() {
        prompt.push_str("\n\nDone when:\n");
        for criterion in &task.acceptance {
            prompt.push_str(&format!("- {criterion}\n"));
        }
    }
    prompt.push_str(&format!(
        "\n(Picked up from the backlog of: {})",
        task.mission
    ));
    prompt
}

/// Backlog screen.
pub struct BacklogScreen {
    state: Entity<AppState>,
    tasks: Vec<BacklogTask>,
    loading: bool,
    loaded_for: Option<(Option<std::path::PathBuf>, Vec<String>)>,
    /// Cards opened for detail: (run, task id).
    open: HashSet<(RunId, String)>,
    /// Show one mission's tasks only.
    mission_filter: Option<RunId>,
}

impl BacklogScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this: &mut Self, _state, cx| {
            this.reload_if_changed(cx)
        })
        .detach();
        let mut this = Self {
            state,
            tasks: Vec::new(),
            loading: false,
            loaded_for: None,
            open: HashSet::new(),
            mission_filter: None,
        };
        this.reload_if_changed(cx);
        this
    }

    fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        let key = {
            let state = self.state.read(cx);
            (
                state.project_path.clone(),
                state
                    .project_runs()
                    .iter()
                    .map(|r| format!("{}:{:?}:{:?}", r.run_id, r.status, r.last_event_seq))
                    .collect::<Vec<_>>(),
            )
        };
        if self.loaded_for.as_ref() == Some(&key) {
            return;
        }
        let root = key.0.clone();
        self.loaded_for = Some(key);
        let Some(root) = root else {
            self.tasks.clear();
            cx.notify();
            return;
        };
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let tasks = match surge_core::home::surge_home_dir() {
                Some(home) => backlog_source::load(&root, &home).await,
                None => Vec::new(),
            };
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.tasks = tasks;
                    screen.loading = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_card(&self, task: &BacklogTask, cx: &mut Context<Self>) -> Div {
        let column = Column::of(task.status);
        let key = (task.implementation_run, task.id.clone());
        let open = self.open.contains(&key);
        let toggle = key.clone();
        let color = if column == Column::Done
            && !task.verified
            && task.status == RoadmapStatus::Completed
        {
            theme::slate()
        } else {
            column.role().color()
        };
        let restartable = task.mission_ended && matches!(column, Column::Todo | Column::NeedsYou);
        let status_label = match task.status {
            RoadmapStatus::ReadyForVerification => Some("checking"),
            RoadmapStatus::FailedVerification => Some("failed its check"),
            RoadmapStatus::Failed => Some("failed"),
            RoadmapStatus::Paused => Some("paused"),
            RoadmapStatus::Skipped => Some("skipped"),
            _ => None,
        };
        let run = task.implementation_run;
        let prompt = restart_prompt(task);

        let head = div()
            .id(SharedString::from(format!(
                "backlog-{}-{}",
                task.implementation_run, task.id
            )))
            .role(Role::Button)
            .aria_label(task.title.clone())
            .v_flex()
            .gap(px(6.0))
            .p(px(11.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _e, _w, cx| {
                if !this.open.remove(&toggle) {
                    this.open.insert(toggle.clone());
                }
                cx.notify();
            }))
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_start()
                    .child(div().pt(px(5.0)).child(if column == Column::InProgress {
                        ui::live_dot(color)
                    } else {
                        ui::status_dot(color)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.5))
                            .line_height(px(17.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if column == Column::Done {
                                theme::text_muted()
                            } else {
                                theme::text_primary()
                            })
                            .child(task.title.clone()),
                    ),
            )
            .child(
                div()
                    .pl(px(15.0))
                    .text_size(px(10.5))
                    .text_color(theme::text_dim())
                    .truncate()
                    .child(match &task.milestone {
                        Some(m) => format!("{m} · {}", task.mission),
                        None => task.mission.clone(),
                    }),
            )
            .child(
                div()
                    .pl(px(15.0))
                    .h_flex()
                    .flex_wrap()
                    .gap(px(5.0))
                    .when(task.verified, |el| {
                        el.child(
                            div()
                                .h_flex()
                                .gap(px(4.0))
                                .items_center()
                                .text_size(px(10.0))
                                .text_color(theme::success())
                                .child(Icon::new(Lucide::ShieldCheck).size(px(11.0)))
                                .child("verified"),
                        )
                    })
                    .when(
                        task.status == RoadmapStatus::Completed && !task.verified,
                        |el| el.child(ui::role_badge("unverified", Semantic::External)),
                    )
                    .when_some(status_label, |el, label| {
                        el.child(ui::pill(label, color, theme::tint(color)))
                    })
                    .when(!task.blocked_by.is_empty(), |el| {
                        el.child(ui::role_badge(
                            format!("blocked by {}", task.blocked_by.join(", ")),
                            Semantic::External,
                        ))
                    })
                    .when_some(task.discovered_from.clone(), |el, from| {
                        el.child(ui::role_badge(format!("found in {from}"), Semantic::Loop))
                    })
                    .when(restartable, |el| {
                        el.child(ui::role_badge("left over", Semantic::You))
                    }),
            );

        ui::panel()
            .flex_none()
            .rounded(px(ui::R_CONTROL + 2.0))
            .overflow_hidden()
            .border_color(if open {
                theme::hairline_strong()
            } else {
                theme::hairline()
            })
            .child(head)
            .when(open, |el| {
                el.child(
                    div()
                        .v_flex()
                        .gap(px(8.0))
                        .px(px(11.0))
                        .pb(px(11.0))
                        .pt(px(4.0))
                        .border_t_1()
                        .border_color(theme::hairline())
                        .children(task.description.clone().map(|d| {
                            div()
                                .pt(px(6.0))
                                .text_size(px(11.5))
                                .line_height(px(17.0))
                                .text_color(theme::text_primary())
                                .child(d)
                        }))
                        .when(!task.acceptance.is_empty(), |el| {
                            el.child(
                                div()
                                    .v_flex()
                                    .gap(px(3.0))
                                    .child(ui::section_label("Done when"))
                                    .children(task.acceptance.iter().map(|c| {
                                        div()
                                            .text_size(px(11.0))
                                            .line_height(px(16.0))
                                            .text_color(theme::text_muted())
                                            .child(format!("· {c}"))
                                    })),
                            )
                        })
                        .child(
                            div()
                                .h_flex()
                                .gap(px(6.0))
                                .pt(px(4.0))
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "backlog-open-{run}-{}",
                                        task.id
                                    )))
                                    .ghost()
                                    .small()
                                    .icon(Lucide::Activity)
                                    .label("Open mission")
                                    .on_click(cx.listener(move |_this, _e, _w, cx| {
                                        cx.emit(BacklogAction::OpenMission(run));
                                    })),
                                )
                                .when(restartable, |el| {
                                    el.child(
                                        Button::new(SharedString::from(format!(
                                            "backlog-start-{run}-{}",
                                            task.id
                                        )))
                                        .primary()
                                        .small()
                                        .icon(IconName::Play)
                                        .label("Start as a mission")
                                        .tooltip("Plan and build just this task as a new mission")
                                        .on_click(
                                            cx.listener(move |_this, _e, _w, cx| {
                                                cx.emit(BacklogAction::Dispatch {
                                                    prompt: prompt.clone(),
                                                });
                                            }),
                                        ),
                                    )
                                }),
                        ),
                )
            })
    }

    fn render_column(&self, column: Column, tasks: &[&BacklogTask], cx: &mut Context<Self>) -> Div {
        let color = column.role().color();
        div()
            .flex_1()
            .min_w(px(220.0))
            .h_full()
            .v_flex()
            .gap(px(8.0))
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(4.0))
                    .child(ui::status_dot(color))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child(column.title()),
                    )
                    .child(ui::pill(
                        tasks.len().to_string(),
                        theme::text_muted(),
                        theme::panel_deep(),
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "backlog-col-{}",
                        column.title()
                    )))
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .v_flex()
                    .gap(px(6.0))
                    .p(px(6.0))
                    .rounded(px(ui::R_PANEL))
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::hairline())
                    .when(tasks.is_empty(), |el| {
                        el.child(
                            div()
                                .py(px(18.0))
                                .text_center()
                                .text_size(px(11.0))
                                .text_color(theme::text_dim())
                                .child("—"),
                        )
                    })
                    .children(tasks.iter().map(|t| self.render_card(t, cx))),
            )
    }
}

impl BacklogScreen {
    /// One chip per mission (newest first), plus "All".
    fn render_mission_filter(&self, cx: &mut Context<Self>) -> Div {
        let mut missions: Vec<(RunId, String, usize)> = Vec::new();
        for task in &self.tasks {
            match missions
                .iter_mut()
                .find(|(run, _, _)| *run == task.implementation_run)
            {
                Some((_, _, open)) => *open += usize::from(Column::of(task.status) != Column::Done),
                None => missions.push((
                    task.implementation_run,
                    task.mission.clone(),
                    usize::from(Column::of(task.status) != Column::Done),
                )),
            }
        }
        let chip = |id: SharedString, label: String, count: Option<usize>, active: bool| {
            div()
                .id(id)
                .role(Role::Tab)
                .h_flex()
                .gap(px(6.0))
                .items_center()
                .max_w(px(280.0))
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(if active {
                    theme::stroke(theme::accent())
                } else {
                    theme::hairline()
                })
                .bg(if active {
                    theme::tint(theme::accent())
                } else {
                    theme::panel_raised()
                })
                .text_size(px(11.0))
                .text_color(if active {
                    theme::text_primary()
                } else {
                    theme::text_muted()
                })
                .cursor_pointer()
                .child(div().truncate().child(label))
                .children(count.filter(|n| *n > 0).map(|n| {
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::text_dim())
                        .child(format!("{n} open"))
                }))
        };
        let started: std::collections::HashMap<RunId, String> = self
            .state
            .read(cx)
            .runs
            .iter()
            .map(|r| {
                (
                    r.run_id,
                    r.started_at
                        .with_timezone(&chrono::Local)
                        .format("%b %d %H:%M")
                        .to_string(),
                )
            })
            .collect();
        let mut row = div().h_flex().flex_wrap().gap(px(6.0)).child(
            chip(
                "backlog-mission-all".into(),
                "All missions".into(),
                None,
                self.mission_filter.is_none(),
            )
            .on_click(cx.listener(|this, _e, _w, cx| {
                this.mission_filter = None;
                cx.notify();
            })),
        );
        for (run, title, open) in missions {
            row = row.child(
                chip(
                    SharedString::from(format!("backlog-mission-{run}")),
                    match started.get(&run) {
                        Some(when) => format!("{when} · {}", ui::headline(&title, 34)),
                        None => ui::headline(&title, 42),
                    },
                    Some(open),
                    self.mission_filter == Some(run),
                )
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.mission_filter = Some(run);
                    cx.notify();
                })),
            );
        }
        row
    }
}

impl Render for BacklogScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all_tasks = self.tasks.clone();
        let tasks: Vec<BacklogTask> = all_tasks
            .iter()
            .filter(|t| {
                self.mission_filter
                    .is_none_or(|run| t.implementation_run == run)
            })
            .cloned()
            .collect();
        let mut by_column: Vec<(Column, Vec<&BacklogTask>)> =
            Column::ALL.iter().map(|c| (*c, Vec::new())).collect();
        for task in &tasks {
            let column = Column::of(task.status);
            if let Some((_, list)) = by_column.iter_mut().find(|(c, _)| *c == column) {
                list.push(task);
            }
        }
        // Ready before blocked; left-over work before work a live mission
        // will still pick up.
        for (column, list) in &mut by_column {
            if *column == Column::Todo {
                list.sort_by_key(|t| (!t.blocked_by.is_empty(), !t.mission_ended));
            }
        }
        let open_count = tasks
            .iter()
            .filter(|t| !matches!(Column::of(t.status), Column::Done))
            .count();
        let subtitle = if all_tasks.is_empty() {
            None
        } else {
            let scope = if self.mission_filter.is_some() {
                "in this mission"
            } else {
                "across your missions"
            };
            Some(SharedString::from(format!(
                "{} tasks {scope} · {open_count} not done yet",
                tasks.len()
            )))
        };

        let header = ui::page_header(
            "Backlog",
            subtitle,
            Button::new("backlog-new")
                .primary()
                .small()
                .icon(IconName::Plus)
                .label("New mission")
                .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(BacklogAction::NewTask))),
        );

        let body: AnyElement = if all_tasks.is_empty() && !self.loading {
            ui::panel()
                .child(ui::empty_state(
                    "▦",
                    "No tasks yet",
                    "Tasks appear here once a mission's plan is approved — every milestone task, \
                     plus anything the agents find along the way.",
                ))
                .into_any_element()
        } else {
            let columns: Vec<Div> = by_column
                .iter()
                .map(|(c, list)| self.render_column(*c, list, cx))
                .collect();
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .gap(px(12.0))
                .children(columns)
                .into_any_element()
        };

        div()
            .size_full()
            .v_flex()
            .gap(px(16.0))
            .bg(theme::background())
            .px(px(24.0))
            .pt(px(22.0))
            .pb(px(20.0))
            .child(header)
            .when(!all_tasks.is_empty(), |el| {
                el.child(self.render_mission_filter(cx))
            })
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{Column, restart_prompt};
    use crate::backlog_source::BacklogTask;
    use surge_core::RunId;
    use surge_core::roadmap::RoadmapStatus;

    #[test]
    fn columns_follow_the_ledger_status() {
        assert_eq!(Column::of(RoadmapStatus::Pending), Column::Todo);
        assert_eq!(
            Column::of(RoadmapStatus::ReadyForVerification),
            Column::InProgress
        );
        assert_eq!(
            Column::of(RoadmapStatus::FailedVerification),
            Column::NeedsYou
        );
        assert_eq!(Column::of(RoadmapStatus::Skipped), Column::Done);
    }

    #[test]
    fn board_renders_real_tasks_in_their_columns() {
        use gpui_kit::{AppContext as _, TestAppContext};
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let run = RunId::new();
        let task = |id: &str, status: RoadmapStatus, verified: bool| BacklogTask {
            id: id.into(),
            title: format!("Task {id}"),
            description: None,
            acceptance: Vec::new(),
            milestone: Some("Core".into()),
            mission: "Timer".into(),
            implementation_run: run,
            status,
            verified,
            discovered_from: None,
            blocked_by: Vec::new(),
            mission_ended: true,
        };
        let (view, window) = cx.add_window_view(|_window, cx| {
            let state = cx.new(|_| crate::app_state::AppState::new());
            let mut screen = super::BacklogScreen::new(state, cx);
            screen.tasks = vec![
                task("a", RoadmapStatus::Completed, true),
                task("b", RoadmapStatus::Pending, false),
                task("c", RoadmapStatus::FailedVerification, false),
            ];
            screen
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        view.update(window, |screen, _| {
            let columns: Vec<Column> = screen.tasks.iter().map(|t| Column::of(t.status)).collect();
            assert_eq!(columns, [Column::Done, Column::Todo, Column::NeedsYou]);
        });
    }

    #[test]
    fn restart_prompt_carries_the_task_and_its_origin() {
        let task = BacklogTask {
            id: "t1".into(),
            title: "Add dark mode".into(),
            description: Some("Follow the OS setting.".into()),
            acceptance: vec!["Toggle persists".into()],
            milestone: Some("UI".into()),
            mission: "Pomodoro timer".into(),
            implementation_run: RunId::new(),
            status: RoadmapStatus::Pending,
            verified: false,
            discovered_from: None,
            blocked_by: Vec::new(),
            mission_ended: true,
        };
        let prompt = restart_prompt(&task);
        assert!(prompt.starts_with("Add dark mode\n\nFollow the OS setting."));
        assert!(prompt.contains("Done when:\n- Toggle persists"));
        assert!(prompt.ends_with("(Picked up from the backlog of: Pomodoro timer)"));
    }
}
