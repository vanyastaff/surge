use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;

use crate::project::RecentProjects;
use crate::router::Screen;
use crate::theme;

/// Events emitted by the TopBar.
#[derive(Clone, PartialEq)]
pub enum TopBarEvent {
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
        cx.notify();
    }

    fn render_breadcrumb(&self) -> Div {
        div().h_flex().gap_1().items_center().child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
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

        div()
            .relative()
            .h_flex()
            .w_full()
            .h(px(40.0))
            .px_4()
            .items_center()
            .justify_between()
            .bg(theme::sidebar_bg())
            .border_b_1()
            .border_color(theme::surface())
            // Left: project name (clickable) + branch
            .child(
                div()
                    .relative()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("project-switcher")
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .cursor_pointer()
                            .hover(|s: StyleRefinement| s.text_color(theme::primary()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_switcher(cx);
                            }))
                            .child(format!("{} ▾", self.project_name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::primary().opacity(0.15))
                            .text_color(theme::primary())
                            .child(self.branch_name.clone()),
                    )
                    .when(switcher_open, |el: Div| {
                        el.child(self.render_switcher_dropdown(cx))
                    }),
            )
            // Center: breadcrumb
            .child(self.render_breadcrumb())
            // Right: agent dots + search hint
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .child(self.render_agent_dots())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.5))
                            .child("Ctrl+K".to_string()),
                    ),
            )
    }
}
