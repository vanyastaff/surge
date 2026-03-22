use eframe::egui;
use surge_acp::registry::DetectedAgent;

use crate::theme::Colors;

/// Render the dashboard panel with stat cards and recent activity.
pub fn show(ui: &mut egui::Ui, detected_agents: &[DetectedAgent]) {
    ui.heading("Dashboard");
    ui.add_space(16.0);

    // Stat cards row
    let agents_count = detected_agents.len().to_string();
    ui.horizontal(|ui| {
        stat_card(ui, "Tasks", "0", Colors::ACCENT);
        stat_card(ui, "Active", "0", Colors::SUCCESS);
        stat_card(ui, "Failed", "0", Colors::ERROR);
        stat_card(ui, "Agents", &agents_count, Colors::PURPLE);
    });

    ui.add_space(24.0);
    ui.separator();
    ui.add_space(12.0);

    // Detected agents summary
    ui.label(egui::RichText::new("Detected Agents").size(16.0).color(Colors::TEXT));
    ui.add_space(8.0);

    egui::Frame::new()
        .fill(Colors::BG_CARD)
        .corner_radius(6.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            if detected_agents.is_empty() {
                ui.colored_label(Colors::TEXT_DIM, "No agents detected. Install an ACP-compatible agent.");
            } else {
                for agent in detected_agents {
                    let caps: Vec<String> = agent.entry.capabilities.iter().map(|c| c.to_string()).collect();
                    ui.horizontal(|ui| {
                        ui.colored_label(Colors::SUCCESS, "✅");
                        ui.strong(&agent.entry.display_name);
                        ui.colored_label(Colors::TEXT_DIM, format!("— {}", caps.join(", ")));
                    });
                }
            }
        });

    ui.add_space(16.0);
    ui.label(egui::RichText::new("Recent Activity").size(16.0).color(Colors::TEXT));
    ui.add_space(8.0);

    egui::Frame::new()
        .fill(Colors::BG_CARD)
        .corner_radius(6.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.colored_label(Colors::TEXT_DIM, "No recent activity. Use 'surge run' to start a pipeline.");
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
