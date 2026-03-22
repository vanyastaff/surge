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

    /// Icon name (Lucide icon identifiers).
    pub fn icon(self) -> &'static str {
        match self {
            Self::Dashboard => "layout-dashboard",
            Self::Kanban => "columns-3",
            Self::TaskDetail => "file-text",
            Self::SpecExplorer => "file-search",
            Self::SpecWizard => "file-plus",
            Self::AgentHub => "bot",
            Self::AgentTerminals => "terminal",
            Self::LiveExecution => "activity",
            Self::DiffViewer => "git-compare",
            Self::FileExplorer => "folder-tree",
            Self::Insights => "bar-chart-3",
            Self::Worktrees => "git-branch",
            Self::GitHubIssues => "circle-dot",
            Self::GitHubPRs => "git-pull-request",
            Self::Roadmap => "map",
            Self::ContextMemory => "brain",
            Self::Settings => "settings",
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
