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
    pub selected_index: Option<usize>,
}

/// Render the Agent Hub panel.
pub fn show(
    ui: &mut egui::Ui,
    detected: &[DetectedAgent],
    missing: &[RegistryEntry],
    state: &mut AgentHubState,
) {
    // ── Header ──
    ui.vertical(|ui| {
        ui.heading(egui::RichText::new("Agent Hub").size(22.0));
        ui.add_space(2.0);
        ui.colored_label(
            Colors::TEXT_DIM,
            format!(
                "{} installed  ·  {} in registry  ·  {} total",
                detected.len(),
                missing.len(),
                detected.len() + missing.len()
            ),
        );
    });
    ui.add_space(12.0);

    // ── Tabs ──
    ui.horizontal(|ui| {
        tab_btn(ui, "✅  Installed", detected.len(), AgentTab::Installed, state);
        ui.add_space(4.0);
        tab_btn(ui, "📦  Registry", missing.len(), AgentTab::Registry, state);
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // ── Content: list on top, detail below (vertical, fully adaptive) ──
    let has_selection = state.selected_index.is_some();

    // Agent list (scrollable, full width, compact cards)
    let list_height = if has_selection { 200.0 } else { 500.0 };

    egui::ScrollArea::vertical()
        .id_salt("agent_list_scroll")
        .max_height(list_height)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            match state.active_tab {
                AgentTab::Installed => {
                    if detected.is_empty() {
                        empty_state(ui, "No agents detected on this system.", "Switch to the Registry tab to browse available agents.");
                    } else {
                        for (i, agent) in detected.iter().enumerate() {
                            let selected = state.selected_index == Some(i);
                            if list_card(ui, &agent.entry.display_name, &agent.entry.id, &agent.entry.description, true, selected) {
                                state.selected_index = Some(i);
                            }
                        }
                    }
                }
                AgentTab::Registry => {
                    if missing.is_empty() {
                        empty_state(ui, "All known agents are already installed!", "");
                    } else {
                        for (i, entry) in missing.iter().enumerate() {
                            let selected = state.selected_index == Some(i);
                            if list_card(ui, &entry.display_name, &entry.id, &entry.description, false, selected) {
                                state.selected_index = Some(i);
                            }
                        }
                    }
                }
            }
        });

    // ── Detail panel (below the list, full width) ──
    if state.selected_index.is_none() {
        // Auto-select first if available
        match state.active_tab {
            AgentTab::Installed if !detected.is_empty() => state.selected_index = Some(0),
            AgentTab::Registry if !missing.is_empty() => state.selected_index = Some(0),
            _ => {}
        }
    }

    if state.selected_index.is_some() {
        ui.add_space(8.0);

        egui::Frame::new()
            .fill(Colors::BG_PANEL)
            .corner_radius(10.0)
            .inner_margin(20.0)
            .stroke(egui::Stroke::new(1.0, Colors::BORDER))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                egui::ScrollArea::vertical()
                    .id_salt("agent_detail_scroll")
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        match state.active_tab {
                            AgentTab::Installed => {
                                if let Some(agent) = state.selected_index.and_then(|i| detected.get(i)) {
                                    detail_installed(ui, agent);
                                }
                            }
                            AgentTab::Registry => {
                                if let Some(entry) = state.selected_index.and_then(|i| missing.get(i)) {
                                    detail_registry(ui, entry);
                                }
                            }
                        }
                    });
            });
    }
}

// ═══════════════════════════════════════════════════════════════
// Widgets
// ═══════════════════════════════════════════════════════════════

fn tab_btn(ui: &mut egui::Ui, label: &str, count: usize, tab: AgentTab, state: &mut AgentHubState) {
    let active = state.active_tab == tab;
    let bg = if active { Colors::ACCENT.gamma_multiply(0.2) } else { Colors::BG_CARD };
    let fg = if active { Colors::ACCENT } else { Colors::TEXT_DIM };
    let stroke = if active { egui::Stroke::new(1.0, Colors::ACCENT) } else { egui::Stroke::NONE };

    let resp = egui::Frame::new()
        .fill(bg)
        .corner_radius(6.0)
        .inner_margin(egui::Vec2::new(14.0, 6.0))
        .stroke(stroke)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(format!("{label}  {count}")).color(fg).strong().size(13.0));
        });

    if resp.response.interact(egui::Sense::click()).clicked() {
        state.active_tab = tab;
        state.selected_index = None;
    }
}

