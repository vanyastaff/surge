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

use gpui_kit::*;

use crate::theme;

/// Native sans-serif family for prose, controls, and navigation.
#[cfg(target_os = "macos")]
pub const BODY: &str = "Helvetica Neue";
#[cfg(target_os = "windows")]
pub const BODY: &str = "Segoe UI";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const BODY: &str = "sans-serif";

/// Monospace family for code, identifiers, and terminal output.
///
/// A system face on each OS, so nothing is downloaded or bundled: Menlo
/// ships with every macOS, Consolas with every Windows. On Linux the name
/// is resolved against fontconfig; "JetBrainsMono Nerd Font" is named
/// explicitly because a plain "JetBrains Mono" falls back to Noto.
#[cfg(target_os = "macos")]
pub const MONO: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const MONO: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO: &str = "JetBrainsMono Nerd Font";

/// First line of free text, cut to `max` characters with an ellipsis —
/// how a run's request becomes a list/card headline.
pub fn headline(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Render a shortcut written as `Ctrl+K` / `Ctrl+Shift+P` the way this
/// platform spells it: `⌘K` / `⌘⇧P` on macOS (the bindings use the
/// platform modifier), unchanged elsewhere.
pub fn shortcut_label(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.replace("Ctrl+", "⌘").replace("Shift+", "⇧")
    } else {
        keys.to_string()
    }
}

/// Corner radii (Archify: precise / control / panel).
pub const R_PRECISE: f32 = 3.0;
pub const R_CONTROL: f32 = 6.0;
pub const R_PANEL: f32 = 12.0;

/// A small round status dot.
pub fn status_dot(color: Hsla) -> Div {
    div()
        .w(px(7.0))
        .h(px(7.0))
        .rounded_full()
        .bg(color)
        .flex_shrink_0()
}

/// A status dot with a soft halo — for *live* state only (running,
/// connected). Idle state uses [`status_dot`]; the halo must mean something.
pub fn live_dot(color: Hsla) -> Div {
    div()
        .size(px(13.0))
        .rounded_full()
        .bg(color.opacity(0.16))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(status_dot(color))
}

/// A bordered capsule label (Archify badge): uppercase, bold, the owning
/// semantic color on its tint. `fg` is the meaning; `bg` the fill.
pub fn pill(text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    let text: SharedString = text.into();
    div()
        .flex_none()
        .px(px(7.0))
        .py(px(1.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(fg.opacity(0.38))
        .text_color(fg)
        .text_size(px(11.5))
        .font_weight(FontWeight::BOLD)
        .child(SharedString::from(text.to_uppercase()))
}

/// A badge in a semantic role's color.
pub fn role_badge(text: impl Into<SharedString>, role: theme::Semantic) -> Div {
    let color = role.color();
    pill(text, color, theme::tint(color))
}

/// A keycap hint (⌘K, ↵, 1–5…).
pub fn kbd(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(R_PRECISE))
        .border_1()
        .border_color(theme::hairline_strong())
        .bg(theme::panel_deep())
        .text_color(theme::text_muted())
        .font_family(MONO)
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A quiet uppercase section micro-header (Archify "label" tier).
pub fn section_label(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .text_color(theme::text_dim())
        .text_size(px(12.0))
        .font_weight(FontWeight::BOLD)
        .child(SharedString::from(text.to_uppercase()))
}

/// Muted, tabular meta text.
pub fn meta(text: impl Into<SharedString>) -> Div {
    div()
        .text_color(theme::text_muted())
        .text_size(px(13.0))
        .child(text.into())
}

/// Base panel / card surface — tonal fill, hairline border, panel radius.
/// Flat at rest: no shadow unless it floats. Caller adds padding.
pub fn panel() -> Div {
    div()
        .bg(theme::panel_raised())
        .border_1()
        .border_color(theme::hairline())
        .rounded(px(R_PANEL))
}

/// A semantic node card: the role color as a faint tint with a saturated
/// border, exactly how a diagram node reads. Caller adds content.
pub fn node_card(color: Hsla) -> Div {
    div()
        .border_1()
        .border_color(theme::stroke(color))
        .rounded(px(R_CONTROL + 2.0))
        .bg(theme::tint(color))
}

/// Screen header: a title, an optional one-line purpose, and a right-hand
/// slot for actions. Every top-level screen opens with one so the app reads
/// as one product.
pub fn page_header(
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    actions: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .pb(px(14.0))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(24.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child(title.into()),
                )
                .children(subtitle.map(|sub| {
                    div()
                        .text_size(px(14.0))
                        .text_color(theme::text_muted())
                        .child(sub)
                })),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(8.0))
                .items_center()
                .child(actions),
        )
}

