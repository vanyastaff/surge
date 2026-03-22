use eframe::egui;
use surge_acp::registry::{DetectedAgent, Registry};

use crate::panels::{self, ActivePanel};
use crate::theme::{self, Colors};

/// Main application state for the Surge GUI.
pub struct SurgeApp {
    active_panel: ActivePanel,
    /// Detected agents from the registry.
    pub detected_agents: Vec<DetectedAgent>,
    /// Agents that are NOT installed.
    pub missing_agents: Vec<surge_acp::registry::RegistryEntry>,
    /// Agent Hub UI state.
    pub agent_hub_state: panels::agent_hub::AgentHubState,
}

impl SurgeApp {
    /// Create a new `SurgeApp`, applying the dark theme and detecting agents.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply_theme(&cc.egui_ctx);

        let registry = Registry::builtin();
        let detected = registry.detect_installed_with_paths();
        let detected_ids: Vec<String> = detected.iter().map(|d| d.entry.id.clone()).collect();
        let missing: Vec<_> = registry
            .list()
            .iter()
            .filter(|e| !detected_ids.contains(&e.id))
            .cloned()
            .collect();

        Self {
            active_panel: ActivePanel::default(),
            detected_agents: detected,
            missing_agents: missing,
            agent_hub_state: panels::agent_hub::AgentHubState::default(),
        }
    }
}

impl eframe::App for SurgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Sidebar
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .exact_width(180.0)
            .frame(
                egui::Frame::new()
                    .fill(Colors::BG_PANEL)
                    .inner_margin(8.0)
                    .stroke(egui::Stroke::new(1.0, Colors::BORDER)),
            )
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.heading(egui::RichText::new("⚡ Surge").color(Colors::ACCENT).size(20.0));
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(8.0);

                for &panel in ActivePanel::all() {
                    let label = format!("{} {}", panel.icon(), panel.label());
                    let is_selected = self.active_panel == panel;
                    let response = ui.selectable_label(is_selected, label);
                    if response.clicked() {
                        self.active_panel = panel;
                    }
                }

                // Show detected agent count in sidebar
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.colored_label(
                    Colors::TEXT_DIM,
                    format!("{} agents detected", self.detected_agents.len()),
                );
            });

        // Central panel
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Colors::BG_DARK)
                    .inner_margin(24.0),
            )
            .show(ctx, |ui| match self.active_panel {
                ActivePanel::Dashboard => panels::dashboard::show(ui, &self.detected_agents),
                ActivePanel::Kanban => panels::kanban::show(ui),
                ActivePanel::Execution => panels::execution::show(ui),
                ActivePanel::AgentHub => panels::agent_hub::show(
                    ui,
                    &self.detected_agents,
                    &self.missing_agents,
                    &mut self.agent_hub_state,
                ),
                ActivePanel::DiffViewer => panels::diff_viewer::show(ui),
                ActivePanel::Terminal => panels::terminal::show(ui),
            });
    }
}
