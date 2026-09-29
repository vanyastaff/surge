//! Command palette (⌘K): jump to any screen or run an app command.
//!
//! Type to filter, ↑/↓ to move, ⏎ to run, Esc to close. Every command does
//! what its label says — a command with no handler does not belong here.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Escape as InputEscape, Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::router::Screen;
use crate::theme;
use crate::ui;

/// What running a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteCommand {
    Navigate(Screen),
    ToggleSidebar,
    OpenProject,
    NewApp,
}

/// A command in the palette.
#[derive(Clone)]
pub struct Command {
    pub label: SharedString,
    pub category: &'static str,
    pub command: PaletteCommand,
    pub shortcut: Option<SharedString>,
}

impl Command {
    fn nav(label: &str, screen: Screen, shortcut: Option<&str>) -> Self {
        Self {
            label: SharedString::from(label.to_string()),
            category: "Go to",
            command: PaletteCommand::Navigate(screen),
            shortcut: shortcut.map(|s| SharedString::from(ui::shortcut_label(s))),
        }
    }

    fn app(label: &str, command: PaletteCommand, shortcut: Option<&str>) -> Self {
        Self {
            label: SharedString::from(label.to_string()),
            category: "Command",
            command,
            shortcut: shortcut.map(|s| SharedString::from(ui::shortcut_label(s))),
        }
    }

    fn icon(&self) -> Icon {
        match self.command {
            PaletteCommand::Navigate(screen) => Icon::new(screen.icon()),
            PaletteCommand::ToggleSidebar => Icon::new(IconName::PanelLeft),
            PaletteCommand::OpenProject => Icon::new(IconName::FolderOpen),
            PaletteCommand::NewApp => Icon::new(Lucide::FolderPlus),
        }
    }
}

/// Surfaces first (with their shortcuts), then secondary screens reachable
/// only from here, then app commands.
fn all_commands() -> Vec<Command> {
    vec![
        Command::nav("Fleet", Screen::Fleet, Some("Ctrl+1")),
        Command::nav("Roadmap", Screen::Roadmap, Some("Ctrl+2")),
        Command::nav("Missions", Screen::Runs, Some("Ctrl+3")),
        Command::nav("Flow", Screen::Flow, Some("Ctrl+4")),
        Command::nav("Inbox", Screen::Inbox, Some("Ctrl+5")),
        Command::nav("Backlog", Screen::Backlog, Some("Ctrl+6")),
        Command::nav("Agents", Screen::Agents, Some("Ctrl+7")),
        Command::nav("Memory", Screen::ContextMemory, Some("Ctrl+8")),
        Command::nav("Settings", Screen::Settings, Some("Ctrl+9")),
        Command::nav("Plan a task", Screen::SpecWizard, Some("Ctrl+N")),
        Command::nav("Agent catalog", Screen::AgentHub, None),
        Command::nav("Terminals", Screen::AgentTerminals, None),
        Command::app("Open project…", PaletteCommand::OpenProject, Some("Ctrl+O")),
        Command::app("New app…", PaletteCommand::NewApp, None),
        Command::app(
            "Toggle sidebar",
            PaletteCommand::ToggleSidebar,
            Some("Ctrl+B"),
        ),
    ]
}

/// Events emitted by the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteEvent {
    Run(PaletteCommand),
    Dismiss,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

