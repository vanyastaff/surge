//! Terminal panel — command output viewer.

use eframe::egui;

use crate::theme::Colors;

/// Render the terminal panel.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Terminal");
    ui.add_space(8.0);

    let mut auto_scroll = true;
    ui.horizontal(|ui| {
        ui.colored_label(Colors::TEXT_DIM, "Worktree:");
        ui.label("(none)");
        ui.separator();
        ui.checkbox(&mut auto_scroll, "Auto-scroll");
    });

    ui.add_space(8.0);

    // Terminal output area
    egui::Frame::new()
        .fill(Colors::BG_DARK)
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("terminal_output")
                .max_height(500.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(13.0));
                    ui.colored_label(Colors::SUCCESS, "$ surge ready");
                    ui.colored_label(Colors::TEXT_DIM, "Terminal output will appear here during pipeline execution.");
                });
        });

    ui.add_space(8.0);

    // Input bar
    ui.horizontal(|ui| {
        ui.colored_label(Colors::SUCCESS, "$");
        let mut input = String::new();
        let response = ui.add(
            egui::TextEdit::singleline(&mut input)
                .desired_width(ui.available_width() - 60.0)
                .font(egui::FontId::monospace(13.0))
                .hint_text("Enter command...")
        );
        if ui.button("Send").clicked() || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            // TODO: send command
        }
    });
}
