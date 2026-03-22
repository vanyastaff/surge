use gpui_component::IconName;

/// All screens available in Surge UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Dashboard,
    Kanban,
    TaskDetail,
    SpecExplorer,
    SpecWizard,
    AgentHub,
    AgentTerminals,
    LiveExecution,
    DiffViewer,
    FileExplorer,
    Insights,
    Worktrees,
    GitHubIssues,
    GitHubPRs,
    Roadmap,
    ContextMemory,
    Settings,
}

impl Screen {
    /// Display name for the sidebar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Kanban => "Kanban",
            Self::TaskDetail => "Task Detail",
            Self::SpecExplorer => "Specs",
            Self::SpecWizard => "New Spec",
            Self::AgentHub => "Agents",
            Self::AgentTerminals => "Terminals",
            Self::LiveExecution => "Execution",
            Self::DiffViewer => "Diff",
            Self::FileExplorer => "Files",
            Self::Insights => "Insights",
            Self::Worktrees => "Worktrees",
            Self::GitHubIssues => "Issues",
            Self::GitHubPRs => "Pull Requests",
            Self::Roadmap => "Roadmap",
            Self::ContextMemory => "Context",
            Self::Settings => "Settings",
        }
    }

    /// Lucide icon for this screen (from gpui-component IconName).
    pub fn icon(self) -> IconName {
        match self {
            Self::Dashboard => IconName::LayoutDashboard,
            Self::Kanban => IconName::Frame,           // columns layout
            Self::TaskDetail => IconName::File,
            Self::SpecExplorer => IconName::Search,
            Self::SpecWizard => IconName::Plus,
            Self::AgentHub => IconName::Bot,
            Self::AgentTerminals => IconName::SquareTerminal,
            Self::LiveExecution => IconName::Loader,   // activity/spinner
            Self::DiffViewer => IconName::Replace,     // git compare
            Self::FileExplorer => IconName::Folder,
            Self::Insights => IconName::ChartPie,
            Self::Worktrees => IconName::FolderOpen,   // git branch
            Self::GitHubIssues => IconName::Info,      // circle-dot
            Self::GitHubPRs => IconName::GitHub,
            Self::Roadmap => IconName::Map,
            Self::ContextMemory => IconName::BookOpen,
            Self::Settings => IconName::Settings,
        }
    }

    /// Keyboard shortcut label (for sidebar badges).
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::Dashboard => Some("Ctrl+1"),
            Self::Kanban => Some("Ctrl+2"),
            Self::SpecExplorer => Some("Ctrl+3"),
            Self::AgentHub => Some("Ctrl+4"),
            Self::AgentTerminals => Some("Ctrl+5"),
            Self::LiveExecution => Some("Ctrl+6"),
            Self::DiffViewer => Some("Ctrl+7"),
            Self::Insights => Some("Ctrl+8"),
            Self::Settings => Some("Ctrl+9"),
            _ => None,
        }
    }

    /// Screens shown in main sidebar navigation (top section).
    pub fn sidebar_items() -> &'static [Screen] {
        &[
            Self::Dashboard,
            Self::Kanban,
            Self::SpecExplorer,
            Self::AgentHub,
            Self::AgentTerminals,
            Self::LiveExecution,
            Self::DiffViewer,
            Self::FileExplorer,
            Self::Insights,
            Self::Worktrees,
            Self::GitHubIssues,
            Self::GitHubPRs,
            Self::Roadmap,
            Self::ContextMemory,
            Self::Settings,
        ]
    }
}
