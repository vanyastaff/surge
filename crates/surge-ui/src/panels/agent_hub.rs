//! Agent Hub panel — browse, inspect, and manage agents.

use eframe::egui;
use surge_acp::registry::{DetectedAgent, RegistryEntry};

use crate::theme::Colors;

/// Which tab is active in the Agent Hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentTab {
    #[default]
    Installed,
    Registry,
}

/// Persistent state for the Agent Hub panel.
#[derive(Default)]
pub struct AgentHubState {
    pub active_tab: AgentTab,
    /// Index of the selected agent (in the current tab's list).
    pub selected_index: Option<usize>,
}

/// Render the Agent Hub panel.
pub fn show(
    ui: &mut egui::Ui,
    detected: &[DetectedAgent],
    missing: &[RegistryEntry],
    state: &mut AgentHubState,
) {
    // Header
    ui.horizontal(|ui| {
        ui.heading(egui::RichText::new("Agent Hub").size(22.0));
        ui.add_space(12.0);
        ui.colored_label(
            Colors::TEXT_DIM,
            format!("{} installed / {} available", detected.len(), detected.len() + missing.len()),
        );
    });
    ui.add_space(12.0);

    // Tabs
    ui.horizontal(|ui| {
        tab_button(ui, "Installed", detected.len(), AgentTab::Installed, &mut state.active_tab, &mut state.selected_index);
        ui.add_space(4.0);
        tab_button(ui, "Registry", missing.len(), AgentTab::Registry, &mut state.active_tab, &mut state.selected_index);
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // Two-column layout: agent list (left) + detail panel (right)
    let available_width = ui.available_width();
    let list_width = (available_width * 0.38).min(350.0);

    ui.horizontal_top(|ui| {
        // Left: agent list
        egui::Frame::new()
            .fill(Colors::BG_PANEL)
            .corner_radius(8.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(list_width);
                ui.set_max_width(list_width);

                egui::ScrollArea::vertical()
                    .id_salt("agent_list")
                    .max_height(ui.available_height().max(500.0))
                    .show(ui, |ui| {
                        match state.active_tab {
                            AgentTab::Installed => {
                                if detected.is_empty() {
                                    ui.add_space(20.0);
                                    ui.colored_label(Colors::TEXT_DIM, "No agents detected.");
                                    ui.colored_label(Colors::TEXT_DIM, "Switch to Registry tab to see available agents.");
                                } else {
                                    for (i, agent) in detected.iter().enumerate() {
                                        let selected = state.selected_index == Some(i);
                                        agent_list_item(ui, &agent.entry.display_name, &agent.entry.description, true, selected, || {
                                            state.selected_index = Some(i);
                                        });
                                    }
                                }
                            }
                            AgentTab::Registry => {
                                if missing.is_empty() {
                                    ui.add_space(20.0);
                                    ui.colored_label(Colors::SUCCESS, "All known agents are installed!");
                                } else {
                                    for (i, entry) in missing.iter().enumerate() {
                                        let selected = state.selected_index == Some(i);
                                        agent_list_item(ui, &entry.display_name, &entry.description, false, selected, || {
                                            state.selected_index = Some(i);
                                        });
                                    }
                                }
                            }
                        }
                    });
            });

        ui.add_space(8.0);

        // Right: detail panel
        egui::Frame::new()
            .fill(Colors::BG_PANEL)
            .corner_radius(8.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                egui::ScrollArea::vertical()
                    .id_salt("agent_detail")
                    .max_height(ui.available_height().max(500.0))
                    .show(ui, |ui| {
                        match state.active_tab {
                            AgentTab::Installed => {
                                if let Some(idx) = state.selected_index {
                                    if let Some(agent) = detected.get(idx) {
                                        detail_installed(ui, agent);
                                    } else {
                                        detail_placeholder(ui);
                                    }
                                } else if let Some(first) = detected.first() {
                                    state.selected_index = Some(0);
                                    detail_installed(ui, first);
                                } else {
                                    detail_placeholder(ui);
                                }
                            }
                            AgentTab::Registry => {
                                if let Some(idx) = state.selected_index {
                                    if let Some(entry) = missing.get(idx) {
                                        detail_registry(ui, entry);
                                    } else {
                                        detail_placeholder(ui);
                                    }
                                } else if let Some(first) = missing.first() {
                                    state.selected_index = Some(0);
                                    detail_registry(ui, first);
                                } else {
                                    detail_placeholder(ui);
                                }
                            }
                        }
                    });
            });
    });
}

