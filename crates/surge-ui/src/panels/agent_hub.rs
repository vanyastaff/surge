//! Agent Hub panel — manage connected agents.

use eframe::egui;

use crate::theme::Colors;

struct AgentInfo {
    name: &'static str,
    command: &'static str,
    transport: &'static str,
    status: AgentStatus,
    requests: u64,
    failures: u64,
    latency_ms: u64,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum AgentStatus {
    Online,
    Offline,
    RateLimited,
}

impl AgentStatus {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Online => Colors::SUCCESS,
            Self::Offline => Colors::ERROR,
            Self::RateLimited => Colors::WARNING,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Online => "Online",
            Self::Offline => "Offline",
            Self::RateLimited => "Rate Limited",
        }
    }
}

/// Render the agent hub panel.
pub fn show(ui: &mut egui::Ui) {
    ui.heading("Agent Hub");
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        if ui.button("+ Add Agent").clicked() {
            // TODO: open add agent dialog
        }
        if ui.button("🔄 Refresh").clicked() {
            // TODO: refresh agent status
        }
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // Agent cards
    // Show placeholder with instructions
    ui.colored_label(Colors::TEXT_DIM, "Agents are loaded from surge.toml configuration.");
    ui.add_space(8.0);

    // Example agent card template
    agent_card(ui, &AgentInfo {
        name: "claude",
        command: "claude",
        transport: "stdio",
        status: AgentStatus::Offline,
        requests: 0,
        failures: 0,
        latency_ms: 0,
    });
}

fn agent_card(ui: &mut egui::Ui, agent: &AgentInfo) {
    egui::Frame::new()
        .fill(Colors::BG_CARD)
        .corner_radius(8.0)
        .inner_margin(12.0)
        .stroke(egui::Stroke::new(1.0, Colors::BORDER))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Status dot
                let status_color = agent.status.color();
                ui.colored_label(status_color, "●");

                ui.strong(agent.name);
                ui.colored_label(Colors::TEXT_DIM, format!("({})", agent.status.label()));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Test").clicked() {
                        // TODO: test connection
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.colored_label(Colors::TEXT_DIM, "Command:");
                ui.label(agent.command);
                ui.colored_label(Colors::TEXT_DIM, "| Transport:");
                ui.label(agent.transport);
            });

            ui.horizontal(|ui| {
                ui.colored_label(Colors::TEXT_DIM, format!("Requests: {}", agent.requests));
                ui.colored_label(Colors::TEXT_DIM, format!("| Failures: {}", agent.failures));
                ui.colored_label(Colors::TEXT_DIM, format!("| Latency: {}ms", agent.latency_ms));
            });
        });
}
