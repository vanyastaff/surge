//! Immutable per-kind guardian receipt payloads. No claimed observation is authenticated here.
use super::super::guardian::{WindowsGuardianContainer, WindowsGuardianProcessIdentity};
use super::{
    GuardianBarrierGeneration, GuardianHostObservationGeneration, GuardianJournalSequence,
    GuardianLease, GuardianOperationId, GuardianQueryGeneration, GuardianRecordError,
    GuardianRegistryGeneration, GuardianRevocationGeneration, WindowsGuardianBinding,
};
use crate::ContentHash;
use serde::{Deserialize, Serialize};

/// Validated typed fields for GuardianLaunchIntent; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianLaunchIntent")]
pub struct GuardianLaunchIntent {
    host: WindowsGuardianProcessIdentity,
    registry_generation: GuardianRegistryGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianLaunchIntent {
    host: WindowsGuardianProcessIdentity,
    registry_generation: GuardianRegistryGeneration,
}
impl GuardianLaunchIntent {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        host: WindowsGuardianProcessIdentity,
        registry_generation: GuardianRegistryGeneration,
    ) -> Self {
        Self {
            host,
            registry_generation,
        }
    }
    /// Recorded host.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        &self.host
    }
    /// Recorded registry generation.
    #[must_use]
    pub fn registry_generation(&self) -> &GuardianRegistryGeneration {
        &self.registry_generation
    }
}
impl From<RawGuardianLaunchIntent> for GuardianLaunchIntent {
    fn from(raw: RawGuardianLaunchIntent) -> Self {
        Self::new(raw.host, raw.registry_generation)
    }
}

/// Validated typed fields for GuardianCancelledBeforeSpawn; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianCancelledBeforeSpawn")]
pub struct GuardianCancelledBeforeSpawn {
    launch_operation: GuardianOperationId,
    host: WindowsGuardianProcessIdentity,
    registry_generation: GuardianRegistryGeneration,
    revocation_generation: GuardianRevocationGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianCancelledBeforeSpawn {
    launch_operation: GuardianOperationId,
    host: WindowsGuardianProcessIdentity,
    registry_generation: GuardianRegistryGeneration,
    revocation_generation: GuardianRevocationGeneration,
}
impl GuardianCancelledBeforeSpawn {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        launch_operation: GuardianOperationId,
        host: WindowsGuardianProcessIdentity,
        registry_generation: GuardianRegistryGeneration,
        revocation_generation: GuardianRevocationGeneration,
    ) -> Self {
        Self {
            launch_operation,
            host,
            registry_generation,
            revocation_generation,
        }
    }
    /// Recorded launch operation.
    #[must_use]
    pub fn launch_operation(&self) -> &GuardianOperationId {
        &self.launch_operation
    }
    /// Recorded host.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        &self.host
    }
    /// Recorded registry generation.
    #[must_use]
    pub fn registry_generation(&self) -> &GuardianRegistryGeneration {
        &self.registry_generation
    }
    /// Recorded revocation generation.
    #[must_use]
    pub fn revocation_generation(&self) -> &GuardianRevocationGeneration {
        &self.revocation_generation
    }
}
impl From<RawGuardianCancelledBeforeSpawn> for GuardianCancelledBeforeSpawn {
    fn from(raw: RawGuardianCancelledBeforeSpawn) -> Self {
        Self::new(
            raw.launch_operation,
            raw.host,
            raw.registry_generation,
            raw.revocation_generation,
        )
    }
}

