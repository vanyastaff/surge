//! Informational manifest-refusal receipts. These never grant dispatch or cleanup authority.
use super::WorkItemBinding;
use crate::{
    ContentHash, RunId,
    id::{StageInvocationId, WorkItemId, WorkItemOperationId},
};
use serde::{Deserialize, Serialize};

/// Invalid receipt representation; submitted values are never included.
#[derive(Debug, thiserror::Error)]
#[error("invalid owned-flow wake refusal receipt")]
pub struct OwnedFlowWakeRefusalError;

/// The sole permanent category in this bounded recovery contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedFlowWakeRefusalReason {
    /// Structurally valid MCP projection contradicts independent accepted ownership.
    InputsAssociationMismatch,
}

/// Exact observed quota lineage, rechecked by the host before refusal commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedFlowWakeLineage {
    /// Original stage occurrence.
    pub invocation: StageInvocationId,
    /// Current observed control, including a validated capacity transfer.
    pub control_generation: u64,
    /// Original selected quota cycle.
    pub cycle_generation: u64,
    /// Original observed metadata revision, not a manufactured rebind.
    pub source_revision: u64,
    /// Exact immutable due-wake identity.
    pub wake_identity: String,
}

/// Immutable historical refusal. Deserializing it grants no host capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawReceipt")]
pub struct OwnedFlowWakeRefusalReceipt {
    schema_version: u32,
    run: RunId,
    operation: WorkItemOperationId,
    binding: WorkItemBinding,
    lineage: OwnedFlowWakeLineage,
    reason: OwnedFlowWakeRefusalReason,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReceipt {
    schema_version: u32,
    run: RunId,
    operation: WorkItemOperationId,
    binding: WorkItemBinding,
    lineage: OwnedFlowWakeLineage,
    reason: OwnedFlowWakeRefusalReason,
}
impl TryFrom<RawReceipt> for OwnedFlowWakeRefusalReceipt {
    type Error = OwnedFlowWakeRefusalError;
    fn try_from(raw: RawReceipt) -> Result<Self, Self::Error> {
        if raw.schema_version != 1 {
            return Err(OwnedFlowWakeRefusalError);
        }
        Self::new(raw.run, raw.operation, raw.binding, raw.lineage, raw.reason)
    }
}
impl OwnedFlowWakeRefusalReceipt {
    /// Validate bounded informational identity; the host validates all actual associations.
    /// # Errors
    /// Rejects nil identities, zero/oversized generations or invalid wake identities.
    pub fn new(
        run: RunId,
        operation: WorkItemOperationId,
        binding: WorkItemBinding,
        lineage: OwnedFlowWakeLineage,
        reason: OwnedFlowWakeRefusalReason,
    ) -> Result<Self, OwnedFlowWakeRefusalError> {
        let bounded = |value: u64| value > 0 && value <= i64::MAX as u64;
        if run == RunId::nil()
            || operation == WorkItemOperationId::nil()
            || binding.item == WorkItemId::nil()
            || lineage.invocation == StageInvocationId::nil()
            || ![
                binding.revision,
                binding.generation,
                lineage.control_generation,
                lineage.cycle_generation,
                lineage.source_revision,
            ]
            .into_iter()
            .all(bounded)
            || lineage.wake_identity.trim().is_empty()
            || lineage.wake_identity.len() > 256
            || lineage.wake_identity.chars().any(char::is_control)
        {
            return Err(OwnedFlowWakeRefusalError);
        }
        Ok(Self {
            schema_version: 1,
            run,
            operation,
            binding,
            lineage,
            reason,
        })
    }
    /// Original execution.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }
    /// Original accepted operation.
    #[must_use]
    pub fn operation(&self) -> WorkItemOperationId {
        self.operation
    }
    /// Immutable accepted reservation.
    #[must_use]
    pub fn binding(&self) -> &WorkItemBinding {
        &self.binding
    }
    /// Original observed quota lineage.
    #[must_use]
    pub fn lineage(&self) -> &OwnedFlowWakeLineage {
        &self.lineage
    }
    /// Fixed informational reason.
    #[must_use]
    pub fn reason(&self) -> OwnedFlowWakeRefusalReason {
        self.reason
    }
    /// Domain-separated identity of this receipt, never an authentication tag.
    /// # Errors
    /// Returns serialization failure without changing the receipt.
    pub fn hash(&self) -> Result<ContentHash, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        value.sort_all_objects();
        let mut bytes = b"surge.owned-flow-wake-refusal.v1\0".to_vec();
        bytes.extend(serde_json::to_vec(&value)?);
        Ok(ContentHash::compute(&bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt() -> OwnedFlowWakeRefusalReceipt {
        OwnedFlowWakeRefusalReceipt::new(
            RunId::new(),
            WorkItemOperationId::new(),
            WorkItemBinding {
                item: WorkItemId::new(),
                revision: 1,
                requirements_hash: ContentHash::compute(b"accepted"),
                generation: 1,
            },
            OwnedFlowWakeLineage {
                invocation: StageInvocationId::new(),
                control_generation: 2,
                cycle_generation: 1,
                source_revision: 3,
                wake_identity: "wake-original".into(),
            },
            OwnedFlowWakeRefusalReason::InputsAssociationMismatch,
        )
        .unwrap()
    }
    #[test]
    fn refuses_forged_schema_and_generations_without_echoing_values() {
        let original = receipt();
        let value = serde_json::to_value(&original).unwrap();
        assert_eq!(
            serde_json::from_value::<OwnedFlowWakeRefusalReceipt>(value.clone()).unwrap(),
            original
        );
        for path in ["schema_version", "binding", "lineage"] {
            let mut invalid = value.clone();
            match path {
                "schema_version" => invalid[path] = 9.into(),
                "binding" => invalid[path]["generation"] = 0.into(),
                _ => invalid[path]["control_generation"] = 0.into(),
            }
            assert!(serde_json::from_value::<OwnedFlowWakeRefusalReceipt>(invalid).is_err());
        }
        let raw = serde_json::to_vec(&original).unwrap();
        assert_ne!(original.hash().unwrap(), ContentHash::compute(&raw));
        let mut changed = original.clone();
        changed.lineage.wake_identity = "another-wake".into();
        assert_ne!(original.hash().unwrap(), changed.hash().unwrap());
    }
}
