//! Native workbench design tokens: readable prose, quiet surfaces, and
//! semantic colors for progress, decisions, and evidence.
//!
//! Screens read tokens through the free functions below; the palette lives
//! in a thread-local so a theme switch re-colors every surface on the next
//! frame. [`sync_component_theme`] pushes the same palette into
//! gpui-component so its buttons, tabs and inputs speak the same language.

use std::cell::RefCell;
use std::path::PathBuf;

use gpui_kit::{App, Hsla};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct SurgeThemeColors {
    pub primary: Hsla,
    pub surface: Hsla,
    pub background: Hsla,
    pub sidebar_bg: Hsla,
    pub text_primary: Hsla,
    pub text_muted: Hsla,
    pub text_dim: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub panel: Hsla,
    pub panel_raised: Hsla,
    pub panel_deep: Hsla,
    pub hairline: Hsla,
    pub hairline_strong: Hsla,
    pub graph_line: Hsla,
    pub grid: Hsla,
    pub violet: Hsla,
    pub orange: Hsla,
    pub slate: Hsla,
    pub on_accent: Hsla,
    pub dark: bool,
}

impl SurgeThemeColors {
    fn dark(primary: Hsla) -> Self {
        Self {
            primary,
            background: rgb(0x212121),
            panel_deep: rgb(0x171717),
            panel: rgb(0x242424),
            sidebar_bg: rgb(0x171717),
            surface: rgb(0x303030),
            panel_raised: rgb(0x292929),
            hairline: rgb(0x383838),
            hairline_strong: rgb(0x505050),
            graph_line: rgb(0x7D7D7D),
            grid: rgba(0x94A3B8, 0.055),
            text_primary: rgb(0xF2F2F2),
            text_muted: rgb(0xBDBDBD),
            text_dim: rgb(0xAAAAAA),
            success: rgb(0x34D399),
            warning: rgb(0xFBBF24),
            error: rgb(0xFB7185),
            violet: rgb(0xA78BFA),
            orange: rgb(0xFB923C),
            slate: rgb(0x94A3B8),
            on_accent: rgb(0x171717),
            dark: true,
        }
    }

    fn light(primary: Hsla) -> Self {
        Self {
            primary,
            background: rgb(0xFFFFFF),
            panel_deep: rgb(0xF7F7F7),
            panel: rgb(0xF5F5F5),
            sidebar_bg: rgb(0xF5F5F5),
            surface: rgb(0xEBEBEB),
            panel_raised: rgb(0xFFFFFF),
            hairline: rgb(0xE2E2E2),
            hairline_strong: rgb(0xC5C6C8),
            graph_line: rgb(0x8A929D),
            grid: rgba(0x242424, 0.05),
            text_primary: rgb(0x20252D),
            text_muted: rgb(0x515B68),
            text_dim: rgb(0x626A77),
            success: rgb(0x147D54),
            warning: rgb(0xA85F08),
            error: rgb(0xC5384B),
            violet: rgb(0x7C3AED),
            orange: rgb(0xEA580C),
            slate: rgb(0x64748B),
            on_accent: rgb(0xFFFFFF),
            dark: false,
        }
    }
}

/// `0xRRGGBB` → [`Hsla`].
fn rgb(hex: u32) -> Hsla {
    gpui_kit::rgb(hex).into()
}

fn rgba(hex: u32, alpha: f32) -> Hsla {
    let mut color = rgb(hex);
    color.a = alpha;
    color
}

/// The accent a person picks in Settings. Only the accent changes; the
/// semantic vocabulary (agent / plan / verified / you / failure) is fixed so
/// meaning survives every theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeName {
    /// Verified Cyan — the Archify signal color.
    #[serde(alias = "Default")]
    Signal,
    /// Proof Green.
    #[serde(alias = "Verdant", alias = "Lime")]
    Proof,
    /// Repository Violet.
    #[serde(alias = "Neo")]
    Violet,
    /// Cloud Amber — the original Surge accent.
    #[serde(alias = "Dusk", alias = "Retro")]
    Amber,
    /// Ocean blue.
    Ocean,
    /// No hue at all.
    Monochrome,
}