// ── Tab button ──────────────────────────────────────────────────

fn tab_button(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    tab: AgentTab,
    active: &mut AgentTab,
    selected_index: &mut Option<usize>,
) {
    let is_active = *active == tab;
    let text = format!("{label} ({count})");

    let bg = if is_active { Colors::ACCENT.gamma_multiply(0.2) } else { Colors::BG_CARD };
    let text_color = if is_active { Colors::ACCENT } else { Colors::TEXT_DIM };

    let resp = egui::Frame::new()
        .fill(bg)
        .corner_radius(6.0)
        .inner_margin(egui::Vec2::new(16.0, 6.0))
        .stroke(egui::Stroke::new(
            if is_active { 1.0 } else { 0.0 },
            Colors::ACCENT,
        ))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(text_color).strong());
        });

    if resp.response.interact(egui::Sense::click()).clicked() {
        *active = tab;
        *selected_index = None;
    }
}

// ── Agent list item ─────────────────────────────────────────────

fn agent_list_item(
    ui: &mut egui::Ui,
    name: &str,
    description: &str,
    installed: bool,
    selected: bool,
    on_click: impl FnOnce(),
) {
    let bg = if selected {
        Colors::ACCENT.gamma_multiply(0.15)
    } else {
        Colors::BG_CARD
    };
    let border = if selected { Colors::ACCENT } else { Colors::BORDER.gamma_multiply(0.5) };

    let resp = egui::Frame::new()
        .fill(bg)
        .corner_radius(6.0)
        .inner_margin(10.0)
        .stroke(egui::Stroke::new(1.0, border))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let dot_color = if installed { Colors::SUCCESS } else { Colors::ERROR };
                ui.colored_label(dot_color, "●");
                ui.strong(egui::RichText::new(name).color(Colors::TEXT));
            });
            ui.colored_label(
                Colors::TEXT_DIM,
                egui::RichText::new(description).size(11.0),
            );
        });

    if resp.response.interact(egui::Sense::click()).clicked() {
        on_click();
    }

    ui.add_space(2.0);
}

// ── Detail: installed agent ─────────────────────────────────────

fn detail_installed(ui: &mut egui::Ui, agent: &DetectedAgent) {
    let entry = &agent.entry;

    // Title + status
    ui.horizontal(|ui| {
        ui.colored_label(Colors::SUCCESS, egui::RichText::new("●").size(16.0));
        ui.heading(egui::RichText::new(&entry.display_name).color(Colors::TEXT));
        ui.colored_label(Colors::SUCCESS, egui::RichText::new("Installed").size(12.0));
    });

    ui.add_space(4.0);
    ui.colored_label(Colors::TEXT_DIM, &entry.description);

    if !entry.long_description.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&entry.long_description).color(Colors::TEXT).size(12.5));
    }

    ui.add_space(12.0);

    // ── Capabilities ──
    section_header(ui, "Capabilities");
    ui.horizontal_wrapped(|ui| {
        for cap in &entry.capabilities {
            capability_badge(ui, cap);
        }
    });

    ui.add_space(12.0);

    // ── Models ──
    if !entry.models.is_empty() {
        section_header(ui, "Supported Models");
        for model in &entry.models {
            ui.horizontal(|ui| {
                ui.colored_label(Colors::PURPLE, "  ◆");
                ui.label(egui::RichText::new(model).color(Colors::TEXT));
            });
        }
        ui.add_space(12.0);
    }

    // ── Configuration ──
    section_header(ui, "Configuration");
    info_row(ui, "Command", &entry.command);
    if !entry.default_args.is_empty() {
        info_row(ui, "Arguments", &entry.default_args.join(" "));
    }
    info_row(ui, "Transport", &format!("{:?}", entry.transport));
    if let Some(path) = &agent.command_path {
        info_row(ui, "Path", path);
    }

    if let Some(url) = &entry.website {
        ui.add_space(8.0);
        info_row(ui, "Website", url);
    }

    // Tags
    if !entry.tags.is_empty() {
        ui.add_space(12.0);
        section_header(ui, "Tags");
        ui.horizontal_wrapped(|ui| {
            for tag in &entry.tags {
                egui::Frame::new()
                    .fill(Colors::BG_CARD)
                    .corner_radius(4.0)
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(format!("#{tag}")).size(11.0));
                    });
            }
        });
    }
}

