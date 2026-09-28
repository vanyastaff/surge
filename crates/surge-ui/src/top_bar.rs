use gpui_kit::component::StyledExt;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::project::RecentProjects;
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
}

impl EventEmitter<TopBarEvent> for TopBar {}

/// Top bar / header component showing project context and global actions.
pub struct TopBar {
    project_name: String,
    branch_name: String,
    active_screen: Screen,
    agent_statuses: Vec<(String, bool)>,
    switcher_open: bool,
}

impl TopBar {
    pub fn new(project_name: &str, active_screen: Screen, _cx: &mut Context<Self>) -> Self {
        Self {
            project_name: project_name.to_string(),
            branch_name: "main".to_string(),
            active_screen,
            agent_statuses: vec![],
            switcher_open: false,
        }
    }

    pub fn set_screen(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active_screen = screen;
        cx.notify();
    }

    pub fn set_project(&mut self, name: &str, cx: &mut Context<Self>) {
        self.project_name = name.to_string();
        cx.notify();
    }

    pub fn set_agents(&mut self, agents: Vec<(String, bool)>, cx: &mut Context<Self>) {
        self.agent_statuses = agents;
        cx.notify();
    }

    pub fn toggle_switcher(&mut self, cx: &mut Context<Self>) {
        self.switcher_open = !self.switcher_open;
        if self.switcher_open {
            cx.emit(TopBarEvent::ProjectSwitcherOpened);
        }
        cx.notify();
    }

    fn render_breadcrumb(&self) -> Div {
        div()
            .h_flex()
            .gap(px(6.0))
            .items_center()
            .text_color(theme::text_muted())
            .child(
                Icon::new(IconName::ChevronRight)
                    .size(px(11.0))
                    .text_color(theme::text_muted().opacity(0.6)),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.active_screen.label().to_string()),
            )
    }

    fn render_agent_dots(&self) -> Div {
        let dots: Vec<Div> = self
            .agent_statuses
            .iter()
            .map(|(_name, connected)| {
                let color = if *connected {
                    theme::success()
                } else {
                    theme::error()
                };
                div().w(px(8.0)).h(px(8.0)).rounded_full().bg(color)
            })
            .collect();

        div().h_flex().gap_1().children(dots)
    }

    fn render_switcher_dropdown(&self, cx: &mut Context<Self>) -> Div {
        let recent = RecentProjects::load();
        let projects = recent.sorted();

        let items: Vec<Stateful<Div>> = projects
            .iter()
            .map(|p| {
                let path = p.path.clone();
                let name = p.name.clone();
                let display_path = p.path.display().to_string();

                div()
                    .id(SharedString::from(format!("switch-{display_path}")))
                    .role(Role::Button)
                    .aria_label(format!("Open project {name}"))
                    .h_flex()
                    .justify_between()
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .hover(|s: StyleRefinement| s.bg(theme::primary().opacity(0.1)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.switcher_open = false;
                        cx.emit(TopBarEvent::SwitchProject(path.clone()));
                    }))
                    .child(
                        div()
                            .v_flex()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_primary())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(display_path),
                            ),
                    )
            })
            .collect();

        div()
            .absolute()
            .top(px(36.0))
            .left_0()
            .w(px(320.0))
            .v_flex()
            .bg(theme::surface())
            .rounded_lg()
            .border_1()
            .border_color(theme::text_muted().opacity(0.2))
            .shadow_lg()
            .p_1()
            .gap_0p5()
            .children(items)
            .child(
                div()
                    .border_t_1()
                    .border_color(theme::text_muted().opacity(0.1))
                    .mt_1()
                    .pt_1()
                    .child(
                        div()
                            .id("switch-open-other")
                            .role(Role::Button)
                            .aria_label("Open another project")
                            .px_3()
                            .py(px(6.0))
                            .cursor_pointer()
                            .rounded_md()
                            .text_sm()
                            .text_color(theme::text_muted())
                            .hover(|s: StyleRefinement| s.bg(theme::primary().opacity(0.1)))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.switcher_open = false;
                                cx.emit(TopBarEvent::OpenOther);
                            }))
                            .child("Open Other...".to_string()),
                    )
                    .child(
                        div()
                            .id("switch-new-project")
                            .role(Role::Button)
                            .aria_label("New project")
                            .px_3()
                            .py(px(6.0))
                            .cursor_pointer()
                            .rounded_md()
                            .text_sm()
                            .text_color(theme::text_muted())
                            .hover(|s: StyleRefinement| s.bg(theme::primary().opacity(0.1)))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.switcher_open = false;
                                cx.emit(TopBarEvent::NewProject);
                            }))
                            .child("New Project...".to_string()),
                    ),
            )
    }
}

impl Render for TopBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let switcher_open = self.switcher_open;
        let agents_online = self.agent_statuses.iter().filter(|(_, up)| *up).count();

        div()
            .relative()
            .h_flex()
            .w_full()
            .h(px(44.0))
            .gap(px(12.0))
            .px(px(16.0))
            .items_center()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            // Left: repo chip (click to switch project)
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id("project-switcher")
                            .role(Role::Button)
                            .aria_label("Switch project")
                            .h_flex()
                            .gap(px(7.0))
                            .items_center()
                            .px(px(11.0))
                            .py(px(5.0))
                            .rounded_lg()
                            .bg(theme::panel_raised())
                            .border_1()
                            .border_color(theme::hairline_strong())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .cursor_pointer()
                            .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.5)))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_switcher(cx);
                            }))
                            .child(self.project_name.clone())
                            .child(
                                div()
                                    .text_color(theme::text_muted())
                                    .child("▾"),
                            ),
                    )
                    .when(switcher_open, |el: Div| {
                        el.child(self.render_switcher_dropdown(cx))
                    }),
            )
            // Breadcrumb: branch → screen
            .child(self.render_breadcrumb())
            // Spacer
            .child(div().flex_1())
            // Right: context info + ⌘K
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(self.render_agent_dots())
                    .child(ui::meta(format!(
                        "{}  ·  {} connected",
                        self.branch_name, agents_online
                    )))
                    .child(ui::kbd("⌘K")),
            )
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
        let bar = cx.new(|cx| super::TopBar::new("project", crate::router::Screen::Runs, cx));
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
}
