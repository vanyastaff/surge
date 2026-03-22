use gpui::*;
use gpui_component::StyledExt;

use crate::router::Screen;
use crate::theme;

/// Top bar / header component showing project context and global actions.
pub struct TopBar {
    project_name: String,
    branch_name: String,
    active_screen: Screen,
    agent_statuses: Vec<(String, bool)>, // (name, connected)
}

impl TopBar {
    pub fn new(
        project_name: &str,
        active_screen: Screen,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            project_name: project_name.to_string(),
            branch_name: "main".to_string(),
            active_screen,
            agent_statuses: vec![],
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

    fn render_breadcrumb(&self) -> Div {
        div()
            .h_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(self.active_screen.label().to_string()),
            )
    }

    fn render_agent_dots(&self) -> Div {
        let dots: Vec<Div> = self
            .agent_statuses
            .iter()
            .map(|(name, connected)| {
                let color = if *connected {
                    theme::SUCCESS
                } else {
                    theme::ERROR
                };
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(color)
            })
            .collect();

        div().h_flex().gap_1().children(dots)
    }
}

impl Render for TopBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_flex()
            .w_full()
            .h(px(40.0))
            .px_4()
            .items_center()
            .justify_between()
            .bg(theme::SIDEBAR_BG)
            .border_b_1()
            .border_color(theme::SURFACE)
            // Left: project name + branch
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .cursor_pointer()
                            .child(self.project_name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::PRIMARY.opacity(0.15))
                            .text_color(theme::PRIMARY)
                            .child(self.branch_name.clone()),
                    ),
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
                            .text_color(theme::TEXT_MUTED.opacity(0.5))
                            .child("Ctrl+K".to_string()),
                    ),
            )
    }
}
