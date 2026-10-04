//! Persistent work items own requirements and attempt history above run execution.
use crate::id::{WorkItemId, WorkItemOperationId, WorkItemProjectId};
use crate::{ContentHash, Graph, RunId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod flow_inputs;
mod wake_refusal;
pub use wake_refusal::{
    OwnedFlowWakeLineage, OwnedFlowWakeRefusalError, OwnedFlowWakeRefusalReason,
    OwnedFlowWakeRefusalReceipt,
};
mod origin;
pub use flow_inputs::{
    FrozenOwnedFlowMcp, HmacSha256Tag, OwnedFlowInputsManifest, OwnedFlowManifestError,
    OwnedFlowMcpManifestEntry, OwnedFlowMcpSelection, OwnedFlowObjectRef, OwnedFlowRequestIdentity,
};
pub use origin::{AcceptedFlowContract, AcceptedFlowError, AcceptedWorkItemOrigin, FlowOrigin};

/// Immutable accepted requirement text and criterion definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRequirements")]
pub struct WorkItemRequirements {
    text: String,
    criteria: Vec<String>,
}
#[derive(Deserialize)]
struct RawRequirements {
    text: String,
    criteria: Vec<String>,
}
impl TryFrom<RawRequirements> for WorkItemRequirements {
    type Error = String;
    fn try_from(raw: RawRequirements) -> Result<Self, Self::Error> {
        Self::new(raw.text, raw.criteria)
    }
}
impl WorkItemRequirements {
    /// Validate bounded, nonempty agreed requirements.
    pub fn new(text: String, criteria: Vec<String>) -> Result<Self, String> {
        if text.trim().is_empty()
            || text.len() > 131_072
            || criteria.is_empty()
            || criteria.len() > 256
            || criteria
                .iter()
                .any(|criterion| criterion.trim().is_empty() || criterion.len() > 8192)
        {
            return Err("requirements need bounded text and nonempty acceptance criteria".into());
        }
        Ok(Self { text, criteria })
    }
    /// Accepted specification text.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Accepted acceptance criteria, in stable order.
    pub fn criteria(&self) -> &[String] {
        &self.criteria
    }
    /// Immutable canonical content identity.
    pub fn hash(&self) -> Result<ContentHash, serde_json::Error> {
        serde_json::to_vec(self).map(|bytes| ContentHash::compute(&bytes))
    }
    /// Host context injected into every agent stage.
    pub fn prompt(&self, revision: u64, hash: ContentHash) -> String {
        let criteria = self
            .criteria
            .iter()
            .enumerate()
            .map(|(index, value)| format!("{}. {value}", index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Accepted task requirements (revision {revision}, hash {hash})\n{}\nAcceptance criteria:\n{criteria}",
            self.text
        )
    }
}
/// Host-pinned task association recorded atomically with a run startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemBinding {
    /// Persistent task identity, distinct from tracker and roadmap IDs.
    pub item: WorkItemId,
    /// Accepted immutable revision.
    pub revision: u64,
    /// Accepted requirement bytes identity.
    pub requirements_hash: ContentHash,
    /// Reservation generation.
    pub generation: u64,
}
/// Saved Git creation intent; never inferred from an existing arbitrary directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemWorkspace {
    /// Canonical common Git directory.
    pub repository: PathBuf,
    /// Original checkout used for provisioning.
    pub checkout: PathBuf,
    /// Canonical retained task workspace locator.
    pub path: PathBuf,
    /// Registered linked-worktree name.
    pub ownership: String,
    /// Retained branch name.
    pub branch: String,
    /// Pinned creation base commit.
    pub base_commit: String,
}
/// Work item lifecycle is independent of terminal legacy task cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemRecord {
    /// Task identity.
    pub id: WorkItemId,
    /// Immutable project identity.
    pub project: WorkItemProjectId,
    /// Human title.
    pub title: String,
    /// Accepted requirement revision.
    pub accepted_revision: u64,
    /// Optimistic mutation version.
    pub version: u64,
    /// Archive time. Archive never removes files.
    pub archived_at_ms: Option<i64>,
    /// Retained original creation intent.
    pub workspace: WorkItemWorkspace,
    /// Sole active run, when reserved or executing.
    pub active_run: Option<RunId>,
    /// Current reservation fence.
    pub generation: u64,
}
/// Immutable accepted history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "RawRevision")]
pub struct WorkItemRevision {
    /// Revision number.
    pub revision: u64,
    /// Exact accepted content.
    #[serde(flatten)]
    pub origin: AcceptedWorkItemOrigin,
    /// Content identity.
    pub hash: ContentHash,
    /// Acceptance provenance.
    pub actor: String,
    /// Discussion proposal explicitly accepted by this revision, if any.
    #[serde(default)]
    pub accepted_proposal: Option<u64>,
    /// Acceptance timestamp.
    pub accepted_at_ms: i64,
}
#[derive(Deserialize)]
struct RawRevision {
    revision: u64,
    #[serde(flatten)]
    origin: AcceptedWorkItemOrigin,
    hash: ContentHash,
    actor: String,
    #[serde(default)]
    accepted_proposal: Option<u64>,
    accepted_at_ms: i64,
}
impl TryFrom<RawRevision> for WorkItemRevision {
    type Error = String;
    fn try_from(raw: RawRevision) -> Result<Self, Self::Error> {
        if raw.revision == 0 || raw.origin.hash().map_err(|error| error.to_string())? != raw.hash {
            return Err("invalid accepted task revision".into());
        }
        Ok(Self {
            revision: raw.revision,
            origin: raw.origin,
            hash: raw.hash,
            actor: raw.actor,
            accepted_proposal: raw.accepted_proposal,
            accepted_at_ms: raw.accepted_at_ms,
        })
    }
}
/// Relation of a historical attempt to the item's currently accepted requirements.
/// This describes requirement ownership, not verification evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedRevisionRelation {
    /// Attempt uses the current accepted revision.
    MatchesCurrent,
    /// A newer accepted revision superseded this attempt.
    Superseded,
}
/// One run association and its durable reservation state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemAttempt {
    /// Persistent task.
    pub item: WorkItemId,
    /// Unique associated execution.
    pub run: RunId,
    /// Item-local attempt ordinal.
    pub ordinal: u64,
    /// Accepted context.
    pub binding: WorkItemBinding,
    /// Dynamic operator projection; historical proof never changes this relation.
    pub accepted_revision_relation: AcceptedRevisionRelation,
    /// Frozen graph; launch retries do not re-read mutable files.
    pub graph: Box<Graph>,
    /// Frozen host configuration JSON, validated by the engine service.
    pub config: String,
    /// Reservation lifecycle.
    pub state: WorkItemAttemptState,
    /// Diagnostic preserving uncertain execution.
    pub diagnostic: Option<String>,
    /// Last durable usage sequence observed.
    pub usage_seq: u64,
    /// Known input/output usage.
    pub input_tokens: u64,
    /// Known output tokens.
    pub output_tokens: u64,
    /// Known monetary cost, not a quota enforcement cap.
    pub known_cost_usd: f64,
    /// Whether a missing observation or unpriced usage remains.
    pub usage_unknown: bool,
}
/// Attempt state never substitutes a registry label for journal evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemAttemptState {
    Reserved,
    Launched,
    Attention,
    Suspended,
    Completed,
    Failed,
    Aborted,
    Rejected,
}
impl WorkItemAttemptState {
    /// Whether this attempt still owns dispatch rights.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Reserved | Self::Launched | Self::Attention | Self::Suspended
        )
    }
}
/// Single immutable PR association. No provider operation is implied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPr")]
pub struct WorkItemPr {
    pub provider: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
}
#[derive(Deserialize)]
struct RawPr {
    provider: String,
    repository: String,
    number: u64,
    url: String,
}
impl TryFrom<RawPr> for WorkItemPr {
    type Error = String;
    fn try_from(value: RawPr) -> Result<Self, String> {
        let pr = Self {
            provider: value.provider,
            repository: value.repository,
            number: value.number,
            url: value.url,
        };
        pr.validate()?;
        Ok(pr)
    }
}

