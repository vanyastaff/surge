use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::Icon;
use gpui_component::StyledExt;

use crate::router::Screen;
use crate::theme;

/// Action dispatched when a nav item is clicked.
#[derive(Clone, PartialEq)]
pub struct NavigateTo(pub Screen);

impl EventEmitter<NavigateTo> for AppSidebar {}

/// Action to toggle sidebar collapsed state.
#[derive(Clone, PartialEq)]
pub struct ToggleSidebar;

impl EventEmitter<ToggleSidebar> for AppSidebar {}

/// Sidebar navigation panel.
pub struct AppSidebar {
    active: Screen,
    collapsed: bool,
}

impl AppSidebar {
    pub fn new(active: Screen, collapsed: bool, _cx: &mut Context<Self>) -> Self {
        Self { active, collapsed }
    }

    pub fn set_active(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active = screen;
        cx.notify();
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

    fn render_nav_item(
        &self,
        screen: Screen,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let is_active = self.active == screen;
        let collapsed = self.collapsed;
        let label = screen.label();

        let base = div()
            .id(SharedString::from(format!("nav-{}", label)))
            .h_flex()
            .gap_3()
            .px_3()
            .py(px(6.0))
            .mx_2()
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(NavigateTo(screen));
            }));

        let base = if is_active {
            base.bg(theme::PRIMARY.opacity(0.15))
                .text_color(theme::PRIMARY)
        } else {
            base.text_color(theme::TEXT_MUTED)
                .hover(|s: StyleRefinement| {
                    s.bg(theme::SURFACE).text_color(theme::TEXT_PRIMARY)
                })
        };

        let icon_color = if is_active { theme::PRIMARY } else { theme::TEXT_MUTED };
        let mut row = base.child(
            Icon::new(screen.icon()).size_4().text_color(icon_color),
        );

        if !collapsed {
            row = row.child(
                div()
                    .flex_1()
                    .text_sm()
                    .child(label.to_string()),
            );

            if let Some(sc) = screen.shortcut() {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED.opacity(0.5))
                        .child(sc.to_string()),
                );
            }
        }

        row
    }

    fn render_toggle_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        use gpui_component::IconName;
        let icon_name = if self.collapsed {
            IconName::PanelLeftOpen
        } else {
            IconName::PanelLeftClose
        };
        div()
            .id("sidebar-toggle")
            .h_flex()
            .justify_center()
            .py_2()
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.text_color(theme::TEXT_PRIMARY))
            .on_click(cx.listener(|_this, _event, _window, cx| {
                cx.emit(ToggleSidebar);
            }))
            .child(Icon::new(icon_name).size_4().text_color(theme::TEXT_MUTED))
    }
}

impl Render for AppSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = if self.collapsed { px(52.0) } else { px(220.0) };

        let items: Vec<Stateful<Div>> = Screen::sidebar_items()
            .iter()
            .map(|&screen| self.render_nav_item(screen, cx))
            .collect();

        div()
            .v_flex()
            .w(width)
            .h_full()
            .flex_shrink_0()
            .bg(theme::SIDEBAR_BG)
            .border_r_1()
            .border_color(theme::SURFACE)
            .pt_4()
            .gap_0p5()
            // Logo
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .px_4()
                    .pb_4()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::PRIMARY)
                            .child("⚡".to_string()),
                    )
                    .when(!self.collapsed, |el: Div| {
                        el.child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::TEXT_PRIMARY)
                                .child("Surge".to_string()),
                        )
                    }),
            )
            // Nav items
            .children(items)
            // Spacer
            .child(div().flex_1())
            // Toggle button at bottom
            .child(self.render_toggle_button(cx))
    }
}
