//! Host-owned acceptance of one durable human decision and its stage effects.
use serde::{Deserialize, Serialize};

use super::RecoveryIdentityError;
use crate::{ContentHash, NodeKey, OutcomeKey, VersionedEventPayload, id::GateRequestId};

/// Original decision occurrence; a node revisit owns a different request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRequest")]
pub struct GateCommitRequest {
    node: NodeKey,
    request: GateRequestId,
    stage_entry_seq: u64,
    requested_seq: u64,
}
#[derive(Deserialize)]
struct RawRequest {
    node: NodeKey,
    request: GateRequestId,
    stage_entry_seq: u64,
    requested_seq: u64,
}
impl TryFrom<RawRequest> for GateCommitRequest {
    type Error = RecoveryIdentityError;
    fn try_from(raw: RawRequest) -> Result<Self, Self::Error> {
        Self::new(
            raw.node,
            raw.request,
            raw.stage_entry_seq,
            raw.requested_seq,
        )
    }
}
impl GateCommitRequest {
    /// Bind a nonnil request to strictly ordered durable stage/request events.
    pub fn new(
        node: NodeKey,
        request: GateRequestId,
        stage_entry_seq: u64,
        requested_seq: u64,
    ) -> Result<Self, RecoveryIdentityError> {
        if request.as_ulid() == ulid::Ulid::nil()
            || stage_entry_seq == 0
            || requested_seq <= stage_entry_seq
        {
            return Err(RecoveryIdentityError::InvalidCommit);
        }
        Ok(Self {
            node,
            request,
            stage_entry_seq,
            requested_seq,
        })
    }
    /// Owning node from the original accepted graph.
    pub fn node(&self) -> &NodeKey {
        &self.node
    }
    /// Exact request identity, never a provider/session identity.
    pub fn request(&self) -> GateRequestId {
        self.request
    }
    /// Original stage occurrence envelope sequence.
    pub fn stage_entry_seq(&self) -> u64 {
        self.stage_entry_seq
    }
    /// Original request envelope sequence.
    pub fn requested_seq(&self) -> u64 {
        self.requested_seq
    }
}

/// The durable human answer accepted before any stage effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAnswer")]
pub struct GateCommitAnswer {
    request: GateCommitRequest,
    resolved_seq: u64,
    response_hash: ContentHash,
    outcome: OutcomeKey,
}
#[derive(Deserialize)]
struct RawAnswer {
    request: GateCommitRequest,
    resolved_seq: u64,
    response_hash: ContentHash,
    outcome: OutcomeKey,
}
impl TryFrom<RawAnswer> for GateCommitAnswer {
    type Error = RecoveryIdentityError;
    fn try_from(raw: RawAnswer) -> Result<Self, Self::Error> {
        Self::new(
            raw.request,
            raw.resolved_seq,
            raw.response_hash,
            raw.outcome,
        )
    }
}
impl GateCommitAnswer {
    /// Bind the exact committed response and its chosen outcome to the request.
    pub fn new(
        request: GateCommitRequest,
        resolved_seq: u64,
        response_hash: ContentHash,
        outcome: OutcomeKey,
    ) -> Result<Self, RecoveryIdentityError> {
        if resolved_seq <= request.requested_seq() {
            return Err(RecoveryIdentityError::InvalidCommit);
        }
        Ok(Self {
            request,
            resolved_seq,
            response_hash,
            outcome,
        })
    }
    /// Original request and occurrence.
    pub fn request(&self) -> &GateCommitRequest {
        &self.request
    }
    /// Actual accepted response envelope sequence.
    pub fn resolved_seq(&self) -> u64 {
        self.resolved_seq
    }
    /// Hash of the complete accepted response, including feedback.
    pub fn response_hash(&self) -> &ContentHash {
        &self.response_hash
    }
    /// Chosen outcome; bootstrap failure audits do not imply successful routing.
    pub fn outcome(&self) -> &OutcomeKey {
        &self.outcome
    }
}

/// Stage result represented by the committed human decision effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateCommitDisposition {
    /// Route the accepted declared outcome.
    Route,
    /// Replay the existing typed bootstrap rejection without repeating its audit.
    BootstrapRejected,
    /// Replay the existing typed edit-limit escalation without another edit/audit.
    BootstrapEscalated,
}

