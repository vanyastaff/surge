use eframe::egui;

/// Catppuccin-inspired dark theme colors.
pub struct Colors;

#[allow(dead_code)]
impl Colors {
    pub const BG_DARK: egui::Color32 = egui::Color32::from_rgb(17, 17, 27);
    pub const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(24, 24, 37);
    pub const BG_CARD: egui::Color32 = egui::Color32::from_rgb(30, 30, 46);
    pub const BORDER: egui::Color32 = egui::Color32::from_rgb(49, 50, 68);
    pub const TEXT: egui::Color32 = egui::Color32::from_rgb(205, 214, 244);
    pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(147, 153, 178);
    pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(137, 180, 250);
    pub const SUCCESS: egui::Color32 = egui::Color32::from_rgb(166, 227, 161);
    pub const WARNING: egui::Color32 = egui::Color32::from_rgb(249, 226, 175);
    pub const ERROR: egui::Color32 = egui::Color32::from_rgb(243, 139, 168);
    pub const PURPLE: egui::Color32 = egui::Color32::from_rgb(203, 166, 247);
}

/// Apply the Surge dark theme to the given egui context.
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = Colors::BG_DARK;
    visuals.window_fill = Colors::BG_PANEL;
    visuals.extreme_bg_color = Colors::BG_PANEL;
    visuals.faint_bg_color = Colors::BG_CARD;

    visuals.widgets.noninteractive.bg_fill = Colors::BG_CARD;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, Colors::TEXT);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Colors::BORDER);

    visuals.widgets.inactive.bg_fill = Colors::BG_CARD;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, Colors::TEXT_DIM);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, Colors::BORDER);

    visuals.widgets.hovered.bg_fill = Colors::BG_PANEL;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Colors::ACCENT);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Colors::ACCENT);

    visuals.widgets.active.bg_fill = Colors::ACCENT;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, Colors::BG_DARK);

    visuals.selection.bg_fill = Colors::ACCENT.linear_multiply(0.3);
    visuals.selection.stroke = egui::Stroke::new(1.0, Colors::ACCENT);

    visuals.override_text_color = Some(Colors::TEXT);

    ctx.set_visuals(visuals);
}
