//! Shared, reusable UI building blocks + the design-system layer screens
//! compose with.
//!
//! These exist to kill the per-screen duplication that had crept in
//! (`dashboard.card`, `agent_hub.section_card`, repeated `status_dot` /
//! `info_item` / `usage_bar` / `stat_card_with_period` copies, and ad-hoc
//! "coming soon" placeholders). Everything here renders through the
//! semantic color roles in [`crate::theme`], so a theme change is felt
//! everywhere automatically.
//!
//! Conventions:
//! - Free functions returning [`Div`] (or [`Stateful<Div>`] when they need
//!   an interactivity id) — mirrors the helpers already living in
//!   `agent_hub.rs`. No new entities / state; these are pure render helpers.
//! - Every color parameter is an [`Hsla`] from the caller so screens keep
//!   deciding *which* semantic color a thing is; this module only fixes the
//!   *shape* and the structural styling.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{Icon, IconName, StyledExt};

use crate::theme;

// ── Containers ───────────────────────────────────────────────────────

/// Titled surface card: header (title + optional subtitle) over `content`.
///
/// Drop-in for the old `DashboardScreen::card` helper. The card has a
/// default-strength border and the surface fill; the header uses a semibold
/// title and a muted subtitle. Pass `""` for subtitle to omit it.
pub fn card(title: &str, subtitle: &str, content: Div) -> Div {
    div()
        .v_flex()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(theme::surface_raised())
        .border_1()
        .border_color(theme::border_default())
        .child(
            div()
                .v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_primary())
                        .child(title.to_string()),
                )
                .when(!subtitle.is_empty(), |el: Div| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(subtitle.to_string()),
                    )
                }),
        )
        .child(content)
}

/// Headerless surface panel — just the bordered/rounded container. Drop-in
/// for the old `agent_hub::section_card`.
pub fn panel(content: Div) -> Div {
    content
        .rounded_lg()
        .bg(theme::surface_raised())
        .border_1()
        .border_color(theme::border_subtle())
        .overflow_hidden()
}

/// Small uppercase-style label that sits above a section. Drop-in for the
/// old `agent_hub::section_label`.
pub fn section_label(title: &str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme::text_muted().opacity(0.6))
        .pt(px(4.0))
        .child(title.to_string())
}

// ── Atoms ───────────────────────────────────────────────────────────

/// Filled status dot. Used in dashboards, agent rows, gate pills.
pub fn status_dot(color: Hsla, size_px: f32) -> Div {
    div().w(px(size_px)).h(px(size_px)).rounded_full().bg(color)
}

/// Filled accent chip — colored background tint + label. Used for agent
/// badges, task-state pills, vendor tags.
pub fn badge(label: &str, color: Hsla) -> Div {
    div()
        .text_xs()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(color.opacity(0.15))
        .text_color(color)
        .font_weight(FontWeight::BOLD)
        .child(label.to_string())
}

/// Outline chip with an enabled/disabled check glyph — for permission /
/// capability toggles. Drop-in for the old agent-hub permission chip.
pub fn toggle_chip(label: &str, enabled: bool) -> Div {
    let color = if enabled {
        theme::success()
    } else {
        theme::text_muted()
    };
    div()
        .h_flex()
        .gap(px(4.0))
        .items_center()
        .px(px(8.0))
        .py(px(4.0))
        .rounded_md()
        .bg(if enabled {
            theme::success().opacity(0.06)
        } else {
            theme::text_muted().opacity(0.03)
        })
        .border_1()
        .border_color(if enabled {
            theme::success().opacity(0.12)
        } else {
            theme::text_muted().opacity(0.05)
        })
        .child(
            div()
                .text_xs()
                .text_color(if enabled {
                    theme::success()
                } else {
                    theme::text_muted().opacity(0.3)
                })
                .child(if enabled { "✓" } else { "✕" }),
        )
        .child(
            div()
                .text_xs()
                .text_color(if enabled {
                    theme::text_primary()
                } else {
                    theme::text_muted().opacity(0.4)
                })
                .child(label.to_string()),
        )
}

