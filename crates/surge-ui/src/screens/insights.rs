use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

/// Time period selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::All => "All Time",
        }
    }

    fn all() -> &'static [Period] {
        &[Self::Today, Self::Week, Self::Month, Self::All]
    }
}

/// Token usage for a single agent.
#[derive(Debug, Clone)]
pub struct AgentUsage {
    pub name: String,
    pub requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// QA metrics.
#[derive(Debug, Clone)]
pub struct QaMetrics {
    pub total_tasks: u32,
    pub first_pass_rate: f32,
    pub avg_iterations: f32,
    pub avg_duration_min: f32,
}

/// Insights/Analytics screen.
pub struct InsightsScreen {
    period: Period,
    agent_usage: Vec<AgentUsage>,
    qa_metrics: QaMetrics,
}

impl InsightsScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            period: Period::Week,
            agent_usage: demo_usage(),
            qa_metrics: QaMetrics {
                total_tasks: 24,
                first_pass_rate: 0.71,
                avg_iterations: 1.4,
                avg_duration_min: 3.2,
            },
        }
    }

    fn total_cost(&self) -> f64 {
        self.agent_usage.iter().map(|a| a.cost_usd).sum()
    }

    fn total_tokens(&self) -> u64 {
        self.agent_usage.iter().map(|a| a.input_tokens + a.output_tokens).sum()
    }

    fn render_period_selector(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = Period::all()
            .iter()
            .map(|&p| {
                let is_active = p == self.period;
                div()
                    .id(SharedString::from(format!("period-{}", p.label())))
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .text_sm()
                    .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                    .bg(if is_active { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.period = p;
                        cx.notify();
                    }))
                    .child(p.label().to_string())
            })
            .collect();

        div().h_flex().gap_1().children(tabs)
    }

    fn render_summary_cards(&self) -> Div {
        let total_requests: u32 = self.agent_usage.iter().map(|a| a.requests).sum();

        div()
            .h_flex()
            .gap_4()
            .child(self.stat_card("Total Tokens", &format_tokens(self.total_tokens())))
            .child(self.stat_card("Total Cost", &format!("${:.2}", self.total_cost())))
            .child(self.stat_card("Requests", &format!("{total_requests}")))
            .child(self.stat_card("Tasks Completed", &format!("{}", self.qa_metrics.total_tasks)))
    }

    fn stat_card(&self, label: &str, value: &str) -> Div {
        div()
            .flex_1()
            .v_flex()
            .gap_1()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(value.to_string()),
            )
    }

    fn render_usage_table(&self) -> Div {
        // Header row
        let header = div()
            .h_flex()
            .gap_2()
            .px_3()
            .py(px(8.0))
            .border_b_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(div().w(px(140.0)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED).child("Agent".to_string()))
            .child(div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED).child("Requests".to_string()))
            .child(div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED).child("Input Tokens".to_string()))
            .child(div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED).child("Output Tokens".to_string()))
            .child(div().w(px(80.0)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED).child("Cost".to_string()));

        let rows: Vec<Div> = self
            .agent_usage
            .iter()
            .map(|agent| {
                div()
                    .h_flex()
                    .gap_2()
                    .px_3()
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.05))
                    .child(
                        div()
                            .w(px(140.0))
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(agent.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(format!("{}", agent.requests)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(format_tokens(agent.input_tokens)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(format_tokens(agent.output_tokens)),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(format!("${:.2}", agent.cost_usd)),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Token Usage by Agent".to_string()),
            )
            .child(
                div()
                    .v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .bg(theme::SURFACE)
                    .overflow_hidden()
                    .child(header)
                    .children(rows),
            )
    }

    fn render_qa_metrics(&self) -> Div {
        div()
            .v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("QA Metrics".to_string()),
            )
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(self.metric_row(
                        "First-Pass Rate",
                        &format!("{:.0}%", self.qa_metrics.first_pass_rate * 100.0),
                        self.qa_metrics.first_pass_rate,
                    ))
                    .child(self.metric_row(
                        "Avg Iterations",
                        &format!("{:.1}", self.qa_metrics.avg_iterations),
                        1.0 - (self.qa_metrics.avg_iterations - 1.0) / 3.0, // normalize
                    ))
                    .child(self.metric_row(
                        "Avg Duration",
                        &format!("{:.1} min", self.qa_metrics.avg_duration_min),
                        1.0 - (self.qa_metrics.avg_duration_min / 10.0), // normalize
                    )),
            )
    }

    fn metric_row(&self, label: &str, value: &str, ratio: f32) -> Div {
        let bar_color = if ratio > 0.7 {
            theme::SUCCESS
        } else if ratio > 0.4 {
            theme::WARNING
        } else {
            theme::ERROR
        };

        div()
            .v_flex()
            .gap_1()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_MUTED)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(value.to_string()),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme::TEXT_MUTED.opacity(0.15))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(bar_color)
                            .w(relative(ratio.clamp(0.0, 1.0))),
                    ),
            )
    }
}

impl Render for InsightsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .child("Insights".to_string()),
                    )
                    .child(self.render_period_selector(cx)),
            )
            // Summary cards
            .child(self.render_summary_cards())
            // Content: table + QA metrics side by side
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap_4()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .child(self.render_usage_table()),
                    )
                    .child(
                        div()
                            .w(px(300.0))
                            .child(self.render_qa_metrics()),
                    ),
            )
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}

fn demo_usage() -> Vec<AgentUsage> {
    vec![
        AgentUsage {
            name: "claude-acp".into(),
            requests: 42,
            input_tokens: 856_000,
            output_tokens: 312_000,
            cost_usd: 4.82,
        },
        AgentUsage {
            name: "copilot-cli".into(),
            requests: 15,
            input_tokens: 124_000,
            output_tokens: 48_000,
            cost_usd: 0.86,
        },
        AgentUsage {
            name: "zed-agent".into(),
            requests: 8,
            input_tokens: 62_000,
            output_tokens: 24_000,
            cost_usd: 0.34,
        },
    ]
}
