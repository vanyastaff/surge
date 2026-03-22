//! Events emitted throughout the Surge system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::id::{SpecId, SubtaskId, TaskId};
use crate::state::TaskState;

/// Events emitted by Surge for monitoring, UI updates, and logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurgeEvent {
    // --- Agent events ---
    /// Agent connected successfully.
    AgentConnected { agent_name: String },

    /// Agent disconnected.
    AgentDisconnected { agent_name: String },

    /// Agent requested a permission.
    PermissionRequested { description: String },

    /// Permission was granted or denied.
    PermissionResolved { granted: bool },

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
    /// Terminal command executed.
    TerminalCommand {
        command: String,
        exit_code: Option<i32>,
    },

    // --- Spec events ---
    /// Spec was loaded or created.
    SpecLoaded { spec_id: SpecId },
}
