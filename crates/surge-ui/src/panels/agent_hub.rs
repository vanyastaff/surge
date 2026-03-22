//! Agent Hub panel — shows detected and missing agents.

use eframe::egui;
use surge_acp::registry::{DetectedAgent, RegistryEntry};

use crate::theme::Colors;

/// Render the agent hub panel with real detected agents.
pub fn show(ui: &mut egui::Ui, detected: &[DetectedAgent], missing: &[RegistryEntry]) {
    ui.heading("Agent Hub");
    ui.add_space(8.0);

    ui.colored_label(
        Colors::TEXT_DIM,
        format!(
            "{} installed, {} not installed",
            detected.len(),
            missing.len()
        ),
    );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // Installed agents
    if detected.is_empty() {
        ui.colored_label(Colors::WARNING, "No agents detected on this system.");
        ui.add_space(4.0);
        ui.colored_label(Colors::TEXT_DIM, "Install an agent and restart Surge.");
    } else {
        ui.label(
            egui::RichText::new("Installed")
                .strong()
                .color(Colors::SUCCESS),
        );
        ui.add_space(4.0);

        for agent in detected {
            detected_agent_card(ui, agent);
            ui.add_space(4.0);
        }
    }

    // Missing agents
    if !missing.is_empty() {
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("Not Installed")
                .strong()
                .color(Colors::TEXT_DIM),
        );
        ui.add_space(4.0);

        for entry in missing {
            missing_agent_card(ui, entry);
            ui.add_space(4.0);
        }
    }
}

fn detected_agent_card(ui: &mut egui::Ui, agent: &DetectedAgent) {
    egui::Frame::new()
        .fill(Colors::BG_CARD)
        .corner_radius(8.0)
        .inner_margin(12.0)
        .stroke(egui::Stroke::new(1.0, Colors::BORDER))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(Colors::SUCCESS, "●");
                ui.strong(&agent.entry.display_name);
                ui.colored_label(Colors::TEXT_DIM, format!("({})", agent.entry.id));
            });

            ui.add_space(4.0);

            // Path
            if let Some(path) = &agent.command_path {
                ui.horizontal(|ui| {
                    ui.colored_label(Colors::TEXT_DIM, "Path:");
                    ui.label(
                        egui::RichText::new(path)
                            .monospace()
                            .color(Colors::TEXT),
                    );
                });
            }

            // Command
            ui.horizontal(|ui| {
                ui.colored_label(Colors::TEXT_DIM, "Command:");
                let cmd = if agent.entry.default_args.is_empty() {
                    agent.entry.command.clone()
                } else {
                    format!("{} {}", agent.entry.command, agent.entry.default_args.join(" "))
                };
                ui.label(egui::RichText::new(cmd).monospace().color(Colors::TEXT));
            });

            // Capabilities
            ui.horizontal(|ui| {
                ui.colored_label(Colors::TEXT_DIM, "Capabilities:");
                for cap in &agent.entry.capabilities {
                    let color = match cap.to_string().as_str() {
                        "code" => Colors::ACCENT,
                        "plan" => Colors::PURPLE,
                        "review" => Colors::WARNING,
                        "test" => Colors::SUCCESS,
                        _ => Colors::TEXT_DIM,
                    };
                    egui::Frame::new()
                        .fill(color.gamma_multiply(0.2))
                        .corner_radius(4.0)
                        .inner_margin(4.0)
                        .show(ui, |ui| {
                            ui.colored_label(color, cap.to_string());
                        });
                }
            });

            // Description
            ui.add_space(2.0);
            ui.colored_label(Colors::TEXT_DIM, &agent.entry.description);
        });
}

fn missing_agent_card(ui: &mut egui::Ui, entry: &RegistryEntry) {
    egui::Frame::new()
        .fill(Colors::BG_CARD.gamma_multiply(0.7))
        .corner_radius(8.0)
        .inner_margin(12.0)
        .stroke(egui::Stroke::new(1.0, Colors::BORDER.gamma_multiply(0.5)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(Colors::ERROR, "●");
                ui.strong(
                    egui::RichText::new(&entry.display_name).color(Colors::TEXT_DIM),
                );
                ui.colored_label(Colors::TEXT_DIM, format!("({})", entry.id));
            });

            ui.add_space(4.0);
            ui.colored_label(Colors::TEXT_DIM, &entry.description);

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.colored_label(Colors::TEXT_DIM, "Install:");
                ui.label(
                    egui::RichText::new(&entry.install_instructions)
                        .monospace()
                        .color(Colors::WARNING),
                );
            });
        });
}
