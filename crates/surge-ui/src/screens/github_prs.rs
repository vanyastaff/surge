use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};

use crate::theme;

/// PR status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

impl PrStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Merged => "Merged",
            Self::Closed => "Closed",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Open => theme::success(),
            Self::Merged => theme::primary(),
            Self::Closed => theme::error(),
        }
    }
}

/// CI check status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiStatus {
    Passing,
    Failing,
    Pending,
}

impl CiStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Passing => "CI Passing",
            Self::Failing => "CI Failing",
            Self::Pending => "CI Pending",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Passing => "✓",
            Self::Failing => "✗",
            Self::Pending => "○",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Passing => theme::success(),
            Self::Failing => theme::error(),
            Self::Pending => theme::warning(),
        }
    }
}

/// Review status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStatus {
    Approved,
    ChangesRequested,
    Pending,
    None,
}

impl ReviewStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Approved => "Approved",
            Self::ChangesRequested => "Changes Requested",
            Self::Pending => "Review Pending",
            Self::None => "No Reviews",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Approved => theme::success(),
            Self::ChangesRequested => theme::warning(),
            Self::Pending => theme::text_muted(),
            Self::None => theme::text_muted(),
        }
    }
}

/// A pull request entry.
#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub branch: String,
    pub status: PrStatus,
    pub ci: CiStatus,
    pub review: ReviewStatus,
    pub author: String,
    pub created: String,
    pub additions: u32,
    pub deletions: u32,
}

/// GitHub PRs screen.
pub struct GithubPrsScreen {
    prs: Vec<PullRequest>,
}

impl GithubPrsScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self { prs: demo_prs() }
    }

    fn render_pr_card(&self, idx: usize, pr: &PullRequest) -> Div {
        div()
            .v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.1))
            // Header: number + title + status
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(format!("#{}", pr.number)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(pr.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(pr.status.color().opacity(0.15))
                            .text_color(pr.status.color())
                            .child(pr.status.label().to_string()),
                    ),
            )
            // Branch + author + date
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::primary())
                            .font_family("monospace")
                            .child(pr.branch.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(format!("by {}", pr.author)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.6))
                            .child(pr.created.clone()),
                    ),
            )
            // Badges row: CI + Review + +/- counts
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    // CI badge
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .items_center()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(pr.ci.color().opacity(0.1))
                            .text_color(pr.ci.color())
                            .child(pr.ci.icon().to_string())
                            .child(pr.ci.label().to_string()),
                    )
                    // Review badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(pr.review.color().opacity(0.1))
                            .text_color(pr.review.color())
                            .child(pr.review.label().to_string()),
                    )
                    // Spacer
                    .child(div().flex_1())
                    // +/- counts
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::success())
                            .child(format!("+{}", pr.additions)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::error())
                            .child(format!("-{}", pr.deletions)),
                    ),
            )
            // Actions
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::text_muted().opacity(0.05))
                    .when(pr.status == PrStatus::Open, |el: Div| {
                        el.child(
                            Button::new(SharedString::from(format!("pr-merge-{idx}")))
                                .primary()
                                .label("Merge"),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("pr-diff-{idx}")))
                            .ghost()
                            .label("View Diff"),
                    )
                    .child(
                        Button::new(SharedString::from(format!("pr-github-{idx}")))
                            .ghost()
                            .label("View on GitHub"),
                    ),
            )
    }
}

impl Render for GithubPrsScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let open_count = self
            .prs
            .iter()
            .filter(|p| p.status == PrStatus::Open)
            .count();
        let merged_count = self
            .prs
            .iter()
            .filter(|p| p.status == PrStatus::Merged)
            .count();

        let cards: Vec<Div> = self
            .prs
            .iter()
            .enumerate()
            .map(|(idx, pr)| self.render_pr_card(idx, pr))
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
                                    .text_color(theme::text_primary())
                                    .child("GitHub PRs".to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_muted())
                                    .child(format!("{open_count} open  {merged_count} merged")),
                            ),
                    )
                    .child(Button::new("pr-create").primary().label("+ Create PR")),
            )
            // PR list
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .gap_3()
                    .overflow_hidden()
                    .children(cards),
            )
    }
}

fn demo_prs() -> Vec<PullRequest> {
    vec![
        PullRequest {
            number: 42,
            title: "feat: add JWT auth middleware".into(),
            branch: "surge/auth-middleware".into(),
            status: PrStatus::Open,
            ci: CiStatus::Passing,
            review: ReviewStatus::Approved,
            author: "surge-bot".into(),
            created: "2h ago".into(),
            additions: 148,
            deletions: 22,
        },
        PullRequest {
            number: 41,
            title: "refactor: extract DB connection pool".into(),
            branch: "surge/db-refactor".into(),
            status: PrStatus::Open,
            ci: CiStatus::Failing,
            review: ReviewStatus::ChangesRequested,
            author: "surge-bot".into(),
            created: "5h ago".into(),
            additions: 312,
            deletions: 198,
        },
        PullRequest {
            number: 40,
            title: "ci: update GitHub Actions workflow".into(),
            branch: "surge/ci-pipeline".into(),
            status: PrStatus::Open,
            ci: CiStatus::Pending,
            review: ReviewStatus::Pending,
            author: "surge-bot".into(),
            created: "1d ago".into(),
            additions: 45,
            deletions: 12,
        },
        PullRequest {
            number: 39,
            title: "feat: add rate limiting to API".into(),
            branch: "surge/rate-limiting".into(),
            status: PrStatus::Merged,
            ci: CiStatus::Passing,
            review: ReviewStatus::Approved,
            author: "surge-bot".into(),
            created: "2d ago".into(),
            additions: 89,
            deletions: 4,
        },
        PullRequest {
            number: 38,
            title: "fix: correct logging format".into(),
            branch: "surge/fix-logging".into(),
            status: PrStatus::Closed,
            ci: CiStatus::Passing,
            review: ReviewStatus::None,
            author: "surge-bot".into(),
            created: "3d ago".into(),
            additions: 8,
            deletions: 8,
        },
    ]
}
