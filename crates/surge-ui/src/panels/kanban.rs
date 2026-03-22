//! Kanban Board panel — tasks organized by state.

use eframe::egui;

use crate::theme::Colors;

const COLUMNS: &[(&str, &str)] = &[
    ("Draft", "📝"),
    ("Planning", "🔍"),
    ("Executing", "⚙️"),
    ("QA Review", "🔬"),
    ("Human Review", "👤"),
    ("Merging", "🔀"),
    ("Completed", "✅"),
    ("Failed", "❌"),
];

/// Render the kanban board panel.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Kanban Board");
    ui.add_space(8.0);

    // Filter bar
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.colored_label(Colors::TEXT_DIM, "All agents");
        ui.separator();
        ui.colored_label(Colors::TEXT_DIM, "All complexity");
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // Columns
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal_top(|ui| {
            for (name, icon) in COLUMNS {
                egui::Frame::new()
                    .fill(Colors::BG_DARK)
                    .corner_radius(8.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.set_max_width(200.0);

                        // Column header
                        ui.horizontal(|ui| {
                            ui.label(*icon);
                            ui.strong(*name);
                            ui.colored_label(Colors::TEXT_DIM, "(0)");
                        });
                        ui.separator();

                        // Empty state
                        ui.add_space(40.0);
                        ui.centered_and_justified(|ui| {
                            ui.colored_label(Colors::TEXT_DIM, "No tasks");
                        });
                        ui.add_space(40.0);
                    });
                ui.add_space(4.0);
            }
        });
    });
}