/// Command Palette overlay.
pub struct CommandPalette {
    query: String,
    commands: Vec<Command>,
    filtered: Vec<usize>,
    selected_index: usize,
    /// Search field — created lazily on first render (needs a Window).
    input: Option<Entity<InputState>>,
    scroll: ScrollHandle,
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
            input: None,
            scroll: ScrollHandle::new(),
        }
    }

    /// Lazily build + focus the search input so the palette is
    /// immediately typeable when it opens.
    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = &self.input {
            return input.clone();
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search screens and commands…"));
        cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, _window, cx| match event {
                InputEvent::Change => {
                    this.query = input.read(cx).value().to_string();
                    this.filter();
                    cx.notify();
                },
                InputEvent::PressEnter { .. } => {
                    this.select_current(cx);
                },
                _ => {},
            },
        )
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.input = Some(input.clone());
        input
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
        self.scroll.scroll_to_item(0);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.filtered.len();
        if len == 0 {
            return;
        }
        let next = (self.selected_index as isize + delta).rem_euclid(len as isize);
        self.selected_index = next as usize;
        self.scroll.scroll_to_item(self.selected_index);
        cx.notify();
    }

    fn select_current(&mut self, cx: &mut Context<Self>) {
        if let Some(&idx) = self.filtered.get(self.selected_index) {
            cx.emit(PaletteEvent::Run(self.commands[idx].command));
        }
    }

    fn render_item(&self, list_idx: usize, cx: &mut Context<Self>) -> AnyElement {
        let cmd_idx = self.filtered[list_idx];
        let cmd = &self.commands[cmd_idx];
        let is_selected = list_idx == self.selected_index;

        div()
            .id(("palette-cmd", cmd_idx))
            .test_support()
            .role(Role::Button)
            .aria_label(cmd.label.clone())
            .h_flex()
            .gap(px(10.0))
            .items_center()
            .px(px(10.0))
            .h(px(32.0))
            .rounded(px(ui::R_CONTROL))
            .cursor_pointer()
            .when(is_selected, |el| el.bg(theme::surface()))
            .when(!is_selected, |el| {
                el.hover(|s: StyleRefinement| s.bg(theme::surface().opacity(0.6)))
            })
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.selected_index = list_idx;
                this.select_current(cx);
            }))
            .child(cmd.icon().size(px(14.0)).text_color(if is_selected {
                theme::accent()
            } else {
                theme::text_muted()
            }))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(if is_selected {
                        theme::text_primary()
                    } else {
                        theme::text_muted()
                    })
                    .child(cmd.label.clone()),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_dim())
                    .child(cmd.category),
            )
            .children(cmd.shortcut.clone().map(ui::kbd))
            .into_any_element()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.ensure_input(window, cx);
        let item_count = self.filtered.len();
        let items: Vec<_> = (0..item_count).map(|i| self.render_item(i, cx)).collect();

        ui::panel()
            .v_flex()
            .w(px(540.0))
            .max_h(px(440.0))
            .bg(theme::panel_raised())
            .border_color(theme::hairline_strong())
            .shadow_lg()
            .overflow_hidden()
            // The search field binds Esc to its own `Escape` action, so the
            // keystroke never reaches a key listener — catch the action.
            .capture_action(cx.listener(|_this, _: &InputEscape, _window, cx| {
                cx.stop_propagation();
                cx.emit(PaletteEvent::Dismiss);
            }))
            // Capture phase: the search field would otherwise swallow these.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        cx.emit(PaletteEvent::Dismiss);
                    },
                    "up" => {
                        cx.stop_propagation();
                        this.move_selection(-1, cx);
                    },
                    "down" => {
                        cx.stop_propagation();
                        this.move_selection(1, cx);
                    },
                    _ => {},
                }
            }))
            // Search field
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .px(px(14.0))
                    .h(px(46.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(Icon::new(IconName::Search).size(px(14.0)).text_color(theme::text_muted()))
                    .child(
                        div().flex_1().child(
                            Input::new(&input)
                                .appearance(false)
                                .accessibility_id("command-search")
                                .aria_label("Search commands"),
                        ),
                    )
                    .child(ui::kbd("esc")),
            )
            // Results
            .child(
                div()
                    .id("palette-results")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .flex_1()
                    .v_flex()
                    .p(px(6.0))
                    .gap(px(1.0))
                    .when(item_count == 0, |el| {
                        el.child(
                            div()
                                .py(px(22.0))
                                .text_center()
                                .text_size(px(12.0))
                                .text_color(theme::text_muted())
                                .child(format!("Nothing matches “{}”", self.query)),
                        )
                    })
                    .children(items),
            )
            // Footer: the keys, nothing else.
            .child(
                div()
                    .h_flex()
                    .gap(px(14.0))
                    .items_center()
                    .px(px(14.0))
                    .h(px(32.0))
                    .border_t_1()
                    .border_color(theme::hairline())
                    .text_size(px(10.5))
                    .text_color(theme::text_dim())
                    .child(div().h_flex().gap(px(5.0)).child(ui::kbd("↑↓")).child("move"))
                    .child(div().h_flex().gap(px(5.0)).child(ui::kbd("↵")).child("open"))
                    .child(div().h_flex().gap(px(5.0)).child(ui::kbd("esc")).child("close")),
            )
    }
}

#[cfg(test)]
mod accessibility_tests {
    use super::{CommandPalette, PaletteCommand, PaletteEvent};
    use crate::router::Screen;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, Role, TestAppContext, WindowOptions};
    use std::{cell::RefCell, rc::Rc};

    #[gpui_kit::test]
    fn filtered_command_keeps_identity_and_dispatches_its_destination(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let selected = Rc::new(RefCell::new(None));
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(CommandPalette::new)
            })
            .unwrap()
        });
        let copy = selected.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &PaletteEvent, _| {
                copy.replace(Some(*event));
            })
            .detach();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find(("palette-cmd", 4_usize)).label(), Some("Inbox"));
            let input = view.read(cx).input.clone().unwrap();
            input.update(cx, |input, cx| input.set_value("Inbox", window, cx));
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(("palette-cmd", 4_usize)).role(),
                Some(Role::Button)
            );
            assert_eq!(window.find(("palette-cmd", 4_usize)).label(), Some("Inbox"));
            window.click(("palette-cmd", 4_usize), cx);
        })
        .unwrap();
        cx.update(|_| {
            assert_eq!(
                *selected.borrow(),
                Some(PaletteEvent::Run(PaletteCommand::Navigate(Screen::Inbox)))
            );
        });
    }
}
