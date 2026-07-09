use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::Icon;
use gpui_component::StyledExt;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::AppState;
use crate::daemon_link::ConnectionState;
use crate::router::Screen;
use crate::theme;
use crate::ui;

/// Action dispatched when a nav item is clicked.
#[derive(Clone, PartialEq)]
pub struct NavigateTo(pub Screen);

impl EventEmitter<NavigateTo> for AppSidebar {}

/// Action to toggle sidebar collapsed state.
#[derive(Clone, PartialEq)]
pub struct ToggleSidebar;

impl EventEmitter<ToggleSidebar> for AppSidebar {}

/// Operator clicked the offline daemon footer — start the daemon.
#[derive(Clone, PartialEq)]
pub struct StartDaemon;

impl EventEmitter<StartDaemon> for AppSidebar {}

/// The fleet-ops navigation rail: logo, destinations, live daemon
/// footer. Restyled from the classic sidebar per the "Surge -
/// Interactive" concept — same nav model, new chrome.
pub struct AppSidebar {
    active: Screen,
    collapsed: bool,
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
            state,
        }
    }

    pub fn set_active(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active = screen;
        cx.notify();
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

    /// Count of decisions blocked on the operator — the Inbox badge.
    /// Live gates from run streams + tasks in review + failed runs.
    fn needs_you_count(&self, cx: &Context<Self>) -> usize {
        use surge_core::TaskState;
        let state = self.state.read(cx);
        let live = state
            .run_streams
            .values()
            .map(|s| s.pending.len())
            .sum::<usize>();
        let reviews = state
            .tasks
            .iter()
            .filter(|t| matches!(t.state, TaskState::HumanReview | TaskState::QaReview { .. }))
            .count();
        let failed = state
            .runs
            .iter()
            .filter(|r| matches!(r.status, RunStatus::Failed | RunStatus::Aborted))
            .count();
        live + reviews + failed
    }

    fn render_nav_item(&self, screen: Screen, cx: &mut Context<Self>) -> Stateful<Div> {
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
            .h_flex()
            .gap(px(9.0))
            .items_center()
            .px(px(10.0))
            .py(px(6.0))
            .mx(px(6.0))
            .rounded_md()
            .cursor_pointer()
            .when(collapsed, |el| el.justify_center())
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(NavigateTo(screen));
            }));

        let base = if is_active {
            base.bg(theme::accent().opacity(0.12))
                .text_color(theme::accent())
        } else {
            base.text_color(theme::text_muted())
                .hover(|s: StyleRefinement| {
                    s.bg(theme::panel_raised())
                        .text_color(theme::text_primary())
                })
        };

        let icon_color = if is_active {
            theme::accent()
        } else {
            theme::text_muted()
        };
        let mut row = base.child(
            Icon::new(screen.icon())
                .size(px(15.0))
                .text_color(icon_color),
        );

        if !collapsed {
            row = row.child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.to_string()),
            );

            if let Some(n) = badge {
                row = row.child(
                    div()
                        .px(px(6.0))
                        .rounded_full()
                        .bg(theme::accent())
                        .text_size(px(9.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::on_accent())
                        .child(n.to_string()),
                );
            }

            if let Some(sc) = screen.shortcut() {
                // Show just the trailing digit (Ctrl+N → N) as a quiet key hint.
                let key = sc.rsplit('+').next().unwrap_or(sc);
                row = row.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::text_muted().opacity(0.6))
                        .child(key.to_string()),
                );
            }
        }

        row
    }

    /// Live daemon status footer — color + text derived from the real
    /// connection state; run counts from the real run list.
    fn render_footer(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let state = self.state.read(cx);
        let offline = matches!(
            &state.daemon_state,
            ConnectionState::Failed(_) | ConnectionState::Disconnected
        );
        let (dot, label): (Hsla, &str) = match &state.daemon_state {
            ConnectionState::Connected(_) => (theme::success(), "DAEMON · LIVE"),
            ConnectionState::Connecting => (theme::warning(), "DAEMON · SYNC"),
            ConnectionState::Failed(_) | ConnectionState::Disconnected => {
                (theme::text_muted(), "DAEMON · OFFLINE — START")
            },
        };

        let total = state.runs.len();
        let active = state
            .runs
            .iter()
            .filter(|r| matches!(r.status, RunStatus::Active))
            .count();
        let stats = format!("{total} runs · {active} active");

        if self.collapsed {
            return div()
                .id("daemon-footer")
                .py(px(12.0))
                .flex()
                .justify_center()
                .border_t_1()
                .border_color(theme::hairline())
                .when(offline, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(StartDaemon)))
                })
                .child(ui::status_dot(dot));
        }

        div()
            .id("daemon-footer")
            .v_flex()
            .gap(px(8.0))
            .px(px(14.0))
            .py(px(12.0))
            .border_t_1()
            .border_color(theme::hairline())
            .when(offline, |el| {
                el.cursor_pointer()
                    .hover(|s: StyleRefinement| s.bg(theme::panel_raised()))
                    .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(StartDaemon)))
            })
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(ui::status_dot(dot))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child(label.to_string()),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_muted().opacity(0.8))
                    .child(stats),
            )
    }

    fn render_logo(&self) -> Div {
        let mark = div()
            .w(px(20.0))
            .h(px(20.0))
            .rounded_md()
            .bg(theme::accent())
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(hsla(0.0, 0.0, 0.1, 1.0))
                    .child("⚡"),
            );

        div()
            .h_flex()
            .gap(px(9.0))
            .items_center()
            .px(px(14.0))
            .pt(px(15.0))
            .pb(px(13.0))
            .when(self.collapsed, |el| el.justify_center().px(px(0.0)))
            .child(mark)
            .when(!self.collapsed, |el| {
                el.child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child("SURGE"),
                )
            })
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
            .py(px(8.0))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.text_color(theme::text_primary()))
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
        let width = if self.collapsed { px(56.0) } else { px(210.0) };

        let items: Vec<Stateful<Div>> = Screen::sidebar_items()
            .iter()
            .map(|&screen| self.render_nav_item(screen, cx))
            .collect();

        div()
            .v_flex()
            .w(width)
            .h_full()
            .flex_shrink_0()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            // Logo
            .child(self.render_logo())
            // Nav items
            .child(div().v_flex().gap(px(2.0)).py(px(4.0)).children(items))
            // Spacer
            .child(div().flex_1())
            // Live daemon footer
            .child(self.render_footer(cx))
            // Collapse toggle
            .child(self.render_toggle_button(cx))
    }
}
