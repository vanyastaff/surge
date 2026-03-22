use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

/// File change status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl ChangeStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Added => theme::SUCCESS,
            Self::Modified => theme::WARNING,
            Self::Deleted => theme::ERROR,
            Self::Renamed => theme::PRIMARY,
        }
    }
}

/// A changed file entry.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub directory: String,
    pub filename: String,
    pub status: ChangeStatus,
    pub added: u32,
    pub removed: u32,
}

/// Directory group in the tree view.
#[derive(Debug, Clone)]
struct DirGroup {
    dir: String,
    files: Vec<ChangedFile>,
    expanded: bool,
}

/// File Explorer screen — tree view of changed files.
pub struct FileExplorerScreen {
    groups: Vec<DirGroup>,
    selected_file: Option<String>,
}

impl FileExplorerScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let files = demo_changed_files();

        // Group by directory
        let mut dir_map: Vec<(String, Vec<ChangedFile>)> = Vec::new();
        for file in files {
            if let Some(group) = dir_map.iter_mut().find(|(d, _)| *d == file.directory) {
                group.1.push(file);
            } else {
                let dir = file.directory.clone();
                dir_map.push((dir, vec![file]));
            }
        }

        let groups = dir_map
            .into_iter()
            .map(|(dir, files)| DirGroup { dir, files, expanded: true })
            .collect();

        Self {
            groups,
            selected_file: None,
        }
    }

    fn total_stats(&self) -> (usize, u32, u32) {
        let mut count = 0;
        let mut added = 0;
        let mut removed = 0;
        for g in &self.groups {
            for f in &g.files {
                count += 1;
                added += f.added;
                removed += f.removed;
            }
        }
        (count, added, removed)
    }

    fn render_dir_group(&self, group_idx: usize, group: &DirGroup, cx: &mut Context<Self>) -> Div {
        let header = div()
            .id(SharedString::from(format!("dir-{group_idx}")))
            .h_flex()
            .gap_2()
            .px_3()
            .py(px(6.0))
            .cursor_pointer()
            .rounded_md()
            .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.05)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.groups[group_idx].expanded = !this.groups[group_idx].expanded;
                cx.notify();
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(if group.expanded { "v" } else { ">" }.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(group.dir.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(theme::TEXT_MUTED.opacity(0.15))
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("{}", group.files.len())),
            );

        let mut container = div().v_flex().child(header);

        if group.expanded {
            let file_rows: Vec<Stateful<Div>> = group
                .files
                .iter()
                .enumerate()
                .map(|(fi, file)| {
                    let path = file.path.clone();
                    let is_selected = self.selected_file.as_deref() == Some(&file.path);

                    div()
                        .id(SharedString::from(format!("file-{group_idx}-{fi}")))
                        .h_flex()
                        .gap_2()
                        .pl_6()
                        .pr_3()
                        .py(px(4.0))
                        .cursor_pointer()
                        .rounded_md()
                        .bg(if is_selected { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                        .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.05)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.selected_file = Some(path.clone());
                            cx.notify();
                        }))
                        // Status badge
                        .child(
                            div()
                                .text_xs()
                                .w(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(file.status.color())
                                .child(file.status.label().to_string()),
                        )
                        // Filename
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(theme::TEXT_PRIMARY)
                                .child(file.filename.clone()),
                        )
                        // +/- counts
                        .child(
                            div()
                                .h_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::SUCCESS)
                                        .child(format!("+{}", file.added)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::ERROR)
                                        .child(format!("-{}", file.removed)),
                                ),
                        )
                })
                .collect();

            container = container.child(div().v_flex().children(file_rows));
        }

        container
    }

    fn render_context_bar(&self) -> Div {
        let (count, added, removed) = self.total_stats();
        div()
            .h_flex()
            .gap_4()
            .px_4()
            .py_3()
            .bg(theme::SURFACE)
            .rounded_lg()
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("{count} files changed")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::SUCCESS)
                    .child(format!("+{added} lines")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::ERROR)
                    .child(format!("-{removed} lines")),
            )
            .when(self.selected_file.is_some(), |el: Div| {
                let selected = self.selected_file.as_deref().unwrap_or("");
                el.child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(theme::PRIMARY)
                        .child(format!("Selected: {selected}")),
                )
            })
    }
}

impl Render for FileExplorerScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let groups: Vec<Div> = self
            .groups
            .iter()
            .enumerate()
            .map(|(idx, group)| self.render_dir_group(idx, group, cx))
            .collect();

        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            .overflow_hidden()
            // Header
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("File Explorer".to_string()),
                    )
                    .child(
                        Button::new("fe-view-diff")
                            .primary()
                            .label("View Diff"),
                    ),
            )
            // Tree view
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_lg()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .overflow_hidden()
                    .children(groups),
            )
            // Context info
            .child(self.render_context_bar())
    }
}

fn demo_changed_files() -> Vec<ChangedFile> {
    vec![
        ChangedFile {
            path: "src/auth/middleware.rs".into(),
            directory: "src/auth".into(),
            filename: "middleware.rs".into(),
            status: ChangeStatus::Added,
            added: 45,
            removed: 0,
        },
        ChangedFile {
            path: "src/auth/mod.rs".into(),
            directory: "src/auth".into(),
            filename: "mod.rs".into(),
            status: ChangeStatus::Modified,
            added: 2,
            removed: 0,
        },
        ChangedFile {
            path: "src/routes/mod.rs".into(),
            directory: "src/routes".into(),
            filename: "mod.rs".into(),
            status: ChangeStatus::Modified,
            added: 3,
            removed: 1,
        },
        ChangedFile {
            path: "src/routes/auth.rs".into(),
            directory: "src/routes".into(),
            filename: "auth.rs".into(),
            status: ChangeStatus::Added,
            added: 28,
            removed: 0,
        },
        ChangedFile {
            path: "src/old_auth.rs".into(),
            directory: "src".into(),
            filename: "old_auth.rs".into(),
            status: ChangeStatus::Deleted,
            added: 0,
            removed: 22,
        },
        ChangedFile {
            path: "src/config.rs".into(),
            directory: "src".into(),
            filename: "config.rs".into(),
            status: ChangeStatus::Renamed,
            added: 5,
            removed: 3,
        },
        ChangedFile {
            path: "Cargo.toml".into(),
            directory: ".".into(),
            filename: "Cargo.toml".into(),
            status: ChangeStatus::Modified,
            added: 2,
            removed: 0,
        },
    ]
}
