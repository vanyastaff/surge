use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
use crate::theme;

/// Worktree status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeStatus {
    Active,
    Idle,
    Merging,
    Error,
}

impl WorktreeStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Idle => "Idle",
            Self::Merging => "Merging",
            Self::Error => "Error",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Active => theme::SUCCESS,
            Self::Idle => theme::TEXT_MUTED,
            Self::Merging => theme::WARNING,
            Self::Error => theme::ERROR,
        }
    }
}

/// A worktree entry.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub spec_name: String,
    pub branch: String,
    pub status: WorktreeStatus,
    pub file_count: u32,
    pub disk_mb: f32,
    pub path: String,
}

/// Worktrees Panel screen.
pub struct WorktreesScreen {
    state: Entity<AppState>,
}

impl WorktreesScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }

    /// Build WorktreeInfo list from AppState.
    fn build_worktrees(&self, cx: &Context<Self>) -> Vec<WorktreeInfo> {
        let app_state = self.state.read(cx);
        app_state
            .worktrees
            .iter()
            .map(|wt| {
                let status = if wt.exists {
                    WorktreeStatus::Active
                } else {
                    WorktreeStatus::Error
                };
                WorktreeInfo {
                    spec_name: wt.spec_id.clone(),
                    branch: wt.branch.clone(),
                    status,
                    file_count: 0,
                    disk_mb: 0.0,
                    path: wt.path.display().to_string(),
                }
            })
            .collect()
    }

    fn total_disk(worktrees: &[WorktreeInfo]) -> f32 {
        worktrees.iter().map(|w| w.disk_mb).sum()
    }

    fn render_worktree_card(&self, idx: usize, wt: &WorktreeInfo) -> Div {
        div()
            .v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            // Header: spec name + status badge
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
                            .child(wt.spec_name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(wt.status.color().opacity(0.15))
                            .text_color(wt.status.color())
                            .child(wt.status.label().to_string()),
                    ),
            )
            // Info rows
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(self.info_row("Branch", &wt.branch))
                    .child(self.info_row("Files", &format!("{} changed", wt.file_count)))
                    .child(self.info_row("Disk", &format!("{:.1} MB", wt.disk_mb)))
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child("Path".to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED.opacity(0.7))
                                    .font_family("monospace")
                                    .overflow_hidden()
                                    .child(wt.path.clone()),
                            ),
                    ),
            )
            // Actions
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.05))
                    .child(
                        Button::new(SharedString::from(format!("wt-ide-{idx}")))
                            .ghost()
                            .label("Open IDE"),
                    )
                    .child(
                        Button::new(SharedString::from(format!("wt-diff-{idx}")))
                            .ghost()
                            .label("Diff"),
                    )
                    .child(
                        Button::new(SharedString::from(format!("wt-merge-{idx}")))
                            .primary()
                            .label("Merge"),
                    )
                    .child(
                        Button::new(SharedString::from(format!("wt-discard-{idx}")))
                            .ghost()
                            .label("Discard"),
                    ),
            )
    }

    fn info_row(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(value.to_string()),
            )
    }
}

impl Render for WorktreesScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let worktrees = self.build_worktrees(cx);
        let is_empty = worktrees.is_empty();

        let cards: Vec<Div> = worktrees
            .iter()
            .enumerate()
            .map(|(idx, wt)| self.render_worktree_card(idx, wt))
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
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child("Worktrees".to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(format!(
                                        "{} worktrees  {:.1} MB total",
                                        worktrees.len(),
                                        Self::total_disk(&worktrees)
                                    )),
                            ),
                    )
                    // Bulk actions
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("wt-merge-all")
                                    .primary()
                                    .label("Merge All"),
                            )
                            .child(
                                Button::new("wt-prune")
                                    .ghost()
                                    .label("Prune"),
                            ),
                    ),
            )
            // Empty state or cards grid
            .when(is_empty, |el: Div| {
                el.child(
                    div()
                        .flex_1()
                        .v_flex()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::FolderOpen)
                                .size_8()
                                .text_color(theme::TEXT_MUTED.opacity(0.3)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::TEXT_MUTED)
                                .child("No active worktrees".to_string()),
                        ),
                )
            })
            .when(!is_empty, |el: Div| {
                el.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_wrap()
                        .gap_4()
                        .overflow_hidden()
                        .children(
                            cards.into_iter().map(|card| {
                                card.w(relative(0.48)).min_w(px(340.0))
                            }),
                        ),
                )
            })
    }
}
