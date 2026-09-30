use gpui_kit::*;

// All global actions triggered by keyboard shortcuts.
actions!(
    surge,
    [
        // Application lifecycle
        Quit,
        // Surface navigation (Ctrl+1..9)
        GoToFleet,
        GoToRoadmap,
        GoToRuns,
        GoToFlow,
        GoToInbox,
        GoToBacklog,
        GoToAgents,
        GoToMemory,
        GoToSettings,
        // UI toggles
        ToggleSidebarAction,
        ToggleCommandPalette,
        // Project
        SwitchProject,
        // Tasks
        NewTask,
        OpenProjectDialog,
    ]
);
