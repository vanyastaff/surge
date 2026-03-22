use gpui::*;

// Navigation actions triggered by keyboard shortcuts.
actions!(
    surge,
    [
        GoToDashboard,
        GoToKanban,
        GoToSpecs,
        GoToAgents,
        GoToTerminals,
        GoToExecution,
        GoToDiff,
        GoToInsights,
        GoToSettings,
        ToggleSidebarAction,
        ToggleCommandPalette,
    ]
);
