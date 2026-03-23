use gpui::*;

// All global actions triggered by keyboard shortcuts.
actions!(
    surge,
    [
        // Navigation (Ctrl+1..9)
        GoToDashboard,
        GoToKanban,
        GoToSpecs,
        GoToAgents,
        GoToTerminals,
        GoToExecution,
        GoToDiff,
        GoToInsights,
        GoToSettings,
        // UI toggles
        ToggleSidebarAction,
        ToggleCommandPalette,
        // Project
        SwitchProject,
        // Tasks
        NewTask,
        ApproveGate,
        // Diff
        OpenDiffViewer,
    ]
);
