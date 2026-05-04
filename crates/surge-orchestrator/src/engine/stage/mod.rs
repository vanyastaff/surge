//! Stage execution dispatch.

pub mod agent;
pub mod bindings;
pub mod branch;
pub mod human_gate;
pub mod terminal;
pub mod notify;

use crate::engine::error::EngineError;
use surge_core::keys::OutcomeKey;
use thiserror::Error;

/// Outcome of one stage's execution. The cursor's next position is determined
/// by routing on this `OutcomeKey`.
pub type StageResult = Result<OutcomeKey, StageError>;

/// Errors that can occur during a single stage's execution.
#[derive(Debug, Error)]
pub enum StageError {
    /// The ACP agent process crashed or the session ended abnormally.
    #[error("agent crashed: {0}")]
    AgentCrashed(String),

    /// The agent reported an outcome not declared in the node's spec.
    #[error("agent reported undeclared outcome: {0}")]
    UndeclaredOutcome(String),

    /// A `HumanGate` timed out or was explicitly rejected.
    #[error("human gate rejected (timeout or explicit)")]
    HumanGateRejected,

    /// `TimeoutAction::Continue` was set but no default outcome is configured.
    #[error("human gate has TimeoutAction::Continue but no default outcome configured")]
    HumanGateContinueWithoutDefault,

    /// A persistence write failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An ACP bridge call failed.
    #[error("bridge error: {0}")]
    Bridge(String),

    /// The run was cancelled while the stage was executing.
    #[error("cancelled")]
    Cancelled,

    /// An unexpected internal condition occurred within the stage executor.
    #[error("internal: {0}")]
    Internal(String),
}

impl From<StageError> for EngineError {
    fn from(e: StageError) -> Self {
        EngineError::Internal(format!("stage error: {e}"))
    }
}
