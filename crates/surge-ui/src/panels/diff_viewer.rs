//! Diff Viewer panel — view worktree changes.

use eframe::egui;

use crate::theme::Colors;

/// Render the diff viewer panel.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Diff Viewer");
    ui.add_space(8.0);

    // Spec selector
    ui.horizontal(|ui| {
        ui.colored_label(Colors::TEXT_DIM, "Spec:");
        ui.text_edit_singleline(&mut String::new());
        if ui.button("Load Diff").clicked() {
            // TODO: load diff for spec
        }
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // Two-column: file list + diff content
    ui.columns(2, |cols| {
        // Left: changed files
        cols[0].heading("Changed Files");
        cols[0].separator();
        egui::ScrollArea::vertical()
            .id_salt("file_list")
            .max_height(500.0)
            .show(&mut cols[0], |ui| {
                ui.colored_label(Colors::TEXT_DIM, "No diff loaded.");
                ui.colored_label(Colors::TEXT_DIM, "Enter a spec ID and click Load Diff.");
            });

        // Right: diff content
        cols[1].heading("Diff");
        cols[1].separator();
        egui::ScrollArea::vertical()
            .id_salt("diff_content")
            .max_height(500.0)
            .show(&mut cols[1], |ui| {
                // Monospace placeholder
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(13.0));
                ui.colored_label(Colors::TEXT_DIM, "Select a file to view diff.");
            });
    });
}
