use gpui_component::IconName;

/// All screens available in Surge UI.
///
/// The first nine (see [`Screen::sidebar_items`]) are the fleet-ops
/// surfaces from the "Surge - Interactive" concept; everything else is
/// a secondary screen reachable through the command palette or
/// contextual navigation (never faked into the rail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    /// Fleet — the run constellation (mission-control home).
    Fleet,
    /// Roadmap — description → milestones → runs.
    Roadmap,
    /// Runs — per-run cockpit (rail, stage pipeline, event log).
    Runs,
    /// Flow — the DAG editor.
    Flow,
    /// Inbox — decisions blocked on the operator.
    Inbox,
    /// Backlog — triage → ready → in flight → shipped.
    Backlog,
    /// Agents — the crew: capacity, health, config.
    Agents,
    /// Memory — the project knowledge graph.
    ContextMemory,
    Settings,
    // ── Secondary screens (palette / contextual) ──
    Dashboard,
    Kanban,
    TaskDetail,
    GateApproval,
    SpecExplorer,
    SpecWizard,
    AgentHub,
    AgentTerminals,
    DiffViewer,
    FileExplorer,
    Insights,
    Worktrees,
    GitHubIssues,
    GitHubPRs,
}

impl Screen {
    /// Display name for the sidebar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fleet => "Fleet",
            Self::Roadmap => "Roadmap",
            Self::Runs => "Runs",
            Self::Flow => "Flow",
            Self::Inbox => "Inbox",
            Self::Backlog => "Backlog",
            Self::Agents => "Agents",
            Self::ContextMemory => "Memory",
            Self::Settings => "Settings",
            Self::Dashboard => "Dashboard",
            Self::Kanban => "Kanban",
            Self::TaskDetail => "Task Detail",
            Self::GateApproval => "Gate Approval",
            Self::SpecExplorer => "Specs",
            Self::SpecWizard => "New Spec",
            Self::AgentHub => "Agent Hub",
            Self::AgentTerminals => "Terminals",
            Self::DiffViewer => "Diff",
            Self::FileExplorer => "Files",
            Self::Insights => "Insights",
            Self::Worktrees => "Worktrees",
            Self::GitHubIssues => "Issues",
            Self::GitHubPRs => "Pull Requests",
        }
    }

    /// Lucide icon for this screen (from gpui-component IconName).
    pub fn icon(self) -> IconName {
        match self {
            Self::Fleet => IconName::GalleryVerticalEnd,
            Self::Roadmap => IconName::Map,
            Self::Runs => IconName::LoaderCircle,
            Self::Flow => IconName::Inspector,
            Self::Inbox => IconName::Inbox,
            Self::Backlog => IconName::Frame, // kanban columns
            Self::Agents => IconName::Bot,
            Self::ContextMemory => IconName::BookOpen,
            Self::Settings => IconName::Settings,
            Self::Dashboard => IconName::LayoutDashboard,
            Self::Kanban => IconName::Frame,
            Self::TaskDetail => IconName::File,
            Self::GateApproval => IconName::Check,
            Self::SpecExplorer => IconName::Search,
            Self::SpecWizard => IconName::Plus,
            Self::AgentHub => IconName::Bot,
            Self::AgentTerminals => IconName::SquareTerminal,
            Self::DiffViewer => IconName::Replace, // git compare
            Self::FileExplorer => IconName::Folder,
            Self::Insights => IconName::ChartPie,
            Self::Worktrees => IconName::FolderOpen,
            Self::GitHubIssues => IconName::Info,
            Self::GitHubPRs => IconName::GitHub,
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

    /// Screens shown in main sidebar navigation (top section) —
    /// the concept's eight surfaces plus Settings.
    pub fn sidebar_items() -> &'static [Screen] {
        &[
            Self::Fleet,
            Self::Roadmap,
            Self::Runs,
            Self::Flow,
            Self::Inbox,
            Self::Backlog,
            Self::Agents,
            Self::ContextMemory,
            Self::Settings,
        ]
    }
}
