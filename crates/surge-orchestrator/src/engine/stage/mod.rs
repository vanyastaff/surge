//! Stage execution dispatch.

pub mod agent;
pub mod bindings;
pub mod branch;
pub mod human_gate;
pub mod loop_stage;
pub mod notify;
pub mod subgraph_stage;
pub mod terminal;

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

    /// Loop body subgraph not found in `Graph::subgraphs`.
    #[error("loop body subgraph not found: {0}")]
    LoopBodyMissing(surge_core::keys::SubgraphKey),

    /// Loop iterable resolved to more items than `MAX_LOOP_ITEMS_RESOLVED`.
    #[error("loop iterable too large: {count}/{max}")]
    LoopItemsTooLarge {
        /// Actual number of items resolved.
        count: u32,
        /// Maximum permitted (`MAX_LOOP_ITEMS_RESOLVED`).
        max: u32,
    },

    /// Subgraph reference not found in `Graph::subgraphs`. (Reserved for Task 6.1.)
    #[error("subgraph reference not found: {0}")]
    SubgraphMissing(surge_core::keys::SubgraphKey),

    /// Notify channel delivery failed and `on_failure: Fail` is configured. (Reserved for Phase 8.)
    #[error("notify delivery error: {0}")]
    NotifyDelivery(String),
}

impl From<StageError> for EngineError {
    fn from(e: StageError) -> Self {
        EngineError::Internal(format!("stage error: {e}"))
    }
}
