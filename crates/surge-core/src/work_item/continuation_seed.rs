//! Public seed correlation metadata; this is never a hydrated execution capability.
use super::{
    WorkItemBinding,
    flow_inputs::{HmacSha256Tag, OwnedFlowObjectRef},
};
use crate::execution_recovery::semantic_position::{ContinuationSeedId, SourceEventRef};
use crate::{ContentHash, RunId, id::WorkItemOperationId};
use serde::{Deserialize, Serialize};

/// Structural fields used to construct a manifest. Authentication remains host-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationSeedFields {
    /// Manifest metadata format; currently one.
    pub schema_version: u32,
    /// The host continuation operation correlation identity.
    pub operation: WorkItemOperationId,
    /// The fresh successor run.
    pub run: RunId,
    /// The successor work-item revision and attempt generation.
    pub binding: WorkItemBinding,
    /// The original run retaining the registered workspace.
    pub workspace_owner: RunId,
    /// The retained workspace identity digest.
    pub workspace_identity: ContentHash,
    /// The original bootstrap operation.
    pub source_bootstrap: RunId,
    /// The inclusive semantic source event boundary.
    pub source: SourceEventRef,
    /// The exact source checkpoint digest.
    pub source_checkpoint: ContentHash,
    /// The raw registered successor graph identity.
    pub graph_hash: ContentHash,
    /// The semantic seed correlation digest.
    pub seed: ContinuationSeedId,
    /// Opaque reference to the separately protected private seed object.
    pub object_ref: OwnedFlowObjectRef,
    /// Authentication tag checked by the host, never by this metadata type.
    pub object_auth_tag: HmacSha256Tag,
}
/// Pure metadata identifying one required private seed. No paths or private body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ContinuationSeedFields", into = "ContinuationSeedFields")]
pub struct ContinuationSeedManifest(ContinuationSeedFields);
/// Invalid public correlation metadata, without echoing private values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid continuation seed manifest identity or version")]
pub struct ContinuationSeedManifestError;
impl TryFrom<ContinuationSeedFields> for ContinuationSeedManifest {
    type Error = ContinuationSeedManifestError;
    fn try_from(fields: ContinuationSeedFields) -> Result<Self, Self::Error> {
        if fields.schema_version != 1
            || fields.operation == WorkItemOperationId::nil()
            || fields.run == RunId::nil()
            || fields.source_bootstrap == RunId::nil()
            || fields.run == fields.source.run()
            || fields.run == fields.source_bootstrap
            || fields.workspace_owner != fields.source.run()
            || fields.binding.item == crate::id::WorkItemId::nil()
            || fields.binding.revision == 0
            || fields.binding.generation == 0
        {
            return Err(ContinuationSeedManifestError);
        }
        Ok(Self(fields))
    }
}
impl From<ContinuationSeedManifest> for ContinuationSeedFields {
    fn from(manifest: ContinuationSeedManifest) -> Self {
        manifest.0
    }
}
impl ContinuationSeedManifest {
    #[must_use]
    /// Inspect structurally validated metadata without acquiring a read capability.
    pub fn fields(&self) -> &ContinuationSeedFields {
        &self.0
    }
}