fn list_card(ui: &mut egui::Ui, name: &str, id: &str, desc: &str, installed: bool, selected: bool) -> bool {
    let bg = if selected { Colors::ACCENT.gamma_multiply(0.12) } else { Colors::BG_CARD };
    let border_color = if selected { Colors::ACCENT } else { Colors::BORDER.gamma_multiply(0.4) };
    let border_w = if selected { 1.5 } else { 1.0 };

    let resp = egui::Frame::new()
        .fill(bg)
        .corner_radius(8.0)
        .inner_margin(12.0)
        .stroke(egui::Stroke::new(border_w, border_color))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            ui.horizontal(|ui| {
                // Status dot
                let dot = if installed { Colors::SUCCESS } else { Colors::ERROR };
                ui.colored_label(dot, egui::RichText::new("●").size(14.0));

                // Name + id
                ui.strong(egui::RichText::new(name).size(14.0).color(Colors::TEXT));
                ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(id).size(11.0));
            });

            // Description (wrapped)
            ui.add_space(2.0);
            ui.label(egui::RichText::new(desc).size(11.5).color(Colors::TEXT_DIM));
        });

    ui.add_space(3.0);
    resp.response.interact(egui::Sense::click()).clicked()
}

fn empty_state(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(title).size(14.0));
        if !subtitle.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(subtitle).size(12.0));
        }
    });
}

// ═══════════════════════════════════════════════════════════════
// Detail views
// ═══════════════════════════════════════════════════════════════

fn detail_installed(ui: &mut egui::Ui, agent: &DetectedAgent) {
    let e = &agent.entry;

    // Header
    ui.horizontal(|ui| {
        ui.colored_label(Colors::SUCCESS, egui::RichText::new("●").size(18.0));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&e.display_name).size(20.0).strong().color(Colors::TEXT));

        // Status badge
        egui::Frame::new()
            .fill(Colors::SUCCESS.gamma_multiply(0.15))
            .corner_radius(4.0)
            .inner_margin(egui::Vec2::new(8.0, 2.0))
            .show(ui, |ui| {
                ui.colored_label(Colors::SUCCESS, egui::RichText::new("Installed").size(11.0));
            });
    });

    ui.add_space(6.0);
    ui.label(egui::RichText::new(&e.description).size(13.0).color(Colors::TEXT_DIM));

    if !e.long_description.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&e.long_description).size(12.5).color(Colors::TEXT));
    }

    ui.add_space(16.0);

    // Sections in a grid-like layout using two columns
    let section_width = ui.available_width();
    let use_two_cols = section_width > 500.0;

    if use_two_cols {
        ui.columns(2, |cols| {
            // Left column: capabilities + models
            section_capabilities(&mut cols[0], &e.capabilities);
            cols[0].add_space(12.0);
            section_models(&mut cols[0], &e.models);

            // Right column: configuration + path
            section_config(&mut cols[1], e, agent.command_path.as_deref());
            cols[1].add_space(12.0);
            section_tags(&mut cols[1], &e.tags);
        });
    } else {
        section_capabilities(ui, &e.capabilities);
        ui.add_space(12.0);
        section_models(ui, &e.models);
        ui.add_space(12.0);
        section_config(ui, e, agent.command_path.as_deref());
        ui.add_space(12.0);
        section_tags(ui, &e.tags);
    }
}

fn detail_registry(ui: &mut egui::Ui, e: &RegistryEntry) {
    // Header
    ui.horizontal(|ui| {
        ui.colored_label(Colors::ERROR, egui::RichText::new("●").size(18.0));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&e.display_name).size(20.0).strong().color(Colors::TEXT));

        egui::Frame::new()
            .fill(Colors::ERROR.gamma_multiply(0.15))
            .corner_radius(4.0)
            .inner_margin(egui::Vec2::new(8.0, 2.0))
            .show(ui, |ui| {
                ui.colored_label(Colors::ERROR, egui::RichText::new("Not Installed").size(11.0));
            });
    });

    ui.add_space(6.0);
    ui.label(egui::RichText::new(&e.description).size(13.0).color(Colors::TEXT_DIM));

    if !e.long_description.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&e.long_description).size(12.5).color(Colors::TEXT));
    }

    ui.add_space(12.0);

    // Install box
    section_label(ui, "INSTALLATION");
    egui::Frame::new()
        .fill(Colors::BG_DARK)
        .corner_radius(6.0)
        .inner_margin(12.0)
        .stroke(egui::Stroke::new(1.0, Colors::WARNING.gamma_multiply(0.3)))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.colored_label(Colors::WARNING, "$ ");
                ui.label(
                    egui::RichText::new(&e.install_instructions)
                        .monospace()
                        .color(Colors::WARNING)
                        .size(13.0),
                );
            });
        });

    ui.add_space(16.0);

    let section_width = ui.available_width();
    let use_two_cols = section_width > 500.0;

    if use_two_cols {
        ui.columns(2, |cols| {
            section_capabilities(&mut cols[0], &e.capabilities);
            cols[0].add_space(12.0);
            section_models(&mut cols[0], &e.models);

            section_config(&mut cols[1], e, None);
            cols[1].add_space(12.0);
            section_tags(&mut cols[1], &e.tags);
        });
    } else {
        section_capabilities(ui, &e.capabilities);
        ui.add_space(12.0);
        section_models(ui, &e.models);
        ui.add_space(12.0);
        section_config(ui, e, None);
        ui.add_space(12.0);
        section_tags(ui, &e.tags);
    }
}