/// Validated typed fields for GuardianChildEstablished; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianChildEstablished")]
pub struct GuardianChildEstablished {
    container: WindowsGuardianContainer,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianChildEstablished {
    container: WindowsGuardianContainer,
}
impl GuardianChildEstablished {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(container: WindowsGuardianContainer) -> Self {
        Self { container }
    }
    /// Recorded container.
    #[must_use]
    pub fn container(&self) -> &WindowsGuardianContainer {
        &self.container
    }
}
impl From<RawGuardianChildEstablished> for GuardianChildEstablished {
    fn from(raw: RawGuardianChildEstablished) -> Self {
        Self::new(raw.container)
    }
}

/// Validated typed fields for GuardianResumeAuthorized; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianResumeAuthorized")]
pub struct GuardianResumeAuthorized {
    container: WindowsGuardianContainer,
    barrier_generation: GuardianBarrierGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianResumeAuthorized {
    container: WindowsGuardianContainer,
    barrier_generation: GuardianBarrierGeneration,
}
impl GuardianResumeAuthorized {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        container: WindowsGuardianContainer,
        barrier_generation: GuardianBarrierGeneration,
    ) -> Self {
        Self {
            container,
            barrier_generation,
        }
    }
    /// Recorded container.
    #[must_use]
    pub fn container(&self) -> &WindowsGuardianContainer {
        &self.container
    }
    /// Recorded barrier generation.
    #[must_use]
    pub fn barrier_generation(&self) -> &GuardianBarrierGeneration {
        &self.barrier_generation
    }
}
impl From<RawGuardianResumeAuthorized> for GuardianResumeAuthorized {
    fn from(raw: RawGuardianResumeAuthorized) -> Self {
        Self::new(raw.container, raw.barrier_generation)
    }
}

/// Validated typed fields for GuardianResumeObserved; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianResumeObserved")]
pub struct GuardianResumeObserved {
    container: WindowsGuardianContainer,
    resume_operation: GuardianOperationId,
    barrier_generation: GuardianBarrierGeneration,
    outcome: ResumeObservation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianResumeObserved {
    container: WindowsGuardianContainer,
    resume_operation: GuardianOperationId,
    barrier_generation: GuardianBarrierGeneration,
    outcome: ResumeObservation,
}
impl GuardianResumeObserved {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        container: WindowsGuardianContainer,
        resume_operation: GuardianOperationId,
        barrier_generation: GuardianBarrierGeneration,
        outcome: ResumeObservation,
    ) -> Self {
        Self {
            container,
            resume_operation,
            barrier_generation,
            outcome,
        }
    }
    /// Recorded container.
    #[must_use]
    pub fn container(&self) -> &WindowsGuardianContainer {
        &self.container
    }
    /// Recorded resume operation.
    #[must_use]
    pub fn resume_operation(&self) -> &GuardianOperationId {
        &self.resume_operation
    }
    /// Recorded barrier generation.
    #[must_use]
    pub fn barrier_generation(&self) -> &GuardianBarrierGeneration {
        &self.barrier_generation
    }
    /// Recorded outcome.
    #[must_use]
    pub fn outcome(&self) -> &ResumeObservation {
        &self.outcome
    }
}
impl From<RawGuardianResumeObserved> for GuardianResumeObserved {
    fn from(raw: RawGuardianResumeObserved) -> Self {
        Self::new(
            raw.container,
            raw.resume_operation,
            raw.barrier_generation,
            raw.outcome,
        )
    }
}

/// Validated typed fields for GuardianSettlementRequested; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianSettlementRequested")]
pub struct GuardianSettlementRequested {
    binding: WindowsGuardianBinding,
    barrier_generation: GuardianBarrierGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianSettlementRequested {
    binding: WindowsGuardianBinding,
    barrier_generation: GuardianBarrierGeneration,
}
impl GuardianSettlementRequested {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        binding: WindowsGuardianBinding,
        barrier_generation: GuardianBarrierGeneration,
    ) -> Self {
        Self {
            binding,
            barrier_generation,
        }
    }
    /// Recorded binding.
    #[must_use]
    pub fn binding(&self) -> &WindowsGuardianBinding {
        &self.binding
    }
    /// Recorded barrier generation.
    #[must_use]
    pub fn barrier_generation(&self) -> &GuardianBarrierGeneration {
        &self.barrier_generation
    }
}
impl From<RawGuardianSettlementRequested> for GuardianSettlementRequested {
    fn from(raw: RawGuardianSettlementRequested) -> Self {
        Self::new(raw.binding, raw.barrier_generation)
    }
}

/// Validated typed fields for GuardianSettlementObserved; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianSettlementObserved")]
pub struct GuardianSettlementObserved {
    settlement_operation: GuardianOperationId,
    proof: GuardianSettlementProof,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianSettlementObserved {
    settlement_operation: GuardianOperationId,
    proof: GuardianSettlementProof,
}
impl GuardianSettlementObserved {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(settlement_operation: GuardianOperationId, proof: GuardianSettlementProof) -> Self {
        Self {
            settlement_operation,
            proof,
        }
    }
    /// Recorded settlement operation.
    #[must_use]
    pub fn settlement_operation(&self) -> &GuardianOperationId {
        &self.settlement_operation
    }
    /// Recorded proof.
    #[must_use]
    pub fn proof(&self) -> &GuardianSettlementProof {
        &self.proof
    }
}
impl From<RawGuardianSettlementObserved> for GuardianSettlementObserved {
    fn from(raw: RawGuardianSettlementObserved) -> Self {
        Self::new(raw.settlement_operation, raw.proof)
    }
}

/// Validated typed fields for GuardianSettlementConsumed; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianSettlementConsumed")]
pub struct GuardianSettlementConsumed {
    settlement_operation: GuardianOperationId,
    settlement_hash: ContentHash,
    settlement_sequence: GuardianJournalSequence,
    barrier_generation: GuardianBarrierGeneration,
    query_generation: GuardianQueryGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianSettlementConsumed {
    settlement_operation: GuardianOperationId,
    #[serde(deserialize_with = "super::wire::canonical_hash")]
    settlement_hash: ContentHash,
    settlement_sequence: GuardianJournalSequence,
    barrier_generation: GuardianBarrierGeneration,
    query_generation: GuardianQueryGeneration,
}
impl GuardianSettlementConsumed {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        settlement_operation: GuardianOperationId,
        settlement_hash: ContentHash,
        settlement_sequence: GuardianJournalSequence,
        barrier_generation: GuardianBarrierGeneration,
        query_generation: GuardianQueryGeneration,
    ) -> Self {
        Self {
            settlement_operation,
            settlement_hash,
            settlement_sequence,
            barrier_generation,
            query_generation,
        }
    }
    /// Recorded settlement operation.
    #[must_use]
    pub fn settlement_operation(&self) -> &GuardianOperationId {
        &self.settlement_operation
    }
    /// Recorded settlement hash.
    #[must_use]
    pub fn settlement_hash(&self) -> &ContentHash {
        &self.settlement_hash
    }
    /// Recorded settlement sequence.
    #[must_use]
    pub fn settlement_sequence(&self) -> &GuardianJournalSequence {
        &self.settlement_sequence
    }
    /// Recorded barrier generation.
    #[must_use]
    pub fn barrier_generation(&self) -> &GuardianBarrierGeneration {
        &self.barrier_generation
    }
    /// Recorded query generation.
    #[must_use]
    pub fn query_generation(&self) -> &GuardianQueryGeneration {
        &self.query_generation
    }
}
impl From<RawGuardianSettlementConsumed> for GuardianSettlementConsumed {
    fn from(raw: RawGuardianSettlementConsumed) -> Self {
        Self::new(
            raw.settlement_operation,
            raw.settlement_hash,
            raw.settlement_sequence,
            raw.barrier_generation,
            raw.query_generation,
        )
    }
}

/// Validated typed fields for GuardianPreviousHostTerminated; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianPreviousHostTerminated")]
pub struct GuardianPreviousHostTerminated {
    host: WindowsGuardianProcessIdentity,
    lease: GuardianLease,
    observation_generation: GuardianHostObservationGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianPreviousHostTerminated {
    host: WindowsGuardianProcessIdentity,
    lease: GuardianLease,
    observation_generation: GuardianHostObservationGeneration,
}
impl GuardianPreviousHostTerminated {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        host: WindowsGuardianProcessIdentity,
        lease: GuardianLease,
        observation_generation: GuardianHostObservationGeneration,
    ) -> Self {
        Self {
            host,
            lease,
            observation_generation,
        }
    }
    /// Recorded host.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        &self.host
    }
    /// Recorded lease.
    #[must_use]
    pub fn lease(&self) -> &GuardianLease {
        &self.lease
    }
    /// Recorded observation generation.
    #[must_use]
    pub fn observation_generation(&self) -> &GuardianHostObservationGeneration {
        &self.observation_generation
    }
}
impl From<RawGuardianPreviousHostTerminated> for GuardianPreviousHostTerminated {
    fn from(raw: RawGuardianPreviousHostTerminated) -> Self {
        Self::new(raw.host, raw.lease, raw.observation_generation)
    }
}

/// Validated typed fields for GuardianPreviousHostRelinquished; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianPreviousHostRelinquished")]
pub struct GuardianPreviousHostRelinquished {
    host: WindowsGuardianProcessIdentity,
    lease: GuardianLease,
    operation: GuardianOperationId,
    hash: ContentHash,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianPreviousHostRelinquished {
    host: WindowsGuardianProcessIdentity,
    lease: GuardianLease,
    operation: GuardianOperationId,
    #[serde(deserialize_with = "super::wire::canonical_hash")]
    hash: ContentHash,
}
impl GuardianPreviousHostRelinquished {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        host: WindowsGuardianProcessIdentity,
        lease: GuardianLease,
        operation: GuardianOperationId,
        hash: ContentHash,
    ) -> Self {
        Self {
            host,
            lease,
            operation,
            hash,
        }
    }
    /// Recorded host.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        &self.host
    }
    /// Recorded lease.
    #[must_use]
    pub fn lease(&self) -> &GuardianLease {
        &self.lease
    }
    /// Recorded operation.
    #[must_use]
    pub fn operation(&self) -> &GuardianOperationId {
        &self.operation
    }
    /// Recorded hash.
    #[must_use]
    pub fn hash(&self) -> &ContentHash {
        &self.hash
    }
}
impl From<RawGuardianPreviousHostRelinquished> for GuardianPreviousHostRelinquished {
    fn from(raw: RawGuardianPreviousHostRelinquished) -> Self {
        Self::new(raw.host, raw.lease, raw.operation, raw.hash)
    }
}

