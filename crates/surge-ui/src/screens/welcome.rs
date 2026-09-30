//! Start page: what Surge is, how a run moves, and where you left off.
//!
//! Left: the product in one line, the two ways in, and the run pipeline
//! drawn in the app's semantic colors (plan · agent · verified · you), so
//! the first screen shows the vocabulary every later screen uses instead
//! of explaining it. Right: recent projects.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::project::{RecentProject, RecentProjects};
use crate::theme::{self, Semantic};
use crate::ui;

/// Events emitted by the Welcome screen.
#[derive(Clone, PartialEq)]
pub enum WelcomeEvent {
    /// User selected a project to open.
    OpenProject(PathBuf),
    /// User wants to browse for a project.
    BrowseProject,
    /// User wants to start a new application in an empty folder.
    NewProject,
    /// User removed a project from the list.
    RemoveProject(PathBuf),
    /// User toggled pin on a project.
    TogglePin(PathBuf),
}

impl EventEmitter<WelcomeEvent> for WelcomeScreen {}

/// The Welcome / Project Picker screen shown on startup.
pub struct WelcomeScreen {
    recent: RecentProjects,
    /// Recent entries whose folder is gone, checked on load (not per frame).
    missing: HashSet<PathBuf>,
    /// Show every remembered project instead of the most recent few.
    show_all: bool,
}

/// Unpinned projects listed before "Show all"; pinned ones always show.
const RECENT_VISIBLE: usize = 6;

/// One stage of the run pipeline shown on the start page.
struct Stage {
    role: Semantic,
    icon: Lucide,
    title: &'static str,
    detail: &'static str,
}

const STAGES: [Stage; 4] = [
    Stage {
        role: Semantic::Plan,
        icon: Lucide::FileText,
        title: "Plan",
        detail: "spec · roadmap",
    },
    Stage {
        role: Semantic::Agent,
        icon: Lucide::Bot,
        title: "Build",
        detail: "agent · worktree",
    },
    Stage {
        role: Semantic::Verified,
        icon: Lucide::ShieldCheck,
        title: "Verify",
        detail: "tests · evidence",
    },
    Stage {
        role: Semantic::You,
        icon: Lucide::UserCheck,
        title: "You",
        detail: "approve · merge",
    },
];