impl ThemeName {
    pub fn accent_for(self, dark: bool) -> Hsla {
        match (self, dark) {
            (Self::Signal, true) => rgb(0x22D3EE),
            (Self::Signal, false) => rgb(0x0891B2),
            (Self::Proof, true) => rgb(0x34D399),
            (Self::Proof, false) => rgb(0x059669),
            (Self::Violet, true) => rgb(0xA78BFA),
            (Self::Violet, false) => rgb(0x7C3AED),
            (Self::Amber, true) => rgb(0xFBBF24),
            (Self::Amber, false) => rgb(0xD97706),
            (Self::Ocean, true) => rgb(0x60A5FA),
            (Self::Ocean, false) => rgb(0x2563EB),
            (Self::Monochrome, true) => rgb(0xEEEEEE),
            (Self::Monochrome, false) => rgb(0x242424),
        }
    }

    /// Swatch shown in Settings (the dark-mode accent).
    pub fn accent(self) -> Hsla {
        self.accent_for(true)
    }

    pub fn all() -> &'static [ThemeName] {
        &[
            Self::Signal,
            Self::Proof,
            Self::Violet,
            Self::Amber,
            Self::Ocean,
            Self::Monochrome,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Signal => "Signal",
            Self::Proof => "Proof",
            Self::Violet => "Violet",
            Self::Amber => "Amber",
            Self::Ocean => "Ocean",
            Self::Monochrome => "Monochrome",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Signal => "Cyan for active work",
            Self::Proof => "Evidence green as the focus color",
            Self::Violet => "Repository violet, calm and dense",
            Self::Amber => "The original Surge amber",
            Self::Ocean => "Plain, professional blue",
            Self::Monochrome => "No accent hue — ink only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    Dark,
    Light,
}

/// Semantic roles. Every saturated color in the app maps to one of these.
/// (Not `Role`: that name is gpui's accessibility role.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Semantic {
    /// An agent is doing the work (implement, run, stream). The accent.
    Agent,
    /// Something written down: spec, roadmap, plan, artifact.
    Plan,
    /// Proof: a verifier passed, evidence exists, a run completed.
    Verified,
    /// A person is needed: approval gate, question, inbox.
    You,
    /// Failure, policy block, rejected.
    Failure,
    /// Repetition: loops, retries, iterations.
    Loop,
    /// Outside Surge or idle: terminal, external tools, unknown.
    External,
}

impl Semantic {
    pub fn color(self) -> Hsla {
        match self {
            Self::Agent => primary(),
            Self::Plan => violet(),
            Self::Verified => success(),
            Self::You => warning(),
            Self::Failure => error(),
            Self::Loop => orange(),
            Self::External => slate(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Plan => "Plan",
            Self::Verified => "Verified",
            Self::You => "You",
            Self::Failure => "Failure",
            Self::Loop => "Loop",
            Self::External => "External",
        }
    }
}

thread_local! {
    static COLORS: RefCell<SurgeThemeColors> = RefCell::new(
        SurgeThemeColors::dark(ThemeName::Monochrome.accent_for(true))
    );
    static CURRENT: RefCell<(ThemeName, ThemeMode)> =
        const { RefCell::new((ThemeName::Monochrome, ThemeMode::Dark)) };
}

/// Persisted appearance choice (`$SURGE_HOME/ui/appearance.json`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Appearance {
    theme: ThemeName,
    mode: ThemeMode,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeName::Monochrome,
            mode: ThemeMode::Dark,
        }
    }
}

fn appearance_path() -> Option<PathBuf> {
    surge_core::home::surge_home_dir().map(|home| home.join("ui").join("appearance.json"))
}

fn load_appearance() -> Option<Appearance> {
    let text = std::fs::read_to_string(appearance_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_appearance(appearance: Appearance) {
    let Some(path) = appearance_path() else {
        return;
    };
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            let text = serde_json::to_string_pretty(&appearance).map_err(std::io::Error::other)?;
            std::fs::write(&path, text)
        });
    if let Err(error) = result {
        tracing::warn!(%error, path = %path.display(), "could not persist appearance");
    }
}

