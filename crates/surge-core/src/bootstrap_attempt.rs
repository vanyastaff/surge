//! Durable identities for isolated continuation of failed bootstrap implementation.
use crate::{ContentHash, RunId};
use serde::{Deserialize, Serialize};

/// State of a separately owned successor; the original operation stays failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapAttemptState {
    /// Identity committed before any creation effects; cold recovery must refuse.
    Reserved,
    /// Fork, lineage and checked writer closes completed.
    ForkReady,
    /// Admitted by the ordinary engine owner.
    Running,
    /// Creation or execution could not be confirmed; no automatic replacement.
    NeedsAttention,
    /// Successor durably completed.
    Completed,
    /// Successor durably failed.
    Failed,
}

/// Immutable source/request association with a separately changing state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAttempt {
    /// Stable caller request identity, retained across retries.
    pub continuation_id: RunId,
    /// Original bootstrap operation.
    pub operation_id: RunId,
    /// Failed source history, preserved by the fork.
    pub source_run: RunId,
    /// Host-reserved successor identity.
    pub run: RunId,
    /// Exact nonterminal completed stage boundary.
    pub boundary: u64,
    /// Host-captured current runtime identity, never executable client input.
    pub runtime: ContentHash,
    /// Current confirmed ownership state.
    pub state: BootstrapAttemptState,
}
