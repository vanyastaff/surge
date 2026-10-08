//! Durable provider-session identity and recoverable execution boundaries.

pub mod commit;
pub mod gate_commit;
pub mod guardian;
pub mod guardian_ledger;
pub mod process;
use crate::{ContentHash, SessionId, id::StageInvocationId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A provider's opaque ACP identifier, distinct from Surge's internal session ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderSessionId(String);
impl ProviderSessionId {
    /// Validate a bounded opaque ID without interpreting its provider-specific format.
    pub fn new(value: String) -> Result<Self, RecoveryIdentityError> {
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(RecoveryIdentityError::InvalidProviderId);
        }
        Ok(Self(value))
    }
    /// The exact identifier returned by the provider.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ProviderSessionId {
    type Error = RecoveryIdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<ProviderSessionId> for String {
    fn from(value: ProviderSessionId) -> Self {
        value.0
    }
}

/// Capabilities actually observed on an ACP initialize response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionRestoreCapabilities {
    /// The peer supports session/resume without history replay.
    pub resume: bool,
    /// The peer supports session/load with history replay.
    pub load: bool,
}
/// The session operation that actually established this bridge session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOpenMode {
    /// A new session, including an explicitly authorized replacement.
    New,
    /// Restored without history replay.
    Resume,
    /// Restored with historical notification replay.
    Load,
}

