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

#[derive(Debug, Error)]
pub enum StageError {
    #[error("agent crashed: {0}")]
    AgentCrashed(String),

    #[error("agent reported undeclared outcome: {0}")]
    UndeclaredOutcome(String),

    #[error("human gate rejected (timeout or explicit)")]
    HumanGateRejected,

    #[error("human gate has TimeoutAction::Continue but no default outcome configured")]
    HumanGateContinueWithoutDefault,

    #[error("storage error: {0}")]
    Storage(String),

    #[error("bridge error: {0}")]
    Bridge(String),

    #[error("cancelled")]
    Cancelled,

    #[error("internal: {0}")]
    Internal(String),
}

impl From<StageError> for EngineError {
    fn from(e: StageError) -> Self {
        EngineError::Internal(format!("stage error: {e}"))
    }
}