impl WorkItemPr {
    /// Validate an explicit provider locator; URLs must agree with its identity.
    pub fn validate(&self) -> Result<(), String> {
        if self.provider != "github"
            || self.number == 0
            || self.repository.split('/').count() != 2
            || self.repository.split('/').any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            })
            || self.url
                != format!(
                    "https://github.com/{}/pull/{}",
                    self.repository, self.number
                )
        {
            return Err("PR association needs a normalized GitHub repository/number URL".into());
        }
        Ok(())
    }
}
/// Durable discussion and pending spec proposals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemDiscussion {
    pub sequence: u64,
    pub actor: String,
    pub body: String,
    pub proposal: Option<WorkItemRequirements>,
    pub created_at_ms: i64,
}
/// Indexed task totals; unknown is never presented as zero observed usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkItemUsage {
    pub runs: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub known_cost_usd: f64,
    pub unknown_runs: u64,
}
/// Read projection; histories are paginated separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemDetail {
    pub item: WorkItemRecord,
    pub revision: WorkItemRevision,
    pub pr: Option<WorkItemPr>,
    pub usage: WorkItemUsage,
    /// Latest durable control and its confirmed continuation state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<crate::execution_recovery::WorkItemExecutionControl>,
}
/// Bounded stable cursor page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemPage<T> {
    pub entries: Vec<T>,
    pub next_cursor: Option<String>,
}
/// Shared CLI/MCP/daemon control requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WorkItemCommand {
    Create {
        operation_id: WorkItemOperationId,
        project: PathBuf,
        title: String,
        requirements: WorkItemRequirements,
    },
    Show {
        item: WorkItemId,
    },
    List {
        after: Option<String>,
        limit: u32,
    },
    Revisions {
        item: WorkItemId,
        after: Option<String>,
        limit: u32,
    },
    Discussion {
        item: WorkItemId,
        after: Option<String>,
        limit: u32,
    },
    Attempts {
        item: WorkItemId,
        after: Option<String>,
        limit: u32,
    },
    Edit {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
        expected_revision: u64,
        requirements: WorkItemRequirements,
    },
    Discuss {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
        body: String,
        proposal: Option<WorkItemRequirements>,
    },
    AcceptProposal {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
        expected_revision: u64,
        proposal: u64,
    },
    Start {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
        graph: Box<Graph>,
        /// Optional host-validated frozen quota fallback policy. Kept as wire JSON
        /// here to avoid making the core domain depend on persistence storage types.
        #[serde(default)]
        quota_recovery: Option<serde_json::Value>,
    },
    Suspend {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
    },
    Continue {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
        #[serde(default)]
        new_session: bool,
    },
    Archive {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
    },
    AttachPr {
        operation_id: WorkItemOperationId,
        item: WorkItemId,
        expected_version: u64,
        pr: WorkItemPr,
    },
}
impl WorkItemCommand {
    /// Stable idempotency identity for every mutation.
    pub fn operation_id(&self) -> Option<WorkItemOperationId> {
        match self {
            Self::Create { operation_id, .. }
            | Self::Edit { operation_id, .. }
            | Self::Discuss { operation_id, .. }
            | Self::AcceptProposal { operation_id, .. }
            | Self::Start { operation_id, .. }
            | Self::Suspend { operation_id, .. }
            | Self::Continue { operation_id, .. }
            | Self::Archive { operation_id, .. }
            | Self::AttachPr { operation_id, .. } => Some(*operation_id),
            _ => None,
        }
    }
}