/// Immutable continuation identity. Host I/O validates canonical cwd and runtime ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawDescriptor")]
pub struct ProviderSessionDescriptor {
    provider_session_id: ProviderSessionId,
    invocation: StageInvocationId,
    runtime: String,
    launch_hash: ContentHash,
    cwd: PathBuf,
    capabilities: SessionRestoreCapabilities,
}
#[derive(Deserialize)]
struct RawDescriptor {
    provider_session_id: ProviderSessionId,
    invocation: StageInvocationId,
    runtime: String,
    launch_hash: ContentHash,
    cwd: PathBuf,
    capabilities: SessionRestoreCapabilities,
}
impl TryFrom<RawDescriptor> for ProviderSessionDescriptor {
    type Error = RecoveryIdentityError;
    fn try_from(raw: RawDescriptor) -> Result<Self, Self::Error> {
        Self::new(
            raw.provider_session_id,
            raw.invocation,
            raw.runtime,
            raw.launch_hash,
            raw.cwd,
            raw.capabilities,
        )
    }
}
impl ProviderSessionDescriptor {
    /// Construct validated host metadata; relative directories and empty runtimes are rejected.
    pub fn new(
        provider_session_id: ProviderSessionId,
        invocation: StageInvocationId,
        runtime: String,
        launch_hash: ContentHash,
        cwd: PathBuf,
        capabilities: SessionRestoreCapabilities,
    ) -> Result<Self, RecoveryIdentityError> {
        if runtime.trim().is_empty() || runtime.len() > 512 || runtime.chars().any(char::is_control)
        {
            return Err(RecoveryIdentityError::InvalidRuntime);
        }
        if !cwd.is_absolute() {
            return Err(RecoveryIdentityError::RelativeDirectory);
        }
        if invocation.as_ulid() == ulid::Ulid::nil() {
            return Err(RecoveryIdentityError::InvalidInvocation);
        }
        Ok(Self {
            provider_session_id,
            invocation,
            runtime,
            launch_hash,
            cwd,
            capabilities,
        })
    }
    /// Provider identity, never the internal Surge ID.
    pub fn provider_session_id(&self) -> &ProviderSessionId {
        &self.provider_session_id
    }
    /// Authenticated invocation owning this provider session.
    pub fn invocation(&self) -> StageInvocationId {
        self.invocation
    }
    /// Canonical registered runtime identity.
    pub fn runtime(&self) -> &str {
        &self.runtime
    }
    /// Fingerprint of the provider launch contract.
    pub fn launch_hash(&self) -> &ContentHash {
        &self.launch_hash
    }
    /// Canonical directory used when establishing the session.
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }
    /// Observed capabilities, rechecked on every restore handshake.
    pub fn capabilities(&self) -> SessionRestoreCapabilities {
        self.capabilities
    }
}
/// Atomic opening result; metadata travels with the internal handle rather than a mutable slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawOpenedSession")]
pub struct OpenedSession {
    /// Actual local writer identity; absent legacy metadata cannot prove cold cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_writer: Option<process::ExecutionWriterObservation>,
    /// New internal handle for this bridge connection.
    pub session: SessionId,
    /// Exact durable provider identity for continuation.
    pub descriptor: ProviderSessionDescriptor,
    /// Operation that actually succeeded.
    pub mode: SessionOpenMode,
}
#[derive(Deserialize)]
struct RawOpenedSession {
    #[serde(default)]
    execution_writer: Option<process::ExecutionWriterObservation>,
    session: SessionId,
    descriptor: ProviderSessionDescriptor,
    mode: SessionOpenMode,
}
impl TryFrom<RawOpenedSession> for OpenedSession {
    type Error = RecoveryIdentityError;
    fn try_from(raw: RawOpenedSession) -> Result<Self, Self::Error> {
        Self::new(raw.session, raw.descriptor, raw.mode).map(|mut opened| {
            opened.execution_writer = raw.execution_writer;
            opened
        })
    }
}
impl OpenedSession {
    /// Associate a nonempty internal handle with immutable provider metadata.
    pub fn new(
        session: SessionId,
        descriptor: ProviderSessionDescriptor,
        mode: SessionOpenMode,
    ) -> Result<Self, RecoveryIdentityError> {
        if session.as_ulid() == ulid::Ulid::nil() {
            return Err(RecoveryIdentityError::InvalidInvocation);
        }
        Ok(Self {
            execution_writer: None,
            session,
            descriptor,
            mode,
        })
    }
}
/// Opening intent. Continue cannot silently fall back to creating a new provider session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionOpening {
    /// Start a first session or an explicitly recorded replacement.
    #[default]
    New,
    /// Restore the saved identity; prefer resume, then supported load.
    Continue(ProviderSessionDescriptor),
}
/// Invalid durable continuation metadata.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecoveryIdentityError {
    /// Empty, oversized or control-containing provider ID.
    #[error("invalid provider session identifier")]
    InvalidProviderId,
    /// Runtime identity is not bounded and nonempty.
    #[error("invalid provider runtime identity")]
    InvalidRuntime,
    /// Relative directories cannot establish durable cwd identity.
    #[error("provider session directory must be absolute")]
    RelativeDirectory,
    /// An unassigned invocation cannot authenticate restored execution.
    #[error("invalid provider session invocation")]
    InvalidInvocation,
    /// A process/group identity is absent, zero or malformed.
    #[error("invalid execution writer process identity")]
    InvalidProcess,
    /// A stage acceptance marker lacks its authenticated identity or bounded effects batch.
    #[error("invalid stage outcome commit identity")]
    InvalidCommit,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_id_is_opaque_but_bounded_on_constructor_and_json() {
        assert!(ProviderSessionId::new("provider:session/opaque".into()).is_ok());
        for value in [String::new(), "bad\nidentity".into(), "x".repeat(4097)] {
            assert!(ProviderSessionId::new(value.clone()).is_err());
            assert!(serde_json::from_value::<ProviderSessionId>(serde_json::json!(value)).is_err());
        }
    }
    #[test]
    fn descriptor_decode_cannot_bypass_absolute_cwd() {
        let descriptor = ProviderSessionDescriptor::new(
            ProviderSessionId::new("provider-1".into()).unwrap(),
            StageInvocationId::new(),
            "registered-runtime".into(),
            ContentHash::compute(b"launch"),
            std::env::temp_dir(),
            SessionRestoreCapabilities {
                resume: true,
                load: false,
            },
        )
        .unwrap();
        let mut encoded = serde_json::to_value(&descriptor).unwrap();
        encoded["cwd"] = "relative".into();
        assert!(serde_json::from_value::<ProviderSessionDescriptor>(encoded).is_err());
        assert_eq!(
            serde_json::from_value::<ProviderSessionDescriptor>(
                serde_json::to_value(&descriptor).unwrap()
            )
            .unwrap(),
            descriptor
        );
    }
}