/// An honest empty state: what this place is for, and what fills it.
pub fn empty_state(
    glyph: &'static str,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .p(px(32.0))
        .child(
            div()
                .size(px(40.0))
                .rounded(px(R_PANEL))
                .border_1()
                .border_color(theme::hairline_strong())
                .bg(theme::panel_raised())
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::text_muted())
                .text_size(px(16.0))
                .child(glyph),
        )
        .child(
            div()
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_primary())
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(420.0))
                .text_center()
                .text_size(px(14.0))
                .line_height(px(21.0))
                .text_color(theme::text_muted())
                .child(body.into()),
        )
}

/// One legend entry: a swatch in the role's node style, its name, and an
/// optional count.
pub fn legend_chip(role: theme::Semantic, count: Option<usize>) -> Div {
    let color = role.color();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .w(px(14.0))
                .h(px(10.0))
                .rounded(px(R_PRECISE))
                .bg(theme::tint(color))
                .border_1()
                .border_color(theme::stroke(color)),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme::text_muted())
                .child(role.label()),
        )
        .children(count.map(|n| {
            div()
                .px(px(5.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(theme::hairline_strong())
                .text_size(px(9.5))
                .text_color(theme::text_dim())
                .child(n.to_string())
        }))
}

/// Blueprint grid behind a canvas: faint lines every `step` px with a
/// stronger line every fourth. Absolute, fills its (relative) parent.
pub fn grid_backdrop(step: f32) -> impl IntoElement {
    let minor = theme::grid();
    let major = theme::grid().opacity((theme::grid().a * 2.0).min(1.0));
    canvas(
        |_bounds, _window, _cx| {},
        move |bounds, _prepaint, window, _cx| {
            let ox = bounds.origin.x;
            let oy = bounds.origin.y;
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let mut i = 0_u32;
            let mut x = 0.0_f32;
            while x < w {
                let color = if i.is_multiple_of(4) { major } else { minor };
                window.paint_quad(fill(
                    Bounds::new(point(ox + px(x), oy), size(px(1.0), px(h))),
                    color,
                ));
                x += step;
                i += 1;
            }
            i = 0;
            let mut y = 0.0_f32;
            while y < h {
                let color = if i.is_multiple_of(4) { major } else { minor };
                window.paint_quad(fill(
                    Bounds::new(point(ox, oy + px(y)), size(px(w), px(1.0))),
                    color,
                ));
                y += step;
                i += 1;
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The Surge mark: a signal ring around a live dot, in the accent — the
/// same "one thing is happening here" dot Archify puts before a title.
pub fn brand_mark(size_px: f32) -> Div {
    let accent = theme::accent();
    div()
        .size(px(size_px))
        .flex_none()
        .rounded_full()
        .border(px((size_px / 9.0).max(1.5)))
        .border_color(accent)
        .bg(accent.opacity(0.14))
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(px(size_px * 0.36)).rounded_full().bg(accent))
}

/// "5m ago" / "3h ago" / "2d ago" for a unix-seconds timestamp string.
/// Unparseable or future input yields `None` rather than a made-up age.
pub fn relative_age(unix_secs: &str) -> Option<String> {
    let then: u64 = unix_secs.trim().parse().ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let secs = now.checked_sub(then)?;
    Some(match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        86_400..=2_591_999 => format!("{}d ago", secs / 86_400),
        _ => format!("{}mo ago", secs / 2_592_000),
    })
}

/// `/Users/me/Develop/app` → `~/Develop/app`; other paths unchanged.
pub fn abbreviate_home(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .and_then(|home| path.strip_prefix(&home).ok().map(|rest| rest.to_path_buf()))
        .map_or_else(
            || path.display().to_string(),
            |rest| format!("~/{}", rest.display()),
        )
}

/// The checked-out branch of the repository containing `path`, or `None`
/// for a detached HEAD, an unborn branch, or no repository at all.
pub fn current_branch(path: &std::path::Path) -> Option<String> {
    let repo = git2::Repository::discover(path).ok()?;
    let head = repo.head().ok()?;
    head.is_branch()
        .then(|| head.shorthand().map(str::to_string))
        .flatten()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that pulls in gpui's `test` attribute over std's.
    use super::{relative_age, shortcut_label};

    #[test]
    fn relative_age_rejects_garbage_and_future() {
        assert_eq!(relative_age("not a time"), None);
        assert_eq!(relative_age("99999999999"), None);
    }

    #[test]
    fn relative_age_buckets() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        assert_eq!(relative_age(&now.to_string()).as_deref(), Some("just now"));
        assert_eq!(
            relative_age(&(now - 7_200).to_string()).as_deref(),
            Some("2h ago")
        );
        assert_eq!(
            relative_age(&(now - 3 * 86_400).to_string()).as_deref(),
            Some("3d ago")
        );
    }

    #[test]
    fn shortcut_label_follows_platform() {
        let label = shortcut_label("Ctrl+Shift+P");
        if cfg!(target_os = "macos") {
            assert_eq!(label, "⌘⇧P");
        } else {
            assert_eq!(label, "Ctrl+Shift+P");
        }
    }
}
