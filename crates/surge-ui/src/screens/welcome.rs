use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::project::{RecentProject, RecentProjects};
use crate::theme;

/// Events emitted by the Welcome screen.
#[derive(Clone, PartialEq)]
pub enum WelcomeEvent {
    /// User selected a project to open.
    OpenProject(std::path::PathBuf),
    /// User wants to browse for a project.
    BrowseProject,
    /// User wants to init a new project.
    InitProject,
    /// User removed a project from the list.
    RemoveProject(std::path::PathBuf),
    /// User toggled pin on a project.
    TogglePin(std::path::PathBuf),
}

impl EventEmitter<WelcomeEvent> for WelcomeScreen {}

/// The Welcome / Project Picker screen shown on startup.
pub struct WelcomeScreen {
    recent: RecentProjects,
}

impl WelcomeScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            recent: RecentProjects::load(),
        }
    }

    fn render_logo(&self) -> Div {
        div()
            .v_flex()
            .items_center()
            .gap_2()
            .pb_8()
            .child(
                div()
                    .text_color(theme::PRIMARY)
                    .child("⚡".to_string()),
            )
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Surge".to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child("Any Agent. One Protocol. Pure Rust.".to_string()),
            )
    }

    fn render_project_item(
        &self,
        project: &RecentProject,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let path = project.path.clone();
        let path_open = path.clone();
        let path_pin = path.clone();
        let path_remove = path.clone();
        let name = project.name.clone();
        let display_path = project.path.display().to_string();
        let pinned = project.pinned;
        let active_tasks = project.active_tasks;

        div()
            .id(SharedString::from(format!("project-{}", display_path)))
            .h_flex()
            .justify_between()
            .items_center()
            .px_4()
            .py_3()
            .rounded_lg()
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.bg(theme::SURFACE))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(WelcomeEvent::OpenProject(path_open.clone()));
            }))
            // Left: project info
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .flex_1()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .when(pinned, |el: Div| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::WARNING)
                                        .child("★".to_string()),
                                )
                            })
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(name),
                            )
                            .when(active_tasks > 0, |el: Div| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(theme::PRIMARY.opacity(0.2))
                                        .text_color(theme::PRIMARY)
                                        .child(format!("{active_tasks} tasks")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(display_path),
                    ),
            )
            // Right: action buttons
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("pin-{}", path.display())))
                            .ghost()
                            .label(if pinned { "★" } else { "☆" })
                            .on_click(move |_event, _window, cx| {
                                // Note: can't emit from Button directly, need workaround
                                let _ = &path_pin;
                                let _ = cx;
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rm-{}", path_remove.display())))
                            .ghost()
                            .label("✕")
                            .on_click(move |_event, _window, cx| {
                                let _ = &path_remove;
                                let _ = cx;
                            }),
                    ),
            )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .gap_3()
            .justify_center()
            .pt_4()
            .child(
                Button::new("open-project")
                    .primary()
                    .label("Open Project")
                    .on_click(|_event, _window, _cx| {
                        // Will trigger native file dialog.
                    }),
            )
            .child(
                Button::new("init-project")
                    .label("Init New Project")
                    .on_click(|_event, _window, _cx| {
                        // Will open init wizard.
                    }),
            )
            .child(
                Button::new("clone-open")
                    .ghost()
                    .label("Clone & Open")
                    .on_click(|_event, _window, _cx| {
                        // Will open clone dialog.
                    }),
            )
    }
}

impl Render for WelcomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sorted = self.recent.sorted();

        let project_items: Vec<Stateful<Div>> = sorted
            .iter()
            .map(|p| self.render_project_item(p, cx))
            .collect();

        div()
            .size_full()
            .v_flex()
            .items_center()
            .justify_center()
            .bg(theme::BACKGROUND)
            .child(
                div()
                    .v_flex()
                    .w(px(600.0))
                    .gap_4()
                    // Logo
                    .child(self.render_logo())
                    // Recent Projects header
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT_MUTED)
                                    .child("Recent Projects".to_string()),
                            ),
                    )
                    // Project list
                    .child(
                        div()
                            .v_flex()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme::SURFACE)
                            .overflow_hidden()
                            .when(project_items.is_empty(), |el: Div| {
                                el.child(
                                    div()
                                        .p_8()
                                        .text_center()
                                        .text_color(theme::TEXT_MUTED)
                                        .child("No recent projects. Open or create one to get started.".to_string()),
                                )
                            })
                            .children(project_items),
                    )
                    // Action buttons
                    .child(self.render_actions(cx)),
            )
    }
}
