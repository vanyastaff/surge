use eframe::egui;

use crate::theme::Colors;

/// Render the agent hub panel.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Agent Hub");
    ui.add_space(8.0);
    ui.label(egui::RichText::new("Coming soon...").color(Colors::TEXT_DIM));
}
