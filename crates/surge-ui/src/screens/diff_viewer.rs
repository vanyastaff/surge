use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

/// Status of a file in the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
}

impl FileStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Added => theme::SUCCESS,
            Self::Modified => theme::WARNING,
            Self::Deleted => theme::ERROR,
        }
    }
}

/// A single diff line.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub left_num: Option<u32>,
    pub right_num: Option<u32>,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// A file with its diff hunks.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub status: FileStatus,
    pub added: u32,
    pub removed: u32,
    pub lines: Vec<DiffLine>,
}

/// Active filter for file status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffFilter {
    All,
    Added,
    Modified,
    Deleted,
}

impl DiffFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Added => "Added",
            Self::Modified => "Modified",
            Self::Deleted => "Deleted",
        }
    }

    fn all() -> &'static [DiffFilter] {
        &[Self::All, Self::Added, Self::Modified, Self::Deleted]
    }
}

/// Diff Viewer screen — side-by-side diff display.
pub struct DiffViewerScreen {
    files: Vec<FileDiff>,
    selected_file: usize,
    active_filter: DiffFilter,
}

impl DiffViewerScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            files: demo_files(),
            selected_file: 0,
            active_filter: DiffFilter::All,
        }
    }

    fn filtered_files(&self) -> Vec<(usize, &FileDiff)> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, f)| match self.active_filter {
                DiffFilter::All => true,
                DiffFilter::Added => f.status == FileStatus::Added,
                DiffFilter::Modified => f.status == FileStatus::Modified,
                DiffFilter::Deleted => f.status == FileStatus::Deleted,
            })
            .collect()
    }

    fn render_filter_bar(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = DiffFilter::all()
            .iter()
            .map(|&filter| {
                let is_active = filter == self.active_filter;
                div()
                    .id(SharedString::from(format!("filter-{}", filter.label())))
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .text_sm()
                    .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                    .bg(if is_active { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.active_filter = filter;
                        // Reset selection if current file is filtered out
                        let visible = this.filtered_files();
                        if !visible.iter().any(|(i, _)| *i == this.selected_file) {
                            if let Some((i, _)) = visible.first() {
                                this.selected_file = *i;
                            }
                        }
                        cx.notify();
                    }))
                    .child(filter.label().to_string())
            })
            .collect();

        div().h_flex().gap_1().children(tabs)
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> Div {
        let items: Vec<Stateful<Div>> = self
            .filtered_files()
            .iter()
            .map(|&(idx, file)| {
                let is_selected = idx == self.selected_file;
                div()
                    .id(SharedString::from(format!("file-{idx}")))
                    .h_flex()
                    .gap_2()
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .bg(if is_selected { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                    .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.05)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.selected_file = idx;
                        cx.notify();
                    }))
                    // Status badge
                    .child(
                        div()
                            .text_xs()
                            .w(px(18.0))
                            .text_color(file.status.color())
                            .font_weight(FontWeight::BOLD)
                            .child(file.status.label().to_string()),
                    )
                    // File name
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .overflow_hidden()
                            .child(file.path.clone()),
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

        div()
            .w(px(280.0))
            .v_flex()
            .gap_1()
            .overflow_hidden()
            .children(items)
    }

    fn render_diff_content(&self) -> Div {
        let file = &self.files[self.selected_file];

        let lines: Vec<Div> = file
            .lines
            .iter()
            .map(|line| {
                let (bg, text_color, prefix) = match line.kind {
                    DiffLineKind::Added => (
                        theme::SUCCESS.opacity(0.1),
                        theme::SUCCESS,
                        "+",
                    ),
                    DiffLineKind::Removed => (
                        theme::ERROR.opacity(0.1),
                        theme::ERROR,
                        "-",
                    ),
                    DiffLineKind::Context => (
                        gpui::transparent_black(),
                        theme::TEXT_MUTED,
                        " ",
                    ),
                };

                let left = line.left_num.map_or("   ".to_string(), |n| format!("{n:3}"));
                let right = line.right_num.map_or("   ".to_string(), |n| format!("{n:3}"));

                div()
                    .h_flex()
                    .px_2()
                    .bg(bg)
                    // Line numbers
                    .child(
                        div()
                            .text_xs()
                            .w(px(36.0))
                            .text_color(theme::TEXT_MUTED.opacity(0.5))
                            .font_family("monospace")
                            .child(left),
                    )
                    .child(
                        div()
                            .text_xs()
                            .w(px(36.0))
                            .text_color(theme::TEXT_MUTED.opacity(0.5))
                            .font_family("monospace")
                            .child(right),
                    )
                    // Prefix and content
                    .child(
                        div()
                            .text_xs()
                            .w(px(12.0))
                            .text_color(text_color)
                            .font_family("monospace")
                            .child(prefix.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(text_color)
                            .font_family("monospace")
                            .child(line.content.clone()),
                    )
            })
            .collect();

        div()
            .flex_1()
            .v_flex()
            .gap_2()
            // File header
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(file.path.clone()),
                    )
                    .child(
                        Button::new("open-ide")
                            .ghost()
                            .label("Open in IDE"),
                    ),
            )
            // Diff lines
            .child(
                div()
                    .v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .bg(theme::BACKGROUND)
                    .overflow_hidden()
                    .children(lines),
            )
    }
}