/// Validated typed fields for GuardianBoundJobEmpty; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianBoundJobEmpty")]
pub struct GuardianBoundJobEmpty {
    binding: WindowsGuardianBinding,
    barrier_generation: GuardianBarrierGeneration,
    query_generation: GuardianQueryGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianBoundJobEmpty {
    binding: WindowsGuardianBinding,
    barrier_generation: GuardianBarrierGeneration,
    query_generation: GuardianQueryGeneration,
}
impl GuardianBoundJobEmpty {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        binding: WindowsGuardianBinding,
        barrier_generation: GuardianBarrierGeneration,
        query_generation: GuardianQueryGeneration,
    ) -> Self {
        Self {
            binding,
            barrier_generation,
            query_generation,
        }
    }
    /// Recorded binding.
    #[must_use]
    pub fn binding(&self) -> &WindowsGuardianBinding {
        &self.binding
    }
    /// Recorded barrier generation.
    #[must_use]
    pub fn barrier_generation(&self) -> &GuardianBarrierGeneration {
        &self.barrier_generation
    }
    /// Recorded query generation.
    #[must_use]
    pub fn query_generation(&self) -> &GuardianQueryGeneration {
        &self.query_generation
    }
}
impl From<RawGuardianBoundJobEmpty> for GuardianBoundJobEmpty {
    fn from(raw: RawGuardianBoundJobEmpty) -> Self {
        Self::new(raw.binding, raw.barrier_generation, raw.query_generation)
    }
}

