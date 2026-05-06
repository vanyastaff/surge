//! Core types for task intake.

// Task identification
pub type TaskId = String;

// Priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    // Filled in by later tasks.
}

// Decision types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageDecision {
    // Filled in by later tasks.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier1Decision {
    // Filled in by later tasks.
}

// Task events and details
#[derive(Debug, Clone)]
pub enum TaskEventKind {
    // Filled in by later tasks.
}

#[derive(Debug, Clone)]
pub struct TaskEvent {
    // Filled in by later tasks.
}

#[derive(Debug, Clone)]
pub struct TaskDetails {
    // Filled in by later tasks.
}

#[derive(Debug, Clone)]
pub struct TaskSummary {
    // Filled in by later tasks.
}