impl Render for DiffViewerScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_added: u32 = self.files.iter().map(|f| f.added).sum();
        let total_removed: u32 = self.files.iter().map(|f| f.removed).sum();

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
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child("Diff Viewer".to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(format!(
                                        "{} files  +{}  -{}",
                                        self.files.len(),
                                        total_added,
                                        total_removed
                                    )),
                            ),
                    )
                    .child(self.render_filter_bar(cx)),
            )
            // Main: file list + diff
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap_4()
                    .overflow_hidden()
                    .child(self.render_file_list(cx))
                    .child(self.render_diff_content()),
            )
    }
}

fn demo_files() -> Vec<FileDiff> {
    vec![
        FileDiff {
            path: "src/auth/middleware.rs".into(),
            status: FileStatus::Added,
            added: 45,
            removed: 0,
            lines: vec![
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(1), content: "use actix_web::{HttpRequest, HttpResponse};".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(2), content: "use jsonwebtoken::{decode, Validation};".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(3), content: "".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(4), content: "pub async fn auth_middleware(req: HttpRequest) -> Result<(), HttpResponse> {".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(5), content: "    let token = req.headers().get(\"Authorization\");".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(6), content: "    // TODO: validate token".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(7), content: "    Ok(())".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(8), content: "}".into() },
            ],
        },
        FileDiff {
            path: "src/routes/mod.rs".into(),
            status: FileStatus::Modified,
            added: 3,
            removed: 1,
            lines: vec![
                DiffLine { kind: DiffLineKind::Context, left_num: Some(10), right_num: Some(10), content: "use crate::handlers::*;".into() },
                DiffLine { kind: DiffLineKind::Removed, left_num: Some(11), right_num: None, content: "use crate::db::Pool;".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(11), content: "use crate::auth::middleware::auth_middleware;".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(12), content: "use crate::db::Pool;".into() },
                DiffLine { kind: DiffLineKind::Context, left_num: Some(12), right_num: Some(13), content: "".into() },
                DiffLine { kind: DiffLineKind::Context, left_num: Some(13), right_num: Some(14), content: "pub fn configure(cfg: &mut web::ServiceConfig) {".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(15), content: "    cfg.app_data(auth_middleware);".into() },
                DiffLine { kind: DiffLineKind::Context, left_num: Some(14), right_num: Some(16), content: "    cfg.service(health_check);".into() },
            ],
        },
        FileDiff {
            path: "src/old_auth.rs".into(),
            status: FileStatus::Deleted,
            added: 0,
            removed: 22,
            lines: vec![
                DiffLine { kind: DiffLineKind::Removed, left_num: Some(1), right_num: None, content: "// Legacy auth module — replaced by middleware".into() },
                DiffLine { kind: DiffLineKind::Removed, left_num: Some(2), right_num: None, content: "pub fn check_token(token: &str) -> bool {".into() },
                DiffLine { kind: DiffLineKind::Removed, left_num: Some(3), right_num: None, content: "    token == \"hardcoded-secret\"".into() },
                DiffLine { kind: DiffLineKind::Removed, left_num: Some(4), right_num: None, content: "}".into() },
            ],
        },
        FileDiff {
            path: "Cargo.toml".into(),
            status: FileStatus::Modified,
            added: 2,
            removed: 0,
            lines: vec![
                DiffLine { kind: DiffLineKind::Context, left_num: Some(8), right_num: Some(8), content: "[dependencies]".into() },
                DiffLine { kind: DiffLineKind::Context, left_num: Some(9), right_num: Some(9), content: "actix-web = \"4\"".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(10), content: "jsonwebtoken = \"9\"".into() },
                DiffLine { kind: DiffLineKind::Added, left_num: None, right_num: Some(11), content: "serde = { version = \"1\", features = [\"derive\"] }".into() },
                DiffLine { kind: DiffLineKind::Context, left_num: Some(10), right_num: Some(12), content: "tokio = { version = \"1\", features = [\"full\"] }".into() },
            ],
        },
    ]
}
