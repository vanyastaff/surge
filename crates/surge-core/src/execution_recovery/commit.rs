//! Host acceptance identity for an atomic stage outcome and its required effects.
use super::RecoveryIdentityError;
use crate::{
    ContentHash, OutcomeKey, SessionId, id::StageInvocationId, stage_tool::StageToolContext,
};
use serde::{Deserialize, Serialize};

/// Immutable acceptance marker. Its event envelope supplies the actual durable commit sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCommit")]
pub struct StageOutcomeCommit {
    context: StageToolContext,
    provider_connection: SessionId,
    invocation: StageInvocationId,
    outcome: OutcomeKey,
    effects_count: u32,
    effects_hash: ContentHash,
}

#[derive(Deserialize)]
struct RawCommit {
    context: StageToolContext,
    provider_connection: SessionId,
    invocation: StageInvocationId,
    outcome: OutcomeKey,
    effects_count: u32,
    effects_hash: ContentHash,
}
impl TryFrom<RawCommit> for StageOutcomeCommit {
    type Error = RecoveryIdentityError;
    fn try_from(raw: RawCommit) -> Result<Self, Self::Error> {
        Self::new(
            raw.context,
            raw.provider_connection,
            raw.invocation,
            raw.outcome,
            raw.effects_count,
            raw.effects_hash,
        )
    }
}
impl StageOutcomeCommit {
    /// Bind a bounded nonempty ordered effects batch to its host-authenticated invocation.
    pub fn new(
        context: StageToolContext,
        provider_connection: SessionId,
        invocation: StageInvocationId,
        outcome: OutcomeKey,
        effects_count: u32,
        effects_hash: ContentHash,
    ) -> Result<Self, RecoveryIdentityError> {
        if [
            context.run.as_ulid(),
            context.session.as_ulid(),
            context.generation.as_ulid(),
            provider_connection.as_ulid(),
            invocation.as_ulid(),
        ]
        .contains(&ulid::Ulid::nil())
            || !(1..=4096).contains(&effects_count)
        {
            return Err(RecoveryIdentityError::InvalidCommit);
        }
        Ok(Self {
            context,
            provider_connection,
            invocation,
            outcome,
            effects_count,
            effects_hash,
        })
    }
    /// Authenticated current connection identity, distinct from the stable invocation.
    pub fn context(&self) -> &StageToolContext {
        &self.context
    }
    /// Surge's provider connection handle; the MCP authority session is a separate identity.
    pub fn provider_connection(&self) -> SessionId {
        self.provider_connection
    }
    /// Logical invocation retained across restoration and provider rotation.
    pub fn invocation(&self) -> StageInvocationId {
        self.invocation
    }
    /// Accepted declared outcome.
    pub fn outcome(&self) -> &OutcomeKey {
        &self.outcome
    }
    /// Number of immediately preceding events committed with this marker.
    pub fn effects_count(&self) -> u32 {
        self.effects_count
    }
    /// Hash of the ordered serialized `VersionedEventPayload` batch.
    pub fn effects_hash(&self) -> &ContentHash {
        &self.effects_hash
    }
}

/// Folded commit consumption, derived from the journal rather than a provider response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedStageOutcome {
    /// Immutable host acceptance identity.
    pub commit: StageOutcomeCommit,
    /// Actual acceptance marker event sequence.
    pub committed_seq: u64,
    /// Actual route-consumption event sequence, when its snapshot transaction committed.
    pub routed_seq: Option<u64>,
    /// Conflicting commit or route evidence must never authorize continuation.
    pub conflicting: bool,
}
