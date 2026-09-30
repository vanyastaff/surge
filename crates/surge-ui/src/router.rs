use gpui_kit::assets::IconName;

/// All screens available in Surge UI.
///
/// Primary destinations follow the operator's work; setup and specialist
/// tools remain available through Customize and contextual navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    /// Tasks — work and progress in the current project.
    Fleet,
    /// Roadmap — description → milestones → runs.
    Roadmap,
    /// Results — per-run evidence, stage progress, and event log.
    Runs,
    /// Flow — the DAG editor.
    Flow,
    /// Decisions — gates and failures that need the operator.
    Inbox,
    /// Backlog — triage → ready → in flight → shipped.
    Backlog,
    /// Agents — the crew: capacity, health, config.
    Agents,
    /// Memory — the project knowledge graph.
    ContextMemory,
    Settings,
    // ── Secondary screens (palette / contextual) ──
    SpecWizard,
    AgentHub,
    AgentTerminals,
}

impl Screen {
    /// Display name for the sidebar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fleet => "Tasks",
            Self::Roadmap => "Plan",
            Self::Runs => "Results",
            Self::Flow => "Workflows",
            Self::Inbox => "Decisions",
            Self::Backlog => "Backlog",
            Self::Agents => "Agents",
            Self::ContextMemory => "Memory",
            Self::Settings => "Settings",
            Self::SpecWizard => "New task",
            Self::AgentHub => "Agent Hub",
            Self::AgentTerminals => "Terminals",
        }
    }

    /// Lucide icon for this screen. Non-default icons must be listed in
    /// [`crate::assets`].
    pub fn icon(self) -> IconName {
        match self {
            Self::Fleet => IconName::LayoutDashboard,
            Self::Roadmap => IconName::Map,
            Self::Runs => IconName::Activity,
            Self::Flow => IconName::Workflow,
            Self::Inbox => IconName::Inbox,
            Self::Backlog => IconName::Kanban,
            Self::Agents => IconName::Bot,
            Self::ContextMemory => IconName::Brain,
            Self::Settings => IconName::Settings,
            Self::SpecWizard => IconName::Plus,
            Self::AgentHub => IconName::Bot,
            Self::AgentTerminals => IconName::SquareTerminal,
        }
    }

    /// Keyboard shortcut label (for sidebar badges).
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::Fleet => Some("Ctrl+1"),
            Self::Roadmap => Some("Ctrl+2"),
            Self::Runs => Some("Ctrl+3"),
            Self::Flow => Some("Ctrl+4"),
            Self::Inbox => Some("Ctrl+5"),
            Self::Backlog => Some("Ctrl+6"),
            Self::Agents => Some("Ctrl+7"),
            Self::ContextMemory => Some("Ctrl+8"),
            Self::Settings => Some("Ctrl+9"),
            _ => None,
        }
    }

    /// Everyday destinations, in the order work moves through them.
    pub fn sidebar_items() -> &'static [Screen] {
        &[
            Self::Fleet,
            Self::Roadmap,
            Self::Inbox,
            Self::Runs,
            Self::Settings,
        ]
    }

    /// Existing setup and specialist routes, always discoverable in Customize.
    pub fn customize_items() -> &'static [Screen] {
        &[
            Self::Flow,
            Self::Backlog,
            Self::Agents,
            Self::ContextMemory,
            Self::SpecWizard,
            Self::AgentHub,
            Self::AgentTerminals,
        ]
    }
}
