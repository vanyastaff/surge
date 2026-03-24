use gpui::*;
use gpui_component::StyledExt;

use crate::router::Screen;
use crate::theme;

/// A command in the palette.
#[derive(Clone)]
pub struct Command {
    pub label: SharedString,
    pub category: SharedString,
    pub screen: Option<Screen>,
    pub shortcut: Option<SharedString>,
}

impl Command {
    fn nav(label: &str, screen: Screen, shortcut: Option<&str>) -> Self {
        Self {
            label: SharedString::from(label.to_string()),
            category: SharedString::from("Navigation"),
            screen: Some(screen),
            shortcut: shortcut.map(|s| SharedString::from(s.to_string())),
        }
    }
}

/// Build the full command list.
fn all_commands() -> Vec<Command> {
    vec![
        Command::nav("Dashboard", Screen::Dashboard, Some("Ctrl+1")),
        Command::nav("Kanban Board", Screen::Kanban, Some("Ctrl+2")),
        Command::nav("Spec Explorer", Screen::SpecExplorer, Some("Ctrl+3")),
        Command::nav("Agent Hub", Screen::AgentHub, Some("Ctrl+4")),
        Command::nav("Terminals", Screen::AgentTerminals, Some("Ctrl+5")),
        Command::nav("Live Execution", Screen::LiveExecution, Some("Ctrl+6")),
        Command::nav("Diff Viewer", Screen::DiffViewer, Some("Ctrl+7")),
        Command::nav("Insights", Screen::Insights, Some("Ctrl+8")),
        Command::nav("Settings", Screen::Settings, Some("Ctrl+9")),
        Command::nav("File Explorer", Screen::FileExplorer, None),
        Command::nav("Worktrees", Screen::Worktrees, None),
        Command::nav("GitHub Issues", Screen::GitHubIssues, None),
        Command::nav("Pull Requests", Screen::GitHubPRs, None),
        Command::nav("Roadmap", Screen::Roadmap, None),
        Command::nav("Context / Memory", Screen::ContextMemory, None),
        Command {
            label: "Toggle Sidebar".into(),
            category: "UI".into(),
            screen: None,
            shortcut: Some("Ctrl+B".into()),
        },
    ]
}

/// Event emitted when a command is selected.
#[derive(Clone, PartialEq)]
pub struct CommandSelected(pub Option<Screen>);

impl EventEmitter<CommandSelected> for CommandPalette {}

/// Command Palette overlay.
pub struct CommandPalette {
    query: String,
    commands: Vec<Command>,
    filtered: Vec<usize>,
    selected_index: usize,
}

impl CommandPalette {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let commands = all_commands();
        let filtered: Vec<usize> = (0..commands.len()).collect();
        Self {
            query: String::new(),
            commands,
            filtered,
            selected_index: 0,
        }
    }

    fn filter(&mut self) {
        let q = self.query.to_lowercase();
        if q.is_empty() {
            self.filtered = (0..self.commands.len()).collect();
        } else {
            self.filtered = self
                .commands
                .iter()
                .enumerate()
                .filter(|(_, cmd)| {
                    cmd.label.to_lowercase().contains(&q)
                        || cmd.category.to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.selected_index = 0;
    }

    fn select_current(&mut self, cx: &mut Context<Self>) {
        if let Some(&idx) = self.filtered.get(self.selected_index) {
            let cmd = &self.commands[idx];
            cx.emit(CommandSelected(cmd.screen));
        }
    }

    fn render_item(&self, list_idx: usize, _cx: &mut Context<Self>) -> Div {
        let cmd_idx = self.filtered[list_idx];
        let cmd = &self.commands[cmd_idx];
        let is_selected = list_idx == self.selected_index;

        let base = div()
            .h_flex()
            .justify_between()
            .px_3()
            .py(px(6.0))
            .rounded_md();

        let base = if is_selected {
            base.bg(theme::PRIMARY.opacity(0.15))
        } else {
            base
        };

        let mut row = base.child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .child(cmd.category.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(if is_selected {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_MUTED
                        })
                        .child(cmd.label.clone()),
                ),
        );

        if let Some(sc) = &cmd.shortcut {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED.opacity(0.5))
                    .child(sc.clone()),
            );
        }

        row
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let item_count = self.filtered.len();
        let items: Vec<Div> = (0..item_count).map(|i| self.render_item(i, cx)).collect();

        div()
            .v_flex()
            .w(px(500.0))
            .max_h(px(400.0))
            .bg(theme::SURFACE)
            .rounded_lg()
            .border_1()
            .border_color(theme::PRIMARY.opacity(0.3))
            .shadow_lg()
            .overflow_hidden()
            // Header with query display
            .child(
                div()
                    .h_flex()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::BACKGROUND)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_MUTED)
                            .child("⌘ ".to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(if self.query.is_empty() {
                                "Type to search commands...".to_string()
                            } else {
                                self.query.clone()
                            }),
                    ),
            )
            // Results
            .child(div().v_flex().px_1().py_1().gap_0p5().children(items))
            // Footer
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme::BACKGROUND)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED.opacity(0.5))
                            .child(format!("{} commands", item_count)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED.opacity(0.5))
                            .child("↑↓ Navigate  ⏎ Select  Esc Close".to_string()),
                    ),
            )
    }
}