/// Immutable marker for one atomic host-authored human-gate effect batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCommit")]
pub struct GateStageCommit {
    answer: GateCommitAnswer,
    disposition: GateCommitDisposition,
    effects_count: u32,
    effects_hash: ContentHash,
}
#[derive(Deserialize)]
struct RawCommit {
    answer: GateCommitAnswer,
    disposition: GateCommitDisposition,
    effects_count: u32,
    effects_hash: ContentHash,
}
impl TryFrom<RawCommit> for GateStageCommit {
    type Error = RecoveryIdentityError;
    fn try_from(raw: RawCommit) -> Result<Self, Self::Error> {
        Self::new(
            raw.answer,
            raw.disposition,
            raw.effects_count,
            raw.effects_hash,
        )
    }
}
impl GateStageCommit {
    /// Accept a bounded nonempty batch; journal inspection verifies its authority.
    pub fn new(
        answer: GateCommitAnswer,
        disposition: GateCommitDisposition,
        effects_count: u32,
        effects_hash: ContentHash,
    ) -> Result<Self, RecoveryIdentityError> {
        if !(1..=4096).contains(&effects_count) {
            return Err(RecoveryIdentityError::InvalidCommit);
        }
        Ok(Self {
            answer,
            disposition,
            effects_count,
            effects_hash,
        })
    }
    /// Original accepted answer and request occurrence.
    pub fn answer(&self) -> &GateCommitAnswer {
        &self.answer
    }
    /// Successful routing or a typed bootstrap failure audit.
    pub fn disposition(&self) -> GateCommitDisposition {
        self.disposition
    }
    /// Number of immediately preceding effects in the same transaction.
    pub fn effects_count(&self) -> u32 {
        self.effects_count
    }
    /// Domain-separated digest of their original versioned wrappers.
    pub fn effects_hash(&self) -> &ContentHash {
        &self.effects_hash
    }
}

/// Hash the original JSON response under the gate-response protocol domain.
pub fn gate_response_hash(response: &serde_json::Value) -> Result<ContentHash, serde_json::Error> {
    let mut bytes = b"surge-gate-response-v1\0".to_vec();
    bytes.extend(serde_json::to_vec(response)?);
    Ok(ContentHash::compute(&bytes))
}

/// Hash the ordered original versioned effects under a distinct protocol domain.
pub fn gate_effects_hash(
    effects: &[VersionedEventPayload],
) -> Result<ContentHash, serde_json::Error> {
    let mut bytes = b"surge-gate-effects-v1\0".to_vec();
    bytes.extend(serde_json::to_vec(&(effects.len(), effects))?);
    Ok(ContentHash::compute(&bytes))
}

/// Journal-derived gate acceptance and exact route consumption.
/// This record is replay state, not an externally supplied acceptance capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedGateStage {
    /// Host acceptance marker validated against the original decision.
    pub commit: GateStageCommit,
    /// Marker envelope sequence, assigned by the durable writer.
    pub committed_seq: u64,
    /// Matching route marker sequence, absent while effects await routing.
    pub routed_seq: Option<u64>,
    /// Conflicting evidence prevents this record authorizing recovery.
    pub conflicting: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_commit() -> GateStageCommit {
        let request =
            GateCommitRequest::new("gate".parse().unwrap(), GateRequestId::new(), 6, 7).unwrap();
        let response = serde_json::json!({"outcome":"approve", "comment":"original feedback"});
        let answer = GateCommitAnswer::new(
            request,
            8,
            gate_response_hash(&response).unwrap(),
            "approve".parse().unwrap(),
        )
        .unwrap();
        GateStageCommit::new(
            answer,
            GateCommitDisposition::Route,
            1,
            ContentHash::compute(b"effects"),
        )
        .unwrap()
    }

    #[test]
    fn serialized_identity_cannot_bypass_occurrence_or_batch_bounds() {
        let original = serde_json::to_value(valid_commit()).unwrap();
        for (pointer, value) in [
            (
                "/answer/request/request",
                serde_json::json!(ulid::Ulid::nil().to_string()),
            ),
            ("/answer/request/stage_entry_seq", serde_json::json!(0)),
            ("/answer/request/requested_seq", serde_json::json!(6)),
            ("/answer/resolved_seq", serde_json::json!(7)),
            ("/effects_count", serde_json::json!(0)),
            ("/effects_count", serde_json::json!(4097)),
        ] {
            let mut corrupt = original.clone();
            *corrupt.pointer_mut(pointer).unwrap() = value;
            assert!(
                serde_json::from_value::<GateStageCommit>(corrupt).is_err(),
                "{pointer}"
            );
        }
        assert!(serde_json::from_value::<GateStageCommit>(original).is_ok());
    }

    #[test]
    fn response_hash_includes_feedback_and_effects_preserve_original_schema() {
        assert_ne!(
            gate_response_hash(&serde_json::json!({"outcome":"edit","comment":"first"})).unwrap(),
            gate_response_hash(&serde_json::json!({"outcome":"edit","comment":"second"})).unwrap()
        );
        let old = VersionedEventPayload {
            schema_version: 12,
            payload: crate::EventPayload::OutcomeReported {
                node: "gate".parse().unwrap(),
                outcome: "approve".parse().unwrap(),
                summary: "approved".into(),
            },
        };
        let digest = gate_effects_hash(std::slice::from_ref(&old)).unwrap();
        assert_eq!(
            digest.to_string(),
            "sha256:24b5adb6c51bf4ee18e40d53a4fb25fb45c8d7ccec83328d87be53fb89ab3b97"
        );
        let mut changed = old;
        changed.schema_version = 13;
        assert_ne!(gate_effects_hash(&[changed]).unwrap(), digest);
    }
}
