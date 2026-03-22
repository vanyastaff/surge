use eframe::egui;

use crate::theme::Colors;

/// Render the dashboard panel with stat cards and recent activity.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Dashboard");
    ui.add_space(16.0);

    // Stat cards row
    ui.horizontal(|ui| {
        stat_card(ui, "Tasks", "12", Colors::ACCENT);
        stat_card(ui, "Active", "3", Colors::SUCCESS);
        stat_card(ui, "Failed", "1", Colors::ERROR);
        stat_card(ui, "Agents", "4", Colors::PURPLE);
    });

    ui.add_space(24.0);
    ui.separator();
    ui.add_space(12.0);

    ui.label(egui::RichText::new("Recent Activity").size(16.0).color(Colors::TEXT));
    ui.add_space(8.0);

    egui::Frame::new()
        .fill(Colors::BG_CARD)
        .corner_radius(6.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("• Task #001 completed successfully").color(Colors::SUCCESS));
            ui.label(egui::RichText::new("• Task #002 in progress — agent claude-1 executing").color(Colors::ACCENT));
            ui.label(egui::RichText::new("• Task #003 failed — QA review rejected").color(Colors::ERROR));
            ui.label(egui::RichText::new("• Agent copilot-2 connected").color(Colors::TEXT_DIM));
        });
}

fn stat_card(ui: &mut egui::Ui, title: &str, value: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(Colors::BG_CARD)
        .corner_radius(8.0)
        .inner_margin(16.0)
        .stroke(egui::Stroke::new(1.0, Colors::BORDER))
        .show(ui, |ui| {
            ui.set_min_width(120.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).size(12.0).color(Colors::TEXT_DIM));
                ui.label(egui::RichText::new(value).size(28.0).strong().color(color));
            });
        });
}