/// Load the persisted appearance (or the Monochrome/Dark default) into the
/// token store. Call once at startup, before the first window renders.
pub fn init() {
    let Appearance { theme, mode } = load_appearance().unwrap_or_default();
    set_palette(theme, mode);
}

/// The appearance currently applied.
pub fn current() -> (ThemeName, ThemeMode) {
    CURRENT.with(|c| *c.borrow())
}

fn set_palette(name: ThemeName, mode: ThemeMode) {
    let colors = match mode {
        ThemeMode::Dark => SurgeThemeColors::dark(name.accent_for(true)),
        ThemeMode::Light => SurgeThemeColors::light(name.accent_for(false)),
    };
    COLORS.with(|c| *c.borrow_mut() = colors);
    CURRENT.with(|c| *c.borrow_mut() = (name, mode));
}

/// Switch theme: update tokens, gpui-component chrome, persist, and
/// repaint every window.
pub fn apply_theme(name: ThemeName, mode: ThemeMode, cx: &mut App) {
    set_palette(name, mode);
    save_appearance(Appearance { theme: name, mode });
    sync_component_theme(cx);
    cx.refresh_windows();
}

/// Push the Surge palette into gpui-component's global theme so stock
/// widgets (Button, Tab, Input, Switch, lists, scrollbars) match.
pub fn sync_component_theme(cx: &mut App) {
    use gpui_kit::component::{Theme, ThemeMode as ComponentMode};

    let c = COLORS.with(|c| *c.borrow());
    let mode = if c.dark {
        ComponentMode::Dark
    } else {
        ComponentMode::Light
    };
    Theme::change(mode, None, cx);
    Theme::update(cx, |t| {
        t.font_family = crate::ui::BODY.into();
        t.mono_font_family = crate::ui::MONO.into();
        t.font_size = gpui_kit::px(15.0);
        t.mono_font_size = gpui_kit::px(12.0);
        t.radius = gpui_kit::px(6.0);
        t.radius_lg = gpui_kit::px(12.0);
        t.shadow = false;

        let colors = &mut t.colors;
        colors.background = c.background;
        colors.foreground = c.text_primary;
        colors.border = c.hairline;
        colors.input = c.hairline_strong;
        colors.ring = c.primary;
        colors.caret = c.primary;
        colors.selection = c.primary.opacity(0.28);
        colors.muted = c.surface;
        colors.muted_foreground = c.text_muted;
        colors.accent = c.surface;
        colors.accent_foreground = c.text_primary;
        colors.popover = c.panel_raised;
        colors.popover_foreground = c.text_primary;
        colors.overlay = gpui_kit::hsla(0.0, 0.0, 0.0, if c.dark { 0.6 } else { 0.3 });
        colors.window_border = c.hairline;
        colors.title_bar = c.sidebar_bg;
        colors.title_bar_border = c.hairline;
        colors.status_bar = c.sidebar_bg;
        colors.status_bar_border = c.hairline;
        colors.sidebar = c.sidebar_bg;
        colors.sidebar_border = c.hairline;
        colors.sidebar_foreground = c.text_muted;
        colors.sidebar_accent = c.surface;
        colors.sidebar_accent_foreground = c.text_primary;
        colors.sidebar_primary = c.primary;
        colors.sidebar_primary_foreground = c.on_accent;
        colors.link = c.primary;
        colors.link_hover = c.primary.opacity(0.85);
        colors.link_active = c.primary;

        colors.primary = c.primary;
        colors.primary_hover = c.primary.opacity(0.88);
        colors.primary_active = c.primary.opacity(0.76);
        colors.primary_foreground = c.on_accent;
        colors.secondary = c.panel_raised;
        colors.secondary_hover = c.surface;
        colors.secondary_active = c.hairline;
        colors.secondary_foreground = c.text_primary;

        colors.button = c.panel_raised;
        colors.button_hover = c.surface;
        colors.button_active = c.hairline;
        colors.button_foreground = c.text_primary;
        colors.button_primary = c.primary;
        colors.button_primary_hover = c.primary.opacity(0.88);
        colors.button_primary_active = c.primary.opacity(0.76);
        colors.button_primary_foreground = c.on_accent;
        colors.button_secondary = c.panel_raised;
        colors.button_secondary_hover = c.surface;
        colors.button_secondary_active = c.hairline;
        colors.button_secondary_foreground = c.text_primary;
        colors.button_danger = c.error;
        colors.button_danger_hover = c.error.opacity(0.88);
        colors.button_danger_active = c.error.opacity(0.76);
        colors.button_danger_foreground = c.on_accent;
        colors.button_success = c.success;
        colors.button_success_hover = c.success.opacity(0.88);
        colors.button_success_active = c.success.opacity(0.76);
        colors.button_success_foreground = c.on_accent;
        colors.button_warning = c.warning;
        colors.button_warning_hover = c.warning.opacity(0.88);
        colors.button_warning_active = c.warning.opacity(0.76);
        colors.button_warning_foreground = c.on_accent;

        colors.danger = c.error;
        colors.danger_hover = c.error.opacity(0.88);
        colors.danger_active = c.error.opacity(0.76);
        colors.danger_foreground = c.on_accent;
        colors.success = c.success;
        colors.success_hover = c.success.opacity(0.88);
        colors.success_active = c.success.opacity(0.76);
        colors.success_foreground = c.on_accent;
        colors.warning = c.warning;
        colors.warning_hover = c.warning.opacity(0.88);
        colors.warning_active = c.warning.opacity(0.76);
        colors.warning_foreground = c.on_accent;
        colors.info = c.primary;
        colors.info_hover = c.primary.opacity(0.88);
        colors.info_active = c.primary.opacity(0.76);
        colors.info_foreground = c.on_accent;

        colors.tab_bar = c.panel;
        colors.tab_bar_segmented = c.panel_deep;
        colors.tab = gpui_kit::transparent_black();
        colors.tab_foreground = c.text_muted;
        colors.tab_active = c.surface;
        colors.tab_active_foreground = c.text_primary;

        colors.list = c.background;
        colors.list_even = c.panel;
        colors.list_head = c.panel;
        colors.list_hover = c.surface;
        colors.list_active = c.primary.opacity(0.12);
        colors.list_active_border = c.primary.opacity(0.5);
        colors.table = c.background;
        colors.table_even = c.panel;
        colors.table_head = c.panel;
        colors.table_head_foreground = c.text_muted;
        colors.table_hover = c.surface;
        colors.table_active = c.primary.opacity(0.12);
        colors.table_active_border = c.primary.opacity(0.5);
        colors.table_row_border = c.hairline;

        colors.switch = c.hairline_strong;
        colors.switch_thumb = c.text_primary;
        colors.slider_bar = c.primary;
        colors.slider_thumb = c.text_primary;
        colors.progress_bar = c.primary;
        colors.skeleton = c.surface;
        colors.scrollbar = gpui_kit::transparent_black();
        colors.scrollbar_thumb = c.hairline_strong;
        colors.scrollbar_thumb_hover = c.graph_line;
        colors.group_box = c.panel;
        colors.group_box_foreground = c.text_primary;
        colors.accordion = c.panel;
        colors.description_list_label = c.panel;
        colors.description_list_label_foreground = c.text_muted;
        colors.drag_border = c.primary;
        colors.drop_target = c.primary.opacity(0.12);

        colors.chart_1 = c.primary;
        colors.chart_2 = c.success;
        colors.chart_3 = c.violet;
        colors.chart_4 = c.warning;
        colors.chart_5 = c.orange;
        colors.chart_grid = c.hairline;
        colors.chart_bullish = c.success;
        colors.chart_bearish = c.error;

        colors.red = c.error;
        colors.green = c.success;
        colors.yellow = c.warning;
        colors.cyan = c.primary;
        colors.magenta = c.violet;
    });
}

