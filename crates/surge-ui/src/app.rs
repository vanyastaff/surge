use eframe::egui;

use crate::panels::{self, ActivePanel};
use crate::theme::{self, Colors};

/// Main application state for the Surge GUI.
pub struct SurgeApp {
    active_panel: ActivePanel,
}

impl SurgeApp {
    /// Create a new `SurgeApp`, applying the dark theme.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply_theme(&cc.egui_ctx);
        Self {
            active_panel: ActivePanel::default(),
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
            });

        // Central panel
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Colors::BG_DARK)
                    .inner_margin(24.0),
            )
            .show(ctx, |ui| match self.active_panel {
                ActivePanel::Dashboard => panels::dashboard::show(ui),
                ActivePanel::Kanban => panels::kanban::show(ui),
                ActivePanel::Execution => panels::execution::show(ui),
                ActivePanel::AgentHub => panels::agent_hub::show(ui),
                ActivePanel::DiffViewer => panels::diff_viewer::show(ui),
                ActivePanel::Terminal => panels::terminal::show(ui),
            });
    }
}
