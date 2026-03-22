pub mod agent_hub;
pub mod dashboard;
pub mod diff_viewer;
pub mod execution;
pub mod kanban;
pub mod terminal;

/// Which panel is currently active in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePanel {
    #[default]
    Dashboard,
    Kanban,
    Execution,
    AgentHub,
    DiffViewer,
    Terminal,
}

impl ActivePanel {
    /// Human-readable label for the panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Kanban => "Kanban Board",
            Self::Execution => "Execution",
            Self::AgentHub => "Agent Hub",
            Self::DiffViewer => "Diff Viewer",
            Self::Terminal => "Terminal",
        }
    }

    /// Icon (emoji) for the panel.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Dashboard => "📊",
            Self::Kanban => "📋",
            Self::Execution => "▶",
            Self::AgentHub => "🤖",
            Self::DiffViewer => "🔀",
            Self::Terminal => "🖥",
        }
    }

    /// All panel variants in display order.
    pub fn all() -> &'static [ActivePanel] {
        &[
            Self::Dashboard,
            Self::Kanban,
            Self::Execution,
            Self::AgentHub,
            Self::DiffViewer,
            Self::Terminal,
        ]
    }
}
