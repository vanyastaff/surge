use gpui::*;
use gpui_component::button::Button;
use gpui_component::WindowExt as _;
use gpui_component::StyledExt as _;

use crate::actions::*;
use crate::command_palette::{CommandPalette, CommandSelected};
use crate::notifications::SurgeNotification;
use crate::router::Screen;
use crate::sidebar::{AppSidebar, NavigateTo, ToggleSidebar};
use crate::theme;

/// Root application view.
pub struct SurgeApp {
    active_screen: Screen,
    sidebar_collapsed: bool,
    sidebar: Entity<AppSidebar>,
    command_palette_open: bool,
    command_palette: Option<Entity<CommandPalette>>,
}

impl SurgeApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let active_screen = Screen::Dashboard;
        let sidebar = cx.new(|cx| AppSidebar::new(active_screen, false, cx));

        cx.subscribe(&sidebar, |this: &mut Self, _sidebar, event: &NavigateTo, cx| {
            this.navigate(event.0, cx);
        })
        .detach();

        cx.subscribe(&sidebar, |this: &mut Self, _sidebar, _event: &ToggleSidebar, cx| {
            this.toggle_sidebar(cx);
        })
        .detach();

        Self {
            active_screen,
            sidebar_collapsed: false,
            sidebar,
            command_palette_open: false,
            command_palette: None,
        }
    }

    fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active_screen = screen;
        self.sidebar.update(cx, |sb, cx| sb.set_active(screen, cx));
        self.close_palette(cx);
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        let collapsed = self.sidebar_collapsed;
        self.sidebar.update(cx, |sb, cx| sb.set_collapsed(collapsed, cx));
        cx.notify();
    }

    fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.command_palette_open {
            self.close_palette(cx);
        } else {
            self.open_palette(cx);
        }
    }

    fn open_palette(&mut self, cx: &mut Context<Self>) {
        let palette = cx.new(CommandPalette::new);
        cx.subscribe(&palette, |this: &mut Self, _palette, event: &CommandSelected, cx| {
            if let Some(screen) = event.0 {
                this.navigate(screen, cx);
            } else {
                this.close_palette(cx);
            }
        })
        .detach();
        self.command_palette = Some(palette);
        self.command_palette_open = true;
        cx.notify();
    }

    fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette = None;
        self.command_palette_open = false;
        cx.notify();
    }

    pub fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("ctrl-1", GoToDashboard, None),
            KeyBinding::new("ctrl-2", GoToKanban, None),
            KeyBinding::new("ctrl-3", GoToSpecs, None),
            KeyBinding::new("ctrl-4", GoToAgents, None),
            KeyBinding::new("ctrl-5", GoToTerminals, None),
            KeyBinding::new("ctrl-6", GoToExecution, None),
            KeyBinding::new("ctrl-7", GoToDiff, None),
            KeyBinding::new("ctrl-8", GoToInsights, None),
            KeyBinding::new("ctrl-9", GoToSettings, None),
            KeyBinding::new("ctrl-b", ToggleSidebarAction, None),
            KeyBinding::new("ctrl-k", ToggleCommandPalette, None),
        ]);
    }

    fn render_screen_content(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Div {
        let label = self.active_screen.label();
        let icon = self.active_screen.icon();
        div()
            .flex_1()
            .p_6()
            .v_flex()
            .gap_4()
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_2xl()
                            .text_color(theme::PRIMARY)
                            .child(icon.to_string()),
                    )
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(label.to_string()),
                    ),
            )
            .child(
                div()
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("This screen will show {}. Coming soon.", label.to_lowercase())),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .mt_4()
                    .child(
                        Button::new("test-notif")
                            .label("Test Notification")
                            .on_click(|_event, window, cx| {
                                window.push_notification(
                                    SurgeNotification::agent_connected("Claude Code"),
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new("test-error")
                            .label("Test Error")
                            .on_click(|_event, window, cx| {
                                window.push_notification(
                                    SurgeNotification::task_failed("build-auth", "compilation error"),
                                    cx,
                                );
                            }),
                    ),
            )
    }

    fn render_palette_overlay(&self) -> impl IntoElement {
        if let Some(palette) = &self.command_palette {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(80.0))
                .bg(hsla(0.0, 0.0, 0.0, 0.5))
                .child(palette.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }
}

impl Render for SurgeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("SurgeApp")
            .size_full()
            .bg(theme::BACKGROUND)
            .text_color(theme::TEXT_PRIMARY)
            .on_action(cx.listener(|this, _: &GoToDashboard, _w, cx| this.navigate(Screen::Dashboard, cx)))
            .on_action(cx.listener(|this, _: &GoToKanban, _w, cx| this.navigate(Screen::Kanban, cx)))
            .on_action(cx.listener(|this, _: &GoToSpecs, _w, cx| this.navigate(Screen::SpecExplorer, cx)))
            .on_action(cx.listener(|this, _: &GoToAgents, _w, cx| this.navigate(Screen::AgentHub, cx)))
            .on_action(cx.listener(|this, _: &GoToTerminals, _w, cx| this.navigate(Screen::AgentTerminals, cx)))
            .on_action(cx.listener(|this, _: &GoToExecution, _w, cx| this.navigate(Screen::LiveExecution, cx)))
            .on_action(cx.listener(|this, _: &GoToDiff, _w, cx| this.navigate(Screen::DiffViewer, cx)))
            .on_action(cx.listener(|this, _: &GoToInsights, _w, cx| this.navigate(Screen::Insights, cx)))
            .on_action(cx.listener(|this, _: &GoToSettings, _w, cx| this.navigate(Screen::Settings, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebarAction, _w, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, _w, cx| this.toggle_palette(cx)))
            .child(
                div()
                    .size_full()
                    .h_flex()
                    .child(self.sidebar.clone())
                    .child(self.render_screen_content(window, cx)),
            )
            .child(self.render_palette_overlay())
    }
}