/// Label/value pair stacked. Drop-in for `agent_hub::info_item`.
pub fn info_item(label: &str, value: &str) -> Div {
    div()
        .v_flex()
        .gap(px(2.0))
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted().opacity(0.5))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_primary())
                .child(value.to_string()),
        )
}

// ── Bars ─────────────────────────────────────────────────────────────

/// Thin determinate progress bar. `pct` is clamped to `[0,1]`.
pub fn progress_bar(pct: f32, color: Hsla, height_px: f32) -> Div {
    div()
        .w_full()
        .h(px(height_px))
        .rounded_full()
        .bg(theme::text_muted().opacity(0.1))
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(color)
                .w(relative(pct.clamp(0.0, 1.0))),
        )
}

/// Labeled usage bar with a caption row. Drop-in for the old
/// `agent_hub::usage_bar`.
pub fn usage_bar(label: &str, detail: &str, pct: f32, color: Hsla) -> Div {
    div()
        .v_flex()
        .gap(px(4.0))
        .child(
            div()
                .h_flex()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted().opacity(0.6))
                        .child(detail.to_string()),
                ),
        )
        .child(progress_bar(pct, color, 5.0))
}

/// Compact stat tile: label + big value + optional period caption. Drop-in
/// for the old `agent_hub::stat_card_with_period`.
pub fn stat_card(label: &str, value: &str, period: &str, color: Hsla) -> Div {
    div()
        .flex_1()
        .v_flex()
        .gap(px(3.0))
        .p(px(10.0))
        .rounded_lg()
        .bg(theme::surface_raised())
        .border_1()
        .border_color(theme::border_subtle())
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child(label.to_string()),
        )
        .child(
            div()
                .h_flex()
                .gap(px(4.0))
                .items_end()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .child(value.to_string()),
                )
                .when(!period.is_empty(), |el: Div| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.4))
                            .pb(px(1.0))
                            .child(period.to_string()),
                    )
                }),
        )
}

// ── States ───────────────────────────────────────────────────────────

/// Centered empty state: muted icon + title + subtitle + optional action.
/// Replaces the ad-hoc "coming soon" / "select an agent" / "Benchmarks —
/// Phase 7" placeholders scattered across screens.
pub fn empty_state(icon: IconName, title: &str, subtitle: &str, action: Option<Div>) -> Div {
    div()
        .flex_1()
        .v_flex()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            Icon::new(icon)
                .size_8()
                .text_color(theme::text_muted().opacity(0.15)),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::text_muted().opacity(0.4))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted().opacity(0.3))
                .child(subtitle.to_string()),
        )
        .when_some(action, |el: Div, a: Div| el.child(a))
}

/// Loading placeholder. Kept deliberately quiet (no spinner animation yet —
/// gpui animation is a follow-up); the point is a consistent shape so screens
/// don't each hand-roll a "loading…" string.
pub fn loading_state(text: &str) -> Div {
    div()
        .flex_1()
        .v_flex()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            Icon::new(IconName::Loader)
                .size_6()
                .text_color(theme::text_muted().opacity(0.25)),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::text_muted().opacity(0.4))
                .child(text.to_string()),
        )
}

/// Error state with optional retry affordance.
pub fn error_state(text: &str, retry: Option<Div>) -> Div {
    div()
        .flex_1()
        .v_flex()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            Icon::new(IconName::TriangleAlert)
                .size_8()
                .text_color(theme::error().opacity(0.5)),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::error())
                .child(text.to_string()),
        )
        .when_some(retry, |el: Div, r: Div| el.child(r))
}

/// Keyboard shortcut hint chip (e.g. "Ctrl+K"). Drop-in for the inline
/// muted-text shortcuts in the sidebar / top bar.
pub fn kbd(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::text_muted().opacity(0.5))
        .child(text.to_string())
}

// ── Layout / framing ────────────────────────────────────────────────

