## Goal

Build a "complete, usable local habit-tracker application through Surge" so a person can create habits, track today's completion, and return after reloading without losing their habits or daily records. Deliver working software through two executed milestones, with visible implementation and verification evidence rather than stopping at plans or a demonstration summary.

## Context

The project is initially empty apart from a placeholder README, repository metadata, and Surge bootstrap metadata. There is no existing application or selected web stack. Choose a suitable ordinary web application stack for local use. This description is the source contract for the Roadmap Planner, Spec Author, and Flow Generator. The project will receive a separate follow-up after the working result, so preserve its earlier history and artifacts.

## Requirements

- **P0 — Execute two milestones in order.** Plan and execute milestone 1: create habits, display them in a clear usable interface, and check or uncheck completion for the current day. Then execute milestone 2: persist habits and daily completion across reloads and provide filtering. Both milestones must produce implemented, verified behavior recorded in Surge.
- **P0 — Habit creation and display.** A user can create a named habit and see it in the habit list. Reject empty or whitespace-only names with an understandable validation message and a way to correct the input.
- **P0 — Daily completion.** Each habit exposes its current-day completion state and allows the user to check and uncheck it. Habits can be updated independently. Use the user's local calendar day; a new day has its own completion state and changes must not overwrite another day's record.
- **P0 — Persistence.** Created habits and completion records associated with their calendar dates survive page reloads. Returning on another day retains prior records while showing that day's completion states.
- **P0 — Filtering.** Provide clearly labeled filters for all habits, habits completed today, and habits incomplete today. Filtering must reflect completion changes and must not discard habits or completion records.
- **P0 — Usable and accessible interaction.** Habit creation, completion toggles, and filters are operable by keyboard with logical focus order, visible focus, accessible names, and programmatically exposed control states. Completion status must be understandable without relying on color alone.
- **P0 — Empty and error states.** Explain an empty habit list and how to add the first habit; distinguish a filter with no matching habits. Surface validation and persistence failures with understandable messages and an actionable recovery path. Do not silently present failed persistence as successful saving.
- **P0 — Local operation.** Deliver an application that runs locally using documented prerequisite, installation, startup, and automated-test commands. Documentation must describe how persistence works for the user and any material local-use limitations.
- **P0 — Meaningful automated checks.** Supply and run automated tests covering habit creation and invalid input, independent check/uncheck behavior, date-specific completion, reload persistence, filtering, and relevant empty/error behavior. Verify keyboard interaction and accessible control semantics, recording which checks were automated and which were manual. Record actual commands and results; do not claim unexecuted checks passed.
- **P0 — Flow runtime and model.** Use the "codex-acp runtime and exact gpt-6.1-sol model for every agent stage in the generated Flow". Make these settings explicit so no agent stage inherits a different runtime or model.
- **P0 — Visible delivery evidence and continuity.** Keep "real implementation, checks, and results visible in the Flow/task history" throughout both milestones. Preserve project history and generated artifacts for the separate follow-up. Completion requires the working application, documentation, and verification evidence; planning documents or a demonstration summary alone do not satisfy delivery.

## Out of Scope

- User accounts, authentication, multi-user collaboration, remote services, or cross-device synchronization.
- Production hosting or public deployment; the required delivery target is local execution.
- Reminders, notifications, streak analytics, habit scheduling, or a historical-record browsing interface.
- Habit editing or deletion beyond the required creation and daily-completion operations.
- Implementing or anticipating the separate follow-up request, or resetting/removing earlier project history and artifacts.