/// Why execution stopped without a definitive terminal outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SuspensionReason {
    /// Explicit operator pause; no automatic wake is allowed.
    Manual,
    /// All eligible candidates are unavailable; observations stay authoritative.
    Capacity {
        /// Next bounded probe/attempt time, never a claim that quota recovered.
        wake_at_ms: Option<i64>,
    },
    /// Recovery requires an explicit decision rather than another agent dispatch.
    RecoveryRequired {
        /// Actionable diagnostic.
        diagnostic: String,
    },
}
/// Stage state retained at a suspension boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PendingStagePhase {
    /// No accepted outcome/effects; restore the saved provider invocation.
    Interrupted {
        /// Node retained in the cursor.
        node: crate::NodeKey,
        /// Stable invocation, distinct from the new MCP generation.
        invocation: crate::id::StageInvocationId,
    },
    /// An exact host plan parked before any provider was opened.
    PlannedCapacity {
        /// Node belonging to the actual anchored stage occurrence.
        node: crate::NodeKey,
        /// Stable logical stage, separate from later provider invocations.
        logical_invocation: crate::id::StageInvocationId,
        /// Actual StageEntered event sequence.
        stage_entry_seq: u64,
        /// Durable QuotaStagePlanned occurrence.
        plan_seq: u64,
    },
    /// An unanswered host decision retained without a provider invocation.
    WaitingHumanGate {
        /// Exact gate node.
        node: crate::NodeKey,
        /// Original actionable request identity.
        request: crate::id::GateRequestId,
        /// Original stage occurrence.
        stage_entry_seq: u64,
        /// Original question event.
        requested_seq: u64,
    },
    /// Outcome/effects are committed, but the edge has not yet been applied.
    CommittedOutcomeAwaitingRoute {
        /// Exact accepted provider invocation; absent for non-provider stages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<crate::id::StageInvocationId>,
        /// Stage owning the committed result.
        node: crate::NodeKey,
        /// Accepted outcome to route exactly once.
        outcome: crate::OutcomeKey,
        /// Prefix containing the accepted effects.
        committed_seq: u64,
    },
    /// The cursor is between stages; no prior provider must be restored.
    BetweenStages,
}
/// Durable evidence that dispatch stopped and cleanup completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspensionFence {
    /// Exact request generation being acknowledged.
    pub control_generation: u64,
    /// Snapshot must correspond to this trusted journal prefix.
    pub snapshot_seq: u64,
    /// Recovery phase, not an inference from the last routed node.
    pub pending_stage: PendingStagePhase,
    /// Confirmed quiescence is required before a registry suspension can acknowledge.
    pub cleanup_confirmed: bool,
    /// Manual, quota or explicit recovery decision.
    pub reason: SuspensionReason,
}
/// Durable command state, separate from a task's accepted revision and attempt generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionControlState {
    /// Intent persisted, owner not yet quiesced.
    SuspendRequested,
    /// Journal fence and cleanup have been confirmed.
    Suspended,
    /// Continue reserved before restore/dispatch.
    ContinueReserved,
    /// Continued owner is executing.
    Executing,
    /// Cleanup/restoration cannot be confirmed.
    Attention,
    /// A genuine definitive result won the control race.
    Terminal,
}
/// Historical control result with exact operation/run/attempt association.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemExecutionControl {
    /// Owning task.
    pub item: crate::id::WorkItemId,
    /// Same run; Continue does not allocate an attempt.
    pub run: crate::RunId,
    /// Original attempt owner generation.
    pub attempt_generation: u64,
    /// Monotonic control generation fencing old requests and wakes.
    pub generation: u64,
    /// Stable operation identity.
    pub operation: crate::id::WorkItemOperationId,
    /// Requested control progress.
    pub state: ExecutionControlState,
    /// Present only after confirmed durable quiescence.
    pub fence: Option<SuspensionFence>,
    /// Explicit replacement choice; ordinary Continue never starts a new session.
    pub allow_new_session: bool,
    /// Failure after durable admission; it does not revoke the accepted operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}
