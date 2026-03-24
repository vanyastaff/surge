use gpui::*;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};

use crate::app_state::AppState;
use crate::theme;

/// Settings tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Agents,
    Pipeline,
    Git,
    Appearance,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Agents => "Agents",
            Self::Pipeline => "Pipeline",
            Self::Git => "Git",
            Self::Appearance => "Appearance",
        }
    }

    fn all() -> &'static [SettingsTab] {
        &[
            Self::General,
            Self::Agents,
            Self::Pipeline,
            Self::Git,
            Self::Appearance,
        ]
    }
}

/// Agent config entry.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub enabled: bool,
    pub model: String,
    pub routing: String,
}

/// Pipeline gate toggle.
#[derive(Debug, Clone)]
pub struct GateToggle {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

/// Settings screen.
pub struct SettingsScreen {
    state: Entity<AppState>,
    active_tab: SettingsTab,
    // Pipeline (UI-only toggles, initialized from config)
    gates: Vec<GateToggle>,
}

impl SettingsScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            active_tab: SettingsTab::General,
            gates: vec![
                GateToggle {
                    name: "After Spec".into(),
                    description: "Review gate after spec creation".into(),
                    enabled: true,
                },
                GateToggle {
                    name: "After Plan".into(),
                    description: "Review gate after planning phase".into(),
                    enabled: true,
                },
                GateToggle {
                    name: "After Each Subtask".into(),
                    description: "Review gate after each subtask completes".into(),
                    enabled: false,
                },
                GateToggle {
                    name: "After QA".into(),
                    description: "Review gate after QA review".into(),
                    enabled: true,
                },
            ],
        }
    }

    /// Read general settings from AppState.
    fn project_path(&self, cx: &Context<Self>) -> String {
        let state = self.state.read(cx);
        state
            .project_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no project)".to_string())
    }

    fn default_ide(&self, cx: &Context<Self>) -> String {
        let state = self.state.read(cx);
        state
            .config
            .as_ref()
            .and_then(|c| c.ide.editor.clone())
            .unwrap_or_else(|| "VS Code".to_string())
    }

    fn max_parallel(&self, cx: &Context<Self>) -> usize {
        let state = self.state.read(cx);
        state
            .config
            .as_ref()
            .map(|c| c.pipeline.max_parallel)
            .unwrap_or(3)
    }

    /// Build agent configs from AppState.
    fn build_agent_configs(&self, cx: &Context<Self>) -> Vec<AgentConfig> {
        let state = self.state.read(cx);
        state
            .installed_agents
            .iter()
            .map(|a| AgentConfig {
                name: a.entry.id.clone(),
                enabled: true, // Installed agents are "enabled".
                model: a
                    .entry
                    .models
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "-".to_string()),
                routing: "-".to_string(),
            })
            .collect()
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = SettingsTab::all()
            .iter()
            .map(|&tab| {
                let is_active = tab == self.active_tab;
                div()
                    .id(SharedString::from(format!("stab-{}", tab.label())))
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .text_sm()
                    .text_color(if is_active {
                        theme::PRIMARY
                    } else {
                        theme::TEXT_MUTED
                    })
                    .bg(if is_active {
                        theme::PRIMARY.opacity(0.1)
                    } else {
                        gpui::transparent_black()
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.active_tab = tab;
                        cx.notify();
                    }))
                    .child(tab.label().to_string())
            })
            .collect();

        div().h_flex().gap_1().children(tabs)
    }

    fn render_tab_content(&self, cx: &mut Context<Self>) -> Div {
        match self.active_tab {
            SettingsTab::General => self.render_general(cx),
            SettingsTab::Agents => self.render_agents(cx),
            SettingsTab::Pipeline => self.render_pipeline(cx),
            SettingsTab::Git => self.render_git(cx),
            SettingsTab::Appearance => self.render_appearance(),
        }
    }

    fn render_general(&self, cx: &Context<Self>) -> Div {
        let ide = self.default_ide(cx);
        let project_path = self.project_path(cx);
        div()
            .v_flex()
            .gap_4()
            .child(self.section("Editor & IDE"))
            .child(self.setting_row("Default IDE", &ide))
            .child(self.section("Paths"))
            .child(self.setting_row("Project Path", &project_path))
            .child(self.setting_row("Surge Directory", ".surge"))
            .child(self.section("Spec Format"))
            .child(self.setting_row("Format", "TOML"))
            .child(self.setting_row("Spec Directory", "specs/"))
    }

    fn render_agents(&self, cx: &Context<Self>) -> Div {
        let agents = self.build_agent_configs(cx);
        let rows: Vec<Div> = agents
            .iter()
            .map(|agent| {
                let status_color = if agent.enabled {
                    theme::SUCCESS
                } else {
                    theme::TEXT_MUTED
                };
                let status_label = if agent.enabled { "Enabled" } else { "Disabled" };

                div()
                    .h_flex()
                    .gap_3()
                    .p_3()
                    .rounded_lg()
                    .bg(theme::BACKGROUND)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    // Status dot
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(status_color)
                            .mt_1(),
                    )
                    // Info
                    .child(
                        div()
                            .flex_1()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(agent.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(format!("Model: {}", agent.model)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(format!("Routing: {}", agent.routing)),
                            ),
                    )
                    // Status badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(status_color.opacity(0.15))
                            .text_color(status_color)
                            .child(status_label.to_string()),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_3()
            .child(self.section("Configured Agents"))
            .children(rows)
            .child(
                div().pt_2().child(
                    Button::new("settings-add-agent")
                        .ghost()
                        .label("+ Add Agent"),
                ),
            )
    }

    fn render_pipeline(&self, cx: &mut Context<Self>) -> Div {
        let gate_rows: Vec<Div> = self
            .gates
            .iter()
            .enumerate()
            .map(|(idx, gate)| {
                let indicator_color = if gate.enabled {
                    theme::SUCCESS
                } else {
                    theme::TEXT_MUTED
                };

                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.05))
                    .child(
                        div()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(gate.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(gate.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("gate-{idx}")))
                            .w(px(40.0))
                            .h(px(22.0))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(if gate.enabled {
                                theme::SUCCESS.opacity(0.3)
                            } else {
                                theme::TEXT_MUTED.opacity(0.2)
                            })
                            .child(
                                div()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded_full()
                                    .bg(indicator_color)
                                    .mt(px(3.0))
                                    .ml(if gate.enabled { px(21.0) } else { px(3.0) }),
                            )
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.gates[idx].enabled = !this.gates[idx].enabled;
                                cx.notify();
                            })),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_4()
            .child(self.section("Pipeline Gates"))
            .child(
                div()
                    .v_flex()
                    .p_3()
                    .rounded_lg()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .children(gate_rows),
            )
            .child(self.section("Concurrency"))
            .child(self.setting_row("Max Parallel Tasks", &format!("{}", self.max_parallel(cx))))
    }

    fn render_git(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let current_branch = state.current_branch.clone();

        div()
            .v_flex()
            .gap_4()
            .child(self.section("Git Configuration"))
            .child(self.setting_row("Current Branch", &current_branch))
            .child(self.setting_row("Branch Prefix", "surge/"))
            .child(self.setting_row("Auto-commit", "Enabled"))
            .child(self.setting_row("Worktree Directory", ".surge/worktrees/"))
            .child(self.section("PR Settings"))
            .child(self.setting_row("Auto-create PRs", "Enabled"))
            .child(self.setting_row("PR Template", "Default"))
            .child(self.setting_row("Label Prefix", "surge/"))
    }

    fn render_appearance(&self) -> Div {
        div()
            .v_flex()
            .gap_4()
            .child(self.section("Theme"))
            .child(self.setting_row("Current Theme", "Surge Dark"))
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child("Color Preview".to_string()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(self.color_swatch("Primary", theme::PRIMARY))
                            .child(self.color_swatch("Success", theme::SUCCESS))
                            .child(self.color_swatch("Warning", theme::WARNING))
                            .child(self.color_swatch("Error", theme::ERROR))
                            .child(self.color_swatch("Surface", theme::SURFACE))
                            .child(self.color_swatch("Background", theme::BACKGROUND)),
                    ),
            )
            .child(self.section("Layout"))
            .child(self.setting_row("Sidebar Position", "Left"))
            .child(self.setting_row("Compact Mode", "Disabled"))
    }

    fn section(&self, title: &str) -> Div {
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::TEXT_PRIMARY)
            .pb_1()
            .border_b_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(title.to_string())
    }

    fn setting_row(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .justify_between()
            .items_center()
            .py(px(4.0))
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_PRIMARY)
                    .px_3()
                    .py(px(4.0))
                    .rounded_md()
                    .bg(theme::BACKGROUND)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .child(value.to_string()),
            )
    }

    fn color_swatch(&self, label: &str, color: Hsla) -> Div {
        div()
            .v_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(color)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.2)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            .overflow_hidden()
            // Header
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Settings".to_string()),
            )
            // Tab bar
            .child(self.render_tab_bar(cx))
            // Tab content
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .p_4()
                    .rounded_lg()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .overflow_hidden()
                    .child(self.render_tab_content(cx)),
            )
            // Save button
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("settings-reset").ghost().label("Reset"))
                    .child(
                        Button::new("settings-save")
                            .primary()
                            .label("Save Settings"),
                    ),
            )
    }
}
