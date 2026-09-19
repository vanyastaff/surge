use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
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
            Self::Draft => theme::text_muted(),
            Self::Active => theme::warning(),
            Self::Completed => theme::success(),
            Self::Failed => theme::error(),
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
    state: Entity<AppState>,
    search_query: String,
    filter_status: Option<SpecStatus>,
}

impl SpecExplorerScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            search_query: String::new(),
            filter_status: None,
        }
    }

    /// Build SpecCards from AppState specs.
    fn build_specs(&self, cx: &Context<Self>) -> Vec<SpecCard> {
        let app_state = self.state.read(cx);
        app_state
            .specs
            .iter()
            .map(|spec| {
                let complexity = match spec.complexity {
                    surge_core::Complexity::Simple => "Simple",
                    surge_core::Complexity::Standard => "Standard",
                    surge_core::Complexity::Complex => "Complex",
                };
                SpecCard {
                    id: spec.id.to_string(),
                    title: spec.title.clone(),
                    description: spec.description.clone(),
                    status: SpecStatus::Draft, // All specs start as Draft; real status from tasks later.
                    complexity: complexity.to_string(),
                    subtask_count: spec.subtasks.len(),
                    agent: None,
                }
            })
            .collect()
    }

    fn filtered_specs<'a>(&self, specs: &'a [SpecCard]) -> Vec<&'a SpecCard> {
        specs
            .iter()
            .filter(|s| {
                if let Some(status) = self.filter_status
                    && s.status != status
                {
                    return false;
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
                    .bg(if is_active {
                        theme::primary().opacity(0.15)
                    } else {
                        theme::surface()
                    })
                    .text_color(if is_active {
                        theme::primary()
                    } else {
                        theme::text_muted()
                    })
                    .hover(|s: StyleRefinement| s.bg(theme::primary().opacity(0.1)))
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
            "Simple" => theme::success(),
            "Standard" => theme::warning(),
            "Complex" => theme::error(),
            _ => theme::text_muted(),
        };

        div()
            .id(SharedString::from(format!("spec-{}", spec.id)))
            .v_flex()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.1))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::primary().opacity(0.3)))
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
                    .text_color(theme::text_primary())
                    .child(spec.title.clone()),
            )
            // Description
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
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
                            .text_color(theme::text_muted())
                            .child(format!("{} subtasks", spec.subtask_count)),
                    )
                    .when(spec.agent.is_some(), |el: Div| {
                        let agent = spec.agent.as_deref().unwrap_or("");
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::primary())
                                .child(agent.to_string()),
                        )
                    }),
            )
    }
}

impl Render for SpecExplorerScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all_specs = self.build_specs(cx);
        let specs = self.filtered_specs(&all_specs);
        let is_empty = specs.is_empty() && all_specs.is_empty();

        let cards: Vec<Stateful<Div>> =
            specs.iter().map(|s| self.render_spec_card(s, cx)).collect();

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
                            .text_color(theme::text_primary())
                            .child("Specs".to_string()),
                    )
                    .child(Button::new("new-spec").primary().label("+ New Spec")),
            )
            // Filters
            .child(self.render_filter_chips(cx))
            // Grid or empty state
            .when(is_empty, |el: Div| {
                el.child(
                    div()
                        .flex_1()
                        .v_flex()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::File)
                                .size_8()
                                .text_color(theme::text_muted().opacity(0.3)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text_muted())
                                .child("No specs yet".to_string()),
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
                        .content_start()
                        .children(cards.into_iter().map(|card| div().w(px(280.0)).child(card))),
                )
            })
    }
}
