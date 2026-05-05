use std::cell::RefCell;

use gpui::Hsla;

// ── Dynamic theme colors ───────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct SurgeThemeColors {
    pub primary: Hsla,
    pub surface: Hsla,
    pub background: Hsla,
    pub sidebar_bg: Hsla,
    pub text_primary: Hsla,
    pub text_muted: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
}

impl SurgeThemeColors {
    fn dark(primary: Hsla) -> Self {
        Self {
            primary,
            surface: hsla(240.0, 0.33, 0.14),
            background: hsla(240.0, 0.33, 0.07),
            sidebar_bg: hsla(240.0, 0.33, 0.10),
            text_primary: hsla(0.0, 0.0, 0.93),
            text_muted: hsla(0.0, 0.0, 0.55),
            success: hsla(142.0, 0.71, 0.45),
            warning: hsla(38.0, 0.92, 0.50),
            error: hsla(0.0, 0.84, 0.60),
        }
    }

    fn light(primary: Hsla) -> Self {
        Self {
            primary,
            surface: hsla(220.0, 0.15, 0.95),
            background: hsla(0.0, 0.0, 1.0),
            sidebar_bg: hsla(220.0, 0.15, 0.97),
            text_primary: hsla(0.0, 0.0, 0.10),
            text_muted: hsla(0.0, 0.0, 0.45),
            success: hsla(142.0, 0.71, 0.35),
            warning: hsla(38.0, 0.92, 0.45),
            error: hsla(0.0, 0.84, 0.50),
        }
    }
}

const fn hsla(h_deg: f32, s: f32, l: f32) -> Hsla {
    Hsla {
        h: h_deg / 360.0,
        s,
        l,
        a: 1.0,
    }
}

// ── Predefined themes ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Default,
    Dusk,
    Lime,
    Ocean,
    Retro,
    Neo,
    Verdant,
    Monochrome,
}

impl ThemeName {
    pub fn accent(self) -> Hsla {
        match self {
            Self::Default => hsla(45.0, 0.85, 0.55),
            Self::Dusk => hsla(25.0, 0.85, 0.55),
            Self::Lime => hsla(90.0, 0.80, 0.50),
            Self::Ocean => hsla(200.0, 0.80, 0.55),
            Self::Retro => hsla(20.0, 0.85, 0.55),
            Self::Neo => hsla(330.0, 0.85, 0.55),
            Self::Verdant => hsla(155.0, 0.75, 0.45),
            Self::Monochrome => hsla(0.0, 0.0, 0.65),
        }
    }

    pub fn all() -> &'static [ThemeName] {
        &[
            Self::Default,
            Self::Dusk,
            Self::Lime,
            Self::Ocean,
            Self::Retro,
            Self::Neo,
            Self::Verdant,
            Self::Monochrome,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Dusk => "Dusk",
            Self::Lime => "Lime",
            Self::Ocean => "Ocean",
            Self::Retro => "Retro",
            Self::Neo => "Neo",
            Self::Verdant => "Verdant",
            Self::Monochrome => "Monochrome",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Default => "Oscura-inspired with pale yellow accents",
            Self::Dusk => "Warmer variant with lighter dark mode",
            Self::Lime => "Fresh, energetic lime with purple accents",
            Self::Ocean => "Calm, professional blue tones",
            Self::Retro => "Warm, nostalgic amber vibes",
            Self::Neo => "Modern cyberpunk pink/magenta",
            Self::Verdant => "Clean green, inspired by nature",
            Self::Monochrome => "Pure black & white, no color",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

// ── Thread-local storage ───────────────────────────────────────────

thread_local! {
    static COLORS: RefCell<SurgeThemeColors> = RefCell::new(
        SurgeThemeColors::dark(ThemeName::Default.accent())
    );
}

pub fn init() {
    apply_theme(ThemeName::Default, ThemeMode::Dark);
}

pub fn apply_theme(name: ThemeName, mode: ThemeMode) {
    let colors = match mode {
        ThemeMode::Dark => SurgeThemeColors::dark(name.accent()),
        ThemeMode::Light => SurgeThemeColors::light(name.accent()),
    };
    COLORS.with(|c| *c.borrow_mut() = colors);
}

pub fn set_accent(accent: Hsla) {
    COLORS.with(|c| c.borrow_mut().primary = accent);
}

fn get<F: FnOnce(&SurgeThemeColors) -> Hsla>(f: F) -> Hsla {
    COLORS.with(|c| f(&c.borrow()))
}

// ── Public accessors (drop-in replacement for old constants) ───────

pub fn primary() -> Hsla {
    get(|c| c.primary)
}
pub fn surface() -> Hsla {
    get(|c| c.surface)
}
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
pub fn success() -> Hsla {
    get(|c| c.success)
}
pub fn warning() -> Hsla {
    get(|c| c.warning)
}
pub fn error() -> Hsla {
    get(|c| c.error)
}

// ── Backwards compat aliases (will be removed) ────────────────────
// These keep old code compiling during migration. Values are kept in
// sync with `SurgeThemeColors::dark(ThemeName::Default.accent())` so
// screens that still read these consts render the same colours as
// screens that have already migrated to the dynamic accessors —
// nobody sees a half-yellow / half-purple frame mid-migration.

pub const PRIMARY: Hsla = Hsla {
    // ThemeName::Default.accent() = hsla(45°, 0.85, 0.55).
    h: 45.0 / 360.0,
    s: 0.85,
    l: 0.55,
    a: 1.0,
};
pub const SURFACE: Hsla = Hsla {
    h: 240.0 / 360.0,
    s: 0.33,
    l: 0.14,
    a: 1.0,
};
pub const BACKGROUND: Hsla = Hsla {
    h: 240.0 / 360.0,
    s: 0.33,
    l: 0.07,
    a: 1.0,
};
pub const SIDEBAR_BG: Hsla = Hsla {
    h: 240.0 / 360.0,
    s: 0.33,
    l: 0.10,
    a: 1.0,
};
pub const TEXT_PRIMARY: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.93,
    a: 1.0,
};
pub const TEXT_MUTED: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.55,
    a: 1.0,
};
pub const SUCCESS: Hsla = Hsla {
    h: 142.0 / 360.0,
    s: 0.71,
    l: 0.45,
    a: 1.0,
};
pub const WARNING: Hsla = Hsla {
    h: 38.0 / 360.0,
    s: 0.92,
    l: 0.50,
    a: 1.0,
};
pub const ERROR: Hsla = Hsla {
    h: 0.0 / 360.0,
    s: 0.84,
    l: 0.60,
    a: 1.0,
};