// ── Detail: registry (not installed) ────────────────────────────

fn detail_registry(ui: &mut egui::Ui, entry: &RegistryEntry) {
    // Title + status
    ui.horizontal(|ui| {
        ui.colored_label(Colors::ERROR, egui::RichText::new("●").size(16.0));
        ui.heading(egui::RichText::new(&entry.display_name).color(Colors::TEXT));
        ui.colored_label(Colors::ERROR, egui::RichText::new("Not Installed").size(12.0));
    });

    ui.add_space(4.0);
    ui.colored_label(Colors::TEXT_DIM, &entry.description);

    if !entry.long_description.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&entry.long_description).color(Colors::TEXT).size(12.5));
    }

    ui.add_space(12.0);

    // ── Install instructions ──
    section_header(ui, "Installation");
    egui::Frame::new()
        .fill(Colors::BG_DARK)
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(&entry.install_instructions)
                    .monospace()
                    .color(Colors::WARNING)
                    .size(13.0),
            );
        });

    ui.add_space(12.0);

    // ── Capabilities ──
    section_header(ui, "Capabilities");
    ui.horizontal_wrapped(|ui| {
        for cap in &entry.capabilities {
            capability_badge(ui, cap);
        }
    });

    ui.add_space(12.0);

    // ── Models ──
    if !entry.models.is_empty() {
        section_header(ui, "Supported Models");
        for model in &entry.models {
            ui.horizontal(|ui| {
                ui.colored_label(Colors::PURPLE, "  ◆");
                ui.label(egui::RichText::new(model).color(Colors::TEXT));
            });
        }
        ui.add_space(12.0);
    }

    // ── Configuration ──
    section_header(ui, "Configuration");
    info_row(ui, "Command", &entry.command);
    if !entry.default_args.is_empty() {
        info_row(ui, "Arguments", &entry.default_args.join(" "));
    }
    info_row(ui, "Transport", &format!("{:?}", entry.transport));

    if let Some(url) = &entry.website {
        ui.add_space(8.0);
        info_row(ui, "Website", url);
    }

    // Tags
    if !entry.tags.is_empty() {
        ui.add_space(12.0);
        section_header(ui, "Tags");
        ui.horizontal_wrapped(|ui| {
            for tag in &entry.tags {
                egui::Frame::new()
                    .fill(Colors::BG_CARD)
                    .corner_radius(4.0)
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(format!("#{tag}")).size(11.0));
                    });
            }
        });
    }
}

// ── Detail: placeholder ─────────────────────────────────────────

fn detail_placeholder(ui: &mut egui::Ui) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.colored_label(Colors::TEXT_DIM, egui::RichText::new("Select an agent to view details").size(14.0));
    });
}

// ── Helper widgets ──────────────────────────────────────────────

fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong().color(Colors::TEXT_DIM).size(12.0));
    ui.add_space(4.0);
}

fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(format!("{label}:")).size(12.0));
        ui.label(egui::RichText::new(value).monospace().color(Colors::TEXT).size(12.0));
    });
}

fn capability_badge(ui: &mut egui::Ui, cap: &surge_acp::registry::AgentCapability) {
    let (color, icon) = match cap.to_string().as_str() {
        "code" => (Colors::ACCENT, "{}"),
        "plan" => (Colors::PURPLE, "📋"),
        "review" => (Colors::WARNING, "🔍"),
        "test" => (Colors::SUCCESS, "✓"),
        "refactor" => (egui::Color32::from_rgb(148, 226, 213), "⟳"),
        "chat" => (Colors::TEXT_DIM, "💬"),
        _ => (Colors::TEXT_DIM, "•"),
    };

    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .corner_radius(4.0)
        .inner_margin(4.0)
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
        .show(ui, |ui| {
            ui.colored_label(color, egui::RichText::new(format!("{icon} {cap}")).size(11.0));
        });
}
