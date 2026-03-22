use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecStatus {
    Draft,
    Active,
    Completed,
    Failed,
}

impl SpecStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Active => "Active",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    fn color(self) -> Hsla {
        match self {
            Self::Draft => theme::TEXT_MUTED,
            Self::Active => theme::WARNING,
            Self::Completed => theme::SUCCESS,
            Self::Failed => theme::ERROR,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpecCard {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: SpecStatus,
    pub complexity: String,
    pub subtask_count: usize,
    pub agent: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct SpecClicked(pub String);

impl EventEmitter<SpecClicked> for SpecExplorerScreen {}

pub struct SpecExplorerScreen {
    specs: Vec<SpecCard>,
    search_query: String,
    filter_status: Option<SpecStatus>,
}

impl SpecExplorerScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            specs: vec![
                SpecCard {
                    id: "spec-01".into(),
                    title: "Auth Middleware".into(),
                    description: "Implement JWT-based auth for all API endpoints".into(),
                    status: SpecStatus::Active,
                    complexity: "Standard".into(),
                    subtask_count: 5,
                    agent: Some("claude-code".into()),
                },
                SpecCard {
                    id: "spec-02".into(),
                    title: "Database Migration".into(),
                    description: "Migrate from SQLite to PostgreSQL".into(),
                    status: SpecStatus::Draft,
                    complexity: "Complex".into(),
                    subtask_count: 8,
                    agent: None,
                },
                SpecCard {
                    id: "spec-03".into(),
                    title: "Fix Login Bug".into(),
                    description: "Session token not refreshing on mobile".into(),
                    status: SpecStatus::Completed,
                    complexity: "Simple".into(),
                    subtask_count: 2,
                    agent: Some("claude-code".into()),
                },
                SpecCard {
                    id: "spec-04".into(),
                    title: "Rate Limiting".into(),
                    description: "Add per-user rate limiting to public API".into(),
                    status: SpecStatus::Active,
                    complexity: "Standard".into(),
                    subtask_count: 4,
                    agent: Some("claude-code".into()),
                },
                SpecCard {
                    id: "spec-05".into(),
                    title: "CI Pipeline".into(),
                    description: "Setup GitHub Actions with test + lint + deploy".into(),
                    status: SpecStatus::Completed,
                    complexity: "Simple".into(),
                    subtask_count: 3,
                    agent: Some("claude-code".into()),
                },
                SpecCard {
                    id: "spec-06".into(),
                    title: "WebSocket Events".into(),
                    description: "Real-time notifications via WebSocket".into(),
                    status: SpecStatus::Draft,
                    complexity: "Complex".into(),
                    subtask_count: 0,
                    agent: None,
                },
            ],
            search_query: String::new(),
            filter_status: None,
        }
    }

    fn filtered_specs(&self) -> Vec<&SpecCard> {
        self.specs
            .iter()
            .filter(|s| {
                if let Some(status) = self.filter_status {
                    if s.status != status {
                        return false;
                    }
                }
                if !self.search_query.is_empty() {
                    let q = self.search_query.to_lowercase();
                    return s.title.to_lowercase().contains(&q)
                        || s.description.to_lowercase().contains(&q);
                }
                true
            })
            .collect()
    }

    fn render_filter_chips(&self, cx: &mut Context<Self>) -> Div {
        let statuses = [
            (None, "All"),
            (Some(SpecStatus::Draft), "Draft"),
            (Some(SpecStatus::Active), "Active"),
            (Some(SpecStatus::Completed), "Completed"),
            (Some(SpecStatus::Failed), "Failed"),
        ];

        let chips: Vec<Stateful<Div>> = statuses
            .iter()
            .map(|(status, label)| {
                let is_active = self.filter_status == *status;
                let s = *status;
                div()
                    .id(SharedString::from(format!("filter-{label}")))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .cursor_pointer()
                    .text_xs()
                    .bg(if is_active { theme::PRIMARY.opacity(0.15) } else { theme::SURFACE })
                    .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                    .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.1)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.filter_status = s;
                        cx.notify();
                    }))
                    .child(label.to_string())
            })
            .collect();

        div().h_flex().gap_2().children(chips)
    }

    fn render_spec_card(&self, spec: &SpecCard, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = spec.id.clone();
        let complexity_color = match spec.complexity.as_str() {
            "Simple" => theme::SUCCESS,
            "Standard" => theme::WARNING,
            "Complex" => theme::ERROR,
            _ => theme::TEXT_MUTED,
        };

        div()
            .id(SharedString::from(format!("spec-{}", spec.id)))
            .v_flex()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::PRIMARY.opacity(0.3)))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(SpecClicked(id.clone()));
            }))
            // Status + complexity
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(spec.status.color().opacity(0.15))
                            .text_color(spec.status.color())
                            .child(spec.status.label().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(complexity_color.opacity(0.1))
                            .text_color(complexity_color)
                            .child(spec.complexity.clone()),
                    ),
            )
            // Title
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(spec.title.clone()),
            )
            // Description
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(spec.description.clone()),
            )
            // Footer
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{} subtasks", spec.subtask_count)),
                    )
                    .when(spec.agent.is_some(), |el: Div| {
                        let agent = spec.agent.as_deref().unwrap_or("");
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::PRIMARY)
                                .child(agent.to_string()),
                        )
                    }),
            )
    }
}

impl Render for SpecExplorerScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let specs = self.filtered_specs();
        let cards: Vec<Stateful<Div>> = specs
            .iter()
            .map(|s| self.render_spec_card(s, cx))
            .collect();

        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
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
                            .child("Specs".to_string()),
                    )
                    .child(
                        Button::new("new-spec")
                            .primary()
                            .label("+ New Spec"),
                    ),
            )
            // Filters
            .child(self.render_filter_chips(cx))
            // Grid
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .content_start()
                    .children(
                        cards.into_iter().map(|card| {
                            div().w(px(280.0)).child(card)
                        }),
                    ),
            )
    }
}