impl WelcomeScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let recent = RecentProjects::load();
        let missing = missing_folders(&recent);
        Self {
            recent,
            missing,
            show_all: false,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.recent = RecentProjects::load();
        self.missing = missing_folders(&self.recent);
        cx.notify();
    }

    fn render_brand(&self) -> Div {
        div()
            .h_flex()
            .gap(px(10.0))
            .items_center()
            .child(ui::brand_mark(22.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child("SURGE"),
            )
            .child(ui::pill(
                concat!("v", env!("CARGO_PKG_VERSION")),
                theme::text_muted(),
                theme::panel_raised(),
            ))
    }

    fn render_pitch(&self) -> Div {
        div()
            .v_flex()
            .gap(px(14.0))
            .child(
                div()
                    .v_flex()
                    .text_size(px(30.0))
                    .line_height(px(38.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child("Delegate a task.")
                    .child(div().text_color(theme::accent()).child("Review the result.")),
            )
            .child(
                div()
                    .max_w(px(470.0))
                    .text_size(px(16.0))
                    .line_height(px(24.0))
                    .text_color(theme::text_muted())
                    .child("Use your coding agents to plan, implement, and check work in your project. Review plans, follow progress, and make decisions."),
            )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .gap(px(10.0))
            // Open an existing project → native directory picker.
            .child(
                Button::new("open-project")
                    .primary()
                    .large()
                    .icon(IconName::FolderOpen)
                    .label("Open project")
                    .accessibility_id("open-project")
                    .on_click(cx.listener(|_this, _event, _window, cx| {
                        cx.emit(WelcomeEvent::BrowseProject);
                    })),
            )
            // New app → empty folder picker + repository initialization.
            .child(
                Button::new("new-project")
                    .outline()
                    .large()
                    .icon(Lucide::FolderPlus)
                    .label("New app")
                    .accessibility_id("new-project")
                    .on_click(cx.listener(|_this, _event, _window, cx| {
                        cx.emit(WelcomeEvent::NewProject);
                    })),
            )
            .child(
                div()
                    .h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .ml(px(6.0))
                    .text_size(px(10.0))
                    .text_color(theme::text_dim())
                    .child(ui::kbd(ui::shortcut_label("Ctrl+N")))
                    .child("new")
                    .child(ui::kbd(ui::shortcut_label("Ctrl+O")))
                    .child("open"),
            )
    }

    /// The run pipeline as four semantic nodes with connectors.
    fn render_pipeline(&self) -> Div {
        let mut row = div().h_flex().items_center();
        for (i, stage) in STAGES.iter().enumerate() {
            if i > 0 {
                row = row.child(
                    div()
                        .h_flex()
                        .items_center()
                        .flex_none()
                        .child(div().w(px(12.0)).h(px(1.0)).bg(theme::graph_line()))
                        .child(
                            Icon::new(Lucide::ArrowRight)
                                .size(px(11.0))
                                .text_color(theme::graph_line()),
                        ),
                );
            }
            let color = stage.role.color();
            row = row.child(
                ui::node_card(color)
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(10.0))
                    .py(px(9.0))
                    .v_flex()
                    .gap(px(5.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(Icon::new(stage.icon).size(px(12.0)).text_color(color))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(stage.title),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .truncate()
                            .child(stage.detail),
                    ),
            );
        }

        div()
            .v_flex()
            .gap(px(10.0))
            .child(ui::section_label("How a run moves"))
            .child(row)
    }

    fn render_project_row(&self, project: &RecentProject, cx: &mut Context<Self>) -> Stateful<Div> {
        let path_open = project.path.clone();
        let path_pin = project.path.clone();
        let path_remove = project.path.clone();
        let name = project.name.clone();
        let display_path = ui::abbreviate_home(&project.path);
        let missing = self.missing.contains(&project.path);
        let pinned = project.pinned;
        let active_tasks = project.active_tasks;
        let age = ui::relative_age(&project.last_opened);
        let group = SharedString::from(format!("project-row-{}", project.path.display()));
        let has_work = active_tasks > 0 && !missing;
        let glyph_color = if missing {
            theme::error()
        } else if has_work {
            theme::accent()
        } else {
            theme::text_muted()
        };

        div()
            .id(SharedString::from(format!("project-{}", project.path.display())))
            .group(group.clone())
            .role(Role::Button)
            .aria_label(if missing {
                format!("{name} — folder not found")
            } else {
                format!("Open project {name}")
            })
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(12.0))
            .py(px(10.0))
            .rounded(px(ui::R_CONTROL + 2.0))
            .border_1()
            .border_color(transparent_black())
            .cursor_pointer()
            .hover(|s: StyleRefinement| {
                s.bg(theme::surface()).border_color(theme::hairline_strong())
            })
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(WelcomeEvent::OpenProject(path_open.clone()));
            }))
            // Folder glyph in a node-style tile; cyan when work is in flight.
            .child(
                div()
                    .size(px(32.0))
                    .flex_none()
                    .rounded(px(ui::R_CONTROL + 2.0))
                    .border_1()
                    .border_color(theme::stroke(glyph_color).opacity(0.5))
                    .bg(theme::tint(glyph_color))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(if missing { IconName::TriangleAlert } else { IconName::Folder })
                            .size(px(14.0))
                            .text_color(glyph_color),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap(px(3.0))
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(7.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if missing {
                                        theme::text_muted()
                                    } else {
                                        theme::text_primary()
                                    })
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .when(pinned, |el| {
                                el.child(
                                    Icon::new(Lucide::Pin)
                                        .size(px(11.0))
                                        .text_color(theme::text_muted()),
                                )
                            })
                            .when(has_work, |el| {
                                el.child(ui::role_badge(format!("{active_tasks} active"), Semantic::Agent))
                            })
                            .when(missing, |el| el.child(ui::role_badge("missing", Semantic::Failure))),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::text_dim())
                            .truncate()
                            .child(display_path),
                    ),
            )
            // Right edge: the age at rest, the row's actions on hover — one
            // slot, so nothing is reserved for controls you cannot see.
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(56.0))
                    .h(px(26.0))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .text_size(px(10.5))
                            .text_color(theme::text_dim())
                            .group_hover(group.clone(), |s| s.opacity(0.0))
                            .children(age),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h_flex()
                            .gap(px(2.0))
                            .opacity(0.0)
                            .group_hover(group, |s| s.opacity(1.0))
                            .child(self.row_action(
                                format!("pin-{}", path_pin.display()),
                                if pinned { Lucide::PinOff } else { Lucide::Pin },
                                if pinned { "Unpin project" } else { "Pin project" },
                                theme::text_primary(),
                                cx.listener(move |_this, _event: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    cx.emit(WelcomeEvent::TogglePin(path_pin.clone()));
                                }),
                            ))
                            .child(self.row_action(
                                format!("rm-{}", path_remove.display()),
                                Lucide::X,
                                "Remove from recent projects",
                                theme::error(),
                                cx.listener(move |_this, _event: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    cx.emit(WelcomeEvent::RemoveProject(path_remove.clone()));
                                }),
                            )),
                    ),
            )
    }

    fn row_action(
        &self,
        id: String,
        icon: Lucide,
        label: &'static str,
        hover_color: Hsla,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .role(Role::Button)
            .aria_label(label)
            .size(px(26.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(ui::R_CONTROL))
            .text_color(theme::text_muted())
            .cursor_pointer()
            .hover(move |s: StyleRefinement| s.bg(theme::panel_raised()).text_color(hover_color))
            .on_click(on_click)
            .child(Icon::new(icon).size(px(13.0)))
    }

    fn render_recent(&self, cx: &mut Context<Self>) -> Div {
        let sorted = self.recent.sorted();
        let count = sorted.len();
        let unpinned = sorted.iter().filter(|p| !p.pinned).count();
        let hidden = if self.show_all {
            0
        } else {
            unpinned.saturating_sub(RECENT_VISIBLE)
        };
        let mut shown_unpinned = 0;
        let rows: Vec<Stateful<Div>> = sorted
            .iter()
            .filter(|p| {
                if p.pinned || self.show_all {
                    return true;
                }
                shown_unpinned += 1;
                shown_unpinned <= RECENT_VISIBLE
            })
            .map(|p| self.render_project_row(p, cx))
            .collect();
        let can_collapse = self.show_all && unpinned > RECENT_VISIBLE;

        ui::panel()
            .v_flex()
            .w(px(420.0))
            .flex_shrink(1.0)
            .min_w(px(340.0))
            .max_h(px(560.0))
            .overflow_hidden()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .px(px(16.0))
                    .py(px(12.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(ui::section_label("Recent projects"))
                            .when(count > 0, |el| {
                                el.child(ui::pill(
                                    count.to_string(),
                                    theme::text_muted(),
                                    theme::panel_deep(),
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .id("recent-projects")
                    .v_flex()
                    .gap(px(2.0))
                    .p(px(6.0))
                    .flex_1()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |el| {
                        el.child(ui::empty_state(
                            "◎",
                            "No projects yet",
                            "Open a project to delegate your first task, or start in an empty folder. Your projects will appear here next time.",
                        ))
                    })
                    .children(rows),
            )
            .when(hidden > 0 || can_collapse, |el| {
                el.child(
                    div()
                        .id("recent-show-all")
                        .role(Role::Button)
                        .h_flex()
                        .justify_center()
                        .py(px(9.0))
                        .border_t_1()
                        .border_color(theme::hairline())
                        .text_size(px(11.0))
                        .text_color(theme::text_muted())
                        .cursor_pointer()
                        .hover(|s: StyleRefinement| {
                            s.bg(theme::surface()).text_color(theme::text_primary())
                        })
                        .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.show_all = !this.show_all;
                            cx.notify();
                        }))
                        .child(if hidden > 0 {
                            format!("Show all · {hidden} more")
                        } else {
                            "Show fewer".to_string()
                        }),
                )
            })
    }
}

impl Render for WelcomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(theme::background())
            .overflow_hidden()
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(48.0))
                    .py(px(32.0))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(48.0))
                            .max_w(px(1120.0))
                            .w_full()
                            .child(
                                div()
                                    .v_flex()
                                    .gap(px(28.0))
                                    .flex_1()
                                    .min_w(px(380.0))
                                    .max_w(px(560.0))
                                    .child(self.render_brand())
                                    .child(self.render_pitch())
                                    .child(self.render_actions(cx))
                                    .child(self.render_pipeline()),
                            )
                            .child(self.render_recent(cx)),
                    ),
            )
    }
}

/// Recent entries whose folder no longer exists.
fn missing_folders(recent: &RecentProjects) -> HashSet<PathBuf> {
    recent
        .projects
        .iter()
        .filter(|p| !p.path.is_dir())
        .map(|p| p.path.clone())
        .collect()
}