fn get<F: FnOnce(&SurgeThemeColors) -> Hsla>(f: F) -> Hsla {
    COLORS.with(|c| f(&c.borrow()))
}

/// Whether the dark palette is active.
pub fn is_dark() -> bool {
    COLORS.with(|c| c.borrow().dark)
}

pub fn primary() -> Hsla {
    get(|c| c.primary)
}
/// Interactive hover / selected-row surface.
pub fn surface() -> Hsla {
    get(|c| c.surface)
}
/// The canvas.
pub fn background() -> Hsla {
    get(|c| c.background)
}
pub fn sidebar_bg() -> Hsla {
    get(|c| c.sidebar_bg)
}
pub fn text_primary() -> Hsla {
    get(|c| c.text_primary)
}
pub fn text_muted() -> Hsla {
    get(|c| c.text_muted)
}
/// Annotation ink, one step quieter than muted.
pub fn text_dim() -> Hsla {
    get(|c| c.text_dim)
}
pub fn success() -> Hsla {
    get(|c| c.success)
}
pub fn warning() -> Hsla {
    get(|c| c.warning)
}
pub fn error() -> Hsla {
    get(|c| c.error)
}
pub fn violet() -> Hsla {
    get(|c| c.violet)
}
pub fn orange() -> Hsla {
    get(|c| c.orange)
}
pub fn slate() -> Hsla {
    get(|c| c.slate)
}

