//! Composite UI kit for the fleet-ops shell.
//!
//! Small, reusable presentational primitives adapted from the
//! "Surge - Interactive" design concept. Screens compose these instead
//! of re-deriving pill / dot / keycap / panel styling by hand, so the
//! chrome stays consistent as new surfaces land.
//!
//! Everything here is stateless: each fn returns a styled [`Div`] the
//! caller drops children into (or a finished atom). Interactivity is the
//! caller's job (wrap in `.id(..).on_click(..)`).

use gpui::*;

use crate::theme;

/// Monospace family for the whole shell.
///
/// Resolved against fontconfig — "JetBrainsMono Nerd Font" is installed
/// on this machine; a plain "JetBrains Mono" falls back to Noto, so we
/// name the Nerd Font explicitly. Applied once at the app root.
pub const MONO: &str = "JetBrainsMono Nerd Font";

/// A small round status dot.
pub fn status_dot(color: Hsla) -> Div {
    div()
        .w(px(7.0))
        .h(px(7.0))
        .rounded_full()
        .bg(color)
        .flex_shrink_0()
}

/// A rounded label chip. `fg`/`bg` carry the semantic color.
pub fn pill(text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    div()
        .px(px(7.0))
        .py(px(1.0))
        .rounded_md()
        .bg(bg)
        .text_color(fg)
        .text_size(px(10.0))
        .font_weight(FontWeight::BOLD)
        .child(text.into())
}

/// A keycap hint (⌘K, ↵, 1–5…).
pub fn kbd(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded_md()
        .border_1()
        .border_color(theme::hairline_strong())
        .text_color(theme::text_muted())
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A muted, letter-spaced section micro-header (rendered uppercase by
/// the caller's text; we only carry the size/weight/color).
pub fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_color(theme::text_muted())
        .text_size(px(9.0))
        .font_weight(FontWeight::BOLD)
        .child(text.into())
}

/// Muted, tabular meta text.
pub fn meta(text: impl Into<SharedString>) -> Div {
    div()
        .text_color(theme::text_muted())
        .text_size(px(10.0))
        .child(text.into())
}

/// Base panel / card surface — raised bg, hairline border, rounded.
/// Caller adds padding + children.
pub fn panel() -> Div {
    div()
        .bg(theme::panel_raised())
        .border_1()
        .border_color(theme::hairline())
        .rounded_lg()
}