/// Validated typed fields for GuardianChildJobEmpty; these fields convey no runtime authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawGuardianChildJobEmpty")]
pub struct GuardianChildJobEmpty {
    container: WindowsGuardianContainer,
    barrier_generation: GuardianBarrierGeneration,
    query_generation: GuardianQueryGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuardianChildJobEmpty {
    container: WindowsGuardianContainer,
    barrier_generation: GuardianBarrierGeneration,
    query_generation: GuardianQueryGeneration,
}
impl GuardianChildJobEmpty {
    /// Assemble already validated typed fields; sequencing is checked by the journal owner.
    #[must_use]
    pub fn new(
        container: WindowsGuardianContainer,
        barrier_generation: GuardianBarrierGeneration,
        query_generation: GuardianQueryGeneration,
    ) -> Self {
        Self {
            container,
            barrier_generation,
            query_generation,
        }
    }
    /// Recorded container.
    #[must_use]
    pub fn container(&self) -> &WindowsGuardianContainer {
        &self.container
    }
    /// Recorded barrier generation.
    #[must_use]
    pub fn barrier_generation(&self) -> &GuardianBarrierGeneration {
        &self.barrier_generation
    }
    /// Recorded query generation.
    #[must_use]
    pub fn query_generation(&self) -> &GuardianQueryGeneration {
        &self.query_generation
    }
}
impl From<RawGuardianChildJobEmpty> for GuardianChildJobEmpty {
    fn from(raw: RawGuardianChildJobEmpty) -> Self {
        Self::new(raw.container, raw.barrier_generation, raw.query_generation)
    }
}

/// Childless binding to the initial host and its registry generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawBound")]
pub struct GuardianBound {
    binding: WindowsGuardianBinding,
    host: WindowsGuardianProcessIdentity,
    registry_generation: GuardianRegistryGeneration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBound {
    binding: WindowsGuardianBinding,
    host: WindowsGuardianProcessIdentity,
    registry_generation: GuardianRegistryGeneration,
}
impl GuardianBound {
    /// Reject a host colliding with the retained guardian PID.
    pub fn new(
        binding: WindowsGuardianBinding,
        host: WindowsGuardianProcessIdentity,
        registry_generation: GuardianRegistryGeneration,
    ) -> Result<Self, GuardianRecordError> {
        if binding.guardian().pid() == host.pid() {
            return Err(GuardianRecordError::InvalidBinding);
        }
        Ok(Self {
            binding,
            host,
            registry_generation,
        })
    }
    /// Exact childless binding.
    #[must_use]
    pub fn binding(&self) -> &WindowsGuardianBinding {
        &self.binding
    }
    /// Declared initial host.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        &self.host
    }
    /// Declared registry generation.
    #[must_use]
    pub fn registry_generation(&self) -> &GuardianRegistryGeneration {
        &self.registry_generation
    }
}
impl TryFrom<RawBound> for GuardianBound {
    type Error = GuardianRecordError;
    fn try_from(raw: RawBound) -> Result<Self, Self::Error> {
        Self::new(raw.binding, raw.host, raw.registry_generation)
    }
}

/// Takeover data with strictly advancing registry generation and exact old-host evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawTakeover")]
pub struct GuardianLeaseTakenOver {
    binding: WindowsGuardianBinding,
    previous_lease: GuardianLease,
    previous_host: WindowsGuardianProcessIdentity,
    host: WindowsGuardianProcessIdentity,
    previous_registry_generation: GuardianRegistryGeneration,
    registry_generation: GuardianRegistryGeneration,
    authorization: GuardianTakeoverAuthorization,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTakeover {
    binding: WindowsGuardianBinding,
    previous_lease: GuardianLease,
    previous_host: WindowsGuardianProcessIdentity,
    host: WindowsGuardianProcessIdentity,
    previous_registry_generation: GuardianRegistryGeneration,
    registry_generation: GuardianRegistryGeneration,
    authorization: GuardianTakeoverAuthorization,
}
impl GuardianLeaseTakenOver {
    /// Validate local takeover relationships; live authorization is a separate operation.
    pub fn new(
        binding: WindowsGuardianBinding,
        previous_lease: GuardianLease,
        previous_host: WindowsGuardianProcessIdentity,
        host: WindowsGuardianProcessIdentity,
        previous_registry_generation: GuardianRegistryGeneration,
        registry_generation: GuardianRegistryGeneration,
        authorization: GuardianTakeoverAuthorization,
    ) -> Result<Self, GuardianRecordError> {
        if previous_lease.get().checked_add(1).is_none()
            || registry_generation.get() <= previous_registry_generation.get()
            || previous_host == host
            || previous_host.pid() == binding.guardian().pid()
            || host.pid() == binding.guardian().pid()
            || authorization.host() != &previous_host
            || authorization.lease() != &previous_lease
        {
            return Err(GuardianRecordError::InvalidTakeover);
        }
        Ok(Self {
            binding,
            previous_lease,
            previous_host,
            host,
            previous_registry_generation,
            registry_generation,
            authorization,
        })
    }
    /// Exact childless binding.
    #[must_use]
    pub fn binding(&self) -> &WindowsGuardianBinding {
        &self.binding
    }
    /// Previous guardian-allocated lease.
    #[must_use]
    pub fn previous_lease(&self) -> &GuardianLease {
        &self.previous_lease
    }
    /// Previous exact host identity.
    #[must_use]
    pub fn previous_host(&self) -> &WindowsGuardianProcessIdentity {
        &self.previous_host
    }
    /// New exact host identity.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        &self.host
    }
    /// Previous registry generation.
    #[must_use]
    pub fn previous_registry_generation(&self) -> &GuardianRegistryGeneration {
        &self.previous_registry_generation
    }
    /// New registry generation.
    #[must_use]
    pub fn registry_generation(&self) -> &GuardianRegistryGeneration {
        &self.registry_generation
    }
    /// Declared takeover evidence, not an authenticated capability.
    #[must_use]
    pub fn authorization(&self) -> &GuardianTakeoverAuthorization {
        &self.authorization
    }
}
impl TryFrom<RawTakeover> for GuardianLeaseTakenOver {
    type Error = GuardianRecordError;
    fn try_from(raw: RawTakeover) -> Result<Self, Self::Error> {
        Self::new(
            raw.binding,
            raw.previous_lease,
            raw.previous_host,
            raw.host,
            raw.previous_registry_generation,
            raw.registry_generation,
            raw.authorization,
        )
    }
}

/// A declared successful first resume; any other prior suspension count is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawStarted")]
pub struct GuardianResumeStarted {
    previous_suspend_count: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStarted {
    previous_suspend_count: u32,
}
impl GuardianResumeStarted {
    /// Accept only the exact first-resume count.
    pub fn new(previous_suspend_count: u32) -> Result<Self, GuardianRecordError> {
        if previous_suspend_count != 1 {
            return Err(GuardianRecordError::InvalidOutcome);
        }
        Ok(Self {
            previous_suspend_count,
        })
    }
    /// Declared prior suspension count, exactly one.
    #[must_use]
    pub fn previous_suspend_count(&self) -> u32 {
        self.previous_suspend_count
    }
}
impl TryFrom<RawStarted> for GuardianResumeStarted {
    type Error = GuardianRecordError;
    fn try_from(raw: RawStarted) -> Result<Self, Self::Error> {
        Self::new(raw.previous_suspend_count)
    }
}
/// A declared failed resume with a nonzero native error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawFailed")]
pub struct GuardianResumeFailed {
    win32_error: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFailed {
    win32_error: u32,
}
impl GuardianResumeFailed {
    /// Reject the native success code as failure evidence.
    pub fn new(win32_error: u32) -> Result<Self, GuardianRecordError> {
        if win32_error == 0 {
            return Err(GuardianRecordError::InvalidOutcome);
        }
        Ok(Self { win32_error })
    }
    /// Nonzero native error code.
    #[must_use]
    pub fn win32_error(&self) -> u32 {
        self.win32_error
    }
}
impl TryFrom<RawFailed> for GuardianResumeFailed {
    type Error = GuardianRecordError;
    fn try_from(raw: RawFailed) -> Result<Self, Self::Error> {
        Self::new(raw.win32_error)
    }
}
/// Explicitly empty uncertain observation. Unknown wire fields still refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawUncertain")]
pub struct GuardianResumeUncertain {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUncertain {}
impl GuardianResumeUncertain {
    /// Record the absence of a definitive observation, without supplying evidence.
    #[must_use]
    fn new() -> Self {
        Self {}
    }
}
impl From<RawUncertain> for GuardianResumeUncertain {
    fn from(_: RawUncertain) -> Self {
        Self::new()
    }
}
/// Shape of a declared resume observation, without any live authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResumeObservation {
    /// Exact successful first-resume result.
    Started(GuardianResumeStarted),
    /// Nonzero native failure result.
    Failed(GuardianResumeFailed),
    /// No definitive observation exists.
    Uncertain(GuardianResumeUncertain),
}
impl ResumeObservation {
    /// Explicitly record uncertainty; no default observation is inferred.
    #[must_use]
    pub fn uncertain() -> Self {
        Self::Uncertain(GuardianResumeUncertain::new())
    }
}
/// Declared previous-host evidence; authentication is a runtime responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuardianTakeoverAuthorization {
    /// Exact old host was declared terminated under a host observation generation.
    PreviousHostTerminated(GuardianPreviousHostTerminated),
    /// Exact old host declared an authenticated relinquishment operation.
    PreviousHostRelinquished(GuardianPreviousHostRelinquished),
}
impl GuardianTakeoverAuthorization {
    /// Exact old-host identity named by this evidence.
    #[must_use]
    pub fn host(&self) -> &WindowsGuardianProcessIdentity {
        match self {
            Self::PreviousHostTerminated(value) => value.host(),
            Self::PreviousHostRelinquished(value) => value.host(),
        }
    }
    /// Exact old lease named by this evidence.
    #[must_use]
    pub fn lease(&self) -> &GuardianLease {
        match self {
            Self::PreviousHostTerminated(value) => value.lease(),
            Self::PreviousHostRelinquished(value) => value.lease(),
        }
    }
}
/// Declared observation of an original retained Job; shape validation proves no observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuardianSettlementProof {
    /// Bound Job was declared empty without a required child identity.
    BoundJobEmpty(GuardianBoundJobEmpty),
    /// Bound Job was declared empty with the exact established child identity.
    ChildJobEmpty(GuardianChildJobEmpty),
}
/// Typed version-one record vocabulary; payload constructors close local invariants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuardianRecordBody {
    /// Intent before any helper spawn.
    LaunchIntent(GuardianLaunchIntent),
    /// Declared revocation before spawn.
    CancelledBeforeSpawn(GuardianCancelledBeforeSpawn),
    /// Childless guardian binding.
    GuardianBound(GuardianBound),
    /// New guardian lease replacing one precise previous lease.
    LeaseTakenOver(GuardianLeaseTakenOver),
    /// Recorded child within its original guardian container.
    ChildEstablished(GuardianChildEstablished),
    /// Declared fenced resume authorization.
    ResumeAuthorized(GuardianResumeAuthorized),
    /// Declared resume result.
    ResumeObserved(GuardianResumeObserved),
    /// Declared fenced settlement request.
    SettlementRequested(GuardianSettlementRequested),
    /// Declared settlement observation.
    SettlementObserved(GuardianSettlementObserved),
    /// Reference to a complete committed settlement-observation record.
    SettlementConsumed(GuardianSettlementConsumed),
}
