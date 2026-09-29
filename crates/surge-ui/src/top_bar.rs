//! Top bar: which project you are in, where in it, and search.
//!
//! Left: the project switcher (name + chevrons; opens the recent list) and
//! the current screen. Right: the checked-out git branch — read from the
//! repository, never assumed — and a clickable search / command button.

use std::path::PathBuf;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::StyledExt;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::project::{RecentProject, RecentProjects};
use crate::router::Screen;
use crate::theme;
use crate::ui;

/// Events emitted by the TopBar.
#[derive(Clone, PartialEq)]
pub enum TopBarEvent {
    /// Project menu is opening; native child views must yield to the overlay.
    ProjectSwitcherOpened,
    /// User clicked project name — wants to switch project.
    SwitchProject(std::path::PathBuf),
    /// User wants to open another project.
    OpenOther,
    /// User wants a new project.
    NewProject,
    /// User clicked search — open the command palette.
    OpenPalette,
}

impl EventEmitter<TopBarEvent> for TopBar {}

/// Top bar / header component showing project context and global actions.
pub struct TopBar {
    project_name: String,
    project_path: Option<PathBuf>,
    /// Read once when the project opens; `None` hides the chip.
    branch_name: Option<String>,
    active_screen: Screen,
    /// Recent projects, loaded when the switcher opens (not per frame).
    switcher: Option<Vec<RecentProject>>,
}

impl TopBar {
    pub fn new(
        project_name: &str,
        project_path: Option<PathBuf>,
        active_screen: Screen,
        _cx: &mut Context<Self>,
    ) -> Self {
        let branch_name = project_path.as_deref().and_then(ui::current_branch);
        Self {
            project_name: project_name.to_string(),
            project_path,
            branch_name,
            active_screen,
            switcher: None,
        }
    }

    pub fn set_screen(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active_screen = screen;
        cx.notify();
    }

    pub fn toggle_switcher(&mut self, cx: &mut Context<Self>) {
        if self.switcher.is_some() {
            self.switcher = None;
        } else {
            self.switcher = Some(
                RecentProjects::load()
                    .sorted()
                    .into_iter()
                    .cloned()
                    .collect(),
            );
            cx.emit(TopBarEvent::ProjectSwitcherOpened);
        }
        cx.notify();
    }

    fn close_switcher(&mut self, cx: &mut Context<Self>) {
        if self.switcher.take().is_some() {
            cx.notify();
        }
    }

    fn render_breadcrumb(&self) -> Div {
        div()
            .h_flex()
            .gap(px(8.0))
            .items_center()
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme::hairline_strong())
                    .child("/"),
            )
            .child(
                Icon::new(self.active_screen.icon())
                    .size(px(13.0))
                    .text_color(theme::text_muted()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .child(self.active_screen.label()),
            )
    }

    fn menu_item(id: &'static str, label: &'static str, hint: Option<String>) -> Stateful<Div> {
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .h_flex()
            .justify_between()
            .items_center()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(ui::R_CONTROL))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(theme::text_muted())
            .hover(|s: StyleRefinement| s.bg(theme::surface()).text_color(theme::text_primary()))
            .child(label)
            .children(hint.map(ui::kbd))
    }

    fn render_switcher_dropdown(&self, projects: &[RecentProject], cx: &mut Context<Self>) -> Div {
        let items: Vec<Stateful<Div>> = projects
            .iter()
            .map(|p| {
                let path = p.path.clone();
                let name = p.name.clone();
                let current = self.project_path.as_deref() == Some(p.path.as_path());
                let missing = !p.path.is_dir();

                div()
                    .id(SharedString::from(format!("switch-{}", p.path.display())))
                    .role(Role::Button)
                    .aria_label(format!("Open project {name}"))
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .px(px(10.0))
                    .py(px(7.0))
                    .cursor_pointer()
                    .rounded(px(ui::R_CONTROL))
                    .when(current, |el| el.bg(theme::surface()))
                    .hover(|s: StyleRefinement| s.bg(theme::surface()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.switcher = None;
                        cx.emit(TopBarEvent::SwitchProject(path.clone()));
                    }))
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if missing {
                                        theme::text_muted()
                                    } else {
                                        theme::text_primary()
                                    })
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_dim())
                                    .truncate()
                                    .child(ui::abbreviate_home(&p.path)),
                            ),
                    )
                    .when(missing, |el| {
                        el.child(ui::role_badge("missing", theme::Semantic::Failure))
                    })
                    .when(current, |el| {
                        el.child(
                            Icon::new(IconName::Check)
                                .size(px(13.0))
                                .text_color(theme::accent()),
                        )
                    })
            })
            .collect();

        ui::panel()
            .absolute()
            .top(px(38.0))
            .left_0()
            .w(px(340.0))
            .v_flex()
            .p(px(5.0))
            .gap(px(1.0))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| this.close_switcher(cx)))
            .child(
                div()
                    .px(px(10.0))
                    .pt(px(6.0))
                    .pb(px(4.0))
                    .child(ui::section_label("Recent projects")),
            )
            .child(
                div()
                    .id("switcher-list")
                    .v_flex()
                    .gap(px(1.0))
                    .max_h(px(320.0))
                    .overflow_y_scroll()
                    .children(items),
            )
            .child(div().my(px(4.0)).h(px(1.0)).bg(theme::hairline()))
            .child(
                Self::menu_item(
                    "switch-open-other",
                    "Open project…",
                    Some(ui::shortcut_label("Ctrl+O")),
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.switcher = None;
                    cx.emit(TopBarEvent::OpenOther);
                })),
            )
            .child(
                Self::menu_item("switch-new-project", "New app…", None).on_click(cx.listener(
                    |this, _event, _window, cx| {
                        this.switcher = None;
                        cx.emit(TopBarEvent::NewProject);
                    },
                )),
            )
    }

    fn render_search(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("topbar-search")
            .role(Role::Button)
            .aria_label("Search and commands")
            .h_flex()
            .gap(px(8.0))
            .items_center()
            .h(px(28.0))
            .pl(px(10.0))
            .pr(px(5.0))
            .min_w(px(180.0))
            .rounded(px(ui::R_CONTROL))
            .bg(theme::panel_deep())
            .border_1()
            .border_color(theme::hairline())
            .text_size(px(11.5))
            .text_color(theme::text_dim())
            .cursor_pointer()
            .hover(|s: StyleRefinement| {
                s.border_color(theme::hairline_strong())
                    .text_color(theme::text_muted())
            })
            .on_click(cx.listener(|_this, _event, _window, cx| cx.emit(TopBarEvent::OpenPalette)))
            .child(Icon::new(IconName::Search).size(px(12.0)))
            .child(div().flex_1().child("Search"))
            .child(ui::kbd(ui::shortcut_label("Ctrl+K")))
    }
}