/// The big screen title block (`text_2xl` bold). Every screen header is
/// either [`title`] alone, [`title`] + a right action, or
/// [`title_with_meta`] + a right action — all composed inside
/// [`header_row`].
pub fn title(text: &str) -> Div {
    div()
        .text_2xl()
        .font_weight(FontWeight::BOLD)
        .text_color(theme::text_primary())
        .child(text.to_string())
}

/// Standard screen header row: `left` (a title / [`title_with_meta`] /
/// anything) pinned left, optional `right` (buttons / a filter bar /
/// controls) pinned right via `justify_between`. Replaces the per-screen
/// `div().h_flex().justify_between().items_center().child(...).child(...)`
/// block that 7 screens hand-rolled.
pub fn header_row(left: Div, right: Option<Div>) -> Div {
    div()
        .h_flex()
        .justify_between()
        .items_center()
        .child(left)
        .when_some(right, |el: Div, r: Div| el.child(r))
}

/// A thin horizontal rule in the subtle border color.
pub fn divider() -> Div {
    div().w_full().h(px(1.0)).bg(theme::border_subtle())
}

/// Title with an inline meta caption on the same row (`gap_3`): the
/// big title on the left, a small muted `meta` div (a stat line, file
/// counts, worktree size) next to it. Distinct from [`screen_header`],
/// which stacks a subtitle *under* the title and pins actions to the
/// right edge. Used by diff_viewer / github_prs / worktrees headers.
pub fn title_with_meta(title: &str, meta: Div) -> Div {
    div()
        .h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_primary())
                .child(title.to_string()),
        )
        .child(meta)
}

// ── Inline atoms ───────────────────────────────────────────────────

/// Horizontal label/value row (label muted left, value primary right).
/// Drop-in for the old unused `agent_hub::kv_row` and the ad-hoc
/// justify-between label/value rows in settings / insights.
pub fn kv_row(label: &str, value: &str) -> Div {
    div()
        .h_flex()
        .justify_between()
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_primary())
                .child(value.to_string()),
        )
}

/// Inline icon + caption row, e.g. a "calendar · 2h ago" meta line under a
/// card. `color` tints the icon; text stays muted.
pub fn meta_item(icon: IconName, text: &str, color: Hsla) -> Div {
    div()
        .h_flex()
        .gap(px(4.0))
        .items_center()
        .child(Icon::new(icon).size_3().text_color(color))
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted().opacity(0.6))
                .child(text.to_string()),
        )
}

/// Small muted count pill, e.g. the "3" next to a column header.
pub fn count_pill(n: usize) -> Div {
    div()
        .text_xs()
        .text_color(theme::text_muted())
        .child(n.to_string())
}

/// Colored tag chip (kanban tags, task badges). Slightly chunkier than
/// [`badge`] (rounded-4, px-6/py-2) — the kanban-card tag geometry.
pub fn tag(label: &str, color: Hsla) -> Div {
    div()
        .text_xs()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(color.opacity(0.15))
        .text_color(color)
        .child(label.to_string())
}

/// Row of `total` small dots; the first `done` are filled `color`, the rest
/// are a faint track. Used for subtask progress on kanban cards (and later
/// on DAG nodes). Caps the rendered dots at `max` and appends a "+N" tail
/// when `total > max`.
pub fn dots(done: usize, total: usize, color: Hsla, size_px: f32, max: usize) -> Div {
    let shown = total.min(max);
    let dot_els: Vec<Div> = (0..shown)
        .map(|i| {
            div()
                .w(px(size_px))
                .h(px(size_px))
                .rounded_full()
                .bg(if i < done {
                    color
                } else {
                    theme::text_muted().opacity(0.2)
                })
        })
        .collect();
    let extra = total.saturating_sub(max);
    div()
        .h_flex()
        .gap(px(3.0))
        .items_center()
        .children(dot_els)
        .when(extra > 0, |el: Div| {
            el.child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(format!("+{extra}")),
            )
        })
}

