//! Validated child launch data and typed durable bootstrap results.
use crate::{
    RunId,
    bootstrap_operation::{BootstrapContentRef, BootstrapInputError},
    budget::BudgetGuard,
    keys::NodeKey,
    run_state::ArtifactRef,
};
use serde::{Deserialize, Serialize};

/// Immutable allowlisted continuation committed before child admission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawContinuation")]
pub struct BootstrapContinuation {
    parent: RunId,
    graph: BootstrapContentRef,
    #[serde(with = "artifact_refs")]
    artifacts: Vec<ArtifactRef>,
    budget: BudgetGuard,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContinuation {
    parent: RunId,
    graph: BootstrapContentRef,
    #[serde(with = "artifact_refs")]
    artifacts: Vec<ArtifactRef>,
    budget: BudgetGuard,
}
impl TryFrom<RawContinuation> for BootstrapContinuation {
    type Error = BootstrapInputError;
    fn try_from(raw: RawContinuation) -> Result<Self, Self::Error> {
        Self::new(raw.parent, raw.graph, raw.artifacts, raw.budget)
    }
}
impl BootstrapContinuation {
    /// Reject incomplete references and zero-as-unlimited budget remainders.
    pub fn new(
        parent: RunId,
        graph: BootstrapContentRef,
        artifacts: Vec<ArtifactRef>,
        budget: BudgetGuard,
    ) -> Result<Self, BootstrapInputError> {
        if graph.name.trim().is_empty()
            || artifacts.len() != 3
            || ["description", "roadmap", "flow"].into_iter().any(|name| {
                artifacts
                    .iter()
                    .filter(|artifact| artifact.name == name)
                    .count()
                    != 1
            })
        {
            return Err(BootstrapInputError::InvalidReference);
        }
        if budget.limits.tokens == Some(0)
            || budget
                .limits
                .usd
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err(BootstrapInputError::InvalidBudget);
        }
        Ok(Self {
            parent,
            graph,
            artifacts,
            budget,
        })
    }
    /// Reserved parent identity.
    pub fn parent(&self) -> RunId {
        self.parent
    }
    /// Validated graph identity.
    pub fn graph(&self) -> &BootstrapContentRef {
        &self.graph
    }
    /// Exact approved materialization references.
    pub fn artifacts(&self) -> &[ArtifactRef] {
        &self.artifacts
    }
    /// Remaining allowance, reused unchanged after restart.
    pub fn budget(&self) -> BudgetGuard {
        self.budget
    }
}

/// Confirmed execution failure category, without parsing diagnostic strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapFailure {
    /// Engine persisted RunFailed.
    RunFailed,
    /// Engine persisted RunAborted without an operation cancellation request.
    RunAborted,
    /// Planning exhausted the shared allowance before child admission.
    BudgetExhausted,
}

/// Durable operation settlement. Planning success is never operation completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum BootstrapTerminal {
    /// Implementation durably completed.
    Completed { run_id: RunId, terminal: NodeKey },
    /// A reserved run failed or exhausted the operation allowance.
    Failed {
        run_id: RunId,
        reason: BootstrapFailure,
    },
    /// Requested cancellation settled, with no future launch eligible.
    Cancelled,
}

mod artifact_refs {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StoredArtifact {
        hash: crate::ContentHash,
        path: std::path::PathBuf,
        name: String,
        produced_by: NodeKey,
        produced_at_seq: u64,
    }
    pub fn serialize<S: serde::Serializer>(
        artifacts: &[ArtifactRef],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        artifacts
            .iter()
            .map(|artifact| StoredArtifact {
                hash: artifact.hash,
                path: artifact.path.clone(),
                name: artifact.name.clone(),
                produced_by: artifact.produced_by.clone(),
                produced_at_seq: artifact.produced_at_seq,
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<ArtifactRef>, D::Error> {
        Ok(Vec::<StoredArtifact>::deserialize(deserializer)?
            .into_iter()
            .map(|artifact| ArtifactRef {
                hash: artifact.hash,
                path: artifact.path,
                name: artifact.name,
                produced_by: artifact.produced_by,
                produced_at_seq: artifact.produced_at_seq,
            })
            .collect())
    }
}