impl Render for TopBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let switcher = self.switcher.clone();
        let open = switcher.is_some();

        div()
            .relative()
            .h_flex()
            .w_full()
            .h(px(46.0))
            .gap(px(10.0))
            .px(px(12.0))
            .items_center()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            // Left: project switcher
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id("project-switcher")
                            .role(Role::Button)
                            .aria_label("Switch project")
                            .h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .h(px(30.0))
                            .px(px(10.0))
                            .rounded(px(ui::R_CONTROL))
                            .border_1()
                            .border_color(if open {
                                theme::hairline_strong()
                            } else {
                                transparent_black()
                            })
                            .when(open, |el| el.bg(theme::surface()))
                            .text_size(px(12.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .cursor_pointer()
                            .hover(|s: StyleRefinement| s.bg(theme::surface()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_switcher(cx);
                            }))
                            .child(
                                div()
                                    .size(px(18.0))
                                    .rounded(px(ui::R_PRECISE + 1.0))
                                    .bg(theme::tint(theme::accent()))
                                    .border_1()
                                    .border_color(theme::stroke(theme::accent()))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(10.0))
                                    .text_color(theme::accent())
                                    .child(
                                        self.project_name
                                            .chars()
                                            .next()
                                            .map(|c| c.to_uppercase().to_string())
                                            .unwrap_or_default(),
                                    ),
                            )
                            .child(self.project_name.clone())
                            .child(
                                Icon::new(IconName::ChevronsUpDown)
                                    .size(px(12.0))
                                    .text_color(theme::text_dim()),
                            ),
                    )
                    // Deferred: painted after the screen below, which would
                    // otherwise cover the menu.
                    .when_some(switcher, |el, projects| {
                        el.child(deferred(self.render_switcher_dropdown(&projects, cx)).with_priority(1))
                    }),
            )
            .child(self.render_breadcrumb())
            .child(div().flex_1())
            // Right: branch + search
            .when_some(self.branch_name.clone(), |el, branch| {
                el.child(
                    div()
                        .h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .px(px(8.0))
                        .h(px(24.0))
                        .rounded(px(999.0))
                        .border_1()
                        .border_color(theme::hairline())
                        .text_size(px(11.0))
                        .text_color(theme::text_muted())
                        .child(Icon::new(Lucide::GitBranch).size(px(11.0)))
                        .child(branch),
                )
            })
            .child(self.render_search(cx))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn opening_project_switcher_notifies_parent_before_project_selection() {
        use gpui_kit::{AppContext as _, TestAppContext};
        use std::{cell::Cell, rc::Rc};
        let mut cx = TestAppContext::single();
        let opened = Rc::new(Cell::new(0));
        let bar = cx.new(|cx| super::TopBar::new("project", None, crate::router::Screen::Runs, cx));
        let count = opened.clone();
        cx.update(|cx| {
            cx.subscribe(&bar, move |_, event, _| {
                if matches!(event, super::TopBarEvent::ProjectSwitcherOpened) {
                    count.set(count.get() + 1);
                }
            })
            .detach();
        });
        bar.update(&mut cx, |bar, cx| bar.toggle_switcher(cx));
        assert_eq!(opened.get(), 1);
        bar.update(&mut cx, |bar, cx| bar.toggle_switcher(cx));
        assert_eq!(opened.get(), 1);
    }

    #[test]
    fn branch_is_read_from_the_repository_not_assumed() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(crate::ui::current_branch(dir.path()), None);
        let repo = git2::Repository::init(dir.path()).unwrap();
        // Unborn HEAD: no commit yet, so no branch to show.
        assert_eq!(crate::ui::current_branch(dir.path()), None);
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("refs/heads/trunk"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        repo.set_head("refs/heads/trunk").unwrap();
        assert_eq!(
            crate::ui::current_branch(dir.path()).as_deref(),
            Some("trunk")
        );
    }
}
