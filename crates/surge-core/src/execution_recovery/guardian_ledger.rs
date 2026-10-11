//! Pure guardian receipt data; neither decoding nor hashing grants native authority.
//! Record ordering, authentication and actual process observations belong to runtime owners.

use super::guardian::{GuardianAuthority, WindowsGuardianContainer};
use crate::{ContentHash, id::ExecutionWriterId};
use serde::{Deserialize, Serialize};

mod encoding;
mod payloads;
mod scalars;
mod wire;
pub use payloads::*;
pub use scalars::*;

/// Static validation categories; errors never echo receipt-controlled values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GuardianRecordError {
    /// Operation bytes or their wire spelling are invalid.
    #[error("invalid guardian operation")]
    InvalidOperation,
    /// A generation or lease is not positive.
    #[error("invalid guardian counter")]
    InvalidCounter,
    /// Journal sequence is outside the SQLite integer domain.
    #[error("invalid guardian journal sequence")]
    InvalidSequence,
    /// Identifier is nil or not canonically spelled.
    #[error("invalid guardian identifier")]
    InvalidIdentifier,
    /// Digest wire spelling is not canonical.
    #[error("invalid guardian digest")]
    InvalidHash,
    /// Embedded binding disagrees with its context or host.
    #[error("invalid guardian binding")]
    InvalidBinding,
    /// Nullable predecessor/lease fields disagree with the record kind.
    #[error("invalid guardian record phase")]
    InvalidPhase,
    /// Local takeover relationships are inconsistent.
    #[error("invalid guardian takeover")]
    InvalidTakeover,
    /// A resume observation has an impossible result value.
    #[error("invalid guardian resume outcome")]
    InvalidOutcome,
    /// Only inner format version one is supported.
    #[error("unsupported guardian record version")]
    UnsupportedVersion,
}

/// Immutable receipt shape. A valid value does not prove a recorded observation occurred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "wire::RawRecord")]
pub struct WindowsGuardianRecord {
    version: u32,
    context: GuardianRecordContext,
    operation: GuardianOperationId,
    predecessor: Option<ContentHash>,
    lease: Option<GuardianLease>,
    body: GuardianRecordBody,
}
impl WindowsGuardianRecord {
    /// Check local shape and exact embedded binding relationships, fixing version to one.
    pub fn new(
        context: GuardianRecordContext,
        operation: GuardianOperationId,
        predecessor: Option<ContentHash>,
        lease: Option<GuardianLease>,
        body: GuardianRecordBody,
    ) -> Result<Self, GuardianRecordError> {
        let valid_phase = match &body {
            GuardianRecordBody::LaunchIntent(_) => predecessor.is_none() && lease.is_none(),
            GuardianRecordBody::CancelledBeforeSpawn(_) => predecessor.is_some() && lease.is_none(),
            GuardianRecordBody::GuardianBound(_) => {
                predecessor.is_some() && lease.as_ref().is_some_and(|value| value.get() == 1)
            },
            _ => predecessor.is_some() && lease.is_some(),
        };
        if !valid_phase {
            return Err(GuardianRecordError::InvalidPhase);
        }
        validate_binding(&context, &body)?;
        if let GuardianRecordBody::LeaseTakenOver(takeover) = &body
            && lease.as_ref().map(|value| value.get())
                != takeover.previous_lease().get().checked_add(1)
        {
            return Err(GuardianRecordError::InvalidTakeover);
        }
        Ok(Self {
            version: 1,
            context,
            operation,
            predecessor,
            lease,
            body,
        })
    }
    /// Fixed inner receipt format version.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }
    /// Exact run, invocation, occurrence and authority context.
    #[must_use]
    pub fn context(&self) -> &GuardianRecordContext {
        &self.context
    }
    /// Declared operation identifier.
    #[must_use]
    pub fn operation(&self) -> &GuardianOperationId {
        &self.operation
    }
    /// Declared preceding record digest, without transcript validation.
    #[must_use]
    pub fn predecessor(&self) -> Option<&ContentHash> {
        self.predecessor.as_ref()
    }
    /// Declared guardian lease, without live authority.
    #[must_use]
    pub fn lease(&self) -> Option<&GuardianLease> {
        self.lease.as_ref()
    }
    /// Constructor-validated per-kind payload.
    #[must_use]
    pub fn body(&self) -> &GuardianRecordBody {
        &self.body
    }
    /// Exact version-one, domain-separated canonical preimage.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        encoding::encode(self)
    }
    /// SHA-256 of the complete canonical receipt, not an authenticated capability.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        ContentHash::compute(&self.canonical_bytes())
    }
}
fn validate_binding(
    context: &GuardianRecordContext,
    body: &GuardianRecordBody,
) -> Result<(), GuardianRecordError> {
    let binding = match body {
        GuardianRecordBody::GuardianBound(value) => Some(binding_parts(value.binding())),
        GuardianRecordBody::LeaseTakenOver(value) => Some(binding_parts(value.binding())),
        GuardianRecordBody::ChildEstablished(value) => Some(container_parts(value.container())),
        GuardianRecordBody::ResumeAuthorized(value) => Some(container_parts(value.container())),
        GuardianRecordBody::ResumeObserved(value) => Some(container_parts(value.container())),
        GuardianRecordBody::SettlementRequested(value) => Some(binding_parts(value.binding())),
        GuardianRecordBody::SettlementObserved(value) => match value.proof() {
            GuardianSettlementProof::BoundJobEmpty(proof) => Some(binding_parts(proof.binding())),
            GuardianSettlementProof::ChildJobEmpty(proof) => {
                Some(container_parts(proof.container()))
            },
        },
        GuardianRecordBody::LaunchIntent(_)
        | GuardianRecordBody::CancelledBeforeSpawn(_)
        | GuardianRecordBody::SettlementConsumed(_) => None,
    };
    if binding.is_some_and(|(authority, occurrence)| {
        authority != context.authority() || occurrence != context.occurrence()
    }) {
        return Err(GuardianRecordError::InvalidBinding);
    }
    Ok(())
}
fn binding_parts(binding: &WindowsGuardianBinding) -> (&GuardianAuthority, ExecutionWriterId) {
    (binding.authority(), binding.occurrence())
}
fn container_parts(
    container: &WindowsGuardianContainer,
) -> (&GuardianAuthority, ExecutionWriterId) {
    (
        container.identity().authority(),
        container.identity().occurrence(),
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod constructor_tests;
