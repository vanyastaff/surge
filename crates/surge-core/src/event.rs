//! Events emitted throughout the Surge system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::id::{SpecId, SubtaskId, TaskId};
use crate::state::TaskState;

// ── Tool call mirror types ──────────────────────────────────────────

/// Category of tool being invoked (mirrors ACP ToolKind).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// Execution status of a tool call (mirrors ACP ToolCallStatus).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// File location affected by a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLocation {
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// A file diff produced by a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDiff {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

/// Priority of a plan entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

/// Status of a plan entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// A single entry in an agent's execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

// ── Events ──────────────────────────────────────────────────────────

/// Events emitted by Surge for monitoring, UI updates, and logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurgeEvent {
    // --- Agent events ---
    /// Agent connected successfully.
    AgentConnected { agent_name: String },

    /// Agent disconnected.
    AgentDisconnected { agent_name: String },

    /// Agent requested a permission.
    PermissionRequested {
        session_id: String,
        description: String,
        tool_call_id: String,
        options: Vec<String>,
    },

    /// Permission was granted or denied.
    PermissionResolved {
        session_id: String,
        granted: bool,
    },

    // --- Task lifecycle events ---
    /// Task state changed.
    TaskStateChanged {
        task_id: TaskId,
        old_state: TaskState,
        new_state: TaskState,
    },

    /// Subtask started execution.
    SubtaskStarted {
        task_id: TaskId,
        subtask_id: SubtaskId,
    },

    /// Subtask completed.
    SubtaskCompleted {
        task_id: TaskId,
        subtask_id: SubtaskId,
        success: bool,
    },

    // --- File events ---
    /// File operation performed by agent.
    FileOperation { operation: String, path: PathBuf },

    // --- Terminal events ---
    /// Terminal created for command execution.
    TerminalCreated {
        terminal_id: String,
        command: String,
    },

    /// Terminal produced output.
    TerminalOutput {
        terminal_id: String,
        output: String,
    },

    /// Terminal command exited.
    TerminalExited {
        terminal_id: String,
        exit_code: Option<u32>,
    },

    /// Terminal was killed.
    TerminalKilled { terminal_id: String },

    // --- Tool call events ---
    /// Agent initiated a tool call with full metadata.
    ToolCallStarted {
        session_id: String,
        call_id: String,
        title: String,
        kind: ToolKind,
        locations: Vec<ToolLocation>,
        raw_input: Option<String>,
    },

    /// Tool call received an incremental update.
    ToolCallUpdated {
        session_id: String,
        call_id: String,
        status: Option<ToolCallStatus>,
        title: Option<String>,
        diffs: Vec<ToolDiff>,
        locations: Vec<ToolLocation>,
        raw_output: Option<String>,
    },

    // --- Streaming events ---
    /// Agent message chunk received during prompt streaming.
    AgentMessageChunk { session_id: String, text: String },

    /// Agent thought/reasoning chunk received.
    AgentThoughtChunk { session_id: String, text: String },

    // --- Plan events ---
    /// Agent shared or updated its execution plan.
    PlanUpdated {
        session_id: String,
        entries: Vec<PlanEntry>,
    },

    // --- Spec events ---
    /// Spec was loaded or created.
    SpecLoaded { spec_id: SpecId },
}
