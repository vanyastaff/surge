//! Structural continuation correlation; these values grant no execution authority.
use super::{FrozenOwnedFlowMcp, HmacSha256Tag, OwnedFlowManifestError};
use crate::{
    RunId,
    id::WorkItemOperationId,
    work_item::{WorkItemBinding, WorkItemWorkspace},
};
use serde::{Deserialize, Serialize};

/// Required private inputs associated with an existing Bootstrap operation and run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRequirement")]
pub struct ContinuationInputRequirement {
    source_bootstrap: RunId,
    source_run: RunId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequirement {
    source_bootstrap: RunId,
    source_run: RunId,
}
impl TryFrom<RawRequirement> for ContinuationInputRequirement {
    type Error = OwnedFlowManifestError;
    fn try_from(raw: RawRequirement) -> Result<Self, Self::Error> {
        Self::new(raw.source_bootstrap, raw.source_run)
    }
}
impl ContinuationInputRequirement {
    /// Validate correlation identifiers, without proving source eligibility.
    ///
    /// # Errors
    /// Rejects nil identifiers. Their semantic roles require independent storage.
    pub fn new(source_bootstrap: RunId, source_run: RunId) -> Result<Self, OwnedFlowManifestError> {
        if source_bootstrap == RunId::nil() || source_run == RunId::nil() {
            return Err(OwnedFlowManifestError::Identity);
        }
        Ok(Self {
            source_bootstrap,
            source_run,
        })
    }
    /// Bootstrap operation identity; this is not its planning run identity.
    #[must_use]
    pub fn source_bootstrap(&self) -> RunId {
        self.source_bootstrap
    }
    /// Original implementation run whose inputs are required.
    #[must_use]
    pub fn source_run(&self) -> RunId {
        self.source_run
    }
}

/// Public structural purpose; neither variant is an execution capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnedFlowInputPurpose {
    /// Ordinary startup inputs retain their schema1 compatibility contract.
    Ordinary,
    /// A seed is required independently of the selected MCP server count.
    Continuation {
        source: ContinuationInputRequirement,
    },
}
impl OwnedFlowInputPurpose {
    pub(super) fn is_ordinary(&self) -> bool {
        matches!(self, Self::Ordinary)
    }
}

/// Raw constructor fields, without deserialization or authenticated authority.
pub struct ContinuationInputsFields {
    /// Original host operation.
    pub operation: WorkItemOperationId,
    /// Host-created continuation request authentication tag.
    pub request_auth_tag: HmacSha256Tag,
    /// Exact reserved accepted requirements.
    pub binding: WorkItemBinding,
    /// New successor run.
    pub run: RunId,
    /// Original workspace creator, equal to the source implementation run.
    pub workspace_owner: RunId,
    /// Correlated workspace metadata, without adoption authority.
    pub workspace: WorkItemWorkspace,
    /// Original empty or nonempty resolved MCP selection.
    pub mcp: FrozenOwnedFlowMcp,
    /// Independently authenticated later by the host storage owner.
    pub source: ContinuationInputRequirement,
}