/// Shared task command response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WorkItemResult {
    Detail(Box<WorkItemDetail>),
    Items(WorkItemPage<WorkItemRecord>),
    Revisions(WorkItemPage<WorkItemRevision>),
    Discussion(WorkItemPage<WorkItemDiscussion>),
    Attempts(WorkItemPage<WorkItemAttempt>),
    Attempt(Box<WorkItemAttempt>),
    Control(Box<crate::execution_recovery::WorkItemExecutionControl>),
}
/// Host-pinned execution context, validated again when journals are decoded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawContext")]
pub struct WorkItemContext {
    binding: WorkItemBinding,
    #[serde(flatten)]
    origin: AcceptedWorkItemOrigin,
}
#[derive(Deserialize)]
struct RawContext {
    binding: WorkItemBinding,
    #[serde(flatten)]
    origin: AcceptedWorkItemOrigin,
}
impl TryFrom<RawContext> for WorkItemContext {
    type Error = String;
    fn try_from(raw: RawContext) -> Result<Self, Self::Error> {
        Self::from_origin(raw.binding, raw.origin)
    }
}
impl WorkItemContext {
    /// Construct a validated legacy specification context.
    ///
    /// # Errors
    /// Rejects invalid reservation identities or a content/hash mismatch.
    pub fn new(
        binding: WorkItemBinding,
        requirements: WorkItemRequirements,
    ) -> Result<Self, String> {
        Self::from_origin(binding, AcceptedWorkItemOrigin::Requirements(requirements))
    }
    /// Construct a flow context without synthesizing task requirements.
    ///
    /// # Errors
    /// Rejects invalid reservation identities or a content/hash mismatch.
    pub fn new_flow(
        binding: WorkItemBinding,
        contract: AcceptedFlowContract,
    ) -> Result<Self, String> {
        Self::from_origin(binding, AcceptedWorkItemOrigin::Flow(contract))
    }
    /// Construct a validated host context from its typed accepted origin.
    ///
    /// # Errors
    /// Rejects invalid reservation identities or a content/hash mismatch.
    pub fn from_origin(
        binding: WorkItemBinding,
        origin: AcceptedWorkItemOrigin,
    ) -> Result<Self, String> {
        if binding.item == WorkItemId::nil()
            || binding.revision == 0
            || binding.generation == 0
            || origin.hash().map_err(|error| error.to_string())? != binding.requirements_hash
        {
            return Err("invalid host task context".into());
        }
        Ok(Self { binding, origin })
    }
    /// Durable reservation identity.
    #[must_use]
    pub fn binding(&self) -> &WorkItemBinding {
        &self.binding
    }
    /// Exact typed accepted origin.
    #[must_use]
    pub fn origin(&self) -> &AcceptedWorkItemOrigin {
        &self.origin
    }
    /// Exact accepted legacy specification, absent for flows.
    #[must_use]
    pub fn requirements(&self) -> Option<&WorkItemRequirements> {
        self.origin.requirements()
    }
    /// Exact flow contract, absent for legacy specifications.
    #[must_use]
    pub fn flow(&self) -> Option<&AcceptedFlowContract> {
        self.origin.flow()
    }
    /// Accepted context shared by every agent stage.
    #[must_use]
    pub fn prompt(&self) -> String {
        match &self.origin {
            AcceptedWorkItemOrigin::Requirements(requirements) => {
                requirements.prompt(self.binding.revision, self.binding.requirements_hash)
            },
            AcceptedWorkItemOrigin::Flow(flow) => flow.raw_prompt().to_owned(),
        }
    }
}

/// Immutable acknowledgement of one atomically accepted ordinary Flow operation.
/// This projection contains no request body, transport inputs or secret-derived digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedFlowReceipt {
    /// Caller operation whose first body was accepted.
    pub operation_id: crate::id::WorkItemOperationId,
    /// Host-allocated persistent item.
    pub item: crate::id::WorkItemId,
    /// Host-allocated execution, retained across cold recovery.
    pub run: RunId,
    /// Immutable accepted origin/reservation association.
    pub binding: WorkItemBinding,
    /// Original Git workspace creation owner, independent of later attempts.
    pub workspace_owner: RunId,
    /// Atomic acceptance timestamp.
    pub accepted_at_ms: i64,
}

#[cfg(test)]
mod tests;