/// Accent = the active theme's primary. Alias so code can read intent
/// ("accent") instead of "primary".
pub fn accent() -> Hsla {
    primary()
}

/// Ink drawn ON the accent (button labels, badges).
pub fn on_accent() -> Hsla {
    get(|c| c.on_accent)
}

/// Rail / context bar / panel background.
pub fn panel() -> Hsla {
    get(|c| c.panel)
}

/// Raised card surface (cards, inspectors).
pub fn panel_raised() -> Hsla {
    get(|c| c.panel_raised)
}

/// Deep canvas background (diagram fields).
pub fn panel_deep() -> Hsla {
    get(|c| c.panel_deep)
}

/// Hairline divider / border.
pub fn hairline() -> Hsla {
    get(|c| c.hairline)
}

/// Stronger hairline (keycaps, hover borders).
pub fn hairline_strong() -> Hsla {
    get(|c| c.hairline_strong)
}

/// Idle graph edge / trunk line.
pub fn graph_line() -> Hsla {
    get(|c| c.graph_line)
}

/// Canvas grid line.
pub fn grid() -> Hsla {
    get(|c| c.grid)
}

/// Tinted fill for a semantic node: the role color at low alpha, so a
/// node reads as "belongs to" its role without shouting.
pub fn tint(color: Hsla) -> Hsla {
    color.opacity(if is_dark() { 0.10 } else { 0.08 })
}

/// Border for a semantic node.
pub fn stroke(color: Hsla) -> Hsla {
    color.opacity(if is_dark() { 0.62 } else { 0.75 })
}

#[cfg(test)]
mod tests {
    use super::{Appearance, SurgeThemeColors, ThemeMode, ThemeName};

    #[test]
    fn new_install_defaults_preserve_saved_appearance_choices() {
        let defaults = Appearance::default();
        assert_eq!(
            (defaults.theme, defaults.mode),
            (ThemeName::Monochrome, ThemeMode::Dark)
        );
        let saved: Appearance =
            serde_json::from_str(r#"{"theme":"Signal","mode":"Dark"}"#).unwrap();
        assert_eq!(
            (saved.theme, saved.mode),
            (ThemeName::Signal, ThemeMode::Dark)
        );
    }

    #[test]
    fn workbench_palette_keeps_secondary_text_readable() {
        fn luminance(color: gpui_kit::Hsla) -> f32 {
            fn linear(component: f32) -> f32 {
                if component <= 0.04045 {
                    component / 12.92
                } else {
                    ((component + 0.055) / 1.055).powf(2.4)
                }
            }
            let color = color.to_rgb();
            0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
        }
        for colors in [
            SurgeThemeColors::light(ThemeName::Monochrome.accent_for(false)),
            SurgeThemeColors::dark(ThemeName::Monochrome.accent_for(true)),
        ] {
            for text in [colors.text_primary, colors.text_muted, colors.text_dim] {
                for background in [
                    colors.background,
                    colors.sidebar_bg,
                    colors.panel,
                    colors.surface,
                    colors.panel_raised,
                ] {
                    let foreground = luminance(text);
                    let background = luminance(background);
                    let contrast =
                        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
                    assert!(contrast >= 4.5, "body text contrast is {contrast}");
                }
            }
        }
    }
}
