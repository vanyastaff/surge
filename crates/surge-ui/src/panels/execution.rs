//! Execution Monitor panel — real-time pipeline view.

use eframe::egui;

use crate::theme::Colors;

/// Render the execution panel.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Live Execution");
    ui.add_space(8.0);

    // Status bar
    ui.horizontal(|ui| {
        ui.colored_label(Colors::TEXT_DIM, "Status:");
        ui.colored_label(Colors::ACCENT, "Idle");
        ui.separator();
        ui.colored_label(Colors::TEXT_DIM, "Phase:");
        ui.label("—");
    });

    ui.add_space(8.0);

    // Progress
    ui.label("Progress");
    let progress_bar = egui::ProgressBar::new(0.0)
        .text("0 / 0 subtasks");
    ui.add(progress_bar);

    ui.add_space(16.0);

    // Two-column layout: subtask list + log
    ui.columns(2, |cols| {
        // Left: subtask list
        cols[0].heading("Subtasks");
        cols[0].separator();
        egui::ScrollArea::vertical()
            .id_salt("subtask_list")
            .max_height(400.0)
            .show(&mut cols[0], |ui| {
                ui.colored_label(Colors::TEXT_DIM, "No active spec. Use 'surge run <spec_id>' to start.");
            });

        // Right: log output
        cols[1].heading("Pipeline Log");
        cols[1].separator();
        egui::ScrollArea::vertical()
            .id_salt("pipeline_log")
            .max_height(400.0)
            .stick_to_bottom(true)
            .show(&mut cols[1], |ui| {
                ui.colored_label(Colors::TEXT_DIM, "Waiting for pipeline events...");
            });
    });

    ui.add_space(16.0);

    // Control buttons
    ui.horizontal(|ui| {
        if ui.button("▶ Start").clicked() {
            // TODO: start pipeline
        }
        if ui.button("⏸ Pause").clicked() {
            // TODO: pause pipeline
        }
        if ui.button("⏹ Stop").clicked() {
            // TODO: stop pipeline
        }
    });
}