// ═══════════════════════════════════════════════════════════════
// Reusable sections
// ═══════════════════════════════════════════════════════════════

fn section_label(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong().color(Colors::TEXT_DIM).size(11.0));
    ui.add_space(6.0);
}

fn section_capabilities(ui: &mut egui::Ui, caps: &[surge_acp::registry::AgentCapability]) {
    section_label(ui, "CAPABILITIES");
    ui.horizontal_wrapped(|ui| {
        for cap in caps {
            capability_badge(ui, cap);
        }
    });
}

fn section_models(ui: &mut egui::Ui, models: &[String]) {
    if models.is_empty() { return; }
    section_label(ui, "MODELS");

    for model in models {
        egui::Frame::new()
            .fill(Colors::PURPLE.gamma_multiply(0.08))
            .corner_radius(4.0)
            .inner_margin(egui::Vec2::new(8.0, 3.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Colors::PURPLE, egui::RichText::new("◆").size(10.0));
                    ui.label(egui::RichText::new(model).size(12.0).color(Colors::TEXT));
                });
            });
        ui.add_space(2.0);
    }
}

fn section_config(ui: &mut egui::Ui, entry: &RegistryEntry, path: Option<&str>) {
    section_label(ui, "CONFIGURATION");

    egui::Frame::new()
        .fill(Colors::BG_DARK)
        .corner_radius(6.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            config_row(ui, "Command", &entry.command);
            if !entry.default_args.is_empty() {
                config_row(ui, "Args", &entry.default_args.join(" "));
            }
            config_row(ui, "Transport", &format!("{:?}", entry.transport));
            if let Some(p) = path {
                config_row(ui, "Path", p);
            }
            if let Some(url) = &entry.website {
                config_row(ui, "Website", url);
            }
        });
}

fn config_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(format!("{label:<10}")).monospace().size(11.0));
        ui.label(egui::RichText::new(value).monospace().color(Colors::ACCENT).size(11.0));
    });
}

fn section_tags(ui: &mut egui::Ui, tags: &[String]) {
    if tags.is_empty() { return; }
    section_label(ui, "TAGS");

    ui.horizontal_wrapped(|ui| {
        for tag in tags {
            egui::Frame::new()
                .fill(Colors::BG_CARD)
                .corner_radius(10.0)
                .inner_margin(egui::Vec2::new(8.0, 2.0))
                .show(ui, |ui| {
                    ui.colored_label(Colors::TEXT_DIM, egui::RichText::new(format!("#{tag}")).size(10.5));
                });
        }
    });
}

fn capability_badge(ui: &mut egui::Ui, cap: &surge_acp::registry::AgentCapability) {
    let (color, icon) = match cap.to_string().as_str() {
        "code" => (Colors::ACCENT, "{ }"),
        "plan" => (Colors::PURPLE, "▦"),
        "review" => (Colors::WARNING, "◎"),
        "test" => (Colors::SUCCESS, "✓"),
        "refactor" => (egui::Color32::from_rgb(148, 226, 213), "↻"),
        "chat" => (Colors::TEXT_DIM, "◯"),
        _ => (Colors::TEXT_DIM, "·"),
    };

    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .corner_radius(4.0)
        .inner_margin(egui::Vec2::new(8.0, 3.0))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.3)))
        .show(ui, |ui| {
            ui.colored_label(color, egui::RichText::new(format!("{icon}  {cap}")).size(11.5));
        });
}