// ── Interactive atoms ───────────────────────────────────────────────
//
// These take an `on_click` handler so screens can wire them to
// `cx.listener(...)`. The handler bound matches what gpui's `.on_click`
// expects, so `cx.listener(|this, _e, _w, cx| ...)` drops in directly.

/// Round, pill-shaped selectable chip — filter rows, segmented selectors.
/// Active = accent-tinted fill + accent text; inactive = ghost.
pub fn filter_chip(
    id: impl Into<SharedString>,
    label: &str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .px(px(8.0))
        .py(px(4.0))
        .rounded_full()
        .cursor_pointer()
        .text_xs()
        .bg(if active {
            theme::accent_subtle()
        } else {
            gpui::transparent_black()
        })
        .text_color(if active {
            theme::primary()
        } else {
            theme::text_muted()
        })
        .hover(|s: StyleRefinement| s.bg(theme::primary().opacity(0.06)))
        .on_click(on_click)
        .child(label.to_string())
}

/// Rectangular tab pill (top-of-screen tab bars). Slightly taller than
/// [`filter_chip`], with a bolder active state — matches the agent-hub
/// tab style.
pub fn tab_pill(
    id: impl Into<SharedString>,
    label: &str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .px_3()
        .py(px(5.0))
        .cursor_pointer()
        .rounded_md()
        .text_xs()
        .font_weight(if active {
            FontWeight::BOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(if active {
            theme::primary()
        } else {
            theme::text_muted()
        })
        .bg(if active {
            theme::accent_subtle()
        } else {
            gpui::transparent_black()
        })
        .hover(|s: StyleRefinement| s.bg(theme::primary().opacity(0.05)))
        .on_click(on_click)
        .child(label.to_string())
}

// ── Observatory / run views ─────────────────────────────────────────

/// Status pill for a run/task — a filled dot + colored label in a tinted
/// capsule. Heavier than [`badge`]; reads as a live status indicator.
pub fn status_pill(label: &str, color: Hsla) -> Div {
    div()
        .h_flex()
        .gap(px(5.0))
        .items_center()
        .px(px(8.0))
        .py(px(3.0))
        .rounded_full()
        .bg(color.opacity(0.1))
        .child(status_dot(color, 6.0))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(label.to_string()),
        )
}

/// A KPI tile: small muted label over a large value. Wider/flatter than
/// [`stat_card`] (no border by default) — for the run-detail KPI strip.
/// `color` tints the value.
pub fn kpi_tile(label: &str, value: &str, color: Hsla) -> Div {
    div()
        .flex_1()
        .v_flex()
        .gap(px(3.0))
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted().opacity(0.6))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(color)
                .child(value.to_string()),
        )
}

/// Header for a pane inside a multi-pane layout: a small bold title with an
/// optional trailing element (count, action) pushed to the right.
pub fn pane_header(title: &str, trailing: Option<Div>) -> Div {
    div()
        .h_flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_primary())
                .child(title.to_string()),
        )
        .when_some(trailing, |el: Div, t: Div| el.child(t))
}

/// One row in an event/reasoning timeline: a colored dot in a vertical
/// rail, then a timestamp gutter, then the message. Composable into a
/// scrolling column. `color` tints the dot (and, faintly, the rail).
pub fn timeline_row(timestamp: &str, message: &str, color: Hsla) -> Div {
    div()
        .h_flex()
        .gap(px(8.0))
        .items_start()
        .py(px(3.0))
        // Rail + dot
        .child(
            div()
                .flex_shrink_0()
                .w(px(10.0))
                .h_flex()
                .justify_center()
                .pt(px(4.0))
                .child(status_dot(color, 6.0)),
        )
        // Timestamp gutter
        .child(
            div()
                .flex_shrink_0()
                .w(px(64.0))
                .text_xs()
                .text_color(theme::text_muted().opacity(0.5))
                .child(timestamp.to_string()),
        )
        // Message
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(theme::text_primary().opacity(0.9))
                .line_height(relative(1.4))
                .child(message.to_string()),
        )
}
